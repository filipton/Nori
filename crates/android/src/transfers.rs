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
        native!(c"noticeWords", c"()Ljava/lang/String;", notice_words),
        native!(c"noticePermille", c"()I", notice_permille),
        native!(c"summary", c"()Ljava/lang/String;", summary),
        native!(c"summaryFailed", c"()I", summary_failed),
    ],
};

pub(crate) static LINES: Class = Class {
    name: c"dev/nori/music/downloads/DownloadLines",
    methods: &[
        native!(c"row", c"(Ljava/lang/String;)Ljava/lang/String;", row),
        native!(c"summary", c"(III)Ljava/lang/String;", summary_line),
    ],
};

/// Whether `id` is downloaded: 0 no, 1 queued or failed, 2 finished. Asked by every row a list draws.
extern "system" fn held(env: JNIEnv, _: JClass, id: JString) -> jint {
    with_str(&env, &id, transfers::held).unwrap_or(0)
}

/// media3 reported `id` in `state`; returns `transfers::followed`'s flags.
extern "system" fn followed(env: JNIEnv, _: JClass, id: JString, state: jint, now: jlong) -> jint {
    with_str(&env, &id, |id| transfers::followed(id, state, now)).unwrap_or(0)
}

extern "system" fn removed(env: JNIEnv, _: JClass, id: JString) -> jint {
    with_str(&env, &id, transfers::removed).unwrap_or(0)
}

extern "system" fn unmark(env: JNIEnv, _: JClass, id: JString) -> jint {
    with_str(&env, &id, transfers::unmark).unwrap_or(0)
}

extern "system" fn start_fraction(env: JNIEnv, _: JClass, id: JString) -> jfloat {
    with_str(&env, &id, transfers::start_fraction).unwrap_or(-1.0)
}

extern "system" fn open(env: JNIEnv, _: JClass, id: JString, now: jlong) -> jint {
    with_str(&env, &id, |id| transfers::open(id, now)).unwrap_or(-1)
}

/// A chunk arrived on `slot`; called per chunk, so primitives only.
extern "system" fn note(slot: jint, length: jlong, bytes: jlong, now: jlong) -> jfloat {
    transfers::note(slot, length, bytes, now)
}

extern "system" fn notice(listed: jint, waiting: jint, now: jlong) -> jint {
    transfers::notice(listed, waiting != 0, now)
}

/// The notification's title and text in one string, "title\ntext".
extern "system" fn notice_words(env: JNIEnv, _: JClass) -> jstring {
    transfers::notice_words(|s| java_string(&env, s))
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
extern "system" fn row(env: JNIEnv, _: JClass, id: JString) -> jstring {
    with_str(&env, &id, |id| transfers::row(id, |s| java_string(&env, s))).unwrap_or(std::ptr::null_mut())
}

extern "system" fn summary_line(env: JNIEnv, _: JClass, active: jint, queued: jint, failed: jint) -> jstring {
    transfers::summary_line(active, queued, failed, |s| java_string(&env, s))
}
