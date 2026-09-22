//! The transition engine (`nori_player::engine`) as Android reaches it. `TransitionSink` stays a media3
//! AudioSink and forwards every call here; the engine calls back into it for the real output below
//! (`down*`) and for what only the app knows (`host*`: plans, the analysis store, logging).
//!
//! media3's output insists that a buffer it took only part of is offered again as the same Java
//! object. So audio passing straight through goes down as the decoder's own buffer, and each queued
//! chunk gets one Java wrapper that lives until the chunk is gone.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::OnceLock;

use jni::objects::{GlobalRef, JByteBuffer, JClass, JMethodID, JObject, JString, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use parking_lot::Mutex;

use nori_player::automix::analysis::Analyzer;
use nori_player::burst::{Burst, Fed};
use nori_player::engine::{Downstream, Heard, Host, Plan, StreamFormat, TransitionEngine, POSITION_NOT_SET};
use nori_player::pcm::{Encoding, Format};

struct Handle {
    engine: Mutex<TransitionEngine<i32>>,
    /// From other threads, applied at the engine's next call.
    replan: AtomicBool,
    lock_rate: AtomicBool,
    /// The Java wrapper around the queued chunk being offered: (address, length, buffer).
    chunk: Mutex<Option<(usize, usize, GlobalRef)>>,
    /// Wrappers of chunks gone down, kept for the next chunk in the same pooled memory: the engine
    /// recycles its buffers, so after the first few every chunk finds its wrapper here.
    wrappers: Mutex<Vec<(usize, usize, GlobalRef)>>,
    /// Read by Kotlin through a direct buffer over it, with no call: [0] whether audio is queued,
    /// [1] bytes handed to the output so far.
    status: Box<[AtomicI64; 2]>,
    /// Feeding the output in bursts (`nori_player::burst`).
    burst: Mutex<Burst>,
}

fn handle<'a>(h: jlong) -> Option<&'a Handle> {
    (h != 0).then(|| unsafe { &*(h as *const Handle) })
}

/// The two callbacks made for every buffer and every position query, looked up once: a lookup by name
/// costs more than the call itself, and they are made many times a second while music plays.
struct Methods {
    handle_buffer: JMethodID,
    position: JMethodID,
}

static METHODS: OnceLock<Methods> = OnceLock::new();

/// The engine's latest word on what the ear has, for [`crate::heard::HeardClock`].
pub(crate) static HEARD: Mutex<Heard> = Mutex::new(Heard {
    id: None,
    us: 0,
    at_ms: 0,
    until_us: i64::MAX,
    mixing: false,
    next_id: None,
    next_from_us: 0,
    next_rate: 1.0,
    from_id: None,
    audible_us: i64::MAX,
});

fn methods(env: &mut JNIEnv, sink: &JObject) -> Option<&'static Methods> {
    if let Some(m) = METHODS.get() {
        return Some(m);
    }
    let class = env.get_object_class(sink).ok()?;
    let m = Methods {
        handle_buffer: env.get_method_id(&class, "downHandleBuffer", "(Ljava/nio/ByteBuffer;J)J").ok()?,
        position: env.get_method_id(&class, "downPosition", "(Z)J").ok()?,
    };
    Some(METHODS.get_or_init(|| m))
}

/// How many chunk wrappers are kept: more than the engine's pool holds buffers of one size.
const WRAPPERS: usize = 32;

fn keep_wrapper(wrappers: &mut Vec<(usize, usize, GlobalRef)>, w: (usize, usize, GlobalRef)) {
    if wrappers.len() >= WRAPPERS {
        wrappers.remove(0);
    }
    wrappers.push(w);
}

/// The output below: `TransitionSink`'s own super calls.
struct Down<'a, 'e> {
    env: JNIEnv<'e>,
    sink: &'a JObject<'e>,
    methods: &'static Methods,
    /// The output's position Kotlin read just before calling, for the engine's first question about it.
    prefetched: Option<(bool, i64)>,
    /// The decoder's buffer for this call, and the bytes of it the engine was given.
    input: Option<(&'a JByteBuffer<'e>, usize, usize)>,
    chunk: &'a mut Option<(usize, usize, GlobalRef)>,
    wrappers: &'a mut Vec<(usize, usize, GlobalRef)>,
    failed: bool,
}

