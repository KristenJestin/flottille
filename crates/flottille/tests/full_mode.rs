//! The scenarios of Full, the third mode (`docs/design.md` §7.1): every call the gate would hold
//! passes without a request, base calls included, while every refusal of bare mode stays; a
//! session the user starts starts in Auto unless the user names its mode, one an agent starts
//! takes its launcher's mode, and no agent can put a session in it, though it reads its own. The agents and the judge are
//! fake ACP agents (`crates/flottille-fake-agent`), in throwaway repositories and temporary XDG
//! folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository pushed to a throwaway remote, its workspace `w`,
/// and the session of a root agent working there: the agent's id, its session, the workspace's
/// folder.
fn session(user: &User) -> (String, Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let place = workspace(user, "w");
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "add the login page"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization), place)
}

/// A ready workspace of `app`: its folder.
fn workspace(user: &User, name: &str) -> PathBuf {
    let made = user
        .call("workspace.create", json!({"project": "app", "name": name}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": name}))
            .unwrap()["state"]
            == "ready"
    });
    PathBuf::from(made["folder"].as_str().unwrap())
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn mode_of(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["gate_mode"].clone()
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

fn no_request(user: &User) -> bool {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .is_empty()
}

/// The prompts the judge was given.
fn judge_prompts(user: &User) -> usize {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .count()
}

/// The last `agent.tool_served` event of the agent.
fn last_served(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|e| e["name"] == "agent.tool_served")
        .cloned()
        .unwrap()["data"]
        .clone()
}

/// Whether the throwaway remote of `session` has the branch.
fn remote_has(user: &User, branch: &str) -> bool {
    std::process::Command::new("git")
        .arg("--git-dir")
        .arg(user.path("remotes/app.git"))
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn the_user_puts_a_session_in_full_from_the_command_line() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, _, _) = session(&user);

    let output = user.run(&["agent", "mode", &id, "full"]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("Agent {id} is in full mode now.\n")
    );
    assert_eq!(mode_of(&user, &id), "full");
    let watch = user.watch(&["--after", "0"]);
    let seen = watch.until(|e| e["name"] == "agent.gate_mode_changed");
    assert_eq!(
        seen.last().unwrap()["data"],
        json!({"agent": id, "mode": "full"})
    );
}

#[test]
fn in_full_a_call_that_always_asks_in_auto_is_allowed_without_a_request() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push --force origin HEAD:refs/heads/forced"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(remote_has(&user, "forced"));
    assert!(no_request(&user), "a request was made");
    assert_eq!(judge_prompts(&user), 0, "the judge was asked");
}

#[test]
fn in_full_a_call_to_judge_is_allowed_without_the_judge() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");

    done(mcp.call("run", json!({"line": "touch made.txt"})));

    assert!(folder.join("made.txt").exists());
    assert!(no_request(&user));
    assert_eq!(judge_prompts(&user), 0, "the judge was asked");
}

#[test]
fn in_full_removing_a_workspace_with_work_in_it_is_allowed_without_a_request() {
    let user = User::with_fake_agent().with_fake_launchers();
    session(&user);
    let other = workspace(&user, "spike");
    std::fs::write(other.join("draft.txt"), "uncommitted work\n").unwrap();
    // An agent removes a workspace from main, never its own.
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "tidy up"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(&user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    set_mode(&user, &id, "full");

    done(mcp.call("workspace", json!({"action": "remove", "name": "spike"})));

    assert!(!other.exists(), "the workspace is still there");
    assert!(no_request(&user));
}

#[test]
fn in_full_a_shell_line_is_still_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");

    for (tool, args) in [
        ("run", json!({"line": "ls | wc -l"})),
        ("run", json!({"line": "touch a.txt && touch b.txt"})),
        ("run", json!({"line": "sh -c 'touch c.txt; touch d.txt'"})),
        (
            "run",
            json!({"line": format!("flottille agent mode {id} ask")}),
        ),
        ("run", json!({"line": "gh auth token"})),
        ("forge", json!({"action": "pr-delete", "repository": "."})),
        ("shell", json!({"command": "ls"})),
    ] {
        let refused = mcp.call(tool, args.clone());
        assert!(refused.error, "{args}: {}", refused.text);
    }
    for file in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        assert!(!folder.join(file).exists(), "{file} was made");
    }
    assert!(no_request(&user));
    assert_eq!(judge_prompts(&user), 0);
    assert_eq!(mode_of(&user, &id), "full");
}

#[test]
fn a_call_allowed_by_full_is_recorded_as_allowed_by_full_mode() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, mcp, _) = session(&user);
    let output = user.run(&["developer", "on"]);
    assert!(output.status.success());
    set_mode(&user, &id, "full");

    done(mcp.call("run", json!({"line": "touch made.txt"})));

    let served = last_served(&user, &id);
    assert_eq!(served["gate"], "full", "{served}");
    let log = std::fs::read_to_string(user.path("state/flottille/developer.log")).unwrap();
    let call: Value = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .rfind(|line| line["kind"] == "call")
        .unwrap_or_else(|| panic!("no call in {log}"));
    assert_eq!(call["gate"], "full", "{call}");
    assert_eq!(call["allowed_by"], "Full mode", "{call}");
    assert_eq!(call["outcome"], "ok", "{call}");
}

