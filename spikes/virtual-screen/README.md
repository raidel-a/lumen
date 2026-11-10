# Virtual screen spike

Question: on this macOS, does the private `CGVirtualDisplay` API still create a working display,
what does it cost, and can we capture it?

Run on macOS 27.3 (build 26C5035r), Apple Silicon, with two physical displays attached
(internal 2560x1600 Retina, LG ULTRAWIDE 2560x1080).

## Usage

```bash
cargo run -- --methods          # list selectors of the four private classes
cargo run -- --quick            # create, release, report whether it disappeared
cargo run -- --quick --no-pin   # same, without pinning the origin
cargo run                       # full run: create, place a window, capture, benchmark, remove
LUMEN_PIN=session cargo run -- --quick   # try a different CGConfigureOption
```

The binary writes BMP captures to the OpenCode temp directory. `--methods` is safe; everything
else creates a real display for a few seconds.

## API shape found at runtime

The SDK has no headers for these, so the selectors were enumerated with `class_copyMethodList`:

```
CGVirtualDisplay:          initWithDescriptor:, applySettings:, displayID, name, hiDPI, modes,
                           rotation, terminationHandler, red/green/bluePrimary, whitePoint
CGVirtualDisplayDescriptor: setName:, setMaxPixelsWide:, setMaxPixelsHigh:, setSizeInMillimeters:,
                           setVendorID:, setProductID:, setSerialNum:, setDispatchQueue:/setQueue:,
                           setTerminationHandler:, primaries, setDisplayInfoValue:forKey:
CGVirtualDisplaySettings:  setModes:, setHiDPI:, setIsReference:, setRotation:, setRefreshDeadline:
CGVirtualDisplayMode:      initWithWidth:height:refreshRate:
                           initWithWidth:height:refreshRate:transferFunction:
```

There is no terminate or invalidate method; removal is deallocation.

## Measurements

All numbers are from this machine, not estimates.

| Measurement | Result |
| --- | --- |
| Time until the display is in `CGGetActiveDisplayList` | 75-180 ms |
| Total create, including class lookup and settings | 0.38-0.52 s |
| Default placement | right of the main display, origin 1680,0 |
| Bounds and pixels | 1920x1080 at 1x |
| One-shot capture, warm | 12.6-27 ms |
| Sustained capture loop | 75-84 fps, 3% of one core |
| Magenta window pixels in the virtual capture | 19.3% |
| Magenta window pixels in the main capture | 0% |
| User windows moved by create and remove | none, window list identical before and after |
| Process footprint during capture | 42 MB (spike, includes AppKit; Lumen itself is 13 MB) |
| Removal | `release` of all objects plus a 3 s run-loop pump |

## Findings that change the design

1. **It works on 27.3.** Creation, modes, HiDPI, placement and capture all behaved.

2. **Never pin the origin.** `CGConfigureDisplayOrigin` keeps the display registered in the Window
   Server until the owning process exits, for all three `CGConfigureOption` values. After that,
   releasing every object no longer removes it. Without pinning, release plus a run-loop pump
   removes it cleanly. The unpinned default is already deterministic here: directly right of the
   main display at 1680,0, read back with `CGDisplayBounds`.

3. **AppKit clamps windows created too early.** A window created before AppKit has noticed the new
   display was silently moved to the main screen (requested x 1760, got 640). Pumping the run loop
   for about a second after creation, then calling `setFrameOrigin`, places it correctly, and the
   window then does not appear on the main display.

4. **The virtual display is a full desktop**, not a blank framebuffer: its own Space, its own menu
   bar, the default wallpaper. A window placed there is invisible on the physical displays.

5. **Capture was not blocked by TCC here.** `CGDisplayCreateImage` returned frames, including other
   apps' windows on the main display, because the parent terminal already holds Screen Recording.
   A standalone Lumen bundle will prompt. One-shot capture is fast enough for a PIP, but it
   allocates a full BGRA image per frame (about 8 MB at 1920x1080), so the shipping implementation
   should use a stream or reuse buffers.

6. **Window reflow did not happen**, contrary to the usual "plugging in a display reflows windows"
   warning. The window list was byte-identical before and after. This may be because the arrangement
   was already recorded; a first-ever creation could still move something, so the product should
   create screens at login rather than per task.

## Not tested

- Sleep and wake. Virtual displays are expected to disappear; recreation is the planned recovery.
- More than one virtual screen at a time.
- The TCC prompt for a signed app bundle.
- Chrome placement and launch without activation, which belongs to the agent phase.
