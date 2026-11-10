//! Spike: does CGVirtualDisplay still work on this macOS, what does it cost, and can we capture it?
//!
//! Creates a 1920x1080 @ 60 virtual display from the private CoreGraphics classes, pins its origin
//! to the right of the main display, puts a magenta window on it, captures the framebuffer and
//! writes a BMP next to a few measurements. Everything is torn down before exit.

use core_graphics::display::{CGConfigureOption, CGDisplay};
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSDate, NSPoint, NSRect, NSRunLoop, NSSize, NSString,
};
use std::time::{Duration, Instant};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const REFRESH: f64 = 60.0;

fn class(name: &str) -> &'static AnyClass {
    AnyClass::get(name).unwrap_or_else(|| panic!("missing class {name}"))
}

fn make_mode(width: u32, height: u32, refresh: f64) -> *mut AnyObject {
    unsafe {
        let alloc: *mut AnyObject = msg_send![class("CGVirtualDisplayMode"), alloc];
        msg_send![alloc, initWithWidth: width, height: height, refreshRate: refresh]
    }
}

struct Created {
    display: *mut AnyObject,
    descriptor: *mut AnyObject,
    settings: *mut AnyObject,
    modes: *mut AnyObject,
}

fn create_display() -> Created {
    unsafe {
        let descriptor: *mut AnyObject = msg_send![class("CGVirtualDisplayDescriptor"), alloc];
        let descriptor: *mut AnyObject = msg_send![descriptor, init];
        let _: () = msg_send![descriptor, setName: &*NSString::from_str("Lumen Spike")];
        let _: () = msg_send![descriptor, setMaxPixelsWide: WIDTH];
        let _: () = msg_send![descriptor, setMaxPixelsHigh: HEIGHT];
        let _: () = msg_send![
            descriptor,
            setSizeInMillimeters: NSSize::new(500.0, 281.0)
        ];
        let _: () = msg_send![descriptor, setVendorID: 0x1e6du32];
        let _: () = msg_send![descriptor, setProductID: 0x5bf8u32];
        let _: () = msg_send![descriptor, setSerialNum: 0x0007_4f58u32];

        let alloc: *mut AnyObject = msg_send![class("CGVirtualDisplay"), alloc];
        let display: *mut AnyObject = msg_send![alloc, initWithDescriptor: descriptor];

        let modes: *mut AnyObject = msg_send![class("NSMutableArray"), array];
        for (w, h) in [(WIDTH, HEIGHT), (1280, 720)] {
            let _: () = msg_send![modes, addObject: make_mode(w, h, REFRESH)];
        }

        let settings: *mut AnyObject = msg_send![class("CGVirtualDisplaySettings"), alloc];
        let settings: *mut AnyObject = msg_send![settings, init];
        let _: () = msg_send![settings, setHiDPI: 1u32];
        let _: () = msg_send![settings, setModes: modes];
        let applied: bool = msg_send![display, applySettings: settings];
        println!("applySettings: {applied}");
        Created {
            display,
            descriptor,
            settings,
            modes,
        }
    }
}

fn display_id(display: *mut AnyObject) -> u32 {
    unsafe { msg_send![display, displayID] }
}

fn active_ids() -> Vec<u32> {
    CGDisplay::active_displays().unwrap_or_default()
}

