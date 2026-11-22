#![allow(unsafe_op_in_unsafe_fn)]

//! The AppKit settings window.
//!
//! A source list on the left and one scrolling page on the right. Groups are rounded boxes with
//! rows, following the shape of System Settings. Layout inside a page is still explicit: a page
//! builder walks down a flipped document view and records the height it needs. Controls carry `tag`
//! values that the `Actions` target uses to find the row they belong to.

use crate::config::{InputSetting, Protocol, SettingsDisplay};
use crate::model::Model;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn,
    NSApplication,
    NSApplicationDidChangeScreenParametersNotification, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSBox, NSBoxType, NSButton, NSColor, NSControl, NSControlStateValueOff, NSControlStateValueOn,
    NSFont, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold, NSImage, NSImageView,
    NSLayoutAttribute, NSLayoutConstraintOrientation, NSStackView, NSStackViewDistribution,
    NSUserInterfaceLayoutOrientation,
    NSAccessibility, NSControlTextEditingDelegate, NSForegroundColorAttributeName, NSLineBreakMode,
    NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle,
    NSMenu, NSMenuDelegate,
    NSPasteboard, NSPasteboardTypeString,
    NSPopUpButton, NSScreen, NSScrollView,
    NSSlider, NSSwitch, NSTextAlignment, NSTextField,
    NSTickMarkPosition,
    NSTitlePosition, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode, NSToolbarItem,
    NSToolbarItemIdentifier, NSView,
    NSWindow, NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask, NSWindowToolbarStyle,
    NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDistributedNotificationCenter, NSMutableAttributedString,
    NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRect,
    NSSize, NSString,
};
use std::cell::RefCell;

const MIN_WIDTH: f64 = 880.0;
const MIN_HEIGHT: f64 = 500.0;
const MARGIN: f64 = 28.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Displays,
    Virtual,
    About,
}

impl Page {
    fn slug(self) -> &'static str {
        match self {
            Page::General => "general",
            Page::Displays => "displays",
            Page::Virtual => "virtual",
            Page::About => "about",
        }
    }

    fn from_slug(value: &str) -> Option<Self> {
        match value {
            "general" => Some(Page::General),
            "displays" => Some(Page::Displays),
            "virtual" | "virtual-displays" => Some(Page::Virtual),
            "about" => Some(Page::About),
            _ => None,
        }
    }

    /// The four sections in toolbar order.
    const ALL: [Page; 4] = [Page::General, Page::Displays, Page::Virtual, Page::About];

    /// SF Symbol drawn in the toolbar tab for this section.
    fn symbol(self) -> &'static str {
        match self {
            Page::General => "gearshape",
            Page::Displays => "display",
            Page::Virtual => "rectangle.on.rectangle",
            Page::About => "info.circle",
        }
    }

    /// Short label under the toolbar icon. "Virtual Displays" is too wide for a tab, and the icon
    /// already carries the meaning.
    fn tab_label(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Displays => "Displays",
            Page::Virtual => "Virtual",
            Page::About => "About",
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
    /// The native titlebar toolbar carrying the section tabs (icon above label), one selectable
    /// item per page. The selection highlight is drawn by AppKit.
    toolbar: Option<Retained<NSToolbar>>,
    display_list: Option<Retained<NSTableView>>,
    suppress_selection: bool,
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
    agent_picker: Option<Retained<NSPopUpButton>>,
    agent_session_names: Vec<String>,
    agent_note: Option<String>,
    /// Live thumbnails on the Virtual Displays page, keyed by screen name.
    screen_thumbs: Vec<(String, Retained<NSView>)>,
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
            toolbar: None,
            display_list: None,
            suppress_selection: false,
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
            agent_picker: None,
            agent_session_names: Vec::new(),
            agent_note: None,
            screen_thumbs: Vec::new(),
        }
    }
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui::default());
    static ACTIONS: RefCell<Option<Retained<Actions>>> = const { RefCell::new(None) };
    static SOURCE: RefCell<Option<Retained<SourceList>>> = const { RefCell::new(None) };
    static TOOLBAR_DELEGATE: RefCell<Option<Retained<SettingsToolbar>>> = const { RefCell::new(None) };
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

unsafe fn auto_layout(view: &NSView) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
}

unsafe fn fix_height(view: &NSView, height: f64) {
    view.heightAnchor().constraintEqualToConstant(height).setActive(true);
}

unsafe fn fix_width(view: &NSView, width: f64) {
    view.widthAnchor().constraintEqualToConstant(width).setActive(true);
}

unsafe fn pin_edges(view: &NSView, parent: &NSView, top: f64, bottom: f64, leading: f64, trailing: f64) {
    view.topAnchor().constraintEqualToAnchor_constant(&parent.topAnchor(), top).setActive(true);
    view.bottomAnchor().constraintEqualToAnchor_constant(&parent.bottomAnchor(), bottom).setActive(true);
    view.leadingAnchor().constraintEqualToAnchor_constant(&parent.leadingAnchor(), leading).setActive(true);
    view.trailingAnchor().constraintEqualToAnchor_constant(&parent.trailingAnchor(), trailing).setActive(true);
}

unsafe fn vstack() -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm());
    auto_layout(&stack);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::Width);
    stack.setDistribution(NSStackViewDistribution::GravityAreas);
    stack.setSpacing(0.0);
    stack
}

unsafe fn hstack() -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm());
    auto_layout(&stack);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    stack.setAlignment(NSLayoutAttribute::CenterY);
    stack.setDistribution(NSStackViewDistribution::GravityAreas);
    stack.setSpacing(8.0);
    stack
}

unsafe fn auto_label(text: &str, size: f64, weight: f64, color: &NSColor) -> Retained<NSTextField> {
    let field = label(text, size, weight, color, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&field);
    field.setAlignment(NSTextAlignment::Left);
    field
}

unsafe fn auto_paragraph(text: &str, size: f64, color: &NSColor) -> Retained<NSTextField> {
    let field = paragraph(text, size, color, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&field);
    field.setAlignment(NSTextAlignment::Left);
    field
}

unsafe fn paragraph_row(text: &str, size: f64, color: &NSColor, _width: f64) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    // Wrapping only. Do NOT give the row (or the body) a fixed width: a fixed width here makes the
    // page's document exactly as wide as the window's content area, and once the vertical scroller
    // takes its 17 points the overflow feeds back into the window, which grows on every rebuild.
    let body = auto_paragraph(text, size, color);
    // Fill the row from the left and wrap to the actual frame width. The leading(0)+trailing(0)
    // pins below are required; dropping the label's horizontal content hugging/compression lets
    // those pins win so the label stretches to the full row width instead of hugging its text
    // intrinsic width (which would leave it centered/right under a Width-aligned Fill stack).
    // Resize triggers a full rebuild, so frame-driven wrapping replaces a frozen
    // preferredMaxLayoutWidth and the label always wraps to its real frame.
    body.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    body.setContentCompressionResistancePriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    row.addSubview(&body);
    pin_edges(&body, &row, 0.0, 0.0, 0.0, 0.0);
    row
}

unsafe fn auto_separator(theme: &Theme) -> Retained<NSBox> {
    let line = NSBox::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 1.0)));
    line.setBoxType(NSBoxType::NSBoxCustom);
    line.setTitlePosition(NSTitlePosition::NSNoTitle);
    line.setBorderWidth(0.0);
    line.setCornerRadius(0.0);
    line.setFillColor(&theme.border);
    line.setBorderColor(&theme.border);
    auto_layout(&line);
    fix_height(&line, 1.0);
    line
}

unsafe fn section_heading(title: &str, theme: &Theme) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    fix_height(&row, 16.0);
    let label = auto_label(title, 11.0, NSFontWeightSemibold, &theme.secondary);
    row.addSubview(&label);
    label.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    label.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    row
}

unsafe fn hug_height(view: &NSView) {
    view.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Vertical);
}

unsafe fn finish_page(stack: &NSStackView) {
    let spacer = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&spacer);
    spacer.heightAnchor().constraintGreaterThanOrEqualToConstant(0.0).setActive(true);
    spacer.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    stack.addArrangedSubview(&spacer);
}

unsafe fn group_container(theme: &Theme, rows: Vec<Retained<NSView>>) -> Retained<NSView> {
    let container = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&container);
    let background = card_box(theme, &container, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&background);
    pin_edges(&background, &container, 0.0, 0.0, 0.0, 0.0);
    let stack = vstack();
    container.addSubview(&stack);
    pin_edges(&stack, &container, 6.0, -6.0, 16.0, -16.0);
    let count = rows.len();
    for (index, row) in rows.into_iter().enumerate() {
        stack.addArrangedSubview(&row);
        if index + 1 < count {
            stack.addArrangedSubview(&auto_separator(theme));
        }
    }
    container
}

unsafe fn finish_row(
    row: &NSView,
    theme: &Theme,
    title: &str,
    detail: &str,
    control: &NSView,
    control_width: Option<f64>,
) {
    fix_height(row, 56.0);
    let title = auto_label(title, 13.0, NSFontWeightMedium, &theme.text);
    let detail = auto_label(detail, 11.0, NSFontWeightRegular, &theme.secondary);
    row.addSubview(&title);
    row.addSubview(&detail);
    row.addSubview(control);
    auto_layout(control);
    if let Some(width) = control_width {
        fix_width(control, width);
    }
    title.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    title.topAnchor().constraintEqualToAnchor_constant(&row.topAnchor(), 8.0).setActive(true);
    title.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&control.leadingAnchor(), -12.0).setActive(true);
    detail.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    detail.topAnchor().constraintEqualToAnchor_constant(&title.bottomAnchor(), 2.0).setActive(true);
    detail.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&control.leadingAnchor(), -12.0).setActive(true);
    control.trailingAnchor().constraintEqualToAnchor(&row.trailingAnchor()).setActive(true);
    control.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
}

unsafe fn row_toggle(theme: &Theme, title: &str, detail: &str, on: bool, action: objc2::runtime::Sel) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    let control = switch(on, &row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), action);
    control.setAccessibilityLabel(Some(&NSString::from_str(title)));
    finish_row(&row, theme, title, detail, &control, None);
    row
}

unsafe fn row_popup(theme: &Theme, title: &str, detail: &str, items: &[&str], selected: usize, action: objc2::runtime::Sel) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    let control = popup(&row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), items, selected, action);
    finish_row(&row, theme, title, detail, &control, Some(170.0));
    row
}

unsafe fn section_with_button(theme: &Theme, title: &str, button_title: &str, enabled: bool, action: objc2::runtime::Sel) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    fix_height(&row, 24.0);
    let label = auto_label(title, 11.0, NSFontWeightSemibold, &theme.secondary);
    row.addSubview(&label);
    label.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    label.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    let button = button(button_title, &row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, action);
    auto_layout(&button);
    fix_width(&button, 140.0);
    fix_height(&button, 24.0);
    button.setEnabled(enabled);
    button.trailingAnchor().constraintEqualToAnchor(&row.trailingAnchor()).setActive(true);
    button.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    row
}

unsafe fn text_field_auto(value: &str, parent: &NSView, width: f64, placeholder: Option<&str>) -> Retained<NSTextField> {
    let field = text_field(value, parent, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), placeholder);
    auto_layout(&field);
    fix_width(&field, width);
    fix_height(&field, 26.0);
    field
}

unsafe fn number_field_auto(value: &str, parent: &NSView, width: f64, tag_value: isize) -> Retained<NSTextField> {
    let field = number_field(value, parent, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), tag_value);
    auto_layout(&field);
    fix_width(&field, width);
    fix_height(&field, 24.0);
    field
}

unsafe fn row_field_save(theme: &Theme, title: &str, detail: &str, value: &str, action: objc2::runtime::Sel) -> (Retained<NSView>, Retained<NSTextField>) {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    let bar = hstack();
    let field = text_field_auto(value, &bar, 220.0, Some("about:blank"));
    let save = button("Save", &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, action);
    auto_layout(&save);
    fix_width(&save, 60.0);
    fix_height(&save, 26.0);
    bar.addArrangedSubview(&field);
    bar.addArrangedSubview(&save);
    finish_row(&row, theme, title, detail, &bar, Some(288.0));
    (row, field)
}

unsafe fn row_text_button(theme: &Theme, title: &str, detail: &str, button_title: &str, action: objc2::runtime::Sel) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    fix_height(&row, 56.0);
    let title = auto_label(title, 13.0, NSFontWeightMedium, &theme.text);
    let detail = auto_label(detail, 11.0, NSFontWeightRegular, &theme.secondary);
    let control = button(button_title, &row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, action);
    auto_layout(&control);
    // Minimum width keeps short labels ("Open") a comfortable size while longer ones
    // ("Check for Updates…") grow to fit instead of truncating.
    control.widthAnchor().constraintGreaterThanOrEqualToConstant(76.0).setActive(true);
    control.setContentHuggingPriority_forOrientation(
        objc2_app_kit::NSLayoutPriorityDefaultHigh,
        NSLayoutConstraintOrientation::Horizontal,
    );
    fix_height(&control, 24.0);
    row.addSubview(&title);
    row.addSubview(&detail);
    title.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    title.topAnchor().constraintEqualToAnchor_constant(&row.topAnchor(), 8.0).setActive(true);
    title.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&control.leadingAnchor(), -12.0).setActive(true);
    detail.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    detail.topAnchor().constraintEqualToAnchor_constant(&title.bottomAnchor(), 2.0).setActive(true);
    detail.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&control.leadingAnchor(), -12.0).setActive(true);
    control.trailingAnchor().constraintEqualToAnchor(&row.trailingAnchor()).setActive(true);
    control.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    row
}

