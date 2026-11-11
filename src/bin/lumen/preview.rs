//! Live preview windows (picture-in-picture) for virtual screens.
//!
//! Frames come from ScreenCaptureKit rather than a polling `CGDisplayCreateImage`: the stream runs
//! on its own queue, is downscaled to the window, and is stopped while the window is occluded.
//!
//! The objc2-screen-capture-kit 0.2.2 bindings match this app's objc2 0.5 generation but leave a
//! few selectors out. Those are called through `msg_send!` here, in one place: `SCDisplay.displayID`,
//! `SCStream.addStreamOutput:type:sampleHandlerQueue:error:`, `setMinimumFrameInterval:` and
//! `SCScreenshotManager.captureImageWithFilter:configuration:completionHandler:`.

use crate::config::PreviewFrame;
use block2::RcBlock;
use core_foundation::base::{CFRetain, CFTypeRef};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSColor, NSFont, NSFontWeightSemibold, NSFloatingWindowLevel,
    NSImage, NSTextAlignment, NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowCollectionBehavior, NSWindowOcclusionState, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSError, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
};
use objc2_screen_capture_kit::{
    SCDisplay, SCScreenshotManager, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamOutput, SCStreamOutputType, SCWindow, SCContentFilter,
};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CString};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// At most one preview per virtual screen, and no more than the screen limit.
const MAX_PREVIEWS: usize = crate::screens::MAX_SCREENS;
const DEFAULT_WIDTH: f64 = 0.30;
const MIN_WIDTH: f64 = 320.0;
const MAX_WIDTH: f64 = 960.0;
const MARGIN: f64 = 24.0;
const CHROME: f64 = 30.0;

thread_local! {
    static PREVIEWS: RefCell<Vec<Preview>> = const { RefCell::new(Vec::new()) };
    static HEALTH: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
    static THUMBNAILS: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
}

struct Preview {
    name: String,
    window: Retained<NSWindow>,
    surface: Retained<PreviewSurface>,
    placeholder: Retained<NSTextField>,
    pause: Retained<NSButton>,
    stream: Option<Retained<SCStream>>,
    output: Option<Retained<PreviewOutput>>,
    queue: *mut c_void,
    /// The frame currently shown. Owned by this record and also referenced by the layer.
    frame: *mut c_void,
    started: bool,
    paused: bool,
    last_frame: Option<Instant>,
    status: Option<String>,
}

impl Drop for Preview {
    fn drop(&mut self) {
        unsafe {
            if let Some(stream) = &self.stream {
                let _: () = msg_send![&**stream, stopCaptureWithCompletionHandler: std::ptr::null::<AnyObject>()];
            }
            if !self.queue.is_null() {
                dispatch_release(self.queue);
            }
            if !self.frame.is_null() {
                CGImageRelease(self.frame);
            }
        }
    }
}

fn mtm() -> MainThreadMarker {
    unsafe { MainThreadMarker::new_unchecked() }
}

fn previews<R>(f: impl FnOnce(&mut Vec<Preview>) -> R) -> R {
    PREVIEWS.with(|previews| f(&mut previews.borrow_mut()))
}

// --- Public API -------------------------------------------------------------------------------

/// Opens a preview for `name`, or brings it to the front if it is already open.
pub fn open(name: &str) -> Result<(), String> {
    if previews(|previews| previews.iter().any(|preview| preview.name == name)) {
        if let Some(window) = previews(|previews| {
            previews
                .iter()
                .find(|preview| preview.name == name)
                .map(|preview| preview.window.clone())
        }) {
            unsafe { window.orderFrontRegardless() };
        }
        crate::after_change();
        return Ok(());
    }
    let Some(info) = crate::screens::info(name) else {
        return Err(format!("no virtual screen named {name}"));
    };
    if previews(|previews| previews.len()) >= MAX_PREVIEWS {
        return Err(format!("at most {MAX_PREVIEWS} previews can be open"));
    }
    let Some(actions) = crate::ui::target() else {
        return Err("the app is not ready for a preview".to_string());
    };

    let (window, surface, placeholder, pause) = build_window(&info, name, &actions)?;
    previews(|previews| {
        previews.push(Preview {
            name: info.name.clone(),
            window,
            surface,
            placeholder,
            pause,
            stream: None,
            output: None,
            queue: std::ptr::null_mut(),
            frame: std::ptr::null_mut(),
            started: false,
            paused: false,
            last_frame: None,
            status: Some("Loading preview…".to_string()),
        });
    });
    start_health_timer();
    if !screen_access() {
        set_status(
            &info.name,
            Some("Screen Recording permission is required. Grant it in System Settings, then reopen the preview."),
        );
        return Ok(());
    }
    fetch_display(info.name.clone(), info.id);
    crate::after_change();
    Ok(())
}

