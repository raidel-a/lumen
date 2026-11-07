//! The native status item and its menu.
//!
//! The menu is rebuilt from the model every time it opens, and its control rows are custom item
//! views holding real `NSSlider`s, so brightness, contrast and volume can be dragged right in the
//! menu bar instead of picked from fixed percentages.

use crate::engine::DisplayEntry;
use crate::menu::{self, Action};
use crate::model::Model;
use crate::ui::Actions;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{msg_send, sel};
use objc2_app_kit::{
    NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontWeightMedium,
    NSFontWeightRegular, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSTickMarkPosition,
    NSStatusItem, NSSlider, NSTextField, NSVariableStatusItemLength, NSView,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
    static MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    /// Live value labels of the menu sliders, keyed by `(display id, VCP code)`.
    static LABELS: RefCell<HashMap<(u32, u8), Retained<NSTextField>>> =
        RefCell::new(HashMap::new());
}

fn mtm() -> MainThreadMarker {
    unsafe { MainThreadMarker::new_unchecked() }
}

pub fn init(actions: &Actions) {
    unsafe {
        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm()) {
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("display.and.screwdriver"),
                None,
            ) {
                image.setTemplate(true);
                button.setImage(Some(&image));
            }
            button.setToolTip(Some(&NSString::from_str("Lumen - display control")));
        }

        let menu = NSMenu::new(mtm());
        menu.setAutoenablesItems(false);
        menu.setDelegate(Some(ProtocolObject::from_ref(actions)));
        item.setMenu(Some(&menu));

        ITEM.with(|slot| *slot.borrow_mut() = Some(item));
        MENU.with(|slot| *slot.borrow_mut() = Some(menu));
    }
}

fn label(text: &str, size: f64, weight: f64, color: &NSColor, rect: NSRect) -> Retained<NSTextField> {
    unsafe {
        let field = NSTextField::labelWithString(&NSString::from_str(text), mtm());
        field.setFrame(rect);
        field.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
        field.setTextColor(Some(color));
        field
    }
}

fn plain_item(title: &str, enabled: bool) -> Retained<NSMenuItem> {
    unsafe {
        let item = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm().alloc(),
            &NSString::from_str(title),
            None,
            &NSString::from_str(""),
        );
        item.setEnabled(enabled);
        item
    }
}

fn action_item(title: &str, id: &str, checked: bool, enabled: bool, actions: &Actions) -> Retained<NSMenuItem> {
    unsafe {
        let item = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm().alloc(),
            &NSString::from_str(title),
            Some(sel!(menuAction:)),
            &NSString::from_str(""),
        );
        item.setTarget(Some(actions));
        item.setRepresentedObject(Some(&NSString::from_str(id)));
        item.setState(if checked { NSControlStateValueOn } else { NSControlStateValueOff });
        item.setEnabled(enabled);
        item
    }
}

fn separator() -> Retained<NSMenuItem> {
    NSMenuItem::separatorItem(mtm())
}

fn percent(value: u16, max: u16) -> u32 {
    if max == 0 {
        return 0;
    }
    value as u32 * 100 / max as u32
}

/// A menu item whose view is a label, a live slider and a percentage readout.
fn slider_item(
    display: &DisplayEntry,
    code: u8,
    title: &str,
    value: u16,
    max: u16,
    actions: &Actions,
) -> Retained<NSMenuItem> {
    unsafe {
        let container = NSView::initWithFrame(
            mtm().alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, 32.0)),
        );
        let text = NSColor::labelColor();
        let secondary = NSColor::secondaryLabelColor();

        container.addSubview(&label(
            title,
            12.0,
            NSFontWeightMedium,
            &text,
            NSRect::new(NSPoint::new(12.0, 9.0), NSSize::new(80.0, 16.0)),
        ));

        let slider = NSSlider::initWithFrame(
            mtm().alloc(),
            NSRect::new(NSPoint::new(96.0, 6.0), NSSize::new(150.0, 22.0)),
        );
        slider.setMinValue(0.0);
        slider.setMaxValue(max.max(1) as f64);
        slider.setDoubleValue(value as f64);
        slider.setContinuous(true);
        // Five snap points: 0, 25, 50, 75 and 100 percent of the range.
        slider.setNumberOfTickMarks(5);
        slider.setAllowsTickMarkValuesOnly(true);
        slider.setTickMarkPosition(NSTickMarkPosition::Below);
        slider.setTag(((display.id as isize) << 8) | code as isize);
        slider.setTarget(Some(actions));
        slider.setAction(Some(sel!(menuControlChanged:)));
        container.addSubview(&slider);

        let readout = label(
            &format!("{}%", percent(value, max)),
            11.0,
            NSFontWeightRegular,
            &secondary,
            NSRect::new(NSPoint::new(252.0, 9.0), NSSize::new(44.0, 16.0)),
        );
        container.addSubview(&readout);
        LABELS.with(|labels| {
            labels.borrow_mut().insert((display.id, code), readout);
        });

        let item = NSMenuItem::new(mtm());
        item.setView(Some(&container));
        item
    }
}

