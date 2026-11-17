// Flat C surface over Sparkle's SPUStandardUpdaterController.
//
// The Rust side links against this and never touches Objective-C or Sparkle types directly. All
// application logic lives in Rust; this shim only owns the single updater controller instance and
// forwards the three operations the app needs. Every function must be called on the main thread.
#ifndef LUMEN_SPARKLE_H
#define LUMEN_SPARKLE_H

#include <stdbool.h>

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

#ifdef __cplusplus
}
#endif

#endif // LUMEN_SPARKLE_H
