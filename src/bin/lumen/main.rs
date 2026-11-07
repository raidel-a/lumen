//! Lumen: a menu bar front end for DDC/CI display control.

mod icon;
mod menu;
mod state;

use lumen::ddc::{InputCode, InputProtocol, VCP_BRIGHTNESS};
use menu::{Action, MenuState, Released};
use state::DisplayEntry;
use tray_icon::{TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::WindowId;

#[derive(Debug)]
enum Event {
    Menu(muda::MenuId),
    Displays(Vec<DisplayEntry>),
    Message(String),
}

struct Lumen {
    proxy: EventLoopProxy<Event>,
    displays: Vec<DisplayEntry>,
    released: Vec<Released>,
    message: Option<String>,
    tray: Option<TrayIcon>,
}

impl Lumen {
    fn new(proxy: EventLoopProxy<Event>) -> Self {
        Self { proxy, displays: Vec::new(), released: Vec::new(), message: None, tray: None }
    }

    /// Rebuilds the native menu from the current state.
    fn draw_menu(&self) {
        if let Some(tray) = &self.tray {
            tray.set_menu(Some(Box::new(menu::build(&MenuState {
                displays: &self.displays,
                released: &self.released,
                message: self.message.as_deref(),
            }))));
        }
    }

    fn refresh(&self) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let displays = state::scan();
            let _ = proxy.send_event(Event::Displays(displays));
        });
    }

    fn switch_input(&self, display: u32, value: u16, protocol: InputProtocol) {
        let candidate = self
            .displays
            .iter()
            .find(|entry| entry.id == display)
            .and_then(|entry| {
                entry
                    .inputs
                    .iter()
                    .find(|input| input.value == value && input.protocol == protocol)
            })
            .cloned();

        let Some(input) = candidate else {
            return;
        };
        let input: InputCode = input;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let message = state::switch(display, &input);
            let _ = proxy.send_event(Event::Message(message));
            let _ = proxy.send_event(Event::Displays(state::scan()));
        });
    }

    fn set_brightness(&self, display: u32, value: u16) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let message = state::write_vcp(display, VCP_BRIGHTNESS, value);
            let _ = proxy.send_event(Event::Message(message));
            let _ = proxy.send_event(Event::Displays(state::scan()));
        });
    }

    fn release(&mut self, display: u32) {
        let name = self
            .displays
            .iter()
            .find(|entry| entry.id == display)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| format!("Display {display}"));
        if !self.released.iter().any(|entry| entry.id == display) {
            self.released.push(Released { id: display, name: name.clone() });
        }
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let message = state::release(display, &name);
            let _ = proxy.send_event(Event::Message(message));
            let _ = proxy.send_event(Event::Displays(state::scan()));
        });
    }

    fn reclaim(&mut self, display: u32) {
        let Some(index) = self.released.iter().position(|entry| entry.id == display) else {
            return;
        };
        let released = self.released.remove(index);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let message = state::reclaim(released.id, &released.name);
            let _ = proxy.send_event(Event::Message(message));
            let _ = proxy.send_event(Event::Displays(state::scan()));
        });
    }

    /// Reclaims everything on the way out so the Mac does not leave a display switched off.
    fn reclaim_all(&mut self) {
        for released in self.released.drain(..) {
            state::reclaim(released.id, &released.name);
        }
    }
}

impl ApplicationHandler<Event> for Lumen {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if self.tray.is_none() {
            self.tray = TrayIconBuilder::new()
                .with_icon_templated(icon::menu_bar_icon())
                .with_tooltip("Lumen - display control")
                .with_menu(Box::new(menu::build(&MenuState {
                    displays: &self.displays,
                    released: &self.released,
                    message: None,
                })))
                .with_menu_on_left_click(true)
                .build()
                .ok();
            self.refresh();
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, _event: WindowEvent) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Menu(id) => match menu::parse(&id) {
                Some(Action::Refresh) => self.refresh(),
                Some(Action::Quit) => {
                    self.reclaim_all();
                    event_loop.exit();
                }
                Some(Action::Switch { display, value, protocol }) => {
                    self.message = None;
                    self.switch_input(display, value, protocol);
                }
                Some(Action::Brightness { display, value }) => self.set_brightness(display, value),
                Some(Action::Release { display }) => self.release(display),
                Some(Action::Reclaim { display }) => self.reclaim(display),
                None => {}
            },
            Event::Displays(displays) => {
                self.displays = displays;
                // Drop release records for displays that came back on their own.
                self.released.retain(|released| {
                    !self.displays.iter().any(|display| display.id == released.id)
                });
                self.draw_menu();
            }
            Event::Message(message) => {
                self.message = Some(message);
                self.draw_menu();
            }
        }
    }
}

fn main() {
    let event_loop = EventLoop::<Event>::with_user_event()
        .with_activation_policy(ActivationPolicy::Accessory)
        .build()
        .expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let menu_proxy = proxy.clone();
    std::thread::spawn(move || {
        let receiver = muda::MenuEvent::receiver();
        while let Ok(event) = receiver.recv() {
            if menu_proxy.send_event(Event::Menu(event.id)).is_err() {
                break;
            }
        }
    });

    let mut app = Lumen::new(proxy);
    event_loop.run_app(&mut app).expect("run app");
}