unsafe fn row_key_value(theme: &Theme, key: &str, value: &str) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    fix_height(&row, 36.0);
    let key = auto_label(key, 13.0, NSFontWeightMedium, &theme.text);
    let value = auto_label(value, 12.0, NSFontWeightRegular, &theme.secondary);
    value.setAlignment(NSTextAlignment::Right);
    row.addSubview(&key);
    row.addSubview(&value);
    key.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    key.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    value.trailingAnchor().constraintEqualToAnchor(&row.trailingAnchor()).setActive(true);
    value.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    key.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&value.leadingAnchor(), -12.0).setActive(true);
    row
}

unsafe fn page_stack(host: &NSView) -> (Retained<NSScrollView>, Retained<NSStackView>) {
    let bounds = host.bounds();
    let scroll = NSScrollView::initWithFrame(mtm().alloc(), bounds);
    scroll.setHasVerticalScroller(true);
    scroll.setDrawsBackground(false);
    host.addSubview(&scroll);
    // Pin the scroll view instead of leaving it on an autoresizing mask. A frame-based scroll view
    // reports a fitting width that includes its scroller, and the window grows to fit that on every
    // rebuild once the page is long enough to scroll.
    auto_layout(&scroll);
    scroll.leadingAnchor().constraintEqualToAnchor(&host.leadingAnchor()).setActive(true);
    scroll.trailingAnchor().constraintEqualToAnchor(&host.trailingAnchor()).setActive(true);
    scroll.topAnchor().constraintEqualToAnchor(&host.topAnchor()).setActive(true);
    scroll.bottomAnchor().constraintEqualToAnchor(&host.bottomAnchor()).setActive(true);
    let doc = FlippedView::new(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&doc);
    scroll.setDocumentView(Some(&doc));
    let clip = scroll.contentView();
    doc.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()).setActive(true);
    doc.trailingAnchor().constraintEqualToAnchor(&clip.trailingAnchor()).setActive(true);
    doc.topAnchor().constraintEqualToAnchor(&clip.topAnchor()).setActive(true);
    doc.heightAnchor().constraintGreaterThanOrEqualToAnchor(&clip.heightAnchor()).setActive(true);
    let stack = vstack();
    stack.setDistribution(NSStackViewDistribution::Fill);
    doc.addSubview(&stack);
    pin_edges(&stack, &doc, MARGIN, -MARGIN, MARGIN, -MARGIN);
    (scroll, stack)
}

unsafe fn build_general(host: &NSView, model: &Model, theme: &Theme) {
    let (scroll, stack) = page_stack(host);

    let menu_heading = section_heading("Menu Bar", theme);
    stack.addArrangedSubview(&menu_heading);
    stack.setCustomSpacing_afterView(8.0, &menu_heading);
    let menu = group_container(theme, vec![row_toggle(
        theme,
        "Show brightness presets",
        "List 25, 50, 75 and 100 percent under each display in the menu.",
        model.config.show_brightness,
        sel!(toggleBrightness:),
    )]);
    hug_height(&menu);
    stack.addArrangedSubview(&menu);
    stack.setCustomSpacing_afterView(24.0, &menu);

    let displays_heading = section_heading("Displays", theme);
    stack.addArrangedSubview(&displays_heading);
    stack.setCustomSpacing_afterView(8.0, &displays_heading);
    let current = match model.config.auto_reclaim_minutes {
        None => 0,
        Some(1) => 1,
        Some(5) => 2,
        Some(15) => 3,
        Some(_) => 4,
    };
    let displays = group_container(theme, vec![
        row_toggle(
            theme,
            "Reclaim displays on quit",
            "Bring every released display back before Lumen exits.",
            model.config.reclaim_on_quit,
            sel!(toggleReclaim:),
        ),
        row_popup(
            theme,
            "Auto-reclaim",
            "Bring a released display back by itself after a while.",
            &["Off", "After 1 minute", "After 5 minutes", "After 15 minutes", "After 30 minutes"],
            current,
            sel!(autoReclaim:),
        ),
    ]);
    hug_height(&displays);
    stack.addArrangedSubview(&displays);
    stack.setCustomSpacing_afterView(24.0, &displays);

    let keys_heading = section_heading("Brightness Keys", theme);
    stack.addArrangedSubview(&keys_heading);
    stack.setCustomSpacing_afterView(8.0, &keys_heading);
    let step_index = match model.config.brightness_step {
        crate::config::BrightnessStep::Coarse => 0,
        crate::config::BrightnessStep::Normal => 1,
        crate::config::BrightnessStep::Fine => 2,
    };
    let style_index = match model.config.hud_style {
        crate::config::HudStyle::Tahoe => 0,
        crate::config::HudStyle::Classic => 1,
    };
    let mut key_rows = vec![
        row_toggle(
            theme,
            "Use the brightness keys",
            "Let F1/F2 control the display under the pointer and show Lumen's indicator.",
            model.config.brightness_keys_enabled,
            sel!(toggleBrightnessKeys:),
        ),
        row_popup(
            theme,
            "Step",
            "How far one key press moves brightness.",
            &["Coarse", "Normal", "Fine"],
            step_index,
            sel!(brightnessStep:),
        ),
        row_popup(
            theme,
            "Indicator",
            "How the on-screen brightness indicator looks.",
            &["Tahoe", "Classic"],
            style_index,
            sel!(hudStyle:),
        ),
    ];
    let mut blocked_note: Option<&str> = None;
    if model.config.brightness_keys_enabled {
        if !crate::hotkeys::permission::trusted() {
            blocked_note = Some(
                "macOS 27 routes the brightness-key permission through System & App Protection \
                 Override, which grants full data, photo, history, control and input access. \
                 Lumen does not request it automatically. To use F1/F2, grant it manually in \
                 System Settings > Privacy & Security; otherwise use the menu sliders.",
            );
        } else if crate::hotkeys::status() == crate::hotkeys::Status::Offline {
            key_rows.push(row_text_button(
                theme,
                "Media keys offline",
                "The system disabled Lumen's key tap. Retry to reinstall it.",
                "Retry",
                sel!(retryBrightnessKeys:),
            ));
        }
    }
    let keys = group_container(theme, key_rows);
    hug_height(&keys);
    stack.addArrangedSubview(&keys);
    if let Some(note) = blocked_note {
        let note = paragraph_row(
            note,
            11.0,
            &theme.secondary,
            host.bounds().size.width - MARGIN * 2.0,
        );
        stack.setCustomSpacing_afterView(8.0, &keys);
        stack.addArrangedSubview(&note);
        stack.setCustomSpacing_afterView(24.0, &note);
    } else {
        stack.setCustomSpacing_afterView(24.0, &keys);
    }

    let startup_heading = section_heading("Startup", theme);
    stack.addArrangedSubview(&startup_heading);
    stack.setCustomSpacing_afterView(8.0, &startup_heading);
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
    let startup = group_container(theme, vec![row_toggle(
        theme,
        "Launch at login",
        detail,
        on,
        sel!(toggleLoginItem:),
    )]);
    hug_height(&startup);
    stack.addArrangedSubview(&startup);
    if let Some(note) = UI.with(|ui| ui.borrow().login_note.clone()) {
        let note = paragraph_row(&note, 11.0, &theme.secondary, host.bounds().size.width - MARGIN * 2.0);
        stack.setCustomSpacing_afterView(8.0, &startup);
        stack.addArrangedSubview(&note);
        stack.setCustomSpacing_afterView(24.0, &note);
    } else {
        stack.setCustomSpacing_afterView(24.0, &startup);
    }

    let window_heading = section_heading("Settings Window", theme);
    stack.addArrangedSubview(&window_heading);
    stack.setCustomSpacing_afterView(8.0, &window_heading);
    let index = match model.config.settings_display {
        SettingsDisplay::Invocation => 0,
        SettingsDisplay::Agent => 1,
        SettingsDisplay::Main => 2,
        SettingsDisplay::Last => 3,
    };
    let window = group_container(theme, vec![row_popup(
        theme,
        "Open on",
        "Where Settings appears when you choose it from the menu.",
        &["Display I click from", "Agent", "Main display", "Last display"],
        index,
        sel!(settingsDisplay:),
    )]);
    hug_height(&window);
    stack.addArrangedSubview(&window);
    finish_page(&stack);

    UI.with(|ui| ui.borrow_mut().scroll = Some(scroll));
}

unsafe fn slider_row(theme: &Theme, title: &str, value: u16, max: u16, tag_value: isize) -> (Retained<NSView>, Retained<NSTextField>) {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    fix_height(&row, 40.0);
    let label = auto_label(title, 12.0, NSFontWeightMedium, &theme.text);
    let slider = NSSlider::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 22.0)));
    auto_layout(&slider);
    slider.setMinValue(0.0);
    slider.setMaxValue(max.max(1) as f64);
    slider.setDoubleValue(value as f64);
    slider.setContinuous(true);
    slider.setNumberOfTickMarks(5);
    slider.setAllowsTickMarkValuesOnly(false);
    slider.setTickMarkPosition(NSTickMarkPosition::Below);
    slider.setTag(tag_value);
    wire(&slider, sel!(controlChanged:));
    let readout = auto_label(&format!("{}%", percent_of(value, max)), 11.0, NSFontWeightRegular, &theme.secondary);
    readout.setAlignment(NSTextAlignment::Right);
    row.addSubview(&label);
    row.addSubview(&slider);
    row.addSubview(&readout);
    label.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    label.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    slider.leadingAnchor().constraintEqualToAnchor_constant(&label.trailingAnchor(), 12.0).setActive(true);
    slider.trailingAnchor().constraintEqualToAnchor_constant(&readout.leadingAnchor(), -12.0).setActive(true);
    slider.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    slider.heightAnchor().constraintEqualToConstant(22.0).setActive(true);
    readout.trailingAnchor().constraintEqualToAnchor(&row.trailingAnchor()).setActive(true);
    readout.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    readout.widthAnchor().constraintEqualToConstant(56.0).setActive(true);
    (row, readout)
}

unsafe fn input_row(theme: &Theme, index: usize, input: &InputSetting) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    fix_height(&row, 38.0);
    let cb = checkbox(input.enabled, &row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), sel!(toggleInput:));
    auto_layout(&cb);
    fix_width(&cb, 18.0);
    fix_height(&cb, 18.0);
    tag(&cb, index as isize);
    cb.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    cb.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    if let Some((symbol, color)) = match input.verified {
        Some(true) => Some(("checkmark.circle.fill", NSColor::systemGreenColor())),
        Some(false) => Some(("xmark.circle", NSColor::systemRedColor())),
        None => None,
    } {
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(symbol), None) {
            let view = NSImageView::new(mtm());
            auto_layout(&view);
            fix_width(&view, 14.0);
            fix_height(&view, 14.0);
            view.setImage(Some(&image));
            view.setContentTintColor(Some(&color));
            row.addSubview(&view);
            view.leadingAnchor().constraintEqualToAnchor_constant(&cb.trailingAnchor(), 8.0).setActive(true);
            view.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
        }
    }
    let label = auto_label(&input.label, 13.0, NSFontWeightMedium, if input.enabled { &theme.text } else { &theme.secondary });
    fix_width(&label, 150.0);
    let code = mono(&format!("{:#06x}", input.value), 11.0, &theme.secondary, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&code);
    let protocol = button(input.protocol.label(), &row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(cycleProtocol:));
    auto_layout(&protocol);
    fix_width(&protocol, 70.0);
    fix_height(&protocol, 24.0);
    tag(&protocol, index as isize);
    protocol.setToolTip(Some(&NSString::from_str("Click to switch between standard DDC and the LG alternate protocol.")));
    let test = button("Test", &row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), false, sel!(testInput:));
    auto_layout(&test);
    fix_width(&test, 54.0);
    fix_height(&test, 24.0);
    tag(&test, index as isize);
    let remove = button("×", &row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), false, sel!(removeInput:));
    auto_layout(&remove);
    fix_width(&remove, 22.0);
    fix_height(&remove, 24.0);
    tag(&remove, index as isize);
    row.addSubview(&label);
    row.addSubview(&code);
    label.leadingAnchor().constraintEqualToAnchor_constant(&cb.trailingAnchor(), 28.0).setActive(true);
    label.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    code.leadingAnchor().constraintEqualToAnchor_constant(&label.trailingAnchor(), 8.0).setActive(true);
    code.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    remove.trailingAnchor().constraintEqualToAnchor(&row.trailingAnchor()).setActive(true);
    remove.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    test.trailingAnchor().constraintEqualToAnchor_constant(&remove.leadingAnchor(), -8.0).setActive(true);
    test.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    protocol.trailingAnchor().constraintEqualToAnchor_constant(&test.leadingAnchor(), -8.0).setActive(true);
    protocol.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
    code.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&protocol.leadingAnchor(), -12.0).setActive(true);
    row
}

