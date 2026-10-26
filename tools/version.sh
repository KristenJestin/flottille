#!/usr/bin/env bash
# The version a build of HEAD gets, computed from the Angular commits since the last stable tag,
# never typed (docs/releases.md):
#   tools/version.sh stable                  X.Y.Z; exits 3, printing nothing, when no commit bumps
#   tools/version.sh beta                    X.Y.Z-beta.N
#   tools/version.sh alpha [--branch <name>] X.Y.Z-alpha.<branch>.N
# `feat` bumps the minor, `fix` and `perf` the patch, `!` or a `BREAKING CHANGE:` footer the major
# (the minor before 1.0). N counts the commits since the last stable tag.
set -euo pipefail

channel=${1:-}
case "$channel" in stable | beta | alpha | base) ;; *)
  echo "version.sh: the channel is stable, beta or alpha" >&2
  exit 2
  ;;
esac
branch=
if [ "${2:-}" = "--branch" ]; then branch=${3:?version.sh: --branch needs a name}; fi

# The last stable tag HEAD holds: the commit it tags, or, when it tags the merge of dev into
# main, the dev commit merged (its second parent), which dev goes on from.
base= since=
while read -r tag; do
  [[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
  commit=$(git rev-parse "$tag^{commit}")
  if git merge-base --is-ancestor "$commit" HEAD; then
    at=$commit
  elif merged=$(git rev-parse -q --verify "$commit^2") && git merge-base --is-ancestor "$merged" HEAD; then
    at=$merged
  else
    continue
  fi
  if [ -z "$base" ] || [ "$(printf '%s\n%s\n' "${base#v}" "${tag#v}" | sort -V | tail -1)" = "${tag#v}" ]; then
    base=$tag since=$at
  fi
done < <(git tag --list 'v*')

# `base`: the tag and the commit the versions count from (tools/changelog.sh), when there is one.
if [ "$channel" = base ]; then
  if [ -n "$base" ]; then echo "$base $since"; fi
  exit 0
fi
range=HEAD
if [ -n "$since" ]; then range="$since..HEAD"; fi
IFS=. read -r major minor patch <<<"${base#v}"
major=${major:-0} minor=${minor:-0} patch=${patch:-0}

BREAKING='^[a-z]+(\([a-z0-9-]+\))?!: '
FOOTER=$'(^|\n)BREAKING[ -]CHANGE: '
FEATURE='^feat(\([a-z0-9-]+\))?: '
FIX='^(fix|perf)(\([a-z0-9-]+\))?: '
# 3 a breaking change, 2 a feature, 1 a fix, 0 nothing that bumps.
bump=0
while IFS= read -r -d '' message; do
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
done < <(git log -z --no-merges --format=%B "$range")

if [ "$bump" -eq 0 ] && [ "$channel" = stable ]; then
  echo "version.sh: nothing to release since ${base:-the first commit}" >&2
  exit 3
fi
# Before 1.0, a breaking change bumps the minor.
if [ "$bump" -eq 3 ] && [ "$major" -eq 0 ]; then bump=2; fi
case "$bump" in
  3) major=$((major + 1)) minor=0 patch=0 ;;
  2) minor=$((minor + 1)) patch=0 ;;
  # A pre-release with nothing that bumps is still past the stable one.
  *) patch=$((patch + 1)) ;;
esac
next="$major.$minor.$patch"
count=$(git rev-list --count --first-parent "$range")

case "$channel" in
  stable) echo "$next" ;;
  beta) echo "$next-beta.$count" ;;
  alpha)
    if [ -z "$branch" ]; then branch=$(git rev-parse --abbrev-ref HEAD); fi
    # A pre-release's identifiers are letters, digits and dashes.
    name=$(printf '%s' "$branch" | tr -c 'A-Za-z0-9-' '-')
    echo "$next-alpha.$name.$count"
    ;;
esac
