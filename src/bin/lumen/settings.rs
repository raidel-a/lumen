//! The Rune-rendered settings window.

use crate::config::{Config, InputSetting, Protocol};
use crate::model::Model;
use rune::tok::{self, radius, space, type_};
use rune::{Cx, El, TextEdit, col, el, row, text, text_input};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Displays,
}

pub struct AppState {
    pub model: Model,
    pub page: Page,
    pub selected: usize,
    pub draft_label: TextEdit,
    pub draft_value: TextEdit,
    pub draft_protocol: Protocol,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            model: Model { config: Config::load(), ..Default::default() },
            page: Page::General,
            selected: 0,
            draft_label: TextEdit::new(""),
            draft_value: TextEdit::new(""),
            draft_protocol: Protocol::LgAlt,
        }
    }
}

pub fn view(state: &AppState, cx: &mut Cx<AppState>) -> El {
    let t = tok::theme(cx.system_dark());
    col()
        .key("root")
        .w_full()
        .h_full()
        .padding(space::S6)
        .gap(space::S3)
        .fill(t.bg)
        .child(header(state, t))
        .child(match state.page {
            Page::General => general_page(state, t),
            Page::Displays => displays_page(state, t),
        })
}

fn header(state: &AppState, t: tok::Theme) -> El {
    row()
        .w_full()
        .gap(space::S2)
        .center_y()
        .child(text("Lumen").key("title").font(type_::TITLE).color(t.text))
    .child(
        col().grow(1.0).child(
            text("Menu bar display control")
                .font(type_::BODY)
                .color(t.text_secondary),
        ),
    )
    .child(tab("General", Page::General, state.page, t))
    .child(tab("Displays", Page::Displays, state.page, t))
}

fn tab(label: &str, page: Page, current: Page, t: tok::Theme) -> El {
    let active = page == current;
    row()
        .key(format!("tab-{label}"))
        .role("button")
        .h(26.0)
        .px(space::S3)
        .radius(radius::MD)
        .center()
        .fill(if active { tok::accent(0, true) } else { t.card })
        .border(1.0, t.card_border)
        .on_press(move |s: &mut AppState| s.page = page)
        .child(
            text(label)
                .font(type_::LABEL)
                .color(if active { t.knob } else { t.text_secondary }),
        )
}

fn general_page(state: &AppState, t: tok::Theme) -> El {
    let config = &state.model.config;
    col()
        .key("general")
        .w_full()
        .gap(space::S2)
        .child(section("Behaviour", t))
        .child(setting_row(
            "Reclaim displays on quit",
            "Bring every released display back before Lumen exits.",
            switch(config.reclaim_on_quit, t, |s| {
                s.model.config.reclaim_on_quit = !s.model.config.reclaim_on_quit;
                changed(s);
            }),
            t,
        ))
        .child(setting_row(
            "Auto-reclaim",
            "Bring a released display back by itself after a while.",
            auto_reclaim_buttons(config.auto_reclaim_minutes, t),
            t,
        ))
        .child(setting_row(
            "Show brightness presets",
            "List 25/50/75/100% under each display in the menu.",
            switch(config.show_brightness, t, |s| {
                s.model.config.show_brightness = !s.model.config.show_brightness;
                changed(s);
            }),
            t,
        ))
        .child(section("About", t))
        .child(
            row()
                .w_full()
                .padding(space::S3)
                .radius(radius::LG)
                .fill(t.card)
                .border(1.0, t.card_border)
                .child(
                    col().grow(1.0).child(
                        text(
                            "Lumen switches inputs over DDC. When a monitor's firmware ignores \
                             input select, Release cuts the signal instead and the monitor follows \
                             its other live input.",
                        )
                        .font(type_::BODY)
                        .color(t.text_secondary)
                        .wrap(),
                    ),
                ),
        )
}

fn auto_reclaim_buttons(current: Option<u32>, t: tok::Theme) -> El {
    let mut bar = row().gap(space::S1).center_y();
    for (label, value) in
        [("Off", None), ("1 min", Some(1u32)), ("5 min", Some(5)), ("15 min", Some(15))]
    {
        let active = current == value;
        bar = bar.child(
            row()
                .role("button")
                .h(24.0)
                .px(space::S2)
                .radius(radius::SM)
                .center()
                .fill(if active { tok::accent(0, true) } else { t.bg })
                .border(1.0, t.card_border)
                .on_press(move |s: &mut AppState| {
                    s.model.config.auto_reclaim_minutes = value;
                    changed(s);
                })
                .child(
                    text(label)
                        .font(type_::LABEL)
                        .color(if active { t.knob } else { t.text_secondary }),
                ),
        );
    }
    bar
}

fn displays_page(state: &AppState, t: tok::Theme) -> El {
    let list = state
        .model
        .displays
        .iter()
        .enumerate()
        .fold(col().key("display-list").w(190.0).gap(2.0), |list, (index, display)| {
            list.child(display_tab(display.name.clone(), index == state.selected, index, t))
        });

    let list = if state.model.displays.is_empty() {
        list.child(
            text("No DDC displays")
                .font(type_::BODY)
                .color(t.text_secondary),
        )
    } else {
        list
    };

    row()
        .key("displays")
        .w_full()
        .grow(1.0)
        .gap(space::S3)
        .child(list)
        .child(editor(state, t))
}

