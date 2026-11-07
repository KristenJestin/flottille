#!/usr/bin/env bash
# Renders the PNGs of assets/brand from its SVGs, with the fonts of assets/brand/fonts only: a
# fontconfig made for the run, so no font of the machine changes the drawing. Needs rsvg-convert
# (librsvg). The PNGs are committed; run this again after changing an SVG.
set -euo pipefail

brand="$(cd "$(dirname "$0")/../assets/brand" && pwd)"
run="$(mktemp -d)"
trap 'rm -rf "$run"' EXIT

cat >"$run/fonts.conf" <<CONF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig><dir>$brand/fonts</dir><cachedir>$run/cache</cachedir></fontconfig>
CONF
export FONTCONFIG_FILE="$run/fonts.conf"

mkdir -p "$brand/png"
for size in 128 256 512 1024; do
  rsvg-convert -w "$size" -h "$size" "$brand/icon.svg" -o "$brand/png/icon-$size.png"
done
rsvg-convert -w 512 -h 512 "$brand/icon-full.svg" -o "$brand/png/icon-full-512.png"
for size in 16 32 64; do
  rsvg-convert -w "$size" -h "$size" "$brand/favicon.svg" -o "$brand/png/favicon-$size.png"
done
rsvg-convert -w 512 -h 512 "$brand/mark.svg" -o "$brand/png/mark-512.png"
rsvg-convert -w 512 -h 512 "$brand/mark-on-light.svg" -o "$brand/png/mark-on-light-512.png"
for name in screen social-preview social-preview-fr; do
  rsvg-convert "$brand/$name.svg" -o "$brand/png/$name.png"
done