/// Closes the preview for `name`, if it is open.
pub fn close(name: &str) {
    let preview = previews(|previews| {
        previews
            .iter()
            .position(|preview| preview.name == name)
            .map(|index| previews.remove(index))
    });
    if let Some(preview) = preview {
        save_frame(name, &preview.window);
        preview.window.close();
    }
    stop_health_if_idle();
    crate::after_change();
}

/// Closes every preview, saving its frame first.
pub fn close_all() {
    let all = previews(|previews| previews.drain(..).collect::<Vec<_>>());
    for preview in all {
        save_frame(&preview.name, &preview.window);
        preview.window.close();
    }
    stop_health_if_idle();
}

pub fn toggle(name: &str) -> Result<(), String> {
    if is_open(name) {
        close(name);
        Ok(())
    } else {
        open(name)
    }
}

pub fn is_open(name: &str) -> bool {
    previews(|previews| previews.iter().any(|preview| preview.name == name))
}

pub fn open_names() -> Vec<String> {
    previews(|previews| previews.iter().map(|preview| preview.name.clone()).collect())
}

/// The preview name for a window pointer, used by the shared window delegate.
pub fn name_for_window(window: &NSWindow) -> Option<String> {
    let target = window as *const NSWindow;
    previews(|previews| {
        previews
            .iter()
            .find(|preview| (&*preview.window as *const NSWindow) == target)
            .map(|preview| preview.name.clone())
    })
}

/// The close button was pressed on a preview window.
pub fn close_window(window: &NSWindow) {
    if let Some(name) = name_for_window(window) {
        close(&name);
    }
}

/// A preview window is closing through its own close button.
pub fn window_closed(window: &NSWindow) {
    let Some(name) = name_for_window(window) else {
        return;
    };
    // The window is already closing; drop its stream and save the frame without closing again.
    let preview = previews(|previews| {
        previews
            .iter()
            .position(|preview| preview.name == name)
            .map(|index| previews.remove(index))
    });
    if let Some(preview) = preview {
        save_frame(&name, &preview.window);
    }
    stop_health_if_idle();
    crate::after_change();
}

/// The user covered or uncovered the window; stop capturing while it cannot be seen.
pub fn occlusion_changed(window: &NSWindow) {
    let Some(name) = name_for_window(window) else {
        return;
    };
    let visible = window.occlusionState().0 & NSWindowOcclusionState::Visible.0 != 0;
    set_paused(&name, !visible);
}

pub fn toggle_paused(window: &NSWindow) {
    let Some(name) = name_for_window(window) else {
        return;
    };
    let paused = previews(|previews| {
        previews
            .iter()
            .find(|preview| preview.name == name)
            .map(|preview| preview.paused)
            .unwrap_or(false)
    });
    set_paused(&name, !paused);
}

/// Called once a second while any preview is open.
pub fn tick() {
    if previews(|previews| previews.is_empty()) {
        return;
    }
    let names = open_names();
    for name in names {
        let (paused, started, last, status) = previews(|previews| {
            let Some(preview) = previews.iter().find(|preview| preview.name == name) else {
                return (false, false, None, None);
            };
            (
                preview.paused,
                preview.started,
                preview.last_frame,
                preview.status.clone(),
            )
        });
        let wanted = if paused {
            Some("Paused".to_string())
        } else if !started {
            None
        } else if last.map_or(true, |last| last.elapsed() > Duration::from_secs(3)) {
            Some("No frames".to_string())
        } else {
            None
        };
        if matches!(status.as_deref(), Some("Paused") | Some("No frames")) || wanted.is_some() {
            if status.as_deref() != wanted.as_deref() {
                set_status(&name, wanted.as_deref());
            }
        }
    }
}

/// Stops the one-second health timer when there is nothing left to watch.
fn stop_health_if_idle() {
    if previews(|previews| previews.is_empty()) {
        HEALTH.with(|slot| {
            if let Some(timer) = slot.borrow_mut().take() {
                unsafe { timer.invalidate() };
            }
        });
    }
}

// --- Window -----------------------------------------------------------------------------------

fn build_window(
    info: &crate::screens::ScreenInfo,
    name: &str,
    actions: &crate::ui::Actions,
) -> Result<
    (
        Retained<NSWindow>,
        Retained<PreviewSurface>,
        Retained<NSTextField>,
        Retained<NSButton>,
    ),
    String,
