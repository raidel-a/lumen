// Objective-C shim around Sparkle's SPUStandardUpdaterController.
//
// Compiled by build.rs with the `cc` crate (ARC enabled) only when the `sparkle` Cargo feature is
// on. Deliberately dumb: it holds one controller in a static and forwards calls. Decisions about
// when to start, when to check, and how to present results stay in Rust / Sparkle itself.
#import <Foundation/Foundation.h>
#import <Sparkle/Sparkle.h>

#import "lumen_sparkle.h"

// The controller strongly retains the updater and its user driver; keeping it in a static keeps
// Sparkle alive for the lifetime of the process. Only ever touched on the main thread.
static SPUStandardUpdaterController *gUpdaterController = nil;

void lumen_updater_start(bool background_checks) {
    if (gUpdaterController != nil) {
        return;
    }
    // startingUpdater:YES starts the updater immediately. Sparkle's own scheduler then performs
    // background checks according to the app's SUEnableAutomaticChecks / SUScheduledCheckInterval
    // defaults; we gate whether that scheduler may run via the background_checks flag below.
    gUpdaterController =
        [[SPUStandardUpdaterController alloc] initWithStartingUpdater:YES
                                                      updaterDelegate:nil
                                                   userDriverDelegate:nil];

    // When background checks are not wanted, disable Sparkle's automatic scheduler so it only runs
    // on explicit user-initiated checks. When they are wanted, leave the updater's configured
    // behaviour untouched so the app's defaults / first-launch permission prompt apply.
    if (!background_checks) {
        gUpdaterController.updater.automaticallyChecksForUpdates = NO;
    }
}

void lumen_updater_check_now(void) {
    if (gUpdaterController == nil) {
        return;
    }
    [gUpdaterController checkForUpdates:nil];
}

bool lumen_updater_can_check(void) {
    if (gUpdaterController == nil) {
        return false;
    }
    return gUpdaterController.updater.canCheckForUpdates ? true : false;
}
