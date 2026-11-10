# Lumen

A macOS menu bar display controller: native status item, native AppKit settings window. Built as
the start of an open BetterDisplay-style tool. First job: switching a monitor between the Mac and
another machine.

```
LG ULTRAWIDE
  Release to other input (auto-switch)
  ────────────────────────
  Switch input (DDC, if supported)
  DisplayPort 1 / HDMI 1 / HDMI 2 / LG alt variants
  Controls
  Brightness ────●──── 75%
  Contrast   ──────●── 70%
  Volume     ───────●─ 85%
  Mute
Settings…
Refresh
Quit Lumen
```

## Building

```sh
cargo build --release          # seconds, ~1.8 MB binary
cargo run --bin lumen          # status item appears immediately
./scripts/bundle.sh            # builds and signs dist/Lumen.app
./scripts/release.sh           # zip and DMG in dist/
open dist/Lumen.app
```

Requires macOS 27 on Apple silicon. No accessibility or root permissions; Screen Recording is only
asked for when you capture or preview a display.

## Development

The debug binary shares the installed app's support directory by default. Use the dev harness when
you need isolation or a real bundle identity:

```sh
./scripts/dev/run.sh                 # builds dist/Lumen Dev.app, launches with its own support dir
./scripts/dev/stop.sh                # stops only the pid in dist/lumen-dev.pid
./scripts/dev/reset.sh               # clears the dev support dir
./scripts/dev/shot.sh --page virtual --screen agent --out /tmp/settings.png
```

- The dev app uses `~/Library/Application Support/Lumen Dev`, so it cannot overwrite the installed
  config, screens or agent profiles.
- `LUMEN_SUPPORT_DIR` selects a support directory for any build, including the CLI.
- `lumen settings [--page general|displays|virtual|about] [--screen invocation|agent|main|last]`
  opens a page through the control socket. `lumen settings state` reports the page and screen.
- `scripts/dev/ax.swift` lists windows and presses or sets controls by title or description.

Settings placement is a preference: display I click from, Agent, main display, or last display.
The default is the display whose menu bar the command came from.

## Packaging

`scripts/bundle.sh` builds the app bundle, compiles `assets/lumen.icon`, writes the version and build
number, and signs with a stable code-signing identity. The identity matters: TCC grants and the
`SMAppService` login item are keyed to the signature, and ad-hoc signing changes it on every build.

- `LUMEN_SIGN_IDENTITY` selects the identity; otherwise the first code-signing identity in the
  keychain is used, and ad-hoc is the fallback with a warning.
- `LUMEN_VERSION` (default `0.1.0`) and `LUMEN_BUILD` (default: git commit count) feed the plist.
  `lumen --version` and Settings > About show them.
- `scripts/release.sh` produces `dist/Lumen-<version>.zip` and `.dmg`; the DMG contains the app and
  an Applications link and is signed with the same identity.

The icon source is `assets/lumen.icon`, an Icon Composer document. The bundle script compiles it with
`actool` into `Assets.car` (layered, including the Liquid Glass appearance) and a fallback
`AppIcon.icns`, then sets `CFBundleIconName`. `assets/lumen_icon.svg` is the exported artwork.

Notarisation is not wired in. It needs a Developer ID Application certificate and hardened runtime,
and the private display API means direct distribution only, never the App Store.

## How the app is put together

- **Status item and menu**: the menu bar shows the `sparkles.tv` SF Symbol as a template image, so
  it follows light/dark appearance. Lumen owns its `NSStatusItem` and `NSMenu` directly (no tray library).
  The menu rebuilds itself in `menuNeedsUpdate:` each time it opens, and its control rows are
  custom item views holding real `NSSlider`s, so brightness, contrast and volume can be dragged
  right in the menu. Sliders sit in the display's own menu rather than a nested submenu, because
  a second-level submenu opens off-screen when the status item is near the right edge.
- **Settings window** (`src/bin/lumen/ui.rs`): a source list on the left (General, Displays,
  Virtual Displays, About) and scrolling grouped pages on the right, 920 pt wide and resizable.
  One target class handles every control; `tag` values say which input row or screen a button
  belongs to. Pages rebuild in place after model changes, while unsaved text edits are carried
  across rescans. Closing releases the window and its views.
- **DDC engine** (`src/ddc`): own IOKit/CoreDisplay bindings. Finds the display's `IOAVService`
  by matching the port node in the IORegistry path, picks the chip address (`0x37`, or `0xB7`
  behind the M1 HDMI bridge), reads and writes VCP features over I2C, and parses capabilities
  strings for advertised input codes.
- **Signal control** (`src/native`): `CGSConfigureDisplayEnabled`, resolved with `dlsym`.
- **Virtual screens** (`src/bin/lumen/screens.rs`): `CGVirtualDisplay`, a private CoreGraphics API
  declared by name and driven with `msg_send`, because there is no public way to create a display.
  A screen is a real display in the Window Server with no physical output; the menu lists it and
  `lumen virtual ...` creates, measures, captures and removes it.
