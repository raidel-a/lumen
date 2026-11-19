// Objective-C shim around Sparkle's SPUStandardUpdaterController.
//
// Compiled by build.rs with the `cc` crate (ARC enabled) only when the `sparkle` Cargo feature is
// on. Deliberately dumb: it holds one controller in a static and forwards calls. Decisions about
// when to start, when to check, and how to present results stay in Rust / Sparkle itself.
//
// The one piece of state it keeps is the "an update is available" status, recorded by a thin
// SPUUpdaterDelegate so the Rust side can show an update entry in the menu bar only when one
// exists. The delegate records the found update's version and notifies Rust through a callback;
// Rust reads the version back and rebuilds its menu.
#import <Foundation/Foundation.h>
#import <Sparkle/Sparkle.h>

#import "lumen_sparkle.h"

// Forward declaration so the controller can take the delegate below.
@interface LumenUpdaterDelegate : NSObject <SPUUpdaterDelegate>
@end

// The controller strongly retains the updater and its user driver; keeping it in a static keeps
// Sparkle alive for the lifetime of the process. Only ever touched on the main thread.
static SPUStandardUpdaterController *gUpdaterController = nil;
static LumenUpdaterDelegate *gUpdaterDelegate = nil;

// Latest available update version ("" when none), and the Rust callback to fire when it changes.
static NSString *gAvailableVersion = nil;
static void (*gAvailableCallback)(void) = NULL;

// Sets the available-version state and notifies Rust when it actually changed.
static void lumen_set_available_version(NSString *version) {
    NSString *next = version ?: @"";
    if ([next isEqualToString:(gAvailableVersion ?: @"")]) {
        return;
    }
    gAvailableVersion = next;
    if (gAvailableCallback != NULL) {
        gAvailableCallback();
    }
}

@implementation LumenUpdaterDelegate

// Sparkle found a valid update. Record its human-readable version so the menu can offer it.
- (void)updater:(SPUUpdater *)updater didFindValidUpdate:(SUAppcastItem *)item {
    (void)updater;
    NSString *version = item.displayVersionString ?: item.versionString;
    lumen_set_available_version(version);
}

// The check finished without an applicable update: clear any previously-advertised version.
- (void)updaterDidNotFindUpdate:(SPUUpdater *)updater {
    (void)updater;
    lumen_set_available_version(@"");
}

// Once an update cycle finishes (installed, or dismissed), drop the advertised version so the
// menu entry does not linger after the user has acted on it.
- (void)updater:(SPUUpdater *)updater didFinishUpdateCycleForUpdateCheck:(SPUUpdateCheck)updateCheck error:(NSError *)error {
    (void)updater;
    (void)updateCheck;
    (void)error;
    lumen_set_available_version(@"");
}

@end

void lumen_updater_start(bool background_checks) {
    if (gUpdaterController != nil) {
        return;
    }
    gUpdaterDelegate = [[LumenUpdaterDelegate alloc] init];
    // startingUpdater:YES starts the updater immediately. Sparkle's own scheduler then performs
    // background checks according to the app's SUEnableAutomaticChecks / SUScheduledCheckInterval
    // defaults; we gate whether that scheduler may run via the background_checks flag below.
    gUpdaterController =
        [[SPUStandardUpdaterController alloc] initWithStartingUpdater:YES
                                                      updaterDelegate:gUpdaterDelegate
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

void lumen_updater_set_available_callback(void (*callback)(void)) {
    gAvailableCallback = callback;
}

bool lumen_updater_available_version(char *buf, size_t len) {
    if (buf == NULL || len == 0) {
        return false;
    }
    NSString *version = gAvailableVersion ?: @"";
    if (version.length == 0) {
        buf[0] = '\0';
        return false;
    }
    if (![version getCString:buf maxLength:len encoding:NSUTF8StringEncoding]) {
        // Version did not fit; treat as unavailable rather than return a truncated string.
        buf[0] = '\0';
        return false;
    }
    return true;
}
