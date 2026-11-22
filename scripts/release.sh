#!/usr/bin/env bash
# Builds a signed, hardened-runtime Lumen.app, notarises and staples it (when notary credentials
# are available), packages it as a zip and a DMG with an Applications link, then updates the
# Sparkle appcast (./appcast.xml) with the stapled zip as the update enclosure.
#
# Notarisation credentials (either form works; the keychain profile is checked first):
#   LUMEN_NOTARY_PROFILE   notarytool keychain profile name (default "lumen-notary"); used locally
#                          after `xcrun notarytool store-credentials`.
#   LUMEN_NOTARY_KEY       path to the App Store Connect API key (.p8); used in CI where there is
#   LUMEN_NOTARY_KEY_ID    the key ID (10-char),                        no keychain profile. All
#   LUMEN_NOTARY_ISSUER    the issuer UUID.                             three must be set together.
# If neither a usable profile nor a complete key triplet is present, the submit/staple steps are
# skipped with a clear message and the rest of the script (build, sign, dmg, appcast) still runs,
# so an un-notarised local build remains possible.
#
# A zip cannot be stapled, so the flow is: sign the .app, submit it for notarisation, staple the
# .app, then (re)build the zip and dmg FROM the stapled app and staple the dmg. The appcast is
# generated last, over the final stapled zip, so edSignature and length match the published bytes.
#
# Sparkle update flow (zip is the enclosure, DMG stays the human/website download):
#   - generate_appcast signs the zip with the private EdDSA key from LUMEN_SPARKLE_KEY_FILE when
#     set (CI), otherwise from the login keychain. The key is never printed; only the signature
#     lands in the feed.
#   - enclosure URLs are templated to the GitHub Releases download path for the tag, so the feed is
#     correct before the artefacts are uploaded. Uploading to GitHub is NOT automated here.
# Environment: LUMEN_RELEASE_TAG (default v$version), LUMEN_RELEASE_OWNER/REPO for the download URL.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -n "${LUMEN_VERSION:-}" ]]; then
  version="$LUMEN_VERSION"
else
  tag="$(git -C "$root" describe --tags --exact-match 2>/dev/null || true)"
  if [[ -z "$tag" ]]; then
    echo "error: set LUMEN_VERSION, or check out the release tag before running release.sh" >&2
    exit 1
  fi
  version="${tag#v}"
fi
build="${LUMEN_BUILD:-$(git -C "$root" rev-list --count HEAD 2>/dev/null || date +%Y%m%d%H%M)}"

release_owner="${LUMEN_RELEASE_OWNER:-raidel-a}"
release_repo="${LUMEN_RELEASE_REPO:-lumen}"
release_tag="${LUMEN_RELEASE_TAG:-v$version}"
# generate_appcast needs the prefix to end with '/'; it appends the archive filename.
download_url_prefix="https://github.com/${release_owner}/${release_repo}/releases/download/${release_tag}/"

notary_profile="${LUMEN_NOTARY_PROFILE:-lumen-notary}"

LUMEN_VERSION="$version" LUMEN_BUILD="$build" "$root/scripts/bundle.sh"

app="$root/dist/Lumen.app"
zip="$root/dist/Lumen-$version.zip"
dmg="$root/dist/Lumen-$version.dmg"
stage="$root/.build/dmg"

identity="${LUMEN_SIGN_IDENTITY:-$(
  security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(.*\)".*/\1/p' | grep -m1 "^Developer ID Application:" \
  || security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(.*\)".*/\1/p' | head -1
)}"

# --- notarisation helpers -----------------------------------------------------------------------
# Decide once how (or whether) we can reach the notary service. notarytool accepts either a stored
# keychain profile or an explicit key/key-id/issuer triplet; build the right argument set here.
notary_args=()
notary_mode="none"
if [[ -n "${LUMEN_NOTARY_KEY:-}" && -n "${LUMEN_NOTARY_KEY_ID:-}" && -n "${LUMEN_NOTARY_ISSUER:-}" ]]; then
  if [[ -f "$LUMEN_NOTARY_KEY" ]]; then
    notary_args=(--key "$LUMEN_NOTARY_KEY" --key-id "$LUMEN_NOTARY_KEY_ID" --issuer "$LUMEN_NOTARY_ISSUER")
    notary_mode="key"
  else
    echo "error: LUMEN_NOTARY_KEY set but file not found: $LUMEN_NOTARY_KEY" >&2
    exit 1
  fi
elif xcrun notarytool history --keychain-profile "$notary_profile" >/dev/null 2>&1; then
  notary_args=(--keychain-profile "$notary_profile")
  notary_mode="profile"
fi

