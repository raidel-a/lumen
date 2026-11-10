#!/usr/bin/env bash
# Builds a debug Lumen bundle for development. It uses its own bundle identifier and is launched
# with its own support directory by run.sh, so it never touches the installed app's data.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
app="$root/dist/Lumen Dev.app"
minimum="27.0"
identity="${LUMEN_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(.*\)".*/\1/p' | head -1)}"

cargo build --bin lumen --manifest-path "$root/Cargo.toml"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$root/.build/dev-icon/catalog" "$root/.build/dev-icon/out"
cp "$root/target/debug/lumen" "$app/Contents/MacOS/lumen"

cp -R "$root/assets/lumen.icon" "$root/.build/dev-icon/AppIcon.icon"
xcrun actool "$root/.build/dev-icon/catalog" "$root/.build/dev-icon/AppIcon.icon" \
  --compile "$root/.build/dev-icon/out" \
  --platform macosx \
  --minimum-deployment-target "$minimum" \
  --app-icon AppIcon \
  --output-partial-info-plist "$root/.build/dev-icon/partial.plist" >/dev/null
cp "$root/.build/dev-icon/out/AppIcon.icns" "$root/.build/dev-icon/out/Assets.car" "$app/Contents/Resources/"

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
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>CFBundleVersion</key><string>dev</string>
  <key>LSMinimumSystemVersion</key><string>${minimum}</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

if [[ -n "$identity" ]]; then
  codesign --force --sign "$identity" --timestamp=none "$app" >/dev/null
else
  codesign --force --sign - "$app" >/dev/null
fi
codesign --verify --strict "$app"
echo "$app"
