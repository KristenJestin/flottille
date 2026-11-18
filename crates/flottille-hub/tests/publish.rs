//! What publishes the hub (`docs/releases.md`, "What CI publishes"): its archive and its sum, the
//! checks CI runs on the static binary and on the image, the examples, and the workflow that wires
//! them. The checks run here against fakes of `ldd`, `file`, `docker` and `curl`; the real ones run
//! on the pull request's CI, which is where the three scenarios of the issue are proved.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn text(path: &str) -> String {
    std::fs::read_to_string(repo(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// A program of the given name that runs `body`, in a folder that goes first on the `PATH`.
fn fake(folder: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = folder.join(name);
    std::fs::write(
        &path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn with_fakes(folder: &Path, script: &str, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        folder.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(repo(script))
        .args(args)
        .env("PATH", path)
        .env("HOME", folder)
        .output()
        .unwrap_or_else(|error| panic!("{script}: {error}"))
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The text of one job of `ci.yml`: from its key to the next line indented by two spaces only
/// (the next job, or the comment that precedes it).
fn job(workflow: &str, name: &str) -> String {
    let start = workflow
        .find(&format!("\n  {name}:\n"))
        .unwrap_or_else(|| panic!("ci.yml has no job {name}"))
        + 1;
    let mut text = String::new();
    for (at, line) in workflow[start..].lines().enumerate() {
        let shallow = line.starts_with("  ") && !line.starts_with("   ");
        if at > 0 && shallow {
            break;
        }
        text.push_str(line);
        text.push('\n');
    }
    text
}

#[test]
fn the_hub_archive_is_summed_beside_flottilles_and_never_taken_for_it() {
    let work = tempfile::tempdir().unwrap();
    let binary = work.path().join("flottille-hub");
    std::fs::write(&binary, "not really a binary").unwrap();
    let out = work.path().join("out");
    let run = |out: &Path| {
        Command::new(repo("tools/package-hub.sh"))
            .args(["0.4.0-beta.3", "aarch64"])
            .arg(&binary)
            .arg(out)
            .output()
            .unwrap()
    };
    let made = run(&out);
    assert!(made.status.success(), "{}", said(&made));

    // `install.sh` and `flottille update` take every `*-x86_64-linux.tar.gz` of a release for
    // Flottille's own archive: the hub's must not end so.
    let archive = out.join("flottille-hub-0.4.0-beta.3-linux-aarch64.tar.gz");
    assert!(archive.is_file(), "{}", said(&made));
    assert!(!archive.to_string_lossy().ends_with("-x86_64-linux.tar.gz"));
    let sum = Command::new("sha256sum")
        .arg("--check")
        .arg("flottille-hub-0.4.0-beta.3-linux-aarch64.tar.gz.sha256")
        .current_dir(&out)
        .output()
        .unwrap();
    assert!(sum.status.success(), "{}", said(&sum));

    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(&archive)
        .output()
        .unwrap();
    let listing = String::from_utf8(listing.stdout).unwrap();
    for file in [
        "flottille-hub-0.4.0-beta.3/flottille-hub",
        "flottille-hub-0.4.0-beta.3/LICENSE",
        "flottille-hub-0.4.0-beta.3/compose.example.yml",
        "flottille-hub-0.4.0-beta.3/flottille-hub.service",
    ] {
        assert!(
            listing.lines().any(|line| line == file),
            "{file}: {listing}"
        );
    }

    // The same files give the same archive.
    let again = work.path().join("again");
    assert!(run(&again).status.success());
    assert_eq!(
        std::fs::read(&archive).unwrap(),
        std::fs::read(again.join(archive.file_name().unwrap())).unwrap()
    );

    let wrong = Command::new(repo("tools/package-hub.sh"))
        .args(["0.4.0", "riscv64"])
        .arg(&binary)
        .arg(&out)
        .output()
        .unwrap();
    assert!(!wrong.status.success(), "an architecture it does not build");
}

#[test]
fn the_static_binary_runs_with_no_shared_library() {
    let work = tempfile::tempdir().unwrap();
    let binary = work.path().join("flottille-hub");
    std::fs::write(&binary, "").unwrap();
    let binary = binary.to_str().unwrap();

    // A static executable: `ldd` says so and so does `file`.
    fake(
        work.path(),
        "ldd",
        "echo '\tnot a dynamic executable' >&2; exit 1",
    );
    fake(
        work.path(),
        "file",
        "echo \"$1: ELF 64-bit LSB executable, x86-64, statically linked, stripped\"",
    );
    let output = with_fakes(work.path(), "tools/check-static.sh", &[binary]);
    assert!(output.status.success(), "{}", said(&output));

    // A static-pie one is static too.
    fake(
        work.path(),
        "file",
        "echo \"$1: ELF 64-bit LSB pie executable, ARM aarch64, static-pie linked\"",
    );
    let output = with_fakes(work.path(), "tools/check-static.sh", &[binary]);
    assert!(output.status.success(), "{}", said(&output));

    // A dynamic one is refused, saying which library it needs.
    fake(
        work.path(),
        "ldd",
        "echo '\tlibc.so.6 => /lib/libc.so.6 (0x1)'",
    );
    fake(
        work.path(),
        "file",
        "echo \"$1: ELF 64-bit LSB pie executable, x86-64, dynamically linked\"",
    );
    let output = with_fakes(work.path(), "tools/check-static.sh", &[binary]);
    assert!(!output.status.success(), "{}", said(&output));
    assert!(said(&output).contains("libc.so.6"), "{}", said(&output));

    // `ldd` that says nothing of it (a foreign architecture) is not enough: `file` decides.
    fake(
        work.path(),
        "ldd",
        "echo '\tnot a dynamic executable' >&2; exit 1",
    );
    let output = with_fakes(work.path(), "tools/check-static.sh", &[binary]);
    assert!(!output.status.success(), "{}", said(&output));
}

/// A `docker` that runs a container the way the check needs it, and a `curl` that answers as
/// the hub does to a request that asks no upgrade (`426 Upgrade Required`).
fn fake_docker(folder: &Path, logs: &str, code: &str) {
    fake(
        folder,
        "docker",
        &format!(
            r#"echo "$*" >> "$HOME/docker.log"
case "$1" in
  run) echo container-id ;;
  logs) printf '%s\n' '{logs}' ;;
  port) echo '127.0.0.1:49153' ;;
  inspect) echo true ;;
  rm | stop) ;;
  *) echo "unexpected: $*" >&2; exit 2 ;;
esac"#
        ),
    );
    fake(folder, "curl", &format!("echo {code}"));
}

