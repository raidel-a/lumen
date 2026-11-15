//! Persistent settings.
//!
//! The menu bar is the app's primary surface, so the config describes what the menu shows:
//! per-display input lists and a handful of behaviour switches.

use crate::engine::DisplayEntry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Standard,
    LgAlt,
}

impl Protocol {
    pub fn from_ddc(protocol: lumen::ddc::InputProtocol) -> Self {
        match protocol {
            lumen::ddc::InputProtocol::Standard => Protocol::Standard,
            lumen::ddc::InputProtocol::LgAlt => Protocol::LgAlt,
        }
    }

    pub fn to_ddc(self) -> lumen::ddc::InputProtocol {
        match self {
            Protocol::Standard => lumen::ddc::InputProtocol::Standard,
            Protocol::LgAlt => lumen::ddc::InputProtocol::LgAlt,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Standard => "standard",
            Protocol::LgAlt => "LG alt",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputSetting {
    pub label: String,
    pub value: u16,
    pub protocol: Protocol,
    pub enabled: bool,
    /// None until tested, then whether a switch this app sent actually worked.
    #[serde(default)]
    pub verified: Option<bool>,
}

impl InputSetting {
    pub fn from_detected(input: &lumen::ddc::InputCode) -> Self {
        Self {
            label: input.label.clone(),
            value: input.value,
            protocol: Protocol::from_ddc(input.protocol),
            enabled: true,
            verified: None,
        }
    }
}

/// The input the Mac itself is plugged into, remembered across launches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentInput {
    pub value: u16,
    pub protocol: Protocol,
}

/// A display that exists only in memory, created with `CGVirtualDisplay`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VirtualScreenConfig {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Ask for a 2x backing store. 1x is cheaper and gives the agent more logical space.
    pub hidpi: bool,
    pub refresh: f64,
    /// Recreate this screen at launch and after wake.
    pub auto_create: bool,
    /// Where the last preview window sat, in screen coordinates. Geometry only: previews do not
    /// reopen themselves at launch.
    #[serde(default)]
    pub preview: Option<PreviewFrame>,
}

/// A preview window frame, kept in AppKit screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreviewFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for VirtualScreenConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            width: 1920,
            height: 1080,
            hidpi: false,
            refresh: 60.0,
            auto_create: true,
            preview: None,
        }
    }
}

/// A browser this app started on a virtual screen. Persisted so a crash cannot leave it running
/// on a real display after the screen is gone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSessionRecord {
    pub name: String,
    pub pid: i32,
    pub browser: String,
    pub profile: String,
    #[serde(default)]
    pub url: Option<String>,
    /// Headless sessions have no window and are never re-homed.
    #[serde(default)]
    pub headless: bool,
}

/// A display this app released and has not brought back yet. Kept on disk so a crash cannot
/// leave the output switched off until logout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleasedRecord {
    pub id: u32,
    pub key: String,
    pub name: String,
}

/// Defaults applied when a virtual display is created from the settings window.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VirtualDefaults {
    pub width: u32,
    pub height: u32,
    pub hidpi: bool,
    pub refresh: f64,
}

impl Default for VirtualDefaults {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            hidpi: false,
            refresh: 60.0,
        }
    }
}

/// The most recent agent start, remembered so it can be restored at launch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentLaunchRecord {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headless: bool,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    9222
}

fn default_true() -> bool {
    true
}

fn default_capture_limit() -> Option<u32> {
    Some(20)
}

/// How far one brightness key press moves the level, as a fraction of the display's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BrightnessStep {
    /// 1/8 of the range per press.
    Coarse,
    /// 1/16 of the range per press, matching macOS's own brightness increment.
    Normal,
    /// 1/32 of the range per press.
    Fine,
}

impl Default for BrightnessStep {
    fn default() -> Self {
        Self::Normal
    }
}

impl BrightnessStep {
    /// The denominator of the step fraction: the number of presses to cross the whole range.
    pub fn divisions(self) -> u16 {
        match self {
            BrightnessStep::Coarse => 8,
            BrightnessStep::Normal => 16,
            BrightnessStep::Fine => 32,
        }
    }
}

/// The on-screen brightness indicator's look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HudStyle {
    /// The modern frosted panel with a glyph and a continuous bar.
    Tahoe,
    /// The older 200x200 pre-Tahoe panel: an 80 pt sun glyph above a row of sixteen square tiles
    /// that fill left to right, centred near the bottom of the screen. No percentage.
    Classic,
}

