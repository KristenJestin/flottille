//! The `flottille-hub` binary: its state folder, its log at a start, and `reset`, with a temporary
//! `HOME` and XDG folders. A start serves on 127.0.0.1, on a port the system chooses,
//! and is stopped once it says where it listens.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use flottille_hub::{Hub, SystemClock};

/// The binary, in a temporary home: `XDG_STATE_HOME` is `state` in it unless `xdg` is false.
fn hub(home: &Path, xdg: bool, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flottille-hub"));
    command.args(args).env("HOME", home.join("home"));
    if xdg {
        command.env("XDG_STATE_HOME", home.join("state"));
    } else {
        command.env_remove("XDG_STATE_HOME");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success() || !output.stderr.is_empty(),
        "a failure says why"
    );
    output
}

/// The binary started in a temporary home, as `hub` runs it, then stopped once it listens: its
/// log until then.
fn served(home: &Path, xdg: bool, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flottille-hub"));
    command
        .args(args)
        .args(["--listen", "127.0.0.1:0"])
        .env("HOME", home.join("home"))
        .stderr(Stdio::piped());
    if xdg {
        command.env("XDG_STATE_HOME", home.join("state"));
    } else {
        command.env_remove("XDG_STATE_HOME");
    }
    let mut child = command.spawn().unwrap();
    let mut log = String::new();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    for line in lines.by_ref() {
        let line = line.unwrap();
        log.push_str(&line);
        log.push('\n');
        if line.starts_with("flottille-hub: listening on ") {
            break;
        }
    }
    // SAFETY: kill has no precondition; the child is ours and not reaped yet.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    // Read to its end: the hub says it stops, on a log still open.
    for line in lines {
        log.push_str(&line.unwrap());
        log.push('\n');
    }
    let status = child.wait().unwrap();
    assert!(status.success(), "{log}");
    log
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn the_binary_prints_no_setup_code_and_says_how_to_pair() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("hub");
    let state = state.to_str().unwrap();

    let log = served(dir.path(), true, &["--state", state]);
    assert!(!log.contains("setup code"), "{log}");
    assert!(
        log.contains(&format!(
            "flottille-hub: no device is paired yet: run flottille-hub --state {state} pair beside \
             this hub, as its user (in a container, with docker exec)"
        )),
        "{log}"
    );
    // `setup-code` is no command any more.
    let gone = hub(dir.path(), true, &["--state", state, "setup-code"]);
    assert!(!gone.status.success());
    // Nor are the commands of the hub's host anything without a hub that runs.
    let alone = hub(dir.path(), true, &["--state", state, "devices"]);
    assert!(!alone.status.success());
    assert!(
        text(&alone.stderr).contains("no flottille-hub runs with"),
        "{}",
        text(&alone.stderr)
    );
}

#[test]
fn the_state_folder_is_under_xdg_state_home_by_default() {
    let dir = tempfile::tempdir().unwrap();
    served(dir.path(), true, &[]);
    assert!(dir.path().join("state/flottille-hub/hub.key").is_file());

    // Without XDG_STATE_HOME, the specification's fallback in the home.
    let other = tempfile::tempdir().unwrap();
    served(other.path(), false, &[]);
    assert!(
        other
            .path()
            .join("home/.local/state/flottille-hub/hub.key")
            .is_file()
    );
}

#[test]
fn the_binary_refuses_its_state_folder_while_a_hub_uses_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("hub");
    let running = Hub::open(&state, SystemClock).unwrap();

    let state_arg = state.to_str().unwrap();
    for args in [
        &["--state", state_arg, "--listen", "127.0.0.1:0"][..],
        &["--state", state_arg, "reset"],
    ] {
        let refused = hub(dir.path(), true, args);
        assert!(!refused.status.success(), "{args:?}");
        assert!(refused.stdout.is_empty(), "{args:?}");
        assert_eq!(
            text(&refused.stderr),
            format!(
                "flottille-hub: another flottille-hub uses {}: stop it first\n",
                state.display()
            )
        );
    }
    drop(running);
    let reset = hub(dir.path(), true, &["--state", state_arg, "reset"]);
    assert!(reset.status.success(), "{}", text(&reset.stderr));
}