unsafe fn agent_session_row(theme: &Theme, session: &crate::agent::Session, index: usize) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    let alive = crate::agent::session_alive(session);
    let phase = crate::agent::phase(&session.name);
    let mode = if session.headless { "headless" } else { "windowed" };
    let mut detail = if alive {
        format!("{mode} · pid {} · port {}", session.pid, session.port)
    } else {
        format!("{mode} · stopped")
    };
    match phase {
        Some(crate::agent::Phase::Starting) => detail.push_str(" · starting…"),
        Some(crate::agent::Phase::Stopping) => detail.push_str(" · stopping…"),
        Some(crate::agent::Phase::Failed) => detail.push_str(" · failed"),
        _ => {}
    }
    let bar = hstack();
    bar.setSpacing(8.0);
    let shot = button("Screenshot", &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(agentShot:));
    auto_layout(&shot);
    fix_width(&shot, 88.0);
    fix_height(&shot, 24.0);
    tag(&shot, index as isize);
    let stop = button("Stop", &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(agentStop:));
    auto_layout(&stop);
    fix_width(&stop, 60.0);
    fix_height(&stop, 24.0);
    tag(&stop, index as isize);
    if matches!(phase, Some(crate::agent::Phase::Starting | crate::agent::Phase::Stopping)) {
        shot.setEnabled(false);
        stop.setEnabled(false);
    }
    bar.addArrangedSubview(&shot);
    bar.addArrangedSubview(&stop);
    finish_row(&row, theme, &session.name, &detail, &bar, Some(156.0));
    row
}

unsafe fn agent_phase_row(theme: &Theme, name: &str, phase: crate::agent::Phase) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&row);
    fix_height(&row, 44.0);
    let title = auto_label(name, 13.0, NSFontWeightMedium, &theme.text);
    let detail = match phase {
        crate::agent::Phase::Starting => "starting…",
        crate::agent::Phase::Stopping => "stopping…",
        crate::agent::Phase::Failed => "last start failed",
        crate::agent::Phase::Running => "running",
    };
    let detail = auto_label(detail, 11.0, NSFontWeightRegular, &theme.secondary);
    row.addSubview(&title);
    row.addSubview(&detail);
    title.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    title.topAnchor().constraintEqualToAnchor_constant(&row.topAnchor(), 6.0).setActive(true);
    detail.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
    detail.topAnchor().constraintEqualToAnchor_constant(&title.bottomAnchor(), 2.0).setActive(true);
    row
}

unsafe fn build_virtual(host: &NSView, model: &Model, theme: &Theme) {
    UI.with(|ui| ui.borrow_mut().screen_thumbs.clear());
    let (scroll, stack) = page_stack(host);
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
    let new_row = section_with_button(theme, "Virtual Displays", "New Virtual Display", !at_cap, sel!(newScreen:));
    if at_cap {
        if let Some(button) = new_row.subviews().iter().find(|view| view.is_kind_of::<NSButton>()) {
            let _: () = msg_send![&*button, setToolTip: &*NSString::from_str("Lumen supports up to four virtual displays.")];
        }
    }
    stack.addArrangedSubview(&new_row);
    stack.setCustomSpacing_afterView(8.0, &new_row);

    if screens.is_empty() {
        let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&row);
        fix_height(&row, 62.0);
        let title = auto_label("No virtual displays", 13.0, NSFontWeightMedium, &theme.text);
        let detail = auto_paragraph(
            "Create one for an agent workspace, a preview or a capture source. Displays exist only in memory.",
            11.0,
            &theme.secondary,
        );
        // Fill the card from the left and wrap to the frame. Dropping the horizontal content
        // hugging/compression lets the required leading/trailing pins win so the label stretches to
        // the full row width; frame-driven wrapping (resize rebuilds) replaces a frozen
        // preferredMaxLayoutWidth that would otherwise leave the text centered/right.
        detail.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
        detail.setContentCompressionResistancePriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
        row.addSubview(&title);
        row.addSubview(&detail);
        title.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
        title.topAnchor().constraintEqualToAnchor_constant(&row.topAnchor(), 10.0).setActive(true);
        detail.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
        detail.topAnchor().constraintEqualToAnchor_constant(&title.bottomAnchor(), 4.0).setActive(true);
        detail.trailingAnchor().constraintEqualToAnchor(&row.trailingAnchor()).setActive(true);
        let group = group_container(theme, vec![row]);
        hug_height(&group);
        stack.addArrangedSubview(&group);
        stack.setCustomSpacing_afterView(24.0, &group);
    }

    for (index, screen) in screens.iter().enumerate() {
        let config = model.config.virtual_screen(&screen.name).cloned();
        let refresh = config.as_ref().map(|config| config.refresh).unwrap_or(60.0);
        let hidpi = config.as_ref().map(|config| config.hidpi).unwrap_or(false);
        let auto = config.as_ref().map(|config| config.auto_create).unwrap_or(true);

        let thumb_width = 176.0;
        let thumb_height = (thumb_width * screen.height as f64 / screen.width.max(1) as f64)
            .clamp(64.0, 112.0);
        let thumb = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(thumb_width, thumb_height)));
        thumb.setWantsLayer(true);
        if let Some(layer) = thumb.layer() {
            layer.setCornerRadius(6.0);
            layer.setMasksToBounds(true);
            layer.setContentsScale(2.0);
        }
        UI.with(|ui| ui.borrow_mut().screen_thumbs.push((screen.name.clone(), thumb.clone())));
        let thumb_row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&thumb_row);
        fix_height(&thumb_row, thumb_height);
        thumb_row.addSubview(&thumb);
        thumb.leadingAnchor().constraintEqualToAnchor(&thumb_row.leadingAnchor()).setActive(true);
        thumb.topAnchor().constraintEqualToAnchor(&thumb_row.topAnchor()).setActive(true);
        thumb.widthAnchor().constraintEqualToConstant(thumb_width).setActive(true);
        thumb.heightAnchor().constraintEqualToConstant(thumb_height).setActive(true);

        let name_row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&name_row);
        fix_height(&name_row, 44.0);
        let name_label = auto_label("Name", 11.0, NSFontWeightRegular, &theme.secondary);
        let name = text_field_auto(&screen.name, &name_row, 200.0, None);
        let summary = auto_label(
            &format!(
                "{}x{} at {},{}  ·  {}x  ·  {} Hz",
                screen.width, screen.height, screen.x, screen.y, screen.scale, refresh
            ),
            11.0,
            NSFontWeightRegular,
            &theme.secondary,
        );
        name_row.addSubview(&name_label);
        name_row.addSubview(&summary);
        name_label.leadingAnchor().constraintEqualToAnchor(&name_row.leadingAnchor()).setActive(true);
        name_label.centerYAnchor().constraintEqualToAnchor(&name_row.centerYAnchor()).setActive(true);
        name.leadingAnchor().constraintEqualToAnchor_constant(&name_row.leadingAnchor(), 44.0).setActive(true);
        name.centerYAnchor().constraintEqualToAnchor(&name_row.centerYAnchor()).setActive(true);
        summary.leadingAnchor().constraintGreaterThanOrEqualToAnchor_constant(&name.trailingAnchor(), 12.0).setActive(true);
        summary.trailingAnchor().constraintEqualToAnchor(&name_row.trailingAnchor()).setActive(true);
        summary.centerYAnchor().constraintEqualToAnchor(&name_row.centerYAnchor()).setActive(true);

        let geometry_row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&geometry_row);
        fix_height(&geometry_row, 40.0);
        let bar = hstack();
        bar.setSpacing(10.0);
        geometry_row.addSubview(&bar);
        bar.leadingAnchor().constraintEqualToAnchor(&geometry_row.leadingAnchor()).setActive(true);
        bar.centerYAnchor().constraintEqualToAnchor(&geometry_row.centerYAnchor()).setActive(true);
        bar.addArrangedSubview(&auto_label("W", 11.0, NSFontWeightRegular, &theme.secondary));
        let width = number_field_auto(&screen.width.to_string(), &bar, 58.0, (index * 10 + 1) as isize);
        bar.addArrangedSubview(&width);
        bar.addArrangedSubview(&auto_label("H", 11.0, NSFontWeightRegular, &theme.secondary));
        let height = number_field_auto(&screen.height.to_string(), &bar, 58.0, (index * 10 + 2) as isize);
        bar.addArrangedSubview(&height);
        bar.addArrangedSubview(&auto_label("Hz", 11.0, NSFontWeightRegular, &theme.secondary));
        let hz = number_field_auto(&format!("{refresh:.0}"), &bar, 44.0, (index * 10 + 3) as isize);
        bar.addArrangedSubview(&hz);
        let hidpi_box = option_box("2x", hidpi, &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&hidpi_box);
        bar.addArrangedSubview(&hidpi_box);
        let auto_box = option_box("Auto", auto, &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&auto_box);
        bar.addArrangedSubview(&auto_box);

        let actions_row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&actions_row);
        fix_height(&actions_row, 44.0);
        let hint = auto_label("Apply recreates the display when its size or rate changes.", 11.0, NSFontWeightRegular, &theme.secondary);
        actions_row.addSubview(&hint);
        hint.leadingAnchor().constraintEqualToAnchor(&actions_row.leadingAnchor()).setActive(true);
        hint.centerYAnchor().constraintEqualToAnchor(&actions_row.centerYAnchor()).setActive(true);
        let remove = button("Remove", &actions_row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(screenRemove:));
        auto_layout(&remove);
        destructive(&remove);
        fix_width(&remove, 76.0);
        fix_height(&remove, 24.0);
        tag(&remove, index as isize);
        remove.trailingAnchor().constraintEqualToAnchor(&actions_row.trailingAnchor()).setActive(true);
        remove.centerYAnchor().constraintEqualToAnchor(&actions_row.centerYAnchor()).setActive(true);
        let shown = crate::preview::is_open(&screen.name);
        let preview = button(if shown { "Hide" } else { "Preview" }, &actions_row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(screenPreview:));
        auto_layout(&preview);
        fix_width(&preview, 76.0);
        fix_height(&preview, 24.0);
        tag(&preview, index as isize);
        preview.trailingAnchor().constraintEqualToAnchor_constant(&remove.leadingAnchor(), -8.0).setActive(true);
        preview.centerYAnchor().constraintEqualToAnchor(&actions_row.centerYAnchor()).setActive(true);
        let apply = button("Apply", &actions_row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(screenApply:));
        auto_layout(&apply);
        fix_width(&apply, 76.0);
        fix_height(&apply, 24.0);
        tag(&apply, index as isize);
        apply.trailingAnchor().constraintEqualToAnchor_constant(&preview.leadingAnchor(), -8.0).setActive(true);
        apply.centerYAnchor().constraintEqualToAnchor(&actions_row.centerYAnchor()).setActive(true);
        hint.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&apply.leadingAnchor(), -12.0).setActive(true);

        let group = group_container(theme, vec![thumb_row, name_row, geometry_row, actions_row]);
        hug_height(&group);
        stack.addArrangedSubview(&group);
        stack.setCustomSpacing_afterView(16.0, &group);

        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            ui.screen_names.push(name);
            ui.screen_widths.push(width);
            ui.screen_heights.push(height);
            ui.screen_refreshes.push(hz);
            ui.screen_hidpi.push(hidpi_box);
            ui.screen_auto.push(auto_box);
        });
    }

    let agent_heading = section_heading("Agent Workspace", theme);
    stack.addArrangedSubview(&agent_heading);
    stack.setCustomSpacing_afterView(8.0, &agent_heading);
    let (url_row, url_field) = row_field_save(
        theme,
        "Home page",
        "Opened when an agent starts without an explicit URL.",
        model.config.agent_url.as_deref().unwrap_or(""),
        sel!(saveAgentUrl:),
    );
    let agent = group_container(theme, vec![
        row_toggle(theme, "Start agents headless", "No window and no focus change. Capture the page with lumen agent screenshot.", model.config.agent_headless, sel!(toggleAgentHeadless:)),
        url_row,
        row_toggle(theme, "Reopen last session at launch", "Start the most recent agent again when Lumen launches.", model.config.agent_reopen, sel!(toggleAgentReopen:)),
    ]);
    hug_height(&agent);
    stack.addArrangedSubview(&agent);
    stack.setCustomSpacing_afterView(12.0, &agent);
    let agent_note = paragraph_row("Headless sessions use the same CDP endpoint, so harnesses connect as usual.", 11.0, &theme.secondary, host.bounds().size.width - MARGIN * 2.0);
    stack.addArrangedSubview(&agent_note);
    stack.setCustomSpacing_afterView(24.0, &agent_note);
    UI.with(|ui| ui.borrow_mut().agent_url = Some(url_field));

    let agents_heading = section_heading("Agents", theme);
    stack.addArrangedSubview(&agents_heading);
    stack.setCustomSpacing_afterView(8.0, &agents_heading);
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.agent_picker = None;
        ui.agent_session_names.clear();
    });
    let sessions = crate::agent::sessions();
    let mut agent_rows: Vec<Retained<NSView>> = Vec::new();
    for (index, session) in sessions.iter().enumerate() {
        agent_rows.push(agent_session_row(theme, session, index));
        UI.with(|ui| ui.borrow_mut().agent_session_names.push(session.name.clone()));
    }
    for (name, phase) in crate::agent::phases() {
        if phase != crate::agent::Phase::Running
            && screens.iter().any(|screen| screen.name == name)
        {
            agent_rows.push(agent_phase_row(theme, &name, phase));
        }
    }
    if screens.is_empty() {
        let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&row);
        fix_height(&row, 40.0);
        let note = auto_label("Create a virtual display first.", 12.0, NSFontWeightRegular, &theme.secondary);
        row.addSubview(&note);
        note.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
        note.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
        agent_rows.push(row);
    } else {
        let start_row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&start_row);
        let bar = hstack();
        bar.setSpacing(8.0);
        let names: Vec<String> = screens.iter().map(|screen| screen.name.clone()).collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let selected = names.iter().position(|name| name == "Agent").unwrap_or(0);
        let picker = popup(&bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), &name_refs, selected, sel!(noop:));
        auto_layout(&picker);
        fix_width(&picker, 150.0);
        fix_height(&picker, 26.0);
        let headless = button("Headless", &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(agentStart:));
        auto_layout(&headless);
        fix_width(&headless, 90.0);
        fix_height(&headless, 26.0);
        tag(&headless, 0);
        let windowed = button("Windowed", &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(agentStart:));
        auto_layout(&windowed);
        fix_width(&windowed, 90.0);
        fix_height(&windowed, 26.0);
        tag(&windowed, 1);
        bar.addArrangedSubview(&picker);
        bar.addArrangedSubview(&headless);
        bar.addArrangedSubview(&windowed);
        finish_row(&start_row, theme, "Start an agent", "Runs on the selected virtual display.", &bar, Some(346.0));
        agent_rows.push(start_row);
        UI.with(|ui| ui.borrow_mut().agent_picker = Some(picker));
    }
    let agents = group_container(theme, agent_rows);
    hug_height(&agents);
    stack.addArrangedSubview(&agents);
    if let Some(note) = UI.with(|ui| ui.borrow().agent_note.clone()) {
        let row = paragraph_row(&note, 11.0, &theme.secondary, host.bounds().size.width - MARGIN * 2.0);
        stack.setCustomSpacing_afterView(8.0, &agents);
        stack.addArrangedSubview(&row);
        stack.setCustomSpacing_afterView(24.0, &row);
    } else {
        stack.setCustomSpacing_afterView(24.0, &agents);
    }

    let captures_heading = section_heading("Captures", theme);
    stack.addArrangedSubview(&captures_heading);
    stack.setCustomSpacing_afterView(8.0, &captures_heading);
    let limit = model.config.capture_limit;
    let (limit_index, limit_items) = match limit {
        None => (0, ["Keep all", "5", "20", "50"]),
        Some(5) => (1, ["Keep all", "5", "20", "50"]),
        Some(50) => (3, ["Keep all", "5", "20", "50"]),
        Some(_) => (2, ["Keep all", "5", "20", "50"]),
    };
    let captures_folder = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&captures_folder);
    let buttons = hstack();
    buttons.setSpacing(8.0);
    let open = button("Open", &buttons, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(openCaptures:));
    auto_layout(&open);
    fix_width(&open, 76.0);
    fix_height(&open, 24.0);
    let delete = button("Delete", &buttons, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(deleteCaptures:));
    auto_layout(&delete);
    destructive(&delete);
    fix_width(&delete, 76.0);
    fix_height(&delete, 24.0);
    buttons.addArrangedSubview(&delete);
    buttons.addArrangedSubview(&open);
    finish_row(&captures_folder, theme, "Captures folder", "", &buttons, Some(160.0));
    let captures = group_container(theme, vec![
        row_popup(theme, "Keep captures", "Page and screen screenshots kept per display folder.", &limit_items, limit_index, sel!(captureLimit:)),
        captures_folder,
    ]);
    hug_height(&captures);
    stack.addArrangedSubview(&captures);
    let path_note = paragraph_row(&crate::screens::captures_dir().display().to_string(), 11.0, &theme.secondary, host.bounds().size.width - MARGIN * 2.0);
    stack.setCustomSpacing_afterView(8.0, &captures);
    stack.addArrangedSubview(&path_note);
    finish_page(&stack);

    UI.with(|ui| ui.borrow_mut().scroll = Some(scroll));
}

