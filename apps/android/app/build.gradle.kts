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
/** The release key's file, written by CI from the repository's secrets; none on a computer. */
val keystore: String? = System.getenv("FLOTTILLE_KEYSTORE")?.takeIf { it.isNotEmpty() }

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
    signingConfigs {
        if (keystore != null) {
            create("release") {
                storeFile = file(keystore)
                storePassword = System.getenv("FLOTTILLE_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("FLOTTILLE_KEY_ALIAS")
                keyPassword = System.getenv("FLOTTILLE_KEY_PASSWORD")
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            // Signed with the release key when CI has it, debug-signed otherwise: CI says which.
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
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

dependencies {
    implementation(project(":core"))
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    val compose = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(compose)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
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
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.9.0")
    debugImplementation("androidx.compose.ui:ui-test-manifest")

    androidTestImplementation(compose)
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
}
