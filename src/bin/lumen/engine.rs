//! Display operations. Everything here touches hardware, so it runs on worker threads.

use crate::brightness;
use core_graphics::display::CGDisplay;
use lumen::ddc::{
    DataAddress, Display, InputCode, InputProtocol, VCP_BRIGHTNESS, VCP_CONTRAST,
    VCP_INPUT_SOURCE, VCP_VOLUME,
};

pub const VCP_MUTE: u8 = 0x8D;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct DisplayEntry {
    pub id: u32,
    /// Stable identity used by the config file.
    pub key: String,
    pub name: String,
    /// The internal panel: no DDC path, brightness through DisplayServices, no inputs.
    pub builtin: bool,
    pub inputs: Vec<InputCode>,
    pub brightness: Option<(u16, u16)>,
    pub contrast: Option<(u16, u16)>,
    pub volume: Option<(u16, u16)>,
    pub muted: Option<bool>,
    /// What the monitor reports for VCP 0x60, when the firmware returns something valid.
    pub current_input: Option<u16>,
    /// The first DDC reads that failed, for the diagnostics row in Settings.
    pub ddc_error: Option<String>,
}

/// Reads every DDC display and its advertised inputs, plus the internal panel.
pub fn scan() -> Vec<DisplayEntry> {
    let mut displays = Vec::new();
    for mut display in Display::enumerate() {
        // Read the control values before the input list: the capabilities string is fetched in
        // several chunks, and this LG stops answering VCP reads for a moment afterwards.
        let mut errors: Vec<String> = Vec::new();
        let mut read = |code: u8, label: &str| match display.read_vcp(code) {
            Ok(value) => Some(value),
            Err(error) => {
                if errors.len() < 3 {
                    errors.push(format!("{label}: {error}"));
                }
                None
            }
        };
        let brightness = read(VCP_BRIGHTNESS, "brightness").map(|v| (v.value, v.max));
        let contrast = read(VCP_CONTRAST, "contrast").map(|v| (v.value, v.max));
        let volume = read(VCP_VOLUME, "volume").map(|v| (v.value, v.max));
        let muted = read(VCP_MUTE, "mute").map(|v| match v.value {
            1 => true,
            2 => false,
            _ => false,
        });
        // Several LG firmwares answer 0 here regardless of the input; treat that as unknown.
        let current_input = read(VCP_INPUT_SOURCE, "input")
            .map(|v| v.value)
            .filter(|value| *value > 0);
        let ddc_error = ddc_diagnostic(
            &errors,
            brightness.is_some(),
            contrast.is_some(),
            volume.is_some(),
        );
        let inputs = display.input_candidates();

        displays.push(DisplayEntry {
            id: display.id,
            key: display.identity_key(),
            name: display.name.clone(),
            builtin: false,
            inputs,
            brightness,
            contrast,
            volume,
            muted,
            current_input,
            ddc_error,
        });
    }
    scan_builtin(&mut displays);
    // The internal panel is always present; list it before external monitors in the menu and in
    // the Settings source list.
    displays.sort_by_key(|display| !display.builtin);
    displays
}

/// Adds the internal panel. It has no DDC path; DisplayServices drives the backlight, and there is
/// no contrast, volume, input or release. The key is fixed because there is one internal panel and
/// its CoreGraphics id can change across reboots.
fn scan_builtin(displays: &mut Vec<DisplayEntry>) {
    let Ok(ids) = CGDisplay::active_displays() else {
        return;
    };
    for id in ids {
        if !CGDisplay::new(id).is_builtin() {
            continue;
        }
        if displays.iter().any(|display| display.id == id) {
            continue;
        }
        let brightness = brightness::get_percent(id);
        let ddc_error = if brightness.is_some() {
            None
        } else if brightness::available() {
            Some("DisplayServices did not return a brightness value".to_string())
        } else {
            Some("the DisplayServices API is unavailable".to_string())
        };
        displays.push(DisplayEntry {
            id,
            key: "builtin".to_string(),
            name: "Built-in Display".to_string(),
            builtin: true,
            inputs: Vec::new(),
            brightness: brightness.map(|value| (value, 100)),
            contrast: None,
            volume: None,
            muted: None,
            current_input: None,
            ddc_error,
        });
    }
}

pub struct SwitchOutcome {
    pub message: String,
    pub switched: bool,
}