#[test]
fn the_image_starts_prints_a_setup_code_and_answers_on_its_port() {
    let work = tempfile::tempdir().unwrap();
    fake_docker(
        work.path(),
        "flottille-hub: state in /var/lib/flottille-hub; the hub's key is SHA256:abc\n\
         flottille-hub: setup code: 482-193-775, valid until a first member joins\n\
         flottille-hub: listening on 0.0.0.0:7448 (plain: the traffic is in clear)",
        "426",
    );
    let output = with_fakes(
        work.path(),
        "tools/check-hub-image.sh",
        &["example.test/flottille-hub:ci"],
    );
    assert!(output.status.success(), "{}", said(&output));
    let calls = std::fs::read_to_string(work.path().join("docker.log")).unwrap();
    // Its port published on loopback only, and the container removed whatever happened.
    assert!(calls.contains("127.0.0.1::7448"), "{calls}");
    assert!(calls.contains("rm -f"), "{calls}");
    // The code is never printed by the check: CI logs are public.
    assert!(!said(&output).contains("482-193-775"), "{}", said(&output));

    // No setup code in its log: refused.
    fake_docker(
        work.path(),
        "flottille-hub: listening on 0.0.0.0:7448 (plain)",
        "426",
    );
    let output = with_fakes(
        work.path(),
        "tools/check-hub-image.sh",
        &["example.test/flottille-hub:ci"],
    );
    assert!(!output.status.success(), "{}", said(&output));

    // Nothing answering on the port: refused.
    fake_docker(
        work.path(),
        "flottille-hub: setup code: 482-193-775, valid until a first member joins",
        "000",
    );
    let output = with_fakes(
        work.path(),
        "tools/check-hub-image.sh",
        &["example.test/flottille-hub:ci"],
    );
    assert!(!output.status.success(), "{}", said(&output));
}