impl Down<'_, '_> {
    fn down_handle_buffer(&mut self, buf: &JObject, pts_us: i64) -> jni::errors::Result<i64> {
        let args = [JValue::Object(buf).as_jni(), JValue::Long(pts_us).as_jni()];
        unsafe { self.env.call_method_unchecked(self.sink, self.methods.handle_buffer, ReturnType::Primitive(Primitive::Long), &args) }.and_then(|v| v.j())
    }

    fn failed_now(&mut self) -> bool {
        if self.env.exception_check().unwrap_or(true) {
            self.failed = true;
        }
        self.failed
    }
}

impl Downstream for Down<'_, '_> {
    type Config = i32;

    fn configure(&mut self, token: &i32, _: Option<Format>) {
        if self.failed {
            return;
        }
        let _ = self.env.call_method(self.sink, "downConfigure", "(I)V", &[JValue::Int(*token)]);
        self.failed_now();
    }

    fn handle_buffer(&mut self, data: &[u8], from: usize, pts_us: i64) -> (bool, usize) {
        if self.failed {
            return (false, 0);
        }
        let key = (data.as_ptr() as usize, data.len());
        let r = match self.input {
            // The decoder's own buffer, from where the engine was given it: its Java position is there already.
            Some((buf, addr, len)) if from == 0 && key == (addr, len) => self.down_handle_buffer(buf, pts_us),
            _ => {
                if self.chunk.as_ref().map(|(a, l, _)| (*a, *l)) != Some(key) {
                    let made = match self.wrappers.iter().position(|(a, l, _)| (*a, *l) == key) {
                        // A wrapper over this very memory: moved back to where this chunk starts.
                        Some(i) => {
                            let (_, _, g) = self.wrappers.swap_remove(i);
                            self.env.call_method(g.as_obj(), "position", "(I)Ljava/nio/Buffer;", &[JValue::Int(from as jint)]).map(|_| g)
                        }
                        None => unsafe { self.env.new_direct_byte_buffer(data.as_ptr() as *mut u8, data.len()) }.and_then(|b| {
                            // A buffer made here starts big-endian, as Java's do; media3 checks for
                            // little-endian (native) and throws otherwise.
                            let native = self.env.call_static_method("java/nio/ByteOrder", "nativeOrder", "()Ljava/nio/ByteOrder;", &[])?.l()?;
                            self.env.call_method(&b, "order", "(Ljava/nio/ByteOrder;)Ljava/nio/ByteBuffer;", &[JValue::Object(&native)])?;
                            self.env.call_method(&b, "position", "(I)Ljava/nio/Buffer;", &[JValue::Int(from as jint)])?;
                            self.env.new_global_ref(b)
                        }),
                    };
                    // One offered and not taken whole stays current; the one before it is done with.
                    if let Some(old) = self.chunk.take() {
                        keep_wrapper(self.wrappers, old);
                    }
                    match made {
                        Ok(g) => *self.chunk = Some((key.0, key.1, g)),
                        Err(_) => {
                            self.failed_now();
                            return (false, 0);
                        }
                    }
                }
                let buf = self.chunk.as_ref().map(|(_, _, g)| g.clone()).expect("just made");
                self.down_handle_buffer(buf.as_obj(), pts_us)
            }
        };
        let v = r.unwrap_or(0);
        if self.failed_now() {
            return (false, 0);
        }
        let taken = (v >> 32) != 0;
        let used = (v & 0xffff_ffff) as usize;
        // A chunk taken whole is done with: its wrapper waits for the next chunk in that memory.
        if taken && self.input.map_or(true, |(_, a, l)| key != (a, l)) {
            if let Some(old) = self.chunk.take() {
                keep_wrapper(self.wrappers, old);
            }
        }
        (taken, used)
    }

    fn handle_discontinuity(&mut self) {
        if self.failed {
            return;
        }
        let _ = self.env.call_method(self.sink, "downDiscontinuity", "()V", &[]);
        self.failed_now();
    }

    fn position_us(&mut self, source_ended: bool) -> i64 {
        if self.failed {
            return POSITION_NOT_SET;
        }
        if let Some((ended, at)) = self.prefetched.take() {
            if ended == source_ended {
                return at;
            }
        }
        let args = [JValue::Bool(source_ended as jboolean).as_jni()];
        let v = unsafe { self.env.call_method_unchecked(self.sink, self.methods.position, ReturnType::Primitive(Primitive::Long), &args) }.and_then(|v| v.j());
        if self.failed_now() {
            return POSITION_NOT_SET;
        }
        v.unwrap_or(POSITION_NOT_SET)
    }
}

struct App<'a, 'e> {
    env: JNIEnv<'e>,
    sink: &'a JObject<'e>,
    now_ms: i64,
}

