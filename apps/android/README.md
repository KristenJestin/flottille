# Android app

The Android app of `v0.4 - Remote`: a client of the user's own machines, over the link and the
hub (`docs/remote.md`). A Gradle project that uses the mobile core (`crates/flottille-mobile`,
a UniFFI library) for everything that talks to a daemon.

- `core/`: the Kotlin module of the mobile core. It builds the library with `cargo ndk` for
  `arm64-v8a` and `x86_64` (16 KB pages, symbols kept apart in `core/build/rust/symbols`), generates
  the Kotlin bindings from it, and wraps them (`FlottilleCore`, `KeystoreSealer`): the screens use
  the wrapper, never the bindings.
- `gradle.sh <tasks…>`: Gradle without a wrapper (downloaded once into `~/.cache/flottille-android`),
  with a JDK 17 or 21, the Android SDK and NDK (`ANDROID_HOME`, `ANDROID_NDK_HOME`), Rust's targets
  `aarch64-linux-android` and `x86_64-linux-android`, and `cargo-ndk`.
  `apps/android/gradle.sh :core:testDebugUnitTest` runs the unit tests.
- `ci.sh`: CI's `android` job, from the repository's root on a GitHub runner: the unit tests (both
  modules), then the instrumentation tests on an emulator.

- `app/`: the screens (Jetpack Compose): joining, the sessions in one list, the machines, Settings.
  They see `Phone` (the module's own types), never the bindings; their tests play it with a fake
  (`app/src/test`, Robolectric). `ui/Tokens.kt` holds the kit's colours, durations and easing.
- the icon (#1242): `app/src/main/res` holds the adaptive launcher icon and the notification's mark
  (vector drawables of `assets/brand/mark.svg`) and the legacy PNGs, which `icons.sh` renders from
  `assets/brand/icon.svg`.
- `version-code.sh`, `apk.sh`: the APK as CI builds it, with the repository's version
  (`docs/releases.md`, "The version"). Never by hand.

- the session page (`SessionPage.kt`, `Cards.kt`, `Pictures.kt`): the conversation live, the composer
  with pictures (gallery, camera through a `FileProvider`, the share sheet), the ask card, the
  requests, the mode, Stop and the tree read only; `SessionKey` is the one entry point, by machine
  and session id (a notification's intent carries `org.flottille.app.MACHINE` and `AGENT`).

- a run's page (`Runs.kt`, `TunnelBrowser.kt`, `TunnelService.kt`, #831): the session page's runs and
  the links an agent showed open through a tunnel (`Phone.openAddress`), in an in-app web view or, with
  **Open in browser**, in the user's browser with the tunnel kept by a foreground service whose
  notification has Close. `FlottilleApplication.phone` can be replaced, which the instrumentation
  tests do to play the core (`PlayedPhone`) against a fake server.

- Needs you (`NeedsYouScreen.kt`, #1443), where the app opens: what waits for the user on every
  machine, on the session page's cards, answered in place. The notifications come after #826.
