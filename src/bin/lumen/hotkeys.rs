//! Brightness media keys.
//!
//! macOS sends the dedicated brightness keys as `NSSystemDefined` events, not key presses. A
//! `CGEventTap` lets Lumen read and swallow them so its own pipeline drives brightness and the
//! system's brightness HUD does not also appear. The tap needs Accessibility permission; without
//! it the feature stays inactive (see `permission`).
//!
//! The pure helpers here (`next_brightness`, `decode_brightness`) carry the logic and are unit
//! tested; the AppKit/CoreGraphics glue is added on top.

use crate::config::BrightnessStep;

/// Accessibility (Input Monitoring) permission, required for the event tap.
///
/// `AXIsProcessTrusted` lives in ApplicationServices, which the AppKit frameworks already pull
/// in, so it is declared directly rather than `dlopen`ed.
pub mod permission {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    /// Whether this process already holds Accessibility permission.
    pub fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }
}

/// Which way a brightness key moves the level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// The next brightness value after one key press, clamped to `0..=max`.
///
/// The step is a fraction of the display's range (`max / divisions`), rounded up to at least one
/// so a press always moves on displays with a small range.
pub fn next_brightness(value: u16, max: u16, step: BrightnessStep, up: bool) -> u16 {
    if max == 0 {
        return 0;
    }
    let value = value.min(max);
    let delta = (max / step.divisions()).max(1);
    if up {
        value.saturating_add(delta).min(max)
    } else {
        value.saturating_sub(delta)
    }
}

/// Decodes an `NSSystemDefined` key event into a brightness direction.
///
/// Brightness lives in subtype 8 with key codes `NX_KEYTYPE_BRIGHTNESS_UP` (2) and
/// `NX_KEYTYPE_BRIGHTNESS_DOWN` (3). `key_down` is false on the release half of the press, which
/// is ignored. Anything else returns `None` so the event passes through untouched.
pub fn decode_brightness(subtype: i64, key_code: i64, key_down: bool) -> Option<Direction> {
    if subtype != 8 || !key_down {
        return None;
    }
    match key_code {
        NX_KEYTYPE_BRIGHTNESS_UP => Some(Direction::Up),
        NX_KEYTYPE_BRIGHTNESS_DOWN => Some(Direction::Down),
        _ => None,
    }
}

const NX_KEYTYPE_BRIGHTNESS_UP: i64 = 2;
const NX_KEYTYPE_BRIGHTNESS_DOWN: i64 = 3;

/// The display a brightness key should act on, resolved from the pointer's screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrightnessTarget {
    pub id: u32,
    pub value: u16,
    pub max: u16,
    /// False when the display reports no brightness or refuses changes: the HUD shows the
    /// not-available state and nothing is written.
    pub controllable: bool,
}

/// Decides what a brightness key does to one display.
///
/// `builtin_can_change` is the live `DisplayServices` answer for the internal panel, queried by
/// the caller; it is ignored for external displays, whose controllability is read from the cached
/// scan (a brightness value present and no DDC error).
pub fn resolve_target(display: &crate::engine::DisplayEntry, builtin_can_change: bool) -> BrightnessTarget {
    let (value, max) = display.brightness.unwrap_or((0, 100));
    let controllable = if display.builtin {
        display.brightness.is_some() && builtin_can_change
    } else {
        display.brightness.is_some() && display.ddc_error.is_none()
    };
    BrightnessTarget { id: display.id, value, max, controllable }
}

