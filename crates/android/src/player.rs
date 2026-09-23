//! The Rust playback path: nori-engine playing the core's queue (`CoreQueue`, `CoreApp`) into an
//! AudioTrack (track.rs), selected by the "Playback engine" setting instead of ExoPlayer. Kotlin's
//! `EnginePlayer` is a media3 player over these doors, so the session, the notification and the screens
//! follow it as they follow ExoPlayer.
//!
//! What only the platform has comes from Kotlin through a few calls into `RustBridge`, each made rarely:
//! - the AudioTrack, opened by Kotlin (`openTrack`: the attributes, the DAC's preferred device and
//!   mixer attributes, the route listener), then written and driven from Rust;
//! - a song's bytes (`open`, then `read` per 64 KB): through media3's data sources on the app's one
//!   OkHttp client, so the TLS settings, client certificates and headers of the profile apply, and the
//!   downloads and the stream cache are the ones the ExoPlayer path uses - a song downloaded or cached
//!   by either plays from the disk on both, and the precacher fills them for both;
//! - the cache key a song resolves to (`key`), for the container it names;
//! - a wake for the events (`signal`): one call per batch of engine events, however many there are,
//!   and Kotlin takes them from here on its own thread.
//!
//! Every class and method is looked up once, in `create`, on a thread that sees the app's classes; the
//! threads that call them (the engine's, the track's, the loaders') are attached for their whole life.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use jni::objects::{GlobalRef, JByteArray, JClass, JFieldID, JMethodID, JStaticMethodID, JString, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};
use nori_engine::core::{key_format, settings, CoreApp, CoreQueue};
use nori_engine::{Body, ByteSource, Config, Device, Engine, Event, Library, Located, OutputFormat, Source, State};
use nori_player::transitions::WindowSong;
use parking_lot::Mutex;

use crate::track::{mono_ns, Opened, Opener, Shared, Sink, TrackOutput, CHUNK_BYTES};
use crate::{java_string, native, with_str, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/RustPlayerJni",
    methods: &[
        native!(c"create", c"(IZI)J", create),
        native!(c"destroy", c"(J)V", destroy),
        native!(c"playAt", c"(JIJ)V", play_at),
        native!(c"play", c"(J)V", play),
        native!(c"pause", c"(J)V", pause),
        native!(c"queueChanged", c"(J)V", queue_changed),
        native!(c"setRepeat", c"(JI)V", set_repeat),
        native!(c"replan", c"(J)V", replan),
        native!(c"gainChanged", c"(J)V", gain_changed),
        native!(c"applySettings", c"(J)V", apply_settings),
        native!(c"positionMs", c"(J)J", position_ms),
        native!(c"mixing", c"(J)Z", mixing),
        native!(c"bytesWritten", c"(J)J", bytes_written),
        native!(c"event", c"(J)J", event),
        native!(c"eventText", c"(J)Ljava/lang/String;", event_text),
        native!(c"device", c"(JILjava/lang/String;)V", device),
    ],
};

/// A song's bytes are asked for by this address; Kotlin's data source resolves it when it opens, so the
/// quality follows the network the phone is on right then, as on the ExoPlayer path.
const SONG_URL: &str = "nori://song/";

/// The Java side, looked up once.
struct Java {
    vm: JavaVM,
    bridge: GlobalRef,
    open_track: JStaticMethodID,
    open: JStaticMethodID,
    key: JStaticMethodID,
    signal: JStaticMethodID,
    body_read: JMethodID,
    body_close: JMethodID,
    body_buffer: JFieldID,
    body_length: JFieldID,
    position: JMethodID,
    track: TrackMethods,
    timestamp: GlobalRef,
    timestamp_new: JMethodID,
    frame_position: JFieldID,
    nano_time: JFieldID,
}

struct TrackMethods {
    write: JMethodID,
    play: JMethodID,
    pause: JMethodID,
    flush: JMethodID,
    stop: JMethodID,
    set_volume: JMethodID,
    get_timestamp: JMethodID,
    head: JMethodID,
    release: JMethodID,
    buffer_frames: JMethodID,
}

