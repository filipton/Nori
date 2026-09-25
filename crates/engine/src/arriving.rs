//! A song decoded as its bytes arrive, for whatever wants its samples before it is played (AutoMix's
//! measuring, `core::measure_as_it_comes`): whoever fetches the song (the fetching ahead, a download, the
//! loader of the next song) hands each piece to a [`Listening`] as it comes, and a decoder on a thread of the
//! lowest priority reads them from there. The network and the CPU wake together, in the fetch's one
//! burst, and the song is never read back from the disk and decoded a second time.
//!
//! What waits for the decoder is held to [`PIPE`] bytes: a fetch that may wait (fetching ahead, a
//! download) waits for room, one that must not (the player's own loader) gives the decoding up instead,
//! and the song is measured from the disk later. The decoder is woken once [`WAKE`] bytes wait, not per
//! piece. Only a song fetched whole from its first byte is kept as measured: one whose fetch broke off,
//! was left, or jumped is dropped, never stored as if it were complete.

use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use parking_lot::{Condvar, Mutex};
use symphonia::core::io::MediaSource;

/// What hears the samples a [`Listening`] decodes.
pub trait Heard: Send {
    fn samples(&mut self, rate: u32, channels: usize, samples: &[f32]);
    /// The decoding is over: `whole` when the song was decoded to its end and its fetch said all of it came.
    fn done(self: Box<Self>, whole: bool);
}

/// Bytes that may wait for the decoder.
pub const PIPE: usize = 2 << 20;
/// The decoder, waiting, is woken when this many bytes wait for it (or the song ended).
pub const WAKE: usize = 256 << 10;

#[derive(Default)]
struct State {
    bytes: VecDeque<u8>,
    /// The fetch said the song ended, and whether whole.
    ended: Option<bool>,
    /// The decoder is not reading any more: what comes is let go.
    quit: bool,
    /// A fetch that must not wait found no room: the decoding is given up.
    overflowed: bool,
    reader_waits: bool,
    writer_waits: bool,
}

#[derive(Default)]
struct Pipe {
    s: Mutex<State>,
    cv: Condvar,
}

/// A song being decoded as it comes: the fetch's side.
pub struct Listening {
    pipe: Arc<Pipe>,
    /// The fetch may wait for the decoder; otherwise the decoding is given up when it falls behind.
    wait: bool,
    ended: bool,
}

impl Listening {
    /// Starts decoding a song (`hint`: its container, as a file extension or MIME type) on a thread of
    /// its own, `heard` hearing its samples. `wait`: the fetch waits for the decoder when it is behind.
    pub fn start(hint: Option<String>, wait: bool, heard: Box<dyn Heard>) -> Option<Listening> {
        let pipe = Arc::new(Pipe::default());
        pipe.s.lock().bytes.reserve_exact(PIPE);
        let p = pipe.clone();
        std::thread::Builder::new().name("nori-measure".into()).spawn(move || decode(p, hint, heard)).ok()?;
        Some(Listening { pipe, wait, ended: false })
    }

    fn finish(&mut self, whole: bool) {
        if std::mem::replace(&mut self.ended, true) {
            return;
        }
        let mut s = self.pipe.s.lock();
        s.ended = Some(whole && !s.overflowed);
        self.pipe.cv.notify_all();
    }
}

impl Listening {
    /// The song's next bytes, in order from its first.
    pub fn take(&mut self, mut bytes: &[u8]) {
        let mut s = self.pipe.s.lock();
        while !bytes.is_empty() {
            if s.quit || s.overflowed {
                return;
            }
            let room = PIPE - s.bytes.len();
            if room == 0 {
                if !self.wait {
                    s.overflowed = true;
                    s.bytes.clear();
                    self.pipe.cv.notify_all();
                    return;
                }
                s.writer_waits = true;
                self.pipe.cv.wait(&mut s);
                continue;
            }
            let n = room.min(bytes.len());
            s.bytes.extend(&bytes[..n]);
            bytes = &bytes[n..];
            if s.reader_waits && s.bytes.len() >= WAKE {
                self.pipe.cv.notify_all();
            }
        }
    }

    /// The song's bytes ended: `whole` when every one of them came, in order, and was kept. Dropped
    /// without this, it is as `end(false)`.
    pub fn end(mut self, whole: bool) {
        self.finish(whole);
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.finish(false);
    }
}

/// The decoder's side: the bytes in order. The first [`HEAD`] of them are kept, for a container reader
/// that looks at the start and goes back to it (the tag before the music); past them it reads on only.
struct Reader {
    pipe: Arc<Pipe>,
    /// Where it reads, and how many bytes it has taken from the pipe.
    at: u64,
    taken: u64,
    head: Vec<u8>,
}

/// How many of a song's first bytes are kept for going back to.
const HEAD: usize = 64 << 10;

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.at < self.taken {
            // Gone back into the first bytes.
            let from = self.at as usize;
            let n = buf.len().min(self.head.len() - from);
            buf[..n].copy_from_slice(&self.head[from..from + n]);
            self.at += n as u64;
            return Ok(n);
        }
        let n = self.pull(buf)?;
        if (self.taken as usize) < HEAD {
            let keep = n.min(HEAD - self.taken as usize);
            self.head.extend_from_slice(&buf[..keep]);
        }
        self.taken += n as u64;
        self.at = self.taken;
        Ok(n)
    }
}

