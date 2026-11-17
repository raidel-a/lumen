#!/usr/bin/env bash
# Builds a debug Lumen bundle for development. It uses its own bundle identifier and is launched
# with its own support directory by run.sh, so it never touches the installed app's data.
#
# The dev bundle embeds the Sparkle self-update framework (built with --features sparkle) so the
# update path can be exercised end to end against a test appcast. Point it at a local feed with
# LUMEN_FEED_URL (e.g. file:// or http://localhost); it defaults to the same raw.githubusercontent
# feed as the release bundle. The EdDSA public key is read from the login keychain via generate_keys.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
app="$root/dist/Lumen Dev.app"
minimum="27.0"
# Prefer the Developer ID Application identity so the dev bundle exercises the same hardened-runtime
# signing as the release bundle; fall back to the first identity, then ad-hoc.
find_identity() {
  security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(.*\)".*/\1/p' | grep -m1 "^Developer ID Application:" \
  || security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(.*\)".*/\1/p' | head -1
}
identity="${LUMEN_SIGN_IDENTITY:-$(find_identity)}"

sparkle_owner="raidel-a"
sparkle_repo="lumen"
sparkle_branch="main"
feed_url="${LUMEN_FEED_URL:-https://raw.githubusercontent.com/${sparkle_owner}/${sparkle_repo}/${sparkle_branch}/appcast.xml}"

vendor_sparkle="$root/vendor/Sparkle"
sparkle_framework="$vendor_sparkle/Sparkle.framework"
if [[ ! -e "$sparkle_framework/Versions/Current/Sparkle" ]]; then
  echo "error: vendored Sparkle framework missing at $sparkle_framework" >&2
  echo "       run scripts/dev/fetch-sparkle.sh first" >&2
  exit 1
fi

# The dev bundle only needs the EdDSA *public* key (it goes in the Info.plist); the private key
# signs releases and never leaves the host. Prefer an explicit LUMEN_SPARKLE_PUBKEY so a machine
# without the signing key (a development VM) can build dev bundles without touching a keychain.
# Otherwise read the public key from the login keychain, generating one only as a last resort.
if [[ -n "${LUMEN_SPARKLE_PUBKEY:-}" ]]; then
  public_ed_key="$LUMEN_SPARKLE_PUBKEY"
elif ! public_ed_key="$("$vendor_sparkle/bin/generate_keys" -p 2>/dev/null)" || [[ -z "$public_ed_key" ]]; then
  echo "no Sparkle signing key found; generating one in the login keychain" >&2
  echo "(set LUMEN_SPARKLE_PUBKEY to the public key to avoid keychain access, e.g. in a dev VM)" >&2
  "$vendor_sparkle/bin/generate_keys" >/dev/null
  public_ed_key="$("$vendor_sparkle/bin/generate_keys" -p)"
fi

cargo build --features sparkle --bin lumen --manifest-path "$root/Cargo.toml" >&2

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks" "$root/.build/dev-icon/catalog" "$root/.build/dev-icon/out"
cp "$root/target/debug/lumen" "$app/Contents/MacOS/lumen"

cp -R "$root/assets/lumen.icon" "$root/.build/dev-icon/AppIcon.icon"
# actool ships with full Xcode, not the Command Line Tools. In a CLT-only environment (e.g. a
# development VM) compile the layered icon when actool is present and otherwise skip it: a dev
# bundle without the compiled Assets.car still builds, signs and runs. Release bundles are built
# on a host with full Xcode, so they always get the proper icon.
if xcrun --find actool >/dev/null 2>&1; then
  xcrun actool "$root/.build/dev-icon/catalog" "$root/.build/dev-icon/AppIcon.icon" \
    --compile "$root/.build/dev-icon/out" \
    --platform macosx \
    --minimum-deployment-target "$minimum" \
    --app-icon AppIcon \
    --output-partial-info-plist "$root/.build/dev-icon/partial.plist" >/dev/null
  cp "$root/.build/dev-icon/out/AppIcon.icns" "$root/.build/dev-icon/out/Assets.car" "$app/Contents/Resources/"
else
  echo "actool not found (Command Line Tools only); dev bundle built without a compiled icon" >&2
fi

# Embed Sparkle.framework with ditto so the Versions/Current symlink survives.
ditto "$sparkle_framework" "$app/Contents/Frameworks/Sparkle.framework"

# The dev marketing version is overridable so the end-to-end update test can build N and N+1.
dev_version="${LUMEN_DEV_VERSION:-0.0.0}"
dev_build="${LUMEN_DEV_BUILD:-dev}"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Lumen Dev</string>
  <key>CFBundleDisplayName</key><string>Lumen Dev</string>
  <key>CFBundleIdentifier</key><string>pro.lumen.Lumen.dev</string>
  <key>CFBundleExecutable</key><string>lumen</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${dev_version}</string>
  <key>CFBundleVersion</key><string>${dev_build}</string>
  <key>LSMinimumSystemVersion</key><string>${minimum}</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>SUFeedURL</key><string>${feed_url}</string>
  <key>SUPublicEDKey</key><string>${public_ed_key}</string>
</dict>
</plist>
PLIST

sign_sparkle() {
  local sign_id="$1"
  shift
  local fw="$app/Contents/Frameworks/Sparkle.framework"
  local inner=(
    "$fw/Versions/Current/XPCServices/Downloader.xpc"
    "$fw/Versions/Current/XPCServices/Installer.xpc"
    "$fw/Versions/Current/Updater.app"
    "$fw/Versions/Current/Autoupdate"
    "$fw"
  )
  local item
  for item in "${inner[@]}"; do
    [[ -e "$item" ]] && codesign --force --sign "$sign_id" "$@" "$item" >/dev/null
  done
}

if [[ -n "$identity" ]]; then
  sign_sparkle "$identity" --options runtime --timestamp
  codesign --force --sign "$identity" --options runtime --timestamp "$app" >/dev/null
else
  sign_sparkle -
  codesign --force --sign - "$app" >/dev/null
fi
codesign --verify --strict "$app" >&2
echo "$app"