/// What only the app knows. Plans, analyses and the log are the core's own (`super::planner`,
/// `crate::alog`); the one thing that still reaches Kotlin is the nudge that the heard song changed, so a
/// page on screen follows the ear at once.
impl Host for App<'_, '_> {
    fn plan_for(&mut self, outgoing_id: &str) -> Option<Plan> {
        super::planner::plan_for(outgoing_id)
    }

    fn wants_analysis(&mut self, song_id: &str) -> Option<u64> {
        super::planner::wants_analysis(song_id)
    }

    fn analysed(&mut self, song_id: &str, analyzer: Analyzer, _channels: usize, frames: u64, rate: u32) {
        super::planner::analysed(song_id, analyzer, frames, rate);
    }

    fn heard_changed(&mut self) {
        let _ = self.env.call_method(self.sink, "hostHeardChanged", "()V", &[]);
    }

    fn log(&mut self, message: &str) {
        crate::alog::info(message);
    }

    fn now_ms(&self) -> i64 {
        self.now_ms
    }
}

/// Runs `f` on the engine with the output below and the app as the sink provides them, then tells
/// Kotlin what the ear is at if that changed.
fn with<'e, R>(
    env: &mut JNIEnv<'e>, h: jlong, sink: &JObject<'e>, now_ms: jlong, input: Option<(&JByteBuffer<'e>, usize, usize)>, prefetched: Option<(bool, i64)>,
    f: impl FnOnce(&mut TransitionEngine<i32>, &mut Fed<'_, Down<'_, 'e>>, &mut App<'_, 'e>) -> R,
) -> Option<R> {
    let h = handle(h)?;
    let methods = methods(env, sink)?;
    let mut e = h.engine.lock();
    if h.replan.swap(false, Ordering::Relaxed) {
        e.replan();
    }
    e.lock_rate = h.lock_rate.load(Ordering::Relaxed);
    let mut chunk = h.chunk.lock();
    let mut wrappers = h.wrappers.lock();
    let mut down = Down { env: unsafe { env.unsafe_clone() }, sink, methods, prefetched, input, chunk: &mut chunk, wrappers: &mut wrappers, failed: false };
    let mut app = App { env: unsafe { env.unsafe_clone() }, sink, now_ms };
    let mut burst = h.burst.lock();
    let r = f(&mut e, &mut Fed::new(&mut down, &mut burst, now_ms), &mut app);
    h.status[1].store(burst.bytes_written as i64, Ordering::Relaxed);
    // A Java exception is pending: let it reach the player as it is, and say nothing more.
    if down.failed || env.exception_check().unwrap_or(true) {
        return Some(r);
    }
    // Where the app reads what the ear has (crates/core/src/heard.rs). Asked about on every position
    // query while a hold or mix runs, so it is copied in place, reusing the strings it already has.
    h.status[0].store(e.has_pending_data() as i64, Ordering::Relaxed);
    let heard = e.heard();
    let mut shared = HEARD.lock();
    if *shared != *heard {
        shared.assign(heard);
    }
    Some(r)
}

/// Whether a mix is being heard right now (for the test bridge and logs).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_mixing(_: JNIEnv, _: JClass) -> jboolean {
    HEARD.lock().mixing as jboolean
}

/// Whether the ear is behind the player on a held ending (for logs).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_holding(_: JNIEnv, _: JClass) -> jboolean {
    HEARD.lock().id.is_some() as jboolean
}

fn stream(id: Option<String>, rate: jint, channels: jint, encoding: jint) -> StreamFormat {
    let format = Encoding::from_media3(encoding).map(|e| Format { rate: rate.max(1) as u32, channels: channels.clamp(1, 8) as usize, encoding: e });
    StreamFormat { id, format }
}

