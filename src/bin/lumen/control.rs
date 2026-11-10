//! A local control socket for the CLI and for agent harnesses.
//!
//! The menu-bar app owns the virtual displays and the agent browsers, so it has to be the process
//! that answers requests about them. The socket is a Unix domain socket inside the app support
//! folder, mode 0600, and speaks one JSON request and one JSON response per connection.

use crate::config::{Config, VirtualScreenConfig};
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Bumped when the request or reply shape changes in a way a client could notice.
pub const PROTOCOL: u64 = 1;

pub fn socket_path() -> PathBuf {
    Config::support_dir().join("control.sock")
}

/// Whether a live instance is already serving the socket.
pub fn instance_running() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

/// Removes the socket on the way out.
pub fn shutdown() {
    let _ = std::fs::remove_file(socket_path());
}

fn ok(fields: Map<String, Value>) -> String {
    let mut fields = fields;
    fields.insert("ok".to_string(), Value::Bool(true));
    fields.insert("v".to_string(), json!(PROTOCOL));
    Value::Object(fields).to_string()
}

fn ok_value(value: Value) -> String {
    match value {
        Value::Object(fields) => ok(fields),
        other => ok(Map::from_iter([("result".to_string(), other)])),
    }
}

fn error(message: impl Into<String>) -> String {
    json!({ "ok": false, "v": PROTOCOL, "error": message.into() }).to_string()
}

/// Starts the listener once. Safe to call before the app run loop starts.
pub fn serve() {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if path.exists() {
        // A live app already owns the socket; a stale file from a crash can be replaced.
        if instance_running() {
            crate::log::warn("control socket already served by another instance");
            return;
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            crate::log::error(format!("control socket: {error}"));
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

fn handle(line: &str) -> String {
    let request: Value = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(parse_error) => return error(format!("bad request: {parse_error}")),
    };
    let Some(command) = request.get("cmd").and_then(Value::as_str) else {
        return error("missing cmd");
    };
    match command {
        "ping" => ok_value(json!({ "pid": std::process::id() })),
        "settings.open" => {
            let page = request.get("page").and_then(Value::as_str).map(str::to_string);
            let screen = request.get("screen").and_then(Value::as_str).map(str::to_string);
            let result = on_main_value(move || {
                crate::ui::open_command(page.as_deref(), screen.as_deref())
            });
            match result {
                Ok(Ok(())) => ok_value(json!({ "opened": true })),
                Ok(Err(message)) => error(message),
                Err(message) => error(message),
            }
        }
        "settings.state" => match on_main_value(|| crate::ui::state()) {
            Ok(state) => ok_value(state),
            Err(message) => error(message),
        },
        "virtual.list" => match on_main_value(|| crate::screens::list()) {
            Ok(screens) => ok_value(json!({ "screens": screens_json(&screens) })),
            Err(message) => error(message),
        },
        "virtual.preview" => {
            let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
                return error("missing name");
            };
            let show = request.get("show").and_then(Value::as_bool).unwrap_or(true);
            match on_main_value(move || {
                if !show {
                    crate::screens::stop_preview();
                    return Ok(());
                }
                match crate::ui::target() {
                    Some(target) => crate::screens::start_preview(&name, &target),
                    None => Err("the app is not ready for a preview".to_string()),
                }
            }) {
                Ok(Ok(())) => ok_value(json!({ "preview": show })),
                Ok(Err(message)) => error(message),
                Err(message) => error(message),
            }
        }
        "virtual.ensure" | "virtual.bounds" | "virtual.remove" | "virtual.screenshot" => {
            let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
                return error("missing name");
            };
            if let Err(message) = valid_name(&name) {
                return error(message);
            }
            match command {
                "virtual.ensure" => ensure(&request, name),
                "virtual.bounds" => match on_main_value(move || crate::screens::info(&name)) {
                    Ok(Some(info)) => ok_value(json!({ "screen": screen_json(&info) })),
                    Ok(None) => error("no such virtual screen"),
                    Err(message) => error(message),
                },
                "virtual.remove" => remove(&name),
                _ => {
                    let path = request
                        .get("path")
                        .and_then(Value::as_str)
                        .map(std::path::PathBuf::from);
                    match on_main_value(move || crate::screens::screenshot(&name, path.as_deref())) {
                        Ok(Ok(path)) => {
                            ok_value(json!({ "path": path.display().to_string() }))
                        }
                        Ok(Err(message)) => error(message),
                        Err(message) => error(message),
                    }
                }
            }
        }
        "virtual.rename" => {
            let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
                return error("missing name");
            };
            let Some(to) = request.get("to").and_then(Value::as_str).map(str::to_string) else {
                return error("missing to");
            };
            if let Err(message) = valid_name(&to) {
                return error(message);
            }
            let desired = on_main_value({
                let name = name.clone();
                move || crate::with_model(|model| model.config.virtual_screen(&name).cloned())
            });
            let mut desired = match desired {
                Ok(Some(config)) => config,
                Ok(None) => return error("no such virtual screen"),
                Err(message) => return error(message),
            };
            desired.name = to.clone();
            match on_main_value(move || crate::rename_screen(&name, &desired)) {
                Ok(Ok(())) => ok_value(json!({ "renamed": to })),
                Ok(Err(message)) => error(message),
                Err(message) => error(message),
            }
        }
        "virtual.reboot" => {
            let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
                return error("missing name");
            };
            let config = on_main_value({
                let name = name.clone();
                move || crate::with_model(|model| model.config.virtual_screen(&name).cloned())
            });
            let config = match config {
                Ok(Some(config)) => config,
                Ok(None) => return error("no such virtual screen"),
                Err(message) => return error(message),
            };
            match on_main_value(move || crate::screens::reboot(&config)) {
                Ok(Ok(info)) => ok_value(json!({ "screen": screen_json(&info) })),
                Ok(Err(message)) => error(message),
                Err(message) => error(message),
            }
        }
        "agent.start" | "agent.chrome" => agent_start(&request),
        "agent.stop" => agent_stop(&request),
        "agent.status" => agent_status(&request),
        "agent.move" => agent_move(&request),
        "agent.screenshot" => agent_screenshot(&request),
        other => error(format!("unknown command {other}")),
    }
}

