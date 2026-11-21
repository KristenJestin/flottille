#!/bin/bash
# The Android job of CI (`docs/remote.md`, "The Android app"), from the repository's root on a
# GitHub runner (ubuntu, the Android SDK and NDK, KVM): the core built with cargo ndk, the Gradle
# unit tests (the bindings on the JVM, the libraries' 16 KB alignment), then the instrumentation
# tests on an emulator. As #823's spike ran them.
#   apps/android/ci.sh
set -euo pipefail
started=$SECONDS
root=$(pwd)
android=$root/apps/android
out=${ANDROID_OUT:-$(mktemp -d)}
export ANDROID_SDK_ROOT=$ANDROID_HOME
export ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-$ANDROID_NDK_LATEST_HOME}
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
echo no | "$sdk/avdmanager" create avd --force -n core -k "$image" -d pixel_6 >"$out/avd.log" 2>&1 ||
  { tail -20 "$out/avd.log"; exit 1; }
emulator=$ANDROID_HOME/emulator/emulator
adb=$ANDROID_HOME/platform-tools/adb
"$emulator" -avd core -no-window -no-audio -no-boot-anim -no-snapshot -gpu swiftshader_indirect \
  -camera-back none -camera-front none >"$out/emulator.log" 2>&1 &

step "the core, the unit tests and the test APK, while it boots"
"$android/gradle.sh" :core:testDebugUnitTest :core:assembleDebugAndroidTest >"$out/gradle.log" 2>&1 || {
  grep -E '^e: |error|FAILED|FAILURE|What went wrong|Exception' -A4 "$out/gradle.log" | head -80; exit 1; }
grep -E 'Test.*(PASSED|FAILED)|tests completed' "$out/gradle.log" || true
for result in "$android"/core/build/test-results/testDebugUnitTest/*.xml; do
  grep -o 'testcase name="[^"]*"' "$result" || true
done

step "waiting for the emulator to boot"
timeout 300 "$adb" wait-for-device || { echo "no emulator in 5 minutes"; tail -40 "$out/emulator.log"; exit 1; }
for _ in $(seq 1 180); do
  [ "$("$adb" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] && break
  sleep 2
done
[ "$("$adb" shell getprop sys.boot_completed | tr -d '\r')" = 1 ] || { tail -30 "$out/emulator.log"; exit 1; }

step "the instrumentation tests"
"$adb" install -r "$android/core/build/outputs/apk/androidTest/debug/core-debug-androidTest.apk" >/dev/null
set +e
timeout 600 "$adb" shell am instrument -w -r org.flottille.core.test/androidx.test.runner.AndroidJUnitRunner \
  >"$out/instrument.txt" 2>&1
set -e
grep -E 'INSTRUMENTATION_STATUS: (test|stack)=|OK \(|FAILURES|Tests run|INSTRUMENTATION_CODE|Error|Exception' \
  "$out/instrument.txt" | head -60
step "done"
grep -q '^OK (' "$out/instrument.txt"