fn display_tab(name: String, selected: bool, index: usize, t: tok::Theme) -> El {
    row()
        .key(format!("display-tab-{index}"))
        .role("button")
        .w_full()
        .h(30.0)
        .px(space::S2)
        .radius(radius::MD)
        .center_y()
        .fill(if selected { t.card } else { t.bg })
        .border(if selected { 1.0 } else { 0.0 }, t.card_border)
        .on_press(move |s: &mut AppState| s.selected = index)
        .child(
            text(name)
                .font(type_::LABEL)
                .color(if selected { t.text } else { t.text_secondary }),
        )
}

fn editor(state: &AppState, t: tok::Theme) -> El {
    let Some(display) = state.model.displays.get(state.selected) else {
        return col().key("editor").grow(1.0).child(
            text("Connect a display to configure its inputs.")
                .font(type_::BODY)
                .color(t.text_secondary),
        );
    };

    let key = display.key.clone();
    let name = display.name.clone();
    let show_in_menu = state
        .model
        .config
        .display(&key)
        .map(|config| config.show_in_menu)
        .unwrap_or(true);
    let released = state.model.released(&key);
    let inputs = state
        .model
        .config
        .display(&key)
        .map(|config| config.inputs.clone())
        .unwrap_or_default();

    let mut body = col().key("editor").grow(1.0).gap(space::S2);
    body = body
        .child(
            row()
                .w_full()
                .center_y()
                .child(text(name).font(type_::TITLE).color(t.text))
                .child(col().grow(1.0).child(el().size(1.0, 1.0)))
                .child(switch(show_in_menu, t, move |s| {
                    let key = s.key_of_selected();
                    if let Some(config) = s.model.config.display_mut(&key) {
                        config.show_in_menu = !config.show_in_menu;
                    }
                    changed(s);
                })),
        )
        .child(
            text(format!("{}  ·  {}", key, display_key_hint(released)))
                .font(type_::MONO_CAPTION)
                .color(t.text_secondary),
        )
        .child(section("Menu inputs", t));

    if inputs.is_empty() {
        body = body.child(
            text("Nothing enabled yet.")
                .font(type_::BODY)
                .color(t.text_secondary),
        );
    }

    for (index, input) in inputs.iter().enumerate() {
        body = body.child(input_row(index, input, &key, t));
    }

    body.child(add_row(state, &key, t))
        .child(
            row()
                .gap(space::S2)
                .child(
                    row()
                        .role("button")
                        .h(26.0)
                        .px(space::S3)
                        .radius(radius::MD)
                        .center()
                        .fill(t.card)
                        .border(1.0, t.card_border)
                        .on_press(move |s: &mut AppState| {
                            let key = s.key_of_selected();
                            let displays = s.model.displays.clone();
                            s.model.config.reset_inputs(&key, &displays);
                            crate::draw_menu(s);
                        })
                        .child(
                            text("Reset to detected")
                                .font(type_::LABEL)
                                .color(t.text_secondary),
                        ),
                )
                .child(
                    text("Changes apply to the menu immediately.")
                        .font(type_::BODY)
                        .color(t.text_secondary),
                ),
        )
}

fn display_key_hint(released: bool) -> &'static str {
    if released {
        "released"
    } else {
        "connected"
    }
}

fn input_row(index: usize, input: &InputSetting, key: &str, t: tok::Theme) -> El {
    let check = if input.enabled {
        el()
            .size(18.0, 18.0)
            .radius(5.0)
            .fill(tok::accent(0, true))
            .center()
            .child(text("✓").font(type_::LABEL).color(t.knob))
    } else {
        el()
            .size(18.0, 18.0)
            .radius(5.0)
            .fill(t.bg)
            .border(1.0, t.card_border)
    };

    let key_toggle = key.to_string();
    let key_remove = key.to_string();
    let key_protocol = key.to_string();

    row()
        .key(format!("input-{index}"))
        .w_full()
        .h(34.0)
        .px(space::S2)
        .gap(space::S2)
        .radius(radius::MD)
        .fill(t.card)
        .border(1.0, t.card_border)
        .center_y()
        .child(check)
        .child(col().grow(1.0).child(
            text(input.label.clone())
                .font(type_::LABEL)
                .color(if input.enabled { t.text } else { t.text_secondary }),
        ))
        .child(
            text(format!("{:#06x}", input.value))
                .font(type_::MONO_CAPTION)
                .color(t.text_secondary),
        )
        .child(
            row()
                .role("button")
                .h(22.0)
                .px(space::S2)
                .radius(radius::SM)
                .center()
                .fill(t.bg)
                .border(1.0, t.card_border)
                .on_press(move |s: &mut AppState| {
                    if let Some(config) = s.model.config.display_mut(&key_protocol) {
                        if let Some(input) = config.inputs.get_mut(index) {
                            input.protocol = match input.protocol {
                                Protocol::Standard => Protocol::LgAlt,
                                Protocol::LgAlt => Protocol::Standard,
                            };
                        }
                    }
                    changed(s);
                })
                .child(
                    text(input.protocol.label())
                        .font(type_::LABEL)
                        .color(t.text_secondary),
                ),
        )
        .child(
            row()
                .role("button")
                .size(22.0, 22.0)
                .radius(radius::SM)
                .center()
                .on_press(move |s: &mut AppState| {
                    if let Some(config) = s.model.config.display_mut(&key_remove) {
                        if index < config.inputs.len() {
                            config.inputs.remove(index);
                        }
                    }
                    changed(s);
                })
                .child(text("×").font(type_::LABEL).color(t.text_secondary)),
        )
        .on_press(move |s: &mut AppState| {
            if let Some(config) = s.model.config.display_mut(&key_toggle) {
                if let Some(input) = config.inputs.get_mut(index) {
                    input.enabled = !input.enabled;
                }
            }
            changed(s);
        })
}

