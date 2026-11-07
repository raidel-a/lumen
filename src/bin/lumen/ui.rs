//! The AppKit settings window.
//!
//! Built once on first open and rebuilt in place after model changes. Controls carry `tag`
//! values that the `Actions` target uses to find the row they belong to.

use crate::config::{InputSetting, Protocol};
use crate::model::Model;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSControl,
    NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontWeightMedium, NSFontWeightRegular,
    NSFontWeightSemibold, NSLineBreakMode, NSSegmentSwitchTracking, NSSegmentedControl,
    NSApplicationDidChangeScreenParametersNotification, NSSwitch, NSTextField, NSTitlePosition,
    NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotificationCenter, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString,
};
use std::cell::RefCell;

const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 540.0;
const PAD: f64 = 24.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Displays,
}

pub struct Ui {
    pub window: Option<Retained<NSWindow>>,
    pub page: Page,
    pub selected: usize,
    pub draft_label: Option<Retained<NSTextField>>,
    pub draft_value: Option<Retained<NSTextField>>,
    pub draft_protocol: Protocol,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            window: None,
            page: Page::General,
            selected: 0,
            draft_label: None,
            draft_value: None,
            draft_protocol: Protocol::LgAlt,
        }
    }
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui::default());
    static ACTIONS: RefCell<Option<Retained<Actions>>> = const { RefCell::new(None) };
    /// A closed window waiting for the run loop to finish the close before it is released.
    static STALE: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

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

/// A multi-line label that wraps inside its frame.
fn paragraph(text: &str, size: f64, color: &NSColor, rect: NSRect) -> Retained<NSTextField> {
    unsafe {
        let field = NSTextField::initWithFrame(mtm().alloc(), rect);
        field.setStringValue(&NSString::from_str(text));
        field.setBezeled(false);
        field.setEditable(false);
        field.setSelectable(false);
        field.setDrawsBackground(false);
        field.setFont(Some(&NSFont::systemFontOfSize_weight(size, NSFontWeightRegular)));
        field.setTextColor(Some(color));
        field.setLineBreakMode(NSLineBreakMode::NSLineBreakByWordWrapping);
        field.setMaximumNumberOfLines(0);
        field.setUsesSingleLineMode(false);
        field
    }
}

fn mono(text: &str, size: f64, color: &NSColor, rect: NSRect) -> Retained<NSTextField> {
    unsafe {
        let field = NSTextField::labelWithString(&NSString::from_str(text), mtm());
        field.setFrame(rect);
        field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(size, NSFontWeightRegular)));
        field.setTextColor(Some(color));
        field
    }
}

fn wire(control: &NSControl, action: objc2::runtime::Sel) {
    ACTIONS.with(|slot| {
        if let Some(target) = slot.borrow().as_ref() {
            unsafe {
                control.setTarget(Some(target));
                control.setAction(Some(action));
            }
        }
    });
}

fn tag(control: &NSControl, value: isize) {
    unsafe { control.setTag(value) };
}

fn button(text: &str, parent: &NSView, rect: NSRect, bordered: bool, action: objc2::runtime::Sel) -> Retained<NSButton> {
    unsafe {
        let b = NSButton::buttonWithTitle_target_action(&NSString::from_str(text), None, None, mtm());
        b.setFrame(rect);
        b.setBordered(bordered);
        wire(&b, action);
        parent.addSubview(&b);
        b
    }
}

fn checkbox(on: bool, parent: &NSView, rect: NSRect, action: objc2::runtime::Sel) -> Retained<NSButton> {
    unsafe {
        let b = NSButton::checkboxWithTitle_target_action(&NSString::from_str(""), None, None, mtm());
        b.setFrame(rect);
        b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        wire(&b, action);
        parent.addSubview(&b);
        b
    }
}

