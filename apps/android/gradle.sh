#!/bin/bash
# Gradle for the Android app, without installing anything: the distribution it needs is downloaded
# once into a cache folder of the user's (what a Gradle wrapper does), and run with a JDK 17 or 21
# (Android Studio's own on a desktop, JAVA_HOME_17_X64 on a GitHub runner).
# Usage: apps/android/gradle.sh <tasks…>
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
version=8.11.1
# The sum Gradle publishes for gradle-$version-bin.zip (services.gradle.org/distributions): a
# download that does not match is refused, never unzipped.
sha256=f397b287023acdba1e9f6fc5ea72d22dd63669d59ed4a289a29b1a76eee151c6
cache=${XDG_CACHE_HOME:-$HOME/.cache}/flottille-android
gradle=$cache/gradle-$version/bin/gradle
if [ ! -x "$gradle" ]; then
  mkdir -p "$cache"
  curl -fsSL -o "$cache/gradle.zip" "https://services.gradle.org/distributions/gradle-$version-bin.zip"
  if ! echo "$sha256  $cache/gradle.zip" | sha256sum -c --status -; then
    rm -f "$cache/gradle.zip"
    echo "gradle-$version-bin.zip does not match its pinned sha256: refused." >&2
    exit 1
  fi
  (cd "$cache" && unzip -q -o gradle.zip && rm gradle.zip)
fi
for candidate in "${JAVA_HOME_17_X64:-}" "${JAVA_HOME_21_X64:-}" /opt/android-studio/jbr \
  "$HOME/android-studio/jbr" "${JAVA_HOME:-}"; do
  if [ -n "$candidate" ] && [ -x "$candidate/bin/java" ]; then
    export JAVA_HOME=$candidate
    break
  fi
done
if [ -z "${ANDROID_HOME:-}" ] && [ -d "$HOME/Android/Sdk" ]; then
  export ANDROID_HOME=$HOME/Android/Sdk
fi
cd "$here"
exec "$gradle" --no-daemon --console=plain "$@"
