//! Display operations. Everything here touches hardware, so it runs on worker threads.

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
    pub inputs: Vec<InputCode>,
    pub brightness: Option<(u16, u16)>,
    pub contrast: Option<(u16, u16)>,
    pub volume: Option<(u16, u16)>,
    pub muted: Option<bool>,
    /// What the monitor reports for VCP 0x60, when the firmware returns something valid.
    pub current_input: Option<u16>,
}

/// Reads every DDC display and its advertised inputs.
pub fn scan() -> Vec<DisplayEntry> {
    let mut displays = Vec::new();
    for mut display in Display::enumerate() {
        // Read the control values before the input list: the capabilities string is fetched in
        // several chunks, and this LG stops answering VCP reads for a moment afterwards.
        let brightness = display.read_vcp(VCP_BRIGHTNESS).ok().map(|v| (v.value, v.max));
        let contrast = display.read_vcp(VCP_CONTRAST).ok().map(|v| (v.value, v.max));
        let volume = display.read_vcp(VCP_VOLUME).ok().map(|v| (v.value, v.max));
        let muted = display
            .read_vcp(VCP_MUTE)
            .ok()
            .map(|v| match v.value {
                1 => true,
                2 => false,
                _ => false,
            });
        // Several LG firmwares answer 0 here regardless of the input; treat that as unknown.
        let current_input = display
            .read_vcp(VCP_INPUT_SOURCE)
            .ok()
            .map(|v| v.value)
            .filter(|value| *value > 0);
        let inputs = display.input_candidates();

        displays.push(DisplayEntry {
            id: display.id,
            key: display.identity_key(),
            name: display.name.clone(),
            inputs,
            brightness,
            contrast,
            volume,
            muted,
            current_input,
        });
    }
    displays
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