- **Control socket** (`src/bin/lumen/control.rs`): a Unix socket at
  `~/Library/Application Support/Lumen/control.sock`, mode 0600, one JSON request and one reply per
  connection. The menu-bar app owns the displays, so the CLI and agent harnesses ask it over this
  socket rather than creating anything themselves.
- **`src/bin/probe.rs`**: `cargo run --bin lumen-probe -- list` for debugging a display.

Settings persist to `~/Library/Application Support/Lumen/config.json`: behaviour switches, each
display's menu input list (label, VCP value, standard vs LG-alt addressing, enabled), virtual
display defaults, agent defaults (headless, home page, reopen), the last agent launch and the
capture limit. Released displays are also recorded there, so a crash cannot leave an output
switched off: the next launch reclaims them before doing anything else.

Launch at login uses `SMAppService` (macOS 13+). It accepts the ad-hoc signed bundle and shows up
under System Settings > General > Login Items; the switch reflects the live registration status,
including the "needs approval" case. The app also rescans after display configuration changes and
on wake, so the menu stays correct without pressing Refresh.

## Display controls

The Displays page has sliders for brightness, contrast and volume, and the display's menu holds
the same sliders as menu item views plus a mute toggle. Sliders move continuously but pull
magnetically towards five points - 0, 25, 50, 75 and 100 percent - when within 3% of the range, so
those values are easy to repeat while anything in between is still reachable. Holding Option
bypasses the pull for exact values. Drags are coalesced into one DDC write every 150 ms and update
the readout immediately. The editor marks the input the display
reports as active (VCP 0x60) or the one this app last switched to, and the input list scrolls
with the page.

Each input row has a **Test** button. It asks for confirmation first, then sends that input's
command (with the protocol fallbacks) and records whether the display actually left: a ✓ or ✗
badge in the row, persisted in the config. The warning matters because a monitor that accepts the
command switches away and some firmware cannot switch back over DDC - you may need the monitor's
own controls. Successful switches from the menu are recorded the same way, so the list also
learns from normal use.

## Memory

Measured with `footprint`, which is what Activity Monitor reports; `ps` RSS counts shared library
pages and reads much higher.

| state | footprint |
| --- | --- |
| launched, settings never opened | 13 MB |
| settings open (General / Virtual Displays) | 27-31 MB |
| settings closed again | 31 MB |
| idle CPU | 0% |

The launch state is a status item and nothing else. Opening settings brings AppKit's window
machinery and the view hierarchy (~14 MB); closing releases the window and its views, while
AppKit keeps fonts and window-server state for the rest of the process. A Rune-rendered settings
window was measured at 16 MB / 44-53 MB / 31-32 MB for the same three states with a 25 MB binary;
the native window won on both size and build time, so Rune was dropped (the comparison spike is
kept in `spikes/native-settings`).

## Input switching

The app has two ways to move a display to another input.

**DDC input select.** Standard VCP `0x60`, plus the "LG alt" variant (VCP `0xF4` addressed at
`0x50`) that some LG firmware expects. Precise, and works on displays whose firmware implements
input select.

**Release to other input.** Some monitors - including the LG ULTRAWIDE this was built against -
report `VCP 0x60` as bogus (max 18, current `0x00`) and ignore every input-select write, from
standard and LG-alt addressing and from every third-party tool. Their firmware only switches when
the current signal disappears.

The display submenu marks the input the Mac is on: bold title plus checkmark. The monitor's own
report (`VCP 0x60`) wins when it answers, then the input this app last switched to, remembered in
`config.json`, then the first enabled input. The fallback exists because the LG answers `0x60`
with zero and ignores input-select writes, so nothing else would ever be marked.

Release stops macOS from driving the display's output. With the monitor's OSD **Auto Input
Switch** enabled, the monitor locks onto whichever HDMI/DP input still has a live signal.
**Bring <display> back** drives the output again and the monitor follows the returning signal.
Verified round trip on the LG.

## Virtual screens

A virtual screen exists only in memory. Nothing appears on your monitors, but macOS treats it as
a real display: it has its own Space, its own menu bar, and can be arranged, mirrored, captured
and used as a recording source.

```sh
lumen virtual ensure --name Agent            # create, prints display id and bounds
lumen virtual list                           # all live virtual screens
lumen virtual bounds --name Agent            # geometry after any rearrangement
lumen virtual screenshot --name Agent --out frame.png
lumen virtual preview --name Agent            # floating live window, --hide to close
lumen virtual reboot --name Agent             # drop and recreate, for testing recovery
lumen virtual rename --name Agent --to Workspace
lumen virtual remove --name Agent
```