unsafe fn build_displays(host: &NSView, model: &Model, theme: &Theme) {
    let bounds = host.bounds();
    if model.displays.is_empty() {
        let (scroll, stack) = page_stack(host);
        let heading = section_heading("Displays", theme);
        stack.addArrangedSubview(&heading);
        stack.setCustomSpacing_afterView(8.0, &heading);
        let title = auto_label("No displays", 15.0, NSFontWeightSemibold, &theme.text);
        stack.addArrangedSubview(&title);
        stack.setCustomSpacing_afterView(6.0, &title);
        let body = paragraph_row(
            "Lumen talks to monitors over DDC/CI and to the built-in panel through DisplayServices. Connect a monitor, or wake it, then scan again. A released display stays off until it is reclaimed.",
            12.0,
            &theme.secondary,
            bounds.size.width - MARGIN * 2.0,
        );
        stack.addArrangedSubview(&body);
        stack.setCustomSpacing_afterView(12.0, &body);
        let scan = button("Scan displays", &stack, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(refreshDisplays:));
        auto_layout(&scan);
        fix_width(&scan, 120.0);
        fix_height(&scan, 28.0);
        stack.addArrangedSubview(&scan);
        finish_page(&stack);
        UI.with(|ui| ui.borrow_mut().scroll = Some(scroll));
        return;
    }

    let list_width = 196.0;
    let selected = UI.with(|ui| ui.borrow().selected).min(model.displays.len() - 1);
    UI.with(|ui| ui.borrow_mut().selected = selected);

    unsafe {
        let (display_scroll, display_table) = make_source_scroll(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(list_width, bounds.size.height)),
            34.0,
            list_width - 32.0,
            false,
        );
        UI.with(|ui| ui.borrow_mut().display_list = Some(display_table.clone()));
        host.addSubview(&display_scroll);
        reload_and_select(&display_table, selected);
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

    let detail = NSView::initWithFrame(
        mtm().alloc(),
        NSRect::new(
            NSPoint::new(list_width + 1.0, 0.0),
            NSSize::new(bounds.size.width - list_width - 1.0, bounds.size.height),
        ),
    );
    detail.setAutoresizingMask(
        NSAutoresizingMaskOptions::NSViewWidthSizable | NSAutoresizingMaskOptions::NSViewHeightSizable,
    );
    host.addSubview(&detail);
    let (scroll, stack) = page_stack(&detail);

    let display = &model.displays[selected];
    let key = display.key.clone();
    let released = model.released(&key);
    let show_in_menu = model.config.display(&key).map(|config| config.show_in_menu).unwrap_or(true);
    let inputs = model.config.display(&key).map(|config| config.inputs.clone()).unwrap_or_default();

    let header = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&header);
    fix_height(&header, 52.0);
    let name = auto_label(&display.name, 17.0, NSFontWeightSemibold, &theme.text);
    let state = if released {
        "released"
    } else if display.builtin {
        "internal panel"
    } else {
        "connected"
    };
    let status = mono(
        &format!("{key}  ·  {state}"),
        11.0,
        &theme.secondary,
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
    );
    auto_layout(&status);
    header.addSubview(&name);
    header.addSubview(&status);
    name.leadingAnchor().constraintEqualToAnchor(&header.leadingAnchor()).setActive(true);
    name.topAnchor().constraintEqualToAnchor(&header.topAnchor()).setActive(true);
    status.leadingAnchor().constraintEqualToAnchor(&header.leadingAnchor()).setActive(true);
    status.topAnchor().constraintEqualToAnchor_constant(&name.bottomAnchor(), 4.0).setActive(true);
    hug_height(&header);
    stack.addArrangedSubview(&header);
    stack.setCustomSpacing_afterView(20.0, &header);

    if let Some(error) = &display.ddc_error {
        let text = if display.builtin {
            format!("Brightness is unavailable: {error}.")
        } else if display.controls_stale {
            format!(
                "Some DDC reads failed: {error}. Showing the last known values; choose Refresh \
                 from the menu to retry."
            )
        } else {
            format!(
                "DDC reads failed: {error}. Wake the monitor and choose Refresh from the menu to retry."
            )
        };
        let note = paragraph_row(
            &text,
            11.0,
            &theme.secondary,
            host.bounds().size.width - list_width - 1.0 - MARGIN * 2.0,
        );
        stack.addArrangedSubview(&note);
        stack.setCustomSpacing_afterView(16.0, &note);
    }

    let menu_heading = section_heading("Menu", theme);
    stack.addArrangedSubview(&menu_heading);
    stack.setCustomSpacing_afterView(8.0, &menu_heading);
    let menu = group_container(theme, vec![row_toggle(
        theme,
        "Show in menu",
        if display.builtin {
            "List the built-in display and its brightness in the menu bar menu."
        } else {
            "List this display and its inputs in the menu bar menu."
        },
        show_in_menu,
        sel!(toggleShowInMenu:),
    )]);
    hug_height(&menu);
    stack.addArrangedSubview(&menu);
    stack.setCustomSpacing_afterView(24.0, &menu);

    let controls_heading = section_heading("Controls", theme);
    stack.addArrangedSubview(&controls_heading);
    stack.setCustomSpacing_afterView(8.0, &controls_heading);
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.brightness_value = None;
        ui.contrast_value = None;
        ui.volume_value = None;
    });
    let mut control_rows: Vec<Retained<NSView>> = Vec::new();
    if let Some((value, max)) = display.brightness {
        let (row, readout) = slider_row(theme, "Brightness", value, max, 1);
        UI.with(|ui| ui.borrow_mut().brightness_value = Some(readout));
        control_rows.push(row);
    }
    if let Some((value, max)) = display.contrast {
        let (row, readout) = slider_row(theme, "Contrast", value, max, 2);
        UI.with(|ui| ui.borrow_mut().contrast_value = Some(readout));
        control_rows.push(row);
    }
    if let Some((value, max)) = display.volume {
        let (row, readout) = slider_row(theme, "Volume", value, max, 3);
        UI.with(|ui| ui.borrow_mut().volume_value = Some(readout));
        control_rows.push(row);
    }
    if control_rows.is_empty() {
        let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&row);
        fix_height(&row, 36.0);
        let message = if display.builtin {
            "Brightness is not available on this display"
        } else {
            "No DDC controls reported"
        };
        let label = auto_label(message, 12.0, NSFontWeightRegular, &theme.secondary);
        row.addSubview(&label);
        label.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
        label.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
        control_rows.push(row);
    }
    let controls = group_container(theme, control_rows);
    hug_height(&controls);
    stack.addArrangedSubview(&controls);
    stack.setCustomSpacing_afterView(24.0, &controls);

    if !display.builtin {
        let inputs_heading = section_heading("Inputs", theme);
        stack.addArrangedSubview(&inputs_heading);
        stack.setCustomSpacing_afterView(8.0, &inputs_heading);
        let mut rows: Vec<Retained<NSView>> = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            rows.push(input_row(theme, index, input));
        }
        if inputs.is_empty() {
            let row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
            auto_layout(&row);
            fix_height(&row, 36.0);
            let label = auto_label("No inputs configured", 12.0, NSFontWeightRegular, &theme.secondary);
            row.addSubview(&label);
            label.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()).setActive(true);
            label.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()).setActive(true);
            rows.push(row);
        }

        let add_row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&add_row);
        fix_height(&add_row, 44.0);
        let bar = hstack();
        bar.setSpacing(8.0);
        add_row.addSubview(&bar);
        bar.leadingAnchor().constraintEqualToAnchor(&add_row.leadingAnchor()).setActive(true);
        bar.centerYAnchor().constraintEqualToAnchor(&add_row.centerYAnchor()).setActive(true);
        let lf = text_field_auto("", &bar, 140.0, Some("Label"));
        let vf = text_field_auto("", &bar, 80.0, Some("0x00"));
        let protocol = UI.with(|ui| ui.borrow().draft_protocol);
        let protocol_button = button(protocol.label(), &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(cycleDraftProtocol:));
        auto_layout(&protocol_button);
        fix_width(&protocol_button, 70.0);
        fix_height(&protocol_button, 26.0);
        let add = button("Add", &bar, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(addInput:));
        auto_layout(&add);
        fix_width(&add, 60.0);
        fix_height(&add, 26.0);
        bar.addArrangedSubview(&lf);
        bar.addArrangedSubview(&vf);
        bar.addArrangedSubview(&protocol_button);
        bar.addArrangedSubview(&add);
        rows.push(add_row);
        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            ui.draft_label = Some(lf);
            ui.draft_value = Some(vf);
        });

        let reset_row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
        auto_layout(&reset_row);
        fix_height(&reset_row, 44.0);
        let reset = button("Reset to detected", &reset_row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(resetInputs:));
        auto_layout(&reset);
        fix_width(&reset, 140.0);
        fix_height(&reset, 26.0);
        reset.leadingAnchor().constraintEqualToAnchor(&reset_row.leadingAnchor()).setActive(true);
        reset.centerYAnchor().constraintEqualToAnchor(&reset_row.centerYAnchor()).setActive(true);
        let hint = auto_label("Changes apply to the menu immediately.", 11.0, NSFontWeightRegular, &theme.secondary);
        reset_row.addSubview(&hint);
        hint.leadingAnchor().constraintEqualToAnchor_constant(&reset.trailingAnchor(), 12.0).setActive(true);
        hint.trailingAnchor().constraintLessThanOrEqualToAnchor(&reset_row.trailingAnchor()).setActive(true);
        hint.centerYAnchor().constraintEqualToAnchor(&reset_row.centerYAnchor()).setActive(true);
        rows.push(reset_row);

        let inputs_group = group_container(theme, rows);
        hug_height(&inputs_group);
        stack.addArrangedSubview(&inputs_group);
    }
    finish_page(&stack);

    UI.with(|ui| ui.borrow_mut().scroll = Some(scroll));
}

