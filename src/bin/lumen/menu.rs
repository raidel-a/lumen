//! Menu construction for the status item.

use crate::state::DisplayEntry;
use lumen::ddc::{InputCode, InputProtocol};
use muda::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};

pub const REFRESH: &str = "refresh";
pub const QUIT: &str = "quit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Quit,
    Switch { display: u32, value: u16, protocol: InputProtocol },
    Brightness { display: u32, value: u16 },
    Release { display: u32 },
    Reclaim { display: u32 },
}

/// A display whose output this app parked so the monitor could switch away.
#[derive(Debug, Clone)]
pub struct Released {
    pub id: u32,
    pub name: String,
}

pub struct MenuState<'a> {
    pub displays: &'a [DisplayEntry],
    pub released: &'a [Released],
    pub message: Option<&'a str>,
}

/// Builds the whole menu from the current state.
pub fn build(state: &MenuState) -> Menu {
    let menu = Menu::new();

    let headline = match state.message {
        Some(message) => message.to_string(),
        None if state.displays.is_empty() && state.released.is_empty() => {
            "No DDC displays connected".to_string()
        }
        None => "Lumen".to_string(),
    };
    let _ = menu.append(&MenuItem::new(headline, false, None));
    let _ = menu.append(&PredefinedMenuItem::separator());

    for released in state.released {
        let _ = menu.append(&MenuItem::new(
            format!("{} is on another input", released.name),
            false,
            None,
        ));
        let _ = menu.append(&MenuItem::with_id(
            reclaim_id(released.id),
            format!("Bring {} back", released.name),
            true,
            None,
        ));
    }

    if !state.released.is_empty() && !state.displays.is_empty() {
        let _ = menu.append(&PredefinedMenuItem::separator());
    }

    for display in state.displays {
        let _ = menu.append(&display_menu(display));
    }

    if !state.displays.is_empty() {
        let _ = menu.append(&PredefinedMenuItem::separator());
    }
    let _ = menu.append(&MenuItem::with_id(REFRESH, "Refresh", true, None));
    let _ = menu.append(&MenuItem::with_id(QUIT, "Quit Lumen", true, None));
    menu
}

fn display_menu(display: &DisplayEntry) -> Submenu {
    let submenu = Submenu::new(display.name.clone(), true);

    if lumen::native::is_supported() {
        let _ = submenu.append(&MenuItem::with_id(
            release_id(display.id),
            "Release to other input (auto-switch)",
            true,
            None,
        ));
        let _ = submenu.append(&PredefinedMenuItem::separator());
    }

    if display.inputs.is_empty() {
        let _ = submenu.append(&MenuItem::new("No inputs reported", false, None));
    } else {
        let _ = submenu.append(&MenuItem::new("Switch input (DDC, if supported)", false, None));
        for input in &display.inputs {
            let item = CheckMenuItem::with_id(
                switch_id(display.id, input),
                input_label(input),
                true,
                false,
                None,
            );
            let _ = submenu.append(&item);
        }
    }

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
    if raw == REFRESH {
        return Some(Action::Refresh);
    }
    if raw == QUIT {
        return Some(Action::Quit);
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
        "release" => {
            let display = parts.next()?.parse().ok()?;
            Some(Action::Release { display })
        }
        "reclaim" => {
            let display = parts.next()?.parse().ok()?;
            Some(Action::Reclaim { display })
        }
        _ => None,
    }
}

fn switch_id(display: u32, input: &InputCode) -> String {
    let protocol = match input.protocol {
        InputProtocol::Standard => "std",
        InputProtocol::LgAlt => "lg",
    };
    format!("switch:{display}:{}:{protocol}", input.value)
}

fn brightness_id(display: u32, value: u16) -> String {
    format!("brightness:{display}:{value}")
}

fn release_id(display: u32) -> String {
    format!("release:{display}")
}

fn reclaim_id(display: u32) -> String {
    format!("reclaim:{display}")
}

fn input_label(input: &InputCode) -> String {
    match input.protocol {
        InputProtocol::Standard => input.label.clone(),
        InputProtocol::LgAlt => format!("{} (LG alt)", input.label),
    }
}

fn percent(value: u16, max: u16) -> String {
    if max == 0 {
        return String::new();
    }
    format!("{}%", (value as u32 * 100 / max as u32))
}