/// The CoreGraphics display id of the screen under the pointer, or the main display as a fallback.
fn display_under_pointer() -> Option<u32> {
    use objc2::msg_send;
    use objc2_app_kit::{NSEvent, NSScreen};
    use objc2_foundation::{MainThreadMarker, NSString};
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    unsafe {
        let mouse = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let key = NSString::from_str("NSScreenNumber");
        let read_id = |screen: &NSScreen| -> Option<u32> {
            let description = screen.deviceDescription();
            let value = description.objectForKey(&key)?;
            Some(msg_send![&*value, unsignedIntValue])
        };
        // Prefer the screen whose frame contains the pointer.
        for index in 0..screens.len() {
            let Some(screen) = screens.get_retained(index) else {
                continue;
            };
            let frame = screen.frame();
            if mouse.x >= frame.origin.x
                && mouse.x <= frame.origin.x + frame.size.width
                && mouse.y >= frame.origin.y
                && mouse.y <= frame.origin.y + frame.size.height
            {
                if let Some(id) = read_id(&screen) {
                    return Some(id);
                }
            }
        }
        screens.get_retained(0).and_then(|screen| read_id(&screen))
    }
}

/// Moves brightness one step on the display under the pointer and shows the HUD.
///
/// Runs on the main thread. Resolves the pointer's display, reads its cached brightness and
/// controllability, and — when controllable — queues a coalesced VCP write (reusing the 150 ms
/// debounce) with an optimistic model update. An uncontrollable display shows the not-available
/// HUD and changes nothing.
pub fn apply_brightness_step(up: bool) {
    let Some(id) = display_under_pointer() else {
        crate::log::trace("brightness keys: no display under the pointer");
        return;
    };
    let step = crate::with_model(|model| model.config.brightness_step);
    let display = crate::with_model(|model| model.display(id).cloned());
    let Some(display) = display else {
        crate::log::trace(format!("brightness keys: display {id} is not in the model"));
        return;
    };
    let builtin_can_change = display.builtin && crate::brightness::can_change(id);
    let target = resolve_target(&display, builtin_can_change);
    crate::log::trace(format!(
        "brightness keys: target display={id} value={} max={} controllable={}",
        target.value, target.max, target.controllable
    ));

    if !target.controllable {
        crate::hud::show(target.value, target.max, false);
        return;
    }

    let new_value = next_brightness(target.value, target.max, step, up);
    crate::with_model(|model| model.apply_control(id, lumen::ddc::VCP_BRIGHTNESS, new_value));
    crate::queue_control(id, lumen::ddc::VCP_BRIGHTNESS, new_value);
    crate::after_change();
    crate::hud::show(new_value, target.max, true);
}

// ---- CGEventTap for the brightness media keys --------------------------------------------------
//
// macOS delivers the brightness keys as `NSSystemDefined` events (CGEvent type 14), which the
// safe `core_graphics::event::CGEventTap` wrapper cannot select (its `CGEventType` enum omits 14).
// So the tap is created through the raw C API and each event is wrapped into an `NSEvent` with
// `+[NSEvent eventWithCGEvent:]` to read its subtype and `data1`, the usual media-key recipe.

use core_foundation::base::{CFRelease, TCFType};
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopSource};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::time::{Duration, Instant};

type CGEventRef = *mut CGEventOpaque;
type CGEventTapProxy = *const c_void;
type CFMachPortRef = *mut c_void;

/// Opaque CoreGraphics event.
///
/// It exists only to give the pointer the `^{__CGEvent=}` encoding that
/// `+[NSEvent eventWithCGEvent:]` declares. Passing `*mut c_void` panics in objc2 0.5's message
/// send argument check, which aborts the process inside the event tap.
#[repr(C)]
struct CGEventOpaque {
    _private: [u8; 0],
}

unsafe impl objc2::encode::RefEncode for CGEventOpaque {
    const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(
        &objc2::encode::Encoding::Struct("__CGEvent", &[]),
    );
}

#[allow(non_upper_case_globals)]
/// Media keys are intercepted at the HID tap, the standard place media-key utilities hook. A
/// session tap can come up installed but inert after an Accessibility grant; the HID tap is the
/// standard path.
const kCGHIDEventTap: u32 = 0;
#[allow(non_upper_case_globals)]
const kCGHeadInsertEventTap: u32 = 0;
/// NSSystemDefined is CGEvent type 14.
const EVENT_TYPE_SYSTEM_DEFINED: u32 = 14;
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