fn wait_for(id: u32, present: bool, limit: Duration) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if active_ids().contains(&id) == present {
            return Some(start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn pin_origin(id: u32) -> Result<(i32, i32), String> {
    let main = CGDisplay::main();
    let bounds = main.bounds();
    let x = (bounds.origin.x + bounds.size.width) as i32;
    let y = bounds.origin.y as i32;
    let config = main.begin_configuration().map_err(|e| format!("begin: {e}"))?;
    CGDisplay::new(id)
        .configure_display_origin(&config, x, y)
        .map_err(|e| format!("origin: {e}"))?;
    let option = match std::env::var("LUMEN_PIN").as_deref() {
        Ok("session") => CGConfigureOption::ConfigureForSession,
        Ok("app") => CGConfigureOption::ConfigureForAppOnly,
        _ => CGConfigureOption::ConfigurePermanently,
    };
    main.complete_configuration(&config, option)
        .map_err(|e| format!("complete: {e}"))?;
    Ok((x, y))
}

fn diagnostics(id: u32) {
    let display = CGDisplay::new(id);
    println!(
        "flags: main={} online={} active={} mirror_set={} mirrors={}",
        display.is_main(),
        display.is_online(),
        display.is_active(),
        display.is_in_mirror_set(),
        display.mirrors_display()
    );
}

fn show_window(mtm: MainThreadMarker, x: f64, y: f64) -> objc2::rc::Retained<NSWindow> {
    unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            NSRect::new(NSPoint::new(x + 80.0, y + 80.0), NSSize::new(800.0, 500.0)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        );
        window.setTitle(&NSString::from_str("Lumen spike content"));
        window.setBackgroundColor(Some(&NSColor::magentaColor()));
        window.orderFrontRegardless();
        window
    }
}

fn pump(seconds: f64) {
    unsafe {
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(seconds));
    }
}

/// Writes a 24-bit BMP so the capture can be eyeballed without extra crates.
fn write_bmp(
    path: &str,
    width: usize,
    height: usize,
    bgra: &[u8],
    stride: usize,
) -> std::io::Result<()> {
    use std::io::Write;
    let row_bytes = width * 3;
    let padding = (4 - row_bytes % 4) % 4;
    let image_size = (row_bytes + padding) * height;
    let file_size = 54 + image_size;
    let mut out = Vec::with_capacity(file_size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(image_size as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for row in (0..height).rev() {
        let start = row * stride;
        for column in 0..width {
            let pixel = &bgra[start + column * 4..start + column * 4 + 4];
            out.extend_from_slice(&[pixel[0], pixel[1], pixel[2]]);
        }
        out.extend(std::iter::repeat(0).take(padding));
    }
    let mut file = std::fs::File::create(path)?;
    file.write_all(&out)
}

fn capture(id: u32, path: &str) -> Option<(u64, u64, f64)> {
    let started = Instant::now();
    let image = CGDisplay::new(id).image()?;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let width = image.width();
    let height = image.height();
    let data = image.data();
    let bytes = data.bytes();
    let stride = image.bytes_per_row();
    let mut magenta = 0u64;
    for row in 0..height {
        let start = row * stride;
        for column in 0..width {
            let i = start + column * 4;
            if i + 3 < bytes.len() {
                let (b, g, r) = (bytes[i], bytes[i + 1], bytes[i + 2]);
                if r > 200 && g < 60 && b > 200 {
                    magenta += 1;
                }
            }
        }
    }
    let fraction = magenta as f64 / (width * height) as f64 * 100.0;
    let _ = write_bmp(path, width, height, bytes, stride);
    Some((width as u64, height as u64, fraction)).map(|(w, h, f)| {
        println!("capture: {w}x{h} in {elapsed:.1} ms, magenta {f:.1}%");
        (w, h, f)
    })
}

fn cpu_seconds() -> f64 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0 {
            usage.ru_utime.tv_sec as f64
                + usage.ru_utime.tv_usec as f64 / 1e6
                + usage.ru_stime.tv_sec as f64
                + usage.ru_stime.tv_usec as f64 / 1e6
        } else {
            0.0
        }
    }
}

