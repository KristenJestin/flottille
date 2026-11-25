#!/usr/bin/env bash
# The tests a pull request runs (docs/releases.md, "What CI checks"), as the arguments of
# `cargo nextest run`, on one line; nothing when there is none.
#
# - the test binaries whose files the pull request adds or changes: `crates/<crate>/tests/<name>.rs`
#   is the binary `<name>`, and any file of `crates/<crate>/tests/<name>/` is the binary `<name>`
#   when that folder has a `main.rs` (a folder without one holds helpers, not a binary). A file a
#   grouped binary declares (`crates/<crate>/tests/group/<main>.rs`, whose `mod` lines name the
#   files, and `[[test]]` in the crate's Cargo.toml its binary's name) runs that file's tests only:
#   the binary, filtered to `<file>::`. A `mod` line a pull request adds to a group's main adds the
#   file it names; the other lines of a main select nothing;
# - for a changed file of `.github/` or `tools/`, the binaries whose files name its path (the tests
#   that read `ci.yml`, run `package.sh`…);
# - the binaries on a line `Tests: <name> <name>…` of the pull request's description (a name that
#   is a grouped file's, `window_conversation`, runs that file as above);
# - for a changed file of the run sandbox (`mounts.rs`, `landlock.rs`, `ssh_owned.rs` and the run
#   tool `runs.rs` of `crates/flottille/src/`), the file `real_programs` of the tests, which runs
#   real programs through it (the fakes of the other tests read nothing as a real program does);
# - when no binary is selected, the unit tests of the crates whose `src/` the pull request
#   changes, if the crate has a library and is not one of the big ones (compiling them is the
#   cost this avoids).
#
# The output is the arguments of `cargo nextest run` on one line: `--test <binary>` for each binary
# built, and, when a grouped file is selected, `-E <filterset>` with no space in it (the line is
# split on spaces by the workflow).
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
# `binary:file` of each grouped file selected, and the binaries run whole.
filtered=()
crates=()
has() { local item=$1 x; shift; for x in "$@"; do [ "$x" = "$item" ] && return 0; done; return 1; }

# The name of the `[[test]]` of `crates/<crate>/tests/group/<main>.rs`, on stdout.
group_binary() {
  local crate=$1 main=$2
  awk -v want="tests/group/$main.rs" '
    /^\[\[test\]\]/ { name = ""; next }
    /^name[ \t]*=/ { split($0, a, "\""); name = a[2] }
    /^path[ \t]*=/ { split($0, a, "\""); if (a[2] == want) { print name; exit } }
  ' "crates/$crate/Cargo.toml"
}

# The group main of crate $1 that declares the test file $2 (a name without `.rs`), on stdout.
group_of() {
  local crate=$1 file=$2 main
  for main in "crates/$crate/tests/group/"*.rs; do
    [ -f "$main" ] || continue
    if grep -qE "^#\[path = \"\.\./$file\.rs\"\]" "$main"; then
      basename "$main" .rs
      return 0
    fi
  done
}

# Selects the file $2 (a name without `.rs`) of crate $1's tests: its group filtered to it, when a
# group declares it, else the binary of its own name.
add_file() {
  local crate=$1 file=$2 main binary
  main=$(group_of "$crate" "$file")
  if [ -n "$main" ]; then
    binary=$(group_binary "$crate" "$main")
    if [[ $binary =~ $valid ]]; then
      if ! has "$binary:$file" ${filtered[@]+"${filtered[@]}"}; then filtered+=("$binary:$file"); fi
    else
      echo "::warning::crates/$crate/tests/group/$main.rs has no [[test]] with a name this check passes on." >&2
    fi
  elif [[ $file =~ $valid ]]; then
    add_binary "$file"
  else
    echo "::warning::crates/$crate/tests/$file.rs: \`$file\` is not a name this check passes on." >&2
  fi
}