type TapCallback = unsafe extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> *mut c_void;
    fn CFMachPortInvalidate(port: CFMachPortRef);
}

struct Tap {
    port: CFMachPortRef,
    source: CFRunLoopSource,
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe {
            CFRunLoop::get_current().remove_source(&self.source, kCFRunLoopCommonModes);
            CFMachPortInvalidate(self.port);
            CFRelease(self.port as *const c_void);
        }
    }
}

/// What the brightness-key tap is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The feature is turned off in Settings.
    Off,
    /// The feature is on but Accessibility has not been granted.
    NoPermission,
    /// The tap is installed and enabled.
    Active,
    /// The tap was installed but the kernel left it inert or disabled it repeatedly.
    Offline,
}

thread_local! {
    static TAP: RefCell<Option<Tap>> = const { RefCell::new(None) };
    static STATUS: Cell<Status> = const { Cell::new(Status::Off) };
    /// Bumped whenever the tap is replaced or removed, so stale probes do nothing.
    static TAP_GEN: Cell<u64> = const { Cell::new(0) };
    /// Last time the kernel disabled the tap, for the repeated-disable watchdog.
    static LAST_DISABLE: Cell<Option<Instant>> = const { Cell::new(None) };
    static REFRESH_GEN: Cell<u64> = const { Cell::new(0) };
}

/// The tap's current state, for the Settings status row.
pub fn status() -> Status {
    STATUS.with(|status| status.get())
}

fn set_status(next: Status) {
    let changed = STATUS.with(|status| {
        let previous = status.get();
        status.set(next);
        previous != next
    });
    if changed {
        crate::log::info(format!("brightness keys: {next:?}"));
        crate::after_change();
    }
}

/// Reads an `NSSystemDefined` CGEvent's brightness direction by wrapping it as an `NSEvent`.
///
/// `data1`'s high 16 bits hold the key code; the low 16 bits hold flags, where the upper byte is
/// `0x0A` while the key is down and `0x0B` on release.
fn direction_for_event(event: CGEventRef) -> Option<Direction> {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    unsafe {
        let class = AnyClass::get("NSEvent")?;
        let ns_event: *mut objc2::runtime::AnyObject = msg_send![class, eventWithCGEvent: event];
        if ns_event.is_null() {
            return None;
        }
        let subtype: i16 = msg_send![ns_event, subtype];
        let data1: isize = msg_send![ns_event, data1];
        let (key_code, key_down) = parse_system_defined_data1(data1 as i64);
        decode_brightness(subtype as i64, key_code, key_down)
    }
}

/// Splits an `NSSystemDefined` event's `data1` into `(key_code, key_down)`.
///
/// The key code is the high 16 bits; the key-down flag is the upper byte of the low 16 bits,
/// `0x0A` while held and `0x0B` on release.
fn parse_system_defined_data1(data1: i64) -> (i64, bool) {
    let key_code = (data1 & 0xFFFF_0000) >> 16;
    let key_flags = data1 & 0x0000_FFFF;
    let key_down = (key_flags & 0xFF00) >> 8 == 0x0A;
    (key_code, key_down)
}

