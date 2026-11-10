#!/usr/bin/env bash
# Regenerates assets/lumen_icon_1024.png and assets/Lumen.icns from assets/lumen_icon.svg.
#
# The icon is rendered full-bleed at 1024 and flattened onto an opaque near-black
# background on purpose. When a legacy .icns contains transparency, macOS 26 and later
# draws it on a grey plate and scales the artwork down to about 85%; an opaque image is
# used as-is and masked to the standard squircle at full size.
#
# Needs rsvg-convert, ImageMagick (magick), sips and iconutil.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
svg="$root/assets/lumen_icon.svg"
png="$root/assets/lumen_icon_1024.png"
icns="$root/assets/Lumen.icns"
work="$root/.build/icon"
edge="#08070a"   # mean colour of the artwork's outer ring, so the flattening is invisible

for tool in rsvg-convert magick sips iconutil; do
  command -v "$tool" >/dev/null || { echo "missing $tool" >&2; exit 1; }
done

mkdir -p "$work"
rsvg-convert -w 1024 -h 1024 "$svg" -o "$work/raw.png"
magick "$work/raw.png" -background "$edge" -alpha remove -alpha off "$png"

rm -rf "$work/Lumen.iconset"
mkdir -p "$work/Lumen.iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$png" --out "$work/Lumen.iconset/icon_${size}x${size}.png" >/dev/null
  sips -z "$((size * 2))" "$((size * 2))" "$png" --out "$work/Lumen.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$work/Lumen.iconset" -o "$icns"

echo "wrote $png and $icns"