fn switch(on: bool, parent: &NSView, rect: NSRect, action: objc2::runtime::Sel) -> Retained<NSSwitch> {
    unsafe {
        let s = NSSwitch::new(mtm());
        s.setFrame(rect);
        s.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        wire(&s, action);
        parent.addSubview(&s);
        s
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
                NSSize::new(rect.size.width - 150.0, 16.0),
            );
            parent.addSubview(&label(detail, 12.0, NSFontWeightRegular, &theme.secondary, detail_frame));
        }
        rect.origin.x + rect.size.width - 16.0
    }
}

fn display_key(model: &Model, index: usize) -> Option<String> {
    model.displays.get(index).map(|display| display.key.clone())
}

fn key_of_selected(model: &Model) -> Option<String> {
    UI.with(|ui| display_key(model, ui.borrow().selected))
}

fn build_general(parent: &NSView, model: &Model, theme: &Theme) {
    unsafe {
        parent.addSubview(&label(
            "BEHAVIOUR",
            11.0,
            NSFontWeightSemibold,
            &theme.secondary,
            frame(HEIGHT, PAD, 64.0, 200.0, 14.0),
        ));

        let rows = [
            (
                "Reclaim displays on quit",
                "Bring every released display back before Lumen exits.",
                model.config.reclaim_on_quit,
                sel!(toggleReclaim:),
            ),
            (
                "Show brightness presets",
                "List 25/50/75/100% under each display in the menu.",
                model.config.show_brightness,
                sel!(toggleBrightness:),
            ),
        ];
        let mut y = 86.0;
        let rect = frame(HEIGHT, PAD, y, WIDTH - PAD * 2.0, 64.0);
        let right = card(theme, parent, rect, rows[0].0, rows[0].1);
        switch(rows[0].2, parent, NSRect::new(NSPoint::new(right - 38.0, HEIGHT - y - 32.0), NSSize::new(38.0, 22.0)), rows[0].3);
        y += 72.0;

        let rect = frame(HEIGHT, PAD, y, WIDTH - PAD * 2.0, 64.0);
        let right = card(theme, parent, rect, "Auto-reclaim", "Bring a released display back by itself after a while.");
        let current = model.config.auto_reclaim_minutes;
        let options = [None, Some(1u32), Some(5), Some(15)];
        let mut x = right - 4.0 * 58.0;
        for (index, (text, value)) in ["Off", "1 min", "5 min", "15 min"].iter().zip(options).enumerate() {
            let b = button(text, parent, frame(HEIGHT, x, y + 24.0, 54.0, 24.0), true, sel!(autoReclaim:));
            tag(&b, index as isize);
            if current == value {
                b.setState(NSControlStateValueOn);
            }
            x += 58.0;
        }
        y += 72.0;

        let rect = frame(HEIGHT, PAD, y, WIDTH - PAD * 2.0, 64.0);
        let right = card(theme, parent, rect, rows[1].0, rows[1].1);
        switch(rows[1].2, parent, NSRect::new(NSPoint::new(right - 38.0, HEIGHT - y - 32.0), NSSize::new(38.0, 22.0)), rows[1].3);

        parent.addSubview(&label(
            "ABOUT",
            11.0,
            NSFontWeightSemibold,
            &theme.secondary,
            frame(HEIGHT, PAD, 312.0, 200.0, 14.0),
        ));
        let about = frame(HEIGHT, PAD, 332.0, WIDTH - PAD * 2.0, 64.0);
        let _ = card(theme, parent, about, "", "");
        parent.addSubview(&paragraph(
            "Lumen switches inputs over DDC. When a monitor's firmware ignores input select, Release \
             cuts the signal instead and the monitor follows its other live input.",
            12.0,
            &theme.secondary,
            NSRect::new(
                NSPoint::new(about.origin.x + 16.0, about.origin.y + 16.0),
                NSSize::new(about.size.width - 32.0, 36.0),
            ),
        ));
    }
}

