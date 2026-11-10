#![allow(unsafe_op_in_unsafe_fn)]

//! The AppKit settings window.
//!
//! A source list on the left and one scrolling page on the right. Groups are rounded boxes with
//! rows, following the shape of System Settings. Layout inside a page is still explicit: a page
//! builder walks down a flipped document view and records the height it needs. Controls carry `tag`
//! values that the `Actions` target uses to find the row they belong to.

use crate::config::{InputSetting, Protocol};
use crate::model::Model;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication,
    NSApplicationDidChangeScreenParametersNotification, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSBox, NSBoxType, NSButton, NSColor, NSControl, NSControlStateValueOff, NSControlStateValueOn,
    NSFont, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold, NSImage, NSImageView,
    NSAccessibility, NSForegroundColorAttributeName, NSLineBreakMode,
    NSMenu, NSMenuDelegate,
    NSPasteboard, NSPasteboardTypeString,
    NSPopUpButton, NSScrollView, NSSlider, NSSwitch, NSTextAlignment, NSTextField, NSTickMarkPosition,
    NSTitlePosition, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowDelegate, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSMutableAttributedString, NSNotification, NSNotificationCenter, NSObject,
    NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString,
};
use std::cell::RefCell;

const WIDTH: f64 = 920.0;
const HEIGHT: f64 = 600.0;
const MIN_WIDTH: f64 = 880.0;
const MIN_HEIGHT: f64 = 500.0;
const SIDEBAR: f64 = 200.0;
const MARGIN: f64 = 28.0;
const ROW: f64 = 54.0;

const PAGES: [&str; 4] = ["General", "Displays", "Virtual Displays", "About"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Displays,
    Virtual,
    About,
}

impl Page {
    fn from_row(row: isize) -> Self {
        match row {
            1 => Page::Displays,
            2 => Page::Virtual,
            3 => Page::About,
            _ => Page::General,
        }
    }
}

/// Text being edited when a rescan rebuilds the virtual display page.
struct VirtualDrafts {
    names: Vec<String>,
    widths: Vec<String>,
    heights: Vec<String>,
    refreshes: Vec<String>,
    url: Option<String>,
}

pub struct Ui {
    pub window: Option<Retained<NSWindow>>,
    pub page: Page,
    pub selected: usize,
    host: Option<Retained<NSView>>,
    scroll: Option<Retained<NSScrollView>>,
    built_page: Option<Page>,
    login_note: Option<String>,
    brightness_value: Option<Retained<NSTextField>>,
    contrast_value: Option<Retained<NSTextField>>,
    volume_value: Option<Retained<NSTextField>>,
    draft_label: Option<Retained<NSTextField>>,
    draft_value: Option<Retained<NSTextField>>,
    draft_protocol: Protocol,
    agent_url: Option<Retained<NSTextField>>,
    screen_names: Vec<Retained<NSTextField>>,
    screen_widths: Vec<Retained<NSTextField>>,
    screen_heights: Vec<Retained<NSTextField>>,
    screen_refreshes: Vec<Retained<NSTextField>>,
    screen_hidpi: Vec<Retained<NSButton>>,
    screen_auto: Vec<Retained<NSButton>>,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            window: None,
            page: Page::General,
            selected: 0,
            host: None,
            scroll: None,
            built_page: None,
            login_note: None,
            brightness_value: None,
            contrast_value: None,
            volume_value: None,
            draft_label: None,
            draft_value: None,
            draft_protocol: Protocol::LgAlt,
            agent_url: None,
            screen_names: Vec::new(),
            screen_widths: Vec::new(),
            screen_heights: Vec::new(),
            screen_refreshes: Vec::new(),
            screen_hidpi: Vec::new(),
            screen_auto: Vec::new(),
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

#[derive(Clone)]
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

fn stretch(control: &NSView) {
    unsafe {
        control.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewWidthSizable);
    }
}

fn pin_right(control: &NSView) {
    unsafe {
        control.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewMinXMargin);
    }
}

fn pin_top(control: &NSView) {
    unsafe {
        control.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewMinYMargin);
    }
}

fn button(
    text: &str,
    parent: &NSView,
    rect: NSRect,
    bordered: bool,
    action: objc2::runtime::Sel,
) -> Retained<NSButton> {
    unsafe {
        let b = NSButton::buttonWithTitle_target_action(&NSString::from_str(text), None, None, mtm());
        b.setFrame(rect);
        b.setBordered(bordered);
        b.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular)));
        wire(&b, action);
        parent.addSubview(&b);
        b
    }
}

fn checkbox(
    on: bool,
    parent: &NSView,
    rect: NSRect,
    action: objc2::runtime::Sel,
) -> Retained<NSButton> {
    unsafe {
        let b = NSButton::checkboxWithTitle_target_action(&NSString::from_str(""), None, None, mtm());
        b.setFrame(rect);
        b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        wire(&b, action);
        parent.addSubview(&b);
        b
    }
}

fn switch(
    on: bool,
    parent: &NSView,
    rect: NSRect,
    action: objc2::runtime::Sel,
) -> Retained<NSSwitch> {
    unsafe {
        let s = NSSwitch::new(mtm());
        s.setFrame(rect);
        s.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        wire(&s, action);
        parent.addSubview(&s);
        s
    }
}

fn option_box(title: &str, on: bool, parent: &NSView, rect: NSRect) -> Retained<NSButton> {
    unsafe {
        let b = NSButton::checkboxWithTitle_target_action(&NSString::from_str(title), None, None, mtm());
        b.setFrame(rect);
        b.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular)));
        b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        parent.addSubview(&b);
        b
    }
}

fn number_field(
    value: &str,
    parent: &NSView,
    rect: NSRect,
    tag_value: isize,
) -> Retained<NSTextField> {
    unsafe {
        let field = NSTextField::initWithFrame(mtm().alloc(), rect);
        field.setStringValue(&NSString::from_str(value));
        field.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular)));
        field.setBezeled(true);
        field.setEditable(true);
        field.setSelectable(true);
        field.setTag(tag_value);
        parent.addSubview(&field);
        field
    }
}

fn text_field(value: &str, parent: &NSView, rect: NSRect, placeholder: Option<&str>) -> Retained<NSTextField> {
    unsafe {
        let field = NSTextField::initWithFrame(mtm().alloc(), rect);
        field.setStringValue(&NSString::from_str(value));
        field.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular)));
        field.setBezeled(true);
        field.setEditable(true);
        field.setSelectable(true);
        if let Some(placeholder) = placeholder {
            field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
        }
        parent.addSubview(&field);
        field
    }
}

fn popup(
    parent: &NSView,
    rect: NSRect,
    items: &[&str],
    selected: usize,
    action: objc2::runtime::Sel,
) -> Retained<NSPopUpButton> {
    unsafe {
        let p = NSPopUpButton::initWithFrame_pullsDown(mtm().alloc(), rect, false);
        for item in items {
            p.addItemWithTitle(&NSString::from_str(item));
        }
        p.selectItemAtIndex(selected.min(items.len().saturating_sub(1)) as isize);
        p.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular)));
        wire(&p, action);
        parent.addSubview(&p);
        p
    }
}

/// Red title for a destructive button.
unsafe fn destructive(button: &NSButton) {
    let text = button.title();
    let attributed = NSMutableAttributedString::initWithString(mtm().alloc(), &text);
    let _: () = msg_send![
        &attributed,
        addAttribute: NSForegroundColorAttributeName,
        value: &*NSColor::systemRedColor(),
        range: NSRange::new(0, text.length())
    ];
    button.setAttributedTitle(&attributed);
}

fn card_box(theme: &Theme, parent: &NSView, rect: NSRect) -> Retained<NSBox> {
    unsafe {
        let bx = NSBox::initWithFrame(mtm().alloc(), rect);
        bx.setBoxType(NSBoxType::NSBoxCustom);
        bx.setTitlePosition(NSTitlePosition::NSNoTitle);
        bx.setBorderWidth(1.0);
        bx.setCornerRadius(10.0);
        bx.setFillColor(&theme.card);
        bx.setBorderColor(&theme.border);
        parent.addSubview(&bx);
        bx
    }
}

fn separator_line(theme: &Theme, parent: &NSView, rect: NSRect) {
    unsafe {
        let line = NSBox::initWithFrame(mtm().alloc(), rect);
        line.setBoxType(NSBoxType::NSBoxCustom);
        line.setTitlePosition(NSTitlePosition::NSNoTitle);
        line.setBorderWidth(0.0);
        line.setCornerRadius(0.0);
        line.setFillColor(&theme.border);
        line.setBorderColor(&theme.border);
        stretch(&line);
        parent.addSubview(&line);
    }
}

/// Soft rounded background used for the selected sidebar or display row.
unsafe fn selection_background(parent: &NSView, rect: NSRect) -> Retained<NSBox> {
    let bg = NSBox::initWithFrame(mtm().alloc(), rect);
    bg.setBoxType(NSBoxType::NSBoxCustom);
    bg.setTitlePosition(NSTitlePosition::NSNoTitle);
    bg.setBorderWidth(0.0);
    bg.setCornerRadius(7.0);
    bg.setFillColor(&NSColor::unemphasizedSelectedContentBackgroundColor());
    parent.addSubview(&bg);
    bg
}