> {
    unsafe {
        let aspect = info.width as f64 / info.height.max(1) as f64;
        let existing: Vec<NSRect> = previews(|previews| {
            previews.iter().map(|preview| preview.window.frame()).collect()
        });
        let frame = cascade(
            saved_frame(name)
                .map(rect)
                .filter(|frame| valid_frame(*frame))
                .unwrap_or_else(|| default_frame(aspect)),
            &existing,
        );

        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm().alloc(),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::Resizable,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window.setTitle(&NSString::from_str(&format!("{name} preview")));
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setOpaque(false);
        window.setHasShadow(true);
        window.setLevel(NSFloatingWindowLevel);
        window.setMovableByWindowBackground(true);
        window.setHidesOnDeactivate(false);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setContentAspectRatio(NSSize::new(aspect, 1.0));
        window.setContentMinSize(NSSize::new(240.0, 240.0 / aspect));
        window.setDelegate(Some(ProtocolObject::from_ref(actions)));

        let surface = PreviewSurface::new(frame.size);
        window.setContentView(Some(&surface));
        surface.layer().unwrap().setCornerRadius(10.0);
        surface.layer().unwrap().setMasksToBounds(true);

        let (chrome, pause) = build_chrome(&surface, info, actions);
        chrome.setHidden(true);
        surface.set_chrome(chrome);
        surface.addSubview(&surface.chrome_view());

        let placeholder = NSTextField::labelWithString(&NSString::from_str("Loading preview…"), mtm());
        placeholder.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, NSFontWeightSemibold)));
        placeholder.setTextColor(Some(&NSColor::whiteColor()));
        placeholder.setAlignment(NSTextAlignment::Center);
        placeholder.setDrawsBackground(true);
        placeholder.setBackgroundColor(Some(&NSColor::colorWithWhite_alpha(0.0, 0.65)));
        placeholder.setFrame(NSRect::new(
            NSPoint::new(
                (frame.size.width - PLACEHOLDER_WIDTH) / 2.0,
                (frame.size.height - PLACEHOLDER_HEIGHT) / 2.0,
            ),
            NSSize::new(PLACEHOLDER_WIDTH, PLACEHOLDER_HEIGHT),
        ));
        placeholder.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::NSViewMinXMargin
                | objc2_app_kit::NSAutoresizingMaskOptions::NSViewMaxXMargin
                | objc2_app_kit::NSAutoresizingMaskOptions::NSViewMinYMargin
                | objc2_app_kit::NSAutoresizingMaskOptions::NSViewMaxYMargin,
        );
        surface.addSubview(&placeholder);

        window.orderFrontRegardless();
        Ok((window, surface, placeholder, pause))
    }
}

const PLACEHOLDER_WIDTH: f64 = 360.0;
const PLACEHOLDER_HEIGHT: f64 = 24.0;

fn saved_frame(name: &str) -> Option<PreviewFrame> {
    crate::with_model(|model| model.config.virtual_screen(name).and_then(|screen| screen.preview))
}

fn rect(frame: PreviewFrame) -> NSRect {
    NSRect::new(
        NSPoint::new(frame.x, frame.y),
        NSSize::new(frame.width, frame.height),
    )
}

fn save_frame(name: &str, window: &NSWindow) {
    let frame = window.frame();
    crate::with_model(|model| {
        model.config.set_preview_frame(
            name,
            Some(PreviewFrame {
                x: frame.origin.x,
                y: frame.origin.y,
                width: frame.size.width,
                height: frame.size.height,
            }),
        );
    });
}

/// True when at least a corner of the saved frame is still on a screen.
fn valid_frame(frame: NSRect) -> bool {
    if frame.size.width < 160.0 || frame.size.height < 90.0 {
        return false;
    }
    let screens = objc2_app_kit::NSScreen::screens(mtm());
    (0..screens.len()).any(|index| {
        let Some(screen) = screens.get_retained(index) else {
            return false;
        };
        let visible = screen.visibleFrame();
        let width = (frame.origin.x + frame.size.width).min(visible.origin.x + visible.size.width)
            - frame.origin.x.max(visible.origin.x);
        let height = (frame.origin.y + frame.size.height).min(visible.origin.y + visible.size.height)
            - frame.origin.y.max(visible.origin.y);
        width >= 80.0 && height >= 45.0
    })
}

/// Steps a new preview up and left until it does not sit exactly on another preview.
fn cascade(frame: NSRect, existing: &[NSRect]) -> NSRect {
    cascade_in(frame, existing, visible_frame_at(frame.origin))
}