fn build_displays(parent: &NSView, model: &Model, theme: &Theme, selected: usize) {
    unsafe {
        let mut y = 64.0;
        for (index, display) in model.displays.iter().enumerate() {
            let b = button(&display.name, parent, frame(HEIGHT, PAD, y, 182.0, 30.0), true, sel!(selectDisplay:));
            tag(&b, index as isize);
            if index == selected {
                b.setState(NSControlStateValueOn);
            }
            y += 36.0;
        }
        if model.displays.is_empty() {
            parent.addSubview(&label("No DDC displays", 12.0, NSFontWeightRegular, &theme.secondary, frame(HEIGHT, PAD, y, 182.0, 16.0)));
        }

        let Some(display) = model.displays.get(selected) else {
            parent.addSubview(&label(
                "Connect a display to configure its inputs.",
                13.0,
                NSFontWeightRegular,
                &theme.secondary,
                frame(HEIGHT, 230.0, 64.0, 400.0, 18.0),
            ));
            return;
        };

        let key = display.key.clone();
        let released = model.released(&key);
        let show_in_menu = model.config.display(&key).map(|c| c.show_in_menu).unwrap_or(true);
        let inputs = model.config.display(&key).map(|c| c.inputs.clone()).unwrap_or_default();

        let mut y = 64.0;
        parent.addSubview(&label(&display.name, 15.0, NSFontWeightSemibold, &theme.text, frame(HEIGHT, 230.0, y, 300.0, 20.0)));
        let sw = switch(
            show_in_menu,
            parent,
            NSRect::new(NSPoint::new(WIDTH - PAD - 38.0, HEIGHT - y - 4.0), NSSize::new(38.0, 22.0)),
            sel!(toggleShowInMenu:),
        );
        let _ = sw;

        y += 26.0;
        let status = if released { "released" } else { "connected" };
        parent.addSubview(&mono(&format!("{key}  ·  {status}"), 11.0, &theme.secondary, frame(HEIGHT, 230.0, y, 460.0, 16.0)));

        y += 26.0;
        parent.addSubview(&label("MENU INPUTS", 11.0, NSFontWeightSemibold, &theme.secondary, frame(HEIGHT, 230.0, y, 200.0, 14.0)));
        y += 22.0;

        for (index, input) in inputs.iter().enumerate() {
            let row = frame(HEIGHT, 230.0, y, WIDTH - 230.0 - PAD, 34.0);
            let bx = NSBox::initWithFrame(mtm().alloc(), row);
            bx.setBoxType(NSBoxType::NSBoxCustom);
            bx.setTitlePosition(NSTitlePosition::NSNoTitle);
            bx.setBorderWidth(1.0);
            bx.setCornerRadius(8.0);
            bx.setFillColor(&theme.card);
            bx.setBorderColor(&theme.border);
            parent.addSubview(&bx);

            let cb = checkbox(
                input.enabled,
                parent,
                NSRect::new(NSPoint::new(row.origin.x + 10.0, row.origin.y + 8.0), NSSize::new(18.0, 18.0)),
                sel!(toggleInput:),
            );
            tag(&cb, index as isize);
            parent.addSubview(&label(
                &input.label,
                13.0,
                NSFontWeightMedium,
                if input.enabled { &theme.text } else { &theme.secondary },
                NSRect::new(NSPoint::new(row.origin.x + 36.0, row.origin.y + 9.0), NSSize::new(150.0, 18.0)),
            ));
            parent.addSubview(&mono(
                &format!("{:#06x}", input.value),
                11.0,
                &theme.secondary,
                NSRect::new(NSPoint::new(row.origin.x + row.size.width - 200.0, row.origin.y + 9.0), NSSize::new(80.0, 16.0)),
            ));
            let pb = button(
                input.protocol.label(),
                parent,
                NSRect::new(NSPoint::new(row.origin.x + row.size.width - 112.0, row.origin.y + 5.0), NSSize::new(70.0, 24.0)),
                true,
                sel!(cycleProtocol:),
            );
            tag(&pb, index as isize);
            let rb = button(
                "×",
                parent,
                NSRect::new(NSPoint::new(row.origin.x + row.size.width - 34.0, row.origin.y + 5.0), NSSize::new(24.0, 24.0)),
                false,
                sel!(removeInput:),
            );
            tag(&rb, index as isize);

            y += 40.0;
        }

        let add_y = y + 2.0;
        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            ui.draft_label = None;
            ui.draft_value = None;
        });
        let lf = NSTextField::initWithFrame(mtm().alloc(), frame(HEIGHT, 230.0, add_y, 140.0, 28.0));
        lf.setPlaceholderString(Some(&NSString::from_str("Label")));
        parent.addSubview(&lf);
        let vf = NSTextField::initWithFrame(mtm().alloc(), frame(HEIGHT, 378.0, add_y, 80.0, 28.0));
        vf.setPlaceholderString(Some(&NSString::from_str("0x00")));
        parent.addSubview(&vf);
        let protocol = UI.with(|ui| ui.borrow().draft_protocol);
        let _ = button(protocol.label(), parent, frame(HEIGHT, 466.0, add_y, 64.0, 28.0), true, sel!(cycleDraftProtocol:));
        let _ = button("Add", parent, frame(HEIGHT, 538.0, add_y, 64.0, 28.0), true, sel!(addInput:));
        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            ui.draft_label = Some(lf);
            ui.draft_value = Some(vf);
        });

        let _ = button("Reset to detected", parent, frame(HEIGHT, 230.0, add_y + 38.0, 150.0, 28.0), true, sel!(resetInputs:));
        parent.addSubview(&label(
            "Changes apply to the menu immediately.",
            12.0,
            NSFontWeightRegular,
            &theme.secondary,
            frame(HEIGHT, 392.0, add_y + 42.0, 240.0, 18.0),
        ));
    }
}

