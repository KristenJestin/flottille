#!/bin/bash
# The app's APK, as CI builds it (`docs/releases.md`, "The version"), from the repository's root on
# a GitHub runner, never by hand: the version `tools/version.sh` gives the build, whatever its
# channel, is the APK's versionName, and its versionCode is derived from it (`version-code.sh`).
#   apps/android/apk.sh                      the APK is in $APK_OUT (default: a new temporary folder)
# Environment: HEAD_REF, HEAD_SHA (a pull request's head), else GITHUB_REF_NAME and GITHUB_SHA;
# KEYSTORE_BASE64 (the release key, from the repository's secrets), FLOTTILLE_KEYSTORE_PASSWORD,
# FLOTTILLE_KEY_ALIAS, FLOTTILLE_KEY_PASSWORD: all four or none. The key is never printed nor kept
# beyond the run. With none the APK is signed with a debug key, which is said, and its file says it
# (`flottille-<version>-debug-signed.apk`): it installs on a phone for a test, and is never released.
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

signed=false
if [ -n "${KEYSTORE_BASE64:-}" ]; then
  umask 077
  keystore=$(mktemp "${RUNNER_TEMP:-/tmp}/release-key.XXXXXX")
  trap 'rm -f "$keystore"' EXIT
  printf '%s' "$KEYSTORE_BASE64" | base64 -d > "$keystore"
  export FLOTTILLE_KEYSTORE=$keystore
  signed=true
  echo "::notice::signed with the release key of the repository's secrets."
else
  unset FLOTTILLE_KEYSTORE
  echo "::notice::no release key in the repository's secrets: the APK is signed with a debug key, to install for a test and never to release."
fi

"$android/gradle.sh" :app:assembleRelease "-Pflottille.version=$version" "-Pflottille.versionCode=$code"
built=$android/app/build/outputs/apk/release/app-release.apk
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

name=flottille-$version.apk
if [ "$signed" = false ]; then name=flottille-$version-debug-signed.apk; fi
cp "$built" "$out/$name"
echo "apk=$out/$name"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "version=$version"
    echo "signed=$signed"
    echo "dir=$out"
  } >> "$GITHUB_OUTPUT"
fi
