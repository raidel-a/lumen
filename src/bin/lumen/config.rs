//! Persistent settings.
//!
//! The menu bar is the app's primary surface, so the config describes what the menu shows:
//! per-display input lists and a handful of behaviour switches.

use crate::engine::DisplayEntry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Standard,
    LgAlt,
}

impl Protocol {
    pub fn from_ddc(protocol: lumen::ddc::InputProtocol) -> Self {
        match protocol {
            lumen::ddc::InputProtocol::Standard => Protocol::Standard,
            lumen::ddc::InputProtocol::LgAlt => Protocol::LgAlt,
        }
    }

    pub fn to_ddc(self) -> lumen::ddc::InputProtocol {
        match self {
            Protocol::Standard => lumen::ddc::InputProtocol::Standard,
            Protocol::LgAlt => lumen::ddc::InputProtocol::LgAlt,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Standard => "standard",
            Protocol::LgAlt => "LG alt",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputSetting {
    pub label: String,
    pub value: u16,
    pub protocol: Protocol,
    pub enabled: bool,
    /// None until tested, then whether a switch this app sent actually worked.
    #[serde(default)]
    pub verified: Option<bool>,
}

impl InputSetting {
    pub fn from_detected(input: &lumen::ddc::InputCode) -> Self {
        Self {
            label: input.label.clone(),
            value: input.value,
            protocol: Protocol::from_ddc(input.protocol),
            enabled: true,
            verified: None,
        }
    }
}

/// The input the Mac itself is plugged into, remembered across launches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentInput {
    pub value: u16,
    pub protocol: Protocol,
}

/// A display that exists only in memory, created with `CGVirtualDisplay`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VirtualScreenConfig {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Ask for a 2x backing store. 1x is cheaper and gives the agent more logical space.
    pub hidpi: bool,
    pub refresh: f64,
    /// Recreate this screen at launch and after wake.
    pub auto_create: bool,
}

impl Default for VirtualScreenConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            width: 1920,
            height: 1080,
            hidpi: false,
            refresh: 60.0,
            auto_create: true,
        }
    }
}

/// A browser this app started on a virtual screen. Persisted so a crash cannot leave it running
/// on a real display after the screen is gone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSessionRecord {
    pub name: String,
    pub pid: i32,
    pub browser: String,
    pub profile: String,
    #[serde(default)]
    pub url: Option<String>,
}

