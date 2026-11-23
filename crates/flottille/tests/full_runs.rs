//! The scenarios of #1321: in Full, a command an agent runs is the user's own, as in their
//! terminal (no namespace, no Landlock, the daemon's environment: the SSH agent, the session bus,
//! the display, the sign-ins), nothing held back; out of Full, the next run is confined again and
//! the agent is told; and a session is told where its project's pull requests and tickets are.
//! The agents are the fake ACP agent, in throwaway repositories and temporary XDG folders, with
//! the test's own session bus and SSH agent, and nothing of the developer's environment. Only
//! `ssh` is the machine's, the program alone: real, always given `-F` a configuration of the test's
//! own, since `ssh` reads `~/.ssh/config` from the account's home in the password database, not
//! from `$HOME`, which would be the developer's own. Its exit status alone is read.

use crate::common;

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
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

/// The test's own `ssh` configuration, to give `ssh` with `-F`: the account's real one is never
/// read. The machine's `/etc/ssh/ssh_config` is skipped as `-F` says.
fn ssh_config(user: &User) -> PathBuf {
    let config = user.path("ssh_config");
    std::fs::write(&config, "Host *\n\tUser test\n\tBatchMode yes\n").unwrap();
    config
}

/// A user whose daemon, and so every run in Full, has the test's own environment alone: the fake
/// agent, an allowing judge, and what the harness sets, none of the developer's (forge tokens,
/// compositor sockets, sign-ins).
fn full_user() -> User {
    User::with_fake_agent()
        .with_allowing_judge()
        .with_clean_environment()
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
fn a_run_in_full_runs_the_real_ssh_on_a_configuration_of_the_test_s_own() {
    let Some(ssh) = real_ssh() else {
        eprintln!("skipped: this machine has no ssh");
        return;
    };
    let user = full_user();
    empty_ssh_folder(&user);
    let config = ssh_config(&user);
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");

    // `ssh` itself is a launcher the gate refuses in every mode: a program the agent runs calls it,
    // as git does. Its output is never read, only its end.
    let line = python(&format!(
        "import subprocess, sys; sys.exit(subprocess.run(['{}', '-F', '{}', '-G', \
         'example.invalid'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode)",
        ssh.display(),
        config.display()
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
    let user = full_user()
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
fn a_run_in_full_is_held_back_from_nothing_flottille_s_folders_included() {
    let user = full_user();
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");
    // A connection's vault socket, a bridge's MCP configuration with its (invented) token,
    // another session's own folder, where the daemon keeps them.
    let vault = user.path("state/flottille/connections/web");
    std::fs::create_dir_all(&vault).unwrap();
    let _vault = std::os::unix::net::UnixListener::bind(vault.join("vault.sock")).unwrap();
    let bridge = user.path("runtime/flottille/bridge");
    std::fs::create_dir_all(&bridge).unwrap();
    std::fs::write(bridge.join("1.mcp.json"), "{\"token\": \"invented\"}").unwrap();
    let codex = user.path("data/flottille/agents/codex/codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(codex.join("session.json"), "invented").unwrap();

    for open in [bridge.join("1.mcp.json"), codex.join("session.json")] {
        let (read, ran) = exits_zero(&mcp, &reads(&open));
        assert!(read, "{} is held back: {ran}", open.display());
    }
    let (reached, ran) = exits_zero(&mcp, &connects(&vault.join("vault.sock")));
    assert!(reached, "the vault socket is held back: {ran}");
}

#[test]
fn a_run_in_full_does_what_the_user_s_terminal_does() {
    // Flottille's state in the home, where it is on a machine: the folders on the way to it
    // (`~`, `~/.local`) are the user's as any other.
    let user = full_user();
    let home = user.path("home");
    std::fs::create_dir_all(&home).unwrap();
    let state = home.join(".local/state");
    let settings = home.join(".gitconfig");
    let user = user
        .with_env("XDG_STATE_HOME", state.to_str().unwrap())
        .with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap());
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");
    std::fs::write(home.join("listed"), "").unwrap();

    // A file made and renamed at the home's root, as an editor or `~/.claude.json` does.
    let (made, ran) = exits_zero(
        &mcp,
        &python(&format!(
            "import os; open('{h}/draft', 'w').write('x'); os.rename('{h}/draft', '{h}/kept')",
            h = home.display()
        )),
    );
    assert!(made, "a file cannot be made at the home's root: {ran}");
    assert!(home.join("kept").is_file());
    // The user's own git settings, which `git config` writes through a lock file beside them.
    // The gate refuses `git config --global` named on a line in every mode: a program the agent
    // runs calls it, as a tool of the user's would.
    let (configured, ran) = exits_zero(
        &mcp,
        &python(
            "import subprocess, sys; sys.exit(subprocess.run(['git', 'config', '--global', \
             'flottille.tried', 'yes']).returncode)",
        ),
    );
    assert!(configured, "git config --global failed: {ran}");
    assert!(
        std::fs::read_to_string(&settings)
            .unwrap()
            .contains("tried = yes")
    );
    // The home listed.
    let listed = done(mcp.call("run", json!({"line": format!("ls {}", home.display())})));
    assert!(listed.contains("exited with code 0"), "{listed}");
    assert!(listed.contains("listed"), "{listed}");
}

#[test]
fn in_full_the_bus_and_ssh_agent_a_run_gets_are_the_test_s_own() {
    let socket = tempfile::Builder::new()
        .prefix("flottille-agent")
        .tempdir()
        .unwrap();
    let agent = socket.path().join("agent.sock");
    let _listening = std::os::unix::net::UnixListener::bind(&agent).unwrap();
    let user = full_user()
        .with_fake_launchers()
        .with_env("SSH_AUTH_SOCK", agent.to_str().unwrap());
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");

    // Compared inside the run: a value is never printed.
    let own = python(&format!(
        "import os, sys; sys.exit(0 if os.environ.get('DBUS_SESSION_BUS_ADDRESS') ==          'unix:path={}' and os.environ.get('SSH_AUTH_SOCK') == '{}' else 1)",
        user.path("runtime/bus").display(),
        agent.display()
    ));
    let (given, ran) = exits_zero(&mcp, &own);
    assert!(given, "the bus or the SSH agent is not the test's: {ran}");
    for launcher in ["systemd-run", "tmux", "docker", "systemctl", "ssh"] {
        let found = done(mcp.call("run", json!({"line": format!("which {launcher}")})));
        assert!(
            found.contains(user.path("launcher-bin").to_str().unwrap()),
            "{launcher} is not the fake one: {found}"
        );
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
    let user = full_user().with_env("SSH_AUTH_SOCK", agent.to_str().unwrap());
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "auto");

    let (given, ran) = exits_zero(&mcp, "printenv SSH_AUTH_SOCK");
    assert!(!given, "the SSH agent is named: {ran}");
    let (bus, ran) = exits_zero(&mcp, "printenv DBUS_SESSION_BUS_ADDRESS");
    assert!(!bus, "the session bus is named: {ran}");
}

#[test]
fn leaving_full_confines_the_next_run() {
    let user = full_user();
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");
    let (bus, ran) = exits_zero(&mcp, "printenv DBUS_SESSION_BUS_ADDRESS");
    assert!(bus, "the run in Full is not the user's own: {ran}");

    set_mode(&user, &id, "auto");

    let (bus, ran) = exits_zero(&mcp, "printenv DBUS_SESSION_BUS_ADDRESS");
    assert!(!bus, "the run after Full is not confined: {ran}");
}

/// The deliveries of `kind` the agent `id` was given, queued or sent.
fn delivered(user: &User, id: &str, kind: &str) -> Vec<String> {
    let store = Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap();
    ["queued", "sent"]
        .iter()
        .flat_map(|state| store.deliveries(id, state).unwrap())
        .filter(|delivery| delivery.kind == kind)
        .map(|delivery| delivery.body)
        .collect()
}

#[test]
fn leaving_full_tells_the_agent_its_commands_are_confined_again() {
    let user = full_user();
    let (id, _mcp) = session(&user);
    set_mode(&user, &id, "full");
    eventually("the delivery of the switch to Full", || {
        delivered(&user, &id, "mode").len() == 1
    });

    set_mode(&user, &id, "auto");

    eventually("the delivery of the switch out of Full", || {
        delivered(&user, &id, "mode").len() == 2
    });
    let told = delivered(&user, &id, "mode")
        .into_iter()
        .find(|body| body.contains("confined again"));
    assert!(told.is_some(), "{:?}", delivered(&user, &id, "mode"));
    // Out of Full to another mode than Full: nothing more.
    set_mode(&user, &id, "ask");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(delivered(&user, &id, "mode").len(), 2);
}

#[test]
fn leaving_full_tells_a_child_and_not_an_agent_that_ended() {
    let user = full_user();
    let (id, mcp) = session(&user);
    let launch = |title: &str| {
        let text = done(mcp.call(
            "agent_launch",
            json!({"kind": "child", "title": title, "brief": "do it"}),
        ));
        text.split(|c: char| !c.is_ascii_alphanumeric())
            .find(|word| word.len() == 12 && word.chars().all(|c| c.is_ascii_hexdigit()))
            .unwrap_or_else(|| panic!("no agent id in {text}"))
            .to_owned()
    };
    let child = launch("stays");
    let gone = launch("goes");
    done(mcp.call("agent_stop", json!({"agent_id": gone})));
    eventually("the end of the second child", || {
        let shown = user.call("agent.show", json!({"agent": gone})).unwrap();
        shown["agent"]["state"] == "ended"
    });

    set_mode(&user, &id, "full");
    eventually("the child's delivery of the switch to Full", || {
        delivered(&user, &child, "mode").len() == 1
    });
    set_mode(&user, &id, "auto");

    eventually("the child's delivery of the switch out of Full", || {
        delivered(&user, &child, "mode")
            .iter()
            .any(|body| body.contains("confined again"))
    });
    assert!(delivered(&user, &gone, "mode").is_empty());
}

#[test]
fn the_newest_mode_delivery_replaces_the_older_ones_the_agent_has_not_read() {
    let user = full_user();
    let (id, _mcp) = session(&user);

    // Full, Auto, Full, Auto before the agent's next turn: it is told once, the last.
    for mode in ["full", "auto", "full", "auto"] {
        set_mode(&user, &id, mode);
    }

    eventually("the last delivery", || {
        delivered(&user, &id, "mode")
            .iter()
            .any(|body| body.contains("confined again"))
    });
    let told = delivered(&user, &id, "mode");
    assert_eq!(told.len(), 1, "{told:?}");
}

#[test]
fn a_run_in_full_inherits_only_the_test_s_environment() {
    let user = full_user();
    let (id, mcp) = session(&user);
    set_mode(&user, &id, "full");

    // Names alone, never a value: a developer's, or a runner's, forge token, compositor socket
    // or sign-in is not one of the test's own.
    let none = python(
        "import os, sys; names = ['GITHUB_ACTIONS', 'GITHUB_TOKEN', 'GH_TOKEN', 'GITLAB_TOKEN',          'CI', 'HYPRLAND_INSTANCE_SIGNATURE', 'SWAYSOCK', 'NIRI_SOCKET', 'WAYLAND_DISPLAY',          'RUNNER_OS', 'CARGO_HOME']; sys.exit(1 if any(n in os.environ for n in names) else 0)",
    );
    let (clean, ran) = exits_zero(&mcp, &none);
    assert!(
        clean,
        "a variable of the developer's reached the run: {ran}"
    );
}

#[test]
fn in_full_a_push_over_ssh_reaches_the_network_though_the_git_settings_changed() {
    let Some(ssh) = real_ssh() else {
        eprintln!("skipped: this machine has no ssh");
        return;
    };
    let user = full_user();
    empty_ssh_folder(&user);
    // The user's own `ssh` command, on a configuration of the test's own: git runs `ssh` as the
    // terminal would, which reads the account's `~/.ssh/config`, the developer's.
    let command = format!("{} -F {}", ssh.display(), ssh_config(&user).display());
    std::fs::write(
        user.path("home/.gitconfig"),
        "[user]\n\tname = Test\n\temail = test@example.com\n",
    )
    .unwrap();
    // The user's own settings, which the daemon finds at its start: the harness gives none.
    let settings = user.path("home/.gitconfig");
    let user = user
        .with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap())
        .with_env("GIT_SSH_COMMAND", &command);
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
        "never write to Jira with `acli`",
        "work/shop",
    ] {
        assert!(text.contains(said), "{said:?} is not said: {text}");
    }
}

#[test]
fn a_session_in_full_is_told_where_its_pull_requests_and_tickets_are() {
    let user = full_user();
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

#[test]
fn the_map_of_a_project_of_several_repositories_names_each_one_s_folder() {
    let user = full_user();
    let folder = user.path("work/suite");
    for repository in ["api", "web"] {
        let at = folder.join(repository);
        user.git_init(&at);
        std::fs::write(at.join("README.md"), "suite\n").unwrap();
        user.git(&at, &["add", "README.md"]);
        user.git(&at, &["commit", "--quiet", "-m", "start"]);
    }
    user.write_project_file(
        "suite",
        &format!(
            "folder = \"{}\"\n\n\
             [[repositories]]\npath = \"api\"\n\n\
             [[repositories]]\npath = \"web\"\n",
            folder.display()
        ),
    );

    let (id, _mcp) = started(&user, "suite");

    let shown = user.call("agent.show", json!({"agent": id})).unwrap();
    let instructions = shown["agent"]["instructions"].as_str().unwrap();
    for repository in ["api", "web"] {
        let said = format!(
            "Repository `{repository}`, in the folder {}:",
            folder.join(repository).display()
        );
        assert!(instructions.contains(&said), "{said:?}: {instructions}");
    }
}