fn benchmark(id: u32) {
    let cpu_start = cpu_seconds();
    let start = Instant::now();
    let mut frames = 0u32;
    while start.elapsed() < Duration::from_secs(2) {
        if CGDisplay::new(id).image().is_some() {
            frames += 1;
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu_start;
    println!(
        "capture benchmark: {frames} frames in {elapsed:.2}s = {:.1} fps, cpu {:.0}% of one core",
        frames as f64 / elapsed,
        cpu / elapsed * 100.0
    );
}

fn print_methods(name: &str) {
    use std::ffi::CStr;
    extern "C" {
        fn class_copyMethodList(cls: *const AnyObject, out_count: *mut u32) -> *mut *const AnyObject;
        fn method_getName(method: *const AnyObject) -> *const AnyObject;
        fn sel_getName(sel: *const AnyObject) -> *const std::ffi::c_char;
        fn free(ptr: *mut std::ffi::c_void);
    }
    unsafe {
        let cls = class(name);
        let mut count = 0u32;
        let methods = class_copyMethodList(cls as *const AnyClass as *const AnyObject, &mut count);
        let mut names = Vec::new();
        for index in 0..count as usize {
            let method = *methods.add(index);
            let sel = method_getName(method);
            names.push(CStr::from_ptr(sel_getName(sel)).to_string_lossy().into_owned());
        }
        free(methods as *mut std::ffi::c_void);
        names.sort();
        println!("{name}: {}", names.join(", "));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--methods") {
        for name in [
            "CGVirtualDisplay",
            "CGVirtualDisplayDescriptor",
            "CGVirtualDisplaySettings",
            "CGVirtualDisplayMode",
        ] {
            print_methods(name);
        }
        return;
    }
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    unsafe { app.finishLaunching() };

    println!("pid: {}", std::process::id());
    let before = active_ids();
    println!("active displays before: {before:?}");

    let started = Instant::now();
    let created = create_display();
    let display = created.display;
    let id = display_id(display);
    println!("virtual display id: {id}");

    match wait_for(id, true, Duration::from_secs(2)) {
        Some(elapsed) => println!("appeared in {elapsed:?}"),
        None => {
            println!("FAILED: display never appeared in active displays");
            return;
        }
    }
    println!("total create: {:?}", started.elapsed());

    let cg = CGDisplay::new(id);
    let bounds = cg.bounds();
    println!(
        "bounds: origin {:.0},{:.0} size {:.0}x{:.0}",
        bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height
    );
    println!("pixels: {}x{}", cg.pixels_wide(), cg.pixels_high());

    if !args.iter().any(|arg| arg == "--no-pin") {
        match pin_origin(id) {
            Ok((x, y)) => println!("pinned origin: {x},{y}"),
            Err(error) => println!("pin origin failed: {error}"),
        }
    }
    let bounds = CGDisplay::new(id).bounds();
    println!(
        "bounds after pin: {:.0},{:.0}",
        bounds.origin.x, bounds.origin.y
    );

    if args.iter().any(|arg| arg == "--quick") {
        std::thread::sleep(Duration::from_secs(1));
        unsafe {
            for object in [created.settings, created.modes, created.descriptor, display] {
                let _: () = msg_send![object, release];
            }
        }
        pump(3.0);
        println!("quick: present after release = {}", active_ids().contains(&id));
        return;
    }

    diagnostics(id);
    // Let AppKit notice the new display before placing a window on it.
    pump(1.0);
    let window = show_window(mtm, bounds.origin.x, bounds.origin.y);
    unsafe {
        window.setFrameOrigin(NSPoint::new(bounds.origin.x + 80.0, bounds.origin.y + 80.0));
    }
    println!("window frame: {:?}", window.frame());
    pump(1.5);

    let path = "/private/var/folders/b9/y1fq0gcs3z97dq_zt4klqp4w0000gn/T/opencode/lumen-spike.bmp";
    match capture(id, path) {
        Some(_) => println!("wrote {path}"),
        None => println!("CGDisplayCreateImage returned nil (TCC or unsupported)"),
    }
    // Warm capture, to separate first-call setup from steady-state cost.
    let _ = capture(id, "/private/var/folders/b9/y1fq0gcs3z97dq_zt4klqp4w0000gn/T/opencode/lumen-spike-2.bmp");
    println!("main display capture:");
    let _ = capture(CGDisplay::main().id, "/private/var/folders/b9/y1fq0gcs3z97dq_zt4klqp4w0000gn/T/opencode/lumen-main.bmp");

    window.orderOut(None);
    benchmark(id);
    println!("sleeping 6s with the display up for external footprint sampling");
    std::thread::sleep(Duration::from_secs(6));

    unsafe {
        for object in [created.settings, created.modes, created.descriptor, display] {
            let _: () = msg_send![object, release];
        }
    }
    pump(3.0);
    match wait_for(id, false, Duration::from_secs(2)) {
        Some(elapsed) => println!("removed in {elapsed:?}"),
        None => println!("WARNING: display still present after release"),
    }
    println!("active displays after: {:?}", active_ids());
}
