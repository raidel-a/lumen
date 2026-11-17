//! Lumen: native menu bar, AppKit settings window.

mod agent;
mod brightness;
mod cdp;
mod cli;
mod config;
mod control;
mod engine;
mod hotkeys;
mod hud;
mod instance;
mod log;
mod login;
mod menu;
mod model;
mod preview;
mod status;
mod ui;
mod screens;
mod updater;

use config::Protocol;
use menu::Action;
use model::Model;
use objc2::msg_send;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags,
};
use objc2_foundation::{MainThreadMarker, NSString};
use std::cell::RefCell;
use std::time::Duration;

thread_local! {
    static MODEL: RefCell<Model> = RefCell::new(Model::default());
    /// Bumped on every display-change notification so a burst collapses into one scan.
    static RESCAN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Slider values waiting for the debounce to write them.
    static PENDING_CONTROLS: RefCell<std::collections::HashMap<(u32, u8), u16>> =
        RefCell::new(std::collections::HashMap::new());
    static CONTROL_GEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The remembered agent session is reopened once, after the first display restore.
    static REOPEN_DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `job` on the main thread on the next run-loop turn.
/// Marketing version and build number from the app bundle, or "dev" outside one.
pub fn version_string() -> String {
    unsafe {
        let bundle = objc2_foundation::NSBundle::mainBundle();
        let Some(info) = bundle.infoDictionary() else {
            return "dev".to_string();
        };
        let read = |key: &str| -> Option<String> {
            let value = info.objectForKey(&NSString::from_str(key))?;
            let text: *const std::ffi::c_char = msg_send![&*value, UTF8String];
            if text.is_null() {
                return None;
            }
            Some(std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned())
        };
        let short = read("CFBundleShortVersionString").unwrap_or_else(|| "dev".to_string());
        match read("CFBundleVersion") {
            Some(build) if !build.is_empty() => format!("{short} ({build})"),
            _ => short,
        }
    }
}

pub(crate) fn on_main(job: impl FnOnce() + Send + 'static) {
    dispatch::Queue::main().exec_async(job);
}

pub fn with_model<R>(f: impl FnOnce(&mut Model) -> R) -> R {
    MODEL.with(|model| f(&mut model.borrow_mut()))
}

/// True while Option is held: sliders then follow the pointer exactly, without snapping.
pub fn fine_control() -> bool {
    unsafe {
        let flags = NSEvent::modifierFlags_class();
        flags.0 & NSEventModifierFlags::NSEventModifierFlagOption.0 != 0
    }
}

/// Slider value with a magnetic pull towards the tick points, or the exact value when `fine`.
///
/// Five ticks means snap points at 0, 25, 50, 75 and 100 percent; the pull reaches 3% of the
/// range either side, so a drag between ticks still holds any value.
pub fn snap_control(raw: f64, max: u16, fine: bool) -> u16 {
    let max = max.max(1) as f64;
    let value = raw.clamp(0.0, max);
    if fine {
        return value.round() as u16;
    }
    let step = max / 4.0;
    let threshold = max * 0.03;
    let nearest = (value / step).round() * step;
    if (value - nearest).abs() <= threshold {
        nearest.round() as u16
    } else {
        value.round() as u16
    }
}

/// Re-reads the internal panel's backlight, so a change made with the keyboard or Control Center
/// is visible the next time the menu or Settings opens. There is no polling timer.
pub fn refresh_builtin_brightness() {
    let ids: Vec<u32> = with_model(|model| {
        model.displays.iter().filter(|display| display.builtin).map(|display| display.id).collect()
    });
    let values: Vec<(u32, u16)> = ids
        .into_iter()
        .filter_map(|id| brightness::get_percent(id).map(|value| (id, value)))
        .collect();
    if values.is_empty() {
        return;
    }
    with_model(|model| {
        for (id, value) in values {
            if let Some(display) = model.displays.iter_mut().find(|display| display.id == id) {
                display.brightness = Some((value, 100));
            }
        }
    });
}

/// Refreshes the settings window after a model change; the menu rebuilds itself when it opens.
pub fn after_change() {
    on_main(ui::rebuild_if_open);
}

/// Queues a slider write; a burst of drag events collapses into one DDC write per control.
pub fn queue_control(display_id: u32, code: u8, value: u16) {
    crate::log::trace(format!(
        "queue_control display={display_id} code={code:#04x} value={value}"
    ));
    PENDING_CONTROLS.with(|pending| {
        pending.borrow_mut().insert((display_id, code), value);
    });
    let generation = CONTROL_GEN.with(|cell| {
        let next = cell.get() + 1;
        cell.set(next);
        next
    });
    dispatch::Queue::main().exec_after(Duration::from_millis(150), move || {
        if CONTROL_GEN.with(|cell| cell.get()) != generation {
            return;
        }
        let writes: Vec<((u32, u8), u16)> = PENDING_CONTROLS.with(|pending| {
            pending.borrow_mut().drain().collect()
        });
        if writes.is_empty() {
            return;
        }
        let echo = writes.clone();
        std::thread::spawn(move || {
            for ((id, code), value) in writes {
                let _ = engine::write_vcp_quiet(id, code, value);
            }
            on_main(move || {
                for ((id, code), value) in echo {
                    with_model(|model| model.apply_control(id, code, value));
                }
                after_change();
            });
        });
    });
}

/// Rescans after display changes and wake, coalescing bursts into one scan.
pub fn schedule_rescan() {
    let generation = RESCAN.with(|cell| {
        let next = cell.get() + 1;
        cell.set(next);
        next
    });
    dispatch::Queue::main().exec_after(Duration::from_millis(350), move || {
        if RESCAN.with(|cell| cell.get()) == generation {
            crate::log::trace("rescan: display configuration changed");
            spawn_scan();
        }
    });
}

fn spawn_scan() {
    std::thread::spawn(|| {
        let displays = engine::scan();
        on_main(move || {
            with_model(|model| model.set_displays(displays));
            // Screens that exist only in memory are recreated after launch, wake and display
            // changes. `restore` is a no-op for screens that are already up.
            let config = with_model(|model| model.config.clone());
            screens::restore(&config);
            after_change();

            // Bring the last agent back once, on the first restore, if asked.
            if !REOPEN_DONE.with(|cell| cell.replace(true)) {
                let launch = with_model(|model| {
                    if model.config.agent_reopen {
                        model.config.last_agent.clone()
                    } else {
                        None
                    }
                });
                if let Some(launch) = launch {
                    if screens::info(&launch.name).is_some() {
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(600));
                            let request = serde_json::json!({
                                "cmd": "agent.start",
                                "name": launch.name,
                                "url": launch.url,
                                "headless": launch.headless,
                                "port": launch.port,
                            });
                            let reply = control::agent_start(&request);
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&reply) {
                                if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
                                    let message = value
                                        .get("error")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or("agent could not be reopened")
                                        .to_string();
                                    crate::log::warn(format!("agent reopen: {message}"));
                                    on_main(move || {
                                        with_model(|model| model.message = Some(message));
                                        after_change();
                                    });
                                }
                            }
                        });
                    }
                }
            }

            // A display that comes back after wake can leave the agent window on another
            // screen. Move it home once the arrangement has settled.
            for session in agent::sessions() {
                if session.headless || !agent::session_alive(&session) {
                    continue;
                }
                let Some(info) = screens::info(&session.name) else {
                    continue;
                };
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1200));
                    if let Err(error) =
                        cdp::place_window(session.port, info.x, info.y, info.width, info.height)
                    {
                        crate::log::trace(format!("window recovery: {error}"));
                    }
                });
            }
        });
    });
}

