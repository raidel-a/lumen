//! CLI client for the control socket. These commands never touch AppKit; they ask the running
//! menu-bar app to do the work.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub fn run(args: &[String]) -> i32 {
    let reply = match args.first().map(String::as_str) {
        Some("virtual") => virtual_command(&args[1..]),
        Some("agent") => agent_command(&args[1..]),
        Some("settings") => settings_command(&args[1..]),
        _ => {
            help();
            return 2;
        }
    };
    let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
    println!(
        "{}",
        serde_json::to_string_pretty(&reply).unwrap_or_else(|_| reply.to_string())
    );
    if ok {
        0
    } else {
        1
    }
}

fn settings_command(args: &[String]) -> Value {
    if matches!(args.first().map(String::as_str), Some("state")) {
        return send(json!({ "cmd": "settings.state" }));
    }
    let mut request = json!({ "cmd": "settings.open" });
    if let Some(page) = option(args, "--page") {
        request["page"] = json!(page);
    }
    if let Some(screen) = option(args, "--screen") {
        request["screen"] = json!(screen);
    }
    send(request)
}

fn option(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|item| item == key)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn flag(args: &[String], key: &str) -> bool {
    args.iter().any(|item| item == key)
}

fn virtual_command(args: &[String]) -> Value {
    let Some(command) = args.first().map(String::as_str) else {
        help();
        return json!({ "ok": false, "error": "virtual needs a subcommand" });
    };
    match command {
        "list" => send(json!({ "cmd": "virtual.list" })),
        "ensure" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            let mut request = json!({ "cmd": "virtual.ensure", "name": name });
            if let Some(width) = option(args, "--width").and_then(|value| value.parse::<u32>().ok()) {
                request["width"] = json!(width);
            }
            if let Some(height) = option(args, "--height").and_then(|value| value.parse::<u32>().ok()) {
                request["height"] = json!(height);
            }
            if let Some(refresh) = option(args, "--refresh").and_then(|value| value.parse::<f64>().ok()) {
                request["refresh"] = json!(refresh);
            }
            if flag(args, "--hidpi") {
                request["hidpi"] = json!(true);
            }
            if flag(args, "--no-auto-create") {
                request["auto_create"] = json!(false);
            }
            send(request)
        }
        "bounds" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            send(json!({ "cmd": "virtual.bounds", "name": name }))
        }
        "rename" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            let Some(to) = option(args, "--to") else {
                return json!({ "ok": false, "error": "--to is required" });
            };
            send(json!({ "cmd": "virtual.rename", "name": name, "to": to }))
        }
        "reboot" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            send(json!({ "cmd": "virtual.reboot", "name": name }))
        }
        "remove" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            send(json!({ "cmd": "virtual.remove", "name": name }))
        }
        "preview" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            let show = !flag(args, "--hide");
            send(json!({ "cmd": "virtual.preview", "name": name, "show": show }))
        }
        "screenshot" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            let mut request = json!({ "cmd": "virtual.screenshot", "name": name });
            if let Some(path) = option(args, "--out") {
                request["path"] = json!(path);
            }
            send(request)
        }
        other => json!({ "ok": false, "error": format!("unknown virtual subcommand {other}") }),
    }
}

fn agent_command(args: &[String]) -> Value {
    let Some(command) = args.first().map(String::as_str) else {
        help();
        return json!({ "ok": false, "error": "agent needs a subcommand" });
    };
    match command {
        "start" | "chrome" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            let mut request = json!({ "cmd": "agent.start", "name": name });
            if let Some(url) = option(args, "--url") {
                request["url"] = json!(url);
            }
            if let Some(port) = option(args, "--port").and_then(|value| value.parse::<u16>().ok()) {
                request["port"] = json!(port);
            }
            if let Some(browser) = option(args, "--browser") {
                request["browser"] = json!(browser);
            }
            if flag(args, "--headless") {
                request["headless"] = json!(true);
            }
            if flag(args, "--windowed") {
                request["headless"] = json!(false);
            }
            send(request)
        }
        "stop" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            send(json!({ "cmd": "agent.stop", "name": name }))
        }
        "screenshot" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            let mut request = json!({ "cmd": "agent.screenshot", "name": name });
            if let Some(path) = option(args, "--out") {
                request["path"] = json!(path);
            }
            send(request)
        }
        "move" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            let mut request = json!({ "cmd": "agent.move", "name": name });
            for (flag, key) in [("--left", "left"), ("--top", "top"), ("--width", "width"), ("--height", "height")] {
                if let Some(value) = option(args, flag).and_then(|value| value.parse::<f64>().ok()) {
                    request[key] = json!(value);
                }
            }
            send(request)
        }
        "status" => {
            let Some(name) = option(args, "--name") else {
                return json!({ "ok": false, "error": "--name is required" });
            };
            send(json!({ "cmd": "agent.status", "name": name }))
        }
        other => json!({ "ok": false, "error": format!("unknown agent subcommand {other}") }),
    }
}

fn send(request: Value) -> Value {
    match try_send(&request) {
        Ok(reply) => reply,
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

fn try_send(request: &Value) -> Result<Value, String> {
    let path = super::control::socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|_| {
        "Lumen is not running: no control socket. Launch Lumen.app first.".to_string()
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    stream.flush().ok();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| format!("could not read the reply: {error}"))?;
    serde_json::from_str(&line).map_err(|error| format!("bad reply: {error}"))
}

fn help() {
    eprintln!(
        "lumen virtual list
lumen virtual ensure --name <name> [--width N] [--height N] [--hidpi] [--refresh N]
lumen virtual bounds --name <name>
lumen virtual screenshot --name <name> [--out path.png]
lumen virtual preview --name <name> [--hide]
lumen virtual rename --name <name> --to <new-name>
lumen virtual reboot --name <name>
lumen virtual remove --name <name>
lumen agent start --name <screen> [--url URL] [--headless|--windowed] [--port N] [--browser bundle-id|path.app]
lumen agent screenshot --name <screen> [--out page.png]
lumen agent move --name <screen> [--left X] [--top Y] [--width W] [--height H]
lumen agent stop --name <screen>
lumen agent status --name <screen>
lumen settings [--page general|displays|virtual|about] [--screen invocation|agent|main|last]
lumen settings state"
    );
}
