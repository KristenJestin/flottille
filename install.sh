#!/bin/sh
# Installs Flottille from a release, into ~/.local/bin, without root (docs/releases.md):
#   curl -fsSL https://raw.githubusercontent.com/KristenJestin/flottille/main/install.sh | sh
#   … | sh -s -- --channel beta
#   … | sh -s -- --channel alpha --branch feat/46-packages
#   … | sh -s -- --version 0.3.0-beta.12
# The archive's SHA-256 sum is checked before anything is installed; `flot` is a link to
# `flottille`. The launcher's entry and icon go to $XDG_DATA_HOME (docs/releases.md).
# `FLOTTILLE_RELEASES` names another place of the releases' API (for tests).
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

# The published releases' archives in a list of releases of the API, one line each: when its
# release was published, the version and the address. A `tag_name` and `published_at` come before
# a release's `assets` in GitHub's answer, so each address is read with the date of its own
# release: reset at each tag, and empty for a draft (`published_at` null), which is not offered.
# The dates are UTC, all of one shape, so they sort as text.
published() {
  tr ',' '\n' <"$1" | sed -n \
    -e 's/.*"tag_name": *"[^"]*".*/T/p' \
    -e 's/.*"published_at": *"\([^"]*\)".*/P \1/p' \
    -e 's/.*"published_at": *null.*/P/p' \
    -e 's|.*"browser_download_url": *"\([^"]*/flottille-\([^/]*\)-x86_64-linux\.tar\.gz\)".*|U \2 \1|p' |
    awk '$1 == "T" { at = "" } $1 == "P" { at = $2 } $1 == "U" && at != "" { print at, $2, $3 }'
}

# The archive of the release published last in a list of releases of the API, ties going to the
# higher version.
latest_published() {
  published "$1" | awk '$3 ~ /\/download\/v[0-9.]*-beta\.[0-9]*\// { print }' |
    sort -k 1,1 -k 2,2V | tail -n 1 | cut -d ' ' -f 3
}

# The archive of the highest version among the alphas of the branch whose slug is $2, in a list
# of releases: `*-alpha.<slug>.<n>`, the slug whole (`feat-a` is not `feat-a-b`), each alpha being
# a release of its own.
highest_alpha() {
  published "$1" | awk -v slug="$2" '{
      v = $2; sub(/^[0-9]+\.[0-9]+\.[0-9]+-alpha\./, "", v)
      if (v != $2 && v ~ /\.[0-9]+$/) { sub(/\.[0-9]+$/, "", v); if (v == slug) print }
    }' | sort -k 2,2V | tail -n 1 | cut -d ' ' -f 3
}

# The tag a version is published under: `v` and the version, whatever its channel.
tag_of() {
  printf 'v%s' "$1"
}

# The versions of the archives in the answer $1, up to five of those nearest the version $2.
close_versions() {
  archives "$1" | sed -n 's|.*/flottille-\([^/]*\)-x86_64-linux\.tar\.gz$|\1|p' |
    { cat; echo "$2"; } | sort -uV |
    awk -v want="$2" '{ v[NR] = $0; if ($0 == want) at = NR }
      END { from = at - 2; if (from < 1) from = 1
            for (i = from; i < from + 6 && i <= NR; i++) if (i != at) print v[i] }' | head -n 5
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

# The Exec of the desktop entry for the binary $1, on one line: the path as it is, or in double
# quotes when it holds a character the format reserves (a space). Nothing when it holds one that
# Flottille does not write into an entry (the format allows them escaped; `flottille update`
# refuses the same ones, so that both write the same line).
exec_of() {
  newline='
'
  case "$1" in
    *[\"\`\$\\%]* | *"$newline"*) return 1 ;;
    *[!A-Za-z0-9/._+-]*) printf '"%s" window' "$1" ;;
    *) printf '%s window' "$1" ;;
  esac
}

