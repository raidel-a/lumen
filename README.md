# Lumen

A macOS menu bar display controller: native menu, Rune-rendered settings. Built as the start of
an open BetterDisplay-style tool. First job: switching a monitor between the Mac and another
machine.

```
LG ULTRAWIDE
  Release to other input (auto-switch)
  ────────────────────────
  Switch input (DDC, if supported)
  DisplayPort 1 / HDMI 1 / HDMI 2 / LG alt variants
  Brightness 75%   25% 50% 75% 100%
  Contrast 70% / Volume 85%
Settings…
Refresh
Quit Lumen
```

## Building

```sh
cargo build --release          # ~2 min cold, 25 MB binary
cargo run --bin lumen          # status item appears immediately
./scripts/bundle.sh            # builds dist/Lumen.app (LSUIElement, no Dock icon)
open dist/Lumen.app
```

Requires macOS 13+ on Apple silicon. No accessibility, screen recording, or root permissions.

## How the app is put together

- **Rune hosts the event loop.** `src/bin/lumen/main.rs` builds a `rune::App` with a hidden
  settings window (`.keep_running().accessory().window_options(visible: false)`), then
  `app.run()`.
- **The menu bar is native.** `tray-icon`/`muda` create the `NSStatusItem`; a thread receives
  menu events and hands them to `AppHandle::update`, which runs on the UI thread. That is where
  the menu is rebuilt and the settings window opens.
- **The settings window is Rune.** `settings.rs` draws General and Displays with `tok` styling;
  `cargo run --bin lumen -- --snap out.png` and `--scene displays-full` render it headlessly for
  layout checks.
- **DDC lives in `src/ddc`.** Own IOKit/CoreDisplay bindings: find the display's `IOAVService` by
  matching the port node in the IORegistry path, pick the chip address (`0x37`, or `0xB7` behind
  the M1 HDMI bridge), read and write VCP features over I2C, parse capabilities strings.
- **Signal control lives in `src/native`.** `CGSConfigureDisplayEnabled`, resolved with `dlsym`.
- **`src/bin/probe.rs`** is the debugging CLI: `cargo run --bin lumen-probe -- list`.

Settings persist to `~/Library/Application Support/Lumen/config.json`: behaviour switches plus
each display's menu input list (label, VCP value, standard vs LG-alt addressing, enabled).

## Input switching on a stubborn LG

The LG ULTRAWIDE this was built against reports `VCP 0x60` as bogus (max 18, current `0x00`) and
silently ignores every input-select write - standard `0x60`@`0x51`, LG-alt `0xF4`@`0x50`, every
value 0-255, and BetterDisplay's own CLI. Its firmware only switches when the current signal
disappears.

**Release to other input** stops macOS from driving the display's output. With the monitor's OSD
**Auto Input Switch** enabled, the monitor locks onto whichever HDMI/DP input still has a live
signal. **Bring <display> back** drives the output again and the monitor follows the returning
signal. Verified round trip on the LG.

The DDC path remains for monitors that implement input select properly; both are always offered.

## Caveats

- Release needs the monitor's OSD **Auto Input Switch** on.
- While released, the display is gone from System Settings (macOS isn't driving it). Quitting
  Lumen reclaims it; if the app is force-quit while a display is released, log out and back in
  (or re-plug the cable) to get it back.
- Rune is pre-release; the dependency is pinned to a `dev` revision.

## Roadmap

- Global hotkeys for release/reclaim.
- "Learn inputs": probe a display and record which values actually switch.
- Launch at login.
- Brightness/contrast sliders in the settings window instead of menu presets.