// Document view for scrolling content. Flipped so the page builder can walk downwards.
declare_class!(
    struct FlippedView;

    unsafe impl ClassType for FlippedView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "LumenFlippedView";
    }

    impl DeclaredClass for FlippedView {
        type Ivars = ();
    }

    unsafe impl NSObjectProtocol for FlippedView {}

    unsafe impl FlippedView {
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    fn new(frame: NSRect) -> Retained<Self> {
        let view = mtm().alloc::<FlippedView>().set_ivars(());
        unsafe { msg_send_id![super(view), initWithFrame: frame] }
    }
}

/// Builds one page from the top down and reports the height it used.
struct PageBuilder<'a> {
    doc: &'a NSView,
    theme: Theme,
    width: f64,
    y: f64,
}

impl<'a> PageBuilder<'a> {
    fn new(doc: &'a NSView, theme: &Theme, width: f64) -> Self {
        Self {
            doc,
            theme: theme.clone(),
            width,
            y: MARGIN,
        }
    }

    unsafe fn section(&mut self, title: &str) {
        if self.y > MARGIN {
            self.y += 10.0;
        }
        self.doc.addSubview(&label(
            title,
            11.0,
            NSFontWeightSemibold,
            &self.theme.secondary,
            NSRect::new(NSPoint::new(MARGIN, self.y), NSSize::new(self.width - MARGIN * 2.0, 14.0)),
        ));
        self.y += 24.0;
    }

    /// A section header with a button on the trailing edge. Returns the button.
    unsafe fn section_button(
        &mut self,
        title: &str,
        button_title: &str,
        enabled: bool,
        action: objc2::runtime::Sel,
    ) -> Retained<NSButton> {
        if self.y > MARGIN {
            self.y += 10.0;
        }
        self.doc.addSubview(&label(
            title,
            11.0,
            NSFontWeightSemibold,
            &self.theme.secondary,
            NSRect::new(NSPoint::new(MARGIN, self.y), NSSize::new(self.width - MARGIN * 2.0, 14.0)),
        ));
        let b = button(
            button_title,
            &self.doc,
            NSRect::new(
                NSPoint::new(self.width - MARGIN - 140.0, self.y - 5.0),
                NSSize::new(140.0, 24.0),
            ),
            true,
            action,
        );
        b.setEnabled(enabled);
        pin_right(&b);
        self.y += 24.0;
        b
    }

    fn group(&self) -> Group<'a> {
        Group::new(self.doc, &self.theme, self.width, self.y)
    }

    fn end(&mut self, group: Group<'a>) {
        self.y = group.finish();
    }

    unsafe fn note(&mut self, text: &str) {
        let height = 30.0;
        self.doc.addSubview(&paragraph(
            text,
            11.0,
            &self.theme.secondary,
            NSRect::new(
                NSPoint::new(MARGIN + 2.0, self.y),
                NSSize::new(self.width - MARGIN * 2.0 - 4.0, height),
            ),
        ));
        self.y += height + 10.0;
    }

    /// Extra vertical room before the next block.
    fn gap(&mut self, height: f64) {
        self.y += height;
    }
}

struct Group<'a> {
    doc: &'a NSView,
    theme: Theme,
    box_: Retained<NSBox>,
    left: f64,
    right: f64,
    top: f64,
    y: f64,
    width: f64,
}

impl<'a> Group<'a> {
    fn new(doc: &'a NSView, theme: &Theme, page_width: f64, top: f64) -> Self {
        let width = page_width - MARGIN * 2.0;
        let bx = card_box(
            theme,
            doc,
            NSRect::new(NSPoint::new(MARGIN, top), NSSize::new(width, 1.0)),
        );
        stretch(&bx);
        Self {
            doc,
            theme: theme.clone(),
            box_: bx,
            left: MARGIN + 16.0,
            right: MARGIN + width - 16.0,
            top,
            y: top + 6.0,
            width,
        }
    }

    fn separator(&mut self) {
        separator_line(
            &self.theme,
            self.doc,
            NSRect::new(
                NSPoint::new(self.left, self.y),
                NSSize::new(self.width - 32.0, 1.0),
            ),
        );
        self.y += 1.0;
    }

    unsafe fn toggle(
        &mut self,
        title: &str,
        detail: &str,
        on: bool,
        action: objc2::runtime::Sel,
    ) -> Retained<NSSwitch> {
        let text_width = self.right - self.left - 70.0;
        let title_field = label(
            title,
            13.0,
            NSFontWeightMedium,
            &self.theme.text,
            NSRect::new(NSPoint::new(self.left, self.y + 8.0), NSSize::new(text_width, 18.0)),
        );
        stretch(&title_field);
        self.doc.addSubview(&title_field);
        let detail_field = label(
            detail,
            11.0,
            NSFontWeightRegular,
            &self.theme.secondary,
            NSRect::new(NSPoint::new(self.left, self.y + 27.0), NSSize::new(text_width, 16.0)),
        );
        stretch(&detail_field);
        self.doc.addSubview(&detail_field);
        let sw = switch(
            on,
            &self.doc,
            NSRect::new(
                NSPoint::new(self.right - 38.0, self.y + 16.0),
                NSSize::new(38.0, 22.0),
            ),
            action,
        );
        pin_right(&sw);
        unsafe {
            sw.setAccessibilityLabel(Some(&NSString::from_str(title)));
        }
        self.y += ROW;
        sw
    }

    unsafe fn popup(
        &mut self,
        title: &str,
        detail: &str,
        items: &[&str],
        selected: usize,
        action: objc2::runtime::Sel,
    ) -> Retained<NSPopUpButton> {
        let text_width = self.right - self.left - 190.0;
        let title_field = label(
            title,
            13.0,
            NSFontWeightMedium,
            &self.theme.text,
            NSRect::new(NSPoint::new(self.left, self.y + 8.0), NSSize::new(text_width, 18.0)),
        );
        stretch(&title_field);
        self.doc.addSubview(&title_field);
        let detail_field = label(
            detail,
            11.0,
            NSFontWeightRegular,
            &self.theme.secondary,
            NSRect::new(NSPoint::new(self.left, self.y + 27.0), NSSize::new(text_width, 16.0)),
        );
        stretch(&detail_field);
        self.doc.addSubview(&detail_field);
        let p = popup(
            &self.doc,
            NSRect::new(
                NSPoint::new(self.right - 170.0, self.y + 14.0),
                NSSize::new(170.0, 26.0),
            ),
            items,
            selected,
            action,
        );
        pin_right(&p);
        unsafe {
            p.setAccessibilityLabel(Some(&NSString::from_str(title)));
        }
        self.y += ROW;
        p
    }

    unsafe fn field(
        &mut self,
        title: &str,
        detail: &str,
        value: &str,
        placeholder: &str,
        width: f64,
        tag_value: isize,
    ) -> Retained<NSTextField> {
        let text_width = self.right - self.left - width - 20.0;
        let title_field = label(
            title,
            13.0,
            NSFontWeightMedium,
            &self.theme.text,
            NSRect::new(NSPoint::new(self.left, self.y + 8.0), NSSize::new(text_width, 18.0)),
        );
        stretch(&title_field);
        self.doc.addSubview(&title_field);
        let detail_field = label(
            detail,
            11.0,
            NSFontWeightRegular,
            &self.theme.secondary,
            NSRect::new(NSPoint::new(self.left, self.y + 27.0), NSSize::new(text_width, 16.0)),
        );
        stretch(&detail_field);
        self.doc.addSubview(&detail_field);
        let f = text_field(
            value,
            &self.doc,
            NSRect::new(
                NSPoint::new(self.right - width, self.y + 14.0),
                NSSize::new(width, 26.0),
            ),
            Some(placeholder),
        );
        f.setTag(tag_value);
        pin_right(&f);
        self.y += ROW;
        f
    }


    /// One brightness-style slider with a live percentage on the right.
    unsafe fn slider(
        &mut self,
        title: &str,
        value: u16,
        max: u16,
        tag_value: isize,
    ) -> Retained<NSTextField> {
        self.doc.addSubview(&label(
            title,
            12.0,
            NSFontWeightMedium,
            &self.theme.text,
            NSRect::new(NSPoint::new(self.left, self.y + 9.0), NSSize::new(100.0, 18.0)),
        ));
        let slider = unsafe {
            let slider = NSSlider::initWithFrame(
                mtm().alloc(),
                NSRect::new(
                    NSPoint::new(self.left + 110.0, self.y + 6.0),
                    NSSize::new(self.right - self.left - 180.0, 22.0),
                ),
            );
            slider.setMinValue(0.0);
            slider.setMaxValue(max.max(1) as f64);
            slider.setDoubleValue(value as f64);
            slider.setContinuous(true);
            slider.setNumberOfTickMarks(5);
            slider.setAllowsTickMarkValuesOnly(false);
            slider.setTickMarkPosition(NSTickMarkPosition::Below);
            slider.setTag(tag_value);
            wire(&slider, sel!(controlChanged:));
            self.doc.addSubview(&slider);
            slider
        };
        stretch(&slider);
        let text = label(
            &format!("{}%", percent_of(value, max)),
            11.0,
            NSFontWeightRegular,
            &self.theme.secondary,
            NSRect::new(NSPoint::new(self.right - 56.0, self.y + 9.0), NSSize::new(56.0, 16.0)),
        );
        pin_right(&text);
        self.doc.addSubview(&text);
        self.y += 36.0;
        text
    }

    fn finish(self) -> f64 {
        let height = (self.y - self.top) + 6.0;
        unsafe {
            self.box_.setFrameSize(NSSize::new(self.width, height));
        }
        self.top + height + 18.0
    }
}