#[test]
fn a_session_starts_in_auto_unless_the_user_names_a_mode() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, _, _) = session(&user);
    assert_eq!(mode_of(&user, &id), "auto");

    // `mode` is a permission mode of the agent's, which no start takes: the gate's is `gate_mode`.
    let refused = user.call(
        "agent.run",
        json!({"project": "app", "message": "go", "mode": "full"}),
    );
    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn an_agent_cannot_set_full_on_itself_or_a_session_it_starts() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, mcp, _) = session(&user);

    // No tool of an agent sets a mode, nor any tool of the pilot door.
    let names = mcp.names();
    assert!(names.iter().all(|name| !name.contains("mode")), "{names:?}");
    let pilot = Mcp::pilot(&user).names();
    assert!(pilot.iter().all(|name| !name.contains("mode")), "{pilot:?}");
    // Answering a request only forwards or denies it: nothing in it allows, nor names a mode.
    for decision in ["allow", "full"] {
        let refused = mcp.call(
            "approval_answer",
            json!({"request_id": 1, "decision": decision, "why": "go on"}),
        );
        assert!(refused.error, "{decision}: {}", refused.text);
    }
    // Its commands cannot drive the daemon.
    let refused = mcp.call(
        "run",
        json!({"line": format!("flottille agent mode {id} full")}),
    );
    assert!(refused.error, "{}", refused.text);
    assert_eq!(mode_of(&user, &id), "auto");
}

/// The session titled `title` the agent of `mcp` started for the user: its id, once the launch
/// was answered.
fn started(user: &User, title: &str) -> String {
    let started = user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == title)
        .cloned()
        .unwrap();
    assert_eq!(started["parent"], Value::Null);
    started["id"].as_str().unwrap().to_owned()
}

fn launch_root(mcp: &Mcp, title: &str) -> Answer {
    mcp.call(
        "agent_launch",
        json!({"title": title, "brief": "Write the docs.", "kind": "handed"}),
    )
}

#[test]
fn a_session_started_by_an_agent_in_a_full_tree_starts_in_full() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    let launched = done(launch_root(&mcp, "Docs"));

    assert!(
        launched.contains("Started a session for the user"),
        "{launched}"
    );
    assert_eq!(mode_of(&user, &started(&user, "Docs")), "full");
}

#[test]
fn a_session_started_by_an_agent_in_an_ask_tree_starts_in_ask() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "ask");

    let answer = launch_root(&mcp, "Docs");
    let number = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse::<u64>().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the session", || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["title"] == "Docs")
    });

    // Never above its launcher's mode.
    assert_eq!(mode_of(&user, &started(&user, "Docs")), "ask");
}

#[test]
fn a_session_the_user_starts_starts_in_auto() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, _, _) = session(&user);
    set_mode(&user, &id, "full");

    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "write the docs"}),
        )
        .unwrap();

    assert_eq!(
        mode_of(&user, answer["agent"]["id"].as_str().unwrap()),
        "auto"
    );
}

#[test]
fn session_read_of_its_own_session_says_the_tree_s_mode() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, mcp, _) = session(&user);
    for mode in ["ask", "auto", "full"] {
        set_mode(&user, &id, mode);

        let read = done(mcp.call("session_read", json!({"session": id})));

        assert!(
            read.contains(&format!("Your session's tree is in {mode} mode")),
            "{read}"
        );
        assert!(read.contains("Only the user changes it"), "{read}");
        assert!(
            no_request(&user),
            "{mode}: reading its own mode is asked of nobody"
        );
    }
}

#[test]
fn a_switch_of_the_launcher_s_mode_afterwards_leaves_the_session_it_started() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");
    done(launch_root(&mcp, "Docs"));
    let docs = started(&user, "Docs");

    set_mode(&user, &id, "auto");
    assert_eq!(mode_of(&user, &docs), "full");
    set_mode(&user, &docs, "ask");
    assert_eq!(mode_of(&user, &id), "auto");
}

