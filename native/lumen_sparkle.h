// Flat C surface over Sparkle's SPUStandardUpdaterController.
//
// The Rust side links against this and never touches Objective-C or Sparkle types directly. All
// application logic lives in Rust; this shim only owns the single updater controller instance and
// forwards the operations the app needs. Every function must be called on the main thread.
#ifndef LUMEN_SPARKLE_H
#define LUMEN_SPARKLE_H

#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Create (once) and start the shared updater controller. When background_checks is true the
// updater's scheduler is allowed to perform automatic background checks and notify the user.
// Safe to call more than once; only the first call creates and starts the controller.
void lumen_updater_start(bool background_checks);

// Trigger a user-initiated update check with Sparkle's standard progress UI. No-op if the updater
// has not been started.
void lumen_updater_check_now(void);

// Whether an update check can currently be initiated (updater started and not already busy).
bool lumen_updater_can_check(void);

// Register a callback invoked on the main thread whenever the "an update is available" state
// changes (a valid update is found, or the update cycle finishes without one). The callback takes
// no arguments; the Rust side then reads lumen_updater_available_version to refresh its UI. Pass
// NULL to clear it.
void lumen_updater_set_available_callback(void (*callback)(void));

// Copy the short version string of the update Sparkle last found (e.g. "0.1.4") into buf, which
// must hold at least len bytes, NUL-terminated. Returns true when an update is currently known to
// be available, false otherwise (buf is set to an empty string).
bool lumen_updater_available_version(char *buf, size_t len);

#ifdef __cplusplus
}
#endif

#endif // LUMEN_SPARKLE_H