static JAVA: OnceLock<Java> = OnceLock::new();

fn look_up(env: &mut JNIEnv) -> jni::errors::Result<Java> {
    let bridge = env.find_class("dev/nori/music/playback/RustBridge")?;
    let body = env.find_class("dev/nori/music/playback/RustBody")?;
    let track = env.find_class("android/media/AudioTrack")?;
    let buffer = env.find_class("java/nio/Buffer")?;
    let timestamp = env.find_class("android/media/AudioTimestamp")?;
    Ok(Java {
        vm: env.get_java_vm()?,
        open_track: env.get_static_method_id(&bridge, "openTrack", "(IIZI)Landroid/media/AudioTrack;")?,
        open: env.get_static_method_id(&bridge, "open", "(Ljava/lang/String;J)Ldev/nori/music/playback/RustBody;")?,
        key: env.get_static_method_id(&bridge, "key", "(Ljava/lang/String;)Ljava/lang/String;")?,
        signal: env.get_static_method_id(&bridge, "signal", "()V")?,
        bridge: env.new_global_ref(&bridge)?,
        body_read: env.get_method_id(&body, "read", "(I)I")?,
        body_close: env.get_method_id(&body, "close", "()V")?,
        body_buffer: env.get_field_id(&body, "buffer", "[B")?,
        body_length: env.get_field_id(&body, "length", "J")?,
        position: env.get_method_id(&buffer, "position", "(I)Ljava/nio/Buffer;")?,
        track: TrackMethods {
            write: env.get_method_id(&track, "write", "(Ljava/nio/ByteBuffer;II)I")?,
            play: env.get_method_id(&track, "play", "()V")?,
            pause: env.get_method_id(&track, "pause", "()V")?,
            flush: env.get_method_id(&track, "flush", "()V")?,
            stop: env.get_method_id(&track, "stop", "()V")?,
            set_volume: env.get_method_id(&track, "setVolume", "(F)I")?,
            get_timestamp: env.get_method_id(&track, "getTimestamp", "(Landroid/media/AudioTimestamp;)Z")?,
            head: env.get_method_id(&track, "getPlaybackHeadPosition", "()I")?,
            release: env.get_method_id(&track, "release", "()V")?,
            buffer_frames: env.get_method_id(&track, "getBufferSizeInFrames", "()I")?,
        },
        timestamp_new: env.get_method_id(&timestamp, "<init>", "()V")?,
        frame_position: env.get_field_id(&timestamp, "framePosition", "J")?,
        nano_time: env.get_field_id(&timestamp, "nanoTime", "J")?,
        timestamp: env.new_global_ref(&timestamp)?,
    })
}

/// This thread's JNIEnv, attached for the rest of its life (it is detached when it ends).
fn env() -> Option<(&'static Java, JNIEnv<'static>)> {
    let java = JAVA.get()?;
    let env = java.vm.attach_current_thread_permanently().ok()?;
    Some((java, env))
}

/// One line in the app's log.
fn log(message: &str) {
    nori_core::alog::info(&format!("rust player: {message}"));
}

/// Whatever Java threw is written to the log and cleared: a native thread has nobody to throw it to.
fn cleared(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

fn bridge(java: &Java) -> &JClass<'static> {
    <&JClass>::from(java.bridge.as_obj())
}

// ---- the AudioTrack ----

/// An AudioTrack Kotlin opened, written from the track's thread through a direct buffer over memory
/// kept for its whole life.
struct JavaTrack {
    track: GlobalRef,
    buffer: GlobalRef,
    staging: Vec<f32>,
    timestamp: GlobalRef,
    /// The error the last write returned, so that a track that keeps refusing is logged once.
    failing: i32,
}

impl JavaTrack {
    fn void(&mut self, m: impl FnOnce(&TrackMethods) -> JMethodID) {
        let Some((java, mut env)) = env() else { return };
        // SAFETY: the method was looked up on AudioTrack with this signature, and takes no arguments.
        let _ = unsafe { env.call_method_unchecked(&self.track, m(&java.track), ReturnType::Primitive(Primitive::Void), &[]) };
        cleared(&mut env);
    }
}

