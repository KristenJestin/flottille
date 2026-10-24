//! The scenarios of the daemon's skeleton, against the real binary, with temporary XDG folders.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

/// A throwaway user: its own home and XDG folders, and the daemon it starts, stopped on drop.
struct User {
    dir: TempDir,
}

impl User {
    fn new() -> User {
        let dir = tempfile::Builder::new()
            .prefix("flottille-test")
            .tempdir()
            .unwrap();
        for sub in ["home", "runtime", "state", "config", "data"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(dir.path().join(sub))
                .unwrap();
        }
        User { dir }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_flottille"));
        command
            .args(args)
            .env("HOME", self.path("home"))
            .env("XDG_RUNTIME_DIR", self.path("runtime"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn status(&self) -> Value {
        let output = self.run(&["status", "--json"]);
        assert!(
            output.status.success(),
            "status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn watch(&self, args: &[&str]) -> Watch {
        let mut child = self
            .command(&[&["watch"], args].concat())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Watch { child, lines }
    }
}

impl Drop for User {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

/// A running `flottille watch`, its output read line by line.
struct Watch {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Watch {
    fn next_event(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(10))
            .expect("no event within 10 s");
        serde_json::from_str(&line).unwrap()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn mode(path: PathBuf) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_first_command_starts_a_daemon() {
    let user = User::new();
    assert!(!user.path("runtime/flottille/daemon.sock").exists());

    user.status();

    assert!(user.path("runtime/flottille/daemon.sock").exists());
    assert!(user.path("state/flottille/flottille.db").exists());
}

#[test]
fn the_folders_are_created_for_the_user_only() {
    let user = User::new();
    user.status();

    assert_eq!(mode(user.path("runtime/flottille")), 0o700);
    assert_eq!(mode(user.path("state/flottille")), 0o700);
}

#[test]
fn a_folder_open_to_others_is_refused() {
    let user = User::new();
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(user.path("runtime/flottille"))
        .unwrap();
    std::fs::set_permissions(
        user.path("runtime/flottille"),
        PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let output = user.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("other users"),
        "unexpected refusal: {stderr}"
    );
}

#[test]
fn a_second_daemon_refuses_to_start() {
    let user = User::new();
    let pid = user.status()["pid"].as_u64().unwrap();

    let output = user.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already running"),
        "unexpected refusal: {stderr}"
    );
    assert!(
        stderr.contains(&pid.to_string()),
        "the refusal names the running daemon: {stderr}"
    );
    assert_eq!(
        user.status()["pid"].as_u64().unwrap(),
        pid,
        "the first daemon still answers"
    );
}

#[test]
fn status_json_answers() {
    let user = User::new();

    let status = user.status();

    assert_eq!(
        status["version"].as_str().unwrap(),
        version_of_the_binary(&user)
    );
    assert!(status["uptime_s"].is_u64());
    assert!(status["pid"].is_u64());
    assert_eq!(status["agents"]["running"], 0);
    assert_eq!(status["agents"]["waiting_for_user"], 0);
    assert_eq!(status["agents"]["failed"], 0);
}

#[test]
fn status_without_json_is_read_by_a_person() {
    let user = User::new();

    let output = user.run(&["status"]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 running, 0 waiting for you, 0 failed"),
        "unexpected status: {stdout}"
    );
}

#[test]
fn version_prints_the_full_version() {
    let user = User::new();

    let version = version_of_the_binary(&user);

    assert!(
        version.starts_with(env!("CARGO_PKG_VERSION")),
        "unexpected version: {version}"
    );
}

fn version_of_the_binary(user: &User) -> String {
    let output = user.run(&["--version"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout.trim().strip_prefix("flottille ").unwrap().to_owned()
}

#[test]
fn watch_prints_an_event_emitted_by_a_request_and_resumes_after_it() {
    let user = User::new();
    let watch = user.watch(&["--after", "0"]);
    let started = watch.next_event();
    assert_eq!(started["name"], "daemon.started");
    let started_seq = started["seq"].as_u64().unwrap();

    // `daemon stop` is a request; the daemon tells it before it goes.
    assert!(user.run(&["daemon", "stop"]).status.success());
    let stopping = watch.next_event();
    assert_eq!(stopping["name"], "daemon.stopping");
    assert_eq!(stopping["data"]["reason"], "request");
    assert!(stopping["seq"].as_u64().unwrap() > started_seq);
    drop(watch);

    // Resuming after the first event gives what came after it, from the store, then what is new.
    let resumed = user.watch(&["--after", &started_seq.to_string()]);
    assert_eq!(resumed.next_event(), stopping);
    let restarted = resumed.next_event();
    assert_eq!(restarted["name"], "daemon.started");
    assert!(restarted["seq"].as_u64().unwrap() > stopping["seq"].as_u64().unwrap());
}

#[test]
fn watch_without_after_prints_only_what_is_new() {
    let user = User::new();
    user.status();
    let watch = user.watch(&[]);
    // The subscription is answered before anything is printed; give it a moment to be in place.
    std::thread::sleep(Duration::from_secs(1));

    assert!(user.run(&["daemon", "stop"]).status.success());

    assert_eq!(watch.next_event()["name"], "daemon.stopping");
}

#[test]
fn a_stopped_daemon_is_restarted_by_the_next_command() {
    let user = User::new();
    let first = user.status()["pid"].as_u64().unwrap();

    let stop = user.run(&["daemon", "stop"]);
    assert!(stop.status.success());
    assert!(!user.path("runtime/flottille/daemon.sock").exists());

    let second = user.status()["pid"].as_u64().unwrap();
    assert_ne!(first, second);
}

#[test]
fn stopping_when_no_daemon_runs_starts_none() {
    let user = User::new();

    let output = user.run(&["daemon", "stop"]);

    assert!(output.status.success());
    assert!(!user.path("runtime/flottille/daemon.sock").exists());
}
