//! Real programs through the real run sandbox (#1303, `docs/vault.md`, "The runtime folder"):
//! the tests of the sandbox drive it with fake programs, which do not read the system's files as a
//! real one does, so a sandbox change could break `ssh` or `git` while every test stayed green
//! (#1302: root-owned files seen as owned by nobody, and OpenSSH refusing `/etc/ssh/ssh_config`).
//! Here the programs an agent runs every day (`ssh`, `git`, `gh`, `cargo`, `node`, `pnpm`,
//! `curl`) run as a command of an agent working in Auto, and as the same script outside the
//! sandbox, with the same temporary home; each must end as it ends outside, and none may complain
//! of an owner or a permission. Only an exit status and a few verdicts are kept: what a real
//! program prints is never shown nor stored. The hosts are invented (`.invalid` never resolves),
//! the home is the test's with an empty `~/.ssh`, the agent is the fake ACP agent.
//!
//! Never in Full: a run in Full is the user's own, with no namespace and no Landlock
//! (`docs/tools.md`), so the test would stay green whatever the sandbox did to a program. The
//! agent works in Auto, its judge allowing every call, and the probe also reports the mount
//! namespace it runs in: a test whose run shares the daemon's namespace fails, so that it cannot
//! be put back in Full (or anywhere unconfined) unseen.
//!
//! A program not on the `PATH`, or one that does not work outside either, is skipped with its
//! reason, except where `CI` is set: CI installs them, and a missing one fails there.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{User, user_namespaces};
use serde_json::{Value, json};