#[test]
fn in_full_a_child_s_call_is_allowed_without_asking_its_parent() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (lead, mcp, _) = session(&user);
    set_mode(&user, &lead, "full");
    let opened = user.received_requests("session/new").len();
    let launched = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);

    let pushed = done(child.call(
        "run",
        json!({"line": "git push --force origin HEAD:refs/heads/child"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(remote_has(&user, "child"));
    // No request was made, not even one its parent would hold.
    let store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    assert!(store.request(1).unwrap().is_none(), "a request was made");
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.starts_with("refused: "), "{}", answer.text);
    answer.text
}

/// A fake login of an agent in the throwaway home: its path.
fn credential(user: &User) -> PathBuf {
    let login = user.path("home/.codex/auth.json");
    std::fs::create_dir_all(login.parent().unwrap()).unwrap();
    std::fs::write(&login, "{\"token\": \"fake-login\"}\n").unwrap();
    login
}

/// Launchers out of the daemon's tree, with what they run harmless: a gate that let one pass
/// would start it on the machine's own user manager or tmux.
const LAUNCHED: [&str; 2] = [
    "systemd-run --user flottille --version",
    "tmux -L flottille-test new-session -d true",
];

/// The calls refused in every mode (`docs/design.md` §7.1), whatever the user would say: a
/// login read, Flottille's own folders written, what drives Flottille later or from outside the
/// daemon's tree.
fn always_refused(user: &User, id: &str) -> Vec<(&'static str, Value)> {
    let login = credential(user);
    let store = user.path("state/flottille/flottille.db");
    let providers = user.path("config/flottille/providers.toml");
    let bashrc = user.path("home/.bashrc");
    vec![
        ("fs_read", json!({"path": login})),
        ("run", json!({"line": format!("cat {}", login.display())})),
        (
            "fs_write",
            json!({"path": providers, "content": "[judge]\nprovider = \"claude\"\n"}),
        ),
        (
            "run",
            json!({"line": format!(
                "sqlite3 {} \"update agents set gate_mode = 'full'\"",
                store.display()
            )}),
        ),
        (
            "fs_write",
            json!({"path": bashrc, "content": format!("flottille agent mode {id} full\n")}),
        ),
        ("run", json!({"line": LAUNCHED[0]})),
        ("run", json!({"line": LAUNCHED[1]})),
    ]
}

#[test]
fn in_full_reading_a_credential_file_is_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");
    let login = credential(&user);

    for (tool, args) in [
        ("fs_read", json!({"path": login})),
        ("run", json!({"line": format!("cat {}", login.display())})),
    ] {
        let said = refused(mcp.call(tool, args.clone()));
        assert!(said.contains("login"), "{args}: {said}");
        assert!(!said.contains("fake-login"), "{said}");
    }
    assert!(no_request(&user));
}

#[test]
fn in_full_writing_to_flottille_s_store_or_config_is_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");
    let providers = user.path("config/flottille/providers.toml");
    let before = std::fs::read(&providers).ok();
    let store = user.path("state/flottille/flottille.db");

    let said = refused(mcp.call(
        "fs_write",
        json!({"path": providers, "content": "[judge]\nprovider = \"claude\"\n"}),
    ));
    assert!(said.contains("Flottille's own folders"), "{said}");
    let said = refused(mcp.call(
        "run",
        json!({"line": format!(
            "sqlite3 {} \"update agents set title = 'changed'\"",
            store.display()
        )}),
    ));
    assert!(said.contains("Flottille's own folders"), "{said}");

    assert_eq!(std::fs::read(&providers).ok(), before);
    assert!(no_request(&user));
}

#[test]
fn in_full_writing_bashrc_is_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");
    let bashrc = user.path("home/.bashrc");

    for (tool, args) in [
        (
            "fs_write",
            json!({"path": bashrc, "content": format!("flottille agent mode {id} ask\n")}),
        ),
        (
            "run",
            json!({"line": format!("cp README.md {}", bashrc.display())}),
        ),
    ] {
        let said = refused(mcp.call(tool, args.clone()));
        assert!(said.contains("startup"), "{args}: {said}");
    }
    assert!(!bashrc.exists(), "~/.bashrc was written");
    assert!(no_request(&user));
}

#[test]
fn in_full_systemd_run_and_tmux_are_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    for line in [
        LAUNCHED[0],
        LAUNCHED[1],
        // Inside a line a wrapper the gate does not know runs, too.
        "nice -n 5 tmux -L flottille-test new-session -d true",
        "cargo run -- systemd-run --user flottille --version",
    ] {
        let said = refused(mcp.call("run", json!({ "line": line })));
        assert!(said.contains("outside"), "{line}: {said}");
    }
    assert!(no_request(&user));
    assert_eq!(mode_of(&user, &id), "full");
    assert!(user.launches().is_empty(), "{:?}", user.launches());
}

#[test]
fn in_ask_and_auto_the_same_calls_are_refused_not_held() {
    for mode in ["ask", "auto"] {
        let user = User::with_fake_agent()
            .with_allowing_judge()
            .with_fake_launchers();
        let (id, mcp, _) = session(&user);
        set_mode(&user, &id, mode);

        for (tool, args) in always_refused(&user, &id) {
            refused(mcp.call(tool, args.clone()));
        }
        assert!(no_request(&user), "{mode}: a call was held");
        assert_eq!(judge_prompts(&user), 0, "{mode}: the judge was asked");
        assert!(!user.path("home/.bashrc").exists());
        assert_eq!(mode_of(&user, &id), mode);
        assert!(user.launches().is_empty(), "{:?}", user.launches());
    }
}

#[test]
fn in_full_inline_interpreter_code_passes() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    let ran = done(mcp.call("run", json!({"line": "python3 -c \"print(1)\""})));

    assert!(ran.contains("exited with code 0"), "{ran}");
    assert!(no_request(&user));
    assert_eq!(judge_prompts(&user), 0, "the judge was asked");
}

#[test]
fn the_test_daemon_reaches_no_session_bus_nor_a_real_launcher() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    let bus = done(mcp.call("run", json!({"line": "printenv DBUS_SESSION_BUS_ADDRESS"})));
    assert!(
        bus.contains("exited with code 1"),
        "the session bus is reachable"
    );
    let tmux = done(mcp.call("run", json!({"line": "printenv TMUX_TMPDIR"})));
    assert!(
        tmux.contains(user.path("runtime").to_str().unwrap()),
        "tmux's sockets are the machine's"
    );
    for launcher in ["systemd-run", "tmux", "docker", "systemctl", "ssh"] {
        let found = done(mcp.call("run", json!({"line": format!("which {launcher}")})));
        assert!(
            found.contains(user.path("launcher-bin").to_str().unwrap()),
            "{launcher} is not the fake one"
        );
    }
}

