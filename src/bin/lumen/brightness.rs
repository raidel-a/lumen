//! Brightness for the built-in display.
//!
//! The internal panel has no DDC path. macOS exposes its backlight through the private
//! DisplayServices framework; the symbols are resolved at runtime so the app still starts if the
//! framework or a symbol disappears.

use std::ffi::{c_void, CString};
use std::sync::OnceLock;

type GetBrightness = unsafe extern "C" fn(u32, *mut f32) -> i32;
type SetBrightness = unsafe extern "C" fn(u32, f32) -> i32;
type CanChangeBrightness = unsafe extern "C" fn(u32) -> bool;

struct Api {
    get: GetBrightness,
    set: SetBrightness,
    can_change: CanChangeBrightness,
}

static API: OnceLock<Option<Api>> = OnceLock::new();

fn api() -> Option<&'static Api> {
    API.get_or_init(|| unsafe {
        let path = CString::new(
            "/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices",
        )
        .ok()?;
        let handle = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if handle.is_null() {
            return None;
        }
        // The handle is deliberately not closed; the resolved function pointers must stay valid
        // for the life of the process.
        let get = libc::dlsym(handle, c"DisplayServicesGetBrightness".as_ptr()) as *mut c_void;
        let set = libc::dlsym(handle, c"DisplayServicesSetBrightness".as_ptr()) as *mut c_void;
        let can = libc::dlsym(handle, c"DisplayServicesCanChangeBrightness".as_ptr()) as *mut c_void;
        if get.is_null() || set.is_null() || can.is_null() {
            return None;
        }
        Some(Api {
            get: std::mem::transmute::<*mut c_void, GetBrightness>(get),
            set: std::mem::transmute::<*mut c_void, SetBrightness>(set),
            can_change: std::mem::transmute::<*mut c_void, CanChangeBrightness>(can),
        })
    })
    .as_ref()
}

pub fn available() -> bool {
    api().is_some()
}

fn percent(value: f32) -> u16 {
    if !value.is_finite() {
        return 0;
    }
    (value.clamp(0.0, 1.0) * 100.0).round() as u16
}

pub fn get_percent(display: u32) -> Option<u16> {
    let api = api()?;
    let mut value = 0.0f32;
    let result = unsafe { (api.get)(display, &mut value) };
    if result != 0 || !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(percent(value))
}

pub fn can_change(display: u32) -> bool {
    api().map(|api| unsafe { (api.can_change)(display) }).unwrap_or(false)
}

pub fn set_percent(display: u32, value: u16) -> Result<(), String> {
    let api = api().ok_or_else(|| "DisplayServices brightness is unavailable".to_string())?;
    if !can_change(display) {
        return Err("the display does not accept brightness changes".to_string());
    }
    let unit = f32::from(value.min(100)) / 100.0;
    let result = unsafe { (api.set)(display, unit) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!("DisplayServicesSetBrightness returned {result}"))
    }
}

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn maps_unit_brightness_to_percent() {
        assert_eq!(percent(0.0), 0);
        assert_eq!(percent(0.5), 50);
        assert_eq!(percent(0.5388), 54);
        assert_eq!(percent(1.0), 100);
    }

    #[test]
    fn clamps_out_of_range_and_non_finite_values() {
        assert_eq!(percent(-0.2), 0);
        assert_eq!(percent(1.4), 100);
        assert_eq!(percent(f32::NAN), 0);
    }
}
