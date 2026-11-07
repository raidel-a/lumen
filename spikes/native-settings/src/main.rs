//! Spike: the Lumen settings window rebuilt with AppKit controls, no Rune and no GPU.
//!
//! Modes:
//!   `native-settings --idle`      status item only, pump the run loop
//!   `native-settings --open`      build and show the window once, print build/show timings
//!   `native-settings --cycle N`   build/show/close N times, print each timing
//!
//! The UI mirrors the Rune version: General (three settings cards + about) and Displays
//! (display list + per-input rows + add row). Nothing is wired up; this measures the memory and
//! latency of an AppKit settings surface in the same accessory-app shell.

use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBox, NSBoxType, NSButton,
    NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSLineBreakMode, NSSegmentSwitchTracking,
    NSSegmentedControl, NSSwitch, NSTextField, NSTitlePosition, NSView, NSWindow,
    NSWindowStyleMask,
};
use block2::RcBlock;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSPoint, NSRect, NSRunLoop, NSSize, NSString, NSTimer,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::ptr::NonNull;
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::TrayIconBuilder;

const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 540.0;
const PAD: f64 = 24.0;

fn mtm() -> MainThreadMarker {
    unsafe { MainThreadMarker::new_unchecked() }
}

struct Theme {
    card: Retained<NSColor>,
    border: Retained<NSColor>,
    text: Retained<NSColor>,
    secondary: Retained<NSColor>,
}

impl Theme {
    fn new() -> Self {
        unsafe {
            Self {
                card: NSColor::controlBackgroundColor(),
                border: NSColor::separatorColor(),
                text: NSColor::labelColor(),
                secondary: NSColor::secondaryLabelColor(),
            }
        }
    }
}

/// A frame measured from the top of the window rather than AppKit's bottom-left origin.
fn frame(top: f64, x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, top - y - h), NSSize::new(w, h))
}

fn label(text: &str, size: f64, weight: f64, color: &NSColor, rect: NSRect) -> Retained<NSTextField> {
    unsafe {
        let field = NSTextField::labelWithString(&NSString::from_str(text), mtm());
        field.setFrame(rect);
        field.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
        field.setTextColor(Some(color));
        field.setLineBreakMode(NSLineBreakMode::NSLineBreakByTruncatingTail);
        field
    }
}

fn mono(text: &str, size: f64, color: &NSColor, rect: NSRect) -> Retained<NSTextField> {
    unsafe {
        let field = NSTextField::labelWithString(&NSString::from_str(text), mtm());
        field.setFrame(rect);
        field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            size,
            NSFontWeightRegular,
        )));
        field.setTextColor(Some(color));
        field
    }
}

fn card(theme: &Theme, parent: &NSView, rect: NSRect, title: &str, detail: &str) -> f64 {
    unsafe {
        let bx = NSBox::initWithFrame(mtm().alloc(), rect);
        bx.setBoxType(NSBoxType::NSBoxCustom);
        bx.setTitlePosition(NSTitlePosition::NSNoTitle);
        bx.setBorderWidth(1.0);
        bx.setCornerRadius(10.0);
        bx.setFillColor(&theme.card);
        bx.setBorderColor(&theme.border);
        parent.addSubview(&bx);

        if !title.is_empty() {
            let title_frame = NSRect::new(
                NSPoint::new(rect.origin.x + 16.0, rect.origin.y + rect.size.height - 30.0),
                NSSize::new(rect.size.width - 32.0, 18.0),
            );
            parent.addSubview(&label(title, 13.0, NSFontWeightMedium, &theme.text, title_frame));
        }
        if !detail.is_empty() {
            let detail_frame = NSRect::new(
                NSPoint::new(rect.origin.x + 16.0, rect.origin.y + 12.0),
                NSSize::new(rect.size.width - 130.0, 16.0),
            );
            parent.addSubview(&label(detail, 12.0, NSFontWeightRegular, &theme.secondary, detail_frame));
        }

        rect.origin.x + rect.size.width - 16.0
    }
}

