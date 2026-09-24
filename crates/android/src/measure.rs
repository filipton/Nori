//! AutoMix's measuring ahead on Android, for both players: nori-engine's `Measurer` (the core picks the
//! songs, each is decoded once, whole, on a thread of the lowest priority) over media3's caches. Kotlin
//! only says where a song's bytes are (`MeasureBridge.whole`: the files of a download, or of a copy in
//! the stream cache, once every byte is there) and when one has become whole (`arrived`, from the
//! caches' own callbacks); the files are then read straight, half a megabyte at a time.
//!
//! This replaced a Kotlin measurer over MediaExtractor, which read each packet through the platform's
//! extractor and each of the extractor's reads back through media3's data sources: a small blocking
//! step per packet, so the thread woke tens of times a second for as long as a song took, and it was
//! started again on every loading burst.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use jni::objects::{GlobalRef, JClass, JObjectArray, JStaticMethodID, JString, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::{JNIEnv, JavaVM};
use nori_engine::core::{key_format, Measurer, Shelf, Whole};
use parking_lot::Mutex;

use crate::{native, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/MeasureJni",
    methods: &[
        native!(c"start", c"()V", start),
        native!(c"update", c"()V", update),
        native!(c"arrived", c"()V", arrived),
        native!(c"stop", c"()V", stop),
    ],
};

/// `MeasureBridge`, looked up once.
struct Java {
    vm: JavaVM,
    bridge: GlobalRef,
    whole: JStaticMethodID,
    measured: JStaticMethodID,
}

static JAVA: OnceLock<Java> = OnceLock::new();

/// The measurer while the playback service runs.
static MEASURER: Mutex<Option<Arc<Measurer>>> = Mutex::new(None);

fn look_up(env: &mut JNIEnv) -> jni::errors::Result<Java> {
    let bridge = env.find_class("dev/nori/music/playback/MeasureBridge")?;
    Ok(Java {
        vm: env.get_java_vm()?,
        whole: env.get_static_method_id(&bridge, "whole", "(Ljava/lang/String;)[Ljava/lang/String;")?,
        measured: env.get_static_method_id(&bridge, "measured", "()V")?,
        bridge: env.new_global_ref(&bridge)?,
    })
}

/// The measuring thread's JNIEnv, attached under its own name.
fn env() -> Option<(&'static Java, JNIEnv<'static>)> {
    let java = JAVA.get()?;
    Some((java, crate::attached(&java.vm)?))
}

fn cleared(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

/// Where media3 keeps a song: asked of Kotlin, which knows its caches.
struct Media3;

impl Shelf for Media3 {
    fn whole(&self, id: &str) -> Option<Whole> {
        let (java, mut env) = env()?;
        let parts = env.with_local_frame(8, |env| -> jni::errors::Result<Option<Vec<String>>> {
            let id = env.new_string(id)?;
            let bridge = <&JClass>::from(java.bridge.as_obj());
            // SAFETY: MeasureBridge.whole(String): String[], looked up with this signature.
            let found = unsafe { env.call_static_method_unchecked(bridge, java.whole, ReturnType::Object, &[JValue::Object(&id).as_jni()]) }?.l()?;
            if found.is_null() {
                return Ok(None);
            }
            let found = JObjectArray::from(found);
            let n = env.get_array_length(&found)?;
            let mut parts = Vec::with_capacity(n as usize);
            for i in 0..n {
                let s = JString::from(env.get_object_array_element(&found, i)?);
                parts.push(String::from(env.get_string(&s)?));
                env.delete_local_ref(s)?;
            }
            Ok(Some(parts))
        });
        cleared(&mut env);
        let mut parts = parts.ok().flatten()?;
        if parts.len() < 2 {
            return None;
        }
        // The cache key first, then the files. A download may have been transcoded, and its key says
        // nothing: the file says what it is. A stream's key names its format.
        let key = parts.remove(0);
        let hint = if key == nori_core::stream::download_key(id.to_string()) {
            None
        } else {
            key_format(&key).or_else(|| nori_core::queue::queue_song(id.to_string()).map(|s| s.suffix)).filter(|s| !s.is_empty())
        };
        Some(Whole { files: parts.into_iter().map(PathBuf::from).collect(), hint })
    }
}

/// A song was measured: the transitions around it are planned again (Kotlin's `MeasureBridge.measured`).
fn told() {
    if let Some((java, mut env)) = env() {
        let bridge = <&JClass>::from(java.bridge.as_obj());
        // SAFETY: MeasureBridge.measured(), looked up with this signature.
        let _ = unsafe { env.call_static_method_unchecked(bridge, java.measured, ReturnType::Primitive(Primitive::Void), &[]) };
        cleared(&mut env);
    }
}

fn measurer() -> Option<Arc<Measurer>> {
    MEASURER.lock().clone()
}

/// The playback service started: the measurer exists from now on, idle until it is asked for songs.
extern "system" fn start(mut env: JNIEnv, _: JClass) {
    if JAVA.get().is_none() {
        match look_up(&mut env) {
            Ok(j) => {
                let _ = JAVA.set(j);
            }
            Err(e) => {
                cleared(&mut env);
                nori_core::alog::info(&format!("measuring ahead: the Java side is missing: {e}"));
                return;
            }
        }
    }
    let mut m = MEASURER.lock();
    if m.is_none() {
        *m = Some(Measurer::on_shelf(nori_core::active, Box::new(Media3), Some(Box::new(told))));
    }
}

/// The songs coming up may have changed: the core names them (`queue_measure`, none while AutoMix is
/// off), and the same songs as before change nothing.
extern "system" fn update() {
    if let Some(m) = measurer() {
        m.ask(nori_core::rules::queue_measure());
    }
}

/// A song has become whole in one of the caches.
extern "system" fn arrived() {
    if let Some(m) = measurer() {
        m.arrived();
    }
}

/// The playback service stops: a song being measured is left, and the measurer goes.
extern "system" fn stop() {
    if let Some(m) = MEASURER.lock().take() {
        m.ask(Vec::new());
    }
}
