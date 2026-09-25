//! The two answers about the sound chain the settings ask for as they are read: whether the chain has
//! anything to do, and the pre-amp in effect. The chain itself runs inside the Rust player
//! (crates/android/src/player.rs).

use jni::objects::{JClass, JFloatArray, JIntArray};
use jni::sys::{jboolean, jfloat};
use jni::JNIEnv;

use crate::{native, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/Dsp",
    methods: &[
        native!(c"effectivePreampDb", c"(ZFZ[I[F)F", effective_preamp_db),
        native!(c"soundOn", c"(ZFFZZ)Z", sound_on),
    ],
};

/// The pre-amp in effect (`nori_core::settings::effective_preamp_db`), worked out once per settings but
/// for each step of a band's drag on the equalizer screen: the bands' kinds and gains come in as two
/// primitive arrays.
extern "system" fn effective_preamp_db(
    env: JNIEnv, _: JClass, eq_enabled: jboolean, eq_preamp_db: jfloat, automatic: jboolean, kinds: JIntArray, gains: JFloatArray,
) -> jfloat {
    let n = env.get_array_length(&kinds).unwrap_or(0).max(0) as usize;
    let (mut k, mut g) = (vec![0i32; n], vec![0f32; n]);
    if env.get_int_array_region(&kinds, 0, &mut k).is_err() || env.get_float_array_region(&gains, 0, &mut g).is_err() {
        return 0.0;
    }
    nori_core::settings::effective_preamp_db(eq_enabled != 0, (automatic == 0).then_some(eq_preamp_db), k.into_iter().zip(g))
}

/// Whether the sound chain has anything to do (see `nori_player::sound::sound_on`). Read once per
/// settings change, possibly while a screen is drawn, so primitives in and out.
extern "system" fn sound_on(eq_enabled: jboolean, crossfeed_db: jfloat, balance: jfloat, mono: jboolean, limiter: jboolean) -> jboolean {
    nori_player::sound::sound_on(eq_enabled != 0, crossfeed_db, balance, mono != 0, limiter != 0) as jboolean
}
