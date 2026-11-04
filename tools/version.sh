#!/usr/bin/env bash
# The version a build gets, computed from the Angular commits since the last release, never
# typed (docs/releases.md):
#   tools/version.sh stable [--at <rev>]                  X.Y.Z; exits 3, printing nothing, when
#                                                         nothing was committed since the release
#   tools/version.sh beta [--at <rev>]                    X.Y.Z-beta.N
#   tools/version.sh alpha [--branch <name>] [--at <rev>] X.Y.Z-alpha.<branch>.N
#   tools/version.sh base [--at <rev>]                    <tag> <commit> the versions count from
# `--at` is the commit to compute for, HEAD by default. The last release is the highest stable
# tag `vX.Y.Z`, wherever it is, and the versions count from its mark `released/vX.Y.Z`, the head
# of dev it released (the tag's own commit when it has no mark). `feat` bumps the minor from 1.0
# on and the patch before it, `fix` and `perf` the patch, `!` or a `BREAKING CHANGE:` footer the
# major (the minor before 1.0), and nothing that bumps is the next patch. A `Release-As: X.Y.Z`
# footer in a commit of the line sets the next version when it is above the computed one, and is
# refused (exit 1, naming the commit) when it is below or not X.Y.Z. The stable version is the
# beta's without its pre-release. N counts the commits since the mark.
set -euo pipefail

channel=${1:-}
case "$channel" in stable | beta | alpha | base) ;; *)
  echo "version.sh: the channel is stable, beta or alpha" >&2
  exit 2
  ;;
esac
shift
branch= at=HEAD
while [ $# -gt 0 ]; do
  case "$1" in
    --branch) branch=${2:?version.sh: --branch needs a name} ;;
    --at) at=${2:?version.sh: --at needs a commit} ;;
    *)
      echo "version.sh: unknown argument $1" >&2
      exit 2
      ;;
  esac
  shift 2
done
at=$(git rev-parse --verify "$at^{commit}")

# The highest stable tag, compared as versions; its mark, or its own commit.
base=$(git tag --list 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1 || true)
since=
if [ -n "$base" ]; then
  since=$(git rev-parse -q --verify "refs/tags/released/$base^{commit}" ||
    git rev-parse --verify "$base^{commit}")
fi

if [ "$channel" = base ]; then
  if [ -n "$base" ]; then echo "$base $since"; fi
  exit 0
fi
# What the mark released is not counted again, even on a branch that does not hold it.
range=$at
if [ -n "$since" ]; then range="$since..$at"; fi
count=$(git rev-list --count --first-parent "$range")
if [ "$channel" = stable ] && [ "$count" -eq 0 ]; then
  echo "version.sh: nothing to release since ${base:-the first commit}" >&2
  exit 3
fi
IFS=. read -r major minor patch <<<"${base#v}"
major=${major:-0} minor=${minor:-0} patch=${patch:-0}

BREAKING='^[a-z]+(\([a-z0-9-]+\))?!: '
RELEASE_AS='^Release-As:[[:space:]]*(.*[^[:space:]])?[[:space:]]*$'
SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
FOOTER=$'(^|\n)BREAKING[ -]CHANGE: '
FEATURE='^feat(\([a-z0-9-]+\))?: '
FIX='^(fix|perf)(\([a-z0-9-]+\))?: '
# First parents only: the release squash a record on dev reaches through its second parent is
# main's, not dev's (docs/releases.md).
# 3 a breaking change, 2 a feature, 1 a fix, 0 nothing that bumps.
bump=0
declare -a requested=() requested_by=()
while IFS= read -r -d '' record; do
  commit=${record%%$'\n'*}
  message=${record#*$'\n'}
  subject=${message%%$'\n'*}
  level=0
  if [[ $subject =~ $BREAKING ]] || [[ $message =~ $FOOTER ]]; then
    level=3
  elif [[ $subject =~ $FEATURE ]]; then
    level=2
  elif [[ $subject =~ $FIX ]]; then
    level=1
  fi
  if [ "$level" -gt "$bump" ]; then bump=$level; fi
  while IFS= read -r line; do
    if [[ $line =~ $RELEASE_AS ]]; then
      requested+=("${BASH_REMATCH[1]}")
      requested_by+=("$commit")
    fi
  done <<<"$message"
done < <(git log -z --first-parent --no-merges --format=%H%n%B "$range")

# Before 1.0, everything bumps one level less: a breaking change the minor, a feature the patch.
if [ "$major" -eq 0 ] && [ "$bump" -gt 0 ]; then bump=$((bump - 1)); fi
case "$bump" in
  3) major=$((major + 1)) minor=0 patch=0 ;;
  2) minor=$((minor + 1)) patch=0 ;;
  *) patch=$((patch + 1)) ;;
esac
next="$major.$minor.$patch"

# The highest of the requested versions wins, each of them above (or as high as) the computed one.
for i in "${!requested[@]}"; do
  wanted=${requested[$i]}
  if ! [[ $wanted =~ $SEMVER ]]; then
    echo "version.sh: commit ${requested_by[$i]} says Release-As: '$wanted', which is not X.Y.Z" >&2
    exit 1
  fi
  if [ "$(printf '%s\n%s\n' "$wanted" "$next" | sort -V | head -n 1)" != "$next" ]; then
    echo "version.sh: commit ${requested_by[$i]} says Release-As: $wanted, below the computed $next" >&2
    exit 1
  fi
done
for wanted in "${requested[@]}"; do
  if [ "$(printf '%s\n%s\n' "$wanted" "$next" | sort -V | tail -n 1)" = "$wanted" ]; then next=$wanted; fi
done

case "$channel" in
  stable) echo "$next" ;;
  beta) echo "$next-beta.$count" ;;
  alpha)
    if [ -z "$branch" ]; then branch=$(git rev-parse --abbrev-ref HEAD); fi
    # Each byte but ASCII letters, digits and dashes is a dash, the rule of every other place
    # that names an alpha (docs/releases.md).
    name=$(printf '%s' "$branch" | LC_ALL=C tr -c 'A-Za-z0-9-' '-')
    echo "$next-alpha.$name.$count"
    ;;
esac