fn cascade_in(mut frame: NSRect, existing: &[NSRect], visible: NSRect) -> NSRect {
    for _ in 0..12 {
        let crowded = existing.iter().any(|other| {
            let width = (frame.origin.x + frame.size.width).min(other.origin.x + other.size.width)
                - frame.origin.x.max(other.origin.x);
            let height = (frame.origin.y + frame.size.height).min(other.origin.y + other.size.height)
                - frame.origin.y.max(other.origin.y);
            width > frame.size.width * 0.6 && height > frame.size.height * 0.6
        });
        if !crowded {
            break;
        }
        frame.origin.x -= 36.0;
        frame.origin.y += 36.0;
    }
    frame.origin.x = frame.origin.x.max(visible.origin.x + 8.0);
    frame.origin.y = frame
        .origin
        .y
        .min(visible.origin.y + visible.size.height - frame.size.height - 8.0)
        .max(visible.origin.y + 8.0);
    frame
}

fn visible_frame_at(point: NSPoint) -> NSRect {
    let screens = objc2_app_kit::NSScreen::screens(mtm());
    for index in 0..screens.len() {
        let Some(screen) = screens.get_retained(index) else {
            continue;
        };
        let frame = screen.visibleFrame();
        if point.x >= frame.origin.x
            && point.x <= frame.origin.x + frame.size.width
            && point.y >= frame.origin.y
            && point.y <= frame.origin.y + frame.size.height
        {
            return frame;
        }
    }
    screens
        .get_retained(0)
        .map(|screen| screen.visibleFrame())
        .unwrap_or(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 720.0)))
}

fn default_frame(aspect: f64) -> NSRect {
    let mouse = unsafe { objc2_app_kit::NSEvent::mouseLocation() };
    let screens = objc2_app_kit::NSScreen::screens(mtm());
    let mut visible = screens
        .get_retained(0)
        .map(|screen| screen.visibleFrame())
        .unwrap_or(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 720.0)));
    for index in 0..screens.len() {
        let Some(screen) = screens.get_retained(index) else {
            continue;
        };
        let frame = screen.visibleFrame();
        if mouse.x >= frame.origin.x
            && mouse.x <= frame.origin.x + frame.size.width
            && mouse.y >= frame.origin.y
            && mouse.y <= frame.origin.y + frame.size.height
        {
            visible = frame;
            break;
        }
    }
    let width = (visible.size.width * DEFAULT_WIDTH).clamp(MIN_WIDTH, MAX_WIDTH);
    let height = width / aspect;
    NSRect::new(
        NSPoint::new(
            visible.origin.x + visible.size.width - width - MARGIN,
            visible.origin.y + MARGIN,
        ),
        NSSize::new(width, height),
    )
}

fn build_chrome(
    surface: &NSView,
    info: &crate::screens::ScreenInfo,
    actions: &crate::ui::Actions,
) -> (Retained<NSView>, Retained<NSButton>) {
    unsafe {
        let width = surface.frame().size.width;
        let bar = NSVisualEffectView::initWithFrame(
            mtm().alloc(),
            NSRect::new(
                NSPoint::new(0.0, surface.frame().size.height - CHROME),
                NSSize::new(width, CHROME),
            ),
        );
        bar.setMaterial(NSVisualEffectMaterial::HUDWindow);
        bar.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        bar.setState(NSVisualEffectState::Active);
        bar.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::NSViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::NSViewMinYMargin,
        );

        let title = NSTextField::labelWithString(
            &NSString::from_str(&format!(
                "{}  ·  {}x{}",
                info.name, info.width, info.height
            )),
            mtm(),
        );
        title.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, NSFontWeightSemibold)));
        title.setTextColor(Some(&NSColor::whiteColor()));
        title.setFrame(NSRect::new(
            NSPoint::new(10.0, 7.0),
            NSSize::new((width - 84.0).max(80.0), 16.0),
        ));
        title.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::NSViewWidthSizable);
        bar.addSubview(&title);

        let close = NSButton::buttonWithTitle_image_target_action(
            &NSString::from_str(""),
            &symbol("xmark"),
            Some(actions),
            Some(sel!(previewClose:)),
            mtm(),
        );
        close.setBordered(false);
        close.setContentTintColor(Some(&NSColor::whiteColor()));
        close.setFrame(NSRect::new(
            NSPoint::new(width - 28.0, 4.0),
            NSSize::new(22.0, 22.0),
        ));
        close.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::NSViewMinXMargin);
        bar.addSubview(&close);

        let pause = NSButton::buttonWithTitle_image_target_action(
            &NSString::from_str(""),
            &symbol("pause.fill"),
            Some(actions),
            Some(sel!(previewPause:)),
            mtm(),
        );
        pause.setBordered(false);
        pause.setContentTintColor(Some(&NSColor::whiteColor()));
        pause.setFrame(NSRect::new(
            NSPoint::new(width - 54.0, 4.0),
            NSSize::new(22.0, 22.0),
        ));
        pause.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::NSViewMinXMargin);
        bar.addSubview(&pause);

        let view = NSView::initWithFrame(mtm().alloc(), surface.frame());
        view.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::NSViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::NSViewHeightSizable,
        );
        view.addSubview(&bar);
        (view, pause)
    }
}