unsafe fn build_about(host: &NSView, model: &Model, theme: &Theme) {
    let (scroll, stack) = page_stack(host);

    // The header is a plain container with explicit anchors rather than an hstack: NSStackView's
    // GravityAreas distribution will not stretch a single column to fill, so the text block would
    // hug its widest short label ("Lumen") and wrap the description into a narrow column (or, if
    // pinned trailing-only, shove the whole block to the right edge). Pinning the text column
    // leading = icon.trailing+16 and trailing = header.trailing gives it the full remaining width,
    // so the description wraps to the real window width. The header is a Width-aligned arranged
    // subview of the page stack, so no fixed width is introduced.
    let header = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&header);
    let icon = NSImageView::new(mtm());
    auto_layout(&icon);
    fix_width(&icon, 72.0);
    fix_height(&icon, 72.0);
    if let Some(image) = NSApplication::sharedApplication(mtm()).applicationIconImage() {
        icon.setImage(Some(&image));
    }
    header.addSubview(&icon);
    icon.leadingAnchor().constraintEqualToAnchor(&header.leadingAnchor()).setActive(true);
    icon.topAnchor().constraintEqualToAnchor(&header.topAnchor()).setActive(true);

    let text = vstack();
    text.setAlignment(NSLayoutAttribute::Leading);
    text.setSpacing(4.0);
    text.addArrangedSubview(&auto_label("Lumen", 22.0, NSFontWeightSemibold, &theme.text));
    text.addArrangedSubview(&auto_label(
        &format!("Version {}", crate::version_string()),
        12.0,
        NSFontWeightRegular,
        &theme.secondary,
    ));
    let description = auto_paragraph(
        "Menu bar control for DDC displays, virtual displays for agent workspaces, and a local control socket.",
        12.0,
        &theme.secondary,
    );
    // Fill the text column from the left and wrap to the actual frame width. Dropping the
    // horizontal content hugging/compression (instead of freezing a preferredMaxLayoutWidth) lets
    // the column width drive the wrap, so the description reflows at any window size. Resize
    // triggers a full rebuild, so no fixed width is needed; adding one would risk the document
    // exceeding the content area and feeding the scroller width back into the window.
    description.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    description.setContentCompressionResistancePriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    text.addArrangedSubview(&description);
    header.addSubview(&text);
    text.leadingAnchor().constraintEqualToAnchor_constant(&icon.trailingAnchor(), 16.0).setActive(true);
    text.trailingAnchor().constraintEqualToAnchor(&header.trailingAnchor()).setActive(true);
    text.topAnchor().constraintEqualToAnchor(&header.topAnchor()).setActive(true);
    // The header's height is the taller of the icon and the text column; pin the bottom to both so
    // the container hugs its content and the page stack lays the next section below it.
    header.bottomAnchor().constraintGreaterThanOrEqualToAnchor(&icon.bottomAnchor()).setActive(true);
    header.bottomAnchor().constraintEqualToAnchor(&text.bottomAnchor()).setActive(true);
    hug_height(&header);
    stack.addArrangedSubview(&header);
    // NSStackView Width alignment does not force this header to the stack's full width when its
    // content offers no horizontal resistance (the wrapping description has hugging/compression at
    // 1.0), so the header collapses to its content and gravity parks it on the trailing side.
    // Pin the header's leading and trailing to the stack to give it the full content width; the
    // icon then sits at the left and the text column fills to the right, wrapping the description
    // to the real window width. No fixed width is introduced, so the rebuild-on-resize invariant
    // (document never wider than the content area) holds.
    header.leadingAnchor().constraintEqualToAnchor(&stack.leadingAnchor()).setActive(true);
    header.trailingAnchor().constraintEqualToAnchor(&stack.trailingAnchor()).setActive(true);
    stack.setCustomSpacing_afterView(28.0, &header);

    // Updates: "Check for Updates" moved here from the menu bar. The status line reflects whether
    // Sparkle currently knows of an available version.
    if crate::updater::can_check() || cfg!(feature = "sparkle") {
        let updates_heading = section_heading("Updates", theme);
        stack.addArrangedSubview(&updates_heading);
        stack.setCustomSpacing_afterView(8.0, &updates_heading);
        let status = match crate::updater::available_version() {
            Some(version) => format!("Update to {version} is available"),
            None => format!("Lumen {} is up to date", crate::version_string()),
        };
        let updates = group_container(theme, vec![
            row_text_button(
                theme,
                "Software Update",
                &status,
                "Check for Updates…",
                sel!(checkUpdates:),
            ),
        ]);
        hug_height(&updates);
        stack.addArrangedSubview(&updates);
        stack.setCustomSpacing_afterView(24.0, &updates);
    }

    let places_heading = section_heading("Places", theme);
    stack.addArrangedSubview(&places_heading);
    stack.setCustomSpacing_afterView(8.0, &places_heading);
    let places = group_container(theme, vec![
        row_text_button(
            theme,
            "Configuration",
            &crate::config::Config::path().display().to_string(),
            "Open",
            sel!(openConfig:),
        ),
        row_text_button(
            theme,
            "Captures",
            &crate::screens::captures_dir().display().to_string(),
            "Open",
            sel!(openCaptures:),
        ),
    ]);
    hug_height(&places);
    stack.addArrangedSubview(&places);
    stack.setCustomSpacing_afterView(24.0, &places);

    let diagnostics_heading = section_heading("Diagnostics", theme);
    stack.addArrangedSubview(&diagnostics_heading);
    stack.setCustomSpacing_afterView(8.0, &diagnostics_heading);
    let sessions = crate::agent::sessions().len();
    let diagnostics = [
        ("Displays", format!("{} total", model.displays.len())),
        ("Virtual", format!("{} active", crate::screens::list().len())),
        ("Agents", format!("{sessions} session(s)")),
        ("Login item", format!("{:?}", crate::login::status())),
    ];
    let mut rows: Vec<Retained<NSView>> = diagnostics
        .iter()
        .map(|(key, value)| row_key_value(theme, key, value))
        .collect();
    let copy_row = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    auto_layout(&copy_row);
    fix_height(&copy_row, 44.0);
    let copy = button("Copy diagnostics", &copy_row, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)), true, sel!(copyDiagnostics:));
    auto_layout(&copy);
    fix_width(&copy, 140.0);
    fix_height(&copy, 26.0);
    copy.trailingAnchor().constraintEqualToAnchor(&copy_row.trailingAnchor()).setActive(true);
    copy.centerYAnchor().constraintEqualToAnchor(&copy_row.centerYAnchor()).setActive(true);
    rows.push(copy_row);
    let group = group_container(theme, rows);
    hug_height(&group);
    stack.addArrangedSubview(&group);
    finish_page(&stack);

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
    if page != Page::Virtual {
        crate::preview::set_thumbnails_visible(false);
    }
    unsafe { match page {
        Page::General => build_general(host, model, &theme),
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
    crate::preview::set_thumbnails_visible(page == Page::Virtual);
}

/// A native titlebar toolbar carrying the section tabs. `NSWindowToolbarStyle::Preference` renders
/// each item as a large SF Symbol above its label with the native rounded selection highlight, the
/// way System Settings draws its panes.
fn build_settings_toolbar(window: &NSWindow) {
    unsafe {
        let delegate = settings_toolbar();
        let toolbar = NSToolbar::initWithIdentifier(
            mtm().alloc(),
            &NSString::from_str("lumen.settings.tabs"),
        );
        toolbar.setDisplayMode(NSToolbarDisplayMode::IconAndLabel);
        toolbar.setAllowsUserCustomization(false);
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.setToolbar(Some(&toolbar));
        let page = UI.with(|ui| ui.borrow().page);
        toolbar.setSelectedItemIdentifier(Some(&NSString::from_str(page.slug())));
        UI.with(|ui| ui.borrow_mut().toolbar = Some(toolbar));
    }
}

fn build_chrome(window: &NSWindow) {
    unsafe {
        build_settings_toolbar(window);

        let Some(content) = window.contentView() else {
            return;
        };
        let bounds = content.bounds();
        for subview in content.subviews().iter() {
            subview.removeFromSuperview();
        }

        // The tabs live in the titlebar toolbar now, so the page content fills the whole content
        // view. Pin the host with Auto Layout (not an autoresizing mask): the scroll view inside
        // it is constraint-based, and mixing an autoresizing-mask parent with constraint children
        // makes the content drift and clip on live resize.
        let host = NSView::initWithFrame(
            mtm().alloc(),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(bounds.size.width, bounds.size.height),
            ),
        );
        auto_layout(&host);
        content.addSubview(&host);
        host.leadingAnchor().constraintEqualToAnchor(&content.leadingAnchor()).setActive(true);
        host.trailingAnchor().constraintEqualToAnchor(&content.trailingAnchor()).setActive(true);
        host.topAnchor().constraintEqualToAnchor(&content.topAnchor()).setActive(true);
        host.bottomAnchor().constraintEqualToAnchor(&content.bottomAnchor()).setActive(true);
        UI.with(|ui| ui.borrow_mut().host = Some(host));
    }
}

pub fn target() -> Option<Retained<Actions>> {
    ACTIONS.with(|slot| slot.borrow().clone())
}

/// Whether the Virtual Displays page is on screen, so thumbnails should keep refreshing.
pub(crate) fn thumbnails_visible() -> bool {
    UI.with(|ui| {
        let ui = ui.borrow();
        ui.page == Page::Virtual
            && ui.window.as_ref().map(|window| window.isVisible()).unwrap_or(false)
            && !ui.screen_thumbs.is_empty()
    })
}

/// Sets a screen thumbnail from an owned CGImage reference, which this function releases.
pub(crate) fn set_thumbnail(name: &str, image: *mut std::ffi::c_void) {
    if image.is_null() {
        return;
    }
    let view = UI.with(|ui| {
        ui.borrow()
            .screen_thumbs
            .iter()
            .find(|(screen, _)| screen == name)
            .map(|(_, view)| view.clone())
    });
    match view {
        Some(view) => unsafe {
            if let Some(layer) = view.layer() {
                layer.setContents(Some(&*(image as *const AnyObject)));
                layer.setContentsScale(view.window().map(|window| window.backingScaleFactor()).unwrap_or(2.0));
            }
            core_foundation::base::CFRelease(image as core_foundation::base::CFTypeRef);
        },
        None => unsafe {
            core_foundation::base::CFRelease(image as core_foundation::base::CFTypeRef);
        },
    }
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
        NSWorkspace::sharedWorkspace().notificationCenter().addObserver_selector_name_object(
            &actions,
            sel!(sessionActive:),
            Some(objc2_app_kit::NSWorkspaceSessionDidBecomeActiveNotification),
            None,
        );
        NSWorkspace::sharedWorkspace().notificationCenter().addObserver_selector_name_object(
            &actions,
            sel!(sessionResign:),
            Some(objc2_app_kit::NSWorkspaceSessionDidResignActiveNotification),
            None,
        );
        // macOS posts this when the Accessibility list changes. It is how FineTune notices a
        // grant without asking the user to toggle anything a second time.
        let accessibility_changed = NSString::from_str("com.apple.accessibility.api");
        NSDistributedNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &actions,
            sel!(accessibilityChanged:),
            Some(&accessibility_changed),
            None,
        );
    }
    actions
}

