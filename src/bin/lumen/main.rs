//! Lumen: native menu bar, AppKit settings window.

mod config;
mod engine;
mod icon;
mod menu;
mod model;
mod ui;

use config::Protocol;
use menu::Action;
use model::Model;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use std::cell::RefCell;
use std::time::Duration;
use tray_icon::{TrayIcon, TrayIconBuilder};

thread_local! {
    static MODEL: RefCell<Model> = RefCell::new(Model::default());
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    /// Bumped on every display-change notification so a burst collapses into one scan.
    static RESCAN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Runs `job` on the main thread on the next run-loop turn.
fn on_main(job: impl FnOnce() + Send + 'static) {
    dispatch::Queue::main().exec_async(job);
}

pub fn with_model<R>(f: impl FnOnce(&mut Model) -> R) -> R {
    MODEL.with(|model| f(&mut model.borrow_mut()))
}

fn snapshot() -> Model {
    with_model(|model| model.snapshot())
}

pub fn redraw_menu() {
    let model = snapshot();
    TRAY.with(|slot| {
        if let Some(tray) = slot.borrow().as_ref() {
            tray.set_menu(Some(Box::new(menu::build(&model))));
        }
    });
}

/// Persist-then-refresh used by every settings action: menu now, window on the next turn.
pub fn after_change() {
    redraw_menu();
    on_main(ui::rebuild_if_open);
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
            after_change();
        });
    });
}

fn handle_menu(id: &str) {
    let Some(action) = menu::parse(id) else {
        return;
    };
    match action {
        Action::Refresh => spawn_scan(),
        Action::Settings => ui::open(),
        Action::Quit => {
            let reclaim = with_model(|model| {
                if model.config.reclaim_on_quit {
                    for released in model.released.drain(..) {
                        engine::reclaim(released.id, &released.name);
                    }
                }
            });
            let _ = reclaim;
            redraw_menu();
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
            std::thread::spawn(move || {
                let message = engine::switch(display, &input);
                on_main(move || {
                    with_model(|model| {
                        model.message = Some(message);
                        model.set_displays(engine::scan());
                    });
                    after_change();
                });
            });
        }
        Action::Brightness { display, value } => {
            std::thread::spawn(move || {
                let message = engine::write_vcp(display, lumen::ddc::VCP_BRIGHTNESS, value);
                on_main(move || {
                    with_model(|model| {
                        model.message = Some(message);
                        model.set_displays(engine::scan());
                    });
                    after_change();
                });
            });
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
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    unsafe { app.finishLaunching() };

    with_model(|model| model.config = config::Config::load());

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

    ui::init();

    let model = snapshot();
    let tray = TrayIconBuilder::new()
        .with_icon_templated(icon::menu_bar_icon())
        .with_tooltip("Lumen - display control")
        .with_menu(Box::new(menu::build(&model)))
        .with_menu_on_left_click(true)
        .build()
        .ok();
    TRAY.with(|slot| *slot.borrow_mut() = tray);

    spawn_scan();

    std::thread::spawn(|| {
        while let Ok(event) = muda::MenuEvent::receiver().recv() {
            let id = event.id.as_ref().to_string();
            on_main(move || handle_menu(&id));
        }
    });

    unsafe { app.run() };
}