/// A display this app released and has not brought back yet. Kept on disk so a crash cannot
/// leave the output switched off until logout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleasedRecord {
    pub id: u32,
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// `vendor:model:serial`, or `name:<product name>` when the EDID gives nothing better.
    pub key: String,
    pub name: String,
    pub show_in_menu: bool,
    pub inputs: Vec<InputSetting>,
    /// The input this Mac is plugged into, so the menu can mark it even on monitors that do not
    /// report their input (this LG answers `VCP 0x60` with zero).
    #[serde(default)]
    pub current_input: Option<CurrentInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Bring every released display back before the app exits.
    pub reclaim_on_quit: bool,
    /// Reclaim a released display after this many minutes (None keeps it released).
    pub auto_reclaim_minutes: Option<u32>,
    /// Show the brightness percentage and presets in each display's submenu.
    pub show_brightness: bool,
    pub displays: Vec<DisplayConfig>,
    pub released: Vec<ReleasedRecord>,
    /// Screens that exist only in memory, for the agent workspace and for capture.
    pub virtual_screens: Vec<VirtualScreenConfig>,
    /// Agent browsers this app started, so they can be closed on quit or after a crash.
    pub agent_sessions: Vec<AgentSessionRecord>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            reclaim_on_quit: true,
            auto_reclaim_minutes: None,
            show_brightness: true,
            displays: Vec::new(),
            released: Vec::new(),
            virtual_screens: Vec::new(),
            agent_sessions: Vec::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join("Library/Application Support/Lumen/config.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }

    /// Adds configs for displays seen for the first time and refreshes their names and inputs.
    /// Saves only when something changed, since this runs on every scan.
    pub fn sync(&mut self, displays: &[DisplayEntry]) {
        let mut changed = false;
        for display in displays {
            match self.displays.iter_mut().find(|entry| entry.key == display.key) {
                Some(entry) => {
                    if entry.name != display.name {
                        entry.name = display.name.clone();
                        changed = true;
                    }
                    if entry.inputs.is_empty() {
                        entry.inputs = display.inputs.iter().map(InputSetting::from_detected).collect();
                        changed = true;
                    }
                }
                None => {
                    self.displays.push(DisplayConfig {
                        key: display.key.clone(),
                        name: display.name.clone(),
                        show_in_menu: true,
                        inputs: display.inputs.iter().map(InputSetting::from_detected).collect(),
                        current_input: None,
                    });
                    changed = true;
                }
            }
        }
        if changed {
            self.save();
        }
    }

    pub fn display(&self, key: &str) -> Option<&DisplayConfig> {
        self.displays.iter().find(|entry| entry.key == key)
    }

    pub fn display_mut(&mut self, key: &str) -> Option<&mut DisplayConfig> {
        self.displays.iter_mut().find(|entry| entry.key == key)
    }

    /// Records the result of a switch attempt on one input.
    pub fn mark_input(&mut self, key: &str, value: u16, protocol: Protocol, worked: bool) {
        if let Some(entry) = self.display_mut(key) {
            for input in entry.inputs.iter_mut() {
                if input.value == value && input.protocol == protocol {
                    input.verified = Some(worked);
                }
            }
        }
        self.save();
    }

    /// Replaces a display's list with what the monitor currently advertises.
    pub fn reset_inputs(&mut self, key: &str, displays: &[DisplayEntry]) {
        let detected = displays
            .iter()
            .find(|display| display.key == key)
            .map(|display| display.inputs.clone())
            .unwrap_or_default();
        if let Some(entry) = self.display_mut(key) {
            entry.inputs = detected.iter().map(InputSetting::from_detected).collect();
        }
        self.save();
    }

    /// The socket the menu-bar app listens on for CLI and agent commands.
    pub fn support_dir() -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join("Library/Application Support/Lumen")
    }

    pub fn virtual_screen(&self, name: &str) -> Option<&VirtualScreenConfig> {
        self.virtual_screens.iter().find(|screen| screen.name == name)
    }

    /// Adds or updates a screen definition and saves.
    pub fn upsert_virtual_screen(&mut self, screen: VirtualScreenConfig) {
        match self.virtual_screens.iter_mut().find(|entry| entry.name == screen.name) {
            Some(entry) => *entry = screen,
            None => self.virtual_screens.push(screen),
        }
        self.save();
    }

    pub fn remove_virtual_screen(&mut self, name: &str) {
        self.virtual_screens.retain(|screen| screen.name != name);
        self.save();
    }

    pub fn record_agent_session(&mut self, record: AgentSessionRecord) {
        self.agent_sessions.retain(|entry| entry.name != record.name);
        self.agent_sessions.push(record);
        self.save();
    }

    pub fn forget_agent_session(&mut self, name: &str) {
        self.agent_sessions.retain(|entry| entry.name != name);
        self.save();
    }

    /// The remembered Mac-side input, if one has been set.
    pub fn current_input(&self, key: &str) -> Option<CurrentInput> {
        self.display(key).and_then(|entry| entry.current_input)
    }

    /// Records the input the Mac is plugged into. Saves only when it actually changes.
    pub fn set_current_input(&mut self, key: &str, value: u16, protocol: Protocol) {
        let wanted = CurrentInput { value, protocol };
        if let Some(entry) = self.display_mut(key) {
            if entry.current_input != Some(wanted) {
                entry.current_input = Some(wanted);
                self.save();
            }
        }
    }

    pub fn enabled_inputs(&self, key: &str) -> Vec<InputSetting> {
        self.display(key)
            .map(|entry| entry.inputs.iter().filter(|input| input.enabled).cloned().collect())
            .unwrap_or_default()
    }
}