fn percent_of(value: u16, max: u16) -> u32 {
    if max == 0 {
        return 0;
    }
    value as u32 * 100 / max as u32
}

fn key_of_selected(model: &Model) -> Option<String> {
    UI.with(|ui| {
        model
            .displays
            .get(ui.borrow().selected)
            .map(|display| display.key.clone())
    })
}

unsafe fn build_general(parent: &NSView, model: &Model, theme: &Theme) {
    let mut pb = PageBuilder::new(parent, theme, parent.frame().size.width);

    pb.section("Menu Bar");
    let mut group = pb.group();
    group.toggle(
        "Show brightness presets",
        "List 25, 50, 75 and 100 percent under each display in the menu.",
        model.config.show_brightness,
        sel!(toggleBrightness:),
    );
    pb.end(group);

    pb.section("Displays");
    let mut group = pb.group();
    group.toggle(
        "Reclaim displays on quit",
        "Bring every released display back before Lumen exits.",
        model.config.reclaim_on_quit,
        sel!(toggleReclaim:),
    );
    group.separator();
    let current = match model.config.auto_reclaim_minutes {
        None => 0,
        Some(1) => 1,
        Some(5) => 2,
        Some(15) => 3,
        Some(_) => 4,
    };
    group.popup(
        "Auto-reclaim",
        "Bring a released display back by itself after a while.",
        &["Off", "After 1 minute", "After 5 minutes", "After 15 minutes", "After 30 minutes"],
        current,
        sel!(autoReclaim:),
    );
    pb.end(group);

    pb.section("Startup");
    let status = crate::login::status();
    let (detail, on) = match status {
        crate::login::Status::Enabled => ("Starts Lumen when you log in.", true),
        crate::login::Status::RequiresApproval => (
            "Approve Lumen in System Settings > General > Login Items.",
            true,
        ),
        crate::login::Status::NotFound => ("macOS could not find this app's login item.", false),
        crate::login::Status::Unavailable => ("Login items need macOS 13 or later.", false),
        _ => ("Starts Lumen when you log in.", false),
    };
    let mut group = pb.group();
    group.toggle("Launch at login", detail, on, sel!(toggleLoginItem:));
    pb.end(group);
    if let Some(note) = UI.with(|ui| ui.borrow().login_note.clone()) {
        pb.note(&note);
    }
}

unsafe fn build_displays(host: &NSView, model: &Model, theme: &Theme) {
    let bounds = host.bounds();
    let list_width = 196.0;
    if model.displays.is_empty() {
        let height = bounds.size.height;
        host.addSubview(&label(
            "No DDC displays",
            15.0,
            NSFontWeightSemibold,
            &theme.text,
            NSRect::new(NSPoint::new(MARGIN, height - 70.0), NSSize::new(320.0, 22.0)),
        ));
        host.addSubview(&paragraph(
            "Lumen talks to monitors over DDC/CI. Connect one, or wake it, then scan again. A \
             released display stays off until it is reclaimed.",
            12.0,
            &theme.secondary,
            NSRect::new(
                NSPoint::new(MARGIN, height - 132.0),
                NSSize::new(bounds.size.width - MARGIN * 2.0, 52.0),
            ),
        ));
        let _ = button(
            "Scan displays",
            host,
            NSRect::new(NSPoint::new(MARGIN, height - 172.0), NSSize::new(120.0, 28.0)),
            true,
            sel!(refreshDisplays:),
        );
        return;
    }

    let selected = UI.with(|ui| ui.borrow().selected).min(model.displays.len() - 1);
    UI.with(|ui| ui.borrow_mut().selected = selected);

    unsafe {
        for (index, display) in model.displays.iter().enumerate() {
            let row = NSRect::new(
                NSPoint::new(8.0, bounds.size.height - 44.0 - index as f64 * 38.0),
                NSSize::new(list_width - 16.0, 34.0),
            );
            if index == selected {
                let background = selection_background(host, row);
                pin_top(&background);
            }
            let b = button(
                &display.name,
                host,
                NSRect::new(
                    NSPoint::new(row.origin.x + 10.0, row.origin.y + 6.0),
                    NSSize::new(row.size.width - 12.0, 22.0),
                ),
                false,
                sel!(selectDisplay:),
            );
            tag(&b, index as isize);
            b.setAlignment(NSTextAlignment::Left);
            b.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, NSFontWeightRegular)));
            pin_top(&b);
        }

        let line = NSBox::initWithFrame(
            mtm().alloc(),
            NSRect::new(NSPoint::new(list_width, 0.0), NSSize::new(1.0, bounds.size.height)),
        );
        line.setBoxType(NSBoxType::NSBoxCustom);
        line.setTitlePosition(NSTitlePosition::NSNoTitle);
        line.setBorderWidth(0.0);
        line.setCornerRadius(0.0);
        line.setFillColor(&theme.border);
        line.setBorderColor(&theme.border);
        line.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewHeightSizable);
        host.addSubview(&line);
    }

    let detail_width = bounds.size.width - list_width - 1.0;
    let scroll = unsafe {
        let scroll = NSScrollView::initWithFrame(
            mtm().alloc(),
            NSRect::new(
                NSPoint::new(list_width + 1.0, 0.0),
                NSSize::new(detail_width, bounds.size.height),
            ),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::NSViewWidthSizable
                | NSAutoresizingMaskOptions::NSViewHeightSizable,
        );
        host.addSubview(&scroll);
        scroll
    };
    let doc = FlippedView::new(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(detail_width, bounds.size.height),
    ));
    unsafe {
        doc.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewWidthSizable);
    }
    unsafe {
        scroll.setDocumentView(Some(&doc));
    }

    let display = &model.displays[selected];
    let key = display.key.clone();
    let released = model.released(&key);
    let show_in_menu = model
        .config
        .display(&key)
        .map(|config| config.show_in_menu)
        .unwrap_or(true);
    let inputs = model
        .config
        .display(&key)
        .map(|config| config.inputs.clone())
        .unwrap_or_default();

    let mut pb = PageBuilder::new(&doc, theme, detail_width);

    pb.gap(-MARGIN + 24.0);
    doc.addSubview(&label(
        &display.name,
        17.0,
        NSFontWeightSemibold,
        &theme.text,
        NSRect::new(NSPoint::new(MARGIN, 22.0), NSSize::new(detail_width - MARGIN * 2.0, 22.0)),
    ));
    doc.addSubview(&mono(
        &format!("{key}  ·  {}", if released { "released" } else { "connected" }),
        11.0,
        &theme.secondary,
        NSRect::new(NSPoint::new(MARGIN, 46.0), NSSize::new(detail_width - MARGIN * 2.0, 16.0)),
    ));
    pb.gap(76.0);

    pb.section("Menu");
    let mut group = pb.group();
    group.toggle(
        "Show in menu",
        "List this display and its inputs in the menu bar menu.",
        show_in_menu,
        sel!(toggleShowInMenu:),
    );
    pb.end(group);

    pb.section("Controls");
    let mut group = pb.group();
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.brightness_value = None;
        ui.contrast_value = None;
        ui.volume_value = None;
    });
    let mut controls = 0;
    if let Some((value, max)) = display.brightness {
        let text = group.slider("Brightness", value, max, 1);
        UI.with(|ui| ui.borrow_mut().brightness_value = Some(text));
        controls += 1;
    }
    if let Some((value, max)) = display.contrast {
        if controls > 0 {
            group.separator();
        }
        let text = group.slider("Contrast", value, max, 2);
        UI.with(|ui| ui.borrow_mut().contrast_value = Some(text));
        controls += 1;
    }
    if let Some((value, max)) = display.volume {
        if controls > 0 {
            group.separator();
        }
        let text = group.slider("Volume", value, max, 3);
        UI.with(|ui| ui.borrow_mut().volume_value = Some(text));
    }
    pb.end(group);

    pb.section("Inputs");
    let mut group = pb.group();
    for (index, input) in inputs.iter().enumerate() {
        build_input_row(&mut group, theme, index, input);
        if index + 1 < inputs.len() {
            group.separator();
        }
    }
    if inputs.is_empty() {
        group.doc.addSubview(&label(
            "No inputs configured",
            12.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(NSPoint::new(group.left, group.y + 10.0), NSSize::new(240.0, 18.0)),
        ));
        group.y += 36.0;
    }
    // Add row.
    if !inputs.is_empty() {
        group.separator();
    }
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.draft_label = None;
        ui.draft_value = None;
    });
    let lf = text_field(
        "",
        &group.doc,
        NSRect::new(NSPoint::new(group.left, group.y + 9.0), NSSize::new(140.0, 26.0)),
        Some("Label"),
    );
    let vf = text_field(
        "",
        &group.doc,
        NSRect::new(NSPoint::new(group.left + 148.0, group.y + 9.0), NSSize::new(80.0, 26.0)),
        Some("0x00"),
    );
    let protocol = UI.with(|ui| ui.borrow().draft_protocol);
    let _ = button(
        protocol.label(),
        &group.doc,
        NSRect::new(NSPoint::new(group.left + 236.0, group.y + 9.0), NSSize::new(70.0, 26.0)),
        true,
        sel!(cycleDraftProtocol:),
    );
    let _ = button(
        "Add",
        &group.doc,
        NSRect::new(NSPoint::new(group.left + 314.0, group.y + 9.0), NSSize::new(60.0, 26.0)),
        true,
        sel!(addInput:),
    );
    group.y += 44.0;
    group.separator();
    let reset = button(
        "Reset to detected",
        &group.doc,
        NSRect::new(NSPoint::new(group.left, group.y + 9.0), NSSize::new(140.0, 26.0)),
        true,
        sel!(resetInputs:),
    );
    pin_right(&reset);
    unsafe {
        reset.setFrameOrigin(NSPoint::new(group.right - 140.0, group.y + 9.0));
    }
    group.doc.addSubview(&label(
        "Changes apply to the menu immediately.",
        11.0,
        NSFontWeightRegular,
        &theme.secondary,
        NSRect::new(NSPoint::new(group.left, group.y + 13.0), NSSize::new(240.0, 16.0)),
    ));
    group.y += 44.0;
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.draft_label = Some(lf);
        ui.draft_value = Some(vf);
    });
    pb.end(group);

    let height = (pb.y + MARGIN).max(bounds.size.height);
    unsafe {
        doc.setFrameSize(NSSize::new(detail_width, height));
    }
    let _ = height;
}

