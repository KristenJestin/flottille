#!/bin/sh
# Installs Flottille from a release, into ~/.local/bin, without root (docs/releases.md):
#   curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/main/install.sh | sh
#   … | sh -s -- --channel beta
#   … | sh -s -- --channel alpha --branch feat/46-packages
# The archive's SHA-256 sum is checked before anything is installed; `flot` is a link to
# `flottille`. `FLOTTILLE_RELEASES` names another place of the releases' API (for tests).
set -eu

api=${FLOTTILLE_RELEASES:-https://api.github.com/repos/KristenJestin/flottille}
channel=stable
branch=
while [ $# -gt 0 ]; do
  case "$1" in
    --channel) channel=${2:-}; shift 2 || shift ;;
    --branch) branch=${2:-}; shift 2 || shift ;;
    *) echo "install.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

fail() { echo "install.sh: $*" >&2; exit 1; }
for program in curl tar sha256sum uname; do
  command -v "$program" >/dev/null 2>&1 || fail "$program is needed"
done
[ "$(uname -s)-$(uname -m)" = Linux-x86_64 ] || fail "there are packages for Linux x86_64 only"

# The archive's address, from the API's answer: one asset address per line.
assets() { curl -fsSL "$1" | tr ',' '\n' | sed -n 's/.*"browser_download_url": *"\([^"]*\)".*/\1/p'; }
case "$channel" in
  stable) url=$(assets "$api/releases/latest" | grep -- '-x86_64-linux\.tar\.gz$' | head -n 1) ;;
  # The releases come newest first: the first beta is the latest.
  beta) url=$(assets "$api/releases?per_page=100" | grep -- '/v[0-9.]*-beta\.[0-9]*/[^/]*-x86_64-linux\.tar\.gz$' | head -n 1) ;;
  alpha)
    [ -n "$branch" ] || fail "the alpha channel needs --branch <name>"
    tag="alpha-$(printf '%s' "$branch" | tr -c 'A-Za-z0-9-' '-')"
    url=$(assets "$api/releases/tags/$tag" 2>/dev/null | grep -- '-x86_64-linux\.tar\.gz$' | head -n 1)
    [ -n "$url" ] || fail "there is no alpha release for $branch"
    ;;
  *) fail "the channel is stable, beta or alpha" ;;
esac
[ -n "$url" ] || fail "there is no $channel release"
name=${url##*/}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL -o "$work/$name" "$url"
curl -fsSL -o "$work/$name.sha256" "$url.sha256"
(cd "$work" && sha256sum --check --status "$name.sha256") ||
  fail "the SHA-256 sum of $name is not the one published: nothing was installed"
tar -xzf "$work/$name" -C "$work"
bin="$HOME/.local/bin"
mkdir -p "$bin"
folder=${name%-x86_64-linux.tar.gz}
# Beside it, then renamed over it: a running flottille keeps its own file.
install -m 0755 "$work/$folder/flottille" "$bin/.flottille.new"
mv -f "$bin/.flottille.new" "$bin/flottille"
ln -sfn flottille "$bin/flot"
echo "Installed $("$bin/flottille" --version) in $bin."
case ":$PATH:" in *":$bin:"*) ;; *) echo "Add $bin to your PATH to run it." ;; esac
