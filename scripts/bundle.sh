#!/usr/bin/env bash
# Builds Lumen.app, embeds the icon, version, and the Sparkle self-update framework, and signs it
# with a stable identity.
#
# Environment:
#   LUMEN_VERSION        marketing version (default: last git tag, with -dev.N when ahead)
#   LUMEN_BUILD          build number (default: git commit count)
#   LUMEN_SIGN_IDENTITY  codesign identity (default: Developer ID Application, else first identity)
#   LUMEN_FEED_URL       Sparkle appcast URL (default: the raw.githubusercontent.com URL below)
#
# A stable identity matters: TCC grants (Screen Recording) and the SMAppService login item are
# keyed to the code signature. Ad-hoc signing changes the cdhash on every build, which makes macOS
# treat each build as a different app.
#
# Signing: with a real identity the app is signed with the hardened runtime (--options runtime) and
# a secure timestamp (--timestamp), which notarisation requires. The nested Sparkle code is signed
# first (inner-to-outer) with the same identity and runtime. Sparkle's vendored framework ships
# ad-hoc, so re-signing it with our Developer ID gives it our Team ID; library validation is then
# satisfied because the app and the framework share a team, and no disable-library-validation
# entitlement is needed. Ad-hoc fallback stays runtime-free so pure-local dev builds still load.
#
# Sparkle: the vendored framework (vendor/Sparkle) must be present. Run scripts/dev/fetch-sparkle.sh
# once on a fresh checkout. The EdDSA signing key lives in the login keychain (created on demand by
# Sparkle's generate_keys); only its public half is embedded here as SUPublicEDKey. The private key
# is never printed or written to disk.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/version.sh
source "$root/scripts/version.sh"
app="$root/dist/Lumen.app"
version="${LUMEN_VERSION:-$(lumen_local_version "$root")}"
build="${LUMEN_BUILD:-$(git -C "$root" rev-list --count HEAD 2>/dev/null || date +%Y%m%d%H%M)}"

# Prefer the Developer ID Application identity (required for notarised, hardened-runtime builds).
# Fall back to the first code-signing identity, and finally to ad-hoc, for pure local dev.
find_identity() {
  security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(.*\)".*/\1/p' | grep -m1 "^Developer ID Application:" \
  || security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(.*\)".*/\1/p' | head -1
}
identity="${LUMEN_SIGN_IDENTITY:-$(find_identity)}"
minimum="27.0"
icon_build="$root/.build/icon-build"

# --- Sparkle self-update: feed URL and vendored framework ---------------------------------------
# Hosting is GitHub Releases with the appcast served from raw.githubusercontent.com. Override the
# feed with LUMEN_FEED_URL (e.g. to point a local test build at a file:// or localhost appcast).
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

# The EdDSA public key goes in the plist; the private key stays in the login keychain. Create the
# key pair on first run (generate_keys never overrides an existing key and never prints the private
# half). -p prints only the public key for automation.
if ! public_ed_key="$("$vendor_sparkle/bin/generate_keys" -p 2>/dev/null)" || [[ -z "$public_ed_key" ]]; then
  echo "no Sparkle signing key found; generating one in the login keychain"
  "$vendor_sparkle/bin/generate_keys" >/dev/null
  public_ed_key="$("$vendor_sparkle/bin/generate_keys" -p)"
fi
if [[ -z "$public_ed_key" ]]; then
  echo "error: could not obtain the Sparkle EdDSA public key" >&2
  exit 1
fi
# ------------------------------------------------------------------------------------------------

cargo build --release --features sparkle --bin lumen --manifest-path "$root/Cargo.toml"

# Compile the Icon Composer document into Assets.car (layered, Liquid Glass) plus a fallback icns.
# actool wants the document named after the icon it is asked for.
rm -rf "$icon_build"
mkdir -p "$icon_build/catalog" "$icon_build/out"
cp -R "$root/assets/lumen.icon" "$icon_build/AppIcon.icon"
xcrun actool "$icon_build/catalog" "$icon_build/AppIcon.icon" \
  --compile "$icon_build/out" \
  --platform macosx \
  --minimum-deployment-target "$minimum" \
  --app-icon AppIcon \
  --output-partial-info-plist "$icon_build/partial.plist" >/dev/null

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp "$root/target/release/lumen" "$app/Contents/MacOS/lumen"
cp "$icon_build/out/AppIcon.icns" "$icon_build/out/Assets.car" "$app/Contents/Resources/"

# Embed Sparkle.framework with ditto so the Versions/Current symlink survives; codesign rejects a
# framework whose symlinks have been flattened into real files.
ditto "$sparkle_framework" "$app/Contents/Frameworks/Sparkle.framework"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Lumen</string>
  <key>CFBundleDisplayName</key><string>Lumen</string>
  <key>CFBundleIdentifier</key><string>pro.lumen.Lumen</string>
  <key>CFBundleExecutable</key><string>lumen</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${build}</string>
  <key>LSMinimumSystemVersion</key><string>${minimum}</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>SUFeedURL</key><string>${feed_url}</string>
  <key>SUPublicEDKey</key><string>${public_ed_key}</string>
</dict>
</plist>
PLIST

sign_sparkle() {
  # Sign Sparkle's nested code before the outer app. codesign seals inner code into the enclosing
  # signature, so inner-to-outer order is required for `codesign --verify --strict` to pass.
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
    [[ -e "$item" ]] && codesign --force --sign "$sign_id" "$@" "$item"
  done
}

if [[ -n "$identity" ]]; then
  # Hardened runtime + secure timestamp: both are required for notarisation. Nested-first so the
  # inner Sparkle code is sealed into the outer signature.
  sign_sparkle "$identity" --options runtime --timestamp
  codesign --force --sign "$identity" --options runtime --timestamp "$app"
  signer="$identity"
else
  echo "warning: no code-signing identity found, signing ad-hoc; TCC grants will reset on rebuild" >&2
  # Ad-hoc cannot carry a trusted timestamp and gains nothing from the hardened runtime (it is not
  # notarisable); sign without them so the local build still loads.
  sign_sparkle -
  codesign --force --sign - "$app"
  signer="ad-hoc"
fi

codesign --verify --strict --verbose=2 "$app"
echo "built $app ($version build $build, signed by $signer)"
echo "  feed: $feed_url"