impl Reader {
    fn pull(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut s = self.pipe.s.lock();
        loop {
            if s.overflowed {
                return Err(io::Error::other("the fetch went on without the decoder"));
            }
            if !s.bytes.is_empty() {
                break;
            }
            if s.ended.is_some() {
                return Ok(0);
            }
            // Woken once a good piece waits, not for every one the fetch hands over.
            s.reader_waits = true;
            while s.bytes.len() < WAKE && s.ended.is_none() && !s.overflowed {
                self.pipe.cv.wait(&mut s);
            }
            s.reader_waits = false;
        }
        let (a, b) = s.bytes.as_slices();
        let n = buf.len().min(a.len() + b.len());
        let from_a = n.min(a.len());
        buf[..from_a].copy_from_slice(&a[..from_a]);
        buf[from_a..n].copy_from_slice(&b[..n - from_a]);
        s.bytes.drain(..n);
        if s.writer_waits && s.bytes.len() <= PIPE / 2 {
            s.writer_waits = false;
            self.pipe.cv.notify_all();
        }
        Ok(n)
    }
}

impl Seek for Reader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(d) => self.at.checked_add_signed(d).ok_or_else(|| io::Error::other("seek before the start"))?,
            SeekFrom::End(_) => return Err(io::Error::new(io::ErrorKind::Unsupported, "a song still coming has no end yet")),
        };
        // Where it has got to, or back into the first bytes it kept: nothing it would have to fetch.
        if p == self.taken || (p < self.taken && self.head.len() as u64 == self.taken) {
            self.at = p;
            return Ok(p);
        }
        Err(io::Error::new(io::ErrorKind::Unsupported, "a song still coming is read in order"))
    }
}

impl MediaSource for Reader {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn decode(pipe: Arc<Pipe>, hint: Option<String>, mut heard: Box<dyn Heard>) {
    lower_priority();
    let reader = Reader { pipe: pipe.clone(), at: 0, taken: 0, head: Vec::with_capacity(HEAD) };
    let decoded = crate::demux::decode_as_it_comes(Box::new(reader), hint.as_deref(), |rate, channels, samples| {
        heard.samples(rate, channels, samples);
        true
    });
    // Whatever still comes (the tags after the music) is let go, and the fetch's word waited for.
    let whole = {
        let mut s = pipe.s.lock();
        s.quit = true;
        s.bytes.clear();
        pipe.cv.notify_all();
        while s.ended.is_none() {
            pipe.cv.wait(&mut s);
        }
        s.ended == Some(true)
    };
    heard.done(whole && matches!(decoded, Ok(true)));
}

/// This thread below everything else, as the measuring of songs ahead has it.
pub(crate) fn lower_priority() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: a plain system call about the calling thread (on Linux, `0` with PRIO_PROCESS is the
    // thread itself).
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
}

/// CPU time this thread has used, ms; none where it cannot be read.
pub fn thread_cpu_ms() -> Option<u64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime writes the timespec it is given.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) } == 0 {
            return Some(t.tv_sec as u64 * 1000 + t.tv_nsec as u64 / 1_000_000);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Sender};

    /// Hears how many samples came and how it ended.
    struct Count(u64, Sender<(u64, bool)>);

    impl Heard for Count {
        fn samples(&mut self, _rate: u32, _channels: usize, samples: &[f32]) {
            self.0 += samples.len() as u64;
        }
        fn done(self: Box<Self>, whole: bool) {
            let _ = self.1.send((self.0, whole));
        }
    }

    fn wav(frames: usize) -> Vec<u8> {
        let data = (frames * 4) as u32;
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&44_100u32.to_le_bytes());
        w.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        w.extend_from_slice(&4u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        w.extend((0..frames * 2).flat_map(|i| ((i as i16).wrapping_mul(7)).to_le_bytes()));
        w
    }

    fn fed(bytes: &[u8], piece: usize, whole: bool, wait: bool) -> (u64, bool) {
        let (tx, rx) = channel();
        let mut l = Box::new(Listening::start(Some("wav".into()), wait, Box::new(Count(0, tx))).unwrap());
        for p in bytes.chunks(piece) {
            l.take(p);
        }
        l.end(whole);
        rx.recv().unwrap()
    }

    #[test]
    fn a_song_is_decoded_as_it_comes_and_kept_only_when_all_of_it_came() {
        let frames = 44_100 * 40;
        let song = wav(frames);
        assert!(song.len() > 3 * PIPE, "more than the pipe holds: the fetch waits for the decoder");
        assert_eq!(fed(&song, 64 << 10, true, true), (frames as u64 * 2, true), "every sample, and whole");
        let (_, whole) = fed(&song[..song.len() / 2], 64 << 10, false, true);
        assert!(!whole, "a fetch that broke off is not a measured song");
        // Dropped half way (a fetch left for another), it ends as not whole.
        let (tx, rx) = channel();
        let mut l = Listening::start(Some("wav".into()), true, Box::new(Count(0, tx))).unwrap();
        l.take(&song[..PIPE / 2]);
        drop(l);
        assert!(!rx.recv().unwrap().1);
    }

    #[test]
    fn a_fetch_that_must_not_wait_gives_the_decoding_up_rather_than_waiting() {
        let song = wav(44_100 * 20);
        let (tx, rx) = channel();
        let mut l = Box::new(Listening::start(Some("wav".into()), false, Box::new(Count(0, tx))).unwrap());
        // Handed over far faster than a decoder takes it: the pipe fills, and the fetch goes on.
        let started = std::time::Instant::now();
        l.take(&song);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "never waited on the decoder");
        l.end(true);
        assert!(!rx.recv().unwrap().1, "given up: not kept as measured");
    }
}
