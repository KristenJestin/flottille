#!/usr/bin/env bash
# The scenario `the_static_binary_runs_with_no_shared_library` (docs/releases.md): a binary that
# needs no shared library, so that it runs on any Linux of its architecture. `file` decides (it
# reads a foreign architecture too); `ldd` must list no library.
#   tools/check-static.sh <binary>
set -euo pipefail

binary=${1:?check-static.sh: the binary}
kind=$(file -b "$binary")
# A foreign architecture's `ldd` fails: what it says is read, not its status.
libraries=$(ldd "$binary" 2>&1 || true)
if ! grep -qE 'statically linked|static-pie linked' <<<"$kind"; then
  echo "check-static.sh: $binary is not static: $kind" >&2
  grep -E '=>|\.so' <<<"$libraries" >&2 || true
  exit 1
fi
if grep -qE '=>|\.so' <<<"$libraries"; then
  echo "check-static.sh: $binary needs shared libraries:" >&2
  grep -E '=>|\.so' <<<"$libraries" >&2
  exit 1
fi
echo "$binary: $kind"
