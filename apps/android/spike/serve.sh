#!/bin/bash
# The spike's server side on the test machine, in temporary folders of its own (never your
# configuration, keyring or home): a hub on 127.0.0.1:7448 (the phone reaches it through
# `adb reverse`), a daemon whose agent is the fake one, and the gate on 0.0.0.0:7470, the
# address the phone answers to. Stop it with Ctrl+C; its folders are left in /tmp.
# Usage, from the repository's root: apps/android/spike/serve.sh
set -euo pipefail
root=$(git rev-parse --show-toplevel)
spike=$root/apps/android/spike
cd "$root"
nice -n 19 cargo build -p flottille -p flottille-fake-agent --bins
cp -n "$root/Cargo.lock" "$spike/server/Cargo.lock" 2>/dev/null || true
nice -n 19 cargo build --manifest-path "$spike/server/Cargo.toml"
exec "$spike/server/target/debug/flottille-spike-server" --bin "$root/target/debug" \
  --listen "${SPIKE_LISTEN:-0.0.0.0:7470}" --hub 127.0.0.1:7448
