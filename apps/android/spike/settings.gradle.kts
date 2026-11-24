// The Android push spike (#823): a throwaway proof, never shipped. `app` is the phone's side,
// `distributor` a stub UnifiedPush distributor whose instrumentation tests drive the proof.
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
rootProject.name = "flottille-push-spike"
include(":app", ":distributor")