/// The C tap callback. Runs on the main run loop. Swallows a brightness key only when the feature
/// is on and the target display is controllable; anything else passes through untouched.
unsafe extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    // The kernel disables a tap when the callback is slow or when user input arrives. Re-enable
    // once, but a second disable inside five seconds means the tap is not usable: mark it offline
    // so Settings can offer a Retry, instead of silently doing nothing.
    if event_type == TAP_DISABLED_BY_TIMEOUT || event_type == TAP_DISABLED_BY_USER_INPUT {
        if !crate::with_model(|model| model.config.brightness_keys_enabled) {
            return event;
        }
        if !permission::trusted() {
            set_status(Status::NoPermission);
            crate::on_main(uninstall);
            return event;
        }
        let now = Instant::now();
        let repeated = LAST_DISABLE.with(|last| {
            last.get()
                .map(|at| now.duration_since(at) < Duration::from_secs(5))
                .unwrap_or(false)
        });
        LAST_DISABLE.with(|last| last.set(Some(now)));
        if repeated {
            crate::log::warn("brightness keys: tap disabled twice in five seconds; marking offline");
            set_status(Status::Offline);
            crate::on_main(uninstall);
            return event;
        }
        TAP.with(|slot| {
            if let Some(tap) = slot.borrow().as_ref() {
                unsafe { CGEventTapEnable(tap.port, true) };
            }
        });
        set_status(Status::Active);
        return event;
    }
    if event_type != EVENT_TYPE_SYSTEM_DEFINED {
        return event;
    }
    if !crate::with_model(|model| model.config.brightness_keys_enabled) {
        return event;
    }
    let Some(direction) = direction_for_event(event) else {
        return event;
    };
    crate::log::trace(format!("brightness keys: decoded {direction:?}"));

    // Decide whether the target display can be controlled before swallowing the key: an
    // uncontrollable display shows the not-available HUD but lets the event pass through.
    let controllable = pointer_target_controllable();
    let up = direction == Direction::Up;
    apply_brightness_step(up);
    if controllable {
        std::ptr::null_mut()
    } else {
        event
    }
}

/// Whether the display under the pointer can currently be controlled.
fn pointer_target_controllable() -> bool {
    let Some(id) = display_under_pointer() else {
        return false;
    };
    let display = crate::with_model(|model| model.display(id).cloned());
    let Some(display) = display else {
        return false;
    };
    let builtin_can_change = display.builtin && crate::brightness::can_change(id);
    resolve_target(&display, builtin_can_change).controllable
}

/// Installs the tap on the current (main) run loop. Idempotent.
fn install() {
    if TAP.with(|slot| slot.borrow().is_some()) {
        wake();
        return;
    }
    if !permission::trusted() {
        crate::log::trace("brightness keys: Accessibility not granted; tap not installed");
        set_status(Status::NoPermission);
        return;
    }
    unsafe {
        let mask: u64 = 1u64 << EVENT_TYPE_SYSTEM_DEFINED;
        let port = CGEventTapCreate(
            kCGHIDEventTap,
            kCGHeadInsertEventTap,
            0, // default options: an active tap that can alter or swallow events
            mask,
            tap_callback,
            std::ptr::null_mut(),
        );
        if port.is_null() {
            crate::log::warn("brightness keys: could not create the event tap");
            set_status(Status::Offline);
            return;
        }
        let source_ref = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
        if source_ref.is_null() {
            CFMachPortInvalidate(port);
            CFRelease(port as *const c_void);
            crate::log::warn("brightness keys: could not create the run-loop source");
            set_status(Status::Offline);
            return;
        }
        let source = CFRunLoopSource::wrap_under_create_rule(source_ref as _);
        CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
        CGEventTapEnable(port, true);
        TAP.with(|slot| *slot.borrow_mut() = Some(Tap { port, source }));
        crate::log::info("brightness keys: event tap installed");
        set_status(Status::Active);
        arm_ghost_probe();
    }
}

/// Removes the tap if present.
fn uninstall() {
    TAP_GEN.with(|generation| generation.set(generation.get() + 1));
    TAP.with(|slot| {
        if slot.borrow_mut().take().is_some() {
            crate::log::info("brightness keys: event tap removed");
        }
    });
}

/// Installs or removes the tap to match the current config and permission state.
pub fn refresh() {
    if !crate::with_model(|model| model.config.brightness_keys_enabled) {
        uninstall();
        set_status(Status::Off);
        return;
    }
    if !permission::trusted() {
        uninstall();
        set_status(Status::NoPermission);
        return;
    }
    install();
}

/// Reinstalls the tap after the Offline state or a permission change.
pub fn retry() {
    uninstall();
    LAST_DISABLE.with(|last| last.set(None));
    refresh();
}

