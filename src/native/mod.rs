//! Display signal control.
//!
//! Monitors with "Auto Input Switch" (most LG displays) jump to another input when the active
//! one loses its signal. Releasing a display means telling macOS to stop driving its output; the
//! monitor then picks up whatever else is live. Reclaiming it brings the output back, and the
//! monitor follows the fresh signal home.
//!
//! `CGSConfigureDisplayEnabled` is private, so it is resolved at runtime.

use std::ffi::c_void;

type CGDisplayConfigRef = *mut c_void;
type CGError = i32;
type ConfigureEnabled = unsafe extern "C" fn(CGDisplayConfigRef, u32, i32) -> CGError;

const K_CG_CONFIGURE_FOR_SESSION: u32 = 1;

unsafe extern "C" {
    fn CGBeginDisplayConfiguration(config: *mut CGDisplayConfigRef) -> CGError;
    fn CGCompleteDisplayConfiguration(config: CGDisplayConfigRef, option: u32) -> CGError;
    fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
}

/// `RTLD_DEFAULT` from dlfcn.h.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

fn configure_enabled() -> Option<ConfigureEnabled> {
    unsafe {
        let symbol = c"CGSConfigureDisplayEnabled";
        let pointer = dlsym(RTLD_DEFAULT, symbol.as_ptr());
        if pointer.is_null() {
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, ConfigureEnabled>(pointer))
        }
    }
}

/// Stops (or resumes) the display output for this session.
pub fn set_enabled(display: u32, enabled: bool) -> bool {
    let Some(configure) = configure_enabled() else {
        return false;
    };
    unsafe {
        let mut config: CGDisplayConfigRef = std::ptr::null_mut();
        if CGBeginDisplayConfiguration(&mut config) != 0 {
            return false;
        }
        if configure(config, display, i32::from(enabled)) != 0 {
            return false;
        }
        CGCompleteDisplayConfiguration(config, K_CG_CONFIGURE_FOR_SESSION) == 0
    }
}

/// True when the private symbol is available, so the menu can offer release/reclaim.
pub fn is_supported() -> bool {
    configure_enabled().is_some()
}
