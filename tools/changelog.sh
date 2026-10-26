#!/usr/bin/env bash
# The changelog of a release (docs/releases.md): the Angular commits since the last stable tag
# (tools/version.sh), or of the range given, grouped by type, oldest first, each with the pull
# request number its squash kept.
#   tools/changelog.sh [<range>]
set -euo pipefail

range=${1:-}
if [ -z "$range" ]; then
  read -r _ since <<<"$("$(dirname "$0")/version.sh" base)" || true
  range=HEAD
  if [ -n "${since:-}" ]; then range="$since..HEAD"; fi
fi

ANGULAR='^([a-z]+)(\(([a-z0-9-]+)\))?(!)?: (.*)$'
FOOTER=$'(^|\n)BREAKING[ -]CHANGE: '
breaking= features= fixes= performance= other=
while IFS= read -r -d '' message; do
  subject=${message%%$'\n'*}
  [[ $subject =~ $ANGULAR ]] || continue
  type=${BASH_REMATCH[1]} scope=${BASH_REMATCH[3]} bang=${BASH_REMATCH[4]} text=${BASH_REMATCH[5]}
  line="- $text"
  if [ -n "$scope" ]; then line="- **$scope**: $text"; fi
  if [ -n "$bang" ] || [[ $message =~ $FOOTER ]]; then breaking+="$line"$'\n'; fi
  case "$type" in
    feat) features+="$line"$'\n' ;;
    fix) fixes+="$line"$'\n' ;;
    perf) performance+="$line"$'\n' ;;
    *) other+="$line"$'\n' ;;
  esac
done < <(git log -z --no-merges --reverse --format=%B "$range")

first=1
section() { # <title> <lines>
  [ -n "$2" ] || return 0
  if [ "$first" -eq 0 ]; then echo; fi
  first=0
  printf '### %s\n\n%s' "$1" "$2"
}
section "Breaking changes" "$breaking"
section "Features" "$features"
section "Fixes" "$fixes"
section "Performance" "$performance"
section "Other changes" "$other"
