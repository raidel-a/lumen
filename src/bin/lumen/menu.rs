//! Menu model: the ids menu items carry and the actions they mean.

use lumen::ddc::InputProtocol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Quit,
    Settings,
    CheckForUpdates,
    Switch { display: u32, value: u16, protocol: InputProtocol },
    Brightness { display: u32, value: u16 },
    Contrast { display: u32, value: u16 },
    Volume { display: u32, value: u16 },
    Mute { display: u32, muted: bool },
    Release { key: String },
    Reclaim { key: String },
    Preview { name: String },
    RemoveScreen { name: String },
}

pub const REFRESH: &str = "refresh";
pub const QUIT: &str = "quit";
pub const SETTINGS: &str = "settings";
pub const CHECK_UPDATES: &str = "check-updates";

/// Turns a menu id back into the action it stands for.
pub fn parse(raw: &str) -> Option<Action> {
    match raw {
        REFRESH => return Some(Action::Refresh),
        QUIT => return Some(Action::Quit),
        SETTINGS => return Some(Action::Settings),
        CHECK_UPDATES => return Some(Action::CheckForUpdates),
        _ => {}
    }
    if let Some(key) = raw.strip_prefix("release|") {
        return Some(Action::Release { key: key.to_string() });
    }
    if let Some(key) = raw.strip_prefix("reclaim|") {
        return Some(Action::Reclaim { key: key.to_string() });
    }
    if let Some(name) = raw.strip_prefix("preview|") {
        return Some(Action::Preview { name: name.to_string() });
    }
    if let Some(name) = raw.strip_prefix("remove-screen|") {
        return Some(Action::RemoveScreen { name: name.to_string() });
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
        "contrast" => {
            let display = parts.next()?.parse().ok()?;
            let value = parts.next()?.parse().ok()?;
            Some(Action::Contrast { display, value })
        }
        "volume" => {
            let display = parts.next()?.parse().ok()?;
            let value = parts.next()?.parse().ok()?;
            Some(Action::Volume { display, value })
        }
        "mute" => {
            let display = parts.next()?.parse().ok()?;
            let muted = parts.next()? == "1";
            Some(Action::Mute { display, muted })
        }
        _ => None,
    }
}

pub fn switch_id(display: u32, value: u16, protocol: InputProtocol) -> String {
    let protocol = match protocol {
        InputProtocol::Standard => "std",
        InputProtocol::LgAlt => "lg",
    };
    format!("switch:{display}:{value}:{protocol}")
}

pub fn preview_id(name: &str) -> String {
    format!("preview|{name}")
}

pub fn remove_screen_id(name: &str) -> String {
    format!("remove-screen|{name}")
}

pub fn release_id(key: &str) -> String {
    format!("release|{key}")
}

pub fn reclaim_id(key: &str) -> String {
    format!("reclaim|{key}")
}

pub fn mute_id(display: u32, muted: bool) -> String {
    format!("mute:{display}:{}", u8::from(muted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trailer_actions() {
        assert_eq!(parse(REFRESH), Some(Action::Refresh));
        assert_eq!(parse(QUIT), Some(Action::Quit));
        assert_eq!(parse(SETTINGS), Some(Action::Settings));
        assert_eq!(parse(CHECK_UPDATES), Some(Action::CheckForUpdates));
    }

    #[test]
    fn unknown_id_is_none() {
        assert_eq!(parse("nope"), None);
    }
}
