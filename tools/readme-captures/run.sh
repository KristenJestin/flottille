#!/bin/bash
# Renders the README's screenshots (docs/images/) from an invented world: a throwaway daemon with
# its own HOME and XDG folders, the fake ACP agent playing script.json, throwaway git repositories
# (a recipes app, pantry, with an api and a web repository, and a CLI, tidepool) and the window,
# captured in a headless nested Weston. Nothing touches the maintainer's configuration, data,
# programs or screens, and no real agent runs. Every process it starts is stopped by its pid or by
# the throwaway environment it carries, never by name.
#   tools/readme-captures/run.sh [<target>/debug [<out folder>]]
# needs: weston (with Xwayland), weston-screenshooter, dbus-daemon, git, python3, ImageMagick.
# Build first: cargo build -p flottille -p flottille-fake-agent -p flottille-window --examples
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
T=${1:-${CARGO_TARGET_DIR:-$root/target}/debug}
OUT=${2:-$root/docs/images}
W=1280; H=900
for need in weston weston-screenshooter dbus-daemon magick python3 git; do
  command -v "$need" >/dev/null || { echo "readme-captures: $need is needed" >&2; exit 1; }
done
for built in flottille flottille-fake-agent examples/shell; do
  [ -x "$T/$built" ] || { echo "readme-captures: $T/$built is not built" >&2; exit 1; }
