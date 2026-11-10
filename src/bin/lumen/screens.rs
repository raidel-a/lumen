//! Virtual screens: displays that exist only in memory.
//!
//! `CGVirtualDisplay` is private CoreGraphics API with no public replacement. It is declared by
//! name and driven through `msg_send`, in the same spirit as the `CGSConfigureDisplayEnabled`
//! binding in `lumen::native`.
//!
//! Two behaviours found in `spikes/virtual-screen` shape this module:
//!
//! - Pinning the origin with `CGConfigureDisplayOrigin` keeps the display registered in the Window
//!   Server until the process exits, so it is never pinned. The default placement (immediately
//!   right of the main display) is read back with `CGDisplayBounds` instead.
//! - A freshly created display is not yet known to AppKit. Callers must let the run loop turn
//!   before placing windows on it.

use crate::config::{Config, VirtualScreenConfig};
use core_foundation::base::{TCFType, CFRelease, CFTypeRef};
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_graphics::display::CGDisplay;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSDate, NSRunLoop, NSString, NSSize};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ScreenInfo {
    pub name: String,
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub pixel_width: u64,
    pub pixel_height: u64,
    pub scale: f64,
}

#[allow(dead_code)] // the helper objects are kept alive for the display's lifetime, never read
struct Screen {
    info: ScreenInfo,
    config: VirtualScreenConfig,
    display: *mut AnyObject,
    descriptor: *mut AnyObject,
    settings: *mut AnyObject,
    modes: *mut AnyObject,
}

impl Screen {
    fn matches(&self, config: &VirtualScreenConfig) -> bool {
        self.config.width == config.width
            && self.config.height == config.height
            && self.config.hidpi == config.hidpi
            && self.config.refresh == config.refresh
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // Release the display first and leave the descriptor, settings and mode array alone.
        // `-[CGVirtualDisplay dealloc]` reads its descriptor, so freeing that first crashes; and
        // the display keeps its own references to the settings, so freeing ours can double-free.
        // The leftovers are three small objects per removed screen, which is an acceptable trade
        // for a teardown that does not segfault.
        unsafe {
            let _: () = msg_send![self.display, release];
        }
    }
}

thread_local! {
    static SCREENS: RefCell<Vec<Screen>> = const { RefCell::new(Vec::new()) };
}

fn class(name: &str) -> &'static AnyClass {
    AnyClass::get(name).unwrap_or_else(|| panic!("missing Objective-C class {name}"))
}

/// Stable identity for a screen name, so macOS remembers its arrangement across runs.
fn serial_for(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash | 1
}

fn make_mode(width: u32, height: u32, refresh: f64) -> *mut AnyObject {
    unsafe {
        let alloc: *mut AnyObject = msg_send![class("CGVirtualDisplayMode"), alloc];
        msg_send![alloc, initWithWidth: width, height: height, refreshRate: refresh]
    }
}

