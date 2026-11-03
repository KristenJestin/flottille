#!/usr/bin/env bash
# Renders the icon's PNGs (48, 128 and 256 px) from packaging/linux/icons/hicolor/scalable/apps/
# flottille.svg. They are committed, not built by CI: the archive then needs no renderer, and
# the same files give the same archive. Run it, and commit the result, when the SVG changes.
#   tools/icons.sh            (needs rsvg-convert, from librsvg)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
hicolor="$root/packaging/linux/icons/hicolor"
command -v rsvg-convert >/dev/null || { echo "icons.sh: rsvg-convert is needed" >&2; exit 1; }
for size in 48 128 256; do
  mkdir -p "$hicolor/${size}x${size}/apps"
  rsvg-convert --width "$size" --height "$size" --output "$hicolor/${size}x${size}/apps/flottille.png" \
    "$hicolor/scalable/apps/flottille.svg"
done
