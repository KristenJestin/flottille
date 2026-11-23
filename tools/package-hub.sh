#!/usr/bin/env bash
# The hub's archive of a build (docs/releases.md): `flottille-hub-<version>-linux-<arch>.tar.gz`,
# holding its static binary, Flottille's licence and the examples that run it (compose, systemd) in
# one folder, and its SHA-256 sum beside it, as `sha256sum --check` reads it. The name never ends
# `-x86_64-linux.tar.gz`: `install.sh` and `flottille update` take that for Flottille's own archive.
#   tools/package-hub.sh <version> <x86_64|aarch64> <binary> <out folder>
set -euo pipefail

version=${1:?package-hub.sh: the version}
arch=${2:?package-hub.sh: the architecture, x86_64 or aarch64}
binary=${3:?package-hub.sh: the binary}
out=${4:?package-hub.sh: the folder to write to}
case "$arch" in
  x86_64 | aarch64) ;;
  *)
    echo "package-hub.sh: $arch is not built (x86_64 or aarch64)" >&2
    exit 2
    ;;
esac
root=$(cd "$(dirname "$0")/.." && pwd)
name="flottille-hub-$version"
archive="$name-linux-$arch.tar.gz"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/$name"
install -m 0755 "$binary" "$work/$name/flottille-hub"
install -m 0644 "$root/LICENSE" "$work/$name/LICENSE"
install -m 0644 "$root/crates/flottille-hub/compose.example.yml" "$work/$name/compose.example.yml"
install -m 0644 "$root/crates/flottille-hub/flottille-hub.service" "$work/$name/flottille-hub.service"
mkdir -p "$out"
# The same archive from the same files: no owner, no time of the build.
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 -C "$work" -cf - "$name" |
  gzip -9 -n >"$out/$archive"
(cd "$out" && sha256sum "$archive" >"$archive.sha256")
echo "$out/$archive"