/// Creates the display and waits until it is active and measurable.
fn create(config: &VirtualScreenConfig) -> Result<Screen, String> {
    let width = config.width.max(640);
    let height = config.height.max(480);
    let refresh = if config.refresh > 0.0 { config.refresh } else { 60.0 };
    // A 2x backing needs twice the pixels; the modes themselves stay in logical points.
    let max_width = if config.hidpi { width * 2 } else { width };
    let max_height = if config.hidpi { height * 2 } else { height };

    unsafe {
        let alloc: *mut AnyObject = msg_send![class("CGVirtualDisplayDescriptor"), alloc];
        let descriptor: *mut AnyObject = msg_send![alloc, init];
        let _: () = msg_send![descriptor, setName: &*NSString::from_str(&config.name)];
        let _: () = msg_send![descriptor, setMaxPixelsWide: max_width];
        let _: () = msg_send![descriptor, setMaxPixelsHigh: max_height];
        let _: () = msg_send![
            descriptor,
            setSizeInMillimeters: NSSize::new(width as f64 * 0.2646, height as f64 * 0.2646)
        ];
        let _: () = msg_send![descriptor, setVendorID: 0x4c55u32];
        let _: () = msg_send![descriptor, setProductID: 0x4d45u32];
        let _: () = msg_send![descriptor, setSerialNum: serial_for(&config.name)];

        let alloc: *mut AnyObject = msg_send![class("CGVirtualDisplay"), alloc];
        let display: *mut AnyObject = msg_send![alloc, initWithDescriptor: descriptor];

        let modes: *mut AnyObject = msg_send![class("NSMutableArray"), array];
        for (w, h) in mode_list(width, height) {
            let _: () = msg_send![modes, addObject: make_mode(w, h, refresh)];
        }

        let alloc: *mut AnyObject = msg_send![class("CGVirtualDisplaySettings"), alloc];
        let settings: *mut AnyObject = msg_send![alloc, init];
        // `hiDPI = 1` advertises HiDPI capability; the pixel size still comes from maxPixels. The
        // spike created displays reliably with 1 and not with 0.
        let _: () = msg_send![settings, setHiDPI: 1u32];
        let _: () = msg_send![settings, setModes: modes];
        let applied: bool = msg_send![display, applySettings: settings];
        if std::env::var_os("LUMEN_TRACE").is_some() {
            eprintln!("applySettings -> {applied}");
        }
        if !applied {
            for object in [settings, modes, descriptor, display] {
                let _: () = msg_send![object, release];
            }
            return Err("the display system rejected the virtual screen settings".to_string());
        }

        let id: u32 = msg_send![display, displayID];
        if id == 0 {
            for object in [settings, modes, descriptor, display] {
                let _: () = msg_send![object, release];
            }
            return Err("virtual screen was created without a display id".to_string());
        }

        // The id is assigned before the display has a mode, and `is_active` turns true before the
        // geometry is there. Wait for a real size, not just activation.
        let started = Instant::now();
        loop {
            let cg = CGDisplay::new(id);
            let bounds = cg.bounds();
            if cg.is_active()
                && bounds.size.width > 1.0
                && bounds.size.height > 1.0
                && cg.pixels_wide() > 0
            {
                break;
            }
            if std::env::var_os("LUMEN_TRACE").is_some() && started.elapsed().as_millis() % 500 < 12 {
                eprintln!(
                    "waiting for {id}: active={} bounds={}x{} pixels={}x{}",
                    cg.is_active(),
                    bounds.size.width,
                    bounds.size.height,
                    cg.pixels_wide(),
                    cg.pixels_high()
                );
            }
            if started.elapsed() > Duration::from_secs(5) {
                for object in [settings, modes, descriptor, display] {
                    let _: () = msg_send![object, release];
                }
                return Err(format!("virtual screen {id} never got a mode"));
            }
            // Pump the run loop rather than sleeping: the mode is applied through the main
            // queue, so a blocked main thread never sees it.
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
        }

        let info = live_info(&config.name, id);

        Ok(Screen {
            info,
            config: config.clone(),
            display,
            descriptor,
            settings,
            modes,
        })
    }
}

/// The requested size first, then a couple of smaller standard modes for System Settings.
fn mode_list(width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut modes = vec![(width, height), (1600, 900), (1280, 720)];
    modes.retain(|(w, h)| *w <= width && *h <= height);
    modes.dedup();
    modes
}

/// Reads the geometry from the display, so a rearrangement is reflected without a restart.
fn live_info(name: &str, id: u32) -> ScreenInfo {
    let display = CGDisplay::new(id);
    let bounds = display.bounds();
    let pixel_width = display.pixels_wide();
    ScreenInfo {
        name: name.to_string(),
        id,
        x: bounds.origin.x as i32,
        y: bounds.origin.y as i32,
        width: bounds.size.width.max(1.0) as u32,
        height: bounds.size.height.max(1.0) as u32,
        pixel_width,
        pixel_height: display.pixels_high(),
        scale: pixel_width as f64 / bounds.size.width.max(1.0),
    }
}

/// Drops screens whose display disappeared, for example across sleep.
pub fn prune() {
    SCREENS.with(|screens| {
        screens
            .borrow_mut()
            .retain(|screen| CGDisplay::new(screen.info.id).is_active());
    });
}

/// Creates a screen if it does not exist, or replaces it when its geometry changed.
pub fn ensure(config: &VirtualScreenConfig) -> Result<ScreenInfo, String> {
    if config.name.trim().is_empty() {
        return Err("a virtual screen needs a name".to_string());
    }
    let recreate = SCREENS.with(|screens| {
        screens
            .borrow()
            .iter()
            .position(|screen| screen.info.name == config.name)
            .map(|index| !screens.borrow()[index].matches(config))
    });
    match recreate {
        Some(true) => {
            remove(&config.name);
        }
        Some(false) => return info(&config.name).ok_or_else(|| "screen vanished".to_string()),
        None => {}
    }
    let screen = create(config)?;
    let info = screen.info.clone();
    SCREENS.with(|screens| screens.borrow_mut().push(screen));
    Ok(info)
}