# Submit an archive and wait; fail hard on anything other than Accepted, printing the log.
notarise() {
  local archive="$1"
  local submit_json submit_id submit_status
  echo "notarising $(basename "$archive") (this can take a few minutes)…"
  submit_json="$(xcrun notarytool submit "$archive" "${notary_args[@]}" --wait --output-format json)"
  submit_id="$(printf '%s' "$submit_json" | /usr/bin/plutil -extract id raw - 2>/dev/null || true)"
  submit_status="$(printf '%s' "$submit_json" | /usr/bin/plutil -extract status raw - 2>/dev/null || true)"
  echo "  submission $submit_id: $submit_status"
  if [[ "$submit_status" != "Accepted" ]]; then
    echo "error: notarisation of $(basename "$archive") was not Accepted ($submit_status)" >&2
    if [[ -n "$submit_id" ]]; then
      echo "--- notarytool log $submit_id ---" >&2
      xcrun notarytool log "$submit_id" "${notary_args[@]}" >&2 || true
    fi
    exit 1
  fi
}
# ------------------------------------------------------------------------------------------------

# Build the zip once from the freshly signed app. If we notarise, this zip is the submission
# archive for the app and is then rebuilt from the stapled app; if we do not, it is the final zip.
rm -f "$zip" "$dmg"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"

if [[ "$notary_mode" == "none" ]]; then
  echo
  echo "notary credentials not configured — skipping submit/staple."
  echo "  to enable, either store a keychain profile:"
  echo "    xcrun notarytool store-credentials \"$notary_profile\" \\"
  echo "      --key <AuthKey_XXXX.p8> --key-id <KEY_ID> --issuer <ISSUER_UUID>"
  echo "  or set LUMEN_NOTARY_KEY / LUMEN_NOTARY_KEY_ID / LUMEN_NOTARY_ISSUER (used by CI)."
  echo "  the .app and dmg will be signed but NOT notarised (spctl will say Unnotarized)."
else
  echo
  echo "notary mode: $notary_mode"
  # Notarise the app (via its zip), then staple the .app itself (a zip cannot hold the ticket).
  notarise "$zip"
  echo "stapling $app"
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  # Rebuild the zip from the now-stapled app so the enclosure carries the ticket.
  rm -f "$zip"
  ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
fi

# Build the dmg FROM the (possibly stapled) app so the disk image contains the ticketed bundle.
rm -rf "$stage"
mkdir -p "$stage"
cp -R "$app" "$stage/Lumen.app"
ln -s /Applications "$stage/Applications"
diskutil image create from --format UDZO --volumeName "Lumen $version" "$stage" "$dmg" >/dev/null

# Sign the disk image with the same identity, when one is available.
if [[ -n "$identity" ]]; then
  codesign --force --sign "$identity" --timestamp "$dmg"
  codesign --verify --strict "$dmg"
fi

# Notarise and staple the dmg (the stapled .app inside it is already ticketed; stapling the dmg
# lets Gatekeeper accept it offline on first mount).
if [[ "$notary_mode" != "none" ]]; then
  notarise "$dmg"
  echo "stapling $dmg"
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
fi

echo
echo "artefacts:"
echo "  $zip  $(shasum -a 256 "$zip" | awk '{print $1}')"
echo "  $dmg  $(shasum -a 256 "$dmg" | awk '{print $1}')"
if [[ "$notary_mode" != "none" ]]; then
  echo "  notarised + stapled"
  spctl -a -vvv --type exec "$app" 2>&1 | sed 's/^/  spctl: /' || true
else
  echo "  NOT notarised (local build)"
fi

# --- Sparkle appcast ----------------------------------------------------------------------------
# Run generate_appcast over a directory holding only the update zip so the DMG and the two .app
# bundles in dist/ are not treated as update archives. It signs the zip with the private EdDSA key
# from LUMEN_SPARKLE_KEY_FILE when set (CI), otherwise from the login keychain, and templates the
# enclosure URL to the GitHub Releases download path. This runs LAST, over the final (stapled, when
# notarised) zip, so edSignature and length match the published bytes.
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

if [[ -n "${LUMEN_SPARKLE_KEY_FILE:-}" ]]; then
  "$vendor_sparkle/bin/generate_appcast" \
    --ed-key-file "$LUMEN_SPARKLE_KEY_FILE" \
    --download-url-prefix "$download_url_prefix" \
    --link "https://github.com/${release_owner}/${release_repo}" \
    "$appcast_src"
else
  "$vendor_sparkle/bin/generate_appcast" \
    --download-url-prefix "$download_url_prefix" \
    --link "https://github.com/${release_owner}/${release_repo}" \
    "$appcast_src"
fi

cp "$appcast_src/appcast.xml" "$root/appcast.xml"
echo
echo "appcast: $root/appcast.xml"
echo "  enclosure base: $download_url_prefix"
echo "  (upload $zip to the '$release_tag' GitHub release; this script does not upload)"