done
mkdir -p "$OUT"
D=$(mktemp -d /tmp/flottille-readme.XXXX)
mkdir -p "$D"/{home,runtime,state,config,data,fake,wrun,bin,remotes}; chmod 700 "$D"/*

# The processes that carry the throwaway runtime folder: the daemon and its agents.
own() {
  for p in /proc/[0-9]*; do
    tr '\0' '\n' <"$p/environ" 2>/dev/null | grep -qx "XDG_RUNTIME_DIR=$D/runtime" && echo "${p#/proc/}"
  done 2>/dev/null
}
cleanup() {
  kill "${WPID:-}" "${APID:-}" 2>/dev/null
  [ -f "$D/dbus.pid" ] && kill "$(cat "$D/dbus.pid")" 2>/dev/null
  kill $(own) 2>/dev/null
  sleep 1
  rm -rf "$D"
}
trap cleanup EXIT

export HOME=$D/home XDG_RUNTIME_DIR=$D/runtime XDG_STATE_HOME=$D/state XDG_CONFIG_HOME=$D/config XDG_DATA_HOME=$D/data
export FLOTTILLE_AGENT_COMMAND=$T/flottille-fake-agent FLOTTILLE_FAKE_AGENT_DIR=$D/fake
export GIT_AUTHOR_NAME="Sam Rivera" GIT_AUTHOR_EMAIL=sam@example.com
export GIT_COMMITTER_NAME="Sam Rivera" GIT_COMMITTER_EMAIL=sam@example.com
export PATH=$D/bin:$PATH
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
cp "$here/script.json" "$D/fake/script.json"
cp "$here/npm" "$D/bin/npm"; chmod +x "$D/bin/npm"
# Programs the scene must never reach: a stand-in that fails.
for stub in gh bkt acli docker ssh sudo doas pkexec; do
  printf '#!/bin/sh\necho "$0: a stand-in for captures" >&2; exit 1\n' >"$D/bin/$stub"; chmod +x "$D/bin/$stub"
done

repo() { # folder files...
  local r=$1; shift; mkdir -p "$r"; git -C "$r" init -q -b main
  for f in "$@"; do mkdir -p "$r/$(dirname "$f")"; printf '%s\n' "// $f" >"$r/$f"; done
  git -C "$r" add .; git -C "$r" commit -qm "chore: start"
  git init -q --bare "$D/remotes/$(basename "$r").git"
}
# The remote a repository shows is its public address; the push goes to the local bare one.
publish() { # folder public-url
  local name; name=$(basename "$1")
  git -C "$1" remote add origin "$D/remotes/$name.git"; git -C "$1" push -q origin main
  git -C "$1" remote set-url origin "$2"
}
L=$HOME/code/pantry; mkdir -p "$L"
repo "$L/api" src/recipes/service.ts src/shopping/units.ts src/shopping/merge.ts package.json
repo "$L/web" src/pages/RecipePage.tsx src/components/Checklist.tsx package.json
printf '# pantry\n\nRecipes and shopping lists for a household.\n' >"$L/README.md"
P=$HOME/code/tidepool; repo "$P" src/main.rs Cargo.toml README.md
publish "$L/api" https://github.com/pantry-app/api.git
publish "$L/web" https://github.com/pantry-app/web.git
publish "$P" https://github.com/tidepool-app/tidepool.git

F=$T/flottille
"$F" status --json >/dev/null
"$F" project add "$L" --name pantry --yes >/dev/null
"$F" project add "$P" --name tidepool --yes >/dev/null
cat >>"$XDG_CONFIG_HOME/flottille/projects/pantry.toml" <<'TOML'

[tasks.test]
line = "npm test"
repository = "api"
check = true

[[setup]]
run = "npm install"
for_each_repository = true

[skills.api-conventions]
description = "How routes, errors and tests are written in the api"
path = "api/package.json"
TOML

# The permission gate's judge, the daemon's model for it: the fake agent answers it (script.json).
python3 - "$D/runtime/flottille/daemon.sock" <<'PY'
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall((json.dumps({"id": "1", "method": "provider.set_judge", "params": {"provider": "claude", "model": "haiku"}}) + "\n").encode())
s.makefile().readline()
PY
"$F" workspace create pantry shopping-list --offline >/dev/null 2>&1
"$F" workspace create pantry metric-units --offline >/dev/null 2>&1
sleep 4
agent_id() { python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["agent"]["id"])'; }
LEAD=$("$F" agent start pantry "Add a shopping list: an endpoint in the api that merges the ingredients of the chosen recipes and a Shopping list page in the web app. Split it between workers, and start the database for their tests." --workspace shopping-list --json | agent_id)
sleep 2
"$F" agent start pantry "Why does the nightly test run take twice as long since last week?" --json >/dev/null
"$F" agent start pantry "Add metric units (ml, l) to the shopping list conversions and add tests for the boundary at 1000." --workspace metric-units --json >/dev/null
"$F" agent start tidepool "Make the tide chart fetch retry with backoff when the station API times out." --json >/dev/null
sleep 12
CHILD=$("$F" agent list | grep 'api: shopping' | head -1 | grep -o 'agent [0-9a-f]\{12\}' | head -1 | cut -d' ' -f2)

# The window, in a nested headless Weston (with Xwayland: GPUI needs a seat) and a private bus.
XDG_RUNTIME_DIR=$D/wrun weston --backend=headless --renderer=gl --xwayland --shell=kiosk --fake-seat \
  --debug --socket=wl-readme --width=$W --height=$H >"$D/weston.log" 2>&1 &
WPID=$!
for _ in $(seq 50); do grep -q 'xserver listening on display' "$D/weston.log" && break; sleep 0.2; done
DISP=$(grep -o 'listening on display :[0-9]*' "$D/weston.log" | head -1 | grep -o ':[0-9]*')
cat >"$D/dbus.conf" <<CONF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path=$D/bus</listen><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>
CONF
dbus-daemon --config-file="$D/dbus.conf" --fork --print-pid >"$D/dbus.pid"
export DBUS_SESSION_BUS_ADDRESS=unix:path=$D/bus

shot() { # name args...
  local name=$1; shift
  DISPLAY=$DISP "$T/examples/shell" --size ${W}x${H} "$@" >"$D/shell-$name.log" 2>&1 &
  APID=$!
  sleep 8
  (cd "$D" && rm -f wayland-screenshot*.png \
    && XDG_RUNTIME_DIR=$D/wrun WAYLAND_DISPLAY=wl-readme weston-screenshooter >/dev/null 2>&1)
  kill "$APID"; wait "$APID" 2>/dev/null
  # weston-screenshooter writes the image upside down, with the pointer in it: flip it, paint the
  # pointer over with the colour beside it, drop the metadata.
  magick "$D"/wayland-screenshot*.png -flip "$D/flat.png"
  local c; c=$(magick "$D/flat.png" -format "%[pixel:p{150,108}]" info:)
  magick "$D/flat.png" -fill "$c" -draw "rectangle 92,96 120,128" -strip "$OUT/$name.png"
  echo "wrote $OUT/$name.png"
}
shot session --session "$LEAD" --details
shot project --project pantry
shot sub-agent --session "$LEAD" --agent "$CHILD"
