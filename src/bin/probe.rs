//! Development probe for the DDC engine.
//!
//! `lumen-probe list`                    enumerate displays and their advertised inputs
//! `lumen-probe caps <id>`               print the capabilities string
//! `lumen-probe read <id> <code>`        read one VCP feature
//! `lumen-probe set <id> <code> <val>`   write one VCP feature
//! `lumen-probe switch <id> <label>`     try a DDC input switch, matching label
//! `lumen-probe release <id>`            stop driving the output (monitor follows another input)
//! `lumen-probe reclaim <id>`            drive the output again
//! `lumen-probe watch <seconds>`         report when displays come and go

use lumen::ddc::{
    parse_vcp_values, Display, InputCode, VCP_BRIGHTNESS, VCP_CONTRAST, VCP_INPUT_SOURCE,
    VCP_VOLUME,
};
use std::env;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("list");

    match command {
        "list" => list(),
        "caps" => with_display(parse_id(&args, 1), |d| {
            println!("{}", d.capabilities().unwrap_or_else(|e| e.to_string()))
        }),
        "read" => {
            let id = parse_id(&args, 1);
            let code = parse_hex(&args, 2) as u8;
            with_display(id, |d| match d.read_vcp(code) {
                Ok(v) => println!("{:#04x} = {} (max {})", v.code, v.value, v.max),
                Err(e) => println!("{:#04x} error: {e}", code),
            });
        }
        "set" => {
            let id = parse_id(&args, 1);
            let code = parse_hex(&args, 2) as u8;
            let value = parse_hex(&args, 3);
            with_display(id, |d| match d.write_vcp(code, value) {
                Ok(()) => println!("wrote {code:#04x} = {value}"),
                Err(e) => println!("write error: {e}"),
            });
        }
        "switch" => {
            let id = parse_id(&args, 1);
            let label = args.get(2).map(String::as_str).unwrap_or("HDMI 1");
            switch(id, label);
        }
        "release" => {
            let id = parse_id(&args, 1);
            println!("release = {}", lumen::native::set_enabled(id, false));
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(100));
                if !lumen::ddc::is_online(id) {
                    println!("display offline");
                    return;
                }
            }
            println!("display still online");
        }
        "reclaim" => {
            let id = parse_id(&args, 1);
            println!("reclaim = {}", lumen::native::set_enabled(id, true));
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(100));
                if lumen::ddc::is_online(id) {
                    println!("display online");
                    return;
                }
            }
            println!("display still offline");
        }
        "watch" => {
            let seconds: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
            let start = std::time::Instant::now();
            let mut last = true;
            while start.elapsed() < Duration::from_secs(seconds) {
                let present = !Display::enumerate().is_empty();
                if present != last {
                    println!(
                        "{:>6.2}s display {}",
                        start.elapsed().as_secs_f32(),
                        if present { "back" } else { "GONE" }
                    );
                    last = present;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            println!("end: present={last}");
        }
        other => eprintln!("unknown command {other}"),
    }
}

fn list() {
    let displays = Display::enumerate();
    if displays.is_empty() {
        println!("no DDC-capable external displays");
        return;
    }
    for mut display in displays {
        println!("\n[{}] {} ({})", display.id, display.name, display.manufacturer);
        println!("    chip 0x{:02x}  location {}", display.chip_address(), display.location);
        match display.read_vcp(VCP_BRIGHTNESS) {
            Ok(v) => println!("    brightness {} / {}", v.value, v.max),
            Err(e) => println!("    brightness: {e}"),
        }
        match display.read_vcp(VCP_CONTRAST) {
            Ok(v) => println!("    contrast   {} / {}", v.value, v.max),
            Err(e) => println!("    contrast:   {e}"),
        }
        match display.read_vcp(VCP_VOLUME) {
            Ok(v) => println!("    volume     {} / {}", v.value, v.max),
            Err(e) => println!("    volume:     {e}"),
        }
        match display.read_vcp(VCP_INPUT_SOURCE) {
            Ok(v) => println!("    input source VCP 0x60 = {} (max {})", v.value, v.max),
            Err(e) => println!("    input source: {e}"),
        }
        match display.capabilities() {
            Ok(caps) => {
                println!("    caps: {} bytes", caps.len());
                if let Some(values) = parse_vcp_values(&caps, VCP_INPUT_SOURCE) {
                    println!("    advertised inputs: {values:?}");
                }
            }
            Err(e) => println!("    caps: {e}"),
        }
        for input in display.input_candidates() {
            println!("      {:<24} {:#06x} via {:?}", input.label, input.value, input.protocol);
        }
    }
}

/// Tries one real switch by input label, waiting for the display to leave this input.
fn switch(id: u32, label: &str) {
    let Some(mut display) = Display::find(id) else {
        eprintln!("display {id} not found");
        return;
    };
    let wanted = label.to_lowercase();
    let candidates: Vec<InputCode> = display
        .input_candidates()
        .into_iter()
        .filter(|c| c.label.to_lowercase().contains(&wanted))
        .collect();

    if candidates.is_empty() {
        eprintln!("no advertised input matches {label:?}");
        return;
    }

    for candidate in candidates {
        println!(
            "attempt: {} VCP {:#04x} at {:#04x} = {:#06x}",
            candidate.label,
            candidate.protocol.vcp_code(),
            candidate.protocol.data_address().as_u32(),
            candidate.value
        );
        if let Err(e) = display.set_input(candidate.value, candidate.protocol) {
            println!("  write failed: {e}");
            continue;
        }
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(200));
            if !lumen::ddc::is_online(id) {
                println!("  display left this input: switch succeeded");
                return;
            }
        }
        println!("  display stayed: no switch");
    }
    println!("no candidate switched this display");
}

fn with_display(id: u32, f: impl FnOnce(&mut Display)) {
    match Display::find(id) {
        Some(mut d) => f(&mut d),
        None => eprintln!("display {id} not found"),
    }
}

fn parse_id(args: &[String], at: usize) -> u32 {
    args.get(at).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn parse_hex(args: &[String], at: usize) -> u16 {
    args.get(at)
        .map(|s| s.trim_start_matches("0x"))
        .and_then(|s| u16::from_str_radix(s, 16).ok())
        .unwrap_or(0)
}