pub(crate) fn valid_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("a screen needs a name".to_string());
    }
    if name.chars().count() > 64 {
        return Err("a screen name is limited to 64 characters".to_string());
    }
    if !name
        .chars()
        .all(|character| character.is_alphanumeric() || " -_.'".contains(character))
    {
        return Err("a screen name allows letters, digits, spaces, - _ . '".to_string());
    }
    Ok(())
}

fn ensure(request: &Value, name: String) -> String {
    let stored = on_main_value({
        let name = name.clone();
        move || crate::with_model(|model| model.config.virtual_screen(&name).cloned())
    });
    let stored = match stored {
        Ok(stored) => stored,
        Err(message) => return error(message),
    };
    let existing = stored.is_some();
    let mut config = stored.unwrap_or_else(|| {
        let mut config = VirtualScreenConfig::default();
        config.name = name.clone();
        config
    });
    config.name = name.clone();
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
    if !(640..=7680).contains(&config.width) || !(480..=4320).contains(&config.height) {
        return error("screen size must be between 640x480 and 7680x4320");
    }
    if !(1.0..=240.0).contains(&config.refresh) {
        return error("refresh must be between 1 and 240 Hz");
    }
    if config.hidpi && (config.width > 3840 || config.height > 2160) {
        return error("a 2x screen is limited to 3840x2160 logical points");
    }

    if !existing {
        let count = on_main_value(|| crate::screens::list().len()).unwrap_or(0);
        if count >= crate::screens::MAX_SCREENS {
            return error(format!(
                "at most {} virtual screens can exist at once",
                crate::screens::MAX_SCREENS
            ));
        }
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
        Ok(Ok(info)) => ok_value(json!({ "screen": screen_json(&info) })),
        Ok(Err(message)) => error(message),
        Err(message) => error(message),
    }
}

fn remove(name: &str) -> String {
    let name = name.to_string();
    let reply = on_main_value(move || crate::remove_screen(&name));
    match reply {
        Ok((removed, stopped)) if removed => {
            ok_value(json!({ "removed": true, "agent_stopped": stopped }))
        }
        Ok(_) => error("no such virtual screen"),
        Err(message) => error(message),
    }
}

