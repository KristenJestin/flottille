//! The scenarios of #1321: in Full, a command an agent runs is the user's own (no user nor mount
//! namespace, the daemon's environment: the SSH agent, the display, the sign-ins), Landlock alone
//! keeping Flottille's own folders out of its reach; out of Full, the next run is confined again;
//! and a session is told where its project's pull requests and tickets are. The agents are the fake
//! ACP agent, in throwaway repositories and temporary XDG folders. Only `ssh` is the machine's:
//! real, given a temporary home with an empty `~/.ssh`, its exit status alone read.

use crate::common;

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{User, landlock_abi};
use flottille_core::store::Store;
use serde_json::json;

/// A project `app` over a throwaway repository pushed to a throwaway remote, and the session of a
/// root agent in its folder: the agent's id and its session.
fn session(user: &User) -> (String, Mcp) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    started(user, "app")
}

fn started(user: &User, project: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": project, "message": "add the login page"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The machine's `ssh`, if it has one.
fn real_ssh() -> Option<PathBuf> {
    ["/usr/bin/ssh", "/bin/ssh", "/usr/local/bin/ssh"]
        .map(PathBuf::from)
        .into_iter()
        .find(|ssh| ssh.is_file())
}

/// A temporary home's empty `~/.ssh`, as a user who never wrote one has.
fn empty_ssh_folder(user: &User) {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(user.path("home/.ssh"))
        .unwrap();
}

/// Python code, run by the agent as one line, that exits 0 when `condition` holds and prints
/// nothing of what it read.
fn python(code: &str) -> String {
    format!("python3 -c \"{code}\"")
}

/// Whether the run of `line` exited 0.
fn exits_zero(mcp: &Mcp, line: &str) -> (bool, String) {
    let ran = done(mcp.call("run", json!({"line": line})));
    (ran.contains("exited with code 0"), ran)
}

/// A line that connects to the socket at `path`: exit 0 when it could.
fn connects(path: &Path) -> String {
    python(&format!(
        "import socket; socket.socket(socket.AF_UNIX).connect('{}')",
        path.display()
    ))
}

/// A line that reads the file at `path`, printing nothing of it: exit 0 when it could.
fn reads(path: &Path) -> String {
    python(&format!("open('{}').read()", path.display()))
}

#[test]
fn a_run_in_full_reads_the_root_owned_ssh_config_with_the_real_ssh() {
    let Some(ssh) = real_ssh() else {
        eprintln!("skipped: this machine has no ssh");
        return;
    };
    let user = User::with_fake_agent().with_allowing_judge();
    empty_ssh_folder(&user);
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");

    // `ssh` itself is a launcher the gate refuses in every mode: a program the agent runs calls it,
    // as git does. Its output is never read, only its end.
    let line = python(&format!(
        "import subprocess, sys; sys.exit(subprocess.run(['{}', '-G', 'example.invalid'], \
         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode)",
        ssh.display()
    ));
    let (zero, ran) = exits_zero(&mcp, &line);

    assert!(zero, "ssh -G did not read its settings: {ran}");
}

#[test]
fn a_run_in_full_has_the_daemon_s_ssh_agent_and_display() {
    let socket = tempfile::Builder::new()
        .prefix("flottille-agent")
        .tempdir()
        .unwrap();
    let agent = socket.path().join("agent.sock");
    let _listening = std::os::unix::net::UnixListener::bind(&agent).unwrap();
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_env("SSH_AUTH_SOCK", agent.to_str().unwrap())
        .with_env("WAYLAND_DISPLAY", "wayland-test")
        .with_env("DISPLAY", ":99");
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");

    // Their presence alone: a value is never printed.
    let present = python(
        "import os, sys; sys.exit(0 if all(n in os.environ for n in ['SSH_AUTH_SOCK', \
         'WAYLAND_DISPLAY', 'DISPLAY', 'XDG_RUNTIME_DIR']) else 1)",
    );
    let (given, ran) = exits_zero(&mcp, &present);
    assert!(given, "the daemon's environment is not the run's: {ran}");
    let (reached, ran) = exits_zero(
        &mcp,
        &python(
            "import os, socket; socket.socket(socket.AF_UNIX).connect(os.environ['SSH_AUTH_SOCK'])",
        ),
    );
    assert!(reached, "the SSH agent is out of reach: {ran}");
}

#[test]
fn a_run_in_full_still_cannot_reach_the_vault_socket_nor_a_login_token() {
    if landlock_abi() < 2 {
        eprintln!(
            "skipped: this kernel's Landlock (ABI {}) is not used",
            landlock_abi()
        );
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");
    // A connection's vault socket, a bridge's MCP configuration with its token, another session's
    // own folder, where the daemon keeps them.
    let vault = user.path("state/flottille/connections/web");
    std::fs::create_dir_all(&vault).unwrap();
    let _vault = std::os::unix::net::UnixListener::bind(vault.join("vault.sock")).unwrap();
    let bridge = user.path("runtime/flottille/bridge");
    std::fs::create_dir_all(&bridge).unwrap();
    std::fs::write(bridge.join("1.mcp.json"), "{\"token\": \"kept\"}").unwrap();
    let codex = user.path("data/flottille/agents/codex/codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(codex.join("session.json"), "kept").unwrap();
    let open = user.path("home/note");
    std::fs::write(&open, "open to runs").unwrap();

    for kept in [bridge.join("1.mcp.json"), codex.join("session.json")] {
        let (read, ran) = exits_zero(&mcp, &reads(&kept));
        assert!(!read, "{} is in reach: {ran}", kept.display());
    }
    let (read, ran) = exits_zero(&mcp, &reads(&open));
    assert!(read, "{ran}");
    // Connecting to a socket by its path is a right of Landlock's ninth ABI only.
    if landlock_abi() >= 9 {
        let (reached, ran) = exits_zero(&mcp, &connects(&vault.join("vault.sock")));
        assert!(!reached, "the vault socket is in reach: {ran}");
    }
}

#[test]
fn a_run_in_auto_is_still_confined() {
    let socket = tempfile::Builder::new()
        .prefix("flottille-agent")
        .tempdir()
        .unwrap();
    let agent = socket.path().join("agent.sock");
    let _listening = std::os::unix::net::UnixListener::bind(&agent).unwrap();
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_env("SSH_AUTH_SOCK", agent.to_str().unwrap());
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "auto");

    let (given, ran) = exits_zero(&mcp, "printenv SSH_AUTH_SOCK");
    assert!(!given, "the SSH agent is named: {ran}");
    let (bus, ran) = exits_zero(&mcp, "printenv DBUS_SESSION_BUS_ADDRESS");
    assert!(!bus, "the session bus is named: {ran}");
}

#[test]
fn leaving_full_confines_the_next_run() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");
    let (bus, ran) = exits_zero(&mcp, "printenv DBUS_SESSION_BUS_ADDRESS");
    assert!(bus, "the run in Full is not the user's own: {ran}");

    set_mode(&user, &id, "auto");

    let (bus, ran) = exits_zero(&mcp, "printenv DBUS_SESSION_BUS_ADDRESS");
    assert!(!bus, "the run after Full is not confined: {ran}");
}

#[test]
fn in_full_a_push_over_ssh_reaches_the_network_though_the_git_settings_changed() {
    if real_ssh().is_none() {
        eprintln!("skipped: this machine has no ssh");
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    empty_ssh_folder(&user);
    std::fs::write(
        user.path("home/.gitconfig"),
        "[user]\n\tname = Test\n\temail = test@example.com\n",
    )
    .unwrap();
    // The user's own settings, which the daemon finds at its start: the harness gives none.
    let settings = user.path("home/.gitconfig");
    let user = user.with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap());
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");
    user.git(
        &user.path("work/app"),
        &[
            "remote",
            "add",
            "forge",
            "ssh://git@example.invalid/acme/app.git",
        ],
    );
    // A dotfiles update after the daemon's start: the settings it found are no longer there.
    let settings = user.path("home/.gitconfig");
    let moved = user.path("home/.gitconfig.new");
    std::fs::write(
        &moved,
        "[user]\n\tname = Test\n\temail = test@example.com\n[push]\n\tdefault = simple\n",
    )
    .unwrap();
    std::fs::set_permissions(&moved, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::rename(&moved, &settings).unwrap();

    let pushed = done(mcp.call("run", json!({"line": "git push forge main"})));

    assert!(
        pushed.contains("Could not resolve hostname"),
        "the push did not reach the network: {pushed}"
    );
    assert!(!pushed.contains("not allowed"), "{pushed}");
    assert!(!pushed.contains("restart the daemon"), "{pushed}");
}

/// A project `shop` over a repository whose remote is on GitHub, its forge and its Jira tracker
/// declared.
fn shop(user: &User) {
    let folder = user.path("work/shop");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "shop\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/shop.git"],
    );
    user.write_project_file(
        "shop",
        &format!(
            "folder = \"{}\"\n\n\
             [[repositories]]\npath = \".\"\nbase = [\"develop\", \"main\"]\n\
             forge = {{ kind = \"github\", repository = \"acme/shop\" }}\n\n\
             [tracker]\nkind = \"jira\"\nsite = \"example.atlassian.net\"\nprojects = [\"ABC\"]\n",
            folder.display()
        ),
    );
}

