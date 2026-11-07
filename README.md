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
cargo build --release          # seconds, ~1 MB binary
cargo run --bin lumen          # status item appears immediately
./scripts/bundle.sh            # builds dist/Lumen.app (LSUIElement, no Dock icon)
open dist/Lumen.app
```

Requires macOS 14+ on Apple silicon. No accessibility, screen recording, or root permissions.

## How the app is put together

- **Status item and menu**: Lumen owns its `NSStatusItem` and `NSMenu` directly (no tray library).
  The menu rebuilds itself in `menuNeedsUpdate:` each time it opens, and its control rows are
  custom item views holding real `NSSlider`s, so brightness, contrast and volume can be dragged
  right in the menu. Sliders sit in the display's own menu rather than a nested submenu, because
  a second-level submenu opens off-screen when the status item is near the right edge.
- **Settings window** (`src/bin/lumen/ui.rs`): plain AppKit views built with `objc2`, laid out with
  frames in a fixed 720x540 content area. One target class handles every control; `tag` values
  say which input row a button belongs to. The window is built on first open, rebuilt in place
  after structural changes, and released on close (the delegate drops it after the close
  notification), so a closed window costs nothing while the menu keeps working.
- **DDC engine** (`src/ddc`): own IOKit/CoreDisplay bindings. Finds the display's `IOAVService`
  by matching the port node in the IORegistry path, picks the chip address (`0x37`, or `0xB7`
  behind the M1 HDMI bridge), reads and writes VCP features over I2C, and parses capabilities
  strings for advertised input codes.
- **Signal control** (`src/native`): `CGSConfigureDisplayEnabled`, resolved with `dlsym`.
- **`src/bin/probe.rs`**: `cargo run --bin lumen-probe -- list` for debugging a display.

Settings persist to `~/Library/Application Support/Lumen/config.json`: behaviour switches plus
each display's menu input list (label, VCP value, standard vs LG-alt addressing, enabled).
Released displays are also recorded there, so a crash cannot leave an output switched off: the
next launch reclaims them before doing anything else.

Launch at login uses `SMAppService` (macOS 13+). It accepts the ad-hoc signed bundle and shows up
under System Settings > General > Login Items; the switch reflects the live registration status,
including the "needs approval" case. The app also rescans after display configuration changes and
on wake, so the menu stays correct without pressing Refresh.

## Display controls

The Displays page has sliders for brightness, contrast and volume, and the display's menu holds
the same sliders as menu item views plus a mute toggle. Drags are coalesced into one DDC write
every 150 ms and update the readout immediately. The editor marks the input the display
reports as active (VCP 0x60) or the one this app last switched to, and the settings window grows
or shrinks to fit the input list.

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
| settings open | 27 MB |
| settings closed again | 26 MB |
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

Release stops macOS from driving the display's output. With the monitor's OSD **Auto Input
Switch** enabled, the monitor locks onto whichever HDMI/DP input still has a live signal.
**Bring <display> back** drives the output again and the monitor follows the returning signal.
Verified round trip on the LG.

## Caveats

- Release needs the monitor's OSD **Auto Input Switch** on.
- While released, the display is gone from System Settings (macOS isn't driving it). Quitting
  Lumen reclaims it; if the app is force-quit while a display is released, log out and back in
  (or re-plug the cable) to get it back.
- The settings window is fixed-size; dynamic resizing would need Auto Layout constraints.

## Roadmap

- Global hotkeys for release/reclaim.