unsafe fn build_input_row(group: &mut Group, theme: &Theme, index: usize, input: &InputSetting) {
    let row_y = group.y;
    let cb = checkbox(
        input.enabled,
        &group.doc,
        NSRect::new(NSPoint::new(group.left, row_y + 10.0), NSSize::new(18.0, 18.0)),
        sel!(toggleInput:),
    );
    tag(&cb, index as isize);
    unsafe {
        cb.setAccessibilityLabel(Some(&NSString::from_str(&format!(
            "Enable {}",
            input.label
        ))));
    }

    let badge = match input.verified {
        Some(true) => Some(("checkmark.circle.fill", NSColor::systemGreenColor())),
        Some(false) => Some(("xmark.circle", NSColor::systemRedColor())),
        None => None,
    };
    if let Some((symbol, color)) = badge {
        if let Some(image) =
            unsafe { NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(symbol), None) }
        {
            let view = unsafe { NSImageView::new(mtm()) };
            view.setFrame(NSRect::new(
                NSPoint::new(group.left + 26.0, row_y + 12.0),
                NSSize::new(14.0, 14.0),
            ));
            unsafe {
                view.setImage(Some(&image));
                view.setContentTintColor(Some(&color));
            }
            group.doc.addSubview(&view);
        }
    }

    group.doc.addSubview(&label(
        &input.label,
        13.0,
        NSFontWeightMedium,
        if input.enabled { &theme.text } else { &theme.secondary },
        NSRect::new(
            NSPoint::new(group.left + 48.0, row_y + 9.0),
            NSSize::new(150.0, 18.0),
        ),
    ));
    group.doc.addSubview(&mono(
        &format!("{:#06x}", input.value),
        11.0,
        &theme.secondary,
        NSRect::new(
            NSPoint::new(group.left + 206.0, row_y + 11.0),
            NSSize::new(64.0, 16.0),
        ),
    ));

    let test = button(
        "Test",
        &group.doc,
        NSRect::new(NSPoint::new(group.right - 84.0, row_y + 7.0), NSSize::new(54.0, 24.0)),
        false,
        sel!(testInput:),
    );
    tag(&test, index as isize);
    pin_right(&test);
    let protocol = button(
        input.protocol.label(),
        &group.doc,
        NSRect::new(
            NSPoint::new(group.right - 162.0, row_y + 7.0),
            NSSize::new(70.0, 24.0),
        ),
        true,
        sel!(cycleProtocol:),
    );
    tag(&protocol, index as isize);
    pin_right(&protocol);
    unsafe {
        protocol.setToolTip(Some(&NSString::from_str(
            "Click to switch between standard DDC and the LG alternate protocol.",
        )));
    }
    let remove = button(
        "×",
        &group.doc,
        NSRect::new(NSPoint::new(group.right - 22.0, row_y + 7.0), NSSize::new(22.0, 24.0)),
        false,
        sel!(removeInput:),
    );
    tag(&remove, index as isize);
    pin_right(&remove);
    group.y += 38.0;
}

unsafe fn build_virtual(host: &NSView, model: &Model, theme: &Theme) {
    let bounds = host.bounds();
    let scroll = unsafe {
        let scroll = NSScrollView::initWithFrame(mtm().alloc(), bounds);
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::NSViewWidthSizable
                | NSAutoresizingMaskOptions::NSViewHeightSizable,
        );
        host.addSubview(&scroll);
        scroll
    };
    let doc = FlippedView::new(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(bounds.size.width, bounds.size.height),
    ));
    unsafe {
        doc.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewWidthSizable);
        scroll.setDocumentView(Some(&doc));
    }
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.screen_names.clear();
        ui.screen_widths.clear();
        ui.screen_heights.clear();
        ui.screen_refreshes.clear();
        ui.screen_hidpi.clear();
        ui.screen_auto.clear();
        ui.agent_url = None;
    });

    let screens = crate::screens::list();
    let at_cap = screens.len() >= crate::screens::MAX_SCREENS;
    let mut pb = PageBuilder::new(&doc, theme, bounds.size.width);
    let new_button = pb.section_button(
        "Virtual Displays",
        "New Virtual Display",
        !at_cap,
        sel!(newScreen:),
    );
    if at_cap {
        unsafe {
            new_button.setToolTip(Some(&NSString::from_str(
                "Lumen supports up to four virtual displays.",
            )));
        }
    }

    if screens.is_empty() {
        let mut group = pb.group();
        group.doc.addSubview(&label(
            "No virtual displays",
            13.0,
            NSFontWeightMedium,
            &theme.text,
            NSRect::new(NSPoint::new(group.left, group.y + 8.0), NSSize::new(300.0, 18.0)),
        ));
        group.doc.addSubview(&paragraph(
            "Create one for an agent workspace, a preview or a capture source. Displays exist only in memory.",
            11.0,
            &theme.secondary,
            NSRect::new(
                NSPoint::new(group.left, group.y + 28.0),
                NSSize::new(group.right - group.left, 32.0),
            ),
        ));
        group.y += 62.0;
        pb.end(group);
    }

    for (index, screen) in screens.iter().enumerate() {
        let config = model.config.virtual_screen(&screen.name).cloned();
        let refresh = config.as_ref().map(|config| config.refresh).unwrap_or(60.0);
        let hidpi = config.as_ref().map(|config| config.hidpi).unwrap_or(false);
        let auto = config.as_ref().map(|config| config.auto_create).unwrap_or(true);

        let mut group = pb.group();
        // Name row.
        group.doc.addSubview(&label(
            "Name",
            11.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(NSPoint::new(group.left, group.y + 15.0), NSSize::new(40.0, 14.0)),
        ));
        let name = text_field(
            &screen.name,
            &group.doc,
            NSRect::new(
                NSPoint::new(group.left + 44.0, group.y + 9.0),
                NSSize::new(200.0, 26.0),
            ),
            None,
        );
        let summary = format!(
            "{}x{} at {},{}  ·  {}x  ·  {} Hz",
            screen.width, screen.height, screen.x, screen.y, screen.scale, refresh
        );
        let summary_field = label(
            &summary,
            11.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(
                NSPoint::new(group.left + 256.0, group.y + 14.0),
                NSSize::new(group.right - group.left - 256.0, 16.0),
            ),
        );
        stretch(&summary_field);
        group.doc.addSubview(&summary_field);
        group.y += 40.0;
        group.separator();

        // Geometry row.
        let field_y = group.y + 7.0;
        let mut x = group.left;
        group.doc.addSubview(&label(
            "W",
            11.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(NSPoint::new(x, field_y + 5.0), NSSize::new(14.0, 14.0)),
        ));
        x += 16.0;
        let width = number_field(
            &screen.width.to_string(),
            &group.doc,
            NSRect::new(NSPoint::new(x, field_y), NSSize::new(58.0, 24.0)),
            (index * 10 + 1) as isize,
        );
        x += 64.0;
        group.doc.addSubview(&label(
            "H",
            11.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(NSPoint::new(x, field_y + 5.0), NSSize::new(14.0, 14.0)),
        ));
        x += 16.0;
        let height = number_field(
            &screen.height.to_string(),
            &group.doc,
            NSRect::new(NSPoint::new(x, field_y), NSSize::new(58.0, 24.0)),
            (index * 10 + 2) as isize,
        );
        x += 64.0;
        group.doc.addSubview(&label(
            "Hz",
            11.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(NSPoint::new(x, field_y + 5.0), NSSize::new(22.0, 14.0)),
        ));
        x += 22.0;
        let hz = number_field(
            &format!("{refresh:.0}"),
            &group.doc,
            NSRect::new(NSPoint::new(x, field_y), NSSize::new(44.0, 24.0)),
            (index * 10 + 3) as isize,
        );
        x += 56.0;
        let hidpi_box = option_box(
            "2x",
            hidpi,
            &group.doc,
            NSRect::new(NSPoint::new(x, field_y + 3.0), NSSize::new(42.0, 20.0)),
        );
        x += 48.0;
        let auto_box = option_box(
            "Auto",
            auto,
            &group.doc,
            NSRect::new(NSPoint::new(x, field_y + 3.0), NSSize::new(56.0, 20.0)),
        );
        group.y += 40.0;
        group.separator();

        // Actions row.
        let remove = button(
            "Remove",
            &group.doc,
            NSRect::new(
                NSPoint::new(group.right - 76.0, group.y + 8.0),
                NSSize::new(76.0, 24.0),
            ),
            true,
            sel!(screenRemove:),
        );
        tag(&remove, index as isize);
        unsafe {
            destructive(&remove);
        }
        pin_right(&remove);
        let shown = crate::screens::preview_name().as_deref() == Some(screen.name.as_str());
        let preview = button(
            if shown { "Hide" } else { "Preview" },
            &group.doc,
            NSRect::new(
                NSPoint::new(group.right - 160.0, group.y + 8.0),
                NSSize::new(76.0, 24.0),
            ),
            true,
            sel!(screenPreview:),
        );
        tag(&preview, index as isize);
        pin_right(&preview);
        let apply = button(
            "Apply",
            &group.doc,
            NSRect::new(
                NSPoint::new(group.right - 244.0, group.y + 8.0),
                NSSize::new(76.0, 24.0),
            ),
            true,
            sel!(screenApply:),
        );
        tag(&apply, index as isize);
        pin_right(&apply);
        group.doc.addSubview(&label(
            "Apply recreates the display when its size or rate changes.",
            11.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(
                NSPoint::new(group.left, group.y + 12.0),
                NSSize::new(group.right - group.left - 260.0, 16.0),
            ),
        ));
        group.y += 40.0;
        pb.end(group);

        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            ui.screen_names.push(name);
            ui.screen_widths.push(width);
            ui.screen_heights.push(height);
            ui.screen_refreshes.push(hz);
            ui.screen_hidpi.push(hidpi_box);
            ui.screen_auto.push(auto_box);
        });
        pb.gap(2.0);
    }

    pb.section("Agent Workspace");
    let mut group = pb.group();
    group.toggle(
        "Start agents headless",
        "No window and no focus change. Capture the page with lumen agent screenshot.",
        model.config.agent_headless,
        sel!(toggleAgentHeadless:),
    );
    group.separator();
    let url = model.config.agent_url.clone().unwrap_or_default();
    let url_field = group.field(
        "Home page",
        "Opened when an agent starts without an explicit URL.",
        &url,
        "about:blank",
        220.0,
        701,
    );
    // Save button beside the field.
    let save = button(
        "Save",
        &group.doc,
        NSRect::new(NSPoint::new(group.right - 220.0, group.y - ROW + 14.0), NSSize::new(0.0, 26.0)),
        true,
        sel!(saveAgentUrl:),
    );
    unsafe {
        save.setFrame(NSRect::new(
            NSPoint::new(group.right - 60.0, group.y - ROW + 14.0),
            NSSize::new(60.0, 26.0),
        ));
        url_field.setFrame(NSRect::new(
            NSPoint::new(group.right - 220.0 - 68.0, group.y - ROW + 14.0),
            NSSize::new(220.0, 26.0),
        ));
    }
    pin_right(&save);
    UI.with(|ui| ui.borrow_mut().agent_url = Some(url_field));
    group.separator();
    group.toggle(
        "Reopen last session at launch",
        "Start the most recent agent again when Lumen launches.",
        model.config.agent_reopen,
        sel!(toggleAgentReopen:),
    );
    pb.end(group);
    pb.note("Headless sessions use the same CDP endpoint, so harnesses connect as usual.");

    pb.section("Captures");
    let limit = model.config.capture_limit;
    let (limit_index, limit_items) = match limit {
        None => (0, ["Keep all", "5", "20", "50"]),
        Some(5) => (1, ["Keep all", "5", "20", "50"]),
        Some(50) => (3, ["Keep all", "5", "20", "50"]),
        Some(_) => (2, ["Keep all", "5", "20", "50"]),
    };
    let mut group = pb.group();
    group.popup(
        "Keep captures",
        "Page and screen screenshots kept per display folder.",
        &limit_items,
        limit_index,
        sel!(captureLimit:),
    );
    group.separator();
    group.doc.addSubview(&label(
        "Captures folder",
        13.0,
        NSFontWeightMedium,
        &theme.text,
        NSRect::new(NSPoint::new(group.left, group.y + 17.0), NSSize::new(140.0, 18.0)),
    ));
    let open = button(
        "Open",
        &group.doc,
        NSRect::new(
            NSPoint::new(group.right - 76.0, group.y + 14.0),
            NSSize::new(76.0, 24.0),
        ),
        true,
        sel!(openCaptures:),
    );
    pin_right(&open);
    let delete = button(
        "Delete",
        &group.doc,
        NSRect::new(
            NSPoint::new(group.right - 160.0, group.y + 14.0),
            NSSize::new(76.0, 24.0),
        ),
        true,
        sel!(deleteCaptures:),
    );
    unsafe {
        destructive(&delete);
    }
    pin_right(&delete);
    group.y += ROW;
    pb.end(group);
    pb.note(&crate::screens::captures_dir().display().to_string());

    let height = (pb.y + MARGIN).max(bounds.size.height);
    unsafe {
        doc.setFrameSize(NSSize::new(bounds.size.width, height));
    }
    UI.with(|ui| ui.borrow_mut().scroll = Some(scroll));
}