# The desktop entry and the icon of the package unpacked at $1/share, written into the XDG data
# folder, with the entry running the binary $2. A package without them (older) writes nothing.
# Every step is checked: called as a condition, `set -e` does not stop it.
desktop_files() {
  share=$1/share
  [ -f "$share/applications/flottille.desktop" ] || return 0
  data=${XDG_DATA_HOME:-}
  case "$data" in /*) ;; *) data=$HOME/.local/share ;; esac
  # The path `flottille update` sees (the binary's real one), not the one reached through a
  # linked home, or the two would write the entry in turn.
  real=$(cd "$(dirname "$2")" && pwd -P) || return 1
  if ! command=$(exec_of "$real/$(basename "$2")"); then
    echo "install.sh: no desktop entry written: $2 holds a character an entry cannot name." >&2
    return 0
  fi
  mkdir -p "$data/applications" || return 1
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      Exec=*) printf 'Exec=%s\n' "$command" ;;
      *) printf '%s\n' "$line" ;;
    esac
  done <"$share/applications/flottille.desktop" >"$data/applications/.flottille.desktop.new" || return 1
  mv -f "$data/applications/.flottille.desktop.new" "$data/applications/flottille.desktop" || return 1
  icons=$(cd "$share" && find icons -type f) || return 1
  for icon in $icons; do
    mkdir -p "$data/$(dirname "$icon")" || return 1
    install -m 0644 "$share/$icon" "$data/$icon" || return 1
  done
  # A desktop's cache of the icon theme is stale when the folder is newer than it: touched, it is
  # read again and finds the icon.
  touch "$data/icons/hicolor" || return 1
  echo "Added Flottille to the application launcher ($data)."
}

main() {
  api=${FLOTTILLE_RELEASES:-https://api.github.com/repos/KristenJestin/flottille}
  channel=stable
  branch=
  version=
  chose=
  while [ $# -gt 0 ]; do
    case "$1" in
      --channel | --branch)
        if [ $# -lt 2 ] || [ -z "$2" ]; then
          echo "install.sh: $1 needs a value" >&2
          exit 2
        fi
        if [ "$1" = --channel ]; then channel=$2; else branch=$2; fi
        chose=1
        shift 2
        ;;
      --version)
        if [ $# -lt 2 ] || [ -z "$2" ]; then
          echo "install.sh: $1 needs a value" >&2
          exit 2
        fi
        version=$2
        shift 2
        ;;
      *)
        echo "install.sh: unknown argument $1" >&2
        exit 2
        ;;
    esac
  done

  if [ -n "$version" ] && [ -n "$chose" ]; then
    echo "install.sh: --version names a release of any channel: it goes without --channel and --branch" >&2
    exit 2
  fi
  # Written into an address below: a semantic version, nothing else.
  case "$version" in
    '') ;;
    *[!0-9A-Za-z.-]* | .* | -* | *.. | *-) fail "$version is not a version (X.Y.Z, or X.Y.Z-pre)" ;;
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) fail "$version is not a version (X.Y.Z, or X.Y.Z-pre)" ;;
  esac

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
  if [ -n "$version" ]; then
    tag=$(tag_of "$version")
    name="flottille-$version-x86_64-linux.tar.gz"
    url=
    if read_api "$api/releases/tags/$tag" "$answer"; then
      for found in $(archives "$answer"); do
        case "$found" in */"$name") url=$found ;; esac
      done
    fi
    if [ -z "$url" ]; then
      echo "install.sh: no release $version for Linux x86_64." >&2
      list="$work/list.json"
      if read_api "$api/releases?per_page=100" "$list"; then
        close=$(close_versions "$list" "$version" | tr '\n' ' ')
        [ -z "$close" ] || echo "install.sh: close versions: $close" >&2
      fi
      exit 1
    fi
  else
    case "$channel" in
      stable)
        read_api "$api/releases/latest" "$answer" || fail "there is no stable release yet"
        url=$(archives "$answer" | head -n 1)
        ;;
      beta)
        read_api "$api/releases?per_page=100" "$answer" || fail "cannot read the releases at $api"
        url=$(latest_published "$answer")
        ;;
      alpha)
        [ -n "$branch" ] || fail "the alpha channel needs --branch <name>"
        # Each byte but ASCII letters, digits and dashes is a dash, as CI names the alpha.
        slug=$(printf '%s' "$branch" | LC_ALL=C tr -c 'A-Za-z0-9-' '-')
        # A branch whose alphas are all deleted has none, as one that never had any.
        read_api "$api/releases?per_page=100" "$answer" || : >"$answer"
        url=$(highest_alpha "$answer" "$slug")
        [ -n "$url" ] || fail "there is no alpha release for $branch"
        ;;
      *) fail "the channel is stable, beta or alpha" ;;
    esac
  fi
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
  # The binary is installed: the launcher's files failing (a folder that cannot be written) is
  # said, not undone.
  desktop_files "$work/$folder" "$bin/flottille" ||
    echo "install.sh: the launcher entry was not written: see above." >&2
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
