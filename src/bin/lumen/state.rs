//! Application state and the blocking display operations.
//!
//! Everything here talks to hardware, so it runs on a worker thread and the result is handed
//! back to the menu bar thread.

use lumen::ddc::{
    DataAddress, Display, InputCode, InputProtocol, VCP_BRIGHTNESS, VCP_CONTRAST, VCP_VOLUME,
};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct DisplayEntry {
    pub id: u32,
    pub name: String,
    pub inputs: Vec<InputCode>,
    pub brightness: Option<(u16, u16)>,
    pub contrast: Option<(u16, u16)>,
    pub volume: Option<(u16, u16)>,
}

/// Reads every DDC display and its advertised inputs.
pub fn scan() -> Vec<DisplayEntry> {
    let mut displays = Vec::new();
    for mut display in Display::enumerate() {
        let inputs = display.input_candidates();
        let brightness = display.read_vcp(VCP_BRIGHTNESS).ok().map(|v| (v.value, v.max));
        let contrast = display.read_vcp(VCP_CONTRAST).ok().map(|v| (v.value, v.max));
        let volume = display.read_vcp(VCP_VOLUME).ok().map(|v| (v.value, v.max));
        displays.push(DisplayEntry {
            id: display.id,
            name: display.name.clone(),
            inputs,
            brightness,
            contrast,
            volume,
        });
    }
    displays
}

/// Switches a display to `input`, trying the neighbouring protocol variants as fallbacks.
///
/// A successful switch removes the display from macOS, so "the display is gone" is the success
/// signal. A monitor that ignores the command keeps the display online and the next variant runs.
pub fn switch(display_id: u32, input: &InputCode) -> String {
    let Some(mut display) = Display::find(display_id) else {
        return "Display is not connected".to_string();
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
            return format!("{name}: switched to {base_label}");
        };
        let _ = display.set_input(attempt.value, attempt.protocol);
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(100));
            if !lumen::ddc::is_online(display_id) {
                return format!("{name}: switched to {base_label}");
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }

    format!("{name}: no response to {base_label}")
}

/// Stops driving the display so the monitor can follow another input.
pub fn release(display_id: u32, name: &str) -> String {
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
    let Some(mut display) = Display::find(display_id) else {
        return "Display is not connected".to_string();
    };
    match display.write_vcp_with(code, value, DataAddress::Host) {
        Ok(()) => format!("{} set to {value}", control_name(code)),
        Err(error) => format!("{}: {error}", control_name(code)),
    }
}

fn control_name(code: u8) -> &'static str {
    match code {
        VCP_BRIGHTNESS => "Brightness",
        VCP_CONTRAST => "Contrast",
        VCP_VOLUME => "Volume",
        _ => "Control",
    }
}
