#!/bin/sh
# Installs Flottille from a release, into ~/.local/bin, without root (docs/releases.md):
#   curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/main/install.sh | sh
#   … | sh -s -- --channel beta
#   … | sh -s -- --channel alpha --branch feat/46-packages
# The archive's SHA-256 sum is checked before anything is installed; `flot` is a link to
# `flottille`. `FLOTTILLE_RELEASES` names another place of the releases' API (for tests).
# Everything is in `main`, called on the last line: a download cut short runs nothing.
set -eu

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

# The API's answer at $1 in the file $2: 0 when it is there, 1 when it is not (404), and a
# failure said as one otherwise, the network's or GitHub's.
read_api() {
  code=$(curl -sSL -o "$2" -w '%{http_code}' -- "$1") || fail "cannot read $1 (is the network up?)"
  case "$code" in
    200) return 0 ;;
    404) return 1 ;;
    *) fail "cannot read $1: GitHub answered $code" ;;
  esac
}

# The archives' addresses in an answer of the API, one per line.
archives() {
  tr ',' '\n' <"$1" | sed -n 's/.*"browser_download_url": *"\([^"]*-x86_64-linux\.tar\.gz\)".*/\1/p'
}

# The archive of the highest version among the addresses read, whatever order they come in.
highest() {
  sed 's|.*/flottille-\([^/]*\)-x86_64-linux\.tar\.gz$|\1 &|' | sort -t ' ' -k 1,1V | tail -n 1 | cut -d ' ' -f 2
}

# Whether Flottille's daemon runs: it holds the lock beside its socket. Asked without starting
# one, and without speaking to it.
daemon_runs() {
  [ -n "${XDG_RUNTIME_DIR:-}" ] || return 1
  [ -e "$XDG_RUNTIME_DIR/flottille/daemon.lock" ] || return 1
  if command -v flock >/dev/null 2>&1; then
    ! flock -n "$XDG_RUNTIME_DIR/flottille/daemon.lock" true
  else
    [ -S "$XDG_RUNTIME_DIR/flottille/daemon.sock" ]
  fi
}

main() {
  api=${FLOTTILLE_RELEASES:-https://api.github.com/repos/KristenJestin/flottille}
  channel=stable
  branch=
  while [ $# -gt 0 ]; do
    case "$1" in
      --channel | --branch)
        if [ $# -lt 2 ] || [ -z "$2" ]; then
          echo "install.sh: $1 needs a value" >&2
          exit 2
        fi
        if [ "$1" = --channel ]; then channel=$2; else branch=$2; fi
        shift 2
        ;;
      *)
        echo "install.sh: unknown argument $1" >&2
        exit 2
        ;;
    esac
  done

  for program in curl tar sha256sum sort uname; do
    command -v "$program" >/dev/null 2>&1 || fail "$program is needed"
  done
  [ "$(uname -s)-$(uname -m)" = Linux-x86_64 ] || fail "there are packages for Linux x86_64 only"
  # It would go on from the file renamed over, and fail its next start of an agent.
  if daemon_runs; then
    fail "Flottille's daemon is running: stop it first with \`flottille daemon stop\` (its agents stop with it), then install."
  fi

  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  answer="$work/answer.json"
  case "$channel" in
    stable)
      read_api "$api/releases/latest" "$answer" || fail "there is no stable release yet"
      url=$(archives "$answer" | head -n 1)
      ;;
    beta)
      read_api "$api/releases?per_page=100" "$answer" || fail "cannot read the releases at $api"
      url=$(archives "$answer" | grep -- '/download/v[0-9.]*-beta\.[0-9]*/' | highest)
      ;;
    alpha)
      [ -n "$branch" ] || fail "the alpha channel needs --branch <name>"
      # Each byte but ASCII letters, digits and dashes is a dash, as CI names the alpha.
      tag="alpha-$(printf '%s' "$branch" | LC_ALL=C tr -c 'A-Za-z0-9-' '-')"
      read_api "$api/releases/tags/$tag" "$answer" || fail "there is no alpha release for $branch"
      # Two while it is replaced: the newer.
      url=$(archives "$answer" | highest)
      ;;
    *) fail "the channel is stable, beta or alpha" ;;
  esac
  [ -n "$url" ] || fail "there is no $channel release for Linux x86_64"
  name=${url##*/}

  curl -fsSL -o "$work/$name" -- "$url" || fail "cannot download $url"
  curl -fsSL -o "$work/$name.sha256" -- "$url.sha256" || fail "cannot download $url.sha256"
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
  echo "Installed ${folder#flottille-} in $bin."
  # Installed is installed: a binary that cannot run here (a system library missing) is said,
  # not undone.
  if said=$("$bin/flottille" --version 2>&1); then
    echo "$said"
  else
    echo "install.sh: $bin/flottille --version did not answer: $said" >&2
  fi
  case ":$PATH:" in *":$bin:"*) ;; *) echo "Add $bin to your PATH to run it." ;; esac
}

main "$@"