unsafe fn build_about(host: &NSView, model: &Model, theme: &Theme) {
    let bounds = host.bounds();
    let scroll = unsafe {
        let scroll = NSScrollView::initWithFrame(mtm().alloc(), bounds);
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::NSViewWidthSizable
                | NSAutoresizingMaskOptions::NSViewHeightSizable,
        );
        host.addSubview(&scroll);
        scroll
    };
    let doc = FlippedView::new(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(bounds.size.width, bounds.size.height),
    ));
    unsafe {
        doc.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewWidthSizable);
        scroll.setDocumentView(Some(&doc));
    }

    if let Some(icon) = unsafe { NSApplication::sharedApplication(mtm()).applicationIconImage() } {
        let view = unsafe { NSImageView::new(mtm()) };
        view.setFrame(NSRect::new(NSPoint::new(MARGIN, MARGIN), NSSize::new(72.0, 72.0)));
        unsafe {
            view.setImage(Some(&icon));
        }
        doc.addSubview(&view);
    }
    doc.addSubview(&label(
        "Lumen",
        22.0,
        NSFontWeightSemibold,
        &theme.text,
        NSRect::new(NSPoint::new(MARGIN + 88.0, MARGIN + 4.0), NSSize::new(300.0, 26.0)),
    ));
    doc.addSubview(&label(
        &format!("Version {}", crate::version_string()),
        12.0,
        NSFontWeightRegular,
        &theme.secondary,
        NSRect::new(NSPoint::new(MARGIN + 88.0, MARGIN + 34.0), NSSize::new(300.0, 16.0)),
    ));
    doc.addSubview(&paragraph(
        "Menu bar control for DDC displays, virtual displays for agent workspaces, and a local control socket.",
        12.0,
        &theme.secondary,
        NSRect::new(
            NSPoint::new(MARGIN + 88.0, MARGIN + 52.0),
            NSSize::new(bounds.size.width - MARGIN * 2.0 - 88.0, 34.0),
        ),
    ));

    let mut pb = PageBuilder::new(&doc, theme, bounds.size.width);
    pb.gap(MARGIN + 88.0);

    pb.section("Places");
    let mut group = pb.group();
    group.doc.addSubview(&label(
        "Configuration",
        13.0,
        NSFontWeightMedium,
        &theme.text,
        NSRect::new(NSPoint::new(group.left, group.y + 10.0), NSSize::new(130.0, 18.0)),
    ));
    group.doc.addSubview(&label(
        &crate::config::Config::path().display().to_string(),
        11.0,
        NSFontWeightRegular,
        &theme.secondary,
        NSRect::new(
            NSPoint::new(group.left, group.y + 29.0),
            NSSize::new(group.right - group.left - 110.0, 16.0),
        ),
    ));
    let open_config = button(
        "Open",
        &group.doc,
        NSRect::new(
            NSPoint::new(group.right - 76.0, group.y + 17.0),
            NSSize::new(76.0, 24.0),
        ),
        true,
        sel!(openConfig:),
    );
    pin_right(&open_config);
    group.y += ROW;
    group.separator();
    group.doc.addSubview(&label(
        "Captures",
        13.0,
        NSFontWeightMedium,
        &theme.text,
        NSRect::new(NSPoint::new(group.left, group.y + 10.0), NSSize::new(130.0, 18.0)),
    ));
    group.doc.addSubview(&label(
        &crate::screens::captures_dir().display().to_string(),
        11.0,
        NSFontWeightRegular,
        &theme.secondary,
        NSRect::new(
            NSPoint::new(group.left, group.y + 29.0),
            NSSize::new(group.right - group.left - 110.0, 16.0),
        ),
    ));
    let open_captures = button(
        "Open",
        &group.doc,
        NSRect::new(
            NSPoint::new(group.right - 76.0, group.y + 17.0),
            NSSize::new(76.0, 24.0),
        ),
        true,
        sel!(openCaptures:),
    );
    pin_right(&open_captures);
    group.y += ROW;
    pb.end(group);

    pb.section("Diagnostics");
    let sessions = crate::agent::sessions().len();
    let diagnostics = [
        ("Displays", format!("{} DDC", model.displays.len())),
        ("Virtual", format!("{} active", crate::screens::list().len())),
        ("Agents", format!("{sessions} session(s)")),
        ("Login item", format!("{:?}", crate::login::status())),
    ];
    let mut group = pb.group();
    for (index, (name, value)) in diagnostics.iter().enumerate() {
        group.doc.addSubview(&label(
            name,
            13.0,
            NSFontWeightMedium,
            &theme.text,
            NSRect::new(
                NSPoint::new(group.left, group.y + 8.0),
                NSSize::new(120.0, 18.0),
            ),
        ));
        let field = label(
            value,
            12.0,
            NSFontWeightRegular,
            &theme.secondary,
            NSRect::new(
                NSPoint::new(group.left + 130.0, group.y + 9.0),
                NSSize::new(group.right - group.left - 130.0, 16.0),
            ),
        );
        pin_right(&field);
        group.doc.addSubview(&field);
        if index + 1 < diagnostics.len() {
            group.separator();
        }
        group.y += 36.0;
    }
    group.separator();
    let copy = button(
        "Copy diagnostics",
        &group.doc,
        NSRect::new(
            NSPoint::new(group.right - 140.0, group.y + 9.0),
            NSSize::new(140.0, 26.0),
        ),
        true,
        sel!(copyDiagnostics:),
    );
    pin_right(&copy);
    group.y += 44.0;
    pb.end(group);

    let height = (pb.y + MARGIN).max(bounds.size.height);
    unsafe {
        doc.setFrameSize(NSSize::new(bounds.size.width, height));
    }
    UI.with(|ui| ui.borrow_mut().scroll = Some(scroll));
}