fn add_row(state: &AppState, key: &str, t: tok::Theme) -> El {
    let key_add = key.to_string();

    row()
        .w_full()
        .gap(space::S2)
        .center_y()
        .child(
            text_input(&state.draft_label, type_::BODY)
                .key("draft-label")
                .w(150.0)
                .h(28.0)
                .px(space::S2)
                .radius(radius::MD)
                .fill(t.card)
                .border(1.0, t.card_border)
                .on_edit(|s: &mut AppState| &mut s.draft_label),
        )
        .child(
            text_input(&state.draft_value, type_::BODY)
                .key("draft-value")
                .w(90.0)
                .h(28.0)
                .px(space::S2)
                .radius(radius::MD)
                .fill(t.card)
                .border(1.0, t.card_border)
                .on_edit(|s: &mut AppState| &mut s.draft_value),
        )
        .child(
            row()
                .role("button")
                .h(28.0)
                .px(space::S2)
                .radius(radius::MD)
                .center()
                .fill(t.card)
                .border(1.0, t.card_border)
                .on_press(|s: &mut AppState| {
                    s.draft_protocol = match s.draft_protocol {
                        Protocol::Standard => Protocol::LgAlt,
                        Protocol::LgAlt => Protocol::Standard,
                    }
                })
                .child(
                    text(state.draft_protocol.label())
                        .font(type_::LABEL)
                        .color(t.text_secondary),
                ),
        )
        .child(
            row()
                .role("button")
                .h(28.0)
                .px(space::S3)
                .radius(radius::MD)
                .center()
                .fill(tok::accent(0, true))
                .on_press(move |s: &mut AppState| {
                    let raw = s.draft_value.text.trim().trim_start_matches("0x").to_string();
                    let Ok(value) = u16::from_str_radix(&raw, 16) else {
                        return;
                    };
                    let label = s.draft_label.text.trim().to_string();
                    let label = if label.is_empty() {
                        format!("Input {value:#06x}")
                    } else {
                        label
                    };
                    let protocol = s.draft_protocol;
                    if let Some(config) = s.model.config.display_mut(&key_add) {
                        config.inputs.push(InputSetting { label, value, protocol, enabled: true });
                    }
                    s.draft_label.clear();
                    s.draft_value.clear();
                    changed(s);
                })
                .child(text("Add").font(type_::LABEL).color(t.knob)),
        )
}

fn section(title: &str, t: tok::Theme) -> El {
    row().w_full().padding(space::S1).child(
        text(title.to_uppercase())
            .font(type_::SECTION)
            .color(t.text_secondary),
    )
}

fn setting_row(title: &str, detail: &str, control: El, t: tok::Theme) -> El {
    row()
        .w_full()
        .gap(space::S3)
        .padding(space::S3)
        .radius(radius::LG)
        .fill(t.card)
        .border(1.0, t.card_border)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .gap(2.0)
                .child(text(title).font(type_::LABEL).color(t.text))
                .child(text(detail).font(type_::BODY).color(t.text_secondary).wrap()),
        )
        .child(control)
}

fn switch(on: bool, t: tok::Theme, f: impl Fn(&mut AppState) + 'static) -> El {
    let knob = el()
        .size(16.0, 16.0)
        .radius(8.0)
        .fill(t.knob)
        .shadow(1.0, 2.0, t.knob_shadow)
        .absolute(if on { 19.0 } else { 3.0 }, 3.0);

    el()
        .role("switch")
        .checked(on)
        .size(38.0, 22.0)
        .radius(11.0)
        .fill(if on { tok::accent(0, true) } else { t.control_off })
        .on_press(move |s: &mut AppState| f(s))
        .child(knob)
}

/// Persists the config and refreshes the native menu so both surfaces agree.
pub fn changed(state: &mut AppState) {
    state.model.config.save();
    crate::draw_menu(state);
}

impl AppState {
    fn key_of_selected(&self) -> String {
        self.model
            .displays
            .get(self.selected)
            .map(|display| display.key.clone())
            .unwrap_or_default()
    }
}
