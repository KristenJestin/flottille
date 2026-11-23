#!/usr/bin/env bash
# The changelog of a release (docs/releases.md): the pull requests merged since the last release's
# mark (tools/version.sh) up to `--at` (HEAD by default), or of the range given, oldest first, a
# stable release's being the dev commits between the previous stable tag (or its mark) and the
# commit of dev it releases; on a maintenance line (`--branch stable/X.Y`), the line's commits
# since its previous tag.
#
# It says what the user can do now, in the user's words: the `Note: <sentence>` line of the
# description of each feat, fix and perf pull request (read through the API, `gh`), grouped by area
# from the commit's scope (Remote, Window, Agents, Tools, Projects, Other), the fixes in a Fixed
# group of their own, each with the number of its pull request. A pull request without a note is
# not there, nor is one whose author is not a maintainer; one that cannot be read fails the script. Then, under `### All changes`, the raw list of the subjects, grouped by type.
#   tools/changelog.sh [--at <rev> [--branch <name>] | <range>]
set -euo pipefail

range=${1:-}
if [ -z "$range" ] || [ "$range" = --at ]; then
  at=HEAD branch=()
  if [ "$range" = --at ]; then at=${2:?changelog.sh: --at needs a commit}; fi
  if [ "${3:-}" = --branch ]; then branch=(--branch "${4:?changelog.sh: --branch needs a name}"); fi
  read -r _ since <<<"$("$(dirname "$0")/version.sh" base "${branch[@]}" --at "$at")" || true
  range=$at
  if [ -n "${since:-}" ]; then range="$since..$at"; fi
fi

ANGULAR='^([a-z]+)(\(([a-z0-9-]+)\))?(!)?: (.*)$'
FOOTER=$'(^|\n)BREAKING[ -]CHANGE: '
AREAS=(remote window agents tools projects other)
declare -A TITLE=([remote]=Remote [window]=Window [agents]=Agents [tools]=Tools [projects]=Projects [other]=Other [fixed]=Fixed)
declare -A NOTES=()

# The area a change belongs to, from its scope; a change that speaks of remote is Remote whatever
# its scope (`flottille remote on`).
area_of() { # <scope> <text>
  case "$1" in
    link | hub | mobile | android) echo remote ;;
    gui) echo window ;;
    agents | acp | sessions) echo agents ;;
    tools | mcp | gate) echo tools ;;
    workspace | git | store) echo projects ;;
    *) if [[ ${2,,} == *remote* ]]; then echo remote; else echo other; fi ;;
  esac
}

# The `Note:` line of a pull request's description, nothing when it has none. The description is
# read now, after the merge, and its author can have edited it since: only a maintainer's note (the
# author is the OWNER, a MEMBER or a COLLABORATOR) goes into a release's notes, which no one can
# edit. A pull request that cannot be read stops the script: a release made without its notes
# cannot be mended.
note_of() { # <number>
  local answer body line
  if ! answer=$(gh api "repos/{owner}/{repo}/pulls/$1" --jq '.author_association + "\n" + (.body // "")'); then
    echo "changelog.sh: #$1 could not be read from the API." >&2
    return 1
  fi
  case ${answer%%$'\n'*} in
    OWNER | MEMBER | COLLABORATOR) ;;
    *) return 0 ;;
  esac
  body=${answer#*$'\n'}
  body=${body//$'\r'/}
  while IFS= read -r line; do
    if [[ $line == "Note: "* ]]; then
      line=${line#Note: }
      line=${line%"${line##*[![:space:]]}"}
      printf '%s' "$line"
      return 0
    fi
  done <<<"$body"
}

# First parents only: a release squash that a record of the releases made by squash reaches
# through its second parent is main's, not dev's (docs/releases.md).
breaking= features= fixes= performance= other=
while IFS= read -r -d '' message; do
  subject=${message%%$'\n'*}
  [[ $subject =~ $ANGULAR ]] || continue
  type=${BASH_REMATCH[1]} scope=${BASH_REMATCH[3]} bang=${BASH_REMATCH[4]} text=${BASH_REMATCH[5]}
  line="- $text"
  if [ -n "$scope" ]; then line="- **$scope**: $text"; fi
  if [ -n "$bang" ] || [[ $message =~ $FOOTER ]]; then breaking+="$line"$'\n'; fi
  case "$type" in
    feat | fix | perf)
      if [[ $text =~ \(#([0-9]+)\)$ ]]; then
        number=${BASH_REMATCH[1]}
        note=$(note_of "$number") || exit 1
        if [ -n "$note" ]; then
          if [ "$type" = fix ]; then key=fixed; else key=$(area_of "$scope" "$text"); fi
          NOTES[$key]+="- $note (#$number)"$'\n'
        fi
      fi
      ;;
  esac
  case "$type" in
    feat) features+="$line"$'\n' ;;
    fix) fixes+="$line"$'\n' ;;
    perf) performance+="$line"$'\n' ;;
    *) other+="$line"$'\n' ;;
  esac
done < <(git log -z --first-parent --no-merges --reverse --format=%B "$range")

first=1
section() { # <heading> <title> <lines>
  [ -n "$3" ] || return 0
  if [ "$first" -eq 0 ]; then echo; fi
  first=0
  printf '%s %s\n\n%s' "$1" "$2" "$3"
}
for key in "${AREAS[@]}" fixed; do
  section "###" "${TITLE[$key]}" "${NOTES[$key]:-}"
done
# The raw list: its heading is not a section of its own, its groups are.
if [ -n "$breaking$features$fixes$performance$other" ]; then
  if [ "$first" -eq 0 ]; then echo; fi
  printf '### All changes\n\n'
  first=1
  section "####" "Breaking changes" "$breaking"
  section "####" "Features" "$features"
  section "####" "Fixes" "$fixes"
  section "####" "Performance" "$performance"
  section "####" "Other changes" "$other"
fi