#[test]
fn in_full_writing_bashrc_through_the_daemon_s_folder_in_proc_is_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");
    let bashrc = user.path("home/.bashrc");

    // The daemon runs in `/`: its folder in /proc leads to any path of the disk.
    let said = refused(mcp.call(
        "fs_write",
        json!({
            "path": format!("/proc/self/cwd{}", bashrc.display()),
            "content": format!("flottille agent mode {id} ask\n"),
        }),
    ));
    assert!(said.contains("/proc/self/cwd"), "{said}");
    let daemon = daemon_pid(&user);
    let said = refused(mcp.call(
        "run",
        json!({"line": format!("cp README.md /proc/{daemon}/cwd{}", bashrc.display())}),
    ));
    assert!(said.contains("/cwd"), "{said}");
    assert!(!bashrc.exists(), "~/.bashrc was written");
    assert!(no_request(&user));
}

/// The test daemon's process: Flottille's, with this user's state folder.
fn daemon_pid(user: &User) -> u32 {
    let program = std::fs::canonicalize(env!("CARGO_BIN_EXE_flottille")).unwrap();
    let state = format!("XDG_STATE_HOME={}", user.path("state").display());
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| {
            std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe == program)
                && std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environ| {
                    environ
                        .split(|byte| *byte == 0)
                        .any(|variable| variable == state.as_bytes())
                })
                && std::fs::read(format!("/proc/{pid}/cmdline"))
                    .is_ok_and(|line| line.split(|b| *b == 0).any(|word| word == b"daemon"))
        })
        .expect("the test daemon is running")
}

#[test]
fn in_full_a_container_mounting_a_login_is_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");
    let keys = user.path("home/.ssh");
    std::fs::create_dir_all(&keys).unwrap();
    let keys = keys.display();

    for line in [
        format!("docker run -v {keys}:/k alpine cat /k/id_ed25519"),
        format!("podman run --mount type=bind,source={keys},target=/k alpine true"),
    ] {
        let said = refused(mcp.call("run", json!({ "line": line })));
        assert!(said.contains(".ssh"), "{line}: {said}");
    }
    assert!(user.launches().is_empty(), "{:?}", user.launches());
    assert!(no_request(&user));
}

#[test]
fn in_full_git_config_and_systemctl_that_run_later_are_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, place) = session(&user);
    set_mode(&user, &id, "full");
    let hooks = place.join("hooks");

    for line in [
        format!("git config --global core.hooksPath {}", hooks.display()),
        format!("git config core.hooksPath {}", hooks.display()),
        "git config alias.x '!touch y'".into(),
        format!(
            "systemctl --user link {}",
            place.join("x.service").display()
        ),
        "systemctl --user start x".into(),
        "loginctl enable-linger".into(),
    ] {
        refused(mcp.call("run", json!({ "line": line })));
    }
    let config = std::fs::read_to_string(user.path("work/app/.git/config")).unwrap();
    assert!(!config.contains("hooksPath"), "{config}");
    assert!(user.launches().is_empty(), "{:?}", user.launches());
    assert!(no_request(&user));
}

#[test]
fn in_full_a_program_that_prints_a_login_is_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    for line in [
        "aws configure get aws_secret_access_key",
        "kubectl config view --raw",
        "gcloud auth print-access-token",
        "secret-tool lookup service x",
        "pass show web/x",
        "op read op://vault/item/password",
        "bw get password x",
    ] {
        let said = refused(mcp.call("run", json!({ "line": line })));
        assert!(said.contains("login"), "{line}: {said}");
    }
    assert!(user.launches().is_empty(), "{:?}", user.launches());
    assert!(no_request(&user));
}

#[test]
fn in_full_a_launcher_behind_a_wrapper_a_runner_or_a_desktop_is_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    for line in [
        "ionice -c3 at -f job now",
        "uv run ssh localhost true",
        "hyprctl dispatch exec true",
        "swaymsg exec true",
        "rsync -a README.md localhost:x",
        "git -c core.sshCommand=true fetch",
        "git fetch --upload-pack=true origin",
    ] {
        let said = refused(mcp.call("run", json!({ "line": line })));
        assert!(
            said.contains("outside") || said.contains("remote"),
            "{line}: {said}"
        );
    }
    assert!(user.launches().is_empty(), "{:?}", user.launches());
    assert!(no_request(&user));
}

#[test]
fn in_full_a_push_past_the_wall_is_refused_naming_git_push() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    for (command, line) in [
        ("send-pack", "git send-pack ../r HEAD:refs/heads/x"),
        ("http-push", "git http-push https://example.com/r.git x"),
        ("remote-ext", "git remote-ext origin 'git-receive-pack r'"),
    ] {
        let said = refused(mcp.call("run", json!({ "line": line })));
        assert!(
            said.contains(&format!(
                "git {command} pushes past Flottille's wall; use git push"
            )),
            "{line}: {said}"
        );
    }
    assert!(no_request(&user));
}

