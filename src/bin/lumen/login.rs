//! Login item registration through `SMAppService` (macOS 13+).
//!
//! ServiceManagement has no objc2 bindings here, so the class is looked up at runtime after
//! dlopen. Registration requires a signed bundle; an ad-hoc signature may be refused, and the
//! error is surfaced in Settings rather than swallowed.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
    Unavailable,
    Unknown(isize),
}

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
}

fn loaded() -> bool {
    static LOADED: OnceLock<bool> = OnceLock::new();
    *LOADED.get_or_init(|| unsafe {
        let path = CString::new(
            "/System/Library/Frameworks/ServiceManagement.framework/ServiceManagement",
        )
        .expect("valid path");
        !dlopen(path.as_ptr(), 1).is_null()
    })
}

fn service() -> Option<Retained<AnyObject>> {
    if !loaded() {
        return None;
    }
    let class = AnyClass::get("SMAppService")?;
    let raw: *mut AnyObject = unsafe { msg_send![class, mainAppService] };
    unsafe { Retained::retain(raw) }
}

pub fn status() -> Status {
    let Some(service) = service() else {
        return Status::Unavailable;
    };
    let raw: isize = unsafe { msg_send![&*service, status] };
    if std::env::var_os("LUMEN_TRACE").is_some() {
        eprintln!("login status raw={raw}");
    }
    match raw {
        0 => Status::NotRegistered,
        1 => Status::Enabled,
        2 => Status::RequiresApproval,
        3 => Status::NotFound,
        other => Status::Unknown(other),
    }
}

pub fn register() -> Result<(), String> {
    let Some(service) = service() else {
        return Err("ServiceManagement is unavailable on this macOS".into());
    };
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let ok: bool = unsafe { msg_send![&*service, registerAndReturnError: &mut error] };
    if ok {
        Ok(())
    } else {
        Err(error_text(error))
    }
}

pub fn unregister() -> Result<(), String> {
    let Some(service) = service() else {
        return Err("ServiceManagement is unavailable on this macOS".into());
    };
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let ok: bool = unsafe { msg_send![&*service, unregisterAndReturnError: &mut error] };
    if ok {
        Ok(())
    } else {
        Err(error_text(error))
    }
}

/// Opens System Settings on the Login Items pane (for the approval case).
pub fn open_login_items() {
    if !loaded() {
        return;
    }
    if let Some(class) = AnyClass::get("SMAppService") {
        let _: () = unsafe { msg_send![class, openSystemSettingsLoginItems] };
    }
}

fn error_text(error: *mut AnyObject) -> String {
    if error.is_null() {
        return "registration failed".into();
    }
    unsafe {
        let description: *mut AnyObject = msg_send![error, localizedDescription];
        if description.is_null() {
            "registration failed".into()
        } else {
            let text: *mut AnyObject = description;
            let c: *const c_char = msg_send![text, UTF8String];
            CStr::from_ptr(c).to_string_lossy().into_owned()
        }
    }
}
