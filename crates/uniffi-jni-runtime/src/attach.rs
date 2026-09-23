/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use std::cell::RefCell;
use std::ffi::c_void;

use jni_sys::*;

/// NORI: detaches the thread it belongs to when that thread ends. Android aborts the process when a
/// thread still attached to the JVM exits, so a thread the core started, attached here for a callback,
/// has to be let go before it ends; it stays attached until then, so the next callback from it costs
/// no second attach.
struct Detach(*mut JavaVM);

impl Drop for Detach {
    fn drop(&mut self) {
        // Safety: the JavaVM lives as long as the process, and this thread attached itself to it.
        unsafe {
            ((**self.0).v1_2.DetachCurrentThread)(self.0);
        }
    }
}

thread_local! {
    static DETACH: RefCell<Option<Detach>> = const { RefCell::new(None) };
}

/// Attach the current thread to the JVM and run a closure
///
/// This is used to implement callback interfaces.
///
/// The closure must not return any JNI objects,
/// since their lifetime ends before this function returns.
///
/// # Safety
///
/// jvm must point to a valid JavaVM
pub unsafe fn attach_current_thread<F, T>(jvm: *mut JavaVM, f: F) -> T
where
    F: FnOnce(*mut JNIEnv) -> T,
{
    let mut env: *mut JNIEnv = ::std::ptr::null_mut();
    // Safety:
    // We're using the JNI API correctly
    unsafe {
        let penv = std::ptr::from_mut(&mut env).cast::<*mut c_void>();
        // NORI: a thread that is attached already (every Java thread, and one attached here before)
        // is used as it is, and never detached here.
        let mut detach_after = false;
        match ((**jvm).v1_2.GetEnv)(jvm, penv, JNI_VERSION_1_2) {
            JNI_OK => {}
            JNI_EDETACHED => {
                if ((**jvm).v1_2.AttachCurrentThread)(jvm, penv, ::std::ptr::null_mut()) != JNI_OK {
                    panic!("AttachCurrentThread failed");
                }
                // A thread already tearing down its thread locals cannot keep a detacher, so it lets go
                // of the JVM straight after this call.
                detach_after = DETACH.try_with(|d| *d.borrow_mut() = Some(Detach(jvm))).is_err();
            }
            _ => panic!("GetEnv failed"),
        }

        // Create a new local frame, needed to generate JNI references.
        // Create capacity for 16 references, which is what JNA does
        if ((**env).v1_2.PushLocalFrame)(env, 16) != 0 {
            panic!("Out of memory: PushLocalFrame failed");
        }
        let closure_result = f(env);
        ((**env).v1_2.PopLocalFrame)(env, std::ptr::null_mut());
        if detach_after {
            ((**jvm).v1_2.DetachCurrentThread)(jvm);
        }
        closure_result
    }
}

/// NORI: whether this thread was attached to the JVM by [`attach_current_thread`] and will be detached when
/// it ends. For tests.
pub fn attached_here() -> bool {
    DETACH.try_with(|d| d.borrow().is_some()).unwrap_or(false)
}
