#!/bin/bash
# The spike's instrumentation tests on an emulator of a GitHub runner (ubuntu, the Android SDK,
# KVM), from the repository's root, once the workspace's binaries are built (target/debug):
#   apps/android/spike/ci.sh
# KVM opened to the runner's user, an emulator image and an AVD made, the APKs and the server
# built while it boots, the server started, the tests run; the gate's record printed at the end.
set -euo pipefail
started=$SECONDS
root=$(pwd)
spike=$root/apps/android/spike
out=${SPIKE_OUT:-$(mktemp -d)}
export JAVA_HOME=${JAVA_HOME_17_X64:-$JAVA_HOME}
export ANDROID_SDK_ROOT=$ANDROID_HOME
# One folder for the AVD, which avdmanager and the emulator otherwise look for in two places.
export ANDROID_USER_HOME=$out/android-user
export ANDROID_AVD_HOME=$ANDROID_USER_HOME/avd
mkdir -p "$ANDROID_AVD_HOME"
sdk=$ANDROID_HOME/cmdline-tools/latest/bin
image="system-images;android-34;default;x86_64"
step() { echo "== $(( SECONDS - started )) s: $*"; }

step "KVM for the runner's user"
echo 'KERNEL=="kvm", GROUP="kvm", MODE="0666", OPTIONS+="static_node=kvm"' |
  sudo tee /etc/udev/rules.d/99-kvm4all.rules >/dev/null
sudo udevadm control --reload-rules
sudo udevadm trigger --name-match=kvm

step "the emulator and its image"
yes | "$sdk/sdkmanager" --licenses >/dev/null 2>&1 || true
"$sdk/sdkmanager" --install emulator platform-tools "$image" >"$out/sdkmanager.log" 2>&1
echo no | "$sdk/avdmanager" create avd --force -n spike -k "$image" -d pixel_6 >"$out/avd.log" 2>&1 ||
  { tail -20 "$out/avd.log"; exit 1; }
emulator=$ANDROID_HOME/emulator/emulator
"$emulator" -list-avds | grep -qx spike || { echo "the AVD is not where the emulator looks"; tail -20 "$out/avd.log"; exit 1; }
adb=$ANDROID_HOME/platform-tools/adb
"$emulator" -avd spike -no-window -no-audio -no-boot-anim -no-snapshot -gpu swiftshader_indirect \
  -camera-back none -camera-front none >"$out/emulator.log" 2>&1 &

step "the server and the APKs, while it boots"
cp -n "$root/Cargo.lock" "$spike/server/Cargo.lock" 2>/dev/null || true
cargo build --manifest-path "$spike/server/Cargo.toml" >"$out/server-build.log" 2>&1 ||
  { tail -40 "$out/server-build.log"; exit 1; }
"$spike/gradle.sh" :app:assembleDebug :distributor:assembleDebug :distributor:assembleDebugAndroidTest \
  >"$out/gradle.log" 2>&1 || { grep -E '^e: |error|FAILURE|What went wrong' -A3 "$out/gradle.log" | head -60; exit 1; }

step "waiting for the emulator to boot"
timeout 300 "$adb" wait-for-device || {
  echo "no emulator in 5 minutes"; tail -40 "$out/emulator.log"; "$emulator" -list-avds; tail -5 "$out/sdkmanager.log"; exit 1; }
for _ in $(seq 1 180); do
  [ "$("$adb" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] && break
  sleep 2
done
[ "$("$adb" shell getprop sys.boot_completed | tr -d '\r')" = 1 ] || { tail -30 "$out/emulator.log"; exit 1; }
"$adb" shell settings put global window_animation_scale 0
"$adb" shell settings put global transition_animation_scale 0
"$adb" shell settings put global animator_duration_scale 0
"$adb" shell input keyevent 82

# The stub distributor's endpoint is the emulator's address of the host, 10.0.2.2: the host takes
# it too, so that the push goes by HTTP from the server to it as from a push service.
sudo ip addr add 10.0.2.2/32 dev lo

step "the server: a hub, a daemon of temporary folders, the fake agent, the gate"
"$spike/server/target/debug/flottille-spike-server" --bin "$root/target/debug" \
  --listen 0.0.0.0:7470 --hub 127.0.0.1:7448 >"$out/server.out" 2>"$out/server.log" &
server=$!
trap 'kill $server 2>/dev/null || true' EXIT
for _ in $(seq 1 120); do grep -q '"ready"' "$out/server.out" && break; sleep 0.5; done
grep -q '"ready"' "$out/server.out" || { tail -30 "$out/server.log"; exit 1; }
# The hub through adb, as the real-phone run reaches it: only the join goes there. The gate is
# reached over the emulator's network (10.0.2.2 is the host), which Doze cuts.
"$adb" reverse tcp:7448 tcp:7448

step "the tests"
"$adb" install -r "$spike/app/build/outputs/apk/debug/app-debug.apk" >/dev/null
"$adb" install -r "$spike/distributor/build/outputs/apk/debug/distributor-debug.apk" >/dev/null
"$adb" install -r "$spike/distributor/build/outputs/apk/androidTest/debug/distributor-debug-androidTest.apk" >/dev/null
"$adb" logcat -c
set +e
timeout 900 "$adb" shell am instrument -w -r -e gate http://10.0.2.2:7470 \
  org.flottille.spike.distributor.test/androidx.test.runner.AndroidJUnitRunner >"$out/instrument.txt" 2>&1
set -e
"$adb" logcat -d -s FlottilleSpike SpikeDistributor TestRunner >"$out/logcat.txt" 2>&1 || true

step "done"
echo "--- instrumentation"
grep -E 'INSTRUMENTATION_STATUS: (test|stack)=|INSTRUMENTATION_STATUS_CODE|OK \(|FAILURES|Tests run|INSTRUMENTATION_CODE|Error|Exception' \
  "$out/instrument.txt" | head -60
echo "--- the phone's log"
grep -E 'FlottilleSpike|SpikeDistributor' "$out/logcat.txt" | tail -40
echo "--- the gate's record"
grep -v '^$' "$out/server.log" | tail -40
echo "--- the whole run took $(( SECONDS - started )) s"
grep -q '^OK (' "$out/instrument.txt"