pub(crate) fn agent_start(request: &Value) -> String {
    let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
        return error("missing name");
    };
    if let Err(message) = valid_name(&name) {
        return error(message);
    }
    let url = request.get("url").and_then(Value::as_str).map(str::to_string);
    let port = request
        .get("port")
        .and_then(Value::as_u64)
        .unwrap_or(crate::agent::DEFAULT_PORT as u64) as u16;
    let browser = request.get("browser").and_then(Value::as_str).map(str::to_string);
    let (default_headless, default_url) = on_main_value(|| {
        crate::with_model(|model| (model.config.agent_headless, model.config.agent_url.clone()))
    })
    .unwrap_or((true, None));
    let headless = request
        .get("headless")
        .and_then(Value::as_bool)
        .unwrap_or(default_headless);
    let url = url.or(default_url);

    // Reuse a live session instead of launching a second browser on the same profile.
    let existing = on_main_value({
        let name = name.clone();
        move || crate::agent::session(&name)
    });
    let mut reused_session = None;
    if let Ok(Some(session)) = existing {
        if session.headless == headless
            && crate::agent::session_alive(&session)
            && crate::agent::endpoint_alive(session.port)
        {
            reused_session = Some(session);
        } else {
            // Stale or mismatched session: close it and everything left on its profile before
            // launching again, otherwise the new browser forwards and its pid exits at once.
            let stale = name.clone();
            let _ = on_main_value(move || {
                let _ = crate::agent::stop(&stale);
                crate::with_model(|model| model.config.forget_agent_session(&stale));
            });
            crate::agent::stop_profile(&name);
        }
    }
    if let Some(session) = reused_session {
        let version = crate::agent::wait_for_cdp(session.port, Duration::from_millis(500))
            .unwrap_or(Value::Null);
        let endpoint = version.get("webSocketDebuggerUrl").cloned();
        return ok_value(json!({
            "reused": true,
            "session": crate::agent::session_json(&session, endpoint),
        }));
    }

    // Wait for any old browser to release the profile and the port.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5)
        && (crate::agent::endpoint_alive(port) || !crate::agent::profile_pids(&name).is_empty())
    {
        std::thread::sleep(Duration::from_millis(150));
    }

    let screen_name = name.clone();
    let info = match on_main_value(move || crate::screens::info(&screen_name)) {
        Ok(Some(info)) => info,
        Ok(None) => return error("no such virtual screen; call virtual.ensure first"),
        Err(message) => return error(message),
    };

    let launch = {
        let info = info.clone();
        let name = name.clone();
        let url = url.clone();
        let browser = browser.clone();
        match on_main_value(move || {
            crate::agent::launch(
                &info,
                &name,
                url.as_deref(),
                port,
                browser.as_deref(),
                headless,
            )
        }) {
            Ok(Ok(launch)) => launch,
            Ok(Err(message)) => return error(message),
            Err(message) => return error(message),
        }
    };

    let version = match crate::agent::wait_for_cdp(launch.port, Duration::from_secs(45)) {
        Ok(version) => version,
        Err(message) => return error(message),
    };
    let mut pid = crate::agent::wait_for_pid(&launch, Duration::from_secs(5));

    // A forwarded launch exits immediately and leaves the real browser on the profile.
    if !crate::agent::pid_alive(pid) {
        if let Some(actual) = crate::agent::profile_pids(&name)
            .into_iter()
            .find(|pid| crate::agent::pid_alive(*pid))
        {
            pid = actual;
        }
    }

    let session = crate::agent::new_session(&name, &launch, pid, url.clone(), headless);
    let record = crate::agent::session_record(&session);
    let remembered = crate::config::AgentLaunchRecord {
        name: name.clone(),
        url: url.clone(),
        headless,
        port: launch.port,
    };
    let registered = on_main_value({
        let session = session.clone();
        move || {
            crate::agent::register(session.clone());
            crate::with_model(|model| {
                model.config.record_agent_session(record);
                model.config.last_agent = Some(remembered);
                model.config.save();
            });
        }
    });
    if let Err(message) = registered {
        return error(message);
    }

    // Chromium activates when its window appears, which can happen after CDP is ready. A
    // headless session never shows a window, so there is nothing to restore.
    if !headless {
        if let Some(previous) = launch.previous.clone() {
        let browser = launch.browser.clone();
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(300));
            let previous = previous.clone();
            let browser = browser.clone();
            let restored = on_main_value(move || crate::agent::restore_front(&previous, &browser))
                .unwrap_or(false);
            if restored {
                break;
            }
        }
        }
    }

    ok_value(json!({
        "reused": false,
        "session": crate::agent::session_json(
            &session,
            version.get("webSocketDebuggerUrl").cloned(),
        ),
        "screen": screen_json(&info),
    }))
}