# What a changed file of a crate's tests/ selects.
select_test_file() {
  local path=$1 crate rest name main
  crate=${path#crates/}
  crate=${crate%%/*}
  rest=${path#crates/"$crate"/tests/}
  if [[ $rest == group/*.rs ]] && [[ $rest != group/*/* ]]; then
    # A group's main: only the files it gained are selected (those it kept are not changed).
    main=${rest#group/}
    main=${main%.rs}
    while IFS= read -r name; do
      if [ -n "$name" ] && [ -f "crates/$crate/tests/$name.rs" ]; then add_file "$crate" "$name"; fi
    done < <(git diff -U0 --no-renames "$BASE...$HEAD" -- "$path" | sed -n -E 's/^\+mod ([A-Za-z0-9_]+);.*/\1/p')
  elif [[ $rest == */* ]]; then
    name=${rest%%/*}
    if [ ! -f "crates/$crate/tests/$name/main.rs" ]; then
      echo "::notice::$path is not in a test binary: no test is selected for it." >&2
      return 0
    fi
    if [[ $name =~ $valid ]]; then add_binary "$name"; else echo "::warning::$path: \`$name\` is not a name this check passes on." >&2; fi
  elif [[ $rest == *.rs ]]; then
    name=${rest%.rs}
    # In a crate with groups, a file no group declares is no test binary.
    if [ -d "crates/$crate/tests/group" ] && [ -z "$(group_of "$crate" "$name")" ]; then
      echo "::warning::$path is declared by no group, so it is never built nor run: add \`#[path = \"../$name.rs\"]\` and \`mod $name;\` to a group in crates/$crate/tests/group/." >&2
      return 0
    fi
    add_file "$crate" "$name"
  fi
}
add_binary() { if [ -n "$1" ] && ! has "$1" ${binaries[@]+"${binaries[@]}"}; then binaries+=("$1"); fi; }

# Not read as a stream: a failure of git must fail this script, not say "nothing changed".
# Deleted files are left out: there is nothing of them to run.
files=$(git diff --name-only --no-renames --diff-filter=AM "$BASE...$HEAD")
while IFS= read -r path; do
  case "$path" in
    crates/*/tests/*) select_test_file "$path" ;;
    # The run sandbox: real programs run through it, a fake would not tell.
    crates/flottille/src/mounts.rs | crates/flottille/src/landlock.rs | \
      crates/flottille/src/ssh_owned.rs | crates/flottille/src/runs.rs)
      add_file flottille real_programs
      ;;&
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
        if [ -n "$test" ]; then select_test_file "$test"; fi
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
      # A grouped file's name runs that file, as a changed file does; any other is a binary.
      found=no
      for crate_dir in crates/*/; do
        crate=${crate_dir#crates/}
        crate=${crate%/}
        if [ -f "crates/$crate/tests/$name.rs" ] && [ -n "$(group_of "$crate" "$name")" ]; then
          add_file "$crate" "$name"
          found=yes
        fi
      done
      if [ "$found" = no ]; then add_binary "$name"; fi
    else
      echo "::warning::\`Tests:\` lists \`$name\`, which is not a test binary's name: it is not run." >&2
    fi
  done
fi

# A binary run whole needs no filter on its files; the binaries in one order, whatever the order
# the files were read in.
terms=()
if [ ${#filtered[@]} -gt 0 ]; then
  mapfile -t filtered < <(printf '%s\n' "${filtered[@]}" | LC_ALL=C sort)
  built=()
  for item in "${filtered[@]}"; do
    binary=${item%%:*}
    if ! has "$binary" ${binaries[@]+"${binaries[@]}"} && ! has "$binary" ${built[@]+"${built[@]}"}; then built+=("$binary"); fi
  done
  for binary in ${built[@]+"${built[@]}"}; do
    files=()
    for item in "${filtered[@]}"; do
      if [ "${item%%:*}" = "$binary" ]; then files+=("test(/^${item#*:}::/)"); fi
    done
    expr=$(IFS='|'; echo "${files[*]}")
    terms+=("binary(=$binary)&($expr)")
  done
  for binary in ${binaries[@]+"${binaries[@]}"}; do terms+=("binary(=$binary)"); done
  mapfile -t binaries < <(printf '%s\n' ${binaries[@]+"${binaries[@]}"} ${built[@]+"${built[@]}"} | LC_ALL=C sort -u)
fi

args=()
if [ ${#binaries[@]} -gt 0 ]; then
  for name in "${binaries[@]}"; do args+=(--test "$name"); done
  if [ ${#terms[@]} -gt 0 ]; then args+=(-E "$(IFS='|'; echo "${terms[*]}")"); fi
elif [ ${#crates[@]} -gt 0 ]; then
  for crate in "${crates[@]}"; do args+=(-p "$crate"); done
  args+=(--lib)
fi
echo "${args[*]:-}"
