#!/usr/bin/env bash
# Builds a signed Lumen.app and packages it as a zip and a DMG with an Applications link, then
# updates the Sparkle appcast (./appcast.xml) with the new zip as the update enclosure.
#
# Notarisation is deliberately not here yet: it needs a Developer ID Application certificate and
# hardened runtime. When that exists, add
#   codesign --force --options runtime --timestamp --sign "<Developer ID>" "$app"
#   xcrun notarytool submit "$dmg" --keychain-profile <profile> --wait
#   xcrun stapler staple "$dmg"
#
# Sparkle update flow (zip is the enclosure, DMG stays the human/website download):
#   - generate_appcast signs the zip with the private EdDSA key from the login keychain and writes
#     appcast.xml at the repo root. The key is never printed; only the signature lands in the feed.
#   - enclosure URLs are templated to the GitHub Releases download path for the tag, so the feed is
#     correct before the artefacts are uploaded. Uploading to GitHub is NOT automated here.
# Environment: LUMEN_RELEASE_TAG (default v$version), LUMEN_RELEASE_OWNER/REPO for the download URL.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="${LUMEN_VERSION:-0.1.0}"
build="${LUMEN_BUILD:-$(git -C "$root" rev-list --count HEAD 2>/dev/null || date +%Y%m%d%H%M)}"

release_owner="${LUMEN_RELEASE_OWNER:-raidel-a}"
release_repo="${LUMEN_RELEASE_REPO:-lumen}"
release_tag="${LUMEN_RELEASE_TAG:-v$version}"
# generate_appcast needs the prefix to end with '/'; it appends the archive filename.
download_url_prefix="https://github.com/${release_owner}/${release_repo}/releases/download/${release_tag}/"

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

# --- Sparkle appcast ----------------------------------------------------------------------------
# Run generate_appcast over a directory holding only the update zip so the DMG and the two .app
# bundles in dist/ are not treated as update archives. It signs the zip with the private EdDSA key
# from the login keychain and templates the enclosure URL to the GitHub Releases download path.
vendor_sparkle="$root/vendor/Sparkle"
if [[ ! -x "$vendor_sparkle/bin/generate_appcast" ]]; then
  echo "error: generate_appcast missing; run scripts/dev/fetch-sparkle.sh first" >&2
  exit 1
fi

appcast_src="$root/.build/appcast-src"
rm -rf "$appcast_src"
mkdir -p "$appcast_src"
cp "$zip" "$appcast_src/"
# Carry an existing feed forward so older items are preserved and re-signed as needed.
[[ -f "$root/appcast.xml" ]] && cp "$root/appcast.xml" "$appcast_src/appcast.xml"

"$vendor_sparkle/bin/generate_appcast" \
  --download-url-prefix "$download_url_prefix" \
  --link "https://github.com/${release_owner}/${release_repo}" \
  "$appcast_src"

cp "$appcast_src/appcast.xml" "$root/appcast.xml"
echo
echo "appcast: $root/appcast.xml"
echo "  enclosure base: $download_url_prefix"
echo "  (upload $zip to the '$release_tag' GitHub release; this script does not upload)"