impl Sink for JavaTrack {
    fn staging(&mut self) -> &mut [f32] {
        &mut self.staging
    }

    fn write(&mut self, from: usize, len: usize) -> usize {
        let Some((java, mut env)) = env() else { return 0 };
        let (Ok(from), Ok(len)) = (i32::try_from(from), i32::try_from(len)) else { return 0 };
        // SAFETY: Buffer.position(int) and AudioTrack.write(ByteBuffer, int, int), looked up with these
        // signatures; the buffer is the direct one over `staging`, whose range the writer keeps inside it.
        let taken = unsafe {
            if let Ok(b) = env.call_method_unchecked(&self.buffer, java.position, ReturnType::Object, &[JValue::Int(from).as_jni()]) {
                if let Ok(b) = b.l() {
                    let _ = env.delete_local_ref(b);
                }
            }
            let args = [JValue::Object(self.buffer.as_obj()).as_jni(), JValue::Int(len).as_jni(), JValue::Int(WRITE_NON_BLOCKING).as_jni()];
            env.call_method_unchecked(&self.track, java.track.write, ReturnType::Primitive(Primitive::Int), &args).and_then(|v| v.i())
        };
        cleared(&mut env);
        let taken = taken.unwrap_or(ERROR_DEAD_OBJECT);
        if taken < 0 && taken != self.failing {
            log(&format!("the AudioTrack refused a write: {taken}"));
        } else if taken >= 0 && self.failing < 0 {
            log("the AudioTrack takes writes again");
        }
        self.failing = taken.min(0);
        taken.max(0) as usize
    }

    fn play(&mut self) {
        self.void(|t| t.play);
    }

    fn pause(&mut self) {
        self.void(|t| t.pause);
    }

    fn flush(&mut self) {
        self.void(|t| t.flush);
    }

    fn stop(&mut self) {
        self.void(|t| t.stop);
    }

    fn set_volume(&mut self, volume: f32) {
        let Some((java, mut env)) = env() else { return };
        // SAFETY: AudioTrack.setVolume(float), looked up with this signature.
        let _ = unsafe { env.call_method_unchecked(&self.track, java.track.set_volume, ReturnType::Primitive(Primitive::Int), &[JValue::Float(volume).as_jni()]) };
        cleared(&mut env);
    }

    /// While playing, the frame the device presented and when (the AudioTimestamp); otherwise, or
    /// before it has one, the frame the track has handed on.
    fn heard(&mut self, playing: bool) -> Option<(u64, i64)> {
        let (java, mut env) = env()?;
        if playing {
            // SAFETY: AudioTrack.getTimestamp(AudioTimestamp) and the timestamp's two long fields, looked
            // up with these signatures.
            let stamped = unsafe {
                env.call_method_unchecked(&self.track, java.track.get_timestamp, ReturnType::Primitive(Primitive::Boolean), &[JValue::Object(self.timestamp.as_obj()).as_jni()])
                    .and_then(|v| v.z())
                    .unwrap_or(false)
            };
            cleared(&mut env);
            if stamped {
                let long = |env: &mut JNIEnv, f| env.get_field_unchecked(&self.timestamp, f, ReturnType::Primitive(Primitive::Long)).and_then(|v| v.j());
                if let (Ok(frames), Ok(ns)) = (long(&mut env, java.frame_position), long(&mut env, java.nano_time)) {
                    return Some((frames.max(0) as u64, ns));
                }
                cleared(&mut env);
            }
        }
        // SAFETY: AudioTrack.getPlaybackHeadPosition(), looked up with this signature.
        let head = unsafe { env.call_method_unchecked(&self.track, java.track.head, ReturnType::Primitive(Primitive::Int), &[]).and_then(|v| v.i()) };
        cleared(&mut env);
        // An unsigned count that wraps, as the platform documents it.
        head.ok().map(|h| (h as u32 as u64, mono_ns()))
    }

    fn release(&mut self) {
        self.void(|t| t.release);
    }
}