fn switch(on: bool, parent: &NSView, x_right: f64, y: f64) {
    unsafe {
        let s = NSSwitch::new(mtm());
        s.setFrame(NSRect::new(NSPoint::new(x_right - 38.0, y), NSSize::new(38.0, 22.0)));
        s.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        parent.addSubview(&s);
    }
}

fn button(text: &str, parent: &NSView, rect: NSRect, bordered: bool) -> Retained<NSButton> {
    unsafe {
        let b = NSButton::buttonWithTitle_target_action(&NSString::from_str(text), None, None, mtm());
        b.setFrame(rect);
        b.setBordered(bordered);
        parent.addSubview(&b);
        b
    }
}

fn checkbox(on: bool, parent: &NSView, rect: NSRect) {
    unsafe {
        let b = NSButton::checkboxWithTitle_target_action(&NSString::from_str(""), None, None, mtm());
        b.setFrame(rect);
        b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        parent.addSubview(&b);
    }
}

// Sample content from the real config: one LG and its detected inputs.
const INPUTS: [(&str, &str, &str, bool); 7] = [
    ("HDMI 1", "0x0011", "standard", true),
    ("HDMI 2", "0x0012", "standard", true),
    ("DisplayPort 1", "0x000f", "standard", false),
    ("DisplayPort", "0x00d0", "LG alt", true),
    ("HDMI 1", "0x0090", "LG alt", true),
    ("HDMI 2", "0x0091", "LG alt", true),
    ("USB-C", "0x00d2", "LG alt", true),
];