#[test]
fn in_ask_a_text_word_or_a_launcher_s_name_as_an_argument_is_held_not_refused() {
    let user = User::with_fake_agent().with_fake_launchers();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "ask");

    for line in [
        "awk -F/ '{print}' README.md",
        "cut -d/ -f1 README.md",
        "tr / _",
        "cargo test tmux --lib",
        "git commit -m ssh -m body",
    ] {
        let answer = mcp.call("run", json!({ "line": line }));
        assert!(!answer.error, "{line}: {}", answer.text);
        assert!(
            answer.text.starts_with("Waiting for"),
            "{line}: {}",
            answer.text
        );
    }
    assert!(user.launches().is_empty(), "{:?}", user.launches());
}

#[test]
fn in_full_a_repository_s_claude_commands_are_readable_but_never_written() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, place) = session(&user);
    set_mode(&user, &id, "full");
    std::fs::create_dir_all(place.join(".claude/commands")).unwrap();
    std::fs::write(
        place.join(".claude/commands/deploy-preview.md"),
        "Deploy a preview of the branch.\n",
    )
    .unwrap();
    std::fs::write(
        place.join(".claude/.credentials.json"),
        "{\"token\": \"fake-login\"}\n",
    )
    .unwrap();
    let home = user.path("home/.claude/commands/deploy-preview.md");
    std::fs::create_dir_all(home.parent().unwrap()).unwrap();
    std::fs::write(&home, "The user's own command.\n").unwrap();

    let read = done(mcp.call(
        "fs_read",
        json!({"path": ".claude/commands/deploy-preview.md"}),
    ));
    assert!(read.contains("Deploy a preview"), "{read}");
    let ran = done(mcp.call(
        "run",
        json!({"line": "cat -n .claude/commands/deploy-preview.md"}),
    ));
    assert!(ran.contains("exited with code 0"), "{ran}");

    let written = refused(mcp.call(
        "fs_write",
        json!({"path": ".claude/settings.json", "content": "{\"hooks\": {}}\n"}),
    ));
    assert!(
        written.contains("hooks") && written.contains("ask the user"),
        "{written}"
    );
    assert!(!place.join(".claude/settings.json").exists());
    for (tool, args) in [
        ("fs_read", json!({"path": ".claude/.credentials.json"})),
        ("fs_read", json!({"path": home})),
        ("run", json!({"line": format!("cat {}", home.display())})),
    ] {
        let said = refused(mcp.call(tool, args.clone()));
        assert!(said.contains("login"), "{args}: {said}");
        assert!(!said.contains("fake-login"), "{said}");
    }
    assert!(no_request(&user));
}

/// The project `app` over a throwaway repository, no session started, whose fake agent writes
/// `notes.txt` in `main` (a call to judge) in the first turn of a session.
fn writing_project(user: &User) {
    user.fake_script(json!({"turns": [{"when": "write the notes", "do": [
        {"mcp": {"tool": "fs_write", "arguments": {"path": "notes.txt", "content": "made-up notes"}}}
    ]}]}));
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// The user starts a session in `main` of `app`, in `mode`: its id.
fn start_in(user: &User, mode: &str) -> String {
    let answer = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "write the notes", "gate_mode": mode}),
        )
        .unwrap();
    answer["agent"]["id"].as_str().unwrap().to_owned()
}

#[test]
fn a_session_started_in_ask_holds_its_first_turns_calls_for_the_user() {
    // A judge that would allow the call: in Ask it is never asked.
    let user = User::with_fake_agent().with_allowing_judge();
    writing_project(&user);

    let id = start_in(&user, "ask");

    eventually("the first turn's call is held", || !no_request(&user));
    assert_eq!(mode_of(&user, &id), "ask");
    let requests = user.call("approval.list", json!({})).unwrap()["requests"].clone();
    assert_eq!(requests.as_array().unwrap().len(), 1, "{requests}");
    assert_eq!(requests[0]["agent"], id.as_str(), "{requests}");
    assert!(!user.path("work/app/notes.txt").exists());
    assert_eq!(judge_prompts(&user), 0, "the judge was asked");
}

#[test]
fn a_session_started_in_full_is_in_full_before_its_first_message() {
    let user = User::with_fake_agent().with_allowing_judge();
    writing_project(&user);

    let id = start_in(&user, "full");

    eventually("the first turn's call is served", || {
        user.path("work/app/notes.txt").exists()
    });
    assert_eq!(mode_of(&user, &id), "full");
    assert!(no_request(&user), "a request was made");
    assert_eq!(judge_prompts(&user), 0, "the judge was asked");
    eventually("the call is recorded", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.tool_served")
    });
    assert_eq!(last_served(&user, &id)["gate"], "full");
}

#[test]
fn a_start_names_ask_auto_or_full_and_nothing_else() {
    let user = User::with_fake_agent();
    writing_project(&user);

    let (code, message) = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "go", "gate_mode": "yolo"}),
        )
        .unwrap_err();

    assert_eq!(code, "invalid-params", "{message}");
    assert!(message.contains("ask, auto and full"), "{message}");
    let agents = user.call("agent.list", json!({})).unwrap()["agents"].clone();
    assert_eq!(agents.as_array().unwrap().len(), 0, "{agents}");
}