/// `AudioTrack.WRITE_NON_BLOCKING`.
const WRITE_NON_BLOCKING: i32 = 1;
/// `AudioTrack.ERROR_DEAD_OBJECT`: what a write that threw is counted as.
const ERROR_DEAD_OBJECT: i32 = -6;

/// Opens AudioTracks through Kotlin's `RustBridge.openTrack`.
struct JavaOpener {
    /// Android's API level: before 31 a track has no start threshold and starts only once full.
    sdk: i32,
}

impl Opener for JavaOpener {
    fn open(&mut self, format: OutputFormat, float: bool, frames: u64) -> Result<Opened, String> {
        let (java, mut env) = env().ok_or("no JVM")?;
        let opened = env.with_local_frame(8, |env| -> jni::errors::Result<Result<Opened, String>> {
            let args = [
                JValue::Int(format.rate as i32).as_jni(),
                JValue::Int(format.channels as i32).as_jni(),
                JValue::Bool(float as u8).as_jni(),
                JValue::Int(frames.min(i32::MAX as u64) as i32).as_jni(),
            ];
            // SAFETY: RustBridge.openTrack(int, int, boolean, int), looked up with this signature.
            let track = unsafe { env.call_static_method_unchecked(bridge(java), java.open_track, ReturnType::Object, &args) }.and_then(|v| v.l());
            let track = match track {
                Ok(t) if !t.is_null() => t,
                _ => {
                    cleared(env);
                    return Ok(Err("the AudioTrack would not open".into()));
                }
            };
            // SAFETY: AudioTrack.getBufferSizeInFrames(), looked up with this signature.
            let frames = unsafe { env.call_method_unchecked(&track, java.track.buffer_frames, ReturnType::Primitive(Primitive::Int), &[]) }.and_then(|v| v.i()).unwrap_or(0).max(0) as u64;
            cleared(env);
            let mut staging = vec![0f32; CHUNK_BYTES / 4];
            // SAFETY: the memory is `staging`'s, which is kept, never resized, beside the buffer for as
            // long as the buffer lives.
            let buffer = unsafe { env.new_direct_byte_buffer(staging.as_mut_ptr() as *mut u8, CHUNK_BYTES) }?;
            let timestamp_class = <&JClass>::from(java.timestamp.as_obj());
            // SAFETY: AudioTimestamp's no-argument constructor.
            let timestamp = unsafe { env.new_object_unchecked(timestamp_class, java.timestamp_new, &[]) }?;
            let sink = JavaTrack {
                track: env.new_global_ref(&track)?,
                buffer: env.new_global_ref(&buffer)?,
                staging,
                timestamp: env.new_global_ref(&timestamp)?,
                failing: 0,
            };
            Ok(Ok(Opened { sink: Box::new(sink), frames, starts_full: self.sdk < 31 }))
        });
        cleared(&mut env);
        opened.map_err(|e| e.to_string())?
    }
}

// ---- the songs' bytes ----

/// A song's bytes through Kotlin's data sources.
struct JavaBytes;

impl ByteSource for JavaBytes {
    fn open(&self, url: &str, from: u64) -> Result<Body, String> {
        let (java, mut env) = env().ok_or("no JVM")?;
        let body = env.with_local_frame(4, |env| -> jni::errors::Result<Option<(GlobalRef, i64)>> {
            let url = env.new_string(url)?;
            // SAFETY: RustBridge.open(String, long), looked up with this signature.
            let body = unsafe { env.call_static_method_unchecked(bridge(java), java.open, ReturnType::Object, &[JValue::Object(&url).as_jni(), JValue::Long(from as i64).as_jni()]) }?.l()?;
            if body.is_null() {
                return Ok(None);
            }
            let length = env.get_field_unchecked(&body, java.body_length, ReturnType::Primitive(Primitive::Long))?.j()?;
            Ok(Some((env.new_global_ref(&body)?, length)))
        });
        cleared(&mut env);
        match body {
            Ok(Some((body, length))) => {
                log(&format!("{url} from byte {from}: {} bytes come", if length >= 0 { length.to_string() } else { "unknown".into() }));
                Ok(Body { start: from, len: (length >= 0).then(|| from + length as u64), reader: Box::new(JavaBody { body, open: true }) })
            }
            _ => Err("the song's bytes would not come".into()),
        }
    }
}