/// Opens Settings. `invocation` is the pointer at the moment the menu action ran; `forced`
/// overrides the configured placement for scripts and tests.
pub fn open(invocation: Option<NSPoint>, forced: Option<SettingsDisplay>) {
    let start = std::time::Instant::now();
    if ACTIONS.with(|slot| slot.borrow().is_none()) {
        ACTIONS.with(|slot| *slot.borrow_mut() = Some(make_actions()));
    }

    let (configured, width, height, saved_page) = crate::with_model(|model| {
        (
            model.config.settings_display,
            model.config.settings_width,
            model.config.settings_height,
            model.config.settings_page.clone(),
        )
    });
    let choice = forced.unwrap_or(configured);
    let target = resolve_screen(choice, invocation);

    let existing = UI.with(|ui| ui.borrow().window.clone());
    let fresh = existing.is_none();
    let window = match existing {
        Some(window) => window,
        None => {
            let window = create_window(width, height);
            UI.with(|ui| ui.borrow_mut().window = Some(window.clone()));
            window
        }
    };
    if fresh {
        if let Some(page) = Page::from_slug(&saved_page) {
            UI.with(|ui| ui.borrow_mut().page = page);
        }
    }

    if let Some(screen) = target {
        let current = window.screen().map(|screen| unsafe { screen.localizedName().to_string() });
        let name = unsafe { screen.localizedName().to_string() };
        if fresh || forced.is_some() || current.as_deref() != Some(name.as_str()) {
            let size = if fresh {
                NSSize::new(width, height)
            } else {
                window.frame().size
            };
            place_window(&window, &screen, size);
        }
        crate::with_model(|model| {
            if model.config.settings_last_screen.as_deref() != Some(name.as_str()) {
                model.config.settings_last_screen = Some(name.clone());
                model.config.save();
            }
        });
    }

    rebuild_page();
    #[allow(deprecated)]
    {
        NSApplication::sharedApplication(mtm()).activateIgnoringOtherApps(true);
    }
    window.makeKeyAndOrderFront(None);
    let _ = window.makeFirstResponder(None);
    crate::log::trace(format!("settings open: {} ms", start.elapsed().as_millis()));
}

/// Entry point for the control socket. No pointer is available, so `invocation` falls back to
/// the configured placement.
pub fn open_command(page: Option<&str>, screen: Option<&str>) -> Result<(), String> {
    let page = match page {
        Some(value) => Some(Page::from_slug(value).ok_or_else(|| format!("unknown page: {value}"))?),
        None => None,
    };
    let screen = match screen {
        Some("invocation") => Some(SettingsDisplay::Invocation),
        Some("agent") => Some(SettingsDisplay::Agent),
        Some("main") => Some(SettingsDisplay::Main),
        Some("last") => Some(SettingsDisplay::Last),
        Some(value) => return Err(format!("unknown screen: {value}")),
        None => None,
    };
    if let Some(page) = page {
        UI.with(|ui| ui.borrow_mut().page = page);
        crate::with_model(|model| {
            model.config.settings_page = page.slug().to_string();
            model.config.save();
        });
        sync_tabs(page);
    }
    open(None, screen);
    Ok(())
}

/// State for scripts and tests.
pub fn state() -> serde_json::Value {
    let (window, page) = UI.with(|ui| {
        let ui = ui.borrow();
        (ui.window.clone(), ui.page)
    });
    let visible = window.as_ref().map(|window| window.isVisible()).unwrap_or(false);
    let screen = window
        .and_then(|window| window.screen())
        .map(|screen| unsafe { screen.localizedName().to_string() });
    serde_json::json!({
        "page": page.slug(),
        "visible": visible,
        "screen": screen,
    })
}

fn screen_contains(frame: NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.x <= frame.origin.x + frame.size.width
        && point.y >= frame.origin.y
        && point.y <= frame.origin.y + frame.size.height
}

fn all_screens() -> Vec<Retained<NSScreen>> {
    let array = NSScreen::screens(mtm());
    (0..array.len())
        .filter_map(|index| array.get_retained(index))
        .collect()
}

fn screen_for_virtual(name: &str) -> Option<Retained<NSScreen>> {
    // Prefer the display id when this process created the screen. Fall back to the visible name,
    // which lets a dev build place its window on a screen created by the installed app.
    if let Some(info) = crate::screens::info(name) {
        let key = NSString::from_str("NSScreenNumber");
        for screen in all_screens() {
            let description = screen.deviceDescription();
            let Some(value) = (unsafe { description.objectForKey(&key) }) else {
                continue;
            };
            let id: u32 = unsafe { msg_send![&*value, unsignedIntValue] };
            if id == info.id {
                return Some(screen);
            }
        }
    }
    all_screens()
        .into_iter()
        .find(|screen| unsafe { screen.localizedName().to_string() } == name)
}

fn resolve_screen(choice: SettingsDisplay, invocation: Option<NSPoint>) -> Option<Retained<NSScreen>> {
    let screens = all_screens();
    let primary = screens.first().cloned();
    let invoked = invocation.and_then(|point| {
        screens
            .iter()
            .find(|screen| screen_contains(screen.frame(), point))
            .cloned()
    });
    match choice {
        SettingsDisplay::Invocation => invoked.or(primary),
        SettingsDisplay::Agent => screen_for_virtual("Agent").or(invoked).or(primary),
        SettingsDisplay::Main => primary,
        SettingsDisplay::Last => {
            let last = crate::with_model(|model| model.config.settings_last_screen.clone());
            last.and_then(|name| {
                screens
                    .iter()
                    .find(|screen| unsafe { screen.localizedName().to_string() } == name)
                    .cloned()
            })
            .or(invoked)
            .or(primary)
        }
    }
}

fn fit_size(visible: NSSize, size: NSSize) -> NSSize {
    NSSize::new(
        size.width
            .max(600.0)
            .min((visible.width - 24.0).max(600.0)),
        size.height
            .max(400.0)
            .min((visible.height - 24.0).max(400.0)),
    )
}

fn centered_origin(visible: NSRect, size: NSSize) -> NSPoint {
    NSPoint::new(
        visible.origin.x + (visible.size.width - size.width) / 2.0,
        visible.origin.y + (visible.size.height - size.height) / 2.0,
    )
}

/// Centres the window in the target screen's visible frame, clamping to fit.
fn place_window(window: &NSWindow, screen: &NSScreen, size: NSSize) {
    let visible = screen.visibleFrame();
    let fitted = fit_size(visible.size, size);
    window.setFrame_display(NSRect::new(centered_origin(visible, fitted), fitted), true);
}

/// Moves the settings window to the main display if its screen disappeared.
pub fn rescue_windows() {
    if crate::screens::is_mutating() {
        return;
    }
    let window = UI.with(|ui| ui.borrow().window.clone());
    let Some(window) = window else {
        return;
    };
    if !window.isVisible() {
        return;
    }
    let current = match window.screen() {
        Some(screen) => screen,
        None => {
            if let Some(primary) = all_screens().first().cloned() {
                place_window(&window, &primary, window.frame().size);
            }
            return;
        }
    };
    let alive = all_screens().iter().any(|screen| {
        screen_contains(
            screen.frame(),
            NSPoint::new(
                current.frame().origin.x + 1.0,
                current.frame().origin.y + 1.0,
            ),
        )
    });
    if !alive {
        if let Some(primary) = all_screens().first().cloned() {
            place_window(&window, &primary, window.frame().size);
        }
    }
}

/// Moves an open settings window after the placement preference changes.
pub fn reposition() {
    let choice = crate::with_model(|model| model.config.settings_display);
    if choice == SettingsDisplay::Invocation {
        return;
    }
    let Some(screen) = resolve_screen(choice, None) else {
        return;
    };
    let Some(window) = UI.with(|ui| ui.borrow().window.clone()) else {
        return;
    };
    if !window.isVisible() {
        return;
    }
    place_window(&window, &screen, window.frame().size);
    let name = unsafe { screen.localizedName().to_string() };
    crate::with_model(|model| {
        model.config.settings_last_screen = Some(name);
        model.config.save();
    });
}

/// Keeps the toolbar's selected tab in step with the active page, for the paths that change the
/// page without going through the toolbar (the control socket and the saved page).
fn sync_tabs(page: Page) {
    let toolbar = UI.with(|ui| ui.borrow().toolbar.clone());
    if let Some(toolbar) = toolbar {
        unsafe {
            toolbar.setSelectedItemIdentifier(Some(&NSString::from_str(page.slug())));
        }
    }
}

fn rebuild_page() {
    crate::refresh_builtin_brightness();
    let (host, page, scroll, built) = UI.with(|ui| {
        let ui = ui.borrow();
        (ui.host.clone(), ui.page, ui.scroll.clone(), ui.built_page)
    });
    let Some(host) = host else {
        return;
    };
    if host.window().is_none() {
        return;
    }
    // A rebuild replaces the scroll view, so the new one starts at the top. When the same page is
    // rebuilt after a value change, remember the vertical offset and re-apply it, otherwise a
    // single toggle or slider change snaps the whole page back to the top. Switching sections is a
    // different page, so it should start at the top and the offset is dropped.
    let offset = if built == Some(page) {
        scroll.map(|scroll| unsafe { scroll.contentView().bounds().origin.y })
    } else {
        None
    };
    let model = crate::with_model(|model| model.snapshot());
    build_page(&host, &model, page);
    sync_tabs(page);
    if let Some(offset) = offset {
        restore_scroll_offset(offset);
    }
}

/// Scrolls the freshly built page back to a remembered vertical offset, clamped to its new content
/// height so a shorter page does not scroll into empty space.
fn restore_scroll_offset(offset: f64) {
    if offset <= 0.0 {
        return;
    }
    let scroll = UI.with(|ui| ui.borrow().scroll.clone());
    let Some(scroll) = scroll else {
        return;
    };
    unsafe {
        // Layout has to settle before the document and clip heights are final, or the clamp uses
        // the old content size and the offset lands short.
        scroll.layoutSubtreeIfNeeded();
        let clip = scroll.contentView();
        let Some(doc) = scroll.documentView() else {
            return;
        };
        let max = (doc.bounds().size.height - clip.bounds().size.height).max(0.0);
        let target = offset.min(max);
        if target <= 0.0 {
            return;
        }
        let point = NSPoint::new(clip.bounds().origin.x, target);
        clip.scrollToPoint(point);
        scroll.reflectScrolledClipView(&clip);
    }
}

/// Whether `window` is the live settings window (not a preview or other app window). Compares by
/// pointer identity against the window held in `UI`.
fn is_settings_window(window: &NSWindow) -> bool {
    UI.with(|ui| {
        ui.borrow()
            .window
            .as_ref()
            .map(|settings| std::ptr::eq(&**settings as *const NSWindow, window as *const NSWindow))
            .unwrap_or(false)
    })
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

fn create_window(width: f64, height: f64) -> Retained<NSWindow> {
    unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm().alloc(),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(
                    width.max(MIN_WIDTH).min(1440.0),
                    height.max(MIN_HEIGHT).min(1200.0),
                ),
            ),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::NSBackingStoreBuffered,
            false,
        );
        window.setTitle(&NSString::from_str("Lumen Settings"));
        window.setToolbarStyle(NSWindowToolbarStyle::Preference);
        window.setReleasedWhenClosed(false);
        window.setContentMinSize(NSSize::new(MIN_WIDTH, MIN_HEIGHT));
        window.setCollectionBehavior(NSWindowCollectionBehavior::MoveToActiveSpace);
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


/// Presents an alert as a sheet on the settings window. The completion runs on the main thread
/// when the user answers; no modal loop or window moving is involved.
fn present_alert_index<F>(alert: Retained<NSAlert>, completion: F)
where
    F: FnOnce(isize) + 'static,
{
    let Some(settings) = UI.with(|ui| ui.borrow().window.clone()) else {
        return;
    };
    let completion = std::rc::Rc::new(std::cell::RefCell::new(Some(completion)));
    let keep = alert.clone();
    let handler = block2::RcBlock::new(move |response: isize| {
        // Keeping the alert alive keeps its accessory view and controls alive too.
        let _alert = &keep;
        let callback = completion.borrow_mut().take();
        if let Some(callback) = callback {
            callback(response);
        }
    });
    unsafe {
        alert.beginSheetModalForWindow_completionHandler(&settings, Some(&handler));
    }
}

fn present_alert<F>(alert: Retained<NSAlert>, completion: F)
where
    F: FnOnce(bool) + 'static,
{
    present_alert_index(alert, move |response| {
        completion(response == NSAlertFirstButtonReturn);
    });
}