fn capture_virtual_drafts() -> Option<VirtualDrafts> {
    UI.with(|ui| {
        let ui = ui.borrow();
        if ui.screen_names.is_empty() && ui.agent_url.is_none() {
            return None;
        }
        let read = |fields: &[Retained<NSTextField>]| {
            fields
                .iter()
                .map(|field| unsafe { field.stringValue().to_string() })
                .collect::<Vec<_>>()
        };
        Some(VirtualDrafts {
            names: read(&ui.screen_names),
            widths: read(&ui.screen_widths),
            heights: read(&ui.screen_heights),
            refreshes: read(&ui.screen_refreshes),
            url: ui
                .agent_url
                .as_ref()
                .map(|field| unsafe { field.stringValue().to_string() }),
        })
    })
}

fn restore_virtual_drafts(drafts: &VirtualDrafts) {
    UI.with(|ui| {
        let ui = ui.borrow();
        let set = |fields: &[Retained<NSTextField>], values: &[String]| {
            for (field, value) in fields.iter().zip(values.iter()) {
                unsafe { field.setStringValue(&NSString::from_str(value)) };
            }
        };
        set(&ui.screen_names, &drafts.names);
        set(&ui.screen_widths, &drafts.widths);
        set(&ui.screen_heights, &drafts.heights);
        set(&ui.screen_refreshes, &drafts.refreshes);
        if let (Some(field), Some(value)) = (&ui.agent_url, &drafts.url) {
            unsafe { field.setStringValue(&NSString::from_str(value)) };
        }
    });
}

fn build_page(host: &NSView, model: &Model, page: Page) {
    let theme = Theme::new();
    let bounds = host.bounds();
    let drafts = if page == Page::Virtual
        && UI.with(|ui| ui.borrow().built_page == Some(Page::Virtual))
    {
        capture_virtual_drafts()
    } else {
        None
    };
    unsafe {
        for subview in host.subviews().iter() {
            subview.removeFromSuperview();
        }
    }
    UI.with(|ui| {
        ui.borrow_mut().scroll = None;
    });
    unsafe { match page {
        Page::General => {
            let scroll = {
                let scroll = NSScrollView::initWithFrame(mtm().alloc(), bounds);
                scroll.setHasVerticalScroller(true);
                scroll.setDrawsBackground(false);
                scroll.setAutoresizingMask(
                    NSAutoresizingMaskOptions::NSViewWidthSizable
                        | NSAutoresizingMaskOptions::NSViewHeightSizable,
                );
                host.addSubview(&scroll);
                scroll
            };
            let doc = FlippedView::new(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(bounds.size.width, bounds.size.height),
            ));
            doc.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewWidthSizable);
            scroll.setDocumentView(Some(&doc));
            build_general(&doc, model, &theme);
            UI.with(|ui| ui.borrow_mut().scroll = Some(scroll));
        }
        Page::Displays => build_displays(host, model, &theme),
        Page::Virtual => {
            build_virtual(host, model, &theme);
            if let Some(drafts) = &drafts {
                restore_virtual_drafts(drafts);
            }
        }
        Page::About => build_about(host, model, &theme),
    } }
    UI.with(|ui| ui.borrow_mut().built_page = Some(page));
}

fn build_chrome(window: &NSWindow) {
    unsafe {
        let Some(content) = window.contentView() else {
            return;
        };
        let bounds = content.bounds();
        for subview in content.subviews().iter() {
            subview.removeFromSuperview();
        }

        let effect = NSVisualEffectView::initWithFrame(
            mtm().alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SIDEBAR, bounds.size.height)),
        );
        effect.setMaterial(NSVisualEffectMaterial::Sidebar);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewHeightSizable);
        content.addSubview(&effect);

        let page = UI.with(|ui| ui.borrow().page);
        for (index, title) in PAGES.iter().enumerate() {
            let row = NSRect::new(
                NSPoint::new(8.0, bounds.size.height - 40.0 - index as f64 * 32.0),
                NSSize::new(SIDEBAR - 16.0, 30.0),
            );
            if Page::from_row(index as isize) == page {
                let background = selection_background(&content, row);
                pin_top(&background);
            }
            let b = button(
                title,
                &content,
                NSRect::new(
                    NSPoint::new(row.origin.x + 10.0, row.origin.y + 5.0),
                    NSSize::new(row.size.width - 12.0, 20.0),
                ),
                false,
                sel!(pageChanged:),
            );
            tag(&b, index as isize);
            b.setAlignment(NSTextAlignment::Left);
            b.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, NSFontWeightRegular)));
            pin_top(&b);
        }

        let line = NSBox::initWithFrame(
            mtm().alloc(),
            NSRect::new(NSPoint::new(SIDEBAR, 0.0), NSSize::new(1.0, bounds.size.height)),
        );
        line.setBoxType(NSBoxType::NSBoxCustom);
        line.setTitlePosition(NSTitlePosition::NSNoTitle);
        line.setBorderWidth(0.0);
        line.setCornerRadius(0.0);
        line.setFillColor(&NSColor::separatorColor());
        line.setBorderColor(&NSColor::separatorColor());
        line.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewHeightSizable);
        content.addSubview(&line);

        let host = NSView::initWithFrame(
            mtm().alloc(),
            NSRect::new(
                NSPoint::new(SIDEBAR + 1.0, 0.0),
                NSSize::new(bounds.size.width - SIDEBAR - 1.0, bounds.size.height),
            ),
        );
        host.setAutoresizingMask(
            NSAutoresizingMaskOptions::NSViewWidthSizable
                | NSAutoresizingMaskOptions::NSViewHeightSizable,
        );
        content.addSubview(&host);
        UI.with(|ui| ui.borrow_mut().host = Some(host));
    }
}

pub fn target() -> Option<Retained<Actions>> {
    ACTIONS.with(|slot| slot.borrow().clone())
}

pub fn init() -> Retained<Actions> {
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
    actions
}

pub fn open() {
    let start = std::time::Instant::now();
    if ACTIONS.with(|slot| slot.borrow().is_none()) {
        ACTIONS.with(|slot| *slot.borrow_mut() = Some(make_actions()));
    }
    let existing = UI.with(|ui| ui.borrow().window.clone());
    let window = match existing {
        Some(window) => window,
        None => {
            let window = create_window();
            UI.with(|ui| ui.borrow_mut().window = Some(window.clone()));
            window
        }
    };

    rebuild_page();
    #[allow(deprecated)]
    {
        NSApplication::sharedApplication(mtm()).activateIgnoringOtherApps(true);
    }
    window.makeKeyAndOrderFront(None);
    let _ = window.makeFirstResponder(None);
    if std::env::var_os("LUMEN_TRACE").is_some() {
        eprintln!("settings open: {} ms", start.elapsed().as_millis());
    }
}

fn rebuild_page() {
    let (host, page) = UI.with(|ui| {
        let ui = ui.borrow();
        (ui.host.clone(), ui.page)
    });
    let Some(host) = host else {
        return;
    };
    if host.window().is_none() {
        return;
    }
    let model = crate::with_model(|model| model.snapshot());
    build_page(&host, &model, page);
}

pub fn rebuild_if_open() {
    let window = UI.with(|ui| ui.borrow().window.clone());
    let Some(window) = window else {
        return;
    };
    if !window.isVisible() {
        return;
    }
    rebuild_page();
}

fn create_window() -> Retained<NSWindow> {
    unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm().alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        );
        window.setTitle(&NSString::from_str("Lumen Settings"));
        window.setReleasedWhenClosed(false);
        window.setContentMinSize(NSSize::new(MIN_WIDTH, MIN_HEIGHT));
        let name = NSString::from_str("LumenSettings2");
        let restored = window.setFrameUsingName(&name);
        window.setFrameAutosaveName(&name);
        if !restored {
            window.center();
        }
        ACTIONS.with(|slot| {
            if let Some(actions) = slot.borrow().as_ref() {
                window.setDelegate(Some(ProtocolObject::from_ref(&**actions)));
            }
        });
        build_chrome(&window);
        window
    }
}

/// Reads a control's `tag`.
fn sender_tag(sender: *mut AnyObject) -> isize {
    unsafe { msg_send![sender, tag] }
}

fn make_actions() -> Retained<Actions> {
    let object = mtm().alloc::<Actions>().set_ivars(());
    unsafe { msg_send_id![super(object), init] }
}