fn clear(view: &NSView) {
    unsafe {
        for subview in view.subviews().iter() {
            subview.removeFromSuperview();
        }
    }
}

fn build_content(window: &NSWindow, model: &Model, page: Page, selected: usize) {
    let theme = Theme::new();
    unsafe {
        let Some(content) = window.contentView() else {
            return;
        };
        clear(&content);

        content.addSubview(&label("Lumen", 16.0, NSFontWeightSemibold, &theme.text, frame(HEIGHT, PAD, 14.0, 70.0, 22.0)));
        content.addSubview(&label(
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
        tabs.setSelectedSegment(if page == Page::General { 0 } else { 1 });
        wire(&tabs, sel!(pageChanged:));
        content.addSubview(&tabs);

        match page {
            Page::General => build_general(&content, model, &theme),
            Page::Displays => build_displays(&content, model, &theme, selected),
        }
    }
}

fn make_actions() -> Retained<Actions> {
    let object = mtm().alloc::<Actions>().set_ivars(());
    unsafe { msg_send_id![super(object), init] }
}

/// Creates the shared action target and subscribes to display-configuration and wake events.
pub fn init() {
    let actions = ACTIONS.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(make_actions());
        }
        slot.clone().expect("actions")
    });

    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &actions,
            sel!(screensChanged:),
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
        );
        NSWorkspace::sharedWorkspace().notificationCenter().addObserver_selector_name_object(
            &actions,
            sel!(didWake:),
            Some(objc2_app_kit::NSWorkspaceDidWakeNotification),
            None,
        );
    }
}

pub fn open() {
    let start = std::time::Instant::now();
    let model = crate::with_model(|model| model.snapshot());
    let window = UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        if ACTIONS.with(|slot| slot.borrow().is_none()) {
            ACTIONS.with(|slot| *slot.borrow_mut() = Some(make_actions()));
        }
        if ui.window.is_none() {
            ui.window = Some(create_window());
        }
        ui.window.clone().unwrap()
    });

    build_content(&window, &model, UI.with(|ui| ui.borrow().page), UI.with(|ui| ui.borrow().selected));
    #[allow(deprecated)]
    {
        NSApplication::sharedApplication(mtm()).activateIgnoringOtherApps(true);
    }
    window.makeKeyAndOrderFront(None);
    if std::env::var_os("LUMEN_TRACE").is_some() {
        eprintln!("settings open: {} ms", start.elapsed().as_millis());
    }
}

