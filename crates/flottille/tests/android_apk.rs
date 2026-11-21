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

    // The release key is in the repository's secrets, never in a file of it: the signing job is
    // `the_release_key_is_reachable_by_the_signing_job_of_a_push_only`'s.
    let gradle = read("apps/android/app/build.gradle.kts");
    assert!(gradle.contains("flottille.version"), "{gradle}");
    assert!(script.contains("::notice::"), "the build says what it made");
    assert!(script.contains("debug"), "{script}");

    // A release attaches its APK, the signed one.
    let publish = job(".github/workflows/ci.yml", "publish");
    let creates: Vec<&str> = publish
        .lines()
        .filter(|line| line.trim_start().starts_with("gh release create"))
        .collect();
    assert_eq!(creates.len(), 3, "{publish}");
    assert!(
        creates.iter().all(|line| line.contains("\"${apk[@]}\"")),
        "every way of publishing carries the APK: {creates:?}"
    );
    let releases = read("docs/releases.md");
    assert!(
        releases.contains("versionName") && releases.contains("versionCode"),
        "docs/releases.md says how the app is versioned"
    );
}

/// The text of the step of `job` named `step`: from its `- name:` line to the next step's.
fn step(job: &str, step: &str) -> String {
    let lines: Vec<&str> = job.lines().collect();
    let key = format!("      - name: {step}");
    let start = lines
        .iter()
        .position(|line| *line == key)
        .unwrap_or_else(|| panic!("no step {step}: {job}"));
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("      - "))
        .map_or(lines.len(), |at| start + 1 + at);
    lines[start..end].join("\n")
}

#[test]
fn the_release_key_is_reachable_by_the_signing_job_of_a_push_only() {
    let workflow = read(".github/workflows/ci.yml");
    let build = job(".github/workflows/ci.yml", "android");
    // No secret reaches the job that builds the branch's code, nor the scripts it runs.
    assert!(
        !build.contains("secrets."),
        "the build job sees no secret: {build}"
    );
    assert!(!build.contains("ANDROID_KEY"), "{build}");
    for file in ["apps/android/apk.sh", "apps/android/app/build.gradle.kts"] {
        let text = read(file);
        for name in [
            "KEYSTORE_BASE64",
            "KEYSTORE_PASSWORD",
            "KEY_PASSWORD",
            "KEY_ALIAS",
            "signingConfig",
        ] {
            assert!(!text.contains(name), "{file} knows {name}");
        }
    }
    // The APK it builds is unsigned, and is handed over as an artefact of its own.
    assert!(build.contains("android-unsigned"), "{build}");

    // The signing is a job of its own, behind an environment that only the protected branches
    // reach, on a push (never a pull request's run, nor a run started by hand).
    let sign = job(".github/workflows/ci.yml", "android-sign");
    assert!(sign.contains("environment: android-release"), "{sign}");
    assert!(sign.contains("github.event_name == 'push'"), "{sign}");
    assert!(!sign.contains("pull_request"), "{sign}");
    // It runs no third-party action, and no code of the repository: no checkout either.
    for line in sign.lines().filter(|line| line.contains("uses:")) {
        assert!(
            line.contains("uses: actions/download-artifact@")
                || line.contains("uses: actions/upload-artifact@"),
            "a third-party action in the job that holds the key: {line}"
        );
    }
    assert!(!sign.contains("actions/checkout"), "{sign}");
    assert!(sign.contains("name: android-unsigned"), "{sign}");
    assert!(sign.contains("name: android-signed"), "{sign}");
    // Only the signing step holds the secrets, as environment variables, never as arguments.
    let signing = step(&sign, "Sign the APK");
    let before = &sign[..sign.find(&signing).unwrap()];
    assert!(
        !before.contains("secrets."),
        "no secret before the signing step: {before}"
    );
    for name in [
        "ANDROID_KEYSTORE_BASE64",
        "ANDROID_KEYSTORE_PASSWORD",
        "ANDROID_KEY_ALIAS",
        "ANDROID_KEY_PASSWORD",
    ] {
        assert!(
            signing.contains(&format!("secrets.{name}")),
            "{name}: {signing}"
        );
        assert_eq!(
            sign.matches(&format!("secrets.{name}")).count(),
            1,
            "{name}"
        );
    }
    assert!(signing.contains("apksigner"), "{signing}");
    assert!(signing.contains("--ks-pass env:"), "{signing}");
    assert!(signing.contains("--key-pass env:"), "{signing}");
    assert!(!signing.contains("pass:"), "{signing}");
    // The secrets are the environment's: the workflow says where the maintainer keeps them.
    assert_eq!(
        workflow.matches("secrets.ANDROID_").count(),
        4,
        "four secrets, once each"
    );
}

#[test]
fn a_release_attaches_the_signed_artefact_of_a_push_run_only() {
    let publish = job(".github/workflows/ci.yml", "publish");
    assert!(publish.contains("android-sign"), "{publish}");
    assert!(publish.contains("needs.android-sign.result"), "{publish}");
    assert!(
        !publish.contains("android-apk"),
        "no APK of a build job: {publish}"
    );
    let download = step(&publish, "The signed APK");
    assert!(
        download.contains("if: github.event_name == 'push'"),
        "{download}"
    );
    assert!(download.contains("name: android-signed"), "{download}");
    // The name of a file says nothing: only that artefact is attached.
    assert!(!publish.contains("debug-signed"), "{publish}");
}

#[test]
fn the_gradle_distribution_is_checked_against_its_pinned_sum() {
    let script = read("apps/android/gradle.sh");
    // Gradle publishes the sum of gradle-8.11.1-bin.zip; it is compared before anything is unzipped.
    assert!(
        script.contains("f397b287023acdba1e9f6fc5ea72d22dd63669d59ed4a289a29b1a76eee151c6"),
        "{script}"
    );
    let sum = script.find("sha256sum").expect("the script checks a sum");
    let unzip = script.find("unzip -q").expect("the script unzips");
    assert!(sum < unzip, "the sum is checked before unzipping: {script}");
    assert!(script.contains("exit 1"), "a mismatch is refused: {script}");
}

#[test]
fn the_app_is_never_backed_up() {
    let manifest = read("apps/android/app/src/main/AndroidManifest.xml");
    assert!(
        manifest.contains("android:allowBackup=\"false\""),
        "{manifest}"
    );
    assert!(
        manifest.contains("android:dataExtractionRules=\"@xml/data_extraction_rules\""),
        "{manifest}"
    );
    assert!(
        manifest.contains("android:fullBackupContent=\"@xml/backup_rules\""),
        "{manifest}"
    );
    // Every domain is excluded, from the cloud backup and from a device-to-device transfer.
    let rules = read("apps/android/app/src/main/res/xml/data_extraction_rules.xml");
    for section in ["cloud-backup", "device-transfer"] {
        assert!(rules.contains(&format!("<{section}>")), "{rules}");
    }
    let domains = [
        "root",
        "file",
        "database",
        "sharedpref",
        "external",
        "device_root",
        "device_file",
        "device_database",
        "device_sharedpref",
    ];
    for domain in domains {
        assert!(
            rules.contains(&format!("<exclude domain=\"{domain}\" path=\".\" />")),
            "{domain}: {rules}"
        );
    }
    let old = read("apps/android/app/src/main/res/xml/backup_rules.xml");
    for domain in ["root", "file", "database", "sharedpref", "external"] {
        assert!(
            old.contains(&format!("<exclude domain=\"{domain}\" path=\".\" />")),
            "{domain}: {old}"
        );
    }
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
