//! The app's model: what the menu and the settings window read.

use crate::config::Config;
use crate::engine::DisplayEntry;

#[derive(Clone, Debug)]
pub struct Released {
    pub id: u32,
    pub key: String,
    pub name: String,
}

#[derive(Default)]
pub struct Model {
    pub displays: Vec<DisplayEntry>,
    pub released: Vec<Released>,
    pub message: Option<String>,
    pub config: Config,
}

impl Model {
    pub fn display(&self, id: u32) -> Option<&DisplayEntry> {
        self.displays.iter().find(|display| display.id == id)
    }

    pub fn display_by_key(&self, key: &str) -> Option<&DisplayEntry> {
        self.displays.iter().find(|display| display.key == key)
    }

    pub fn released(&self, key: &str) -> bool {
        self.released.iter().any(|entry| entry.key == key)
    }

    /// Applies a fresh scan, keeping release records only for displays that are still away.
    pub fn set_displays(&mut self, displays: Vec<DisplayEntry>) {
        self.displays = displays;
        self.released.retain(|released| !self.displays.iter().any(|display| display.id == released.id));
        self.config.sync(&self.displays);
    }

    pub fn mark_released(&mut self, id: u32, name: &str) {
        if let Some(display) = self.displays.iter().find(|display| display.id == id) {
            let key = display.key.clone();
            if !self.released.iter().any(|entry| entry.key == key) {
                self.released.push(Released { id, key, name: name.to_string() });
            }
        }
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
