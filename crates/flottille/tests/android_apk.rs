//! The Android app's APK (#825, `docs/releases.md`, "The version"): built by CI on every pull
//! request that touches the app, with the version `tools/version.sh` gives as its `versionName`
//! and a `versionCode` that always rises, signed with a key kept in the repository's secrets when
//! there is one and debug-signed (and said so) when there is none, uploaded as an artefact, and
//! attached to the release of its version once signed. What only a real run proves (that Gradle
//! builds it, that a phone installs it) is the `android` job's.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tool(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(tool(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// The text of one job of the workflow: from its key to the next job's.
fn job(workflow: &str, name: &str) -> String {
    let text = read(workflow);
    let lines: Vec<&str> = text.lines().collect();
    let key = format!("  {name}:");
    let start = lines
        .iter()
        .position(|line| *line == key)
        .unwrap_or_else(|| panic!("{workflow} has no job {name}"));
    let end = lines[start + 1..]
        .iter()
        .position(|line| {
            line.len() > 2
                && line.starts_with("  ")
                && !line.starts_with("   ")
                && !line.trim_start().starts_with('#')
                && line.trim_end().ends_with(':')
        })
        .map_or(lines.len(), |at| start + 1 + at);
    lines[start..end].join("\n")
}

/// The `versionCode` the script derives from a version.
fn code(version: &str) -> u64 {
    let output = Command::new("bash")
        .arg(tool("apps/android/version-code.sh"))
        .arg(version)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{version}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{version}: not a number"))
}

#[test]
fn the_apk_is_built_by_ci_with_the_version() {
    let android = job(".github/workflows/ci.yml", "android");
    // The version is the one `tools/version.sh` gives, whatever the channel, and Gradle is told it
    // as the app's versionName and its derived versionCode.
    let script = read("apps/android/apk.sh");
    assert!(script.contains("tools/version.sh"), "{script}");
    for channel in ["stable", "beta", "alpha"] {
        assert!(script.contains(channel), "the {channel} channel: {script}");
    }
    assert!(script.contains("-Pflottille.version="), "{script}");
    assert!(script.contains("-Pflottille.versionCode="), "{script}");
    assert!(script.contains("version-code.sh"), "{script}");
    assert!(script.contains(":app:assembleRelease"), "{script}");
    // The build says which version it is by reading it back out of the APK it made.
    assert!(script.contains("dump badging"), "{script}");
    assert!(android.contains("apps/android/apk.sh"), "{android}");
    // On every pull request that touches the app, the APK is an artefact to download.
    assert!(android.contains("actions/upload-artifact"), "{android}");
    assert!(android.contains("android-apk"), "{android}");
    assert!(
        android.contains("fetch-depth: 0"),
        "the version counts commits: {android}"
    );
    let build = read(".github/workflows/ci.yml");
    assert!(
        build.contains("apps/android/* | crates/flottille-mobile/*"),
        "a pull request that touches the app runs the job"
    );

    // The release key is in the repository's secrets, never in a file of it.
    assert!(
        android.contains("secrets.ANDROID_KEYSTORE_BASE64"),
        "{android}"
    );
    let gradle = read("apps/android/app/build.gradle.kts");
    assert!(gradle.contains("flottille.version"), "{gradle}");
    assert!(
        gradle.contains("System.getenv(\"FLOTTILLE_KEYSTORE\")"),
        "{gradle}"
    );
    assert!(
        !gradle.contains("storePassword = \""),
        "no password in a file of the repository"
    );
    assert!(
        script.contains("::notice::"),
        "the build says how it is signed"
    );
    assert!(script.contains("debug"), "{script}");

    // A release attaches its APK, signed ones only.
    let publish = job(".github/workflows/ci.yml", "publish");
    assert!(publish.contains("android-apk"), "{publish}");
    assert!(publish.contains("debug-signed"), "{publish}");
    assert_eq!(
        publish.matches("gh release create").count(),
        publish
            .lines()
            .filter(|line| line.contains("gh release create") && line.contains("\"${apk[@]}\""))
            .count(),
        "every way of publishing carries the APK: {publish}"
    );
    let releases = read("docs/releases.md");
    assert!(
        releases.contains("versionName") && releases.contains("versionCode"),
        "docs/releases.md says how the app is versioned"
    );
}

#[test]
fn the_version_code_always_rises() {
    // Alpha, then beta, then release candidate, then the release of one version; then the next.
    let versions = [
        "0.4.1-alpha.feat-android.3",
        "0.4.1-alpha.feat-android.12",
        "0.4.1-beta.2",
        "0.4.1-beta.30",
        "0.4.1-rc.1",
        "0.4.1",
        "0.4.2-alpha.fix-x.1",
        "0.4.2-beta.1",
        "0.4.2",
        "0.5.0-beta.1",
        "0.5.0",
        "0.10.0",
        "1.0.0-beta.4",
        "1.0.0",
        "1.2.3",
    ];
    let codes: Vec<u64> = versions.iter().map(|version| code(version)).collect();
    for pair in codes.windows(2).zip(versions.windows(2)) {
        assert!(
            pair.0[0] < pair.0[1],
            "{} ({}) must be below {} ({})",
            pair.1[0],
            pair.0[0],
            pair.1[1],
            pair.0[1]
        );
    }
    // Android takes a code up to 2 100 000 000.
    assert!(codes.iter().all(|code| *code < 2_100_000_000), "{codes:?}");
}

#[test]
fn a_version_the_code_cannot_hold_fails_the_build() {
    for version in ["0.4.100", "0.100.0", "not-a-version", "0.4"] {
        let output = Command::new("bash")
            .arg(tool("apps/android/version-code.sh"))
            .arg(version)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{version} is refused");
        assert!(output.stdout.is_empty(), "{version}: no code is printed");
    }
}