/// Runs an alert over the settings window, so it appears on the same display. The alert centres
/// itself when it is shown, so a one-shot timer moves it after layout.
fn run_alert(alert: &NSAlert) -> isize {
    let Some(settings) = UI.with(|ui| ui.borrow().window.clone()) else {
        return unsafe { alert.runModal() };
    };
    unsafe {
        alert.layout();
        let parent = settings.frame();
        let window = alert.window();
        let size = window.frame().size;
        let target = NSPoint::new(
            parent.origin.x + (parent.size.width - size.width) / 2.0,
            parent.origin.y + (parent.size.height - size.height) / 2.0,
        );
        let place = block2::RcBlock::new(move |_timer: std::ptr::NonNull<objc2_foundation::NSTimer>| {
            window.setFrameOrigin(target);
        });
        let timer = objc2_foundation::NSTimer::timerWithTimeInterval_repeats_block(
            0.05,
            false,
            &place,
        );
        objc2_foundation::NSRunLoop::currentRunLoop().addTimer_forMode(
            &timer,
            objc2_foundation::NSRunLoopCommonModes,
        );
        let response = alert.runModal();
        timer.invalidate();
        response
    }
}

fn confirm(message: &str, detail: &str, action: &str) -> bool {
    unsafe {
        let alert = NSAlert::new(mtm());
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(detail));
        alert.addButtonWithTitle(&NSString::from_str(action));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        run_alert(&alert) == NSAlertFirstButtonReturn
    }
}

fn message(text: &str) {
    crate::with_model(|model| model.message = Some(text.to_string()));
}