/// Reconciles after the Accessibility list changes, debounced by 250 ms.
pub fn schedule_refresh() {
    let generation = REFRESH_GEN.with(|current| {
        let next = current.get() + 1;
        current.set(next);
        next
    });
    dispatch::Queue::main().exec_after(Duration::from_millis(250), move || {
        if REFRESH_GEN.with(|current| current.get()) == generation {
            refresh();
        }
    });
}

/// Re-enables the tap after wake or session activation. The kernel leaves a session tap
/// enabled-but-inert across sleep, so it must be enabled again rather than assumed alive.
pub fn wake() {
    if let Some(port) = TAP.with(|slot| slot.borrow().as_ref().map(|tap| tap.port)) {
        unsafe { CGEventTapEnable(port, true) };
        LAST_DISABLE.with(|last| last.set(None));
        set_status(Status::Active);
        arm_ghost_probe();
    } else {
        refresh();
    }
}

/// Disables the tap for sleep or session resign. The tap stays installed for a quick re-enable.
pub fn suspend() {
    if let Some(port) = TAP.with(|slot| slot.borrow().as_ref().map(|tap| tap.port)) {
        unsafe { CGEventTapEnable(port, false) };
    }
}

/// Prompts are deliberately not offered here. On macOS 27, `AXIsProcessTrustedWithOptions`
/// and the Accessibility pane are routed to the broad System & App Protection Override, which
/// grants full data, photo, history, control and input access. A display utility must not ask
/// for that. The tap is installed only when the narrow permission already exists.

