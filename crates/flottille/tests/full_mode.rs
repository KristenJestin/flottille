//! The scenarios of Full, the third mode (`docs/design.md` §7.1): every call the gate would hold
//! passes without a request, base calls included, while every refusal of bare mode stays; a
//! session never starts in it, and no agent can put a session in it. The agents and the judge are
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
    let user = User::with_fake_agent();
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
    let user = User::with_fake_agent().with_allowing_judge();
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
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp, folder) = session(&user);
    set_mode(&user, &id, "full");

    done(mcp.call("run", json!({"line": "touch made.txt"})));

    assert!(folder.join("made.txt").exists());
    assert!(no_request(&user));
    assert_eq!(judge_prompts(&user), 0, "the judge was asked");
}

#[test]
fn in_full_removing_a_workspace_with_work_in_it_is_allowed_without_a_request() {
    let user = User::with_fake_agent();
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
    let user = User::with_fake_agent().with_allowing_judge();
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
    let user = User::with_fake_agent();
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
fn a_session_starts_in_ask_or_auto_never_full() {
    let user = User::with_fake_agent();
    let (id, _, _) = session(&user);
    assert_eq!(mode_of(&user, &id), "auto");

    // The user's own start takes no mode: Full is chosen afterwards, never at the start.
    let refused = user.call(
        "agent.run",
        json!({"project": "app", "message": "go", "mode": "full"}),
    );
    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn an_agent_cannot_set_full_on_itself_or_a_session_it_starts() {
    let user = User::with_fake_agent();
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

    // A session it starts for the user, from a tree in Full, starts in Auto.
    set_mode(&user, &id, "full");
    let launched = done(mcp.call(
        "agent_launch",
        json!({"title": "Docs", "brief": "Write the docs.", "root": true}),
    ));
    assert!(
        launched.contains("Started a session for the user"),
        "{launched}"
    );
    let started = user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Docs")
        .cloned()
        .unwrap();
    assert_eq!(started["parent"], Value::Null);
    assert_eq!(mode_of(&user, started["id"].as_str().unwrap()), "auto");
}

#[test]
fn in_full_a_child_s_call_is_allowed_without_asking_its_parent() {
    let user = User::with_fake_agent();
    let (lead, mcp, _) = session(&user);
    set_mode(&user, &lead, "full");
    let opened = user.received_requests("session/new").len();
    let launched = mcp.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
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
    let user = User::with_fake_agent().with_allowing_judge();
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
    let user = User::with_fake_agent().with_allowing_judge();
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
    let user = User::with_fake_agent().with_allowing_judge();
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
    let user = User::with_fake_agent().with_allowing_judge();
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
}

#[test]
fn in_ask_and_auto_the_same_calls_are_refused_not_held() {
    for mode in ["ask", "auto"] {
        let user = User::with_fake_agent().with_allowing_judge();
        let (id, mcp, _) = session(&user);
        set_mode(&user, &id, mode);

        for (tool, args) in always_refused(&user, &id) {
            refused(mcp.call(tool, args.clone()));
        }
        assert!(no_request(&user), "{mode}: a call was held");
        assert_eq!(judge_prompts(&user), 0, "{mode}: the judge was asked");
        assert!(!user.path("home/.bashrc").exists());
        assert_eq!(mode_of(&user, &id), mode);
    }
}

#[test]
fn in_full_inline_interpreter_code_passes() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (id, mcp, _) = session(&user);
    set_mode(&user, &id, "full");

    let ran = done(mcp.call("run", json!({"line": "python3 -c \"print(1)\""})));

    assert!(ran.contains("exited with code 0"), "{ran}");
    assert!(no_request(&user));
    assert_eq!(judge_prompts(&user), 0, "the judge was asked");
}