fn build_ui(parent: &NSView, theme: &Theme) {
    unsafe {
        parent.addSubview(&label(
            "Lumen",
            16.0,
            NSFontWeightSemibold,
            &theme.text,
            frame(HEIGHT, PAD, 14.0, 70.0, 22.0),
        ));
        parent.addSubview(&label(
            "Menu bar display control",
            13.0,
            NSFontWeightRegular,
            &theme.secondary,
            frame(HEIGHT, PAD + 76.0, 16.0, 260.0, 18.0),
        ));

        let labels = NSArray::from_id_slice(&[
            NSString::from_str("General"),
            NSString::from_str("Displays"),
        ]);
        let tabs = NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
            &labels,
            NSSegmentSwitchTracking::SelectOne,
            None,
            None,
            mtm(),
        );
        tabs.setFrame(frame(HEIGHT, WIDTH - PAD - 156.0, 12.0, 156.0, 26.0));
        tabs.setSelectedSegment(0);
        parent.addSubview(&tabs);

        // General page.
        let general = NSView::initWithFrame(mtm().alloc(), frame(HEIGHT, 0.0, 0.0, WIDTH, HEIGHT));
        parent.addSubview(&general);
        general.addSubview(&label(
            "BEHAVIOUR",
            11.0,
            NSFontWeightSemibold,
            &theme.secondary,
            frame(HEIGHT, PAD, 64.0, 200.0, 14.0),
        ));

        let rows = [
            ("Reclaim displays on quit", "Bring every released display back before Lumen exits.", true),
            ("Auto-reclaim", "Bring a released display back by itself after a while.", true),
            ("Show brightness presets", "List 25/50/75/100% under each display in the menu.", true),
        ];
        for (index, (title, detail, on)) in rows.iter().enumerate() {
            let y = 86.0 + index as f64 * 72.0;
            let rect = frame(HEIGHT, PAD, y, WIDTH - PAD * 2.0, 64.0);
            let right = card(theme, &general, rect, title, detail);
            if index == 1 {
                let mut x = right - 4.0 * 48.0;
                for text in ["Off", "1 min", "5 min", "15 min"] {
                    let _ = button(text, &general, frame(HEIGHT, x, y + 24.0, 44.0, 24.0), true);
                    x += 48.0;
                }
            } else {
                switch(*on, &general, right, HEIGHT - y - 32.0);
            }
        }

        general.addSubview(&label(
            "ABOUT",
            11.0,
            NSFontWeightSemibold,
            &theme.secondary,
            frame(HEIGHT, PAD, 312.0, 200.0, 14.0),
        ));
        let about = frame(HEIGHT, PAD, 332.0, WIDTH - PAD * 2.0, 64.0);
        let _ = card(theme, &general, about, "", "");
        general.addSubview(&label(
            "Lumen switches inputs over DDC. When a monitor's firmware ignores input select, Release \
             cuts the signal instead and the monitor follows its other live input.",
            12.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(
                NSPoint::new(about.origin.x + 16.0, about.origin.y + 14.0),
                NSSize::new(about.size.width - 32.0, 34.0),
            ),
        ));

        // Displays page.
        let displays = NSView::initWithFrame(mtm().alloc(), frame(HEIGHT, 0.0, 0.0, WIDTH, HEIGHT));
        displays.setHidden(true);
        parent.addSubview(&displays);

        let _ = button("LG ULTRAWIDE", &displays, frame(HEIGHT, PAD, 64.0, 182.0, 30.0), true);

        let mut y = 64.0;
        displays.addSubview(&label(
            "LG ULTRAWIDE",
            15.0,
            NSFontWeightSemibold,
            &theme.text,
            frame(HEIGHT, 230.0, y, 300.0, 20.0),
        ));
        y += 26.0;
        displays.addSubview(&mono(
            "1e6d:5bf8:00074f58  ·  connected",
            11.0,
            &theme.secondary,
            frame(HEIGHT, 230.0, y, 400.0, 16.0),
        ));
        y += 26.0;
        displays.addSubview(&label(
            "MENU INPUTS",
            11.0,
            NSFontWeightSemibold,
            &theme.secondary,
            frame(HEIGHT, 230.0, y, 200.0, 14.0),
        ));
        y += 22.0;

        for (label_text, value, protocol, enabled) in INPUTS {
            let row = frame(HEIGHT, 230.0, y, WIDTH - 230.0 - PAD, 34.0);
            let bx = NSBox::initWithFrame(mtm().alloc(), row);
            bx.setBoxType(NSBoxType::NSBoxCustom);
            bx.setTitlePosition(NSTitlePosition::NSNoTitle);
            bx.setBorderWidth(1.0);
            bx.setCornerRadius(8.0);
            bx.setFillColor(&theme.card);
            bx.setBorderColor(&theme.border);
            displays.addSubview(&bx);

            checkbox(
                enabled,
                &displays,
                NSRect::new(NSPoint::new(row.origin.x + 10.0, row.origin.y + 8.0), NSSize::new(18.0, 18.0)),
            );
            displays.addSubview(&label(
                label_text,
                13.0,
                NSFontWeightMedium,
                &theme.text,
                NSRect::new(NSPoint::new(row.origin.x + 36.0, row.origin.y + 9.0), NSSize::new(150.0, 18.0)),
            ));
            displays.addSubview(&mono(
                value,
                11.0,
                &theme.secondary,
                NSRect::new(
                    NSPoint::new(row.origin.x + row.size.width - 200.0, row.origin.y + 9.0),
                    NSSize::new(80.0, 16.0),
                ),
            ));
            let _ = button(
                protocol,
                &displays,
                NSRect::new(
                    NSPoint::new(row.origin.x + row.size.width - 112.0, row.origin.y + 5.0),
                    NSSize::new(70.0, 24.0),
                ),
                true,
            );
            let _ = button(
                "×",
                &displays,
                NSRect::new(
                    NSPoint::new(row.origin.x + row.size.width - 34.0, row.origin.y + 5.0),
                    NSSize::new(24.0, 24.0),
                ),
                false,
            );
            y += 40.0;
        }

        let add_y = y + 2.0;
        for (x, w) in [(230.0, 140.0), (378.0, 80.0)] {
            let field = NSTextField::initWithFrame(mtm().alloc(), frame(HEIGHT, x, add_y, w, 28.0));
            displays.addSubview(&field);
        }
        let _ = button("LG alt", &displays, frame(HEIGHT, 466.0, add_y, 64.0, 28.0), true);
        let _ = button("Add", &displays, frame(HEIGHT, 538.0, add_y, 64.0, 28.0), true);

        let _ = button(
            "Reset to detected",
            &displays,
            frame(HEIGHT, 230.0, add_y + 38.0, 150.0, 28.0),
            true,
        );
        displays.addSubview(&label(
            "Changes apply to the menu immediately.",
            12.0,
            NSFontWeightRegular,
            &theme.secondary,
            frame(HEIGHT, 392.0, add_y + 42.0, 240.0, 18.0),
        ));
    }
}

