//! The transition engine (`nori_player::engine`) as Android reaches it. `TransitionSink` stays a media3
//! AudioSink and forwards every call here; the engine calls back into it for the real output below
//! (`down*`), and the core answers what only the app knows (`nori_core::automix::host`).
//!
//! media3's output insists that a buffer it took only part of is offered again as the same Java
//! object. So audio passing straight through goes down as the decoder's own buffer, and each queued
//! chunk gets one Java wrapper that lives until the chunk is gone.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::OnceLock;

use jni::objects::{GlobalRef, JByteBuffer, JClass, JMethodID, JObject, JString, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jboolean, jint, jlong, jobject};
use jni::JNIEnv;
use parking_lot::Mutex;

use nori_core::automix::host::CoreHost;
use nori_player::burst::{Burst, Fed};
use nori_player::engine::{Downstream, StreamFormat, TransitionEngine, POSITION_NOT_SET};
use nori_player::pcm::{Encoding, Format};

use crate::{native, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/TransitionEngineJni",
    methods: &[
        native!(c"create", c"()J", create),
        native!(c"destroy", c"(J)V", destroy),
        native!(c"replan", c"(J)V", replan),
        native!(c"setLockRate", c"(JZ)V", set_lock_rate),
        native!(c"setOffset", c"(JJ)V", set_offset),
        native!(c"configure", c"(JLdev/nori/music/playback/TransitionSink;JLjava/lang/String;IIII)V", configure),
        native!(c"handleBuffer", c"(JLdev/nori/music/playback/TransitionSink;JLjava/nio/ByteBuffer;IIJJ)J", handle_buffer),
        native!(c"handleDiscontinuity", c"(JLdev/nori/music/playback/TransitionSink;J)V", handle_discontinuity),
        native!(c"position", c"(JLdev/nori/music/playback/TransitionSink;JZJ)J", position),
        native!(c"playToEnd", c"(JLdev/nori/music/playback/TransitionSink;J)Z", play_to_end),
        native!(c"status", c"(J)Ljava/nio/ByteBuffer;", status),
        native!(c"mixing", c"()Z", mixing),
        native!(c"setBurst", c"(JZ)V", set_burst),
        native!(c"restartBurst", c"(J)V", restart_burst),
        native!(c"holding", c"()Z", holding),
        native!(c"queueEmpty", c"(JLdev/nori/music/playback/TransitionSink;J)Z", queue_empty),
        native!(c"flush", c"(JLdev/nori/music/playback/TransitionSink;J)V", flush),
        native!(c"reset", c"(JLdev/nori/music/playback/TransitionSink;J)V", reset),
    ],
};

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
    // SAFETY: a non-zero `h` is a pointer `create` made with `Box::into_raw`, and Kotlin never passes
    // one on after `destroy`.
    (h != 0).then(|| unsafe { &*(h as *const Handle) })
}

/// The two callbacks made for every buffer and every position query, looked up once: a lookup by name
/// costs more than the call itself, and they are made many times a second while music plays.
struct Methods {
    handle_buffer: JMethodID,
    position: JMethodID,
}

static METHODS: OnceLock<Methods> = OnceLock::new();

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
        // SAFETY: the method id was looked up on the sink's own class with this signature: a ByteBuffer
        // and a long in, a long out.
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
                        // SAFETY: `data` is a chunk of the engine's pool, whose memory stays where it is while
                        // the engine lives; the wrapper is only offered to the output while that chunk is
                        // being played out, and never after `destroy`.
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
        // SAFETY: the method id was looked up on the sink's own class with this signature: a boolean in,
        // a long out.
        let v = unsafe { self.env.call_method_unchecked(self.sink, self.methods.position, ReturnType::Primitive(Primitive::Long), &args) }.and_then(|v| v.j());
        if self.failed_now() {
            return POSITION_NOT_SET;
        }
        v.unwrap_or(POSITION_NOT_SET)
    }
}

