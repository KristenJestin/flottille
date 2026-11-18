#!/usr/bin/env bash
# The scenario `the_image_starts_prints_a_setup_code_and_answers_on_its_port` (docs/releases.md):
# the image started on a port of the loopback only, its log shows a setup code, and its port
# answers (`426 Upgrade Required` to a request that asks no upgrade). The code is never printed:
# CI logs are public.
#   tools/check-hub-image.sh <image>
# HUB_CHECK_TRIES (20) and HUB_CHECK_PAUSE (1, seconds) say how long it waits for each.
set -euo pipefail

image=${1:?check-hub-image.sh: the image}
tries=${HUB_CHECK_TRIES:-20}
pause=${HUB_CHECK_PAUSE:-1}
name="flottille-hub-check-$$"
# Whatever happens, the container goes, and a failure says what the hub said (the code masked).
cleanup() {
  status=$?
  if [ "$status" -ne 0 ]; then
    docker logs "$name" 2>&1 | sed -E 's/(setup code: )[^,]*/\1<masked>/' >&2 || true
  fi
  docker rm -f "$name" >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

# As compose.example.yml starts it: nothing writable but the state volume, no capability.
docker run -d --name "$name" --read-only --cap-drop ALL --security-opt no-new-privileges:true \
  -p 127.0.0.1::7448 "$image" >/dev/null

seen=
for _ in $(seq "$tries"); do
  if docker logs "$name" 2>&1 | grep -q 'flottille-hub: setup code: '; then
    seen=yes
    break
  fi
  sleep "$pause"
done
if [ -z "$seen" ]; then
  echo "check-hub-image.sh: no setup code in the log of $image" >&2
  exit 1
fi

port=$(docker port "$name" 7448/tcp | head -n 1)
port=${port##*:}
answer=000
for _ in $(seq "$tries"); do
  answer=$(curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 "http://127.0.0.1:$port/" || true)
  if [ "$answer" = 426 ]; then
    echo "$image: a setup code in its log, and its port answers ($answer)"
    exit 0
  fi
  sleep "$pause"
done
echo "check-hub-image.sh: the port does not answer as the hub does (got $answer, wanted 426)" >&2
exit 1
