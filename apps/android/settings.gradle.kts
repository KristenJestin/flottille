// The Android app (`docs/remote.md`, "The Android app"). `core` wraps the mobile core
// (`crates/flottille-mobile`): its library built for the phone and the emulator, and its Kotlin
// bindings, which the screens never see. `app` is the screens.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "flottille-android"
include(":core")
include(":app")