/// Runs `f` on the engine with the output below and the core as the host, then tells the core what
/// the ear is at if that changed.
fn with<'e, R>(
    env: &mut JNIEnv<'e>, h: jlong, sink: &JObject<'e>, now_ms: jlong, input: Option<(&JByteBuffer<'e>, usize, usize)>, prefetched: Option<(bool, i64)>,
    f: impl FnOnce(&mut TransitionEngine<i32>, &mut Fed<'_, Down<'_, 'e>>, &mut CoreHost<&mut dyn FnMut()>) -> R,
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
    // SAFETY: the clones are used only within this call, on this thread, while `env` is alive.
    let mut down = Down { env: unsafe { env.unsafe_clone() }, sink, methods, prefetched, input, chunk: &mut chunk, wrappers: &mut wrappers, failed: false };
    let mut app_env = unsafe { env.unsafe_clone() };
    // The one thing that still reaches Kotlin: a page on screen follows the ear at once.
    let mut heard_changed = || {
        let _ = app_env.call_method(sink, "hostHeardChanged", "()V", &[]);
    };
    let mut app = CoreHost { now_ms, heard_changed: &mut heard_changed as &mut dyn FnMut() };
    let mut burst = h.burst.lock();
    let r = f(&mut e, &mut Fed::new(&mut down, &mut burst, now_ms), &mut app);
    h.status[1].store(burst.bytes_written as i64, Ordering::Relaxed);
    // A Java exception is pending: let it reach the player as it is, and say nothing more.
    if down.failed || env.exception_check().unwrap_or(true) {
        return Some(r);
    }
    h.status[0].store(e.has_pending_data() as i64, Ordering::Relaxed);
    nori_core::heard::publish(e.heard());
    Some(r)
}

/// Whether a mix is being heard right now (for the test bridge and logs).
extern "system" fn mixing() -> jboolean {
    nori_core::heard::mixing() as jboolean
}

/// Whether the ear is behind the player on a held ending (for logs).
extern "system" fn holding() -> jboolean {
    nori_core::heard::holding() as jboolean
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

extern "system" fn create() -> jlong {
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

extern "system" fn destroy(h: jlong) {
    if h != 0 {
        // SAFETY: `h` came from `create` and Kotlin destroys it once.
        drop(unsafe { Box::from_raw(h as *mut Handle) });
    }
}

extern "system" fn replan(h: jlong) {
    if let Some(h) = handle(h) {
        h.replan.store(true, Ordering::Relaxed);
    }
}

extern "system" fn set_lock_rate(h: jlong, on: jboolean) {
    if let Some(h) = handle(h) {
        h.lock_rate.store(on != 0, Ordering::Relaxed);
    }
}

extern "system" fn set_offset(h: jlong, offset_us: jlong) {
    if let Some(h) = handle(h) {
        h.engine.lock().set_output_stream_offset_us(offset_us);
    }
}

/// `encoding` is media3's (2 = 16-bit, 4 = float; anything else is not samples).
#[allow(clippy::too_many_arguments)]
extern "system" fn configure<'e>(
    mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong, id: JString<'e>, rate: jint, channels: jint, encoding: jint, token: jint,
) {
    let id = opt_string(&mut env, &id);
    with(&mut env, h, &sink, now_ms, None, None, |e, d, a| e.configure(d, a, stream(id, rate, channels, encoding), token));
}

/// Offers `len` bytes of `buffer` from `pos`; `down_position_us` is the output's clock, read just before.
/// Returns `(taken << 32) | bytes used`.
#[allow(clippy::too_many_arguments)]
extern "system" fn handle_buffer<'e>(
    mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong, buffer: JByteBuffer<'e>, pos: jint, len: jint, pts_us: jlong, down_position_us: jlong,
) -> jlong {
    let Some(data) = crate::region(&env, &buffer, pos, len) else { return 0 };
    let data: &[u8] = data;
    let input = Some((&buffer, data.as_ptr() as usize, data.len()));
    with(&mut env, h, &sink, now_ms, input, Some((false, down_position_us)), |e, d, a| e.handle_buffer(d, a, data, pts_us))
        .map_or(0, |(taken, used)| ((taken as jlong) << 32) | used as jlong)
}

extern "system" fn handle_discontinuity<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) {
    with(&mut env, h, &sink, now_ms, None, None, |e, d, a| e.handle_discontinuity(d, a));
}

extern "system" fn position<'e>(
    mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong, source_ended: jboolean, down_position_us: jlong,
) -> jlong {
    let prefetched = Some((source_ended != 0, down_position_us));
    with(&mut env, h, &sink, now_ms, None, prefetched, |e, d, a| e.position_us(d, a, source_ended != 0)).unwrap_or(POSITION_NOT_SET)
}

/// Whatever is held goes out; true when everything queued went down.
extern "system" fn play_to_end<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) -> jboolean {
    with(&mut env, h, &sink, now_ms, None, None, |e, d, a| e.play_to_end_of_stream(d, a)).unwrap_or(true) as jboolean
}

/// A direct buffer over the engine's status words (see `Handle::status`), made once per engine.
extern "system" fn status(mut env: JNIEnv, _: JClass, h: jlong) -> jobject {
    let Some(h) = handle(h) else { return std::ptr::null_mut() };
    let p = h.status.as_ptr() as *mut u8;
    // SAFETY: the status words live in their own box for as long as the handle, which Kotlin keeps
    // longer than the buffer it reads them through.
    unsafe { env.new_direct_byte_buffer(p, std::mem::size_of::<[AtomicI64; 2]>()) }.map_or(std::ptr::null_mut(), |b| b.into_raw())
}

extern "system" fn queue_empty<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) -> jboolean {
    with(&mut env, h, &sink, now_ms, None, None, |e, d, _| e.queue_empty(d)).unwrap_or(true) as jboolean
}

extern "system" fn flush<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) {
    if let Some(hd) = handle(h) {
        *hd.chunk.lock() = None;
        hd.burst.lock().restart();
    }
    with(&mut env, h, &sink, now_ms, None, None, |e, _, a| e.flush(a));
}

extern "system" fn reset<'e>(mut env: JNIEnv<'e>, _: JClass, h: jlong, sink: JObject<'e>, now_ms: jlong) {
    if let Some(hd) = handle(h) {
        *hd.chunk.lock() = None;
        hd.burst.lock().restart();
    }
    with(&mut env, h, &sink, now_ms, None, None, |e, _, a| e.reset(a));
}

/// Bursts on or off: off while the output decodes by itself, or the equalizer is being tuned.
extern "system" fn set_burst(h: jlong, on: jboolean) {
    if let Some(h) = handle(h) {
        h.burst.lock().enabled = on != 0;
    }
}

/// Playing or pausing: the output may have been stopped and its clock reset meanwhile, so what was
/// written before means nothing now.
extern "system" fn restart_burst(h: jlong) {
    if let Some(h) = handle(h) {
        h.burst.lock().restart();
    }
}
