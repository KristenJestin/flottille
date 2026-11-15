#!/usr/bin/env bash
# Checks the Angular commit convention (see AGENTS.md, Git and pull requests).
#   tools/commit-message.sh <file>             one message (the commit-msg hook, a PR title)
#   tools/commit-message.sh --range <a..b|sha> every commit of a range
set -euo pipefail

TYPES='feat|fix|refactor|test|docs|chore|build|ci|perf'
# The scopes are the ones AGENTS.md lists (its "Scopes:" line, which may run on over the next
# indented lines): one list, read where people read it, so that it cannot drift from this check.
AGENTS="$(dirname "${BASH_SOURCE[0]}")/../AGENTS.md"
SCOPES=$(awk '/^- Scopes:/ {on=1; print; next} on && /^  / {print; next} {on=0}' "$AGENTS" | grep -o '`[a-z0-9-]*`' | tr -d '`' | paste -sd'|')
[ -n "$SCOPES" ] || { echo "no scopes found in $AGENTS (its \"- Scopes:\" line)" >&2; exit 1; }
PATTERN="^($TYPES)\(($SCOPES)\)!?: ([^A-Z.]|[^A-Z].*[^.])$"

check() { # <subject> <where>
  local subject=$1 where=$2
  case "$subject" in Merge\ *|Revert\ *|fixup!*|squash!*) return 0 ;; esac
  if ! [[ $subject =~ $PATTERN ]]; then
    echo "$where: \"$subject\" is not <type>(<scope>): <subject> (types: ${TYPES//|/, }; scopes: ${SCOPES//|/, }; scope required; lowercase start; no final period)" >&2
    return 1
  fi
  if [ "${#subject}" -gt 72 ]; then
    echo "$where: subject is ${#subject} characters, 72 at most" >&2
    return 1
  fi
}

if [ "${1:-}" = "--range" ]; then
  status=0
  while read -r sha; do
    check "$(git log -1 --format=%s "$sha")" "${sha:0:8}" || status=1
  done < <(git rev-list "$2")
  exit $status
fi

check "$(grep -v '^#' "$1" | head -1)" "${1##*/}"