/// A tap can be created and report enabled while the kernel has actually left it inert after a
/// permission grant. Check once after install and surface the Offline state instead of silently
/// swallowing keys.
fn arm_ghost_probe() {
    let generation = TAP_GEN.with(|current| {
        let next = current.get() + 1;
        current.set(next);
        next
    });
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        crate::on_main(move || {
            if TAP_GEN.with(|current| current.get()) != generation {
                return;
            }
            let enabled = TAP.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|tap| unsafe { CGEventTapIsEnabled(tap.port) })
                    .unwrap_or(false)
            });
            if !enabled {
                crate::log::warn("brightness keys: tap installed but disabled; marking offline");
                set_status(Status::Offline);
                uninstall();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DisplayEntry;

    fn entry(builtin: bool, brightness: Option<(u16, u16)>, ddc_error: Option<&str>) -> DisplayEntry {
        DisplayEntry {
            id: 7,
            key: "k".into(),
            name: "D".into(),
            builtin,
            inputs: Vec::new(),
            brightness,
            contrast: None,
            volume: None,
            muted: None,
            controls_stale: false,
            current_input: None,
            ddc_error: ddc_error.map(str::to_string),
        }
    }

    #[test]
    fn steps_up_and_down_by_the_step_fraction() {
        // Normal = 1/16 of a 0..=100 range -> 6 per press.
        assert_eq!(next_brightness(50, 100, BrightnessStep::Normal, true), 56);
        assert_eq!(next_brightness(50, 100, BrightnessStep::Normal, false), 44);
        // Coarse = 1/8 -> 12; Fine = 1/32 -> 3.
        assert_eq!(next_brightness(50, 100, BrightnessStep::Coarse, true), 62);
        assert_eq!(next_brightness(50, 100, BrightnessStep::Fine, true), 53);
    }

    #[test]
    fn clamps_at_both_ends() {
        assert_eq!(next_brightness(98, 100, BrightnessStep::Normal, true), 100);
        assert_eq!(next_brightness(3, 100, BrightnessStep::Normal, false), 0);
        assert_eq!(next_brightness(0, 100, BrightnessStep::Normal, false), 0);
        assert_eq!(next_brightness(100, 100, BrightnessStep::Normal, true), 100);
    }

    #[test]
    fn a_value_above_max_is_pulled_in_first() {
        assert_eq!(next_brightness(200, 100, BrightnessStep::Normal, false), 94);
        assert_eq!(next_brightness(200, 100, BrightnessStep::Normal, true), 100);
    }

    #[test]
    fn small_range_still_moves_by_at_least_one() {
        // 10 / 16 rounds to 0, so the step floors to 1.
        assert_eq!(next_brightness(5, 10, BrightnessStep::Normal, true), 6);
        assert_eq!(next_brightness(5, 10, BrightnessStep::Normal, false), 4);
    }

    #[test]
    fn max_zero_is_safe() {
        assert_eq!(next_brightness(0, 0, BrightnessStep::Normal, true), 0);
        assert_eq!(next_brightness(50, 0, BrightnessStep::Coarse, false), 0);
    }

    #[test]
    fn event_pointer_matches_nsevent_encoding() {
        // +[NSEvent eventWithCGEvent:] declares ^{__CGEvent=}. A plain c_void pointer aborts at
        // the message send inside the tap callback.
        use objc2::encode::{Encode, Encoding};
        assert_eq!(
            <CGEventRef as Encode>::ENCODING,
            Encoding::Pointer(&Encoding::Struct("__CGEvent", &[]))
        );
    }

    #[test]
    fn decodes_only_brightness_key_down_events() {
        assert_eq!(decode_brightness(8, 2, true), Some(Direction::Up));
        assert_eq!(decode_brightness(8, 3, true), Some(Direction::Down));
        // Release half, wrong subtype, and other keys pass through.
        assert_eq!(decode_brightness(8, 2, false), None);
        assert_eq!(decode_brightness(7, 2, true), None);
        assert_eq!(decode_brightness(8, 4, true), None);
    }

    #[test]
    fn parses_system_defined_data1_key_code_and_state() {
        // Brightness-up key down: key code 2 in the high word, flags 0x0A00 (down) in the low word.
        let down = (2i64 << 16) | 0x0A00;
        assert_eq!(parse_system_defined_data1(down), (2, true));
        // Same key released: flags 0x0B00.
        let up = (2i64 << 16) | 0x0B00;
        assert_eq!(parse_system_defined_data1(up), (2, false));
        // Brightness-down key down.
        let bd = (3i64 << 16) | 0x0A00;
        assert_eq!(parse_system_defined_data1(bd), (3, true));
    }

    #[test]
    fn data1_feeds_the_decoder_end_to_end() {
        let (code, down) = parse_system_defined_data1((2i64 << 16) | 0x0A00);
        assert_eq!(decode_brightness(8, code, down), Some(Direction::Up));
        let (code, down) = parse_system_defined_data1((3i64 << 16) | 0x0B00);
        assert_eq!(decode_brightness(8, code, down), None); // release half
    }

    #[test]
    fn builtin_is_controllable_only_when_displayservices_agrees() {
        let display = entry(true, Some((50, 100)), None);
        assert!(resolve_target(&display, true).controllable);
        assert!(!resolve_target(&display, false).controllable);
        // No brightness reading at all is never controllable.
        let blank = entry(true, None, None);
        assert!(!resolve_target(&blank, true).controllable);
    }

    #[test]
    fn external_is_controllable_when_a_reading_exists_and_no_ddc_error() {
        let ok = entry(false, Some((40, 100)), None);
        let target = resolve_target(&ok, false);
        assert!(target.controllable);
        assert_eq!((target.value, target.max), (40, 100));
        // A DDC error, or a missing reading, marks it not-available.
        assert!(!resolve_target(&entry(false, Some((40, 100)), Some("brightness: no reply")), false).controllable);
        assert!(!resolve_target(&entry(false, None, None), false).controllable);
    }

    #[test]
    fn target_carries_value_and_max_even_when_not_controllable() {
        let target = resolve_target(&entry(false, Some((30, 80)), Some("err")), false);
        assert_eq!((target.value, target.max), (30, 80));
        assert!(!target.controllable);
        // With no reading it falls back to a 0/100 range.
        let blank = resolve_target(&entry(false, None, Some("err")), false);
        assert_eq!((blank.value, blank.max), (0, 100));
    }
}
