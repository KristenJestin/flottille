plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "org.flottille.spike"
    compileSdk = 35
    defaultConfig {
        applicationId = "org.flottille.spike"
        // Expedited work without a foreground service's notification, and BroadcastOptions.
        minSdk = 31
        targetSdk = 35
        versionCode = 1
        versionName = "0.0.0-spike"
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    sourceSets["main"].assets.srcDir(layout.buildDirectory.dir("words"))
}

// The four words' list, the link's own (BIP-39's English list), never a copy kept here.
val words by tasks.registering(Copy::class) {
    from(rootProject.file("../../../crates/flottille-link/src/words.txt"))
    into(layout.buildDirectory.dir("words"))
}
tasks.named("preBuild") { dependsOn(words) }

dependencies {
    // Ed25519 in pure Java: no NDK. The mobile core (#824) signs in Rust instead.
    implementation("org.bouncycastle:bcprov-jdk18on:1.79")
    implementation("androidx.work:work-runtime-ktx:2.10.0")
}
