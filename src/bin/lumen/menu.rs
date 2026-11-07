//! The native menu, built from the model so it always matches the settings.

use crate::config::Protocol;
use crate::engine::DisplayEntry;
use crate::model::Model;
use lumen::ddc::InputProtocol;
use muda::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};

pub const REFRESH: &str = "refresh";
pub const QUIT: &str = "quit";
pub const SETTINGS: &str = "settings";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Quit,
    Settings,
    Switch { display: u32, value: u16, protocol: InputProtocol },
    Brightness { display: u32, value: u16 },
    Release { key: String },
    Reclaim { key: String },
}

/// Builds the whole menu.
pub fn build(model: &Model) -> Menu {
    let menu = Menu::new();

    let headline = match &model.message {
        Some(message) => message.clone(),
        None if model.displays.is_empty() && model.released.is_empty() => {
            "No DDC displays connected".to_string()
        }
        None => "Lumen".to_string(),
    };
    let _ = menu.append(&MenuItem::new(headline, false, None));
    let _ = menu.append(&PredefinedMenuItem::separator());

    for released in &model.released {
        let _ = menu.append(&MenuItem::new(
            format!("{} is on another input", released.name),
            false,
            None,
        ));
        let _ = menu.append(&MenuItem::with_id(
            reclaim_id(released.key.clone()),
            format!("Bring {} back", released.name),
            true,
            None,
        ));
    }

    if !model.released.is_empty() && model.menu_displays().next().is_some() {
        let _ = menu.append(&PredefinedMenuItem::separator());
    }

    for display in model.menu_displays() {
        let _ = menu.append(&display_menu(model, display));
    }

    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id(SETTINGS, "Settings…", true, None));
    let _ = menu.append(&MenuItem::with_id(REFRESH, "Refresh", true, None));
    let _ = menu.append(&MenuItem::with_id(QUIT, "Quit Lumen", true, None));
    menu
}

fn display_menu(model: &Model, display: &DisplayEntry) -> Submenu {
    let submenu = Submenu::new(display.name.clone(), true);

    if lumen::native::is_supported() {
        let _ = submenu.append(&MenuItem::with_id(
            release_id(display.key.clone()),
            "Release to other input (auto-switch)",
            true,
            None,
        ));
        let _ = submenu.append(&PredefinedMenuItem::separator());
    }

    let inputs = model.config.enabled_inputs(&display.key);
    if inputs.is_empty() {
        let _ = submenu.append(&MenuItem::new("No inputs enabled", false, None));
    } else {
        let _ = submenu.append(&MenuItem::new("Switch input (DDC, if supported)", false, None));
        for input in inputs {
            let checked = false;
            let item = CheckMenuItem::with_id(
                switch_id(display.id, input.value, input.protocol),
                format!("{} ({})", input.label, input.protocol.label()),
                true,
                checked,
                None,
            );
            let _ = submenu.append(&item);
        }
    }

    if model.config.show_brightness {
        if let Some((value, max)) = display.brightness {
            let _ = submenu.append(&PredefinedMenuItem::separator());
            let _ = submenu.append(&MenuItem::new(
                format!("Brightness {}", percent(value, max)),
                false,
                None,
            ));
            for (label, target) in [("25%", 25u16), ("50%", 50), ("75%", 75), ("100%", 100)] {
                let value = max.saturating_mul(target) / 100;
                let item = MenuItem::with_id(brightness_id(display.id, value), label, true, None);
                let _ = submenu.append(&item);
            }
        }
    }

    if let Some((value, max)) = display.contrast {
        let _ = submenu.append(&MenuItem::new(
            format!("Contrast {}", percent(value, max)),
            false,
            None,
        ));
    }
    if let Some((value, max)) = display.volume {
        let _ = submenu.append(&MenuItem::new(
            format!("Volume {}", percent(value, max)),
            false,
            None,
        ));
    }

    submenu
}

/// Turns a menu id back into the action it stands for.
pub fn parse(id: &MenuId) -> Option<Action> {
    let raw = id.as_ref();
    match raw {
        REFRESH => return Some(Action::Refresh),
        QUIT => return Some(Action::Quit),
        SETTINGS => return Some(Action::Settings),
        _ => {}
    }
    if let Some(key) = raw.strip_prefix("release|") {
        return Some(Action::Release { key: key.to_string() });
    }
    if let Some(key) = raw.strip_prefix("reclaim|") {
        return Some(Action::Reclaim { key: key.to_string() });
    }
    let mut parts = raw.split(':');
    match parts.next()? {
        "switch" => {
            let display = parts.next()?.parse().ok()?;
            let value = parts.next()?.parse().ok()?;
            let protocol = match parts.next()? {
                "lg" => InputProtocol::LgAlt,
                _ => InputProtocol::Standard,
            };
            Some(Action::Switch { display, value, protocol })
        }
        "brightness" => {
            let display = parts.next()?.parse().ok()?;
            let value = parts.next()?.parse().ok()?;
            Some(Action::Brightness { display, value })
        }
        _ => None,
    }
}

fn switch_id(display: u32, value: u16, protocol: Protocol) -> String {
    let protocol = match protocol {
        Protocol::Standard => "std",
        Protocol::LgAlt => "lg",
    };
    format!("switch:{display}:{value}:{protocol}")
}

fn brightness_id(display: u32, value: u16) -> String {
    format!("brightness:{display}:{value}")
}

fn release_id(key: String) -> String {
    format!("release|{key}")
}

fn reclaim_id(key: String) -> String {
    format!("reclaim|{key}")
}

fn percent(value: u16, max: u16) -> String {
    if max == 0 {
        return String::new();
    }
    format!("{}%", (value as u32 * 100 / max as u32))
}