pub fn rebuild_if_open() {
    let window = UI.with(|ui| ui.borrow().window.clone());
    let Some(window) = window else {
        return;
    };
    let visible = window.isVisible();
    if !visible {
        return;
    }
    let model = crate::with_model(|model| model.snapshot());
    let (page, selected) = UI.with(|ui| {
        let ui = ui.borrow();
        (ui.page, ui.selected)
    });
    build_content(&window, &model, page, selected);
}

fn create_window() -> Retained<NSWindow> {
    unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm().alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        );
        window.setTitle(&NSString::from_str("Lumen Settings"));
        window.setReleasedWhenClosed(false);
        window.center();
        ACTIONS.with(|slot| {
            if let Some(actions) = slot.borrow().as_ref() {
                window.setDelegate(Some(ProtocolObject::from_ref(&**actions)));
            }
        });
        window
    }
}

/// Reads a control's `tag`.
fn sender_tag(sender: *mut AnyObject) -> isize {
    unsafe { msg_send![sender, tag] }
}

declare_class!(
    struct Actions;

    unsafe impl ClassType for Actions {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "LumenActions";
    }

    impl DeclaredClass for Actions {
        type Ivars = ();
    }

    unsafe impl NSObjectProtocol for Actions {}

    unsafe impl Actions {
        #[method(screensChanged:)]
        fn screens_changed(&self, _notification: *mut AnyObject) {
            crate::schedule_rescan();
        }

        #[method(didWake:)]
        fn did_wake(&self, _notification: *mut AnyObject) {
            crate::schedule_rescan();
        }

        #[method(pageChanged:)]
        fn page_changed(&self, sender: *mut AnyObject) {
            let segment: isize = unsafe { msg_send![sender, selectedSegment] };
            UI.with(|ui| ui.borrow_mut().page = if segment == 1 { Page::Displays } else { Page::General });
            crate::after_change();
        }

        #[method(toggleReclaim:)]
        fn toggle_reclaim(&self, _sender: *mut AnyObject) {
            crate::with_model(|model| {
                model.config.reclaim_on_quit = !model.config.reclaim_on_quit;
                model.config.save();
            });
            crate::after_change();
        }

        #[method(toggleBrightness:)]
        fn toggle_brightness(&self, _sender: *mut AnyObject) {
            crate::with_model(|model| {
                model.config.show_brightness = !model.config.show_brightness;
                model.config.save();
            });
            crate::after_change();
        }

        #[method(autoReclaim:)]
        fn auto_reclaim(&self, sender: *mut AnyObject) {
            let tag = sender_tag(sender);
            let minutes = match tag {
                1 => Some(1),
                2 => Some(5),
                3 => Some(15),
                _ => None,
            };
            crate::with_model(|model| {
                model.config.auto_reclaim_minutes = minutes;
                model.config.save();
            });
            crate::after_change();
        }

        #[method(selectDisplay:)]
        fn select_display(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender).max(0) as usize;
            UI.with(|ui| ui.borrow_mut().selected = index);
            crate::after_change();
        }

        #[method(toggleShowInMenu:)]
        fn toggle_show_in_menu(&self, _sender: *mut AnyObject) {
            let key = crate::with_model(|model| key_of_selected(model));
            let Some(key) = key else { return };
            crate::with_model(|model| {
                if let Some(config) = model.config.display_mut(&key) {
                    config.show_in_menu = !config.show_in_menu;
                }
                model.config.save();
            });
            crate::after_change();
        }

        #[method(toggleInput:)]
        fn toggle_input(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender).max(0) as usize;
            let key = crate::with_model(|model| key_of_selected(model));
            let Some(key) = key else { return };
            crate::with_model(|model| {
                if let Some(config) = model.config.display_mut(&key) {
                    if let Some(input) = config.inputs.get_mut(index) {
                        input.enabled = !input.enabled;
                    }
                }
                model.config.save();
            });
            crate::after_change();
        }

        #[method(cycleProtocol:)]
        fn cycle_protocol(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender).max(0) as usize;
            let key = crate::with_model(|model| key_of_selected(model));
            let Some(key) = key else { return };
            crate::with_model(|model| {
                if let Some(config) = model.config.display_mut(&key) {
                    if let Some(input) = config.inputs.get_mut(index) {
                        input.protocol = flip(input.protocol);
                    }
                }
                model.config.save();
            });
            crate::after_change();
        }

        #[method(removeInput:)]
        fn remove_input(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender).max(0) as usize;
            let key = crate::with_model(|model| key_of_selected(model));
            let Some(key) = key else { return };
            crate::with_model(|model| {
                if let Some(config) = model.config.display_mut(&key) {
                    if index < config.inputs.len() {
                        config.inputs.remove(index);
                    }
                }
                model.config.save();
            });
            crate::after_change();
        }

        #[method(cycleDraftProtocol:)]
        fn cycle_draft_protocol(&self, _sender: *mut AnyObject) {
            UI.with(|ui| {
                let mut ui = ui.borrow_mut();
                ui.draft_protocol = flip(ui.draft_protocol);
            });
            crate::after_change();
        }

        #[method(addInput:)]
        fn add_input(&self, _sender: *mut AnyObject) {
            let (raw, label, protocol) = UI.with(|ui| {
                let ui = ui.borrow();
                let raw = ui.draft_value.as_ref().map(|f| unsafe { f.stringValue().to_string() }).unwrap_or_default();
                let label = ui.draft_label.as_ref().map(|f| unsafe { f.stringValue().to_string() }).unwrap_or_default();
                (raw, label, ui.draft_protocol)
            });
            let raw = raw.trim().trim_start_matches("0x").to_string();
            let Ok(value) = u16::from_str_radix(&raw, 16) else {
                return;
            };
            let label = if label.trim().is_empty() { format!("Input {value:#06x}") } else { label.trim().to_string() };
            let key = crate::with_model(|model| key_of_selected(model));
            let Some(key) = key else { return };
            crate::with_model(|model| {
                if let Some(config) = model.config.display_mut(&key) {
                    config.inputs.push(InputSetting { label, value, protocol, enabled: true });
                }
                model.config.save();
            });
            crate::after_change();
        }

        #[method(resetInputs:)]
        fn reset_inputs(&self, _sender: *mut AnyObject) {
            let key = crate::with_model(|model| key_of_selected(model));
            let Some(key) = key else { return };
            crate::with_model(|model| {
                let displays = model.displays.clone();
                model.config.reset_inputs(&key, &displays);
            });
            crate::after_change();
        }
    }

    unsafe impl NSWindowDelegate for Actions {
        #[method(windowWillClose:)]
        fn window_will_close(&self, _notification: &objc2_foundation::NSNotification) {
            // Drop the window once the close notification has finished unwinding, so its views
            // and layer go back with it. Reopening builds a fresh one.
            STALE.with(|stale| {
                *stale.borrow_mut() = UI.with(|ui| ui.borrow_mut().window.take());
            });
            dispatch::Queue::main().exec_async(|| {
                STALE.with(|stale| {
                    let _ = stale.borrow_mut().take();
                });
            });
        }
    }

);

fn flip(protocol: Protocol) -> Protocol {
    match protocol {
        Protocol::Standard => Protocol::LgAlt,
        Protocol::LgAlt => Protocol::Standard,
    }
}