fn confirm<F>(message: &str, detail: &str, action: &str, completion: F)
where
    F: FnOnce(bool) + 'static,
{
    unsafe {
        let alert = NSAlert::new(mtm());
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(detail));
        alert.addButtonWithTitle(&NSString::from_str(action));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        present_alert(alert, completion);
    }
}

/// Asks what the monitor is showing after a write this display could not confirm.
///
/// The answer is the only reliable verification for firmware that keeps its old link alive while
/// showing another input. Cancelling leaves the input unverified rather than marking it failed.
pub fn confirm_switch_result(label: &str, key: String, value: u16, protocol: Protocol) {
    unsafe {
        let alert = NSAlert::new(mtm());
        alert.setMessageText(&NSString::from_str(&format!("Did the monitor switch to {label}?")));
        alert.setInformativeText(&NSString::from_str(
            "Lumen sent the input command, but this monitor does not report which input is active. \
             What is on screen now?",
        ));
        alert.addButtonWithTitle(&NSString::from_str("Yes, it switched"));
        alert.addButtonWithTitle(&NSString::from_str("No, it did not"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        present_alert_index(alert, move |response| {
            let verified = if response == NSAlertFirstButtonReturn {
                Some(true)
            } else if response == NSAlertSecondButtonReturn {
                Some(false)
            } else {
                None
            };
            if let Some(verified) = verified {
                crate::with_model(|model| {
                    model.config.mark_input(&key, value, protocol, verified);
                });
                crate::after_change();
            }
        });
    }
}

fn message(text: &str) {
    crate::with_model(|model| model.message = Some(text.to_string()));
}

declare_class!(
    struct SourceList;

    unsafe impl ClassType for SourceList {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "LumenSourceList";
    }

    impl DeclaredClass for SourceList {
        type Ivars = ();
    }

    unsafe impl NSObjectProtocol for SourceList {}

    unsafe impl NSControlTextEditingDelegate for SourceList {}

    unsafe impl NSTableViewDataSource for SourceList {
        #[method(numberOfRowsInTableView:)]
        fn number_of_rows(&self, table: &NSTableView) -> isize {
            let display_list = UI.with(|ui| ui.borrow().display_list.clone());
            if display_list
                .as_ref()
                .map(|list| std::ptr::eq(&**list as *const NSTableView, table))
                .unwrap_or(false)
            {
                crate::with_model(|model| model.displays.len() as isize)
            } else {
                0
            }
        }
    }

    unsafe impl NSTableViewDelegate for SourceList {
        #[method_id(tableView:viewForTableColumn:row:)]
        fn cell_view(
            &self,
            table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>> {
            let display_list = UI.with(|ui| ui.borrow().display_list.clone());
            let text = if display_list
                .as_ref()
                .map(|list| std::ptr::eq(&**list as *const NSTableView, table))
                .unwrap_or(false)
            {
                crate::with_model(|model| {
                    model
                        .displays
                        .get(row as usize)
                        .map(|display| display.name.clone())
                        .unwrap_or_default()
                })
            } else {
                String::new()
            };
            unsafe {
                let cell = NSView::initWithFrame(
                    mtm().alloc(),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(table.frame().size.width, table.rowHeight())),
                );
                let label = label(
                    &text,
                    13.0,
                    NSFontWeightRegular,
                    &NSColor::labelColor(),
                    NSRect::new(NSPoint::new(12.0, 6.0), NSSize::new(table.frame().size.width - 24.0, 18.0)),
                );
                label.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewWidthSizable);
                cell.addSubview(&label);
                Some(cell)
            }
        }

        #[method(tableViewSelectionDidChange:)]
        fn selection_changed(&self, notification: &NSNotification) {
            if UI.with(|ui| ui.borrow().suppress_selection) {
                return;
            }
            let Some(object) = (unsafe { notification.object() }) else {
                return;
            };
            let table = unsafe { &*(&*object as *const AnyObject as *const NSTableView) };
            let display_list = UI.with(|ui| ui.borrow().display_list.clone());
            if display_list
                .as_ref()
                .map(|list| std::ptr::eq(&**list as *const NSTableView, table))
                .unwrap_or(false)
            {
                let row = unsafe { table.selectedRow() };
                if row < 0 {
                    return;
                }
                if UI.with(|ui| ui.borrow().selected) == row as usize {
                    return;
                }
                UI.with(|ui| ui.borrow_mut().selected = row as usize);
                rebuild_page();
            }
        }
    }
);

fn source_list() -> Retained<SourceList> {
    SOURCE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let object = mtm().alloc::<SourceList>().set_ivars(());
            *slot = Some(unsafe { msg_send_id![super(object), init] });
        }
        slot.clone().expect("source list")
    })
}

// The titlebar toolbar's delegate. It serves one selectable item per page, each a page slug
// identifier with an SF Symbol above the short label. AppKit draws the preference-style selection
// highlight for whichever item is selected.
declare_class!(
    struct SettingsToolbar;

    unsafe impl ClassType for SettingsToolbar {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "LumenSettingsToolbar";
    }

    impl DeclaredClass for SettingsToolbar {
        type Ivars = ();
    }

    unsafe impl NSObjectProtocol for SettingsToolbar {}

    unsafe impl NSToolbarDelegate for SettingsToolbar {
        #[method_id(toolbarDefaultItemIdentifiers:)]
        fn default_items(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            toolbar_item_identifiers()
        }

        #[method_id(toolbarAllowedItemIdentifiers:)]
        fn allowed_items(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            toolbar_item_identifiers()
        }

        #[method_id(toolbarSelectableItemIdentifiers:)]
        fn selectable_items(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            toolbar_item_identifiers()
        }

        #[method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:)]
        fn item_for_identifier(
            &self,
            _toolbar: &NSToolbar,
            identifier: &NSToolbarItemIdentifier,
            _inserted: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            make_toolbar_item(identifier)
        }
    }
);

/// Builds the toolbar item for a page slug identifier: an SF Symbol above the short label, wired to
/// `selectPage:` on the Actions target.
fn make_toolbar_item(identifier: &NSToolbarItemIdentifier) -> Option<Retained<NSToolbarItem>> {
    let slug = identifier.to_string();
    let page = Page::from_slug(&slug)?;
    unsafe {
        let item = NSToolbarItem::initWithItemIdentifier(mtm().alloc(), identifier);
        item.setLabel(&NSString::from_str(page.tab_label()));
        item.setPaletteLabel(&NSString::from_str(page.tab_label()));
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(page.symbol()),
            None,
        ) {
            // Template rendering lets AppKit tint the glyph with the selection highlight.
            image.setTemplate(true);
            item.setImage(Some(&image));
        }
        if let Some(target) = target() {
            item.setTarget(Some(&target));
            item.setAction(Some(sel!(selectPage:)));
        }
        Some(item)
    }
}

/// The four toolbar item identifiers (page slugs), in `Page::ALL` order.
fn toolbar_item_identifiers() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let ids: Vec<Retained<NSString>> =
        Page::ALL.iter().map(|page| NSString::from_str(page.slug())).collect();
    NSArray::from_vec(ids)
}

fn settings_toolbar() -> Retained<SettingsToolbar> {
    TOOLBAR_DELEGATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let object = mtm().alloc::<SettingsToolbar>().set_ivars(());
            *slot = Some(unsafe { msg_send_id![super(object), init] });
        }
        slot.clone().expect("settings toolbar")
    })
}

fn make_table(frame: NSRect, row_height: f64, column_width: f64) -> Retained<NSTableView> {
    unsafe {
        let table = NSTableView::initWithFrame(mtm().alloc(), frame);
        table.setStyle(NSTableViewStyle::SourceList);
        table.setRowHeight(row_height);
        table.setHeaderView(None);
        table.setAllowsEmptySelection(false);
        table.setBackgroundColor(&NSColor::clearColor());
        let column = NSTableColumn::initWithIdentifier(mtm().alloc(), &NSString::from_str("value"));
        column.setWidth(column_width);
        table.addTableColumn(&column);
        let source = source_list();
        table.setDataSource(Some(ProtocolObject::from_ref(&*source)));
        table.setDelegate(Some(ProtocolObject::from_ref(&*source)));
        table
    }
}

fn make_source_scroll(
    frame: NSRect,
    row_height: f64,
    column_width: f64,
    scroller: bool,
) -> (Retained<NSScrollView>, Retained<NSTableView>) {
    unsafe {
        let scroll = NSScrollView::initWithFrame(mtm().alloc(), frame);
        scroll.setDrawsBackground(false);
        scroll.setBorderType(objc2_app_kit::NSBorderType::NSNoBorder);
        scroll.setHasVerticalScroller(scroller);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::NSViewWidthSizable
                | NSAutoresizingMaskOptions::NSViewHeightSizable,
        );
        let table = make_table(
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(frame.size.width, row_height),
            ),
            row_height,
            column_width,
        );
        table.setAutoresizingMask(NSAutoresizingMaskOptions::NSViewWidthSizable);
        scroll.setDocumentView(Some(&table));
        (scroll, table)
    }
}

/// Reloads a source list and selects a row. `reloadData` makes AppKit auto-select row 0, which
/// posts a selection notification; without the suppression that notification rebuilds the page
/// from inside the build and leaves two page views stacked.
fn reload_and_select(table: &NSTableView, row: usize) {
    UI.with(|ui| ui.borrow_mut().suppress_selection = true);
    unsafe {
        table.reloadData();
    }
    UI.with(|ui| ui.borrow_mut().suppress_selection = false);
    select_row(table, row);
}

