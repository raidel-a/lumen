//! A local control socket for the CLI and for agent harnesses.
//!
//! The menu-bar app owns the virtual displays, so it has to be the process that answers requests
//! about them. The socket is a Unix domain socket inside the app support folder, mode 0600, and
//! speaks one JSON request and one JSON response per connection.

use crate::config::{Config, VirtualScreenConfig};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

pub fn socket_path() -> PathBuf {
    Config::support_dir().join("control.sock")
}

/// Starts the listener once. Safe to call before the app run loop starts.
pub fn serve() {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if path.exists() {
        // A live app already owns the socket; a stale file from a crash can be replaced.
        if UnixStream::connect(&path).is_ok() {
            if std::env::var_os("LUMEN_TRACE").is_some() {
                eprintln!("control socket already served by another instance");
            }
            return;
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("control socket: {error}");
            return;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut line = String::new();
            {
                let mut reader = BufReader::new(&stream);
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
            }
            let reply = handle(&line);
            let mut stream = stream;
            let _ = writeln!(stream, "{reply}");
            let _ = stream.flush();
        }
    });
}

/// Runs `job` on the main thread and waits for its value.
fn on_main_value<T, F>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    crate::on_main(move || {
        let _ = sender.send(job());
    });
    receiver
        .recv_timeout(Duration::from_secs(20))
        .map_err(|_| "the app did not answer in time".to_string())
}

fn error(message: impl Into<String>) -> String {
    json!({ "ok": false, "error": message.into() }).to_string()
}

fn handle(line: &str) -> String {
    let request: Value = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(parse_error) => return error(format!("bad request: {parse_error}")),
    };
    let Some(command) = request.get("cmd").and_then(Value::as_str) else {
        return error("missing cmd");
    };
    match command {
        "virtual.list" => match on_main_value(|| crate::screens::list()) {
            Ok(screens) => json!({ "ok": true, "screens": screens_json(&screens) }).to_string(),
            Err(message) => error(message),
        },
        "virtual.ensure" | "virtual.bounds" | "virtual.remove" | "virtual.screenshot" => {
            let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
                return error("missing name");
            };
            match command {
                "virtual.ensure" => ensure(&request, name),
                "virtual.bounds" => match on_main_value(move || crate::screens::info(&name)) {
                    Ok(Some(info)) => json!({ "ok": true, "screen": screen_json(&info) }).to_string(),
                    Ok(None) => error("no such virtual screen"),
                    Err(message) => error(message),
                },
                "virtual.remove" => {
                    let reply = on_main_value(move || {
                        let removed = crate::screens::remove(&name);
                        crate::with_model(|model| model.config.remove_virtual_screen(&name));
                        crate::after_change();
                        removed
                    });
                    match reply {
                        Ok(removed) if removed => json!({ "ok": true, "removed": true }).to_string(),
                        Ok(_) => error("no such virtual screen"),
                        Err(message) => error(message),
                    }
                }
                _ => {
                    let path = request
                        .get("path")
                        .and_then(Value::as_str)
                        .map(std::path::PathBuf::from);
                    match on_main_value(move || crate::screens::screenshot(&name, path.as_deref())) {
                        Ok(Ok(path)) => {
                            json!({ "ok": true, "path": path.display().to_string() }).to_string()
                        }
                        Ok(Err(message)) => error(message),
                        Err(message) => error(message),
                    }
                }
            }
        }
        "agent.chrome" => chrome(&request),
        other => error(format!("unknown command {other}")),
    }
}

fn ensure(request: &Value, name: String) -> String {
    let mut config = crate::with_model(|model| model.config.virtual_screen(&name).cloned())
        .unwrap_or_else(|| {
            let mut config = VirtualScreenConfig::default();
            config.name = name.clone();
            config
        });
    config.name = name;
    if let Some(width) = request.get("width").and_then(Value::as_u64) {
        config.width = width as u32;
    }
    if let Some(height) = request.get("height").and_then(Value::as_u64) {
        config.height = height as u32;
    }
    if let Some(hidpi) = request.get("hidpi").and_then(Value::as_bool) {
        config.hidpi = hidpi;
    }
    if let Some(refresh) = request.get("refresh").and_then(Value::as_f64) {
        config.refresh = refresh;
    }
    if let Some(auto) = request.get("auto_create").and_then(Value::as_bool) {
        config.auto_create = auto;
    }

    let reply = on_main_value(move || {
        let result = crate::screens::ensure(&config);
        if result.is_ok() {
            crate::with_model(|model| model.config.upsert_virtual_screen(config.clone()));
            crate::after_change();
        }
        result
    });
    match reply {
        Ok(Ok(info)) => json!({ "ok": true, "screen": screen_json(&info) }).to_string(),
        Ok(Err(message)) => error(message),
        Err(message) => error(message),
    }
}

fn chrome(request: &Value) -> String {
    let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
        return error("missing name");
    };
    let url = request.get("url").and_then(Value::as_str).map(str::to_string);
    let port = request
        .get("port")
        .and_then(Value::as_u64)
        .unwrap_or(crate::agent::DEFAULT_PORT as u64) as u16;
    let browser = request.get("browser").and_then(Value::as_str).map(str::to_string);

    let info = match on_main_value(move || crate::screens::info(&name)) {
        Ok(Some(info)) => info,
        Ok(None) => return error("no such virtual screen; call virtual.ensure first"),
        Err(message) => return error(message),
    };

    let name_for_launch = info.name.clone();
    let launch = {
        let info = info.clone();
        let url = url.clone();
        let browser = browser.clone();
        match on_main_value(move || {
            crate::agent::launch(&info, &name_for_launch, url.as_deref(), port, browser.as_deref())
        }) {
            Ok(Ok(launch)) => launch,
            Ok(Err(message)) => return error(message),
            Err(message) => return error(message),
        }
    };

    // Polling happens here, on the socket thread, so the app stays responsive.
    match crate::agent::wait_for_cdp(launch.port, Duration::from_secs(45)) {
        Ok(version) => {
            // Chromium activates when its window appears, which can happen after CDP is ready.
            // Keep checking for a short while and put the user's app back once it does.
            if let Some(previous) = launch.previous.clone() {
                let browser = launch.browser.clone();
                for _ in 0..12 {
                    std::thread::sleep(Duration::from_millis(300));
                    let previous = previous.clone();
                    let browser = browser.clone();
                    let restored = on_main_value(move || {
                        crate::agent::restore_front(&previous, &browser)
                    })
                    .unwrap_or(false);
                    if restored {
                        break;
                    }
                }
            }
            json!({
            "ok": true,
            "port": launch.port,
            "endpoint": version.get("webSocketDebuggerUrl").cloned().unwrap_or(Value::Null),
            "browser": version.get("Browser").cloned().unwrap_or(Value::Null),
            "bundle": launch.browser,
            "profile": launch.profile.display().to_string(),
            "screen": screen_json(&info),
            })
            .to_string()
        }
        Err(message) => error(message),
    }
}

fn screen_json(info: &crate::screens::ScreenInfo) -> Value {
    json!({
        "name": info.name,
        "id": info.id,
        "x": info.x,
        "y": info.y,
        "width": info.width,
        "height": info.height,
        "pixel_width": info.pixel_width,
        "pixel_height": info.pixel_height,
        "scale": info.scale,
    })
}

fn screens_json(screens: &[crate::screens::ScreenInfo]) -> Vec<Value> {
    screens.iter().map(screen_json).collect()
}
