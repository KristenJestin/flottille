// The Kotlin module of the mobile core (`docs/remote.md`, "The Android app"): the library
// `crates/flottille-mobile` built with `cargo ndk` for the phone (arm64-v8a) and the emulator
// (x86_64), aligned on 16 KB, its symbols kept apart; the Kotlin bindings UniFFI generates from it;
// and the wrapper the screens use (`FlottilleCore`), never the bindings themselves.
plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

val repository: File = rootProject.projectDir.resolve("../..").canonicalFile
val cargoTarget: File = System.getenv("CARGO_TARGET_DIR")?.let(::File) ?: repository.resolve("target")
val ndkHome: String = System.getenv("ANDROID_NDK_HOME") ?: System.getenv("ANDROID_NDK_LATEST_HOME") ?: ""
/** Each ABI the app ships, and Rust's target for it. */
val abis = mapOf("arm64-v8a" to "aarch64-linux-android", "x86_64" to "x86_64-linux-android")
val library = "libflottille_mobile.so"
val jniLibs: File = layout.buildDirectory.dir("rust/jniLibs").get().asFile
val symbols: File = layout.buildDirectory.dir("rust/symbols").get().asFile
val bindingsOut: File = layout.buildDirectory.dir("generated/uniffi").get().asFile

android {
    namespace = "org.flottille.core"
    compileSdk = 35
    defaultConfig {
        // Expedited work without a foreground service's notification (#823's spike).
        minSdk = 31
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    sourceSets["main"].java.srcDir(bindingsOut)
    sourceSets["main"].jniLibs.srcDir(jniLibs)
    packaging {
        // Stripped here already (`rustLibraries`): AGP leaves them as they are.
        jniLibs.keepDebugSymbols += "**/$library"
    }
}

/** Runs `command` in the repository, its output shown; fails the build when it fails. */
fun run(command: List<String>, environment: Map<String, String> = emptyMap()) {
    val process = ProcessBuilder(command).directory(repository).inheritIO()
    process.environment().putAll(environment)
    val code = process.start().waitFor()
    check(code == 0) { "${command.joinToString(" ")} failed ($code)" }
}

// The library for each ABI, every LOAD segment aligned on 16 KB (the pages of Android 15), whatever
// the NDK's default. A release build: the workspace's profile keeps the line tables of its crates.
val rustAndroid by tasks.registering {
    doLast {
        val flags = "-C link-arg=-Wl,-z,max-page-size=16384"
        run(
            listOf("cargo", "ndk", "--platform", "31") +
                abis.keys.flatMap { listOf("-t", it) } +
                listOf("build", "--release", "--locked", "-p", "flottille-mobile", "--lib"),
            mapOf("ANDROID_NDK_HOME" to ndkHome) +
                abis.values.associate {
                    "CARGO_TARGET_${it.uppercase().replace('-', '_')}_RUSTFLAGS" to flags
                },
        )
    }
}

// The symbols kept apart (`build/rust/symbols/<abi>`, for a crash's stack), the libraries shipped
// stripped of them (`build/rust/jniLibs/<abi>`).
val rustLibraries by tasks.registering {
    dependsOn(rustAndroid)
    outputs.dirs(jniLibs, symbols)
    doLast {
        val strip = "$ndkHome/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip"
        abis.forEach { (abi, triple) ->
            val built = cargoTarget.resolve("$triple/release/$library")
            val kept = symbols.resolve(abi).apply { mkdirs() }.resolve(library)
            built.copyTo(kept, overwrite = true)
            val shipped = jniLibs.resolve(abi).apply { mkdirs() }.resolve(library)
            run(listOf(strip, "--strip-all", "-o", shipped.path, built.path))
        }
    }
}

// The library for this machine: what the bindings are generated from, and what the unit tests load.
val rustHost by tasks.registering {
    doLast { run(listOf("cargo", "build", "--locked", "-p", "flottille-mobile", "--lib")) }
}

val bindings by tasks.registering {
    dependsOn(rustHost)
    outputs.dir(bindingsOut)
    doLast {
        run(
            listOf(
                "cargo", "run", "--locked", "-p", "flottille-mobile", "--features", "bindgen",
                "--bin", "uniffi-bindgen", "--", "generate", "--language", "kotlin", "--no-format",
                "--out-dir", bindingsOut.path, cargoTarget.resolve("debug/$library").path,
            ),
        )
    }
}

tasks.named("preBuild") { dependsOn(bindings) }
tasks.matching { it.name.startsWith("merge") && it.name.endsWith("JniLibFolders") }
    .configureEach { dependsOn(rustLibraries) }

tasks.withType<Test>().configureEach {
    dependsOn(rustHost, rustLibraries)
    systemProperty("jna.library.path", cargoTarget.resolve("debug").path)
    systemProperty("flottille.jniLibs", jniLibs.path)
}

dependencies {
    // The bindings reach the library through JNA; on the phone, its Android archive.
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    // The Phone interface hands out Flows: the app sees them too.
    api("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    // The unit tests run on this machine's JVM: JNA's own jar, with its native part for it.
    testImplementation("net.java.dev.jna:jna:5.17.0")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.9.0")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
}