struct JavaBody {
    body: GlobalRef,
    open: bool,
}

impl Read for JavaBody {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let (java, mut env) = env().ok_or_else(|| io::Error::other("no JVM"))?;
        let max = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: RustBody.read(int), looked up with this signature.
        let n = unsafe { env.call_method_unchecked(&self.body, java.body_read, ReturnType::Primitive(Primitive::Int), &[JValue::Int(max).as_jni()]) }.and_then(|v| v.i());
        cleared(&mut env);
        let n = match n {
            Ok(-1) => return Ok(0),
            Ok(n) if n > 0 => (n as usize).min(buf.len()),
            _ => return Err(io::Error::other("the song's bytes stopped coming")),
        };
        let copied = env.with_local_frame(2, |env| -> jni::errors::Result<()> {
            let array = JByteArray::from(env.get_field_unchecked(&self.body, java.body_buffer, ReturnType::Array)?.l()?);
            // SAFETY: i8 and u8 have the same size and alignment, and the slice is the caller's.
            let into = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut i8, n) };
            env.get_byte_array_region(&array, 0, into)
        });
        cleared(&mut env);
        copied.map(|_| n).map_err(|e| io::Error::other(e.to_string()))
    }
}

impl Drop for JavaBody {
    fn drop(&mut self) {
        if !std::mem::take(&mut self.open) {
            return;
        }
        if let Some((java, mut env)) = env() {
            // SAFETY: RustBody.close(), looked up with this signature.
            let _ = unsafe { env.call_method_unchecked(&self.body, java.body_close, ReturnType::Primitive(Primitive::Void), &[]) };
            cleared(&mut env);
        }
    }
}

// ---- where songs are ----

/// Every song is streamed through [`JavaBytes`] by its `nori://song/` address; the data source behind
/// it reads a download or the stream cache first. Radio streams stay on the ExoPlayer path.
struct AndroidLibrary {
    bytes: Arc<dyn ByteSource>,
}

impl Library for AndroidLibrary {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        if id.starts_with("radio:") {
            return Err("internet radio plays on the ExoPlayer engine".into());
        }
        let song = nori_core::queue::queue_song(id.to_string());
        let duration_ms = song.as_ref().map(|s| s.duration as i64 * 1000).filter(|&d| d > 0);
        // The container the resolved copy is in: a transcoded stream says it in its cache key. A download
        // may have been transcoded too, and its key (`dl:<id>`) says nothing: the file says what it is,
        // as on the desktop.
        let key = cache_key(id);
        let hint = match key.as_deref() {
            Some(k) if k == nori_core::stream::download_key(id.to_string()) => None,
            k => k.and_then(key_format).or_else(|| song.map(|s| s.suffix)).filter(|s| !s.is_empty()),
        };
        log(&format!("{id} opens from {} as {}", key.as_deref().unwrap_or("nowhere"), hint.as_deref().unwrap_or("whatever it is")));
        Ok(Located { source: Source::Url { url: format!("{SONG_URL}{id}"), bytes: self.bytes.clone() }, hint, duration_ms })
    }

    fn about(&self, id: &str) -> WindowSong {
        nori_engine::core::about(id)
    }

    fn fetch_ahead(&self, id: &str) -> bool {
        nori_engine::core::fetch_ahead(id)
    }
}

/// The cache key the song resolves to now (a download's, or the stream's at this network's quality).
fn cache_key(id: &str) -> Option<String> {
    let (java, mut env) = env()?;
    let key = env.with_local_frame(4, |env| -> jni::errors::Result<Option<String>> {
        let id = env.new_string(id)?;
        // SAFETY: RustBridge.key(String), looked up with this signature.
        let key = unsafe { env.call_static_method_unchecked(bridge(java), java.key, ReturnType::Object, &[JValue::Object(&id).as_jni()]) }?.l()?;
        if key.is_null() {
            return Ok(None);
        }
        let key = JString::from(key);
        let s: String = env.get_string(&key)?.into();
        Ok(Some(s))
    });
    cleared(&mut env);
    key.ok().flatten()
}

