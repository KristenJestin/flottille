#!/usr/bin/env bash
# The tests a pull request runs (docs/releases.md, "What CI checks"), as the arguments of
# `cargo nextest run`, on one line; nothing when there is none.
#
# - the test binaries whose files the pull request adds or changes: `crates/<crate>/tests/<name>.rs`
#   is the binary `<name>`, and any file of `crates/<crate>/tests/<name>/` is the binary `<name>`
#   when that folder has a `main.rs` (a folder without one holds helpers, not a binary);
# - for a changed file of `.github/` or `tools/`, the binaries whose files name its path (the tests
#   that read `ci.yml`, run `package.sh`…);
# - the binaries on a line `Tests: <name> <name>…` of the pull request's description;
# - when no binary is selected, the unit tests of the crates whose `src/` the pull request
#   changes, if the crate has a library and is not one of the big ones (compiling them is the
#   cost this avoids).
#
# Run in a checkout of the pull request. Reads BASE and HEAD (commits) and BODY (the description).
# A name reaches a command line: only letters, digits, dashes and underscores are kept, and it
# does not start with a dash (that would be an option).
set -euo pipefail

: "${BASE:?the commit the pull request starts from}" "${HEAD:?the last commit of the pull request}"
body=${BODY:-}
big=" flottille flottille-window "
valid='^[A-Za-z0-9_][A-Za-z0-9_-]*$'

binaries=()
crates=()
has() { local item=$1 x; shift; for x in "$@"; do [ "$x" = "$item" ] && return 0; done; return 1; }

# The binary a file of a crate's tests/ belongs to, on stdout; nothing (a notice) when it is none.
binary_of() {
  local path=$1 crate rest name
  crate=${path#crates/}
  crate=${crate%%/*}
  rest=${path#crates/"$crate"/tests/}
  if [[ $rest == */* ]]; then
    name=${rest%%/*}
    if [ ! -f "crates/$crate/tests/$name/main.rs" ]; then
      echo "::notice::$path is not in a test binary: no test is selected for it." >&2
      return 0
    fi
  elif [[ $rest == *.rs ]]; then
    name=${rest%.rs}
  else
    return 0
  fi
  if [[ $name =~ $valid ]]; then
    echo "$name"
  else
    echo "::warning::$path: \`$name\` is not a name this check passes on." >&2
  fi
}
add_binary() { if [ -n "$1" ] && ! has "$1" ${binaries[@]+"${binaries[@]}"}; then binaries+=("$1"); fi; }

# Not read as a stream: a failure of git must fail this script, not say "nothing changed".
# Deleted files are left out: there is nothing of them to run.
files=$(git diff --name-only --no-renames --diff-filter=AM "$BASE...$HEAD")
while IFS= read -r path; do
  case "$path" in
    crates/*/tests/*) add_binary "$(binary_of "$path")" ;;
    crates/*/src/*)
      crate=${path#crates/}
      crate=${crate%%/*}
      if [[ $crate =~ $valid ]] && [[ $big != *" $crate "* ]] && [ -f "crates/$crate/src/lib.rs" ]; then
        if ! has "$crate" ${crates[@]+"${crates[@]}"}; then crates+=("$crate"); fi
      fi
      ;;
    # A workflow, a script or a tool is run by the tests that name it (they read ci.yml, run
    # package.sh…): those binaries run, so that a change of it is not judged by nothing.
    .github/* | tools/*)
      while IFS= read -r test; do
        if [ -n "$test" ]; then add_binary "$(binary_of "$test")"; fi
      done < <(git grep -lF -e "$path" "$HEAD" -- 'crates/*/tests/*' | sed "s/^[^:]*://" || true)
      ;;
  esac
done <<< "$files"
# The files' binaries in one order, whatever the order git lists the paths in.
if [ ${#binaries[@]} -gt 1 ]; then
  mapfile -t binaries < <(printf '%s\n' "${binaries[@]}" | LC_ALL=C sort)
fi

# The description's own list, read on the first line that starts with `Tests:`.
line=$(printf '%s\n' "$body" | tr -d '\r' | grep -m1 -E '^[[:space:]]*Tests:' || true)
if [ -n "$line" ]; then
  line=${line#*Tests:}
  for name in ${line//,/ }; do
    if [[ $name =~ $valid ]]; then
      if ! has "$name" ${binaries[@]+"${binaries[@]}"}; then binaries+=("$name"); fi
    else
      echo "::warning::\`Tests:\` lists \`$name\`, which is not a test binary's name: it is not run." >&2
    fi
  done
fi

args=()
if [ ${#binaries[@]} -gt 0 ]; then
  for name in "${binaries[@]}"; do args+=(--test "$name"); done
elif [ ${#crates[@]} -gt 0 ]; then
  for crate in "${crates[@]}"; do args+=(-p "$crate"); done
  args+=(--lib)
fi
echo "${args[*]:-}"
