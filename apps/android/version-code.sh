#!/bin/bash
# The app's versionCode for a version (`docs/releases.md`, "The version"): a number that always
# rises with the version, so that a phone installs each build over the one before.
#   apps/android/version-code.sh 0.4.1-beta.12   ->   (0*10000 + 4*100 + 1) * 10000 + 3012 = 4013012
#
# (X*10000 + Y*100 + Z) * 10000 + slot. The slot orders the builds of one X.Y.Z: an alpha
# (`X.Y.Z-alpha.<branch>.N`) 0..2999, a beta (`-beta.N`, also the next version's `-next.N`)
# 3000..5999, a release candidate (`-rc.N`) 6000..8999, the release itself 9999; N is the count
# of commits `tools/version.sh` gives, held at 2999. Android takes a code up to 2 100 000 000: Y and Z
# must stay below 100, and X below 21.
set -euo pipefail
version=${1:?version-code.sh: the version is the argument}
if ! [[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([a-z]+)(\.[0-9A-Za-z._-]+)?)?$ ]]; then
  echo "version-code.sh: $version is not a version (X.Y.Z or X.Y.Z-channel.N)" >&2
  exit 1
fi
major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}
channel=${BASH_REMATCH[5]:-} rest=${BASH_REMATCH[6]:-}
if [ "$major" -ge 21 ] || [ "$minor" -ge 100 ] || [ "$patch" -ge 100 ]; then
  echo "version-code.sh: $version does not fit a versionCode (minor and patch below 100, major below 21)" >&2
  exit 1
fi
count=0
if [[ $rest =~ \.([0-9]+)$ ]]; then count=$((10#${BASH_REMATCH[1]})); fi
[ "$count" -le 2999 ] || count=2999
case "$channel" in
  "") slot=9999 ;;
  alpha) slot=$count ;;
  beta | next) slot=$((3000 + count)) ;;
  rc) slot=$((6000 + count)) ;;
  *)
    echo "version-code.sh: $version has no channel the app knows (alpha, beta, next, rc)" >&2
    exit 1
    ;;
esac
echo $(( (major * 10000 + minor * 100 + patch) * 10000 + slot ))
