#!/usr/bin/env bash
# Builds a signed Lumen.app and packages it as a zip and a DMG with an Applications link.
#
# Notarisation is deliberately not here yet: it needs a Developer ID Application certificate and
# hardened runtime. When that exists, add
#   codesign --force --options runtime --timestamp --sign "<Developer ID>" "$app"
#   xcrun notarytool submit "$dmg" --keychain-profile <profile> --wait
#   xcrun stapler staple "$dmg"
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="${LUMEN_VERSION:-0.1.0}"
build="${LUMEN_BUILD:-$(git -C "$root" rev-list --count HEAD 2>/dev/null || date +%Y%m%d%H%M)}"

LUMEN_VERSION="$version" LUMEN_BUILD="$build" "$root/scripts/bundle.sh"

app="$root/dist/Lumen.app"
zip="$root/dist/Lumen-$version.zip"
dmg="$root/dist/Lumen-$version.dmg"
stage="$root/.build/dmg"

rm -f "$zip" "$dmg"
rm -rf "$stage"
mkdir -p "$stage"
cp -R "$app" "$stage/Lumen.app"
ln -s /Applications "$stage/Applications"

ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
diskutil image create from --format UDZO --volumeName "Lumen $version" "$stage" "$dmg" >/dev/null

# Sign the disk image with the same identity, when one is available.
identity="${LUMEN_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(.*\)".*/\1/p' | head -1)}"
if [[ -n "$identity" ]]; then
  codesign --force --sign "$identity" --timestamp=none "$dmg"
  codesign --verify --strict "$dmg"
fi

echo
echo "artefacts:"
echo "  $zip  $(shasum -a 256 "$zip" | awk '{print $1}')"
echo "  $dmg  $(shasum -a 256 "$dmg" | awk '{print $1}')"
