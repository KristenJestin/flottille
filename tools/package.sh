#!/usr/bin/env bash
# The archive of a build (docs/releases.md): `flottille-<version>-x86_64-linux.tar.gz`, holding
# the binary, Flottille's licence and the fonts' licence in one folder, and its SHA-256 sum beside
# it, as `sha256sum --check` reads it.
#   tools/package.sh <version> <binary> <out folder>
set -euo pipefail

version=${1:?package.sh: the version}
binary=${2:?package.sh: the binary}
out=${3:?package.sh: the folder to write to}
root=$(cd "$(dirname "$0")/.." && pwd)
name="flottille-$version"
archive="$name-x86_64-linux.tar.gz"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/$name"
install -m 0755 "$binary" "$work/$name/flottille"
install -m 0644 "$root/LICENSE" "$work/$name/LICENSE"
install -m 0644 "$root/crates/flottille-ui/assets/fonts/OFL.txt" "$work/$name/LICENSE-fonts.txt"
mkdir -p "$out"
# The same archive from the same files: no owner, no time of the build.
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 -C "$work" -cf - "$name" |
  gzip -9 -n >"$out/$archive"
(cd "$out" && sha256sum "$archive" >"$archive.sha256")
echo "$out/$archive"