#[test]
fn the_image_holds_the_binary_only_and_runs_as_nobody() {
    let dockerfile = text("crates/flottille-hub/Dockerfile");
    let lines: Vec<&str> = dockerfile
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .collect();
    assert_eq!(lines[0], "FROM scratch", "a static binary needs no base");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("COPY") && l.contains("flottille-hub /flottille-hub"))
    );
    assert!(lines.contains(&"USER 65532:65532"), "{lines:?}");
    assert!(lines.contains(&"EXPOSE 7448"));
    assert!(lines.contains(&"VOLUME /var/lib/flottille-hub"));
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("ENTRYPOINT [\"/flottille-hub\"]"))
    );
    // Reachable from the host's proxy: plain on every address, the state in the volume.
    let command = lines.iter().find(|l| l.starts_with("CMD")).expect("a CMD");
    for part in [
        "--listen",
        "0.0.0.0:7448",
        "--plain",
        "--state",
        "/var/lib/flottille-hub",
    ] {
        assert!(command.contains(part), "{command}");
    }
    assert!(!lines.iter().any(|l| l.starts_with("RUN")), "{lines:?}");
}

#[test]
fn the_compose_example_is_valid() {
    let compose = text("crates/flottille-hub/compose.example.yml");
    // Loopback only: the reverse proxy of the host is what the world reaches.
    assert!(compose.contains("\"127.0.0.1:7448:7448\""), "{compose}");
    // The state in a named volume, kept across the container's life.
    assert!(
        compose.contains("flottille-hub-state:/var/lib/flottille-hub"),
        "{compose}"
    );
    assert!(
        compose.contains("\nvolumes:\n  flottille-hub-state:"),
        "{compose}"
    );
    assert!(compose.contains("restart: unless-stopped"));
    // The image is named by the one who runs it: no account of the repository's is written in.
    assert!(compose.contains("${FLOTTILLE_HUB_IMAGE:?"), "{compose}");
    assert!(
        !compose.contains("ghcr.io/KristenJestin") && !compose.contains("ghcr.io/kristenjestin")
    );
    // Where the setup code is, and the port to open.
    assert!(compose.contains("docker compose logs"), "{compose}");

    let workflow = text(".github/workflows/ci.yml");
    let hub = job(&workflow, "hub");
    assert!(hub.contains("the_compose_example_is_valid"), "{hub}");
    assert!(
        hub.contains("docker compose -f crates/flottille-hub/compose.example.yml config"),
        "{hub}"
    );
}

#[test]
fn the_systemd_unit_example_runs_the_hub_as_a_service_of_its_own() {
    let unit = text("crates/flottille-hub/flottille-hub.service");
    for line in [
        "[Unit]",
        "[Service]",
        "[Install]",
        "WantedBy=multi-user.target",
        "DynamicUser=yes",
        "StateDirectory=flottille-hub",
        "StateDirectoryMode=0700",
        "Restart=on-failure",
        "NoNewPrivileges=yes",
    ] {
        assert!(unit.lines().any(|l| l == line), "{line}: {unit}");
    }
    let exec = unit
        .lines()
        .find(|l| l.starts_with("ExecStart="))
        .expect("an ExecStart");
    // Loopback behind a proxy by default; the state in the unit's own folder.
    assert!(exec.contains("--listen 127.0.0.1:7448"), "{exec}");
    assert!(exec.contains("--state /var/lib/flottille-hub"), "{exec}");
    // Where the first log shows the setup code.
    assert!(unit.contains("journalctl -u flottille-hub"), "{unit}");
}