fn symbol(name: &str) -> Retained<NSImage> {
    unsafe {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
            .unwrap_or_else(|| NSImage::new())
    }
}

// --- Capture ----------------------------------------------------------------------------------

/// ScreenCaptureKit and `CGDisplayCreateImage` both need this permission.
fn screen_access() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() != 0 }
}

fn target_pixels(info: &crate::screens::ScreenInfo) -> (usize, usize) {
    let width = info.width.max(1) as f64;
    let height = info.height.max(1) as f64;
    let scale = if info.scale > 0.0 { info.scale } else { 1.0 };
    let pixel_width = (info.pixel_width.max(1) as f64).max(width * scale);
    let pixel_height = (info.pixel_height.max(1) as f64).max(height * scale);
    let shrink = (1920.0 / pixel_width).min(1.0);
    (
        (pixel_width * shrink).round().max(2.0) as usize,
        (pixel_height * shrink).round().max(2.0) as usize,
    )
}

/// Asks ScreenCaptureKit for the shareable display matching `display_id`, then attaches a stream.
fn fetch_display(name: String, display_id: u32) {
    let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        if content.is_null() || !error.is_null() {
            let message = if error.is_null() {
                "ScreenCaptureKit returned no content".to_string()
            } else {
                unsafe { (*error).localizedDescription().to_string() }
            };
            let name = name.clone();
            crate::on_main(move || set_status(&name, Some(&message)));
            return;
        }
        unsafe {
            let content = &*content;
            let displays = content.displays();
            for index in 0..displays.len() {
                let Some(display) = displays.get_retained(index) else {
                    continue;
                };
                let id: u32 = msg_send![&*display, displayID];
                if id != display_id {
                    continue;
                }
                let ptr = Retained::into_raw(display) as usize;
                let name = name.clone();
                crate::on_main(move || {
                    let display = Retained::from_raw(ptr as *mut SCDisplay);
                    if let Some(display) = display {
                        attach_stream(&name, display);
                    }
                });
                return;
            }
        }
        let name = name.clone();
        crate::on_main(move || {
            set_status(
                &name,
                Some("The display is not available to ScreenCaptureKit"),
            )
        });
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
}

fn attach_stream(name: &str, display: Retained<SCDisplay>) {
    if !is_open(name) {
        return;
    }
    let Some(info) = crate::screens::info(name) else {
        return;
    };
    unsafe {
        let empty = objc2_foundation::NSArray::<SCWindow>::new();
        let filter = SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &empty,
        );
        let config = SCStreamConfiguration::new();
        let (width, height) = target_pixels(&info);
        config.setWidth(width);
        config.setHeight(height);
        config.setScalesToFit(true);
        config.setPreservesAspectRatio(true);
        config.setShowsCursor(false);
        config.setQueueDepth(3);
        config.setCapturesAudio(false);
        // kCVPixelFormatType_32BGRA
        config.setPixelFormat(0x4247_5241);
        set_minimum_frame_interval(&config, 30);

        let stream = SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            None,
        );
        let output = make_output(name);
        let label = CString::new("com.rai.lumen.preview").unwrap();
        let queue = dispatch_queue_create(label.as_ptr(), std::ptr::null());
        let mut error: *mut NSError = std::ptr::null_mut();
        let added: bool = msg_send![
            &*stream,
            addStreamOutput: &*output,
            type: SCStreamOutputType::Screen,
            sampleHandlerQueue: &*(queue as *const AnyObject),
            error: &mut error
        ];
        if !added {
            if !queue.is_null() {
                dispatch_release(queue);
            }
            let message = if error.is_null() {
                "ScreenCaptureKit refused the output".to_string()
            } else {
                (*error).localizedDescription().to_string()
            };
            set_status(name, Some(&message));
            return;
        }

        let paused = previews(|previews| {
            previews
                .iter()
                .find(|preview| preview.name == name)
                .map(|preview| preview.paused)
                .unwrap_or(false)
        });
        if !paused {
            let started = RcBlock::new(|error: *mut NSError| {
                if !error.is_null() {
                    let message = (*error).localizedDescription().to_string();
                    crate::log::info(format!("preview stream failed: {message}"));
                }
            });
            stream.startCaptureWithCompletionHandler(Some(&started));
        }
        previews(|previews| {
            let Some(preview) = previews.iter_mut().find(|preview| preview.name == name) else {
                return;
            };
            preview.stream = Some(stream);
            preview.output = Some(output);
            preview.queue = queue;
            preview.started = true;
            set_status_locked(preview, if preview.paused { Some("Paused".to_string()) } else { None });
        });
    }
}

