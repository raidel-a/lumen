//! The app's model: what the menu and the settings window read.

use crate::config::{Config, Protocol, ReleasedRecord};
use std::collections::HashMap;
use crate::engine::DisplayEntry;

#[derive(Clone, Debug)]
pub struct Released {
    pub id: u32,
    pub key: String,
    pub name: String,
}

#[derive(Clone, Default)]
pub struct Model {
    pub displays: Vec<DisplayEntry>,
    pub released: Vec<Released>,
    pub message: Option<String>,
    pub config: Config,
    /// The input this app last switched each display to, keyed by display identity.
    pub last_input: HashMap<String, (u16, Protocol)>,
}

impl Model {
    /// A copy for UI building, so callers never hold the model borrow while AppKit runs.
    pub fn snapshot(&self) -> Model {
        self.clone()
    }

    pub fn display(&self, id: u32) -> Option<&DisplayEntry> {
        self.displays.iter().find(|display| display.id == id)
    }

    pub fn display_by_key(&self, key: &str) -> Option<&DisplayEntry> {
        self.displays.iter().find(|display| display.key == key)
    }

    /// Folds a control write into the cached display values so the menu reflects it at once.
    pub fn apply_control(&mut self, display_id: u32, code: u8, value: u16) {
        let Some(display) = self.displays.iter_mut().find(|entry| entry.id == display_id) else {
            return;
        };
        match code {
            crate::engine::VCP_MUTE => display.muted = Some(value == 1),
            0x10 => {
                let max = display.brightness.map(|(_, max)| max).unwrap_or(100);
                display.brightness = Some((value.min(max), max));
            }
            0x12 => {
                let max = display.contrast.map(|(_, max)| max).unwrap_or(100);
                display.contrast = Some((value.min(max), max));
            }
            0x62 => {
                let max = display.volume.map(|(_, max)| max).unwrap_or(100);
                display.volume = Some((value.min(max), max));
            }
            _ => {}
        }
    }

    pub fn remember_input(&mut self, key: &str, value: u16, protocol: Protocol) {
        self.last_input.insert(key.to_string(), (value, protocol));
    }

    /// Whether this input is the one the display is on now, from the monitor's own report or
    /// from the last switch this app made.
    pub fn is_current(&self, display: &DisplayEntry, value: u16, protocol: Protocol) -> bool {
        if self.last_input.get(&display.key) == Some(&(value, protocol)) {
            return true;
        }
        protocol == Protocol::Standard && display.current_input == Some(value)
    }

    pub fn released(&self, key: &str) -> bool {
        self.released.iter().any(|entry| entry.key == key)
    }

    /// Applies a fresh scan, keeping release records only for displays that are still away.
    pub fn set_displays(&mut self, displays: Vec<DisplayEntry>) {
        self.displays = displays;
        let before = self.released.len();
        self.released.retain(|released| !self.displays.iter().any(|display| display.id == released.id));
        if self.released.len() != before {
            // A released display came back on its own; its saved record is stale.
            let live: Vec<u32> = self.released.iter().map(|entry| entry.id).collect();
            self.config.released.retain(|record| live.contains(&record.id));
            self.config.save();
        }
        self.config.sync(&self.displays);
    }

    pub fn mark_released(&mut self, id: u32, name: &str) {
        if let Some(display) = self.displays.iter().find(|display| display.id == id) {
            let key = display.key.clone();
            if !self.released.iter().any(|entry| entry.key == key) {
                self.released.push(Released { id, key: key.clone(), name: name.to_string() });
            }
            self.config.released.retain(|record| record.id != id);
            self.config.released.push(ReleasedRecord { id, key, name: name.to_string() });
            self.config.save();
        }
    }

    /// Forgets a release record, both in memory and on disk.
    pub fn forget_released(&mut self, id: u32) {
        self.released.retain(|entry| entry.id != id);
        self.config.released.retain(|record| record.id != id);
        self.config.save();
    }

    /// Displays to draw in the menu: connected, and not hidden in settings.
    pub fn menu_displays(&self) -> impl Iterator<Item = &DisplayEntry> {
        self.displays.iter().filter(|display| {
            self.config
                .display(&display.key)
                .map(|config| config.show_in_menu)
                .unwrap_or(true)
        })
    }
}
