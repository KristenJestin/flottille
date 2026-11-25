#!/usr/bin/env bash
# Renders the app's legacy launcher icons (`app/src/main/res/mipmap-*dpi/ic_launcher*.png`) from
# assets/brand/icon.svg, the one source of the drawing, with rsvg-convert (librsvg), the tool
# tools/brand.sh uses. The round one is the same icon on a circle. The adaptive icon, the themed
# layer and the notification's mark are vector drawables, written by hand from assets/brand/mark.svg.
# The PNGs are committed; run this again after the brand's icon changes.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
brand="$here/../../assets/brand"
res="$here/app/src/main/res"
run="$(mktemp -d)"
trap 'rm -rf "$run"' EXIT

sed 's|<rect width="256" height="256" rx="64.0" fill="\(#[0-9a-f]*\)"/>|<circle cx="128" cy="128" r="128" fill="\1"/>|' \
  "$brand/icon.svg" >"$run/icon-round.svg"
grep -q '<circle' "$run/icon-round.svg" || { echo "icon.svg's ground is no longer a rect: fix icons.sh" >&2; exit 1; }

for density in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:144 xxxhdpi:192; do
  folder="$res/mipmap-${density%%:*}"
  size="${density##*:}"
  mkdir -p "$folder"
  rsvg-convert -w "$size" -h "$size" "$brand/icon.svg" -o "$folder/ic_launcher.png"
  rsvg-convert -w "$size" -h "$size" "$run/icon-round.svg" -o "$folder/ic_launcher_round.png"
done
