//! Tests leave no process behind (#809): what a test starts (a fake MCP server over HTTP, the
//! daemon the first command starts) ends with the test, whether it passes, fails, panics, or its
//! binary is killed. A process is the test's when its arguments or its environment name the
//! test's temporary folder, so that another run's processes are never seen.

#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::{Spawned, User};
use serde_json::json;

/// The variable that makes `holds_a_daemon_and_a_server` hold them: the file it writes its
/// folder in. Without it, that test is a no-op.
const HOLD: &str = "FLOTTILLE_TEST_HOLD_FOLDER_FILE";

fn fake_mcp() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(
        path.exists(),
        "{} is missing: `cargo build -p flottille-fake-mcp` first",
        path.display()
    );
    path
}

/// The processes whose arguments or environment name `folder`: `pid arguments` each.
fn left_running(folder: &Path) -> Vec<String> {
    let folder = folder.to_str().unwrap().as_bytes().to_vec();
    let named = |bytes: &[u8]| bytes.windows(folder.len()).any(|at| at == folder);
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let read = |file: &str| std::fs::read(entry.path().join(file)).unwrap_or_default();
        let cmdline = read("cmdline");
        if named(&cmdline) || named(&read("environ")) {
            found.push(format!(
                "{name} {}",
                String::from_utf8_lossy(&cmdline).replace('\0', " ")
            ));
        }
    }
    found
}

fn none_left(folder: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let left = left_running(folder);
        if left.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "left running: {left:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The fake server over HTTP in `user`'s folder, once it listens.
fn http_server(user: &User) -> Spawned {
    let script = user.path("notes-script.json");
    std::fs::write(
        &script,
        json!({"server": {"name": "notes", "version": "1.0.0"}, "tools": []}).to_string(),
    )
    .unwrap();
    let port_file = user.path("notes-port");
    let mut command = Command::new(fake_mcp());
    command.args([
        "--script",
        script.to_str().unwrap(),
        "--log",
        user.path("notes-log.jsonl").to_str().unwrap(),
        "--http",
        port_file.to_str().unwrap(),
    ]);
    let server = Spawned::new(command);
    let deadline = Instant::now() + Duration::from_secs(20);
    while std::fs::read_to_string(&port_file)
        .unwrap_or_default()
        .is_empty()
    {
        assert!(Instant::now() < deadline, "the server never listened");
        std::thread::sleep(Duration::from_millis(20));
    }
    server
}

#[test]
fn a_fake_server_ends_with_its_guard() {
    let user = User::new();
    let folder = user.path("");
    let server = http_server(&user);
    let pid = server.id();
    assert!(
        Path::new(&format!("/proc/{pid}")).exists(),
        "the server runs"
    );
    drop(server);
    // Reaped by the guard, not merely signalled: gone as the drop returns.
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "the server is left"
    );
    drop(user);
    none_left(&folder);
}

#[test]
fn a_fake_server_ends_when_its_test_panics() {
    let user = User::new();
    let folder = user.path("");
    let pid = std::cell::Cell::new(0);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let server = http_server(&user);
        pid.set(server.id());
        panic!("the test fails with its server running");
    }));
    assert!(panicked.is_err());
    assert_ne!(pid.get(), 0);
    assert!(
        !Path::new(&format!("/proc/{}", pid.get())).exists(),
        "the server is left"
    );
    drop(user);
    none_left(&folder);
}

#[test]
fn a_scenario_with_a_daemon_and_a_fake_server_leaves_no_process() {
    let user = User::new();
    let folder = user.path("");
    let server = http_server(&user);
    user.status();
    assert!(
        left_running(&folder).iter().any(|p| p.contains("daemon")),
        "the daemon runs: {:?}",
        left_running(&folder)
    );
    drop(server);
    drop(user);
    none_left(&folder);
}

/// What the killed test binary of the next test is: it starts a daemon and a fake server, says
/// where, and waits to be killed.
#[test]
fn holds_a_daemon_and_a_server() {
    let Some(file) = std::env::var_os(HOLD) else {
        return;
    };
    let user = User::new();
    let _server = http_server(&user);
    user.status();
    std::fs::write(&file, user.path("").to_str().unwrap()).unwrap();
    std::thread::sleep(Duration::from_secs(120));
}

#[test]
fn a_killed_test_binary_leaves_no_daemon_nor_fake_server() {
    let told = tempfile::tempdir().unwrap();
    let file = told.path().join("folder");
    let mut binary = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "holds_a_daemon_and_a_server", "--test-threads=1"])
        .env(HOLD, &file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while std::fs::read_to_string(&file)
        .unwrap_or_default()
        .is_empty()
    {
        assert!(Instant::now() < deadline, "the held test never said where");
        std::thread::sleep(Duration::from_millis(50));
    }
    let folder = PathBuf::from(std::fs::read_to_string(&file).unwrap());
    let before = left_running(&folder);
    assert!(
        before.iter().any(|p| p.contains("daemon")) && before.iter().any(|p| p.contains("--http")),
        "a daemon and a server run: {before:?}"
    );
    binary.kill().unwrap();
    binary.wait().unwrap();
    let ended = std::panic::catch_unwind(|| none_left(&folder));
    // The killed binary could not remove its folder.
    let _ = std::fs::remove_dir_all(&folder);
    ended.unwrap();
}