fn build_window() -> Retained<NSWindow> {
    let theme = Theme::new();
    unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm().alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        );
        window.setTitle(&NSString::from_str("Lumen Settings"));
        window.setReleasedWhenClosed(false);
        if let Some(content) = window.contentView() {
            build_ui(&content, &theme);
        }
        window.center();
        window
    }
}

fn pump(seconds: f64) {
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < end {
        unsafe {
            let run_loop = NSRunLoop::currentRunLoop();
            let until = NSDate::dateWithTimeIntervalSinceNow(0.02);
            run_loop.runUntilDate(&until);
        }
    }
}

fn icon() -> tray_icon::Icon {
    let size = 22u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 6..16 {
        for x in 3..19 {
            let i = ((y * size + x) * 4) as usize;
            rgba[i + 3] = 255;
        }
    }
    tray_icon::Icon::from_rgba(rgba, size, size).expect("icon")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("--idle");

    let app = NSApplication::sharedApplication(mtm());
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    unsafe { app.finishLaunching() };

    let menu = Menu::new();
    let _ = menu.append(&MenuItem::new("Lumen", false, None));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("settings", "Settings…", true, None));
    let _ = menu.append(&MenuItem::with_id("quit", "Quit Lumen", true, None));
    let _tray = TrayIconBuilder::new()
        .with_icon_templated(icon())
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .build()
        .expect("tray");

    match mode {
        "--open" => {
            let start = Instant::now();
            let window = build_window();
            let built = start.elapsed();
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            window.makeKeyAndOrderFront(None);
            println!(
                "build={}ms show={}ms",
                built.as_millis(),
                start.elapsed().as_millis()
            );
            unsafe { app.run() };
        }
        "--cycle" => {
            let count: usize = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(3);
            let window: Rc<RefCell<Option<Retained<NSWindow>>>> = Rc::new(RefCell::new(None));
            let opened = Rc::new(Cell::new(0usize));
            let state = window.clone();
            let opens = opened.clone();
            let mut timer: Option<Retained<NSTimer>> = None;

            let app_for_block = app.clone();
            let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
                if let Some(current) = state.borrow_mut().take() {
                    let start = Instant::now();
                    current.close();
                    drop(current);
                    println!("close={}ms", start.elapsed().as_millis());
                } else {
                    let start = Instant::now();
                    let window = build_window();
                    #[allow(deprecated)]
                    app_for_block.activateIgnoringOtherApps(true);
                    window.makeKeyAndOrderFront(None);
                    println!("open={}ms", start.elapsed().as_millis());
                    *state.borrow_mut() = Some(window);
                    opens.set(opens.get() + 1);
                    if opens.get() >= count {
                        // Next tick closes the last window; then the timer stops.
                    }
                }
            });
            let scheduled = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.5, true, &block)
            };
            timer = Some(scheduled);
            let _ = &timer;
            unsafe { app.run() };
        }
        _ => {
            println!("idle");
            unsafe { app.run() };
        }
    }
}