/// Recreates every auto-create screen after launch, wake or a display change.
pub fn restore(config: &Config) {
    prune();
    for screen in &config.virtual_screens {
        if !screen.auto_create {
            continue;
        }
        if info(&screen.name).is_some() {
            continue;
        }
        match ensure(screen) {
            Ok(info) => {
                if std::env::var_os("LUMEN_TRACE").is_some() {
                    eprintln!("virtual screen restored: {} id={}", info.name, info.id);
                }
            }
            Err(error) => eprintln!("virtual screen {} failed: {error}", screen.name),
        }
    }
}

pub fn list() -> Vec<ScreenInfo> {
    SCREENS.with(|screens| {
        screens
            .borrow()
            .iter()
            .map(|screen| live_info(&screen.info.name, screen.info.id))
            .collect()
    })
}

pub fn info(name: &str) -> Option<ScreenInfo> {
    SCREENS.with(|screens| {
        screens
            .borrow()
            .iter()
            .find(|screen| screen.info.name == name)
            .map(|screen| live_info(&screen.info.name, screen.info.id))
    })
}

/// Drops the screen. The display disappears a moment later.
pub fn remove(name: &str) -> bool {
    SCREENS.with(|screens| {
        let mut screens = screens.borrow_mut();
        let Some(index) = screens.iter().position(|screen| screen.info.name == name) else {
            return false;
        };
        screens.remove(index);
        true
    })
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageDestinationCreateWithURL(
        url: *mut std::ffi::c_void,
        kind: *mut std::ffi::c_void,
        count: usize,
        options: *const std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn CGImageDestinationAddImage(
        destination: *mut std::ffi::c_void,
        image: *mut std::ffi::c_void,
        properties: *const std::ffi::c_void,
    );
    fn CGImageDestinationFinalize(destination: *mut std::ffi::c_void) -> bool;
    fn CGDisplayCreateImage(display: u32) -> *mut std::ffi::c_void;
    fn CGImageRelease(image: *mut std::ffi::c_void);
}

fn write_png(image: *mut std::ffi::c_void, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let url = CFURL::from_path(path, false).ok_or_else(|| "bad screenshot path".to_string())?;
    let kind = CFString::new("public.png");
    unsafe {
        let destination = CGImageDestinationCreateWithURL(
            url.as_concrete_TypeRef() as *mut std::ffi::c_void,
            kind.as_concrete_TypeRef() as *mut std::ffi::c_void,
            1,
            std::ptr::null(),
        );
        if destination.is_null() {
            return Err("could not create an image destination".to_string());
        }
        CGImageDestinationAddImage(destination, image, std::ptr::null());
        let finalized = CGImageDestinationFinalize(destination);
        CFRelease(destination as CFTypeRef);
        if !finalized {
            return Err("could not write the screenshot".to_string());
        }
    }
    Ok(())
}

/// Captures one frame of a screen and writes it as PNG.
pub fn screenshot(name: &str, path: Option<&Path>) -> Result<PathBuf, String> {
    let Some(info) = info(name) else {
        return Err(format!("no virtual screen named {name}"));
    };
    let image = unsafe { CGDisplayCreateImage(info.id) };
    if image.is_null() {
        return Err("the display did not return an image".to_string());
    }
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0);
            Config::support_dir()
                .join("agents")
                .join(name)
                .join(format!("screen-{stamp}.png"))
        }
    };
    let written = write_png(image, &path);
    unsafe { CGImageRelease(image) };
    written?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serials_are_stable_and_nonzero() {
        assert_eq!(serial_for("Agent"), serial_for("Agent"));
        assert_ne!(serial_for("Agent"), serial_for("Other"));
        assert_ne!(serial_for("Agent") & 1, 0);
    }

    #[test]
    fn modes_stay_within_the_requested_size() {
        assert_eq!(mode_list(1920, 1080), vec![(1920, 1080), (1600, 900), (1280, 720)]);
        assert_eq!(mode_list(1280, 720), vec![(1280, 720)]);
    }
}