pub fn fill(menu: &NSMenu, actions: &Actions) {
    let model: Model = crate::with_model(|model| model.snapshot());
    LABELS.with(|labels| labels.borrow_mut().clear());

    unsafe {
        menu.removeAllItems();

        let headline = match &model.message {
            Some(message) => message.clone(),
            None if model.displays.is_empty() && model.released.is_empty() => {
                "No DDC displays connected".to_string()
            }
            None => "Lumen".to_string(),
        };
        menu.addItem(&plain_item(&headline, false));
        menu.addItem(&separator());

        for released in &model.released {
            menu.addItem(&plain_item(&format!("{} is on another input", released.name), false));
            menu.addItem(&action_item(
                &format!("Bring {} back", released.name),
                &menu::reclaim_id(&released.key),
                false,
                true,
                actions,
            ));
        }
        if !model.released.is_empty() && model.menu_displays().next().is_some() {
            menu.addItem(&separator());
        }

        for display in model.menu_displays() {
            let submenu = NSMenu::new(mtm());
            submenu.setAutoenablesItems(false);
            fill_display(&submenu, &model, display, actions);

            let entry = plain_item(&display.name, true);
            entry.setSubmenu(Some(&submenu));
            menu.addItem(&entry);
        }

        menu.addItem(&separator());
        menu.addItem(&action_item("Settings…", menu::SETTINGS, false, true, actions));
        menu.addItem(&action_item("Refresh", menu::REFRESH, false, true, actions));
        menu.addItem(&action_item("Quit Lumen", menu::QUIT, false, true, actions));
    }
}

fn fill_display(submenu: &NSMenu, model: &Model, display: &DisplayEntry, actions: &Actions) {
        if lumen::native::is_supported() {
            submenu.addItem(&action_item(
                "Release to other input (auto-switch)",
                &menu::release_id(&display.key),
                false,
                true,
                actions,
            ));
            submenu.addItem(&separator());
        }

        let inputs = model.config.enabled_inputs(&display.key);
        if inputs.is_empty() {
            submenu.addItem(&plain_item("No inputs enabled", false));
        } else {
            submenu.addItem(&plain_item("Switch input (DDC, if supported)", false));
            for input in inputs {
                let checked = model.is_current(display, input.value, input.protocol);
                submenu.addItem(&action_item(
                    &format!("{} ({})", input.label, input.protocol.label()),
                    &menu::switch_id(display.id, input.value, input.protocol.to_ddc()),
                    checked,
                    true,
                    actions,
                ));
            }
        }

        // Sliders sit directly in the display menu: a second-level submenu is placed off-screen
        // when the status item lives near the right edge.
        let mut controls: Vec<(u8, &str, u16, u16)> = Vec::new();
        if model.config.show_brightness {
            if let Some((value, max)) = display.brightness {
                controls.push((0x10, "Brightness", value, max));
            }
        }
        if let Some((value, max)) = display.contrast {
            controls.push((0x12, "Contrast", value, max));
        }
        if let Some((value, max)) = display.volume {
            controls.push((0x62, "Volume", value, max));
        }

        if !controls.is_empty() {
            submenu.addItem(&separator());
            submenu.addItem(&plain_item("Controls", false));
            for (code, title, value, max) in &controls {
                submenu.addItem(&slider_item(display, *code, title, *value, *max, actions));
            }
            if let Some(muted) = display.muted {
                submenu.addItem(&action_item(
                    "Mute",
                    &menu::mute_id(display.id, !muted),
                    muted,
                    true,
                    actions,
                ));
            }
        }
}

/// Updates the percentage beside a menu slider while it is dragged.
pub fn set_readout(display_id: u32, code: u8, value: u16) {
    let max = crate::with_model(|model| {
        model.display(display_id).and_then(|display| match code {
            0x10 => display.brightness,
            0x12 => display.contrast,
            _ => display.volume,
        })
    })
    .map(|(_, max)| max)
    .unwrap_or(100);
    LABELS.with(|labels| {
        if let Some(field) = labels.borrow().get(&(display_id, code)) {
            unsafe {
                field.setStringValue(&NSString::from_str(&format!(
                    "{}%",
                    percent(value.min(max), max)
                )))
            };
        }
    });
}

/// The action a menu item stands for, from its represented object.
pub fn item_action(item: &objc2::runtime::AnyObject) -> Option<Action> {
    unsafe {
        let object: *mut objc2::runtime::AnyObject = msg_send![item, representedObject];
        if object.is_null() {
            return None;
        }
        let text: *mut objc2::runtime::AnyObject = object;
        let c: *const std::ffi::c_char = msg_send![text, UTF8String];
        if c.is_null() {
            return None;
        }
        let id = std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned();
        menu::parse(&id)
    }
}
