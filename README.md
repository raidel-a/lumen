# Lumen

A macOS menu bar display controller: native status item, native AppKit settings window. Built as
an open menu bar display-control tool. First job: switching a monitor between the Mac and
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
Built-in Display
  Controls
  Brightness ──────●── 54%
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

Requires macOS 27 on Apple silicon. No accessibility or root permissions for normal display
control; Screen Recording is only asked for when you capture or preview a display, and brightness
keys need the macOS 27 System & App Protection Override, which Lumen does not request automatically.

## Development

The debug binary shares the installed app's support directory by default. Use the dev harness when
you need isolation or a real bundle identity:

```sh
./scripts/dev/run.sh                 # builds dist/Lumen Dev.app, launches with its own support dir
./scripts/dev/stop.sh                # stops only the pid in dist/lumen-dev.pid
./scripts/dev/reset.sh               # clears the dev support dir
./scripts/dev/shot.sh --page virtual --screen agent --out /tmp/settings.png
./scripts/dev/smoke.sh               # pages, agent start/capture/stop, screen create/remove
./scripts/dev/axcli.sh menu          # snapshot the dev menu-bar extra via agent-desktop
./scripts/check.sh --smoke           # tests, signed bundle, signature, smoke test
```

- The dev app uses `~/Library/Application Support/Lumen Dev`, so it cannot overwrite the installed
  config, screens or agent profiles.
- `LUMEN_SUPPORT_DIR` selects a support directory for any build, including the CLI.
- `lumen settings [--page general|displays|virtual|about] [--screen invocation|agent|main|last]`
  opens a page through the control socket. `lumen settings state` reports the page and screen.
