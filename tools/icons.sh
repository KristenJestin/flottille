#!/usr/bin/env bash
# Writes the packaged icon: packaging/linux/icons/hicolor/scalable/apps/flottille.svg is
# assets/brand/icon-full.svg (the brand's one source of the drawing), and the PNGs (48, 128 and
# 256 px) are renders of it. They are committed, not built by CI: the archive then needs no
# renderer, and the same files give the same archive. Run it, and commit the result, when the
# brand's icon changes.
#   tools/icons.sh            (needs rsvg-convert, from librsvg)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
hicolor="$root/packaging/linux/icons/hicolor"
cp "$root/assets/brand/icon-full.svg" "$hicolor/scalable/apps/flottille.svg"
command -v rsvg-convert >/dev/null || { echo "icons.sh: rsvg-convert is needed" >&2; exit 1; }
for size in 48 128 256; do
  mkdir -p "$hicolor/${size}x${size}/apps"
  rsvg-convert --width "$size" --height "$size" --output "$hicolor/${size}x${size}/apps/flottille.png" \
    "$hicolor/scalable/apps/flottille.svg"
done