/// A project `app` over a throwaway repository, its workspace `w`, and the session of a root
/// agent working there in Auto, its judge allowing every call so that nothing is held (never
/// Full: its runs are not sandboxed). Its session, the workspace's folder.
fn session(user: &User) -> (Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let made = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while user
        .call("workspace.show", json!({"project": "app", "name": "w"}))
        .unwrap()["state"]
        != "ready"
    {
        assert!(Instant::now() < deadline, "the workspace never got ready");
        std::thread::sleep(Duration::from_millis(50));
    }
    let place = PathBuf::from(made["folder"].as_str().unwrap());
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "start the app"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap();
    let (url, authorization) = server_of(user, opened);
    let output = user.run(&["agent", "mode", id, "auto"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (Mcp::initialize(&url, &authorization), place)
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

/// What each program came to, as a JSON line on stdout: `status` (its exit status, `missing`
/// when it is not on the `PATH`, `timeout`), `owner` (it complained of an owner or a permission)
/// and `resolve` (it said the invented host does not resolve), beside `namespace`, the mount
/// namespace the script runs in. Nothing else of what a program printed leaves this script. Run in an empty folder, in the sandbox and outside it.
const PROBE: &str = r#"
import json, os, shutil, subprocess

COMPLAINTS = ("bad owner", "permission denied", "operation not permitted",
              "dubious ownership", "unsafe repository", "not owned")
# A short connect timeout: the invented host stops at its name, but nothing may wait long.
SSH = "ssh -o ConnectTimeout=5 -o BatchMode=yes"
IDENTITY = ["-c", "user.name=Probe", "-c", "user.email=probe@example.invalid",
            "-c", "commit.gpgsign=false"]
PROGRAMS = [
    ("ssh", ["ssh", "-G", "example.invalid"]),
    ("git-ls-remote", ["git", "-c", "core.sshCommand=" + SSH, "ls-remote", "ssh://nohost.invalid/x"]),
    ("git-init", ["git", "init", "--quiet", "repo"]),
    ("git-commit", ["git", "-C", "repo"] + IDENTITY + ["commit", "--quiet", "--allow-empty", "-m", "probe"]),
    ("gh", ["gh", "--version"]),
    ("cargo", ["cargo", "--version"]),
    ("node", ["node", "--version"]),
    ("pnpm", ["pnpm", "--version"]),
    ("curl", ["curl", "--version"]),
]
out = {}
for name, line in PROGRAMS:
    if shutil.which(line[0]) is None:
        out[name] = {"status": "missing"}
        continue
    try:
        done = subprocess.run(line, capture_output=True, text=True, timeout=30, stdin=subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        out[name] = {"status": "timeout"}
        continue
    said = done.stderr.lower()
    out[name] = {"status": done.returncode,
                 "owner": any(word in said for word in COMPLAINTS),
                 "resolve": "nohost.invalid" in said and "resolve" in said}
print(json.dumps({"probe": out, "namespace": os.readlink("/proc/self/ns/mnt")}))
"#;

/// The verdicts of [`PROBE`] in the sandbox of the agent's run, once the run is known to be in a
/// mount namespace of its own: the test's is the daemon's, and a run in Full or with no
/// namespace is in it.
fn through_the_sandbox(mcp: &Mcp, place: &Path) -> Value {
    std::fs::write(place.join("probe.py"), PROBE).unwrap();
    let text = done(mcp.call("run", json!({"line": "python3 probe.py", "timeout": 120})));
    let ours = std::fs::read_link("/proc/self/ns/mnt").unwrap();
    assert_ne!(
        probe_namespace(&text),
        ours.to_str().unwrap(),
        "the run is not sandboxed: it shares the daemon's mount namespace"
    );
    probe_line(&text)
}

/// The verdicts of [`PROBE`] outside any sandbox, in an empty folder, with the user's temporary
/// home and no variable of the machine's but its `PATH`.
fn outside(user: &User) -> Value {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("probe.py"), PROBE).unwrap();
    let mut command = Command::new("python3");
    command
        .arg("probe.py")
        .current_dir(folder.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    // Where rustup keeps its toolchains, which `cargo` (a rustup proxy) looks for in the home
    // otherwise: the same variables the run gets from the daemon, which inherits them.
    for name in ["RUSTUP_HOME", "CARGO_HOME", "RUSTUP_TOOLCHAIN"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let output = command
        .env("HOME", user.path("home"))
        .env("XDG_CONFIG_HOME", user.path("config"))
        .env("XDG_DATA_HOME", user.path("data"))
        .env("XDG_STATE_HOME", user.path("state"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the probe outside the sandbox failed"
    );
    probe_line(&String::from_utf8(output.stdout).unwrap())
}

fn probe_report(text: &str) -> Value {
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("{\"probe\""))
        .expect("the probe said nothing");
    serde_json::from_str::<Value>(line.trim()).unwrap()
}

fn probe_line(text: &str) -> Value {
    probe_report(text)["probe"].clone()
}

fn probe_namespace(text: &str) -> String {
    probe_report(text)["namespace"].as_str().unwrap().to_owned()
}

/// Whether the row `name` can be judged: its program is there and works outside the sandbox.
/// Said when not, and a failure where `CI` is set, which installs them all.
fn judged(name: &str, outside: &Value) -> bool {
    let status = &outside[name]["status"];
    let reason = match status {
        s if s == "missing" => "not on the PATH",
        s if s == "timeout" => "it did not end outside the sandbox either",
        s if *s != json!(0) && name != "git-ls-remote" => "it fails outside the sandbox too",
        _ => return true,
    };
    assert!(
        std::env::var_os("CI").is_none(),
        "{name}: {reason}, and CI installs it"
    );
    eprintln!("skipped {name}: {reason}");
    false
}

/// A user whose home has an empty `~/.ssh`, so that what `ssh` reads there adds nothing to the
/// machine's own `/etc/ssh/ssh_config` and what it includes (and `-F /dev/null` hides #1302).
fn user_with_empty_ssh() -> User {
    let user = User::with_fake_agent().with_allowing_judge();
    std::fs::create_dir_all(user.path("home/.ssh")).unwrap();
    user
}

/// The row `name` of the sandbox's run ended as outside, with no complaint of an owner.
fn same_as_outside(name: &str, inside: &Value, outside: &Value) {
    assert_eq!(
        inside[name], outside[name],
        "{name}: inside the sandbox {} but outside {}",
        inside[name], outside[name]
    );
    assert_eq!(inside[name]["owner"], false, "{name}: {}", inside[name]);
}

#[test]
fn ssh_reads_the_system_config_in_a_run() {
    if !user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let user = user_with_empty_ssh();
    let outside = outside(&user);
    if !judged("ssh", &outside) {
        return;
    }
    let (mcp, place) = session(&user);

    let inside = through_the_sandbox(&mcp, &place);

    same_as_outside("ssh", &inside, &outside);
    assert_eq!(inside["ssh"]["status"], 0, "{}", inside["ssh"]);
}

#[test]
fn git_over_ssh_fails_on_the_host_in_a_run() {
    if !user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let user = user_with_empty_ssh();
    let outside = outside(&user);
    if !judged("git-ls-remote", &outside) {
        return;
    }
    let (mcp, place) = session(&user);

    let inside = through_the_sandbox(&mcp, &place);

    same_as_outside("git-ls-remote", &inside, &outside);
    // The failure is the name that does not resolve, not a configuration ssh refused.
    assert_eq!(
        inside["git-ls-remote"]["resolve"], true,
        "{}",
        inside["git-ls-remote"]
    );
}

#[test]
fn git_inits_and_commits_in_a_run() {
    if !user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let user = user_with_empty_ssh();
    let outside = outside(&user);
    if !judged("git-init", &outside) || !judged("git-commit", &outside) {
        return;
    }
    let (mcp, place) = session(&user);

    let inside = through_the_sandbox(&mcp, &place);

    for name in ["git-init", "git-commit"] {
        same_as_outside(name, &inside, &outside);
        assert_eq!(inside[name]["status"], 0, "{name}: {}", inside[name]);
    }
}

#[test]
fn the_tools_of_an_agent_run_in_a_run() {
    if !user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let user = user_with_empty_ssh();
    let outside = outside(&user);
    let names: Vec<&str> = ["gh", "cargo", "node", "pnpm", "curl"]
        .into_iter()
        .filter(|name| judged(name, &outside))
        .collect();
    if names.is_empty() {
        return;
    }
    let (mcp, place) = session(&user);

    let inside = through_the_sandbox(&mcp, &place);

    for name in names {
        same_as_outside(name, &inside, &outside);
        assert_eq!(inside[name]["status"], 0, "{name}: {}", inside[name]);
    }
}