declare_class!(
    pub struct Actions;

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
        #[method(menuAction:)]
        fn menu_action(&self, sender: *mut AnyObject) {
            let Some(item) = (unsafe { sender.as_ref() }) else {
                return;
            };
            if let Some(action) = crate::status::item_action(item) {
                crate::run_action(action);
            }
        }

        #[method(previewTick:)]
        fn preview_tick(&self, _timer: *mut AnyObject) {
            crate::screens::preview_tick();
        }

        #[method(menuControlChanged:)]
        fn menu_control_changed(&self, sender: *mut AnyObject) {
            let tag = sender_tag(sender);
            let display_id = (tag >> 8) as u32;
            let code = (tag & 0xff) as u8;
            let raw: f64 = unsafe { msg_send![sender, doubleValue] };
            let max = crate::with_model(|model| {
                model.display(display_id).and_then(|display| match code {
                    0x10 => display.brightness,
                    0x12 => display.contrast,
                    _ => display.volume,
                })
            })
            .map(|(_, max)| max)
            .unwrap_or(100);
            let value = crate::snap_control(raw, max, crate::fine_control());
            if (raw - f64::from(value)).abs() >= 0.5 {
                let _: () = unsafe { msg_send![sender, setDoubleValue: f64::from(value)] };
            }
            crate::with_model(|model| model.apply_control(display_id, code, value));
            crate::status::set_readout(display_id, code, value);
            crate::queue_control(display_id, code, value);
        }

        #[method(screenApply:)]
        fn screen_apply(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender) as usize;
            let Some(screen) = crate::screens::list().get(index).cloned() else {
                return;
            };
            let Some(mut config) = crate::with_model(|model| model.config.virtual_screen(&screen.name).cloned()) else {
                return;
            };
            let (name, width, height, refresh, hidpi, auto) = UI.with(|ui| {
                let ui = ui.borrow();
                let text = |field: Option<&Retained<NSTextField>>, fallback: &str| {
                    field
                        .map(|field| unsafe { field.stringValue().to_string() })
                        .unwrap_or_else(|| fallback.to_string())
                };
                let number = |field: Option<&Retained<NSTextField>>, fallback: f64| {
                    field
                        .map(|field| unsafe { field.stringValue().to_string() })
                        .and_then(|text| text.trim().parse::<f64>().ok())
                        .unwrap_or(fallback)
                };
                let state = |box_: Option<&Retained<NSButton>>, fallback: bool| {
                    box_
                        .map(|box_| unsafe { box_.state() } == NSControlStateValueOn)
                        .unwrap_or(fallback)
                };
                (
                    text(ui.screen_names.get(index), &screen.name),
                    number(ui.screen_widths.get(index), screen.width as f64),
                    number(ui.screen_heights.get(index), screen.height as f64),
                    number(ui.screen_refreshes.get(index), config.refresh),
                    state(ui.screen_hidpi.get(index), config.hidpi),
                    state(ui.screen_auto.get(index), config.auto_create),
                )
            });
            let name = name.trim().to_string();
            if let Err(error) = crate::control::valid_name(&name) {
                message(&error);
                crate::after_change();
                return;
            }
            config.name = name.clone();
            config.width = (width.round() as i64).clamp(640, 7680) as u32;
            config.height = (height.round() as i64).clamp(480, 4320) as u32;
            config.refresh = refresh.clamp(1.0, 240.0);
            config.hidpi = hidpi;
            config.auto_create = auto;

            if name != screen.name {
                let confirmed = confirm(
                    &format!("Rename {} to {}?", screen.name, name),
                    "The display is recreated. Any agent browser on it closes, and its position on the desktop may change.",
                    "Rename",
                );
                if !confirmed {
                    return;
                }
                match crate::rename_screen(&screen.name, &config) {
                    Ok(()) => {}
                    Err(error) => message(&error),
                }
            } else {
                match crate::screens::ensure(&config) {
                    Ok(_) => {
                        crate::with_model(|model| {
                            model.config.upsert_virtual_screen(config);
                            model.message = Some("Virtual display updated".to_string());
                        });
                    }
                    Err(error) => message(&error),
                }
            }
            crate::after_change();
        }

        #[method(screenPreview:)]
        fn screen_preview(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender) as usize;
            let Some(screen) = crate::screens::list().get(index).cloned() else {
                return;
            };
            if crate::screens::preview_name().as_deref() == Some(screen.name.as_str()) {
                crate::screens::stop_preview();
            } else if let Err(error) = crate::screens::start_preview(&screen.name, self) {
                message(&error);
            }
            crate::after_change();
        }

        #[method(screenRemove:)]
        fn screen_remove(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender) as usize;
            let Some(screen) = crate::screens::list().get(index).cloned() else {
                return;
            };
            let confirmed = confirm(
                &format!("Remove {}?", screen.name),
                "This closes any agent browser on the display and deletes its definition. Captures stay on disk.",
                "Remove",
            );
            if confirmed {
                crate::remove_screen(&screen.name);
            }
        }

        #[method(newScreen:)]
        fn new_screen(&self, _sender: *mut AnyObject) {
            if crate::screens::list().len() >= crate::screens::MAX_SCREENS {
                message("Lumen supports up to four virtual displays");
                crate::after_change();
                return;
            }
            let existing: Vec<String> = crate::screens::list().iter().map(|screen| screen.name.clone()).collect();
            let mut counter = 2;
            let default_name = loop {
                let candidate = format!("Screen {counter}");
                if !existing.contains(&candidate) {
                    break candidate;
                }
                counter += 1;
            };
            let defaults = crate::with_model(|model| model.config.virtual_defaults.clone());
            let presets = [(1920u32, 1080u32), (2560, 1440), (1280, 720), (3840, 2160), (1600, 900)];
            let selected = presets
                .iter()
                .position(|(w, h)| *w == defaults.width && *h == defaults.height)
                .unwrap_or(0);

            let (accessory, name_field, size_popup) = unsafe {
                let accessory = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 58.0)));
                let name_label = label(
                    "Name",
                    11.0,
                    NSFontWeightRegular,
                    &NSColor::secondaryLabelColor(),
                    NSRect::new(NSPoint::new(0.0, 36.0), NSSize::new(40.0, 14.0)),
                );
                accessory.addSubview(&name_label);
                let name_field = text_field(
                    &default_name,
                    &accessory,
                    NSRect::new(NSPoint::new(44.0, 30.0), NSSize::new(200.0, 26.0)),
                    None,
                );
                let size_items: Vec<String> = presets
                    .iter()
                    .map(|(w, h)| format!("{w} x {h}"))
                    .collect();
                let size_refs: Vec<&str> = size_items.iter().map(String::as_str).collect();
                let size_popup = popup(
                    &accessory,
                    NSRect::new(NSPoint::new(262.0, 30.0), NSSize::new(120.0, 26.0)),
                    &size_refs,
                    selected,
                    sel!(noop:),
                );
                (accessory, name_field, size_popup)
            };

            unsafe {
                let alert = NSAlert::new(mtm());
                alert.setMessageText(&NSString::from_str("New Virtual Display"));
                alert.setInformativeText(&NSString::from_str(
                    "Names allow letters, digits, spaces and - _ . '",
                ));
                alert.addButtonWithTitle(&NSString::from_str("Create"));
                alert.addButtonWithTitle(&NSString::from_str("Cancel"));
                alert.setAccessoryView(Some(&accessory));
                if run_alert(&alert) != NSAlertFirstButtonReturn {
                    return;
                }
            }
            let name = unsafe { name_field.stringValue().to_string() }.trim().to_string();
            if let Err(error) = crate::control::valid_name(&name) {
                message(&error);
                crate::after_change();
                return;
            }
            if crate::with_model(|model| model.config.virtual_screen(&name).is_some()) {
                message(&format!("a virtual screen named {name} already exists"));
                crate::after_change();
                return;
            }
            let (width, height) = presets[unsafe { size_popup.indexOfSelectedItem() }.max(0) as usize % presets.len()];
            let mut config = crate::config::VirtualScreenConfig::default();
            config.name = name;
            config.width = width;
            config.height = height;
            config.hidpi = defaults.hidpi;
            config.refresh = if defaults.refresh > 0.0 { defaults.refresh } else { 60.0 };
            if let Err(error) = crate::create_screen_with(config) {
                message(&error);
            }
            crate::after_change();
        }

        #[method(noop:)]
        fn noop(&self, _sender: *mut AnyObject) {}

        #[method(testInput:)]
        fn test_input(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender).max(0) as usize;
            let selected = UI.with(|ui| ui.borrow().selected);
            let Some((display_id, key, name, input)) = crate::with_model(|model| {
                let display = model.displays.get(selected)?;
                let config = model.config.display(&display.key)?;
                let input = config.inputs.get(index)?.clone();
                Some((display.id, display.key.clone(), display.name.clone(), input))
            }) else {
                return;
            };

            let confirmed = confirm(
                &format!("Test {} on {}?", input.label, name),
                "Lumen sends a DDC input-select command. If the monitor accepts it, it switches away and this display disappears from the Mac. Some monitors cannot switch back over DDC, so you may need the monitor's own controls to return.",
                "Test",
            );
            if !confirmed {
                return;
            }

            let code = lumen::ddc::InputCode {
                label: format!("{} ({})", input.label, input.protocol.label()),
                value: input.value,
                protocol: input.protocol.to_ddc(),
            };
            crate::switch_input(display_id, code, Some(key));
        }

        #[method(controlChanged:)]
        fn control_changed(&self, sender: *mut AnyObject) {
            let tag = sender_tag(sender);
            let code = match tag {
                2 => 0x12u8,
                3 => 0x62,
                _ => 0x10,
            };
            let selected = UI.with(|ui| ui.borrow().selected);
            let Some(display_id) =
                crate::with_model(|model| model.displays.get(selected).map(|display| display.id))
            else {
                return;
            };
            let max = crate::with_model(|model| {
                model.displays.get(selected).and_then(|display| match code {
                    0x10 => display.brightness,
                    0x12 => display.contrast,
                    _ => display.volume,
                })
            })
            .map(|(_, max)| max)
            .unwrap_or(100);

            let raw: f64 = unsafe { msg_send![sender, doubleValue] };
            let value = crate::snap_control(raw, max, crate::fine_control());
            if (raw - f64::from(value)).abs() >= 0.5 {
                let _: () = unsafe { msg_send![sender, setDoubleValue: f64::from(value)] };
            }

            UI.with(|ui| {
                let ui = ui.borrow();
                let target = match tag {
                    2 => ui.contrast_value.clone(),
                    3 => ui.volume_value.clone(),
                    _ => ui.brightness_value.clone(),
                };
                if let Some(text) = target {
                    unsafe {
                        text.setStringValue(&NSString::from_str(&format!(
                            "{}%",
                            percent_of(value, max)
                        )))
                    };
                }
            });

            crate::with_model(|model| model.apply_control(display_id, code, value));
            crate::queue_control(display_id, code, value);
        }

        #[method(screensChanged:)]
        fn screens_changed(&self, _notification: *mut AnyObject) {
            crate::schedule_rescan();
        }

        #[method(didWake:)]
        fn did_wake(&self, _notification: *mut AnyObject) {
            crate::schedule_rescan();
        }

        #[method(refreshDisplays:)]
        fn refresh_displays(&self, _sender: *mut AnyObject) {
            crate::run_action(crate::menu::Action::Refresh);
        }

        #[method(toggleLoginItem:)]
        fn toggle_login_item(&self, _sender: *mut AnyObject) {
            let before = crate::login::status();
            let result = match before {
                crate::login::Status::Enabled => crate::login::unregister(),
                crate::login::Status::RequiresApproval => {
                    crate::login::open_login_items();
                    Ok(())
                }
                _ => crate::login::register(),
            };
            if std::env::var_os("LUMEN_TRACE").is_some() {
                eprintln!("login item: before={before:?} result={result:?} after={:?}", crate::login::status());
            }
            UI.with(|ui| {
                ui.borrow_mut().login_note = result.err();
            });
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
            let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            let minutes = match index {
                1 => Some(1),
                2 => Some(5),
                3 => Some(15),
                4 => Some(30),
                _ => None,
            };
            crate::with_model(|model| {
                model.config.auto_reclaim_minutes = minutes;
                model.config.save();
            });
            crate::after_change();
        }

        #[method(pageChanged:)]
        fn page_changed(&self, sender: *mut AnyObject) {
            let row = sender_tag(sender);
            UI.with(|ui| ui.borrow_mut().page = Page::from_row(row));
            let window = UI.with(|ui| ui.borrow().window.clone());
            if let Some(window) = window {
                build_chrome(&window);
            }
            rebuild_page();
        }

        #[method(selectDisplay:)]
        fn select_display(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender).max(0) as usize;
            UI.with(|ui| ui.borrow_mut().selected = index);
            rebuild_page();
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
                message("enter the input code in hexadecimal, for example 0x0f");
                crate::after_change();
                return;
            };
            let label = if label.trim().is_empty() { format!("Input {value:#06x}") } else { label.trim().to_string() };
            let key = crate::with_model(|model| key_of_selected(model));
            let Some(key) = key else { return };
            crate::with_model(|model| {
                if let Some(config) = model.config.display_mut(&key) {
                    config.inputs.push(InputSetting { label, value, protocol, enabled: true, verified: None });
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

        #[method(toggleAgentHeadless:)]
        fn toggle_agent_headless(&self, _sender: *mut AnyObject) {
            crate::with_model(|model| {
                model.config.agent_headless = !model.config.agent_headless;
                model.config.save();
            });
            crate::after_change();
        }

        #[method(toggleAgentReopen:)]
        fn toggle_agent_reopen(&self, _sender: *mut AnyObject) {
            crate::with_model(|model| {
                model.config.agent_reopen = !model.config.agent_reopen;
                model.config.save();
            });
            crate::after_change();
        }

        #[method(saveAgentUrl:)]
        fn save_agent_url(&self, _sender: *mut AnyObject) {
            let value = UI.with(|ui| {
                ui.borrow()
                    .agent_url
                    .as_ref()
                    .map(|field| unsafe { field.stringValue().to_string() })
                    .unwrap_or_default()
            });
            let value = value.trim().to_string();
            crate::with_model(|model| {
                model.config.agent_url = if value.is_empty() || value == "about:blank" {
                    None
                } else {
                    Some(value)
                };
                model.config.save();
            });
            crate::after_change();
        }

        #[method(captureLimit:)]
        fn capture_limit(&self, sender: *mut AnyObject) {
            let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            let limit = match index {
                0 => None,
                1 => Some(5),
                3 => Some(50),
                _ => Some(20),
            };
            crate::with_model(|model| {
                model.config.capture_limit = limit;
                model.config.save();
            });
            crate::after_change();
        }

        #[method(openCaptures:)]
        fn open_captures(&self, _sender: *mut AnyObject) {
            let path = crate::screens::captures_dir();
            let _ = std::fs::create_dir_all(&path);
            let _ = std::process::Command::new("/usr/bin/open").arg(path).spawn();
        }

        #[method(openConfig:)]
        fn open_config(&self, _sender: *mut AnyObject) {
            let path = crate::config::Config::path();
            if let Some(parent) = path.parent() {
                let _ = std::process::Command::new("/usr/bin/open").arg(parent).spawn();
            }
        }

        #[method(deleteCaptures:)]
        fn delete_captures(&self, _sender: *mut AnyObject) {
            let confirmed = confirm(
                "Delete captures?",
                "Removes page and screen screenshots from every folder. Browser profiles are not touched.",
                "Delete",
            );
            if !confirmed {
                return;
            }
            let removed = crate::screens::delete_captures();
            message(&format!("Deleted {removed} capture(s)"));
            crate::after_change();
        }

        #[method(copyDiagnostics:)]
        fn copy_diagnostics(&self, _sender: *mut AnyObject) {
            let os = std::process::Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let text = crate::with_model(|model| {
                format!(
                    "Lumen {}\nmacOS {}\nDisplays: {}\nVirtual displays: {}\nAgent sessions: {}\nLogin item: {:?}\nConfig: {}\nCaptures: {}",
                    crate::version_string(),
                    os,
                    model.displays.len(),
                    crate::screens::list().len(),
                    crate::agent::sessions().len(),
                    crate::login::status(),
                    crate::config::Config::path().display(),
                    crate::screens::captures_dir().display()
                )
            });
            unsafe {
                let pasteboard = NSPasteboard::generalPasteboard();
                pasteboard.clearContents();
                pasteboard.setString_forType(&NSString::from_str(&text), NSPasteboardTypeString);
            }
            message("Diagnostics copied");
            crate::after_change();
        }
    }

    unsafe impl NSMenuDelegate for Actions {
        #[method(menuNeedsUpdate:)]
        fn menu_needs_update(&self, menu: &NSMenu) {
            crate::status::fill(menu, self);
        }
    }

    unsafe impl NSWindowDelegate for Actions {
        #[method(windowWillClose:)]
        fn window_will_close(&self, notification: &NSNotification) {
            // The preview window owns its own timer; stop it without closing the window again.
            if let Some(object) = unsafe { notification.object() } {
                let closing = &*object as *const AnyObject as *const ();
                if crate::screens::preview_window_ptr().map(|ptr| ptr as *const ()) == Some(closing)
                {
                    crate::screens::preview_closed();
                    return;
                }
            }
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