#[test]
fn an_agent_cannot_start_a_session_in_full() {
    // A judge that allows the launch: in Auto it is the judge who answers.
    let user = User::with_fake_agent()
        .with_fake_launchers()
        .with_allowing_judge();
    let (_, mcp, _) = session(&user);

    // No tool of an agent names a mode, whatever it calls it.
    for field in ["gate_mode", "mode"] {
        let mut args = json!({"title": "Docs", "brief": "Write the docs.", "kind": "handed"});
        args[field] = json!("full");
        let answer = mcp.call("agent_launch", args);
        assert!(answer.error, "{field}: {}", answer.text);
    }
    // Nor does the pilot door, which an outside agent drives.
    let pilot = Mcp::pilot(&user).call(
        "agent_start",
        json!({"project": "app", "message": "go", "gate_mode": "full"}),
    );
    assert!(pilot.error, "{}", pilot.text);
    assert_eq!(
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "a session was started"
    );

    // A session an agent starts from a tree in Auto is in Auto.
    done(launch_root(&mcp, "Docs"));
    assert_eq!(mode_of(&user, &started(&user, "Docs")), "auto");
}

/// A Claude hook in a repository's settings: it runs a program the next time the user opens
/// that repository with Claude's own command line.
const HOOK: &str = "{\"hooks\": {\"SessionStart\": [{\"hooks\": [{\"type\": \"command\", \
    \"command\": \"touch /tmp/flottille-test-hooked\"}]}]}}\n";

/// The agent folder a run wrote, told in its result as refused and put back.
fn put_back(said: &str, path: &str) {
    assert!(said.contains("refused"), "{said}");
    assert!(said.contains(path), "{said}");
    assert!(said.contains("put back"), "{said}");
}

#[test]
fn git_apply_creating_claude_settings_is_refused_or_reverted() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    let patch = format!(
        "diff --git a/.claude/settings.json b/.claude/settings.json\n\
         new file mode 100644\n\
         --- /dev/null\n\
         +++ b/.claude/settings.json\n\
         @@ -0,0 +1 @@\n\
         +{}",
        HOOK
    );
    std::fs::write(folder.join("hooks.patch"), patch).unwrap();

    let said = done(mcp.call("run", json!({"line": "git apply hooks.patch"})));

    put_back(&said, ".claude/settings.json");
    assert!(!folder.join(".claude").exists(), "{said}");
    assert!(folder.join("hooks.patch").exists());
}

#[test]
fn a_checkout_that_brings_a_claude_hook_is_refused_or_reverted() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    // The repository already has its settings; a branch changes them.
    std::fs::create_dir_all(folder.join(".claude")).unwrap();
    std::fs::write(folder.join(".claude/settings.json"), "{}\n").unwrap();
    user.git(&folder, &["add", ".claude/settings.json"]);
    user.git(&folder, &["commit", "--quiet", "-m", "settings"]);
    user.git(&folder, &["branch", "plain"]);
    user.git(&folder, &["checkout", "--quiet", "-b", "hooked"]);
    std::fs::write(folder.join(".claude/settings.json"), HOOK).unwrap();
    std::fs::create_dir_all(folder.join(".codex")).unwrap();
    std::fs::write(folder.join(".codex/config.toml"), "notify = [\"true\"]\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "hooks"]);
    user.git(&folder, &["checkout", "--quiet", "plain"]);

    let said = done(mcp.call("run", json!({"line": "git checkout hooked"})));

    put_back(&said, ".claude/settings.json");
    put_back(&said, ".codex/config.toml");
    assert_eq!(
        std::fs::read_to_string(folder.join(".claude/settings.json")).unwrap(),
        "{}\n"
    );
    assert!(!folder.join(".codex").exists());
    // What did not reach an agent folder is left as the run made it.
    let head = user.git_output(&folder, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(head.trim(), "hooked");
}

#[test]
fn cp_of_a_folder_into_a_claude_folder_is_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    std::fs::create_dir_all(folder.join("template/.claude")).unwrap();
    std::fs::write(folder.join("template/.claude/settings.json"), HOOK).unwrap();
    std::fs::write(folder.join("template/notes.md"), "notes\n").unwrap();
    std::fs::create_dir_all(folder.join("hooks")).unwrap();
    std::fs::write(folder.join("hooks/settings.json"), HOOK).unwrap();

    // Named, the agent folder is refused before the run.
    let said = refused(mcp.call("run", json!({"line": "cp -r hooks/. .claude/"})));
    assert!(said.contains("agent folder"), "{said}");
    // Brought by a folder the line copies, it is put back after it.
    let said = done(mcp.call("run", json!({"line": "cp -r template/. ."})));

    put_back(&said, ".claude/settings.json");
    assert!(!folder.join(".claude").exists(), "{said}");
    assert!(folder.join("notes.md").exists());
    assert_eq!(
        std::fs::read_to_string(folder.join("template/.claude/settings.json")).unwrap(),
        HOOK
    );
}