Screens are persisted in `config.json` with `auto_create`, so they are recreated after launch and
after wake. The default is 1920x1080 at 1x, placed to the right of the main display; the origin is
read back from `CGDisplayBounds`, never pinned (pinning keeps the display registered until the
process exits and breaks removal). At most four virtual screens can exist, sizes are limited to
640x480 through 7680x4320, and captures are pruned per screen using the limit in Settings
(default: newest 20).

Settings > Virtual Displays edits the name, geometry, 1x/2x, refresh and auto-create of each
screen, with preview, apply and remove. Renaming recreates the display because its name is fixed
at creation: the agent browser on it is closed, its capture folder moves, and its desktop
position may change. A rename guard keeps a display rescan from restoring the old name halfway
through. Each screen's menu entry also has preview and remove actions.

The same page holds the agent workspace defaults (start headless, home page, reopen the last
session at launch) and capture retention (keep the last 5, 20, 50 or all, open or delete the
folder).

### Handing a screen to an agent

```sh
lumen agent chrome --name Agent --url https://example.com
```

```sh
lumen agent start --name Agent --url https://example.com      # headless by default
lumen agent start --name Agent --windowed                     # show a window on the screen
lumen agent screenshot --name Agent --out page.png
lumen agent status --name Agent              # session, window bounds, whether it is on screen
lumen agent move --name Agent --left 100 --top 100
lumen agent stop --name Agent
```

`agent start` returns the CDP endpoint, port, profile and screen geometry. Lumen launches a
Chromium-family browser with a dedicated profile and remote debugging, positioned on the virtual
screen. The agent then connects with Playwright's `chromium.connectOverCDP(endpoint)`. No
synthetic input is used, so the cursor never moves. Starting again reuses a live session instead
of launching a second browser.

Agent sessions are headless by default: Chromium's `--headless=new` works over CDP, creates no
window, and never activates the app, so there is no focus flash. Nothing is visible on the
virtual screen in that mode, so use `agent screenshot`, which captures the page over CDP with no
Screen Recording permission. `--windowed` opts into a visible window on the virtual display.

Sessions are tracked by pid and mirrored to `config.json`. `agent stop`, `virtual remove` and
quitting Lumen close the browser after checking that the pid still runs the recorded profile, so a
pid cannot be reused into killing something else. A record left by a crash is closed on the next
launch. When a display change moves the window, Lumen re-homes it with CDP `Browser.setWindowBounds`
and `agent status` reports whether the window is still on its screen.

Browser detection tries installed Chromium browsers (Chrome, Chromium, Brave, Edge) and then the
Chrome for Testing build in the Playwright cache. `--browser <bundle-id|path.app>` selects one
explicitly.

What the launch guarantees, and what it does not:

| Guarantee | How |
| --- | --- |
| Cursor does not move | no `CGEvent` anywhere in the path |
| Your app is in front before and after | the previous front app is restored once the browser window appears |
| The browser window is off your display | pinned position on the virtual screen |
| No DDC, no input switching | virtual screens have no DDC path |

Chromium activates itself when it creates its first window, and neither `activates = false` nor
`open -g` changes that, so a windowed session has a brief focus flash. Headless sessions do not. Lumen puts the previous app back only if
the browser is still frontmost, so if you switched elsewhere during the launch it leaves you
alone. Clipboard and native browser dialogs remain global: drive pages through CDP, and pre-grant
permissions in the profile rather than clicking dialogs.

## Caveats

- Release needs the monitor's OSD **Auto Input Switch** on.
- While released, the display is gone from System Settings (macOS isn't driving it). Quitting
  Lumen reclaims it; if the app is force-quit while a display is released, log out and back in
  (or re-plug the cable) to get it back.
- The LG stops answering VCP reads for a moment after the capabilities string is fetched (it takes
  several chunks), so a scan reads brightness, contrast and volume first and the input list last.
  In the other order those control reads come back empty and the menu loses its sliders.
- The settings window is fixed-size; dynamic resizing would need Auto Layout constraints.
- `CGVirtualDisplay` is private API. It works on macOS 27.3 (measured) and has shipped in other
  apps for years, but an OS update can break it, and the App Store does not accept it.
- Recreating a display with the same identity immediately after removal can return nil from
  `initWithDescriptor:`; Lumen waits for the old display to leave the active list first, and puts
  the previous screen back if creation still fails.
- Removing a virtual screen releases only the display object. Releasing the descriptor first
  crashes in `-[CGVirtualDisplay dealloc]`, so the descriptor, settings and mode array are left to
  the process exit: three small objects per removed screen.
- Screenshots and any preview need Screen Recording permission when Lumen runs as a signed bundle.
  The virtual display itself needs no permission to create or place windows on.

## Roadmap

- Global hotkeys for release/reclaim.
