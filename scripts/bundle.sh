#!/usr/bin/env bash
# Builds Lumen.app, embeds the icon and version, and signs it with a stable identity.
#
# Environment:
#   LUMEN_VERSION        marketing version (default 0.1.0)
#   LUMEN_BUILD          build number (default: git commit count)
#   LUMEN_SIGN_IDENTITY  codesign identity (default: first code-signing identity)
#
# A stable identity matters: TCC grants (Screen Recording) and the SMAppService login item are
# keyed to the code signature. Ad-hoc signing changes the cdhash on every build, which makes macOS
# treat each build as a different app.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="$root/dist/Lumen.app"
version="${LUMEN_VERSION:-0.1.0}"
build="${LUMEN_BUILD:-$(git -C "$root" rev-list --count HEAD 2>/dev/null || date +%Y%m%d%H%M)}"
identity="${LUMEN_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(.*\)".*/\1/p' | head -1)}"
minimum="27.0"
icon_build="$root/.build/icon-build"

cargo build --release --bin lumen --manifest-path "$root/Cargo.toml"

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
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/lumen" "$app/Contents/MacOS/lumen"
cp "$icon_build/out/AppIcon.icns" "$icon_build/out/Assets.car" "$app/Contents/Resources/"

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
</dict>
</plist>
PLIST

if [[ -n "$identity" ]]; then
  codesign --force --sign "$identity" --timestamp=none "$app"
  signer="$identity"
else
  echo "warning: no code-signing identity found, signing ad-hoc; TCC grants will reset on rebuild" >&2
  codesign --force --sign - "$app"
  signer="ad-hoc"
fi

codesign --verify --strict --verbose=2 "$app"
echo "built $app ($version build $build, signed by $signer)"