- `scripts/dev/ax.swift` lists windows and presses or sets controls by title or description.
- `scripts/dev/axcli.sh` drives the dev app through [agent-desktop](https://github.com/vercel-labs/agent-desktop),
  which reads the accessibility tree as JSON with a ref per element (`menu`, `window`, or
  `raw -- <args>`). It targets only "Lumen Dev". agent-desktop is pinned in `mise.toml`; run
  `mise install`, then grant Accessibility via `mise exec -- agent-desktop permissions --guide`.
- `lumen quit` asks the running app to quit cleanly, releasing virtual displays and agent
  browsers. `scripts/dev/stop.sh` uses it before falling back to the recorded PID.

Settings placement is a preference: display I click from, Agent, main display, or last display.
The default is the display whose menu bar the command came from.

## Packaging

`scripts/bundle.sh` builds the app bundle, compiles `assets/lumen.icon`, writes the version and build
number, and signs with a stable code-signing identity under the hardened runtime. The identity
matters: TCC grants and the `SMAppService` login item are keyed to the signature, and ad-hoc signing
changes it on every build.

- `LUMEN_SIGN_IDENTITY` selects the identity; otherwise the **Developer ID Application** identity is
  used when present, then the first code-signing identity, with ad-hoc as the fallback (with a
  warning).
- With a real identity the app is signed with `--options runtime --timestamp` (both required for
  notarisation). The vendored Sparkle framework ships ad-hoc; re-signing its nested code with the
  same Developer ID gives it our Team ID, so hardened-runtime library validation is satisfied and no
  `disable-library-validation` entitlement is needed. Ad-hoc fallback stays runtime- and
  timestamp-free so pure-local dev builds still load.
- `LUMEN_VERSION` defaults to the last git tag, with a `-dev.<commits>` suffix when HEAD is ahead
  of it. Releases pass the tag version explicitly. `LUMEN_BUILD` defaults to the git commit count.
  `lumen --version` and Settings > About show both. A local build after `v0.1.5` reports
  `0.1.5-dev.N` instead of an old `0.1.0`, so Settings > About and Sparkle's "newest version"
  dialog agree; the build number still decides whether Sparkle offers an update.
- `scripts/release.sh` produces `dist/Lumen-<version>.zip` and `.dmg`; the DMG contains the app and
  an Applications link and is signed with the same identity.

The icon source is `assets/lumen.icon`, an Icon Composer document. The bundle script compiles it with
`actool` into `Assets.car` (layered, including the Liquid Glass appearance) and a fallback
`AppIcon.icns`, then sets `CFBundleIconName`. `assets/lumen_icon.svg` is the exported artwork.

### Notarisation

`scripts/release.sh` notarises and staples when notary credentials are available; the private display
API means direct distribution only, never the App Store. The flow signs the `.app`, submits it to
Apple's notary service, staples the ticket to the `.app` (a zip cannot be stapled), then rebuilds the
zip and dmg from the stapled bundle and staples the dmg. The appcast is generated last over the final
stapled zip so its `edSignature` and length match the published bytes. A non-`Accepted` result fails
the release and prints the `notarytool` log.

Credentials reach the script two ways, checked in this order:

- **Local**: a `notarytool` keychain profile named by `LUMEN_NOTARY_PROFILE` (default `lumen-notary`).
  Create it once (see below). If it does not exist, release.sh still builds and signs but skips
  notarisation with a pointer to `store-credentials`.
- **CI**: the explicit App Store Connect API key triplet `LUMEN_NOTARY_KEY` (path to the `.p8`),
  `LUMEN_NOTARY_KEY_ID`, and `LUMEN_NOTARY_ISSUER`. The GitHub Actions workflow uses this form.

The Developer ID identity is kept as an encrypted P12 at
`~/Library/Application Support/Lumen/signing/DeveloperID.p12` (mode 600), outside the repository and
never committed. Its password is stored in the login keychain as a generic password:
`security find-generic-password -s lumen-developer-id-p12 -a "$USER" -w`.

#### One-time: create the notary keychain profile (local)

```
xcrun notarytool store-credentials "lumen-notary" \
  --key   <path to AuthKey_XXXXXXXXXX.p8> \
  --key-id <10-char key id> \
  --issuer cb2fff29-78dd-46b3-9519-4faff2a64499
```

Create the App Store Connect API key (and later rotate it) at App Store Connect → Users and Access →
Integrations → App Store Connect API, with the **Developer** role. Download the `.p8` once (Apple does
not let you re-download it), note its key ID, and keep it with the other signing material. To rotate,
create a new key, re-run `store-credentials`, and update the CI secrets.

### One-time migration note: Apple Development → Developer ID

The first Developer-ID-signed build changes the app's code signature (new Team ID and certificate).
macOS keys TCC permissions and the `SMAppService` login item to the signature, so **existing installed
copies will, once, re-prompt for Screen Recording and re-register the login item** after they
update to the first Developer-ID build. If brightness keys are enabled, they also need the macOS 27
System & App Protection Override again.

Replacing or renewing the Developer ID certificate later has the same effect: the override grant is
keyed to the leaf certificate, so one manual re-grant is required per certificate, not per build.
Builds signed with the same certificate keep the grant.

### Releasing from CI (GitHub Actions)

`.github/workflows/release.yml` runs on a `v*` tag push: it builds, signs, notarises, staples,
publishes a GitHub Release with the zip and dmg, and pushes the regenerated `appcast.xml` to `main`.
It imports the Developer ID identity into a throwaway keychain and deletes it (and every decoded
secret file) in an `always()` cleanup step.

Required repository secrets (Settings → Secrets and variables → Actions):

| Secret | What it is | How to produce it |
| --- | --- | --- |
| `DEVELOPER_ID_P12_BASE64` | Developer ID Application identity (cert + private key + Developer ID G2 intermediate) as a base64 `.p12` | build it with `openssl` (see below), then `base64 -i DeveloperID.p12 -o DeveloperID.p12.b64` |
| `DEVELOPER_ID_P12_PASSWORD` | the `-passout` password used to create that `.p12` | choose it at creation time; keep it in the login keychain |
| `KEYCHAIN_PASSWORD` | any strong string; unlocks the throwaway CI keychain | generate a random string |
| `NOTARY_KEY_P8_BASE64` | App Store Connect API key, base64 | `base64 -i AuthKey_XXXXXXXXXX.p8 \| pbcopy` |
| `NOTARY_KEY_ID` | the 10-char key ID of that API key | shown in App Store Connect next to the key |
| `NOTARY_ISSUER_ID` | the App Store Connect issuer UUID | `cb2fff29-78dd-46b3-9519-4faff2a64499` |
| `SPARKLE_ED_PRIVATE_KEY` | the Sparkle EdDSA private key | `./vendor/Sparkle/bin/generate_keys -x key.txt` then paste the file's contents (keep it secret; delete the file) |

The workflow never echoes a secret; each is written to a file or piped straight into `security` /
`notarytool`.

#### Replacing the Developer ID identity

Keychain export of an existing identity can fail with `The contents of this item cannot be
retrieved` when the private key is not exportable, so build the P12 directly from the key and the
downloaded certificate instead:

```
openssl pkcs12 -export -legacy \
  -inkey developer_id.key \
  -in    developer_id.pem \
  -certfile DeveloperIDG2CA.pem \
  -name "Developer ID Application: Raidel Almeida (MK26S2J3QC)" \
  -out   DeveloperID.p12 -passout file:pass.txt

security create-keychain -p test /tmp/check.keychain-db
security import DeveloperID.p12 -k /tmp/check.keychain-db -P "$(cat pass.txt)"
security find-identity -v -p codesigning /tmp/check.keychain-db   # expect the Developer ID identity
security delete-keychain /tmp/check.keychain-db
```

`-legacy` is required: macOS `security import` rejects the PKCS#12 format OpenSSL 4 writes by
default and reports it as a wrong password. `DeveloperIDG2CA.pem` is the "Developer ID - G2"
intermediate from the Apple Developer portal; without it the CI keychain cannot build the chain.

Then update `DEVELOPER_ID_P12_BASE64` and `DEVELOPER_ID_P12_PASSWORD`, run a release, and confirm the
new signature and notarisation. Update the stored P12 and the `lumen-developer-id-p12` keychain item
at the same time. Replacing the certificate requires one manual re-grant of the System & App
Protection Override (see above).

Remove the old identity from the login keychain once the new one is proven:

```
security delete-identity -Z <old SHA-1> ~/Library/Keychains/login.keychain-db
```

Two certificates with the same common name make `codesign --sign "Developer ID Application: ..."`
ambiguous, which breaks local builds and `scripts/dev/test-update.sh` even though CI still works
(it imports only one identity).

Retire the old certificate only after the new one has produced a signed, notarised release. The
portal may refuse to revoke a Developer ID certificate with "only be revoked by Apple Developer
Program Support"; if so, the old certificate stays listed until it expires and is no longer used by
CI or local signing.

## Self-update (Sparkle)

Lumen updates itself with [Sparkle 2](https://sparkle-project.org). The framework is vendored and
reached from Rust through a small Objective-C shim (`native/lumen_sparkle.m`), all gated behind the
`sparkle` Cargo feature (off by default, so `cargo test` and `lumen-probe` do not need the
framework). A **Check for Updates…** menu item triggers a manual check, and the updater also runs
Sparkle's background scheduler at launch.

### Vendoring Sparkle

The framework and its command-line tools are not committed (they are ~16 MB and gitignored). Fetch
them once on a fresh checkout:

```
./scripts/dev/fetch-sparkle.sh    # pinned to a version, verified by SHA-256, unpacked to vendor/Sparkle
```

`scripts/bundle.sh` and `scripts/release.sh` fail with a pointer to this script if `vendor/Sparkle`
is missing. The pinned version and checksum live in `scripts/dev/fetch-sparkle.sh` and
`vendor/Sparkle/VENDOR.md`.

### EdDSA signing keys

Sparkle verifies every update with an EdDSA (ed25519) signature.

- The **public key** is embedded in `Info.plist` as `SUPublicEDKey` by `bundle.sh`.
- The **private key** lives in your **login keychain** (account `ed25519`, the default used by
  Sparkle's `generate_keys`). Locally it is never printed, never written to disk, and never
  committed. In CI it is provided as the `SPARKLE_ED_PRIVATE_KEY` secret, written to a file only
  long enough to import into the throwaway keychain, then deleted in the cleanup step.
- `bundle.sh` creates the key pair on first run if none exists (`vendor/Sparkle/bin/generate_keys`),
  and reads the public half back with `generate_keys -p`.

To inspect or rotate the key:

```
./vendor/Sparkle/bin/generate_keys -p                 # print the current public key
./vendor/Sparkle/bin/generate_keys -x private-key.txt # export the private key (keep it secret; delete after)
./vendor/Sparkle/bin/generate_keys -f private-key.txt # import a private key into the keychain
```

Rotating the key is a breaking change: already-installed builds trust the old `SUPublicEDKey`, so a
new key only takes effect for users who first install a build carrying it. To rotate, generate a new
key, ship a build with the new `SUPublicEDKey`, and keep signing with the matching private key.

### Cutting a release

Tag-driven (recommended): push a `v*` tag and GitHub Actions does the rest (build, sign, notarise,
staple, publish the release, push the appcast). See *Releasing from CI* above for the required
secrets.

```
git tag v0.2.0 && git push origin v0.2.0
```

Locally:

```
LUMEN_VERSION=0.2.0 ./scripts/release.sh
```

`release.sh` requires `LUMEN_VERSION` unless HEAD is exactly a `v*` tag; it no longer falls back to
`0.1.0`, so an unversioned local build cannot be published by accident.

This builds and signs `Lumen.app` (hardened runtime, Sparkle embedded and nested-code-signed),
notarises and staples it when notary credentials are present (keychain profile `lumen-notary`, or the
`LUMEN_NOTARY_KEY`/`_KEY_ID`/`_ISSUER` triplet), produces `dist/Lumen-<version>.zip` (the Sparkle
update enclosure) and `dist/Lumen-<version>.dmg` (the website download), then runs `generate_appcast`
over the final zip to sign it and update `./appcast.xml`. Enclosure URLs point at the GitHub Releases
download path for the tag (`v<version>` by default; override with
`LUMEN_RELEASE_TAG`/`LUMEN_RELEASE_OWNER`/`LUMEN_RELEASE_REPO`). Without notary credentials the script
still builds and signs, prints a skip notice, and leaves the artefacts un-notarised.

Then, by hand when releasing locally (the script does not upload):

1. Create the GitHub release for the tag and upload `dist/Lumen-<version>.zip` (and the `.dmg` for
   the website).
2. Commit and push the updated `appcast.xml`. It is served from
   `https://raw.githubusercontent.com/raidel-a/lumen/main/appcast.xml`, which is the `SUFeedURL`
   baked into the bundle (override at build time with `LUMEN_FEED_URL`).

### Testing the update flow locally

`scripts/dev/test-update.sh` runs the whole flow against the project's own dev instance only (bundle
id `pro.lumen.Lumen.dev`, a dedicated support dir): it builds version N and N+1, serves a signed
appcast for N+1 on `http://localhost`, launches N, and confirms it checks, downloads, verifies the
EdDSA signature, installs, and relaunches as N+1 — then checks that the signing identity is unchanged
across the update. It never touches `/Applications/Lumen.app` or any other running app.

## How the app is put together

- **Status item and menu**: the menu bar shows the `sparkles.tv` SF Symbol as a template image, so
  it follows light/dark appearance. Lumen owns its `NSStatusItem` and `NSMenu` directly (no tray library).
  The menu rebuilds itself in `menuNeedsUpdate:` each time it opens, and its control rows are
  custom item views holding real `NSSlider`s, so brightness, contrast and volume can be dragged
  right in the menu. Sliders sit in the display's own menu rather than a nested submenu, because
  a second-level submenu opens off-screen when the status item is near the right edge.
- **Settings window** (`src/bin/lumen/ui.rs`): a native source list on the left (General,
  Displays, Virtual Displays, About) and scrolling grouped pages on the right, built with stack
  views and Auto Layout, 920 pt wide and resizable. The display list is a source list too, so both
  have keyboard selection and accessibility.
  One target class handles every control; `tag` values say which input row or screen a button
  belongs to. Pages rebuild in place after model changes, while unsaved text edits are carried
  across rescans. Closing releases the window and its views.
- **DDC engine** (`src/ddc`): own IOKit/CoreDisplay bindings. Finds the display's `IOAVService`
  by matching the port node in the IORegistry path, picks the chip address (`0x37`, or `0xB7`
  behind the M1 HDMI bridge), reads and writes VCP features over I2C, and parses capabilities
  strings for advertised input codes.
- **Signal control** (`src/native`): `CGSConfigureDisplayEnabled`, resolved with `dlsym`.
- **Previews** (`src/bin/lumen/preview.rs`): ScreenCaptureKit streams, one per screen, with a
  borderless floating window, hover controls and geometry stored in the config. The 0.2.2 bindings
  that match objc2 0.5 leave a few selectors out, so those are called with `msg_send!` in this
  module only.
- **Built-in brightness** (`src/bin/lumen/brightness.rs`): the private
  `DisplayServices.framework` (`DisplayServicesGetBrightness`, `SetBrightness`,
  `CanChangeBrightness`), resolved with `dlopen`/`dlsym` so the app still starts when the framework
  or a symbol is missing. Only the internal panel uses it.
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
reports as active (VCP 0x60), or the one this app last sent when the monitor cannot report it, and
the input list scrolls with the page.

If every VCP read fails, the Displays page shows which read failed and asks you to wake the
monitor and refresh, instead of just showing an empty controls group. DDC replies can also be
missed one at a time around an input switch or a link retrain, so a failed read keeps that
control's last value instead of removing its slider, and a switch schedules one more scan after
the monitor has had time to settle. The page says when it is showing the last known values.

## Built-in display

The internal panel has no DDC path, so Lumen lists it separately as **Built-in Display** (config key
`builtin`), ahead of external monitors in the menu and the Displays list, and drives its backlight
through the private DisplayServices framework. It exposes
brightness only: no contrast, volume, mute, input switching or release, and the Settings page omits
the input editor. The menu entry keeps the same brightness slider and tick points as external
displays.

The current value is re-read from the system each time the menu opens and each time Settings
rebuilds, so a change made with the keyboard or Control Center shows up without a polling timer.
If DisplayServices is unavailable, the entry still appears with a note explaining that brightness
cannot be read or set, rather than disappearing.

Each input row has a **Test** button. It asks for confirmation first, then sends that input's
command. If the display leaves the Mac, the row gets a ✓ badge. If the display stays connected,
Lumen asks what is on screen and records that answer, because some firmware keeps the old link
alive while showing another input. A ✗ is recorded only when the monitor rejects the write. The
warning matters because a monitor that accepts the command switches away and some firmware cannot
switch back over DDC - you may need the monitor's own controls. Switches from the menu remember
the chosen input, and a confirmed one also records it as verified.

## Brightness keys and HUD

Lumen can take over the Mac's brightness keys (F1/F2, or the dedicated sun keys) and drive
brightness through its own pipeline, showing an on-screen indicator in place of the system's. It
is off by default and lives under **Settings > General > Brightness Keys**.

- **Use the brightness keys** installs a `CGEventTap` on the `NSSystemDefined` events macOS uses
  for the brightness keys, reads each press, and - when it handles the key - swallows it so the
  system's own brightness HUD does not also appear. The keys act on **the display under the
  pointer**, so moving the mouse to a monitor and pressing the key adjusts that monitor. Built-in
  panels go through DisplayServices; external monitors go through DDC, reusing the same 150 ms
  write coalescing as the menu sliders, so holding a key ramps smoothly.
- **Step** sets how far one press moves brightness: Coarse (1/8 of the range), Normal (1/16,
  matching macOS) or Fine (1/32).
- **Indicator** chooses the HUD look, with two styles:
  - **Tahoe** - a 300x72 rounded pill in the top-right corner with a "Brightness" title, a sun
    glyph, a capsule level bar and the percentage.
  - **Classic** - a 200x200 pre-Tahoe panel centred near the bottom with a large sun glyph above a
    row of sixteen segment tiles, no percentage.
  Both use the system HUD material (`NSVisualEffectView` HUD-window) with a one-point border, fade
  in and out, and appear on the screen under the pointer.
- If the display under the pointer cannot be controlled (an external monitor with no readable
  brightness or a failing DDC write), the HUD shows a **not-available** state - a sun with a warning
  badge and a dimmed bar (or no filled tiles) - and nothing is changed; the key press is left for
  the system to handle. Lumen does not silently redirect the keys to another display.

On macOS 26 and earlier the tap needs **Accessibility** permission. On macOS 27 the permission is
the **System & App Protection Override** under Privacy & Security, the same grant input-intercepting
utilities use.
That grant is broad: it allows full data, photo, history, Mac-control and input access. Lumen does
not trigger the override prompt and does not request it from the toggle. To use F1/F2, grant it
yourself in System Settings > Privacy & Security; otherwise the toggle shows a note and the keys
stay with the system. The menu sliders work without any of this. Permission is keyed to the app's
signature, like the login item, so a stable signing identity keeps the grant across builds.

## Memory

Measured with `footprint`, which is what Activity Monitor reports; `ps` RSS counts shared library
pages and reads much higher.

| state | footprint |
| --- | --- |
| launched, settings never opened | 12 MB |
| settings open (source list, pages, live thumbnails) | 29 MB |
| settings closed again | 28 MB |
| preview open | 29 MB |
| idle CPU | 0% |
| preview visible, static captured screen | 0.2% CPU |

The launch state is a status item and nothing else. Opening settings brings AppKit's window
machinery and the view hierarchy (~14 MB); closing releases the window and its views, while
AppKit keeps fonts and window-server state for the rest of the process. A Rune-rendered settings
window was measured at 16 MB / 44-53 MB / 31-32 MB for the same three states with a 25 MB binary;
the native window won on both size and build time, so Rune was dropped (the comparison spike is
kept in `spikes/native-settings`).

## Input switching

The app has two ways to move a display to another input.

**DDC input select.** Standard VCP `0x60`, plus the "LG alt" variant (VCP `0xF4` addressed at
`0x50`) that some LG firmware expects. A monitor that switches cleanly removes the display from
macOS, which confirms the command. Some firmware keeps the old link and HPD alive while showing
another input, so the display stays online even though the switch happened. Lumen treats an
accepted write as **sent**, not as a failure, and does not try more protocol variants after it.
The Settings Test then asks whether the monitor switched and stores that answer as the input's
verified state.

**Release to other input.** Release stops macOS from driving the display's output. With the
monitor's OSD **Auto Input Switch** enabled, the monitor locks onto whichever HDMI/DP input still
has a live signal. **Bring <display> back** drives the output again and the monitor follows the
returning signal. Release cannot move a monitor whose Auto Input Switch is off: the monitor stays
on the now-dead input. Verified round trip on the LG, with Auto Input Switch on.

The display submenu marks the input the Mac is on: bold title plus checkmark. The monitor's own
report (`VCP 0x60`) wins when it answers, then the input this app last sent, remembered in
`config.json`, then the first enabled input. The fallback exists because some firmware, including
the LG this was built against, answers `0x60` with zero and writes `0xF4` with a constant, so the
active input cannot be read back.

The LG accepts the LG-alt writes (`0x90` HDMI 1, `0x91` HDMI 2, `0xD0` DisplayPort, `0xD1`/`0xD2`
USB-C depending on the model). It keeps the DisplayPort link alive while showing HDMI, so "the
display disappeared" is not a reliable success signal on this unit; the confirmation prompt is.

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
lumen virtual previews                        # previews that are open now
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
screen, with a live thumbnail, preview, apply and remove. Renaming recreates the display because its name is fixed
at creation: the agent browser on it is closed, its capture folder moves, and its desktop
position may change. A display mutation guard keeps a rescan from restoring a screen while it is
being created, removed or renamed. Each screen's menu entry also has preview and remove actions.

The same page holds the agent workspace defaults (start headless, home page, reopen the last
session at launch), an Agents panel that lists live sessions with Screenshot and Stop and starts
new ones on the selected display, and capture retention (keep the last 5, 20, 50 or all, open or
delete the folder).

## Preview windows

A preview is a small always-on-top picture-in-picture window for one virtual screen. Frames come
from ScreenCaptureKit at 30fps, downscaled to the window, so a preview costs far less than the old
full-resolution polling capture and stops entirely while paused or occluded.

- Up to four previews, one per virtual screen, at the same time. Each is toggled independently from
  the screen's menu entry, the Virtual Displays page or `lumen virtual preview`.
- Borderless and rounded, with a hover bar holding the screen name, resolution, pause/resume and
  close. Drag anywhere to move it; resize from the edges with the aspect ratio locked.
- The first preview opens in the bottom-right corner of the screen under the pointer, and later
  previews step up and left instead of covering each other. Size and position are remembered per
  screen in `config.json` and restored next time. Previews do not reopen themselves at launch.
- Pausing, occlusion or a hidden app stops the stream. If no frame arrives for three seconds the
  window says so instead of freezing silently.
- Removing or renaming a screen closes its preview. Settings thumbnails reuse a running preview's
  latest frame, or take a still with ScreenCaptureKit otherwise.

Screen Recording permission is required, as it is for screenshots. Without it the preview opens
with a note instead of a picture.

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

- Release needs the monitor's OSD **Auto Input Switch** on. With it off, the monitor stays on the
  input it was showing when the Mac's signal stopped and the screen goes dark. DDC input select is
  the path in that mode, though some firmware overrides DDC while Auto Input Switch is on.
- A monitor that keeps its old link alive while showing another input cannot confirm an input
  switch to macOS. Lumen reports the command as sent and the Settings Test asks what you see; the
  answer is what marks the input verified.
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
