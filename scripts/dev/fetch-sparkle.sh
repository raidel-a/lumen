#!/usr/bin/env bash
# Vendors the Sparkle framework and its command-line tools into vendor/Sparkle.
#
# The release tarball is pinned by version and verified by SHA-256 before anything is unpacked;
# a checksum mismatch is a hard failure. The framework is copied with symlinks preserved (ditto),
# because Sparkle.framework relies on the Versions/Current symlink and codesign rejects a framework
# whose symlinks have been flattened into real files.
#
# vendor/Sparkle is gitignored (the framework is ~16 MB); run this script once before bundle.sh or
# release.sh on a fresh checkout. Re-running is idempotent: an already-correct vendor tree is left
# in place.
set -euo pipefail

# --- pinned release -----------------------------------------------------------------------------
sparkle_version="2.10.1"
sparkle_sha256="1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27"
tarball_url="https://github.com/sparkle-project/Sparkle/releases/download/${sparkle_version}/Sparkle-${sparkle_version}.tar.xz"
# ------------------------------------------------------------------------------------------------

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
vendor="$root/vendor/Sparkle"
framework="$vendor/Sparkle.framework"
stamp="$vendor/.sparkle-version"

if [[ -f "$stamp" && "$(cat "$stamp" 2>/dev/null)" == "$sparkle_version" \
      && -e "$framework/Versions/Current/Sparkle" \
      && -x "$vendor/bin/generate_keys" ]]; then
  echo "sparkle $sparkle_version already vendored at $vendor"
  exit 0
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/lumen-sparkle.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

tarball="$tmp/Sparkle-${sparkle_version}.tar.xz"
echo "fetching $tarball_url"
curl -fsSL -o "$tarball" "$tarball_url"

echo "verifying checksum"
actual="$(shasum -a 256 "$tarball" | awk '{print $1}')"
if [[ "$actual" != "$sparkle_sha256" ]]; then
  echo "error: checksum mismatch for Sparkle-${sparkle_version}.tar.xz" >&2
  echo "  expected $sparkle_sha256" >&2
  echo "  actual   $actual" >&2
  exit 1
fi

echo "unpacking"
mkdir -p "$tmp/extract"
tar -xJf "$tarball" -C "$tmp/extract"

if [[ ! -e "$tmp/extract/Sparkle.framework/Versions/Current/Sparkle" ]]; then
  echo "error: Sparkle.framework not found in tarball as expected" >&2
  exit 1
fi

# Rebuild the vendor tree from scratch so a partial previous run cannot leave stale files.
rm -rf "$vendor"
mkdir -p "$vendor/bin"

# ditto preserves the Versions/Current symlink the framework and codesign depend on.
ditto "$tmp/extract/Sparkle.framework" "$framework"

for tool in generate_keys sign_update generate_appcast BinaryDelta; do
  if [[ -f "$tmp/extract/bin/$tool" ]]; then
    cp "$tmp/extract/bin/$tool" "$vendor/bin/$tool"
    chmod +x "$vendor/bin/$tool"
  fi
done

cp "$tmp/extract/LICENSE" "$vendor/LICENSE"
printf 'Sparkle %s\nhttps://github.com/sparkle-project/Sparkle\nSHA-256 (Sparkle-%s.tar.xz): %s\nLicense: MIT (see LICENSE)\n' \
  "$sparkle_version" "$sparkle_version" "$sparkle_sha256" > "$vendor/VENDOR.md"

echo "$sparkle_version" > "$stamp"

echo "vendored sparkle $sparkle_version into $vendor"
otool -L "$framework/Versions/Current/Sparkle" | head -2