/// Switches an input on a worker and records whether the attempt actually worked.
pub fn switch_input(display_id: u32, input: lumen::ddc::InputCode, key: Option<String>) {
    let value = input.value;
    let protocol = Protocol::from_ddc(input.protocol);
    std::thread::spawn(move || {
        let outcome = engine::switch(display_id, &input);
        let switched = outcome.switched;
        on_main(move || {
            with_model(|model| {
                model.message = Some(outcome.message);
                model.set_displays(engine::scan());
                if let Some(key) = key {
                    if switched {
                        model.remember_input(&key, value, protocol);
                    }
                    model.config.mark_input(&key, value, protocol, switched);
                }
            });
            after_change();
        });
    });
}

/// Writes one VCP control and folds the value into the model without a full rescan.
pub fn apply_control(display_id: u32, code: u8, value: u16) {
    std::thread::spawn(move || {
        let message = engine::write_vcp(display_id, code, value);
        on_main(move || {
            with_model(|model| {
                model.message = Some(message);
                model.apply_control(display_id, code, value);
            });
            after_change();
        });
    });
}

/// Removes a virtual screen and closes the browser started on it. Main thread.
pub fn remove_screen(name: &str) -> (bool, bool) {
    preview::close(name);
    let _mutation = screens::begin_mutation();
    let stopped = agent::stop(name).unwrap_or(false);
    let removed = screens::remove(name);
    with_model(|model| {
        model.config.remove_virtual_screen(name);
        model.config.forget_agent_session(name);
    });
    after_change();
    (removed, stopped)
}