/// SCStreamConfiguration.minimumFrameInterval is not in the 0.2.2 bindings.
fn set_minimum_frame_interval(config: &SCStreamConfiguration, fps: i32) {
    #[repr(C)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }
    unsafe impl Encode for CMTime {
        // The ABI type code macOS uses for CMTime is `{?=qiIq}`: an anonymous struct.
        const ENCODING: Encoding = Encoding::Struct(
            "?",
            &[Encoding::LongLong, Encoding::Int, Encoding::UInt, Encoding::LongLong],
        );
    }
    let time = CMTime {
        value: 1,
        timescale: fps,
        flags: 3,
        epoch: 0,
    };
    let _: () = unsafe { msg_send![config, setMinimumFrameInterval: time] };
}

fn make_output(name: &str) -> Retained<PreviewOutput> {
    let object = PreviewOutput::alloc().set_ivars(OutputIvars {
        name: name.to_string(),
        last: Mutex::new(None),
    });
    unsafe { msg_send_id![super(object), init] }
}

fn frame_for_sample(sample: *mut AnyObject) -> *mut c_void {
    unsafe {
        let buffer = CMSampleBufferGetImageBuffer(sample as *mut c_void);
        if buffer.is_null() {
            return std::ptr::null_mut();
        }
        let mut image: *mut c_void = std::ptr::null_mut();
        let status = VTCreateCGImageFromCVPixelBuffer(buffer, std::ptr::null(), &mut image);
        if status != 0 {
            return std::ptr::null_mut();
        }
        image
    }
}

/// Called on the main thread with a fresh CGImage.
pub fn show_frame(name: &str, image: *mut c_void) {
    if image.is_null() {
        return;
    }
    previews(|previews| {
        let Some(preview) = previews.iter_mut().find(|preview| preview.name == name) else {
            unsafe { CGImageRelease(image) };
            return;
        };
        unsafe {
            if let Some(layer) = preview.surface.layer() {
                layer.setContents(Some(&*(image as *const AnyObject)));
                if let Some(window) = preview.surface.window() {
                    layer.setContentsScale(window.backingScaleFactor());
                }
            }
            if !preview.frame.is_null() {
                CGImageRelease(preview.frame);
            }
            preview.frame = image;
        }
        preview.last_frame = Some(Instant::now());
        if preview.status.as_deref() == Some("No frames") {
            set_status_locked(preview, None);
        }
    });
}

fn set_paused(name: &str, paused: bool) {
    previews(|previews| {
        let Some(preview) = previews.iter_mut().find(|preview| preview.name == name) else {
            return;
        };
        if preview.paused == paused {
            return;
        }
        preview.paused = paused;
        crate::log::trace(format!(
            "preview {}: {}",
            name,
            if paused { "paused" } else { "resumed" }
        ));
        unsafe {
            let image = symbol(if paused { "play.fill" } else { "pause.fill" });
            preview.pause.setImage(Some(&image));
            preview.pause.setToolTip(Some(&NSString::from_str(if paused { "Resume" } else { "Pause" })));
            match (&preview.stream, paused) {
                (Some(stream), true) => {
                    let _: () = msg_send![&**stream, stopCaptureWithCompletionHandler: std::ptr::null::<AnyObject>()];
                }
                (Some(stream), false) => {
                    let block = RcBlock::new(|error: *mut NSError| {
                        if !error.is_null() {
                            let message = (*error).localizedDescription().to_string();
                            crate::log::info(format!("preview resume failed: {message}"));
                        }
                    });
                    stream.startCaptureWithCompletionHandler(Some(&block));
                }
                (None, _) => {}
            }
        }
        set_status_locked(preview, if paused { Some("Paused".to_string()) } else { None });
    });
}

// --- Status -----------------------------------------------------------------------------------