#[test]
fn a_staged_credentials_file_is_masked_in_a_diff() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    // Invented tokens, of no prefix a known kind of token has.
    std::fs::create_dir_all(folder.join(".claude")).unwrap();
    std::fs::write(
        folder.join(".claude/.credentials.json"),
        "{\"claudeAiOauth\":{\"accessToken\":\"fake-access-0123456789abcdef\",\
         \"refreshToken\":\"fake-refresh-0123456789abcdef\",\"expiresAt\":1762700000000}}\n",
    )
    .unwrap();
    std::fs::create_dir_all(folder.join(".codex")).unwrap();
    std::fs::write(
        folder.join(".codex/auth.json"),
        "{\n  \"tokens\": {\n    \"access_token\": \"fake-codex-access-0123456789\",\n    \
         \"refresh_token\": \"fake-codex-refresh-0123456789\"\n  }\n}\n",
    )
    .unwrap();

    done(mcp.call("run", json!({"line": "git add -A"})));
    let shown = [
        done(mcp.call("run", json!({"line": "git diff --cached"}))),
        done(mcp.call("run", json!({"line": "git stash push --include-untracked"}))),
        done(mcp.call(
            "run",
            json!({"line": "git stash show -p --include-untracked"}),
        )),
    ];

    for said in &shown {
        assert!(!said.contains("fake-"), "{said}");
    }
    assert!(shown[0].contains("accessToken"), "{}", shown[0]);
    assert!(shown[0].contains("refresh_token"), "{}", shown[0]);
    let store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let runs = store.ended_runs_of(&[id], 10, 1_000).unwrap();
    assert_eq!(runs.len(), 4);
    for run in runs {
        assert!(!run.output.contains("fake-"), "{}", run.output);
    }
}

#[test]
fn git_merge_planting_claude_settings_is_put_back() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    user.git(&folder, &["checkout", "--quiet", "-b", "hooked"]);
    std::fs::create_dir_all(folder.join(".claude")).unwrap();
    std::fs::write(folder.join(".claude/settings.json"), HOOK).unwrap();
    std::fs::write(folder.join("feature.md"), "feature\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "hooks"]);
    user.git(&folder, &["checkout", "--quiet", "-"]);

    let said = done(mcp.call("run", json!({"line": "git merge --ff-only hooked"})));

    put_back(&said, ".claude/settings.json");
    assert!(!folder.join(".claude").exists(), "{said}");
    assert!(folder.join("feature.md").exists());
}

#[test]
fn git_stash_pop_planting_claude_settings_is_put_back() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    std::fs::create_dir_all(folder.join(".claude")).unwrap();
    std::fs::write(folder.join(".claude/settings.json"), HOOK).unwrap();
    user.git(
        &folder,
        &["stash", "push", "--quiet", "--include-untracked"],
    );
    assert!(!folder.join(".claude").exists());

    let said = done(mcp.call("run", json!({"line": "git stash pop"})));

    put_back(&said, ".claude/settings.json");
    assert!(!folder.join(".claude").exists(), "{said}");
}

#[test]
fn tar_x_planting_claude_settings_is_put_back() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    let packed = user.path("packed");
    std::fs::create_dir_all(packed.join(".claude")).unwrap();
    std::fs::write(packed.join(".claude/settings.json"), HOOK).unwrap();
    std::fs::write(packed.join("notes.md"), "notes\n").unwrap();
    let made = std::process::Command::new("tar")
        .arg("cf")
        .arg(folder.join("hooks.tar"))
        .args(["-C", packed.to_str().unwrap(), ".claude", "notes.md"])
        .status()
        .unwrap();
    assert!(made.success());

    let said = done(mcp.call("run", json!({"line": "tar xf hooks.tar"})));

    put_back(&said, ".claude/settings.json");
    assert!(!folder.join(".claude").exists(), "{said}");
    assert!(folder.join("notes.md").exists());
    // What it put back is kept for the user.
    assert!(said.contains("agent-folders"), "{said}");
}

#[test]
fn a_place_too_large_to_check_tells_the_user_in_the_inbox() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers()
        .with_env("FLOTTILLE_TEST_AGENT_FOLDER_ENTRIES", "20");
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    for n in 0..30 {
        std::fs::write(folder.join(format!("note-{n}.md")), "\n").unwrap();
    }
    let packed = user.path("packed");
    std::fs::create_dir_all(&packed).unwrap();
    std::fs::write(packed.join("notes.md"), "notes\n").unwrap();
    let made = std::process::Command::new("tar")
        .arg("cf")
        .arg(folder.join("notes.tar"))
        .args(["-C", packed.to_str().unwrap(), "notes.md"])
        .status()
        .unwrap();
    assert!(made.success());

    let said = done(mcp.call("run", json!({"line": "tar xf notes.tar"})));

    assert!(said.contains("could not all be checked"), "{said}");
    let inbox = user.call("inbox.list", json!({})).unwrap();
    let entries = inbox["entries"].as_array().unwrap();
    let entry = entries
        .iter()
        .find(|entry| entry["kind"] == "unchecked")
        .unwrap_or_else(|| panic!("{inbox}"));
    assert_eq!(entry["agent"], id);
    assert!(
        entry["text"]
            .as_str()
            .unwrap()
            .contains("too large for Flottille to check"),
        "{entry}"
    );
}