// ---- the player ----

/// The engine's events, kept until Kotlin takes them: one call into Kotlin per batch.
#[derive(Default)]
struct Events {
    queue: Mutex<VecDeque<(i32, i32, String)>>,
    signalled: AtomicBool,
    text: Mutex<String>,
}

const EVENT_STATE: i32 = 0;
const EVENT_SONG: i32 = 1;
const EVENT_ERROR: i32 = 2;
const EVENT_OUTPUT: i32 = 3;

impl Events {
    fn push(&self, e: Event) {
        match &e {
            Event::State(s) => log(&format!("{s:?}")),
            Event::Song { index, id } => log(&format!("song {index} ({id}) is heard")),
            Event::Output { name } => log(&format!("playing to {name}")),
            _ => {}
        }
        let e = match e {
            Event::State(s) => (EVENT_STATE, state_code(s), String::new()),
            Event::Song { index, id } => (EVENT_SONG, index as i32, id),
            Event::Error { id, message } => (EVENT_ERROR, -1, if id.is_empty() { message } else { format!("{id}: {message}") }),
            Event::Output { name } => (EVENT_OUTPUT, -1, name),
            Event::Position { .. } => return,
        };
        let first = {
            let mut q = self.queue.lock();
            q.push_back(e);
            !self.signalled.swap(true, Ordering::AcqRel)
        };
        if first {
            if let Some((java, mut env)) = env() {
                // SAFETY: RustBridge.signal(), looked up with this signature.
                let _ = unsafe { env.call_static_method_unchecked(bridge(java), java.signal, ReturnType::Primitive(Primitive::Void), &[]) };
                cleared(&mut env);
            }
        }
    }
}

fn state_code(s: State) -> i32 {
    match s {
        State::Idle => 0,
        State::Playing => 1,
        State::Paused => 2,
        State::Ended => 3,
    }
}

struct Player {
    engine: Engine,
    shared: Arc<Shared>,
    events: Arc<Events>,
    /// The place the last jump asked for, and when: until the engine has looked at it, that is the
    /// place. media3 reads the position the moment a seek returns, and a controller runs its seek bar
    /// on from that reading, so the place before the jump would stay on screen.
    jumped: Mutex<Option<(i64, Instant)>>,
}

fn player<'a>(h: jlong) -> Option<&'a Player> {
    // SAFETY: a non-zero `h` is a pointer `create` made with `Box::into_raw`, and Kotlin never passes
    // one on after `destroy`.
    (h != 0).then(|| unsafe { &*(h as *const Player) })
}

/// Starts the engine over the core's queue and settings. `sdk` is Android's API level; `float` the high
/// quality output setting, read once as the ExoPlayer path reads it; `memory_mb` the app's memory class,
/// which sizes how much of a song is kept loaded. 0 when the Java side could not be found.
extern "system" fn create(mut env: JNIEnv, _: JClass, sdk: jint, float: jboolean, memory_mb: jint) -> jlong {
    if JAVA.get().is_none() {
        match look_up(&mut env) {
            Ok(j) => {
                let _ = JAVA.set(j);
            }
            Err(e) => {
                cleared(&mut env);
                nori_core::alog::info(&format!("rust player: the Java side is missing: {e}"));
                return 0;
            }
        }
    }
    let shared = Arc::new(Shared::default());
    let output = TrackOutput::new(Box::new(JavaOpener { sdk }), float != 0, shared.clone());
    let library = AndroidLibrary { bytes: Arc::new(JavaBytes) };
    let sound = nori_core::settings_store::settings_current().map(|p| settings(&p)).unwrap_or_default();
    let config = Config { memory_mb: memory_mb.max(16) as u32, settings: sound, ..Config::default() };
    let events = Arc::new(Events::default());
    let tell = events.clone();
    log(&format!("the engine starts: API {sdk}, {} output, {} MB of memory", if float != 0 { "float" } else { "16-bit" }, config.memory_mb));
    let engine = Engine::start(library, CoreApp::new(), CoreQueue, Box::new(output), config, move |e| tell.push(e));
    Box::into_raw(Box::new(Player { engine, shared, events, jumped: Mutex::new(None) })) as jlong
}