fn set_status(name: &str, text: Option<&str>) {
    previews(|previews| {
        if let Some(preview) = previews.iter_mut().find(|preview| preview.name == name) {
            set_status_locked(preview, text.map(str::to_string));
        }
    });
}

fn set_status_locked(preview: &mut Preview, text: Option<String>) {
    if preview.status == text {
        return;
    }
    preview.status = text.clone();
    unsafe {
        match text {
            Some(text) => {
                preview.placeholder.setStringValue(&NSString::from_str(&text));
                preview.placeholder.setHidden(false);
            }
            None => preview.placeholder.setHidden(true),
        }
    }
}

// --- Health and thumbnails --------------------------------------------------------------------

fn start_health_timer() {
    let Some(actions) = crate::ui::target() else {
        return;
    };
    HEALTH.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                1.0,
                &*actions,
                sel!(previewTick:),
                None,
                true,
            )
        };
        *slot.borrow_mut() = Some(timer);
    });
}

/// Starts or stops the thumbnail refresh for the Settings page. Idempotent.
pub fn set_thumbnails_visible(visible: bool) {
    if !visible {
        THUMBNAILS.with(|slot| {
            if let Some(timer) = slot.borrow_mut().take() {
                unsafe { timer.invalidate() };
            }
        });
        return;
    }
    let Some(actions) = crate::ui::target() else {
        return;
    };
    THUMBNAILS.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                2.0,
                &*actions,
                sel!(thumbnailTick:),
                None,
                true,
            )
        };
        *slot.borrow_mut() = Some(timer);
    });
    thumbnail_tick();
}

/// Refreshes every thumbnail on the Virtual Displays page, reusing preview frames when possible.
pub fn thumbnail_tick() {
    if !crate::ui::thumbnails_visible() {
        return;
    }
    let screens = crate::screens::list();
    if screens.is_empty() {
        return;
    }
    for screen in screens {
        if let Some(image) = preview_image(&screen.name) {
            crate::ui::set_thumbnail(&screen.name, image);
        } else {
            capture_thumbnail(screen.name.clone(), screen.id);
        }
    }
}

fn preview_image(name: &str) -> Option<*mut c_void> {
    previews(|previews| {
        previews
            .iter()
            .find(|preview| preview.name == name)
            .map(|preview| preview.frame)
            .filter(|frame| !frame.is_null())
    })
    .map(|image| {
        unsafe { CFRetain(image as CFTypeRef) };
        image
    })
}

fn capture_thumbnail(name: String, display_id: u32) {
    let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        if content.is_null() || !error.is_null() {
            return;
        }
        unsafe {
            let content = &*content;
            let displays = content.displays();
            for index in 0..displays.len() {
                let Some(display) = displays.get_retained(index) else {
                    continue;
                };
                let id: u32 = msg_send![&*display, displayID];
                if id != display_id {
                    continue;
                }
                let filter = SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &objc2_foundation::NSArray::<SCWindow>::new(),
                );
                let config = SCStreamConfiguration::new();
                config.setWidth(320);
                config.setHeight(180);
                config.setScalesToFit(true);
                config.setPreservesAspectRatio(true);
                config.setShowsCursor(false);
                let name = name.clone();
                let done = RcBlock::new(move |image: *mut AnyObject, error: *mut NSError| {
                    if image.is_null() || !error.is_null() {
                        return;
                    }
                    CFRetain(image as CFTypeRef);
                    let ptr = image as usize;
                    let name = name.clone();
                    crate::on_main(move || {
                        crate::ui::set_thumbnail(&name, ptr as *mut c_void);
                    });
                });
                let _: () = msg_send![
                    SCScreenshotManager::class(),
                    captureImageWithFilter: &*filter,
                    configuration: &*config,
                    completionHandler: &*done
                ];
                return;
            }
        }
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
}

// --- Surface and output classes ---------------------------------------------------------------

