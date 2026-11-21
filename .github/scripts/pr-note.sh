#!/usr/bin/env bash
# The reminder of the `shape` check (.github/workflows/pull-request.yml): a `feat` or `fix` pull
# request that touches the product carries a line `Note: <one sentence, what the user can do now>`
# in its description, which the release's "What's new" page says in the user's words (AGENTS.md,
# "Git and pull requests"). Without one it is a warning, never a failure: a pull request nobody can
# see is allowed to have none.
#
# Reads TITLE and BODY (the pull request's) and CHANGED (the paths it changes, one per line).
# Says its warning as a workflow command, on stdout.
set -euo pipefail

title=${TITLE:-} body=${BODY:-} changed=${CHANGED:-}
[[ $title =~ ^(feat|fix)[(:!] ]] || exit 0

# The product is what ships: its crates and the apps, not the fakes nor the tests of a crate.
product=
while IFS= read -r path; do
  case "$path" in
    crates/flottille-fake-*/* | */tests/*) ;;
    crates/* | apps/*) product=1 ;;
  esac
done <<<"$changed"
[ -n "$product" ] || exit 0

body=${body//$'\r'/}
if grep -qE '^Note: .*[^[:space:]]' <<<"$body"; then exit 0; fi
echo "::warning title=No Note line::This $title pull request changes the product, but its description has no line \`Note: <one sentence, what the user can do now>\`. Add one, or leave it out when the user cannot see the change."
