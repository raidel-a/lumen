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
}

impl InputSetting {
    pub fn from_detected(input: &lumen::ddc::InputCode) -> Self {
        Self {
            label: input.label.clone(),
            value: input.value,
            protocol: Protocol::from_ddc(input.protocol),
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// `vendor:model:serial`, or `name:<product name>` when the EDID gives nothing better.
    pub key: String,
    pub name: String,
    pub show_in_menu: bool,
    pub inputs: Vec<InputSetting>,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            reclaim_on_quit: true,
            auto_reclaim_minutes: None,
            show_brightness: true,
            displays: Vec::new(),
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
    pub fn sync(&mut self, displays: &[DisplayEntry]) {
        for display in displays {
            match self.displays.iter_mut().find(|entry| entry.key == display.key) {
                Some(entry) => {
                    entry.name = display.name.clone();
                    if entry.inputs.is_empty() {
                        entry.inputs = display.inputs.iter().map(InputSetting::from_detected).collect();
                    }
                }
                None => self.displays.push(DisplayConfig {
                    key: display.key.clone(),
                    name: display.name.clone(),
                    show_in_menu: true,
                    inputs: display.inputs.iter().map(InputSetting::from_detected).collect(),
                }),
            }
        }
        self.save();
    }

    pub fn display(&self, key: &str) -> Option<&DisplayConfig> {
        self.displays.iter().find(|entry| entry.key == key)
    }

    pub fn display_mut(&mut self, key: &str) -> Option<&mut DisplayConfig> {
        self.displays.iter_mut().find(|entry| entry.key == key)
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

    pub fn enabled_inputs(&self, key: &str) -> Vec<InputSetting> {
        self.display(key)
            .map(|entry| entry.inputs.iter().filter(|input| input.enabled).cloned().collect())
            .unwrap_or_default()
    }
}
