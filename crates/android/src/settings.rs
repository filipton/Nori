//! The equalizer screen over plain JNI: each figure is asked on every step of a slider's drag (and the
//! limiter's every 120 ms), and through uniffi each one was a call status, a buffer and a cleaner on the
//! Java side for one short string. Here it is primitives in and one Java string out; the words are
//! `nori_core::fmt`'s and the edits `nori_core::settings_store`'s.

use jni::objects::{JClass, JFloatArray};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring};
use jni::JNIEnv;
use nori_core::fmt;
use nori_core::settings::{EqLevel, SoundBand};

use crate::{java_string, native, Class};

pub(crate) static EQ_WORDS: Class = Class {
    name: c"dev/nori/music/settings/EqWords",
    methods: &[
        native!(c"signedDb", c"(F)Ljava/lang/String;", signed_db),
        native!(c"preamp", c"(FZ)Ljava/lang/String;", preamp),
        native!(c"balance", c"(F)Ljava/lang/String;", balance),
        native!(c"ceiling", c"(F)Ljava/lang/String;", ceiling),
        native!(c"reduction", c"(F)Ljava/lang/String;", reduction),
        native!(c"crossfeed", c"(F)Ljava/lang/String;", crossfeed),
        native!(c"hzTitle", c"(F)Ljava/lang/String;", hz_title),
        native!(c"shape", c"(ZF)Ljava/lang/String;", shape),
        native!(c"bandName", c"(IFI)Ljava/lang/String;", band_name),
        native!(c"freqToSlider", c"(F)F", freq_to_slider),
        native!(c"sliderToFreq", c"(F)F", slider_to_freq),
    ],
};

pub(crate) static SOUND_EDIT: Class = Class {
    name: c"dev/nori/music/settings/SoundEdit",
    methods: &[native!(c"setBand", c"(I[F)I", set_band), native!(c"setLevel", c"(IF)J", set_level)],
};

extern "system" fn signed_db(env: JNIEnv, _: JClass, db: jfloat) -> jstring {
    java_string(&env, &fmt::signed_db(db))
}

extern "system" fn preamp(env: JNIEnv, _: JClass, db: jfloat, automatic: jboolean) -> jstring {
    java_string(&env, &fmt::eq_preamp(db, automatic != 0))
}

extern "system" fn balance(env: JNIEnv, _: JClass, balance: jfloat) -> jstring {
    java_string(&env, &fmt::eq_balance(balance))
}

extern "system" fn ceiling(env: JNIEnv, _: JClass, db: jfloat) -> jstring {
    java_string(&env, &fmt::eq_ceiling(db))
}

extern "system" fn reduction(env: JNIEnv, _: JClass, db: jfloat) -> jstring {
    java_string(&env, &fmt::eq_reduction(db))
}

extern "system" fn crossfeed(env: JNIEnv, _: JClass, db: jfloat) -> jstring {
    java_string(&env, &fmt::eq_crossfeed(db))
}

extern "system" fn hz_title(env: JNIEnv, _: JClass, freq: jfloat) -> jstring {
    java_string(&env, &fmt::eq_hz_title(freq))
}

extern "system" fn shape(env: JNIEnv, _: JClass, slope: jboolean, q: jfloat) -> jstring {
    java_string(&env, &fmt::eq_shape(slope != 0, q))
}

/// A band's label (`settings::band_label`) from the three things it reads.
extern "system" fn band_name(env: JNIEnv, _: JClass, kind: jint, freq: jfloat, channel: jint) -> jstring {
    let band = SoundBand { kind, freq, gain_db: 0.0, q: 1.0, channel };
    java_string(&env, &nori_core::settings::band_label(&band))
}

extern "system" fn freq_to_slider(freq: jfloat) -> jfloat {
    fmt::eq_freq_to_slider(freq)
}

extern "system" fn slider_to_freq(x: jfloat) -> jfloat {
    fmt::eq_slider_to_freq(x)
}

/// `settings_store::edit_band` on every step of a slider: the band comes in as `[kind, freq, gain, q,
/// channel]` and goes back out the same way as it was kept. A drag builds no settings record and sends
/// none across. Returns what the player has to apply again, or -1 when nothing changed.
extern "system" fn set_band(env: JNIEnv, _: JClass, index: jint, band: JFloatArray) -> jint {
    let mut b = [0f32; 5];
    if index < 0 || env.get_float_array_region(&band, 0, &mut b).is_err() {
        return -1;
    }
    let asked = SoundBand { kind: b[0] as i32, freq: b[1], gain_db: b[2], q: b[3], channel: b[4] as i32 };
    let Some((effect, kept)) = nori_core::settings_store::edit_band(index as u32, asked) else { return -1 };
    let out = [kept.kind as f32, kept.freq, kept.gain_db, kept.q, kept.channel as f32];
    if env.set_float_array_region(&band, 0, &out).is_err() {
        return -1;
    }
    effect as jint
}

/// `settings_store::edit_level` (`level` an [`EqLevel`] ordinal): the value as it was kept as float bits
/// in the high 32, what the player has to apply again in the low; -1 when nothing changed.
extern "system" fn set_level(level: jint, value: jfloat) -> jlong {
    let level = match level {
        0 => EqLevel::Preamp,
        1 => EqLevel::Balance,
        2 => EqLevel::Limiter,
        3 => EqLevel::Crossfeed,
        4 => EqLevel::ReplayGainPreamp,
        _ => return -1,
    };
    match nori_core::settings_store::edit_level(level, value) {
        Some((effect, kept)) => ((kept.to_bits() as jlong) << 32) | effect as jlong,
        None => -1,
    }
}
