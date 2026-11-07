# Lumen

A macOS menu bar app for controlling external displays over DDC/CI, built as the start of an
open BetterDisplay-style tool. First job: switching the monitor between the Mac and another
machine.

```
LG ULTRAWIDE
  Switch input (DDC)      DisplayPort 1 / HDMI 1 / HDMI 2 ...
  Release to other input  (auto-switch)
  Brightness 63%          25% / 50% / 75% / 100%
  Contrast 70%
  Volume 85%
```

## Building

```sh
cargo run --bin lumen       # from a terminal, status item appears immediately
./scripts/bundle.sh         # builds dist/Lumen.app (LSUIElement, no Dock icon)
open dist/Lumen.app
```

Requires macOS 13+ on Apple silicon. No accessibility, screen recording, or root permissions.

## Input switching

The app has two ways to move a display to another input.

**DDC input select.** Standard VCP `0x60`, plus the "LG alt" variant (VCP `0xF4` addressed at
`0x50`) that some LG firmware expects. This is precise: the monitor switches to the named port
and the Mac loses the display. Works on displays whose firmware implements input select.

**Release to other input.** Some monitors - including the LG ULTRAWIDE this was built against -
report `VCP 0x60` as unsupported (`max` 18, current `0x00`) and silently ignore every input
select write, from both standard and LG-alt addressing, and from every third-party tool. Their
firmware only switches when the current signal disappears.

"Release to other input" stops macOS from driving the display's output (via the private
`CGSConfigureDisplayEnabled`). With the monitor's OSD **Auto Input Switch** enabled, the monitor
then locks onto whichever HDMI/DP input still has a live signal. `Bring <display> back` drives
the output again and the monitor follows the returning signal. The LG verifies this round trip.

Because macOS stops driving the output, the display disappears from System Settings while it is
released. Quitting Lumen reclaims everything it released; if the app is force-quit while a
display is released, log out and back in (or re-plug the cable) to get it back.

## How it is put together

- `src/ddc` - the DDC/CI engine. Own IOKit/CoreDisplay bindings: find the `IOAVService` that
  belongs to a display by matching the port node in the IORegistry path, pick the right chip
  address (`0x37`, or `0xB7` behind the M1 HDMI bridge), then read and write VCP features over
  I2C. Capabilities strings are parsed for advertised input codes.
- `src/native` - display signal control through `CGSConfigureDisplayEnabled`, resolved with
  `dlsym`.
- `src/bin/lumen` - the status item. A `winit` event loop with `tray-icon`/`muda` for the native
  menu; hardware work runs on worker threads and results come back as events.
- `src/bin/probe.rs` - `cargo run --bin lumen-probe -- list` for debugging a display.

## Roadmap

- Rune-rendered popover panel instead of the native menu (sliders, per-display cards).
- Global hotkeys for "release" / "reclaim".
- Brightness and contrast sliders with caching.
- Per-display presets and a remembered "last input".
- Round-trip that also works on monitors without auto input switch.