/// Stops the engine: its thread ends and the AudioTrack is released.
extern "system" fn destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        // SAFETY: `h` came from `create` and Kotlin destroys it once.
        drop(unsafe { Box::from_raw(h as *mut Player) });
    }
}

extern "system" fn play_at(h: jlong, index: jint, ms: jlong) {
    if let (Some(p), Ok(i)) = (player(h), usize::try_from(index)) {
        log(&format!("to song {i} at {} ms", ms.max(0)));
        *p.jumped.lock() = Some((ms.max(0), Instant::now()));
        p.engine.play_at(i, ms.max(0));
    }
}

extern "system" fn play(h: jlong) {
    if let Some(p) = player(h) {
        p.engine.play();
    }
}

extern "system" fn pause(h: jlong) {
    if let Some(p) = player(h) {
        p.engine.pause();
    }
}

extern "system" fn queue_changed(h: jlong) {
    if let Some(p) = player(h) {
        p.engine.queue_changed();
    }
}

extern "system" fn set_repeat(h: jlong, mode: jint) {
    if let Some(p) = player(h) {
        p.engine.set_repeat(mode.clamp(0, 2) as u8);
    }
}

extern "system" fn replan(h: jlong) {
    if let Some(p) = player(h) {
        p.engine.replan();
    }
}

extern "system" fn gain_changed(h: jlong) {
    if let Some(p) = player(h) {
        p.engine.gain_changed();
    }
}

/// The sound and the controls' fades as the core's settings are now.
extern "system" fn apply_settings(h: jlong) {
    if let (Some(p), Some(prefs)) = (player(h), nori_core::settings_store::settings_current()) {
        p.engine.set_settings(settings(&prefs));
    }
}

/// Where the ear is in the song heard, now: the engine's last reading run on at the playing speed, or
/// the place a jump asked for while the engine has not made it yet (it has not looked, or the music is
/// still dipping before it).
extern "system" fn position_ms(h: jlong) -> jlong {
    let Some(p) = player(h) else { return 0 };
    let status = p.engine.status();
    match *p.jumped.lock() {
        Some((ms, at)) if status.at < at || status.switching => ms,
        _ => status.position_now().max(0),
    }
}

extern "system" fn mixing(h: jlong) -> jboolean {
    player(h).is_some_and(|p| p.engine.status().mixing) as jboolean
}

extern "system" fn bytes_written(h: jlong) -> jlong {
    player(h).map_or(0, |p| p.shared.bytes.load(Ordering::Relaxed) as jlong)
}

/// The next event, `kind << 32 | index` (state: its code), its words kept for [`event_text`]; -1 when
/// there is none, and the next event after that calls Kotlin again.
extern "system" fn event(h: jlong) -> jlong {
    let Some(p) = player(h) else { return -1 };
    let mut q = p.events.queue.lock();
    match q.pop_front() {
        Some((kind, index, text)) => {
            *p.events.text.lock() = text;
            ((kind as i64) << 32) | (index as u32 as i64)
        }
        None => {
            p.events.signalled.store(false, Ordering::Release);
            -1
        }
    }
}

/// The words of the event [`event`] last gave: the song's id, the error, the output's name.
extern "system" fn event_text(env: JNIEnv, _: JClass, h: jlong) -> jstring {
    let Some(p) = player(h) else { return std::ptr::null_mut() };
    let text = std::mem::take(&mut *p.events.text.lock());
    java_string(&env, &text)
}

/// The track's route changed: `kind` is the device's `AudioDeviceInfo.TYPE_*`, `name` its product name.
extern "system" fn device(mut env: JNIEnv, _: JClass, h: jlong, kind: jint, name: JString) {
    let Some(p) = player(h) else { return };
    let name = with_str(&mut env, &name, str::to_string).unwrap_or_default();
    if let Some(watch) = &*p.shared.watch.lock() {
        watch(Device { kind: nori_core::outputs::kind(kind), name });
    }
}