#[test]
fn what_a_run_s_put_back_did_tells_the_user_in_the_inbox() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");
    let packed = user.path("packed");
    std::fs::create_dir_all(packed.join(".claude")).unwrap();
    std::fs::write(packed.join(".claude/settings.json"), HOOK).unwrap();
    let made = std::process::Command::new("tar")
        .arg("cf")
        .arg(folder.join("hooks.tar"))
        .args(["-C", packed.to_str().unwrap(), ".claude"])
        .status()
        .unwrap();
    assert!(made.success());

    done(mcp.call("run", json!({"line": "tar xf hooks.tar"})));

    // The run's output may be another agent's, or none: the user hears of it.
    let inbox = user.call("inbox.list", json!({})).unwrap();
    let entry = inbox["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "put_back")
        .unwrap_or_else(|| panic!("{inbox}"));
    assert_eq!(entry["agent"], id);
    let text = entry["text"].as_str().unwrap();
    assert!(text.contains(".claude/settings.json"), "{text}");
    assert!(text.contains("agent-folders"), "{text}");
}

#[test]
fn a_place_guarded_when_the_daemon_stopped_tells_the_user_at_its_next_start() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    // What a daemon that stopped during a guarded run left: its marker.
    let markers = user.path("state/flottille/agent-folders/.guarded");
    std::fs::create_dir_all(&markers).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        user.path("state/flottille"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(
        markers.join("0123456789ab.json"),
        json!({"root": user.path("work/app")}).to_string(),
    )
    .unwrap();

    session(&user);

    let inbox = user.call("inbox.list", json!({})).unwrap();
    let entry = inbox["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "unchecked")
        .unwrap_or_else(|| panic!("{inbox}"));
    let text = entry["text"].as_str().unwrap();
    assert!(text.contains("0123456789ab"), "{text}");
    assert!(text.contains("stopped"), "{text}");
    assert!(std::fs::read_dir(&markers).unwrap().next().is_none());
}

#[test]
fn a_resumed_setup_s_marker_is_not_taken_for_an_interrupted_one() {
    // The markers left by the daemon that stopped are told before any preparation resumes: a
    // resumed setup line's guard writes a live one, which is not to be told nor removed.
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    session(&user);
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[[setup]]\nrun = \"sleep 2\"\n");
    std::fs::write(&file, text).unwrap();
    user.call(
        "workspace.create",
        json!({"project": "app", "name": "slow"}),
    )
    .unwrap();
    eventually("the setup line", || {
        user.call("workspace.show", json!({"project": "app", "name": "slow"}))
            .unwrap()["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["state"] == "running")
    });
    assert!(user.run(&["daemon", "stop"]).status.success());
    let markers = user.path("state/flottille/agent-folders/.guarded");
    std::fs::create_dir_all(&markers).unwrap();
    std::fs::write(
        markers.join("0123456789ab.json"),
        json!({"root": user.path("work/app")}).to_string(),
    )
    .unwrap();

    let watch = user.watch(&["--after", "0"]);
    let events = watch
        .until(|event| event["name"] == "workspace.ready" && event["data"]["workspace"] == "slow");

    let restart = events
        .iter()
        .rposition(|event| event["name"] == "daemon.started")
        .unwrap();
    let after = &events[restart..];
    let names: Vec<&str> = after
        .iter()
        .map(|event| event["name"].as_str().unwrap())
        .collect();
    let resumed = names.iter().position(|name| *name == "workspace.resumed");
    let resumed = resumed.unwrap_or_else(|| panic!("{names:?}"));
    let told: Vec<&Value> = after
        .iter()
        .filter(|event| event["name"] == "run.agent_folders_unchecked")
        .collect();
    assert!(
        told.iter()
            .any(|event| event["data"]["run"] == "0123456789ab"),
        "{names:?}"
    );
    assert!(
        names[..resumed].contains(&"run.agent_folders_unchecked")
            && !names[resumed..].contains(&"run.agent_folders_unchecked"),
        "{names:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["name"] == "daemon.started")
            .count(),
        2
    );
}

#[test]
fn a_workspace_setup_line_that_writes_an_agent_folder_is_put_back_and_told() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_launchers();
    let (_, _, _) = session(&user);
    // A setup line runs what the repository holds, which an agent may have written.
    let main = user.path("work/app");
    let packed = user.path("packed");
    std::fs::create_dir_all(packed.join(".claude")).unwrap();
    std::fs::write(packed.join(".claude/settings.json"), HOOK).unwrap();
    let made = std::process::Command::new("tar")
        .arg("cf")
        .arg(main.join("hooks.tar"))
        .args(["-C", packed.to_str().unwrap(), ".claude"])
        .status()
        .unwrap();
    assert!(made.success());
    user.git(&main, &["add", "hooks.tar"]);
    user.git(&main, &["commit", "--quiet", "-m", "hooks"]);
    user.git(&main, &["push", "--quiet", "origin", "main"]);
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[[setup]]\nrun = \"tar xf hooks.tar\"\n");
    std::fs::write(&file, text).unwrap();

    let folder = workspace(&user, "set");

    assert!(folder.join("hooks.tar").exists());
    assert!(!folder.join(".claude").exists());
    let inbox = user.call("inbox.list", json!({})).unwrap();
    let entry = inbox["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "put_back")
        .unwrap_or_else(|| panic!("{inbox}"));
    assert_eq!(entry["workspace"], "set");
    let text = entry["text"].as_str().unwrap();
    assert!(text.contains(".claude/settings.json"), "{text}");
}
