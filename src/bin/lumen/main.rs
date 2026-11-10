//! Lumen: native menu bar, AppKit settings window.

mod agent;
mod cdp;
mod cli;
mod config;
mod control;
mod engine;
mod login;
mod menu;
mod model;
mod status;
mod ui;
mod screens;

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

/// Refreshes the settings window after a model change; the menu rebuilds itself when it opens.
pub fn after_change() {
    on_main(ui::rebuild_if_open);
}

/// Queues a slider write; a burst of drag events collapses into one DDC write per control.
pub fn queue_control(display_id: u32, code: u8, value: u16) {
    if std::env::var_os("LUMEN_TRACE").is_some() {
        eprintln!("queue_control display={display_id} code={code:#04x} value={value}");
    }
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
            if std::env::var_os("LUMEN_TRACE").is_some() {
                eprintln!("rescan: display configuration changed");
            }
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
                        if std::env::var_os("LUMEN_TRACE").is_some() {
                            eprintln!("window recovery: {error}");
                        }
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
    let mut config = config::VirtualScreenConfig::default();
    config.name = name.to_string();
    config.width = width.clamp(640, 7680);
    config.height = height.clamp(480, 4320);
    config.hidpi = hidpi;
    let info = screens::ensure(&config)?;
    with_model(|model| model.config.upsert_virtual_screen(config));
    after_change();
    Ok(info)
}

pub fn run_action(action: Action) {
    match action {
        Action::Refresh => spawn_scan(),
        Action::Settings => ui::open(),
        Action::Quit => {
            let browsers = agent::stop_all();
            if std::env::var_os("LUMEN_TRACE").is_some() {
                eprintln!("stopped agent browsers: {browsers:?}");
            }
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
            if let Some(target) = ui::target() {
                if let Err(error) = screens::toggle_preview(&name, &target) {
                    with_model(|model| model.message = Some(error));
                }
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("--version") | Some("version")) {
        println!("Lumen {}", version_string());
        return;
    }
    if matches!(args.first().map(String::as_str), Some("virtual") | Some("agent")) {
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

    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    if std::env::var_os("LUMEN_TRACE").is_some() {
        unsafe {
            let bundle = objc2_foundation::NSBundle::mainBundle();
            eprintln!(
                "bundle id: {:?} path: {:?}",
                bundle.bundleIdentifier().map(|s| s.to_string()),
                bundle.bundlePath().to_string()
            );

        }
    }
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    unsafe { app.finishLaunching() };
    if std::env::var_os("LUMEN_TRACE").is_some() {
        eprintln!("login item at startup: {:?}", login::status());
    }

    with_model(|model| model.config = config::Config::load());

    // Agent browsers outlive a crash otherwise, on whatever display they drifted to.
    let orphans = with_model(|model| std::mem::take(&mut model.config.agent_sessions));
    if !orphans.is_empty() {
        let stopped = agent::reconcile(&orphans);
        with_model(|model| model.config.save());
        if std::env::var_os("LUMEN_TRACE").is_some() {
            eprintln!("closed {stopped} agent browser(s) from a previous run");
        }
    }

    // If a previous run died while a display was released, bring it back before anything else.
    let stale = with_model(|model| std::mem::take(&mut model.config.released));
    if !stale.is_empty() {
        for record in &stale {
            let _ = lumen::native::set_enabled(record.id, true);
        }
        if std::env::var_os("LUMEN_TRACE").is_some() {
            eprintln!("reclaimed {} display(s) from a previous run", stale.len());
        }
        with_model(|model| model.config.save());
    }

    let actions = ui::init();
    status::init(&actions);
    control::serve();

    spawn_scan();

    unsafe { app.run() };
}
