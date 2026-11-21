#!/bin/bash
# The app's APK, as CI builds it (`docs/releases.md`, "The version"), from the repository's root on
# a GitHub runner, never by hand: the version `tools/version.sh` gives the build, whatever its
# channel, is the APK's versionName, and its versionCode is derived from it (`version-code.sh`).
#   apps/android/apk.sh                      the APK is in $APK_OUT (default: a new temporary folder)
# Environment: HEAD_REF, HEAD_SHA (a pull request's head), else GITHUB_REF_NAME and GITHUB_SHA.
# This script holds no key and reads no secret: Gradle builds the release APK unsigned, and
#   $APK_OUT/unsigned/flottille-<version>-unsigned.apk   is what CI's signing job signs with the
#                                                        release key, in a job of its own;
#   $APK_OUT/test/flottille-<version>-debug-signed.apk   is the same APK signed with a key made for
#                                                        this run: it installs on a phone for a
#                                                        test, and is never released.
set -euo pipefail
root=$(pwd)
android=$root/apps/android
export ANDROID_SDK_ROOT=${ANDROID_HOME:?the Android SDK}
export ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:?the Android NDK}}
out=${APK_OUT:-$(mktemp -d)}
mkdir -p "$out"

branch=${HEAD_REF:-${GITHUB_REF_NAME:-$(git rev-parse --abbrev-ref HEAD)}}
sha=${HEAD_SHA:-${GITHUB_SHA:-$(git rev-parse HEAD)}}
if [ "${GITHUB_REF_TYPE:-}" = tag ]; then
  # The release of a stable line (`release.yml`): the tag is the version.
  version=${GITHUB_REF_NAME#v}
else
  # The channel of the branch, as `build` decides it: main is stable, dev beta, a stable line's and
  # the next version's builds are the "beta" of version.sh, any other branch its own alpha.
  case "$branch" in
    main) channel=stable ;;
    dev | stable/* | next/*) channel=beta ;;
    *) channel=alpha ;;
  esac
  status=0
  version=$(tools/version.sh "$channel" --branch "$branch" --at "$sha") || status=$?
  # 3: nothing was committed since the last release, which is then this one: nothing to build.
  if [ "$status" -eq 3 ]; then
    echo "::notice::nothing was committed since the last release: no APK."
    exit 0
  fi
  [ "$status" -eq 0 ] || exit "$status"
fi
code=$("$android/version-code.sh" "$version")
echo "The APK of $version (versionCode $code), built from ${sha:0:12} on $branch."

"$android/gradle.sh" :app:assembleRelease "-Pflottille.version=$version" "-Pflottille.versionCode=$code"
built=$android/app/build/outputs/apk/release/app-release-unsigned.apk
[ -f "$built" ] || { echo "no APK was built" >&2; exit 1; }

# The version the APK says, read back out of it.
aapt2=$(ls "${ANDROID_HOME:?}"/build-tools/*/aapt2 | sort -V | tail -n 1)
badging=$("$aapt2" dump badging "$built" | grep '^package:')
case "$badging" in
  *"versionCode='$code'"*"versionName='$version'"*) echo "$badging" ;;
  *)
    echo "the APK does not say $version ($code): $badging" >&2
    exit 1
    ;;
esac

# A phone refuses an APK whose parts are not aligned: said here, not by a phone.
zipalign=$(ls "$ANDROID_HOME"/build-tools/*/zipalign | sort -V | tail -n 1)
"$zipalign" -c 4 "$built" || { echo "the APK is not aligned" >&2; exit 1; }

# The copy to try on a phone: a key made for this run, said by its name and a notice.
apksigner=$(ls "$ANDROID_HOME"/build-tools/*/apksigner | sort -V | tail -n 1)
mkdir -p "$out/unsigned" "$out/test"
unsigned=$out/unsigned/flottille-$version-unsigned.apk
test_apk=$out/test/flottille-$version-debug-signed.apk
cp "$built" "$unsigned"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
keytool -genkeypair -keystore "$scratch/debug.jks" -storepass debug-key -keypass debug-key \
  -alias debug -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Flottille debug" 2>/dev/null
"$apksigner" sign --v4-signing-enabled false --ks "$scratch/debug.jks" --ks-pass pass:debug-key --key-pass pass:debug-key \
  --out "$test_apk" "$unsigned"
"$apksigner" verify "$test_apk"
echo "::notice::the APK is built unsigned; the debug-signed copy is to install for a test and never to release. The release key signs it in the signing job, on a push only."
echo "apk=$test_apk"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "version=$version"
    echo "dir=$out/test"
    echo "unsigned=$out/unsigned"
  } >> "$GITHUB_OUTPUT"
fi