/// Creates a screen with the given geometry. Main thread.
pub fn create_screen(name: &str, width: u32, height: u32, hidpi: bool) -> Result<screens::ScreenInfo, String> {
    let defaults = with_model(|model| model.config.virtual_defaults.clone());
    let mut config = config::VirtualScreenConfig::default();
    config.name = name.to_string();
    config.width = width.clamp(640, 7680);
    config.height = height.clamp(480, 4320);
    config.hidpi = hidpi;
    config.refresh = if defaults.refresh > 0.0 { defaults.refresh } else { 60.0 };
    create_screen_with(config)
}

/// Creates a screen from a complete configuration and records it. Main thread.
pub fn create_screen_with(config: config::VirtualScreenConfig) -> Result<screens::ScreenInfo, String> {
    let _mutation = screens::begin_mutation();
    let info = screens::ensure(&config)?;
    with_model(|model| model.config.upsert_virtual_screen(config));
    after_change();
    Ok(info)
}

/// Renames a live virtual display. The display is recreated because `CGVirtualDisplay` takes its
/// name at creation, and any agent browser on it is closed first. On failure the old display is
/// recreated and the error says so.
pub fn rename_screen(old: &str, desired: &config::VirtualScreenConfig) -> Result<(), String> {
    let new = desired.name.trim();
    if old == new {
        return Ok(());
    }
    control::valid_name(new)?;
    let old_config = with_model(|model| model.config.virtual_screen(old).cloned())
        .ok_or_else(|| format!("no virtual screen named {old}"))?;
    if with_model(|model| model.config.virtual_screen(new).is_some()) {
        return Err(format!("a virtual screen named {new} already exists"));
    }

    let _mutation = screens::begin_mutation();

    if preview::is_open(old) {
        preview::close(old);
    }
    let _ = agent::stop(old);
    agent::stop_profile(old);
    with_model(|model| model.config.forget_agent_session(old));
    // Give a terminating browser a moment to let go of its profile before the folder moves.
    for _ in 0..20 {
        if agent::profile_pids(old).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let old_dir = agent::profile_dir(old)
        .parent()
        .map(std::path::PathBuf::from);
    let new_dir = agent::profile_dir(new)
        .parent()
        .map(std::path::PathBuf::from);
    let mut moved = false;
    if let (Some(old_dir), Some(new_dir)) = (&old_dir, &new_dir) {
        if old_dir.exists() {
            if new_dir.exists() {
                return Err(format!("a capture folder named {new} already exists"));
            }
            if let Some(parent) = new_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::rename(old_dir, new_dir)
                .map_err(|error| format!("could not move the capture folder: {error}"))?;
            moved = true;
        }
    }

    screens::remove_and_wait(old);
    let mut config = desired.clone();
    config.name = new.to_string();
    match screens::ensure(&config) {
        Ok(_) => {
            with_model(|model| {
                model.config.rename_virtual_screen(old, config);
                model.message = Some(format!("Renamed {old} to {new}"));
            });
            after_change();
            Ok(())
        }
        Err(error) => {
            if moved {
                if let (Some(old_dir), Some(new_dir)) = (&old_dir, &new_dir) {
                    let _ = std::fs::rename(new_dir, old_dir);
                }
            }
            let restored = screens::ensure(&old_config);
            with_model(|model| {
                model.config.rename_virtual_screen(new, old_config);
            });
            after_change();
            match restored {
                Ok(_) => Err(format!("rename failed: {error}; the old display was restored")),
                Err(restore_error) => Err(format!(
                    "rename failed: {error}; restoring the old display also failed: {restore_error}"
                )),
            }
        }
    }
}

pub fn run_action(action: Action) {
    match action {
        Action::Refresh => spawn_scan(),
        Action::Settings => {
            let point = unsafe { NSEvent::mouseLocation() };
            ui::open(Some(point), None);
        }
        Action::CheckForUpdates => updater::check_now(),
        Action::Quit => {
            preview::close_all();
            let browsers = agent::stop_all();
            crate::log::info(format!("stopped agent browsers: {browsers:?}"));
            with_model(|model| {
                model.config.agent_sessions.clear();
                model.config.save();
            });
            control::shutdown();
            let reclaim = with_model(|model| {
                if model.config.reclaim_on_quit {
                    for released in model.released.drain(..) {
                        engine::reclaim(released.id, &released.name);
                    }
                }
            });
            let _ = reclaim;
            if let Some(mtm) = MainThreadMarker::new() {
                unsafe { NSApplication::sharedApplication(mtm).terminate(None) };
            }
        }
        Action::Switch { display, value, protocol } => {
            let label = with_model(|model| {
                model
                    .display(display)
                    .and_then(|entry| {
                        model
                            .config
                            .enabled_inputs(&entry.key)
                            .into_iter()
                            .find(|input| input.value == value && input.protocol == Protocol::from_ddc(protocol))
                    })
                    .map(|input| format!("{} ({})", input.label, input.protocol.label()))
            })
            .unwrap_or_else(|| format!("input {value:#06x}"));

            let input = lumen::ddc::InputCode { label, value, protocol };
            let key = with_model(|model| model.display(display).map(|entry| entry.key.clone()));
            switch_input(display, input, key);
        }
        Action::Brightness { display, value } => {
            apply_control(display, lumen::ddc::VCP_BRIGHTNESS, value)
        }
        Action::Contrast { display, value } => {
            apply_control(display, lumen::ddc::VCP_CONTRAST, value)
        }
        Action::Volume { display, value } => apply_control(display, lumen::ddc::VCP_VOLUME, value),
        Action::RemoveScreen { name } => {
            remove_screen(&name);
        }
        Action::Preview { name } => {
            if let Err(error) = preview::toggle(&name) {
                with_model(|model| model.message = Some(error));
            }
        }
        Action::Mute { display, muted } => {
            apply_control(display, engine::VCP_MUTE, if muted { 1 } else { 2 })
        }
        Action::Release { key } => {
            let name = with_model(|model| {
                let Some(id) = model.display_by_key(&key).map(|display| display.id) else {
                    return None;
                };
                let name = model.display(id).map(|display| display.name.clone()).unwrap_or_default();
                model.mark_released(id, &name);
                Some((id, name, model.config.auto_reclaim_minutes))
            });
            let Some((id, name, auto)) = name else {
                return;
            };
            after_change();
            std::thread::spawn(move || {
                let message = engine::release(id, &name);
                on_main(move || {
                    with_model(|model| {
                        model.message = Some(message);
                        model.set_displays(engine::scan());
                    });
                    after_change();
                });

                if let Some(minutes) = auto {
                    std::thread::sleep(Duration::from_secs(u64::from(minutes) * 60));
                    let message = engine::reclaim(id, &name);
                    on_main(move || {
                        with_model(|model| {
                            model.forget_released(id);
                            model.message = Some(message);
                            model.set_displays(engine::scan());
                        });
                        after_change();
                    });
                }
            });
        }
        Action::Reclaim { key } => {
            let released = with_model(|model| {
                let entry = model.released.iter().find(|entry| entry.key == key).cloned();
                if let Some(entry) = &entry {
                    model.forget_released(entry.id);
                }
                entry
            });
            let Some(released) = released else {
                return;
            };
            after_change();
            std::thread::spawn(move || {
                let message = engine::reclaim(released.id, &released.name);
                on_main(move || {
                    with_model(|model| {
                        model.message = Some(message);
                        model.set_displays(engine::scan());
                    });
                    after_change();
                });
            });
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // A dev bundle launched through LaunchServices gets its support directory here. Setting the
    // environment before anything reads it keeps config, socket and agents isolated.
    if let Some(index) = args.iter().position(|arg| arg == "--support-dir") {
        if let Some(dir) = args.get(index + 1).cloned() {
            unsafe {
                std::env::set_var("LUMEN_SUPPORT_DIR", &dir);
            }
        }
        args.drain(index..(index + 2).min(args.len()));
    }
    if matches!(args.first().map(String::as_str), Some("--version") | Some("version")) {
        println!("Lumen {}", version_string());
        return;
    }
    if matches!(
        args.first().map(String::as_str),
        Some("virtual") | Some("agent") | Some("settings") | Some("quit")
    ) {
        std::process::exit(cli::run(&args));
    }

    if let Some(command) = std::env::args().nth(1) {
        match command.as_str() {
            "--login-status" => {
                println!("{:?}", login::status());
                return;
            }
            "--login-register" => {
                println!("register: {:?}", login::register());
                println!("status: {:?}", login::status());
                return;
            }
            "--login-unregister" => {
                println!("unregister: {:?}", login::unregister());
                println!("status: {:?}", login::status());
                return;
            }
            _ => {}
        }
    }

    // Last, so the CLI subcommands and login flags still work while the app is running.
    if control::instance_running() {
        eprintln!("Lumen is already running");
        std::process::exit(0);
    }
    let _instance = match instance::acquire() {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("Lumen is already running ({error})");
            std::process::exit(0);
        }
    };

    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    unsafe {
        let bundle = objc2_foundation::NSBundle::mainBundle();
        crate::log::info(format!(
            "bundle id: {:?} path: {:?} support: {:?}",
            bundle.bundleIdentifier().map(|s| s.to_string()),
            bundle.bundlePath().to_string(),
            config::Config::support_dir()
        ));
    }
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    unsafe { app.finishLaunching() };
    crate::log::trace(format!("login item at startup: {:?}", login::status()));

    with_model(|model| model.config = config::Config::load());

    // Agent browsers outlive a crash otherwise, on whatever display they drifted to.
    let orphans = with_model(|model| std::mem::take(&mut model.config.agent_sessions));
    if !orphans.is_empty() {
        let stopped = agent::reconcile(&orphans);
        with_model(|model| model.config.save());
        crate::log::info(format!("closed {stopped} agent browser(s) from a previous run"));
    }

    // If a previous run died while a display was released, bring it back before anything else.
    let stale = with_model(|model| std::mem::take(&mut model.config.released));
    if !stale.is_empty() {
        for record in &stale {
            let _ = lumen::native::set_enabled(record.id, true);
        }
        crate::log::info(format!(
            "reclaimed {} display(s) from a previous run",
            stale.len()
        ));
        with_model(|model| model.config.save());
    }

    let actions = ui::init();
    status::init(&actions);
    control::serve();

    spawn_scan();

    // Install the brightness-key event tap if the user enabled it and granted Accessibility.
    hotkeys::refresh();

    unsafe { app.run() };
}