/// Switches a display to `input`, trying the neighbouring protocol variants as fallbacks.
///
/// A successful switch removes the display from macOS, so "the display is gone" is the success
/// signal. A monitor that ignores the command keeps the display online and the next variant runs.
pub fn switch(display_id: u32, input: &InputCode) -> SwitchOutcome {
    let Some(mut display) = Display::find(display_id) else {
        return SwitchOutcome { message: "Display is not connected".to_string(), switched: false };
    };
    let name = display.name.clone();
    let base_label = input.label.replace(" (LG alt)", "");
    let mut attempts: Vec<InputCode> = vec![input.clone()];

    let other = match input.protocol {
        InputProtocol::Standard => InputProtocol::LgAlt,
        InputProtocol::LgAlt => InputProtocol::Standard,
    };
    attempts.push(InputCode { label: input.label.clone(), value: input.value, protocol: other });
    for candidate in display.input_candidates() {
        if candidate.label.replace(" (LG alt)", "") == base_label
            && !attempts
                .iter()
                .any(|a| a.value == candidate.value && a.protocol == candidate.protocol)
        {
            attempts.push(candidate);
        }
    }

    for attempt in attempts {
        let Some(mut display) = Display::find(display_id) else {
            return SwitchOutcome { message: format!("{name}: switched to {base_label}"), switched: true };
        };
        let _ = display.set_input(attempt.value, attempt.protocol);
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(100));
            if !lumen::ddc::is_online(display_id) {
                return SwitchOutcome {
                    message: format!("{name}: switched to {base_label}"),
                    switched: true,
                };
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }

    SwitchOutcome { message: format!("{name}: no response to {base_label}"), switched: false }
}

/// Stops driving the display so the monitor can follow another input.
pub fn release(display_id: u32, name: &str) -> String {
    if CGDisplay::new(display_id).is_builtin() {
        return format!("{name}: the built-in display has no other input");
    }
    if !lumen::native::set_enabled(display_id, false) {
        return format!("{name}: could not release the output");
    }
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if !lumen::ddc::is_online(display_id) {
            return format!("{name}: released - monitor follows its other input");
        }
    }
    format!("{name}: release sent")
}

/// Drives the display again; the monitor follows the returning signal.
pub fn reclaim(display_id: u32, name: &str) -> String {
    if !lumen::native::set_enabled(display_id, true) {
        return format!("{name}: could not reclaim the output");
    }
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if lumen::ddc::is_online(display_id) {
            return format!("{name}: back on this Mac");
        }
    }
    format!("{name}: asked macOS to drive the output again")
}

/// Sends a raw VCP write, used by the brightness quick actions.
pub fn write_vcp(display_id: u32, code: u8, value: u16) -> String {
    match write_vcp_quiet(display_id, code, value) {
        Ok(()) => format!("{} set to {value}", control_name(code)),
        Err(error) => format!("{}: {error}", control_name(code)),
    }
}

/// The same write without building a user-facing message, for slider streams.
pub fn write_vcp_quiet(display_id: u32, code: u8, value: u16) -> Result<(), String> {
    if code == VCP_BRIGHTNESS && CGDisplay::new(display_id).is_builtin() {
        return brightness::set_percent(display_id, value);
    }
    let Some(mut display) = Display::find(display_id) else {
        return Err("display is not connected".into());
    };
    display
        .write_vcp_with(code, value, DataAddress::Host)
        .map_err(|error| error.to_string())
}

pub fn control_name(code: u8) -> &'static str {
    match code {
        VCP_BRIGHTNESS => "Brightness",
        VCP_CONTRAST => "Contrast",
        VCP_VOLUME => "Volume",
        VCP_MUTE => "Mute",
        _ => "Control",
    }
}

/// Only explain a completely blank control set; an unsupported volume control is normal.
fn ddc_diagnostic(
    errors: &[String],
    has_brightness: bool,
    has_contrast: bool,
    has_volume: bool,
) -> Option<String> {
    if errors.is_empty() || has_brightness || has_contrast || has_volume {
        return None;
    }
    Some(errors.join("; "))
}

#[cfg(test)]
mod tests {
    use super::ddc_diagnostic;

    #[test]
    fn explains_a_completely_blank_control_set() {
        let errors = vec!["brightness: no reply".to_string()];
        assert_eq!(
            ddc_diagnostic(&errors, false, false, false),
            Some("brightness: no reply".to_string())
        );
    }

    #[test]
    fn ignores_errors_when_one_control_works() {
        let errors = vec!["volume: unsupported".to_string()];
        assert_eq!(ddc_diagnostic(&errors, true, false, false), None);
        assert_eq!(ddc_diagnostic(&errors, false, false, true), None);
    }

    #[test]
    fn no_errors_means_no_diagnostic() {
        assert_eq!(ddc_diagnostic(&[], false, false, false), None);
    }
}
