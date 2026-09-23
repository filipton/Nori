//! Downloads as media3 runs them (`nori_core::transfers`): the platform reports each song's state and
//! each chunk, and asks for the words. The per-chunk report is primitives only; nothing is looked up by
//! name or allocated while bytes flow.

use jni::objects::{JClass, JString};
use jni::sys::{jfloat, jint, jlong, jstring};
use jni::JNIEnv;
use nori_core::transfers;

use crate::{java_string, native, with_str, Class};

pub(crate) static DOWNLOADS: Class = Class {
    name: c"dev/nori/music/downloads/DownloadsJni",
    methods: &[
        native!(c"held", c"(Ljava/lang/String;)I", held),
        native!(c"followed", c"(Ljava/lang/String;IJ)I", followed),
        native!(c"removed", c"(Ljava/lang/String;)I", removed),
        native!(c"unmark", c"(Ljava/lang/String;)I", unmark),
        native!(c"startFraction", c"(Ljava/lang/String;)F", start_fraction),
        native!(c"open", c"(Ljava/lang/String;J)I", open),
        native!(c"note", c"(IJJJ)F", note),
        native!(c"notice", c"(IIJ)I", notice),
        native!(c"noticeTitle", c"()Ljava/lang/String;", notice_title),
        native!(c"noticeText", c"()Ljava/lang/String;", notice_text),
        native!(c"noticePermille", c"()I", notice_permille),
        native!(c"summary", c"()Ljava/lang/String;", summary),
        native!(c"summaryFailed", c"()I", summary_failed),
    ],
};

pub(crate) static LINES: Class = Class {
    name: c"dev/nori/music/downloads/DownloadLines",
    methods: &[
        native!(c"row", c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;", row),
        native!(c"summary", c"(III)Ljava/lang/String;", summary_line),
    ],
};

/// Whether `id` is downloaded: 0 no, 1 queued or failed, 2 finished. Asked by every row a list draws.
extern "system" fn held(mut env: JNIEnv, _: JClass, id: JString) -> jint {
    with_str(&mut env, &id, transfers::held).unwrap_or(0)
}

/// media3 reported `id` in `state`; returns `transfers::followed`'s flags.
extern "system" fn followed(mut env: JNIEnv, _: JClass, id: JString, state: jint, now: jlong) -> jint {
    with_str(&mut env, &id, |id| transfers::followed(id, state, now)).unwrap_or(0)
}

extern "system" fn removed(mut env: JNIEnv, _: JClass, id: JString) -> jint {
    with_str(&mut env, &id, transfers::removed).unwrap_or(0)
}

extern "system" fn unmark(mut env: JNIEnv, _: JClass, id: JString) -> jint {
    with_str(&mut env, &id, transfers::unmark).unwrap_or(0)
}

extern "system" fn start_fraction(mut env: JNIEnv, _: JClass, id: JString) -> jfloat {
    with_str(&mut env, &id, transfers::start_fraction).unwrap_or(-1.0)
}

extern "system" fn open(mut env: JNIEnv, _: JClass, id: JString, now: jlong) -> jint {
    with_str(&mut env, &id, |id| transfers::open(id, now)).unwrap_or(-1)
}

/// A chunk arrived on `slot`; called per chunk, so primitives only.
extern "system" fn note(slot: jint, length: jlong, bytes: jlong, now: jlong) -> jfloat {
    transfers::note(slot, length, bytes, now)
}

extern "system" fn notice(listed: jint, waiting: jint, now: jlong) -> jint {
    transfers::notice(listed, waiting != 0, now)
}

extern "system" fn notice_title(env: JNIEnv, _: JClass) -> jstring {
    transfers::notice_title(|s| java_string(&env, s))
}

extern "system" fn notice_text(env: JNIEnv, _: JClass) -> jstring {
    transfers::notice_text(|s| java_string(&env, s))
}

extern "system" fn notice_permille() -> jint {
    transfers::notice_permille()
}

extern "system" fn summary(env: JNIEnv, _: JClass) -> jstring {
    java_string(&env, &transfers::summary())
}

extern "system" fn summary_failed() -> jint {
    transfers::summary_failed()
}

/// A running song's second line, asked whenever its ring moves.
extern "system" fn row(mut env: JNIEnv, _: JClass, id: JString, artist: JString) -> jstring {
    let (Ok(id), Ok(artist)) = (env.get_string(&id), env.get_string(&artist)) else { return std::ptr::null_mut() };
    let (id, artist): (std::borrow::Cow<str>, std::borrow::Cow<str>) = ((&id).into(), (&artist).into());
    transfers::row(&id, &artist, |s| java_string(&env, s))
}

extern "system" fn summary_line(env: JNIEnv, _: JClass, active: jint, queued: jint, failed: jint) -> jstring {
    transfers::summary_line(active, queued, failed, |s| java_string(&env, s))
}
