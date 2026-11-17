//! Self-update, backed by Sparkle.
//!
//! The actual update machinery lives in the vendored Sparkle framework, reached through a tiny
//! Objective-C shim (native/lumen_sparkle.m). This module is the only Rust surface for it and is
//! split by the `sparkle` Cargo feature:
//!
//! * with the feature on, the functions forward to the shim on the main thread;
//! * with it off, they are logging no-ops so `cargo test`, lumen-probe, and non-bundle builds do
//!   not need the framework.
//!
//! Sparkle keeps its own preferences in NSUserDefaults, so nothing here is persisted through the
//! app's own config.

#[cfg(feature = "sparkle")]
mod imp {
    use objc2_foundation::MainThreadMarker;

    // Flat C surface exported by native/lumen_sparkle.m. All three must be called on the main
    // thread, which the safe wrappers below guarantee.
    unsafe extern "C" {
        fn lumen_updater_start(background_checks: bool);
        fn lumen_updater_check_now();
        fn lumen_updater_can_check() -> bool;
    }

    /// Create and start Sparkle's updater. `background` enables the automatic background-check
    /// scheduler; pass `false` to only ever check on explicit user action.
    pub fn start(background: bool) {
        // Starting the updater touches AppKit; marshal onto the main thread.
        crate::on_main(move || {
            // Safety: dispatched onto the main thread, so Sparkle's main-thread requirement holds.
            unsafe { lumen_updater_start(background) };
            crate::log::info("self-update: updater started");
        });
    }

    /// Begin a user-initiated update check with Sparkle's standard progress UI.
    pub fn check_now() {
        crate::on_main(|| {
            // Safety: dispatched onto the main thread.
            unsafe { lumen_updater_check_now() };
            crate::log::info("self-update: manual check requested");
        });
    }

    /// Whether an update check can be initiated right now. Must be called on the main thread
    /// (menu validation already runs there); returns false if called off-main as a safety net.
    pub fn can_check() -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        // Safety: confirmed to be on the main thread above.
        unsafe { lumen_updater_can_check() }
    }
}

#[cfg(not(feature = "sparkle"))]
mod imp {
    /// No-op: this binary was built without the self-update path.
    pub fn start(_background: bool) {
        crate::log::info("self-update not built into this binary");
    }

    /// No-op: this binary was built without the self-update path.
    pub fn check_now() {
        crate::log::info("self-update not built into this binary");
    }

    /// Always false without the feature, so update UI stays disabled.
    pub fn can_check() -> bool {
        false
    }
}

pub use imp::{can_check, check_now, start};

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "sparkle"))]
    #[test]
    fn can_check_is_false_without_feature() {
        assert!(!can_check());
    }
}