/// What the map of `shop` says, wherever it is told.
fn says_the_map(text: &str) {
    for said in [
        "GitHub acme/shop",
        "`gh`",
        "remote origin",
        "develop",
        "Jira on example.atlassian.net",
        "ABC",
        "`acli`",
        "work/shop",
    ] {
        assert!(text.contains(said), "{said:?} is not said: {text}");
    }
}

#[test]
fn a_session_in_full_is_told_where_its_pull_requests_and_tickets_are() {
    let user = User::with_fake_agent().with_allowing_judge();
    shop(&user);

    // At its start: in the part of its instructions that is the session's.
    let (id, _mcp) = started(&user, "shop");
    let shown = user.call("agent.show", json!({"agent": id})).unwrap();
    let instructions = shown["agent"]["instructions"].as_str().unwrap();
    let (_, session) = instructions
        .split_once("\n\n__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__\n\n")
        .unwrap();
    says_the_map(session);

    // At a switch to Full: a delivery that says so, the map with it.
    set_mode(&user, &id, "full");

    let store = || Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap();
    let told = || {
        let store = store();
        ["queued", "sent"]
            .iter()
            .flat_map(|state| store.deliveries(&id, state).unwrap())
            .find(|delivery| delivery.kind == "mode")
    };
    eventually("the delivery of the switch", || told().is_some());
    let delivery = told().unwrap();
    assert!(delivery.body.contains("Full"), "{}", delivery.body);
    assert!(
        delivery.body.contains("the user's own"),
        "{}",
        delivery.body
    );
    says_the_map(&delivery.body);

    // Switching to Full again tells it nothing more.
    set_mode(&user, &id, "full");
    std::thread::sleep(Duration::from_millis(300));
    let store = store();
    let count = ["queued", "sent"]
        .iter()
        .flat_map(|state| store.deliveries(&id, state).unwrap())
        .filter(|delivery| delivery.kind == "mode")
        .count();
    assert_eq!(count, 1);
}
