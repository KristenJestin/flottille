// The app (`docs/remote.md`, "The Android app"): Kotlin and Jetpack Compose, over the `core`
// module's `Phone`, never the generated bindings. Its version is the repository's
// (`tools/version.sh`, `docs/releases.md`, "The version"), given by CI as -Pflottille.version and
// -Pflottille.versionCode (`apps/android/version-code.sh`); a build by hand says it is none.
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

val appVersion: String = providers.gradleProperty("flottille.version").getOrElse("0.0.0-local")
val appVersionCode: Int = providers.gradleProperty("flottille.versionCode").map(String::toInt).getOrElse(1)

android {
    namespace = "org.flottille.app"
    compileSdk = 35
    defaultConfig {
        applicationId = "org.flottille.app"
        minSdk = 31
        targetSdk = 35
        versionName = appVersion
        versionCode = appVersionCode
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            // Unsigned: the build holds no key. `apk.sh` debug-signs a copy to try on a phone, and
            // CI's signing job (its own environment) signs the release's.
        }
    }
    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    testOptions { unitTests { isIncludeAndroidResources = true } }
}

// Robolectric runs the app's screens on a real Android's classes, an `android-all` jar per SDK of
// the tests (`@Config(sdk = [34])`: Android 14). It fetches that jar from Maven while the tests run
// unless told otherwise: a network hiccup then failed a build. The jar is resolved here, as a
// Gradle dependency (cached, checked by Gradle like any other), copied to a folder by a task that
// every test task waits for, and Robolectric is told to read that folder and fetch nothing. The
// version is the one Robolectric 4.14.1 names for SDK 34 (its `DefaultSdkProvider`); a test on
// another SDK adds its jar here, and fails offline, never by fetching, until it does.
val robolectricJars: Configuration by configurations.creating
val robolectricJarsDir: Provider<Directory> = layout.buildDirectory.dir("robolectric-jars")
val fetchRobolectricJars by tasks.registering(Sync::class) {
    from(robolectricJars)
    into(robolectricJarsDir)
}
tasks.withType<Test>().configureEach {
    dependsOn(fetchRobolectricJars)
    systemProperty("robolectric.offline", "true")
    systemProperty("robolectric.dependency.dir", robolectricJarsDir.get().asFile.absolutePath)
}

dependencies {
    implementation(project(":core"))
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    val compose = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(compose)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    // The QR reader: the camera's frames, decoded by ZXing (no Google service on the phone).
    implementation("androidx.camera:camera-core:1.4.1")
    implementation("androidx.camera:camera-camera2:1.4.1")
    implementation("androidx.camera:camera-lifecycle:1.4.1")
    implementation("androidx.camera:camera-view:1.4.1")
    implementation("com.google.zxing:core:3.5.3")

    testImplementation(compose)
    testImplementation("androidx.compose.ui:ui-test-junit4")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    robolectricJars("org.robolectric:android-all-instrumented:14-robolectric-10818077-i7")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.9.0")
    debugImplementation("androidx.compose.ui:ui-test-manifest")

    androidTestImplementation(compose)
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
}
