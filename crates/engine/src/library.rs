//! Where songs come from: a client says, per song id, whether it is a file or a URL (and through which
//! [`ByteSource`]), and for a URL whether it goes through the stream cache on disk; the engine opens it
//! (from the cache when all of it is there), keeps its bytes while it plays, and starts fetching the
//! next song in the same burst as the one before so the network wakes once for both.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::Thread;

use nori_player::pcm::Encoding;
use nori_player::pipeline::Songs;
use nori_player::transitions::WindowSong;

use crate::demux::Demuxed;
use crate::source::{ByteSource, Loader};
use crate::store::Store;

/// Where one song's bytes are.
#[derive(Clone)]
pub enum Source {
    File(PathBuf),
    Url { url: String, bytes: Arc<dyn ByteSource> },
    /// A URL whose bytes the stream cache keeps under `key`: read from the disk when the cache has all
    /// of it, and written into it as it loads when not.
    Cached { url: String, bytes: Arc<dyn ByteSource>, store: Arc<Store>, key: String },
}

/// A song ready to be opened: where it is, a hint at its container (a file extension such as "mp3",
/// or a MIME type), and its length as tagged, for a container that does not say.
#[derive(Clone)]
pub struct Located {
    pub source: Source,
    pub hint: Option<String>,
    pub duration_ms: Option<i64>,
}

/// The client's side of the songs: where each id is, and what is known of it.
pub trait Library: Send + 'static {
    fn locate(&mut self, id: &str) -> Result<Located, String>;
    /// What the transition planner and the seek bar know of `id`: its length, album and number.
    fn about(&self, id: &str) -> WindowSong;
    /// Whether `id` may be fetched before anyone asked to hear it. A provider's song (an `ext-` id on
    /// octo-fiesta) must not be: asking for it makes the server download it.
    fn fetch_ahead(&self, _id: &str) -> bool {
        true
    }
}

/// How many songs' bytes are kept at once: the one playing, the one after, and the one before.
const KEPT: usize = 3;

/// The engine's [`Songs`]: the library's songs opened, their loaders kept while they may be needed.
pub struct Sources<L: Library> {
    pub library: L,
    /// What songs are decoded to: float for high quality output, 16-bit otherwise.
    pub encoding: Encoding,
    load: [i64; 5],
    engine: Thread,
    loaders: Vec<(String, Arc<Loader>)>,
}

impl<L: Library> Sources<L> {
    /// `load` is `nori_player::transport::load_control`'s answer; `engine` the thread woken when
    /// bytes a song was waiting for arrive.
    pub fn new(library: L, load: [i64; 5], engine: Thread) -> Sources<L> {
        Sources { library, encoding: Encoding::Pcm16, load, engine, loaders: Vec::new() }
    }

    /// The loader of `id`, started if it is not running (writing into `keep`'s cache entry); the most
    /// recently used is kept last.
    fn loader(&mut self, id: &str, url: &str, bytes: &Arc<dyn ByteSource>, duration_ms: Option<i64>, keep: Option<(&Arc<Store>, &str)>) -> Arc<Loader> {
        // One that gave up is not asked again: the song is fetched anew, the network may be back.
        self.loaders.retain(|(i, l)| i != id || l.error().is_none());
        if let Some(k) = self.loaders.iter().position(|(i, _)| i == id) {
            let l = self.loaders.remove(k);
            let loader = l.1.clone();
            self.loaders.push(l);
            return loader;
        }
        let writer = keep.and_then(|(store, key)| store.writer(key));
        let loader = Loader::start(bytes.clone(), url.to_string(), self.load, duration_ms, writer);
        self.loaders.push((id.to_string(), loader.clone()));
        if self.loaders.len() > KEPT {
            self.loaders.remove(0);
        }
        loader
    }

    /// Every song's bytes go (a long pause): they are fetched, or read from the disk, again when needed.
    pub fn let_go(&mut self) {
        self.loaders.clear();
    }

    /// The loader of `id`, if its bytes are being kept.
    pub fn loading(&self, id: &str) -> Option<&Arc<Loader>> {
        self.loaders.iter().find(|(i, _)| i == id).map(|(_, l)| l)
    }
}

impl<L: Library> Songs for Sources<L> {
    type Reading = Demuxed;

    fn open(&mut self, id: &str, from_ms: i64) -> Result<Demuxed, String> {
        let at = self.library.locate(id)?;
        let file = |path: &PathBuf, encoding| {
            let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let hint = at.hint.clone().or_else(|| path.extension().map(|e| e.to_string_lossy().into_owned()));
            Demuxed::open(Box::new(file), hint.as_deref(), from_ms, at.duration_ms, encoding)
        };
        match &at.source {
            Source::File(path) => file(path, self.encoding),
            Source::Cached { store, key, .. } if self.loading(id).is_none() && store.cached(key).is_some() => {
                let path = store.cached(key).expect("checked");
                file(&path, self.encoding)
            }
            Source::Url { url, bytes } => {
                let loader = self.loader(id, url, bytes, at.duration_ms, None);
                Ok(Demuxed::load(loader, self.engine.clone(), at.hint.as_deref(), from_ms, at.duration_ms, self.encoding))
            }
            Source::Cached { url, bytes, store, key } => {
                let loader = self.loader(id, url, bytes, at.duration_ms, Some((store, key)));
                Ok(Demuxed::load(loader, self.engine.clone(), at.hint.as_deref(), from_ms, at.duration_ms, self.encoding))
            }
        }
    }

    fn about(&self, id: &str) -> WindowSong {
        self.library.about(id)
    }

    fn upcoming(&mut self, id: &str) {
        if !self.library.fetch_ahead(id) {
            return;
        }
        match self.library.locate(id) {
            Ok(Located { source: Source::Url { url, bytes }, duration_ms, .. }) => {
                self.loader(id, &url, &bytes, duration_ms, None);
            }
            Ok(Located { source: Source::Cached { url, bytes, store, key }, duration_ms, .. }) if store.cached(&key).is_none() => {
                self.loader(id, &url, &bytes, duration_ms, Some((&store, &key)));
            }
            _ => {}
        }
    }
}