// Content view that reveals the chrome while the pointer is over the preview.
declare_class!(
    pub struct PreviewSurface;

    unsafe impl ClassType for PreviewSurface {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "LumenPreviewSurface";
    }

    impl DeclaredClass for PreviewSurface {
        type Ivars = SurfaceIvars;
    }

    unsafe impl NSObjectProtocol for PreviewSurface {}

    unsafe impl PreviewSurface {
        #[method(updateTrackingAreas)]
        fn update_tracking_areas(&self) {
            unsafe {
                if let Some(old) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&old);
                }
                let options = NSTrackingAreaOptions::NSTrackingMouseEnteredAndExited
                    | NSTrackingAreaOptions::NSTrackingActiveAlways
                    | NSTrackingAreaOptions::NSTrackingInVisibleRect;
                let area = NSTrackingArea::initWithRect_options_owner_userInfo(
                    mtm().alloc(),
                    self.bounds(),
                    options,
                    Some(self),
                    None,
                );
                self.addTrackingArea(&area);
                *self.ivars().tracking.borrow_mut() = Some(area);
            }
        }

        #[method(mouseEntered:)]
        fn mouse_entered(&self, _event: *mut AnyObject) {
            if let Some(chrome) = self.ivars().chrome.borrow().as_ref() {
                chrome.setHidden(false);
            }
        }

        #[method(mouseExited:)]
        fn mouse_exited(&self, _event: *mut AnyObject) {
            if let Some(chrome) = self.ivars().chrome.borrow().as_ref() {
                chrome.setHidden(true);
            }
        }
    }
);

pub struct SurfaceIvars {
    chrome: RefCell<Option<Retained<NSView>>>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
}

impl PreviewSurface {
    fn new(size: NSSize) -> Retained<Self> {
        let view = mtm().alloc::<PreviewSurface>().set_ivars(SurfaceIvars {
            chrome: RefCell::new(None),
            tracking: RefCell::new(None),
        });
        let view: Retained<Self> =
            unsafe { msg_send_id![super(view), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), size)] };
        view.setWantsLayer(true);
        view
    }

    fn set_chrome(&self, chrome: Retained<NSView>) {
        *self.ivars().chrome.borrow_mut() = Some(chrome);
    }

    fn chrome_view(&self) -> Retained<NSView> {
        self.ivars().chrome.borrow().as_ref().unwrap().clone()
    }
}

// Receives ScreenCaptureKit sample buffers on the capture queue.
declare_class!(
    pub struct PreviewOutput;

    unsafe impl ClassType for PreviewOutput {
        type Super = objc2_foundation::NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "LumenPreviewOutput";
    }

    impl DeclaredClass for PreviewOutput {
        type Ivars = OutputIvars;
    }

    unsafe impl NSObjectProtocol for PreviewOutput {}
    unsafe impl SCStreamOutput for PreviewOutput {}

    unsafe impl PreviewOutput {
        #[method(stream:didOutputSampleBuffer:ofType:)]
        fn did_output(
            &self,
            _stream: *mut AnyObject,
            sample: *mut AnyObject,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            let now = Instant::now();
            {
                let Ok(mut last) = self.ivars().last.lock() else {
                    return;
                };
                if let Some(previous) = *last {
                    if now.duration_since(previous) < Duration::from_millis(28) {
                        return;
                    }
                }
                *last = Some(now);
            }
            let image = frame_for_sample(sample);
            if image.is_null() {
                return;
            }
            let name = self.ivars().name.clone();
            let ptr = image as usize;
            crate::on_main(move || show_frame(&name, ptr as *mut c_void));
        }
    }
);

pub struct OutputIvars {
    name: String,
    last: Mutex<Option<Instant>>,
}

// --- Platform bindings ------------------------------------------------------------------------

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> u8;
    fn CGImageRelease(image: *mut c_void);
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferGetImageBuffer(sample: *mut c_void) -> *mut c_void;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTCreateCGImageFromCVPixelBuffer(
        pixel_buffer: *mut c_void,
        options: *const c_void,
        image_out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "System")]
unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
}


#[cfg(test)]
mod tests {
    use super::cascade_in;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    #[test]
    fn cascade_moves_a_new_preview_off_an_existing_one() {
        let visible = rect(0.0, 0.0, 1680.0, 1000.0);
        let first = rect(1176.0, 24.0, 480.0, 270.0);
        let moved = cascade_in(first, &[first], visible);
        assert!(moved.origin.x < first.origin.x);
        assert!(moved.origin.y > first.origin.y);
    }

    #[test]
    fn cascade_leaves_a_free_spot_alone() {
        let visible = rect(0.0, 0.0, 1680.0, 1000.0);
        let frame = rect(100.0, 100.0, 480.0, 270.0);
        assert_eq!(cascade_in(frame, &[], visible), frame);
    }

    #[test]
    fn cascade_keeps_the_window_on_screen() {
        let visible = rect(0.0, 0.0, 1000.0, 800.0);
        let frame = rect(8.0, 8.0, 480.0, 270.0);
        let blocked = rect(8.0, 8.0, 480.0, 270.0);
        let moved = cascade_in(frame, &[blocked], visible);
        assert!(moved.origin.x >= visible.origin.x + 8.0);
        assert!(moved.origin.y >= visible.origin.y + 8.0);
    }
}