#[test]
fn the_hub_jobs_run_on_a_pull_request_only_when_the_hub_or_the_release_changes() {
    let workflow = text(".github/workflows/ci.yml");

    // `changes` says whether the hub is concerned, on the paths of the hub, of what it is built
    // from and of the files that publish it.
    let changes = job(&workflow, "changes");
    assert!(
        changes.contains("hub: ${{ steps.hub.outputs.hub }}"),
        "{changes}"
    );
    for path in [
        "crates/flottille-hub/",
        "crates/flottille-link/",
        "Cargo.lock",
        ".github/workflows/ci.yml",
        ".github/workflows/release.yml",
        "tools/package-hub.sh",
        "tools/check-static.sh",
        "tools/check-hub-image.sh",
    ] {
        assert!(changes.contains(path), "{path}: {changes}");
    }

    // A pull request elsewhere runs none of it; a push runs it, since a push publishes.
    let hub = job(&workflow, "hub");
    assert!(hub.contains("needs.changes.outputs.hub == 'true'"), "{hub}");
    assert!(hub.contains("github.event_name == 'pull_request'"), "{hub}");
    assert!(hub.contains("needs.build.result == 'success'"), "{hub}");

    // Both architectures, musl, the second one cross-compiled: no emulation.
    for part in [
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
        "gcc-aarch64-linux-gnu",
        "tools/check-static.sh",
        "tools/check-hub-image.sh",
        "the_static_binary_runs_with_no_shared_library",
        "the_image_starts_prints_a_setup_code_and_answers_on_its_port",
    ] {
        assert!(hub.contains(part), "{part}: {hub}");
    }
    assert!(!hub.contains("qemu"), "{hub}");
    // It holds no right to write, and uploads on a push only.
    assert!(hub.contains("contents: read"), "{hub}");
    assert!(!hub.contains("write"), "{hub}");
}

#[test]
fn the_release_carries_the_hub_archives_and_its_image_is_pushed_by_a_job_that_runs_no_code_of_the_branch()
 {
    let workflow = text(".github/workflows/ci.yml");

    // The archives ride the release's own `gh release create`, an immutable release taking none
    // after; a release is made whole or not at all.
    let publish = job(&workflow, "publish");
    assert!(
        publish.contains("needs: [verified-on-dev, verify, build, hub]"),
        "{publish}"
    );
    assert!(
        publish.contains("needs.hub.result == 'success'"),
        "{publish}"
    );
    assert!(publish.contains("name: hub-archives"), "{publish}");
    assert_eq!(
        publish.matches("hub/assets/*").count(),
        publish.matches("release/assets/*").count(),
        "every release made carries them: {publish}"
    );
    assert!(!publish.contains("packages: write"), "{publish}");

    // The image: a job of its own, after the release, that loads what the hub job saved (no
    // Dockerfile, no build, nothing of the branch runs) and pushes it under the repository's own
    // path, lowercased as the registry wants.
    let image = job(&workflow, "hub-image");
    assert!(image.contains("needs: [build, hub, publish]"), "{image}");
    assert!(image.contains("packages: write"), "{image}");
    assert!(
        image.contains("ghcr.io/${GITHUB_REPOSITORY,,}-hub"),
        "{image}"
    );
    assert!(image.contains("docker load"), "{image}");
    assert!(
        !image.contains("docker build") && !image.contains("actions/checkout"),
        "{image}"
    );
    assert!(!image.contains("ghcr.io/KristenJestin") && !image.contains("ghcr.io/kristenjestin"));
    // The release's own tag, and `latest` when its release is the latest; no alpha is pushed.
    assert!(image.contains("isLatest"), "{image}");
    assert!(
        image.contains("\"$CHANNEL\" != alpha")
            || image.contains("needs.build.outputs.channel != 'alpha'"),
        "{image}"
    );
    // One image for both architectures.
    assert!(image.contains("imagetools create"), "{image}");
    assert!(
        image.contains("linux/amd64") || image.contains("-amd64"),
        "{image}"
    );
    assert!(image.contains("-arm64"), "{image}");
}

#[test]
fn the_docs_say_what_is_published_and_how_to_run_it() {
    let releases = text("docs/releases.md");
    for part in [
        "flottille-hub-<version>-linux-x86_64.tar.gz",
        "flottille-hub-<version>-linux-aarch64.tar.gz",
        "ghcr.io/<owner>/<repository>-hub",
        "crates/flottille-hub/compose.example.yml",
        "crates/flottille-hub/flottille-hub.service",
        "docker compose logs",
        "journalctl -u flottille-hub",
        "7448",
    ] {
        assert!(releases.contains(part), "{part}");
    }
    let remote = text("docs/remote.md");
    let hub =
        &remote[remote.find("## The hub").unwrap()..remote.find("### The first member").unwrap()];
    assert!(hub.contains("docs/releases.md"), "linked from \"The hub\"");
    assert!(hub.contains("compose.example.yml"));
}