fn opt_string(env: &mut JNIEnv, s: &JString) -> Option<String> {
    if s.is_null() {
        None
    } else {
        env.get_string(s).ok().map(Into::into)
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_create(_: JNIEnv, _: JClass) -> jlong {
    let h = Handle {
        engine: Mutex::new(TransitionEngine::new()),
        replan: AtomicBool::new(false),
        lock_rate: AtomicBool::new(true),
        chunk: Mutex::new(None),
        wrappers: Mutex::new(Vec::with_capacity(WRAPPERS)),
        status: Box::new([AtomicI64::new(0), AtomicI64::new(0)]),
        burst: Mutex::new(Burst::default()),
    };
    Box::into_raw(Box::new(h)) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        drop(unsafe { Box::from_raw(h as *mut Handle) });
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_replan(_: JNIEnv, _: JClass, h: jlong) {
    if let Some(h) = handle(h) {
        h.replan.store(true, Ordering::Relaxed);
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_setLockRate(_: JNIEnv, _: JClass, h: jlong, on: jboolean) {
    if let Some(h) = handle(h) {
        h.lock_rate.store(on != 0, Ordering::Relaxed);
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_setOffset(_: JNIEnv, _: JClass, h: jlong, offset_us: jlong) {
    if let Some(h) = handle(h) {
        h.engine.lock().set_output_stream_offset_us(offset_us);
    }
}

/// `encoding` is media3's (2 = 16-bit, 4 = float; anything else is not samples).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_configure<'e>(
    mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong, id: JString<'e>, rate: jint, channels: jint, encoding: jint, token: jint,
) {
    let id = opt_string(&mut env, &id);
    with(&mut env, h, &sink, now_ms, None, None, |e, d, a| e.configure(d, a, stream(id, rate, channels, encoding), token));
}

/// Offers `len` bytes of `buffer` from `pos`; `down_position_us` is the output's clock, read just before.
/// Returns `(taken << 32) | bytes used`.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_handleBuffer<'e>(
    mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong, buffer: JByteBuffer<'e>, pos: jint, len: jint, pts_us: jlong, down_position_us: jlong,
) -> jlong {
    let Ok(base) = env.get_direct_buffer_address(&buffer) else { return 0 };
    if base.is_null() || pos < 0 || len < 0 {
        return 0;
    }
    let data = unsafe { std::slice::from_raw_parts(base.add(pos as usize), len as usize) };
    let input = Some((&buffer, data.as_ptr() as usize, data.len()));
    with(&mut env, h, &sink, now_ms, input, Some((false, down_position_us)), |e, d, a| e.handle_buffer(d, a, data, pts_us))
        .map_or(0, |(taken, used)| ((taken as jlong) << 32) | used as jlong)
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_handleDiscontinuity<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) {
    with(&mut env, h, &sink, now_ms, None, None, |e, d, a| e.handle_discontinuity(d, a));
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_position<'e>(
    mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong, source_ended: jboolean, down_position_us: jlong,
) -> jlong {
    let prefetched = Some((source_ended != 0, down_position_us));
    with(&mut env, h, &sink, now_ms, None, prefetched, |e, d, a| e.position_us(d, a, source_ended != 0)).unwrap_or(POSITION_NOT_SET)
}

/// Whatever is held goes out; true when everything queued went down.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_playToEnd<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) -> jboolean {
    with(&mut env, h, &sink, now_ms, None, None, |e, d, a| e.play_to_end_of_stream(d, a)).unwrap_or(true) as jboolean
}

/// A direct buffer over the engine's status words (see `Handle::status`), made once per engine.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_status<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong) -> JObject<'e> {
    let Some(h) = handle(h) else { return JObject::null() };
    let p = h.status.as_ptr() as *mut u8;
    unsafe { env.new_direct_byte_buffer(p, std::mem::size_of::<[AtomicI64; 2]>()) }.map(JObject::from).unwrap_or(JObject::null())
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_queueEmpty<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) -> jboolean {
    with(&mut env, h, &sink, now_ms, None, None, |e, d, _| e.queue_empty(d)).unwrap_or(true) as jboolean
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_flush<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) {
    if let Some(hd) = handle(h) {
        *hd.chunk.lock() = None;
        hd.burst.lock().restart();
    }
    with(&mut env, h, &sink, now_ms, None, None, |e, _, a| e.flush(a));
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_reset<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) {
    if let Some(hd) = handle(h) {
        *hd.chunk.lock() = None;
        hd.burst.lock().restart();
    }
    with(&mut env, h, &sink, now_ms, None, None, |e, _, a| e.reset(a));
}

/// Bursts on or off: off while the output decodes by itself, or the equalizer is being tuned.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_setBurst(_: JNIEnv, _: JClass, h: jlong, on: jboolean) {
    if let Some(h) = handle(h) {
        h.burst.lock().enabled = on != 0;
    }
}

/// Playing or pausing: the output may have been stopped and its clock reset meanwhile, so what was
/// written before means nothing now.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_TransitionEngineJni_restartBurst(_: JNIEnv, _: JClass, h: jlong) {
    if let Some(h) = handle(h) {
        h.burst.lock().restart();
    }
}
