//! Lumen: native menu bar, Rune-rendered settings.

mod config;
mod engine;
mod icon;
mod menu;
mod model;
mod settings;

use config::Protocol;
use menu::Action;
use rune::{AppSpec, AppHandle, Cx, El, WindowOptions};
use settings::AppState;
use std::cell::RefCell;
use std::time::Duration;
use tray_icon::{TrayIcon, TrayIconBuilder};

thread_local! {
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

fn view(state: &AppState, cx: &mut Cx<AppState>) -> El {
    settings::view(state, cx)
}

pub(crate) fn draw_menu(state: &AppState) {
    TRAY.with(|slot| {
        if let Some(tray) = slot.borrow().as_ref() {
            tray.set_menu(Some(Box::new(menu::build(&state.model))));
        }
    });
}

fn spawn_scan(handle: AppHandle<AppState>) {
    std::thread::spawn(move || {
        let displays = engine::scan();
        handle.update(move |state, _tx| {
            state.model.set_displays(displays);
            draw_menu(state);
        });
    });
}

/// Centres the settings window on the main display, in physical pixels as Rune expects.
fn settings_position() -> (i32, i32) {
    use core_graphics::display::CGDisplay;

    let display = CGDisplay::main();
    let bounds = display.bounds();
    let scale = display
        .display_mode()
        .map(|mode| mode.pixel_width() as f64 / mode.width().max(1) as f64)
        .unwrap_or(2.0)
        .max(1.0);
    let x = (bounds.size.width - 720.0) / 2.0 * scale;
    let y = (bounds.size.height - 540.0) / 3.0 * scale;
    (x.max(0.0) as i32, y.max(0.0) as i32)
}

/// Sample content for headless layout checks (`--scene displays-full`).
fn fake_displays(state: &mut AppState) {
    use config::{DisplayConfig, InputSetting, Protocol};
    use engine::DisplayEntry;

    state.page = settings::Page::Displays;
    state.model.displays = vec![DisplayEntry {
        id: 99,
        key: "1e6d:5bf8:0007453a".to_string(),
        name: "LG ULTRAWIDE".to_string(),
        inputs: Vec::new(),
        brightness: Some((63, 100)),
        contrast: Some((70, 100)),
        volume: Some((85, 100)),
    }];
    state.model.config.displays = vec![DisplayConfig {
        key: "1e6d:5bf8:0007453a".to_string(),
        name: "LG ULTRAWIDE".to_string(),
        show_in_menu: true,
        inputs: vec![
            InputSetting { label: "HDMI 1".into(), value: 0x11, protocol: Protocol::Standard, enabled: true },
            InputSetting { label: "HDMI 2".into(), value: 0x12, protocol: Protocol::Standard, enabled: true },
            InputSetting { label: "DisplayPort".into(), value: 0x0f, protocol: Protocol::Standard, enabled: false },
            InputSetting { label: "HDMI 1".into(), value: 0x90, protocol: Protocol::LgAlt, enabled: true },
            InputSetting { label: "HDMI 2".into(), value: 0x91, protocol: Protocol::LgAlt, enabled: true },
        ],
    }];
}

fn main() {
    let app = rune::App::new(AppSpec {
        scenes: &[
            ("displays", |state: &mut AppState| state.page = settings::Page::Displays),
            ("displays-full", fake_displays),
        ],
        ..AppSpec::new("Lumen Settings", (720.0, 540.0), AppState::default, view)
    })
    .keep_running()
    .accessory()
    .window_options(WindowOptions { visible: false, ..Default::default() });
    let handle = app.handle();

    let tray = TrayIconBuilder::new()
        .with_icon_templated(icon::menu_bar_icon())
        .with_tooltip("Lumen - display control")
        .with_menu(Box::new(menu::build(&Default::default())))
        .with_menu_on_left_click(true)
        .build()
        .ok();
    TRAY.with(|slot| *slot.borrow_mut() = tray);

    spawn_scan(handle.clone());

    let menu_handle = handle.clone();
    std::thread::spawn(move || {
        while let Ok(event) = muda::MenuEvent::receiver().recv() {
            let Some(action) = menu::parse(&event.id) else {
                continue;
            };
            match action {
                Action::Refresh => spawn_scan(menu_handle.clone()),
                Action::Settings => {
                    let open_handle = menu_handle.clone();
                    menu_handle.update(move |_state, _tx| {
                        open_handle.set_position(settings_position());
                        open_handle.open();
                    });
                }
                Action::Quit => {
                    let quit_handle = menu_handle.clone();
                    menu_handle.update(move |state, tx| {
                        if state.model.config.reclaim_on_quit {
                            for released in state.model.released.drain(..) {
                                engine::reclaim(released.id, &released.name);
                            }
                        }
                        draw_menu(state);
                        tx.quit();
                    });
                    let _ = quit_handle;
                }
                Action::Switch { display, value, protocol } => {
                    let worker = menu_handle.clone();
                    menu_handle.update(move |state, _tx| {
                        let Some(entry) = state.model.display(display) else {
                            return;
                        };
                        let label = state
                            .model
                            .config
                            .enabled_inputs(&entry.key)
                            .into_iter()
                            .find(|input| {
                                input.value == value
                                    && input.protocol == Protocol::from_ddc(protocol)
                            })
                            .map(|input| format!("{} ({})", input.label, input.protocol.label()))
                            .unwrap_or_else(|| format!("input {value:#06x}"));
                        let input = lumen::ddc::InputCode { label, value, protocol };
                        let worker = worker.clone();
                        std::thread::spawn(move || {
                            let message = engine::switch(display, &input);
                            worker.update(move |state, _tx| {
                                state.model.message = Some(message);
                                state.model.set_displays(engine::scan());
                                draw_menu(state);
                            });
                        });
                    });
                }
                Action::Brightness { display, value } => {
                    let worker = menu_handle.clone();
                    std::thread::spawn(move || {
                        let message = engine::write_vcp(display, lumen::ddc::VCP_BRIGHTNESS, value);
                        worker.update(move |state, _tx| {
                            state.model.message = Some(message);
                            state.model.set_displays(engine::scan());
                            draw_menu(state);
                        });
                    });
                }
                Action::Release { key } => {
                    let worker = menu_handle.clone();
                    menu_handle.update(move |state, _tx| {
                        let Some(id) = state.model.display_by_key(&key).map(|d| d.id) else {
                            return;
                        };
                        let name = state
                            .model
                            .display(id)
                            .map(|d| d.name.clone())
                            .unwrap_or_else(|| format!("Display {id}"));
                        state.model.mark_released(id, &name);
                        let minutes = state.model.config.auto_reclaim_minutes;
                        draw_menu(state);

                        let worker = worker.clone();
                        let timer = worker.clone();
                        std::thread::spawn(move || {
                            let message = engine::release(id, &name);
                            worker.update(move |state, _tx| {
                                state.model.message = Some(message);
                                state.model.set_displays(engine::scan());
                                draw_menu(state);
                            });

                            if let Some(minutes) = minutes {
                                std::thread::sleep(Duration::from_secs(u64::from(minutes) * 60));
                                let message = engine::reclaim(id, &name);
                                timer.update(move |state, _tx| {
                                    state.model.released.retain(|entry| entry.id != id);
                                    state.model.message = Some(message);
                                    state.model.set_displays(engine::scan());
                                    draw_menu(state);
                                });
                            }
                        });
                    });
                }
                Action::Reclaim { key } => {
                    let worker = menu_handle.clone();
                    menu_handle.update(move |state, _tx| {
                        let Some(released) =
                            state.model.released.iter().find(|entry| entry.key == key).cloned()
                        else {
                            return;
                        };
                        state.model.released.retain(|entry| entry.key != key);
                        draw_menu(state);

                        let worker = worker.clone();
                        std::thread::spawn(move || {
                            let message = engine::reclaim(released.id, &released.name);
                            worker.update(move |state, _tx| {
                                state.model.message = Some(message);
                                state.model.set_displays(engine::scan());
                                draw_menu(state);
                            });
                        });
                    });
                }
            }
        }
    });

    app.run();
}