impl Default for HudStyle {
    fn default() -> Self {
        Self::Tahoe
    }
}

fn default_brightness_step() -> BrightnessStep {
    BrightnessStep::Normal
}

fn default_hud_style() -> HudStyle {
    HudStyle::Tahoe
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// `vendor:model:serial`, or `name:<product name>` when the EDID gives nothing better.
    pub key: String,
    pub name: String,
    pub show_in_menu: bool,
    pub inputs: Vec<InputSetting>,
    /// The input this Mac is plugged into, so the menu can mark it even on monitors that do not
    /// report their input (this LG answers `VCP 0x60` with zero).
    #[serde(default)]
    pub current_input: Option<CurrentInput>,
}

/// Where the settings window should open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsDisplay {
    /// The display whose menu bar the command came from.
    Invocation,
    /// The virtual display named Agent.
    Agent,
    /// The primary display.
    Main,
    /// The display used last time.
    Last,
}

impl Default for SettingsDisplay {
    fn default() -> Self {
        Self::Invocation
    }
}

fn default_settings_page() -> String {
    "general".to_string()
}

fn default_settings_width() -> f64 {
    920.0
}

fn default_settings_height() -> f64 {
    880.0
}

/// Schema version for config migrations. Bump when a field needs rewriting.
pub const CONFIG_VERSION: u32 = 2;

fn config_version() -> u32 {
    CONFIG_VERSION
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema version. Missing means a pre-versioned file and is treated as current.
    #[serde(default = "config_version")]
    pub version: u32,
    /// Bring every released display back before the app exits.
    pub reclaim_on_quit: bool,
    /// Reclaim a released display after this many minutes (None keeps it released).
    pub auto_reclaim_minutes: Option<u32>,
    /// Show the brightness percentage and presets in each display's submenu.
    pub show_brightness: bool,
    /// Let the brightness media keys (F1/F2) drive brightness through Lumen. Off by default
    /// because it needs Accessibility permission and overrides the system's own keys.
    #[serde(default)]
    pub brightness_keys_enabled: bool,
    /// How far one brightness key press moves the level.
    #[serde(default = "default_brightness_step")]
    pub brightness_step: BrightnessStep,
    /// The on-screen brightness indicator's look.
    #[serde(default = "default_hud_style")]
    pub hud_style: HudStyle,
    pub displays: Vec<DisplayConfig>,
    pub released: Vec<ReleasedRecord>,
    /// Screens that exist only in memory, for the agent workspace and for capture.
    pub virtual_screens: Vec<VirtualScreenConfig>,
    /// Agent browsers this app started, so they can be closed on quit or after a crash.
    pub agent_sessions: Vec<AgentSessionRecord>,
    /// Virtual displays created from settings start from these values.
    pub virtual_defaults: VirtualDefaults,
    /// Start agent browsers without a window unless a request says otherwise.
    #[serde(default = "default_true")]
    pub agent_headless: bool,
    /// Opened when an agent starts without an explicit URL.
    #[serde(default)]
    pub agent_url: Option<String>,
    /// Start the last agent session again when Lumen launches.
    #[serde(default)]
    pub agent_reopen: bool,
    /// Parameters of the last successful agent start, kept across quits.
    #[serde(default)]
    pub last_agent: Option<AgentLaunchRecord>,
    /// How many captures to keep per folder; `None` keeps everything.
    #[serde(default = "default_capture_limit")]
    pub capture_limit: Option<u32>,
    /// Display policy for the settings window.
    #[serde(default)]
    pub settings_display: SettingsDisplay,
    /// Last settings page, restored on open.
    #[serde(default = "default_settings_page")]
    pub settings_page: String,
    /// Display name used when the policy is `Last`.
    #[serde(default)]
    pub settings_last_screen: Option<String>,
    /// Settings window size, persisted when it closes.
    #[serde(default = "default_settings_width")]
    pub settings_width: f64,
    #[serde(default = "default_settings_height")]
    pub settings_height: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            reclaim_on_quit: true,
            auto_reclaim_minutes: None,
            show_brightness: true,
            brightness_keys_enabled: false,
            brightness_step: BrightnessStep::Normal,
            hud_style: HudStyle::Tahoe,
            displays: Vec::new(),
            released: Vec::new(),
            virtual_screens: Vec::new(),
            agent_sessions: Vec::new(),
            virtual_defaults: VirtualDefaults::default(),
            agent_headless: true,
            agent_url: None,
            agent_reopen: false,
            last_agent: None,
            capture_limit: Some(20),
            settings_display: SettingsDisplay::Invocation,
            settings_page: default_settings_page(),
            settings_last_screen: None,
            settings_width: default_settings_width(),
            settings_height: default_settings_height(),
        }
    }
}

fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        sequence
    ));
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

impl Config {
    pub fn path() -> PathBuf {
        Self::support_dir().join("config.json")
    }

    pub fn load() -> Self {
        let mut config: Self = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if config.version == 0 {
            config.version = CONFIG_VERSION;
        }
        config
    }

    /// Writes through a temporary file so a crash or a second writer cannot leave half a file.
    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        if write_atomic(&path, &text).is_err() {
            crate::log::warn(format!("could not save {}", path.display()));
        }
    }

    /// Adds configs for displays seen for the first time and refreshes their names and inputs.
    /// Saves only when something changed, since this runs on every scan.
    pub fn sync(&mut self, displays: &[DisplayEntry]) {
        let mut changed = false;
        for display in displays {
            match self.displays.iter_mut().find(|entry| entry.key == display.key) {
                Some(entry) => {
                    if entry.name != display.name {
                        entry.name = display.name.clone();
                        changed = true;
                    }
                    // The internal panel has no inputs; syncing would otherwise save on every scan.
                    if !display.builtin
                        && entry.inputs.is_empty()
                        && !display.inputs.is_empty()
                    {
                        entry.inputs = display.inputs.iter().map(InputSetting::from_detected).collect();
                        changed = true;
                    }
                }
                None => {
                    self.displays.push(DisplayConfig {
                        key: display.key.clone(),
                        name: display.name.clone(),
                        show_in_menu: true,
                        inputs: if display.builtin {
                            Vec::new()
                        } else {
                            display.inputs.iter().map(InputSetting::from_detected).collect()
                        },
                        current_input: None,
                    });
                    changed = true;
                }
            }
        }
        if changed {
            self.save();
        }
    }

    /// Stores the last preview geometry for a screen. Geometry only; nothing reopens at launch.
    pub fn set_preview_frame(&mut self, name: &str, frame: Option<PreviewFrame>) {
        if let Some(screen) = self.virtual_screens.iter_mut().find(|screen| screen.name == name) {
            screen.preview = frame;
            self.save();
        }
    }

    pub fn display(&self, key: &str) -> Option<&DisplayConfig> {
        self.displays.iter().find(|entry| entry.key == key)
    }

    pub fn display_mut(&mut self, key: &str) -> Option<&mut DisplayConfig> {
        self.displays.iter_mut().find(|entry| entry.key == key)
    }

    /// Records the result of a switch attempt on one input.
    pub fn mark_input(&mut self, key: &str, value: u16, protocol: Protocol, worked: bool) {
        if let Some(entry) = self.display_mut(key) {
            for input in entry.inputs.iter_mut() {
                if input.value == value && input.protocol == protocol {
                    input.verified = Some(worked);
                }
            }
        }
        self.save();
    }

    /// Replaces a display's list with what the monitor currently advertises.
    pub fn reset_inputs(&mut self, key: &str, displays: &[DisplayEntry]) {
        let detected = displays
            .iter()
            .find(|display| display.key == key)
            .map(|display| display.inputs.clone())
            .unwrap_or_default();
        if let Some(entry) = self.display_mut(key) {
            entry.inputs = detected.iter().map(InputSetting::from_detected).collect();
        }
        self.save();
    }

    /// The socket the menu-bar app listens on for CLI and agent commands.
    /// A dev or test build can point at its own directory and never touch the real config.
    pub fn support_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("LUMEN_SUPPORT_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir);
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join("Library/Application Support/Lumen")
    }

    pub fn virtual_screen(&self, name: &str) -> Option<&VirtualScreenConfig> {
        self.virtual_screens.iter().find(|screen| screen.name == name)
    }

    /// Adds or updates a screen definition and saves.
    pub fn upsert_virtual_screen(&mut self, screen: VirtualScreenConfig) {
        match self.virtual_screens.iter_mut().find(|entry| entry.name == screen.name) {
            Some(entry) => *entry = screen,
            None => self.virtual_screens.push(screen),
        }
        self.save();
    }

    pub fn remove_virtual_screen(&mut self, name: &str) {
        self.virtual_screens.retain(|screen| screen.name != name);
        self.save();
    }

    /// Replaces a screen definition in place, keeping its position in the list.
    pub fn rename_virtual_screen(&mut self, old: &str, screen: VirtualScreenConfig) {
        match self.virtual_screens.iter_mut().find(|entry| entry.name == old) {
            Some(entry) => *entry = screen,
            None => self.virtual_screens.push(screen),
        }
        self.save();
    }

    pub fn record_agent_session(&mut self, record: AgentSessionRecord) {
        self.agent_sessions.retain(|entry| entry.name != record.name);
        self.agent_sessions.push(record);
        self.save();
    }

    pub fn forget_agent_session(&mut self, name: &str) {
        self.agent_sessions.retain(|entry| entry.name != name);
        self.save();
    }

    /// The remembered Mac-side input, if one has been set.
    pub fn current_input(&self, key: &str) -> Option<CurrentInput> {
        self.display(key).and_then(|entry| entry.current_input)
    }

    /// Records the input the Mac is plugged into. Saves only when it actually changes.
    pub fn set_current_input(&mut self, key: &str, value: u16, protocol: Protocol) {
        let wanted = CurrentInput { value, protocol };
        if let Some(entry) = self.display_mut(key) {
            if entry.current_input != Some(wanted) {
                entry.current_input = Some(wanted);
                self.save();
            }
        }
    }

    pub fn enabled_inputs(&self, key: &str) -> Vec<InputSetting> {
        self.display(key)
            .map(|entry| entry.inputs.iter().filter(|input| input.enabled).cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_payload_gets_versioned_defaults() {
        let config: Config = serde_json::from_str(
            r#"{"reclaim_on_quit":false,"show_brightness":false}"#,
        )
        .expect("legacy config");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.settings_display, SettingsDisplay::Invocation);
        assert_eq!(config.settings_page, "general");
        assert!(config.agent_headless);
        assert_eq!(config.capture_limit, Some(20));
        assert_eq!(config.virtual_defaults.width, 1920);
        assert!(!config.brightness_keys_enabled);
        assert_eq!(config.brightness_step, BrightnessStep::Normal);
        assert_eq!(config.hud_style, HudStyle::Tahoe);
    }

    #[test]
    fn brightness_key_settings_round_trip() {
        let mut config = Config::default();
        config.brightness_keys_enabled = true;
        config.brightness_step = BrightnessStep::Fine;
        config.hud_style = HudStyle::Classic;
        let text = serde_json::to_string(&config).expect("serialize");
        let back: Config = serde_json::from_str(&text).expect("parse");
        assert!(back.brightness_keys_enabled);
        assert_eq!(back.brightness_step, BrightnessStep::Fine);
        assert_eq!(back.hud_style, HudStyle::Classic);
    }

    #[test]
    fn preview_geometry_is_optional_and_round_trips() {
        let legacy: VirtualScreenConfig = serde_json::from_str(
            r#"{"name":"Agent","width":1920,"height":1080,"hidpi":false,"refresh":60.0,"auto_create":true}"#,
        )
        .expect("legacy screen");
        assert_eq!(legacy.preview, None);

        let mut config = Config::default();
        config.virtual_screens.push(legacy);
        config.set_preview_frame(
            "Agent",
            Some(PreviewFrame {
                x: 10.0,
                y: 20.0,
                width: 480.0,
                height: 270.0,
            }),
        );
        assert_eq!(
            config.virtual_screen("Agent").and_then(|screen| screen.preview),
            Some(PreviewFrame {
                x: 10.0,
                y: 20.0,
                width: 480.0,
                height: 270.0,
            })
        );
    }

    #[test]
    fn settings_display_round_trips() {
        let mut config = Config::default();
        config.settings_display = SettingsDisplay::Agent;
        let text = serde_json::to_string(&config).expect("serialize");
        let back: Config = serde_json::from_str(&text).expect("parse");
        assert_eq!(back.settings_display, SettingsDisplay::Agent);
    }

    #[test]
    fn atomic_write_replaces_the_previous_file() {
        let dir = std::env::temp_dir().join(format!("lumen-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.json");
        write_atomic(&path, "one").expect("first write");
        write_atomic(&path, "two").expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "two");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
