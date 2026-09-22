//! How a page looks, as Kotlin reaches it: `nori_look` over JNI with plain int arrays - a cover's
//! pixels in, its colours and wash out - so nothing is boxed, serialised or copied twice.

use jni::objects::{JClass, JIntArray};
use jni::sys::{jboolean, jint};
use jni::JNIEnv;
use nori_look::cover::{derive, WASH_OUT};

/// The page colours for `w` x `h` ARGB `pixels`. `out` holds edge, background, on, accent and the melt
/// colour, then the wash (`WASH_OUT`² pixels). Returns whether there is a wash (not on AMOLED black).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_derive(
    env: JNIEnv, _: JClass, pixels: JIntArray, w: jint, h: jint, dark: jboolean, amoled: jboolean, out: JIntArray,
) -> jboolean {
    let (w, h) = (w.max(0) as usize, h.max(0) as usize);
    if w == 0 || h == 0 || env.get_array_length(&pixels).unwrap_or(0) as usize != w * h {
        return 0;
    }
    let mut px = vec![0i32; w * h];
    if env.get_int_array_region(&pixels, 0, &mut px).is_err() {
        return 0;
    }
    let px: Vec<u32> = px.into_iter().map(|p| p as u32).collect();
    let c = derive(&px, w, h, dark != 0, amoled != 0);
    let head = [c.edge, c.background, c.on, c.accent, c.wash_edge].map(|v| v as i32);
    if env.set_int_array_region(&out, 0, &head).is_err() {
        return 0;
    }
    match c.wash {
        Some(wash) if env.get_array_length(&out).unwrap_or(0) as usize >= head.len() + WASH_OUT * WASH_OUT => {
            let wash: Vec<i32> = wash.into_iter().map(|p| p as i32).collect();
            env.set_int_array_region(&out, head.len() as i32, &wash).is_ok() as jboolean
        }
        _ => 0,
    }
}

/// A theme's tones from one seed colour, into `out` (11 colours; see `nori_look::theme::seeded`).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_tones(env: JNIEnv, _: JClass, seed: jint, dark: jboolean, out: JIntArray) {
    let tones = nori_look::theme::seeded(seed as u32, dark != 0).map(|v| v as i32);
    let _ = env.set_int_array_region(&out, 0, &tones);
}

