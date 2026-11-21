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
        self.config.set_current_input(key, value, protocol);
    }

    /// The input the display is showing, best effort:
    ///
    /// 1. what the monitor reports through `VCP 0x60`, when it answers;
    /// 2. the input this app last switched it to, remembered on disk;
    /// 3. the first enabled input, which is the usual Mac-side port.
    ///
    /// Step 3 matters here because the LG reports `0x60` as zero and ignores input-select writes.
    pub fn current_input(&self, display: &DisplayEntry) -> Option<(u16, Protocol)> {
        let enabled = self.config.enabled_inputs(&display.key);
        let has = |value: u16, protocol: Protocol| {
            enabled
                .iter()
                .any(|input| input.value == value && input.protocol == protocol)
        };

        if let Some(reported) = display.current_input {
            if has(reported, Protocol::Standard) {
                return Some((reported, Protocol::Standard));
            }
        }
        if let Some(saved) = self.config.current_input(&display.key) {
            if has(saved.value, saved.protocol) {
                return Some((saved.value, saved.protocol));
            }
        }
        enabled.first().map(|input| (input.value, input.protocol))
    }

    /// Whether this input is the one the display is on now.
    pub fn is_current(&self, display: &DisplayEntry, value: u16, protocol: Protocol) -> bool {
        self.current_input(display) == Some((value, protocol))
    }

    pub fn released(&self, key: &str) -> bool {
        self.released.iter().any(|entry| entry.key == key)
    }

    /// Applies a fresh scan, keeping the last control values when a DDC read failed.
    ///
    /// The LG keeps its DisplayPort link alive while showing another input, and its DDC channel
    /// drops replies around a switch and link retrain. Replacing the model outright turned one
    /// missed packet into a control that vanished from the menu until a manual refresh; carrying
    /// the previous value forward keeps the slider there while the monitor settles.
    pub fn set_displays(&mut self, displays: Vec<DisplayEntry>) {
        let previous = std::mem::take(&mut self.displays);
        let mut displays = displays;
        for display in &mut displays {
            let old = previous.iter().find(|entry| entry.key == display.key);
            carry_controls(old, display);
        }
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

/// Carries control values from the previous scan into `display` where this scan found none.
///
/// Returns true when at least one control was carried, so the UI can say the values are the last
/// known ones instead of pretending the read succeeded.
fn carry_controls(previous: Option<&DisplayEntry>, display: &mut DisplayEntry) -> bool {
    display.controls_stale = false;
    let Some(previous) = previous else {
        return false;
    };
    let mut carried = false;
    if display.brightness.is_none() && previous.brightness.is_some() {
        display.brightness = previous.brightness;
        carried = true;
    }
    if display.contrast.is_none() && previous.contrast.is_some() {
        display.contrast = previous.contrast;
        carried = true;
    }
    if display.volume.is_none() && previous.volume.is_some() {
        display.volume = previous.volume;
        carried = true;
    }
    if display.muted.is_none() && previous.muted.is_some() {
        display.muted = previous.muted;
        carried = true;
    }
    if display.inputs.is_empty() && !previous.inputs.is_empty() {
        display.inputs = previous.inputs.clone();
    }
    display.controls_stale = carried;
    carried
}

#[cfg(test)]
mod tests {
    use super::carry_controls;
    use crate::engine::DisplayEntry;

    fn entry(id: u32, key: &str) -> DisplayEntry {
        DisplayEntry {
            id,
            key: key.to_string(),
            name: "LG ULTRAWIDE".to_string(),
            builtin: false,
            inputs: Vec::new(),
            brightness: None,
            contrast: None,
            volume: None,
            muted: None,
            controls_stale: false,
            current_input: None,
            ddc_error: None,
        }
    }

    #[test]
    fn a_missed_read_keeps_the_previous_value() {
        let mut old = entry(2, "lg");
        old.brightness = Some((70, 100));
        old.contrast = Some((50, 100));
        let mut fresh = entry(2, "lg");
        fresh.contrast = Some((60, 100));

        assert!(carry_controls(Some(&old), &mut fresh));
        assert_eq!(fresh.brightness, Some((70, 100)));
        // A value this scan did read wins over the old one.
        assert_eq!(fresh.contrast, Some((60, 100)));
        assert!(fresh.controls_stale);
    }

    #[test]
    fn a_complete_read_clears_the_stale_flag() {
        let mut old = entry(2, "lg");
        old.brightness = Some((70, 100));
        let mut fresh = entry(2, "lg");
        fresh.brightness = Some((80, 100));

        assert!(!carry_controls(Some(&old), &mut fresh));
        assert_eq!(fresh.brightness, Some((80, 100)));
        assert!(!fresh.controls_stale);
    }

    #[test]
    fn a_new_display_borrows_nothing() {
        let mut fresh = entry(3, "other");
        assert!(!carry_controls(None, &mut fresh));
        assert_eq!(fresh.brightness, None);
        assert!(!fresh.controls_stale);
    }

    #[test]
    fn detected_inputs_survive_a_failed_capability_read() {
        let mut old = entry(2, "lg");
        old.inputs = vec![lumen::ddc::InputCode {
            label: "HDMI 1".to_string(),
            value: 0x90,
            protocol: lumen::ddc::InputProtocol::LgAlt,
        }];
        let mut fresh = entry(2, "lg");

        assert!(!carry_controls(Some(&old), &mut fresh));
        assert_eq!(fresh.inputs, old.inputs);
    }
}