fn agent_stop(request: &Value) -> String {
    let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
        return error("missing name");
    };
    let reply = on_main_value({
        let name = name.clone();
        move || {
            let stopped = match crate::agent::stop(&name) {
                Ok(stopped) => stopped,
                Err(message) => return Err(message),
            };
            // Helpers and forwarded instances can outlive the main pid.
            crate::agent::stop_profile(&name);
            // Also close a persisted record left by a crash.
            let record = crate::with_model(|model| {
                model
                    .config
                    .agent_sessions
                    .iter()
                    .find(|entry| entry.name == name)
                    .cloned()
            });
            let recovered = record
                .as_ref()
                .map(|record| crate::agent::terminate_record(record).unwrap_or(false))
                .unwrap_or(false);
            crate::with_model(|model| model.config.forget_agent_session(&name));
            Ok(stopped || recovered)
        }
    });
    match reply {
        Ok(Ok(stopped)) => ok_value(json!({ "stopped": stopped })),
        Ok(Err(message)) => error(message),
        Err(message) => error(message),
    }
}

fn agent_status(request: &Value) -> String {
    let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
        return error("missing name");
    };
    let reply = on_main_value({
        let name = name.clone();
        move || {
            let session = crate::agent::session(&name);
            let screen = crate::screens::info(&name);
            let alive = session
                .as_ref()
                .map(crate::agent::session_alive)
                .unwrap_or(false);
            (session, screen, alive)
        }
    });
    match reply {
        Ok((session, screen, alive)) => {
            let headless = session
                .as_ref()
                .map(|session| session.headless)
                .unwrap_or(false);
            let (window, window_error) = if alive && !headless {
                match session.as_ref().map(|session| crate::cdp::window(session.port)) {
                    Some(Ok(window)) => (Some(window), Value::Null),
                    Some(Err(message)) => (None, json!(message)),
                    None => (None, Value::Null),
                }
            } else {
                (None, Value::Null)
            };
            let on_screen = match (&window, &screen) {
                (Some(window), Some(screen)) => {
                    window.inside(screen.x, screen.y, screen.width, screen.height)
                }
                _ => false,
            };
            let session_json = session
                .as_ref()
                .map(|session| crate::agent::session_json(session, None))
                .unwrap_or(Value::Null);
            ok_value(json!({
                "screen": screen.map(|info| screen_json(&info)).unwrap_or(Value::Null),
                "session": session_json,
                "alive": alive,
                "headless": headless,
                "window": window.map(|window| window.json()).unwrap_or(Value::Null),
                "window_error": window_error,
                "on_screen": on_screen,
            }))
        }
        Err(message) => error(message),
    }
}

fn agent_screenshot(request: &Value) -> String {
    let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
        return error("missing name");
    };
    let session = match on_main_value({
        let name = name.clone();
        move || crate::agent::session(&name)
    }) {
        Ok(Some(session)) => session,
        Ok(None) => return error("no agent session"),
        Err(message) => return error(message),
    };
    let path = match request.get("path").and_then(Value::as_str) {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0);
            Config::support_dir()
                .join("agents")
                .join(&name)
                .join(format!("page-{stamp}.png"))
        }
    };
    match crate::cdp::screenshot(session.port, &path) {
        Ok(()) => {
            let limit = on_main_value(|| crate::with_model(|model| model.config.capture_limit))
                .unwrap_or(Some(20));
            crate::screens::prune_captures(path.parent(), limit);
            ok_value(json!({ "path": path.display().to_string() }))
        }
        Err(message) => error(message),
    }
}

fn agent_move(request: &Value) -> String {
    let Some(name) = request.get("name").and_then(Value::as_str).map(str::to_string) else {
        return error("missing name");
    };
    let session = match on_main_value(move || crate::agent::session(&name)) {
        Ok(Some(session)) => session,
        Ok(None) => return error("no agent session"),
        Err(message) => return error(message),
    };
    let left = request.get("left").and_then(Value::as_f64).unwrap_or(0.0);
    let top = request.get("top").and_then(Value::as_f64).unwrap_or(0.0);
    let width = request.get("width").and_then(Value::as_f64).unwrap_or(1200.0);
    let height = request.get("height").and_then(Value::as_f64).unwrap_or(800.0);
    match crate::cdp::set_window(session.port, left, top, width, height) {
        Ok(window) => ok_value(json!({ "window": window.json() })),
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