fn select_row(table: &NSTableView, row: usize) {
    UI.with(|ui| ui.borrow_mut().suppress_selection = true);
    unsafe {
        table.selectRowIndexes_byExtendingSelection(
            objc2_foundation::NSIndexSet::indexSetWithIndex(row).as_ref(),
            false,
        );
    }
    UI.with(|ui| ui.borrow_mut().suppress_selection = false);
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
            crate::preview::tick();
        }

        #[method(thumbnailTick:)]
        fn thumbnail_tick(&self, _timer: *mut AnyObject) {
            crate::preview::thumbnail_tick();
        }

        #[method(previewClose:)]
        fn preview_close(&self, sender: *mut AnyObject) {
            let window: *mut NSWindow = unsafe { msg_send![sender, window] };
            if let Some(window) = unsafe { window.as_ref() } {
                crate::preview::close_window(window);
            }
        }

        #[method(hudDismiss:)]
        fn hud_dismiss(&self, _timer: *mut AnyObject) {
            crate::hud::dismiss();
        }

        #[method(previewPause:)]
        fn preview_pause(&self, sender: *mut AnyObject) {
            let window: *mut NSWindow = unsafe { msg_send![sender, window] };
            if let Some(window) = unsafe { window.as_ref() } {
                crate::preview::toggle_paused(window);
            }
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
                let old = screen.name.clone();
                let alert = format!("Rename {} to {}?", screen.name, name);
                confirm(
                    &alert,
                    "The display is recreated. Any agent browser on it closes, and its position on the desktop may change.",
                    "Rename",
                    move |confirmed| {
                        if !confirmed {
                            return;
                        }
                        match crate::rename_screen(&old, &config) {
                            Ok(()) => {}
                            Err(error) => message(&error),
                        }
                        crate::after_change();
                    },
                );
                return;
            }
            match crate::screens::ensure(&config) {
                Ok(_) => {
                    crate::with_model(|model| {
                        model.config.upsert_virtual_screen(config);
                        model.message = Some("Virtual display updated".to_string());
                    });
                }
                Err(error) => message(&error),
            }
            crate::after_change();
        }

        #[method(screenPreview:)]
        fn screen_preview(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender) as usize;
            let Some(screen) = crate::screens::list().get(index).cloned() else {
                return;
            };
            if let Err(error) = crate::preview::toggle(&screen.name) {
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
            let name = screen.name.clone();
            let alert = format!("Remove {}?", screen.name);
            confirm(
                &alert,
                "This closes any agent browser on the display and deletes its definition. Captures stay on disk.",
                "Remove",
                move |confirmed| {
                    if confirmed {
                        crate::remove_screen(&name);
                    }
                },
            );
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
                let accessory = NSView::initWithFrame(mtm().alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 58.0)));
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

            let alert = unsafe {
                let alert = NSAlert::new(mtm());
                alert.setMessageText(&NSString::from_str("New Virtual Display"));
                alert.setInformativeText(&NSString::from_str(
                    "Names allow letters, digits, spaces and - _ . '",
                ));
                alert.addButtonWithTitle(&NSString::from_str("Create"));
                alert.addButtonWithTitle(&NSString::from_str("Cancel"));
                alert.setAccessoryView(Some(&accessory));
                alert
            };
            present_alert(alert, move |confirmed| {
                if !confirmed {
                    return;
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
                let (width, height) = presets
                    [unsafe { size_popup.indexOfSelectedItem() }.max(0) as usize % presets.len()];
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
            });
        }

        #[method(noop:)]
        fn noop(&self, _sender: *mut AnyObject) {}

        #[method(selectPage:)]
        fn select_page(&self, _sender: *mut AnyObject) {
            // The native toolbar has already moved its selection; resolve the page from the
            // selected item identifier (its slug) rather than from a control tag.
            let toolbar = UI.with(|ui| ui.borrow().toolbar.clone());
            let Some(toolbar) = toolbar else {
                return;
            };
            let slug = unsafe { toolbar.selectedItemIdentifier() };
            let Some(slug) = slug else {
                return;
            };
            let Some(page) = Page::from_slug(&slug.to_string()) else {
                return;
            };
            if UI.with(|ui| ui.borrow().page) == page {
                return;
            }
            UI.with(|ui| ui.borrow_mut().page = page);
            crate::with_model(|model| {
                model.config.settings_page = page.slug().to_string();
                model.config.save();
            });
            rebuild_page();
        }

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

            let code = lumen::ddc::InputCode {
                label: format!("{} ({})", input.label, input.protocol.label()),
                value: input.value,
                protocol: input.protocol.to_ddc(),
            };
            let alert = format!("Test {} on {}?", input.label, name);
            confirm(
                &alert,
                "Lumen sends a DDC input-select command. Some monitors cannot switch back over DDC, so keep the monitor's own controls nearby. If the monitor does not report the new input, Lumen will ask you what you see.",
                "Test",
                move |confirmed| {
                    if confirmed {
                        crate::switch_input(display_id, code, Some(key), true);
                    }
                },
            );
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
            rescue_windows();
            dispatch::Queue::main()
                .exec_after(std::time::Duration::from_millis(500), rescue_windows);
            crate::schedule_rescan();
        }

        #[method(didWake:)]
        fn did_wake(&self, _notification: *mut AnyObject) {
            crate::hotkeys::wake();
            crate::schedule_rescan();
        }

        #[method(sessionActive:)]
        fn session_active(&self, _notification: *mut AnyObject) {
            crate::hotkeys::wake();
        }

        #[method(sessionResign:)]
        fn session_resign(&self, _notification: *mut AnyObject) {
            crate::hotkeys::suspend();
        }

        #[method(accessibilityChanged:)]
        fn accessibility_changed(&self, _notification: *mut AnyObject) {
            crate::hotkeys::schedule_refresh();
        }

        #[method(retryBrightnessKeys:)]
        fn retry_brightness_keys(&self, _sender: *mut AnyObject) {
            crate::hotkeys::retry();
            crate::after_change();
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
            crate::log::trace(format!(
                "login item: before={before:?} result={result:?} after={:?}",
                crate::login::status()
            ));
            UI.with(|ui| {
                ui.borrow_mut().login_note = result.err();
            });
            crate::after_change();
        }

        #[method(toggleBrightnessKeys:)]
        fn toggle_brightness_keys(&self, _sender: *mut AnyObject) {
            let turning_on = !crate::with_model(|model| model.config.brightness_keys_enabled);
            crate::with_model(|model| {
                model.config.brightness_keys_enabled = turning_on;
                model.config.save();
            });
            crate::hotkeys::refresh();
            crate::after_change();
        }

        #[method(brightnessStep:)]
        fn brightness_step(&self, sender: *mut AnyObject) {
            let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            let step = match index {
                0 => crate::config::BrightnessStep::Coarse,
                2 => crate::config::BrightnessStep::Fine,
                _ => crate::config::BrightnessStep::Normal,
            };
            crate::with_model(|model| {
                model.config.brightness_step = step;
                model.config.save();
            });
            crate::after_change();
        }

        #[method(hudStyle:)]
        fn hud_style(&self, sender: *mut AnyObject) {
            let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            let style = match index {
                1 => crate::config::HudStyle::Classic,
                _ => crate::config::HudStyle::Tahoe,
            };
            crate::with_model(|model| {
                model.config.hud_style = style;
                model.config.save();
            });
            // Preview the chosen style so the change is visible immediately.
            let (value, max, controllable) = crate::with_model(|model| {
                model
                    .displays
                    .iter()
                    .find(|display| display.builtin)
                    .or_else(|| model.displays.first())
                    .and_then(|display| display.brightness.map(|(v, m)| (v, m, true)))
                    .unwrap_or((50, 100, true))
            });
            crate::hud::show(value, max, controllable);
            crate::after_change();
        }

        #[method(settingsDisplay:)]
        fn settings_display(&self, sender: *mut AnyObject) {
            let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            let choice = match index {
                1 => SettingsDisplay::Agent,
                2 => SettingsDisplay::Main,
                3 => SettingsDisplay::Last,
                _ => SettingsDisplay::Invocation,
            };
            crate::with_model(|model| {
                model.config.settings_display = choice;
                model.config.save();
            });
            reposition();
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

        #[method(agentStart:)]
        fn agent_start(&self, sender: *mut AnyObject) {
            let headless = sender_tag(sender) == 0;
            let name = UI.with(|ui| {
                ui.borrow()
                    .agent_picker
                    .as_ref()
                    .and_then(|picker| unsafe { picker.titleOfSelectedItem() })
                    .map(|title| title.to_string())
            });
            let Some(name) = name else {
                return;
            };
            UI.with(|ui| {
                ui.borrow_mut().agent_note = Some(format!("Starting {name}…"));
            });
            crate::after_change();
            std::thread::spawn(move || {
                let request = serde_json::json!({
                    "cmd": "agent.start",
                    "name": name,
                    "headless": headless,
                });
                let reply = crate::control::agent_start(&request);
                let value: serde_json::Value =
                    serde_json::from_str(&reply).unwrap_or(serde_json::Value::Null);
                let ok = value
                    .get("ok")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let message = if ok {
                    format!("Agent started on {name}")
                } else {
                    value
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("agent could not start")
                        .to_string()
                };
                crate::on_main(move || {
                    UI.with(|ui| {
                        ui.borrow_mut().agent_note = Some(message);
                    });
                    crate::after_change();
                });
            });
        }

        #[method(agentStop:)]
        fn agent_stop(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender).max(0) as usize;
            let name = UI.with(|ui| {
                ui.borrow().agent_session_names.get(index).cloned()
            });
            let Some(name) = name else {
                return;
            };
            let _ = crate::agent::stop(&name);
            crate::agent::stop_profile(&name);
            crate::with_model(|model| {
                model.config.forget_agent_session(&name);
                model.message = Some(format!("Stopped the agent on {name}"));
            });
            UI.with(|ui| {
                ui.borrow_mut().agent_note = Some(format!("Stopped {name}"));
            });
            crate::after_change();
        }

        #[method(agentShot:)]
        fn agent_shot(&self, sender: *mut AnyObject) {
            let index = sender_tag(sender).max(0) as usize;
            let name = UI.with(|ui| {
                ui.borrow().agent_session_names.get(index).cloned()
            });
            let Some(name) = name else {
                return;
            };
            let Some(session) = crate::agent::session(&name) else {
                return;
            };
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0);
            let path = crate::config::Config::support_dir()
                .join("agents")
                .join(&name)
                .join(format!("page-{stamp}.png"));
            let limit = crate::with_model(|model| model.config.capture_limit);
            UI.with(|ui| {
                ui.borrow_mut().agent_note = Some(format!("Capturing {name}…"));
            });
            crate::after_change();
            std::thread::spawn(move || {
                let result = crate::cdp::screenshot(session.port, &path);
                if result.is_ok() {
                    crate::screens::prune_captures(path.parent(), limit);
                }
                let message = match result {
                    Ok(()) => format!("Saved {}", path.display()),
                    Err(error) => error,
                };
                crate::on_main(move || {
                    UI.with(|ui| {
                        ui.borrow_mut().agent_note = Some(message);
                    });
                    crate::after_change();
                });
            });
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

        #[method(checkUpdates:)]
        fn check_updates(&self, _sender: *mut AnyObject) {
            crate::updater::check_now();
        }

        #[method(deleteCaptures:)]
        fn delete_captures(&self, _sender: *mut AnyObject) {
            confirm(
                "Delete captures?",
                "Removes page and screen screenshots from every folder. Browser profiles are not touched.",
                "Delete",
                move |confirmed| {
                    if !confirmed {
                        return;
                    }
                    let removed = crate::screens::delete_captures();
                    message(&format!("Deleted {removed} capture(s)"));
                    crate::after_change();
                },
            );
        }

        #[method(copyDiagnostics:)]
        fn copy_diagnostics(&self, _sender: *mut AnyObject) {
            let os = std::process::Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let log = crate::log::snapshot();
            let text = crate::with_model(|model| {
                format!(
                    "Lumen {}\nmacOS {}\nDisplays: {}\nVirtual displays: {}\nAgent sessions: {}\nLogin item: {:?}\nConfig: {}\nCaptures: {}\n\nLog:\n{}",
                    crate::version_string(),
                    os,
                    model.displays.len(),
                    crate::screens::list().len(),
                    crate::agent::sessions().len(),
                    crate::login::status(),
                    crate::config::Config::path().display(),
                    crate::screens::captures_dir().display(),
                    log
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
        // Wrapping text (`paragraph_row`) and the Displays detail pane freeze their layout width at
        // build time via `setPreferredMaxLayoutWidth`, so a resized window leaves that text clipped
        // or short until the page is rebuilt. Rebuild on resize to reflow it. During a live drag,
        // skip the per-frame rebuild (Auto Layout already stretches the host, scroll view and
        // frames live) and do a single rebuild when the drag ends, so dragging stays smooth.
        #[method(windowDidResize:)]
        fn window_did_resize(&self, notification: &NSNotification) {
            let Some(object) = (unsafe { notification.object() }) else {
                return;
            };
            let resized = unsafe { &*(&*object as *const AnyObject as *const NSWindow) };
            if !is_settings_window(resized) || unsafe { resized.inLiveResize() } {
                return;
            }
            rebuild_if_open();
        }

        #[method(windowDidEndLiveResize:)]
        fn window_did_end_live_resize(&self, notification: &NSNotification) {
            let Some(object) = (unsafe { notification.object() }) else {
                return;
            };
            let resized = unsafe { &*(&*object as *const AnyObject as *const NSWindow) };
            if !is_settings_window(resized) {
                return;
            }
            rebuild_if_open();
        }

        #[method(windowDidChangeOcclusionState:)]
        fn window_did_change_occlusion_state(&self, notification: &NSNotification) {
            if let Some(object) = unsafe { notification.object() } {
                let window = unsafe { &*(&*object as *const AnyObject as *const NSWindow) };
                crate::preview::occlusion_changed(window);
            }
        }

        #[method(windowWillClose:)]
        fn window_will_close(&self, notification: &NSNotification) {
            let Some(object) = (unsafe { notification.object() }) else {
                return;
            };
            let closing = unsafe { &*(&*object as *const AnyObject as *const NSWindow) };
            if crate::preview::name_for_window(closing).is_some() {
                // The preview owns its stream; close it without touching the settings window.
                crate::preview::window_closed(closing);
                return;
            }
            if !is_settings_window(closing) {
                // Some other window this app owns closed; it is not the settings window.
                return;
            }
            crate::preview::set_thumbnails_visible(false);
            let window = UI.with(|ui| ui.borrow_mut().window.take());
            if let Some(window) = &window {
                if let Some(content) = window.contentView() {
                    let size = content.frame().size;
                    crate::with_model(|model| {
                        model.config.settings_width = size.width.max(MIN_WIDTH);
                        model.config.settings_height = size.height.max(MIN_HEIGHT);
                        model.config.save();
                    });
                }
            }
            STALE.with(|stale| {
                *stale.borrow_mut() = window;
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

#[cfg(test)]
mod placement_tests {
    use super::{centered_origin, fit_size};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    #[test]
    fn fits_and_centres_in_a_normal_screen() {
        let visible = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1920.0, 1049.0));
        let size = fit_size(visible.size, NSSize::new(920.0, 880.0));
        assert_eq!(size.width, 920.0);
        assert_eq!(size.height, 880.0);
        let origin = centered_origin(visible, size);
        assert_eq!(origin.x, 500.0);
        assert_eq!(origin.y, 84.5);
    }

    #[test]
    fn clamps_to_a_small_screen() {
        let visible = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(700.0, 600.0));
        let size = fit_size(visible.size, NSSize::new(1000.0, 800.0));
        assert_eq!(size.width, 676.0);
        assert_eq!(size.height, 576.0);
        let origin = centered_origin(visible, size);
        assert_eq!(origin.x, 12.0);
        assert_eq!(origin.y, 12.0);
    }

    #[test]
    fn honours_the_origin_of_a_secondary_screen() {
        let visible = NSRect::new(NSPoint::new(1680.0, -30.0), NSSize::new(1920.0, 1080.0));
        let size = fit_size(visible.size, NSSize::new(920.0, 880.0));
        let origin = centered_origin(visible, size);
        assert_eq!(origin.x, 2180.0);
        assert_eq!(origin.y, 70.0);
    }
}
