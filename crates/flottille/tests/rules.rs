//! The scenarios of how agents are told to work (`docs/projects.md`, "Rules"): the user's rules
//! for all projects and a project's own, what an agent reads and proposes of them, the text that
//! says what the gate enforces, and the server's explanation of Flottille. The agents are the fake
//! agent's sessions, driven through their MCP server as their client would, on throwaway
//! repositories and temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::approval::Delivery;
use flottille_core::store::Store;
use serde_json::{Value, json};

const BOUNDARY: &str = "\n\n__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__\n\n";

/// A project `app` that is one repository, its file starting with a comment, and its ready
/// workspace `login`.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
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
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("# The user's own words.\n{text}")).unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    folder
}

/// A root agent the user started, in `login` or in the project folder: its id and MCP session.
fn root(user: &User, workspace: Option<&str>) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let mut params = json!({"project": "app", "message": "hello"});
    if let Some(workspace) = workspace {
        params["workspace"] = workspace.into();
    }
    let answer = user.call("agent.run", params).unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

/// The instructions of an agent, split at the cache boundary.
fn instructions(user: &User, agent: &str) -> (String, String) {
    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    let text = shown["agent"]["instructions"].as_str().unwrap();
    let (before, after) = text.split_once(BOUNDARY).expect("the cache boundary");
    (before.to_owned(), after.to_owned())
}

fn default_rules(user: &User) -> String {
    user.call("rules.show", json!({})).unwrap()["default"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn held(answer: Answer) -> u64 {
    assert!(!answer.error, "an error: {}", answer.text);
    let rest = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    rest.split('.').next().unwrap().parse().unwrap()
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

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

fn cli(user: &User, args: &[&str]) {
    let output = user.run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn settled(user: &User, number: u64) -> Delivery {
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap()
}

#[test]
fn the_edited_global_rules_reach_a_new_session() {
    let user = User::with_fake_agent();
    project(&user);
    let (earlier, _) = root(&user, None);
    let default = default_rules(&user);
    let (before, _) = instructions(&user, &earlier);
    assert!(before.contains("# How to work\n\n"), "{before}");
    assert!(before.contains(default.trim()), "{before}");

    let shown = user
        .call("rules.set", json!({"text": "Answer in French.\n"}))
        .unwrap();
    assert_eq!(shown["source"], "global");
    assert_eq!(shown["text"], "Answer in French.\n");
    let (later, _) = root(&user, None);

    // The cache holds: the rules ride in the shared part, before the boundary, and a session
    // already running keeps the ones it started with.
    let (before, after) = instructions(&user, &later);
    assert!(
        before.contains("# How to work\n\nAnswer in French.\n\n---"),
        "{before}"
    );
    assert!(!before.contains(default.trim()) && !after.contains("French"));
    let (kept, _) = instructions(&user, &earlier);
    assert!(kept.contains(default.trim()) && !kept.contains("French"));
    // The user's file in Flottille's configuration folder is the rules.
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/rules.md")).unwrap(),
        "Answer in French.\n"
    );
}

#[test]
fn project_rules_override_the_global_ones() {
    let user = User::with_fake_agent();
    project(&user);
    user.call("rules.set", json!({"text": "Answer in French."}))
        .unwrap();
    let shown = user
        .call(
            "rules.set",
            json!({"project": "app", "text": "Answer in Spanish."}),
        )
        .unwrap();
    assert_eq!(shown["source"], "project");
    assert_eq!(shown["text"], "Answer in Spanish.");
    assert_eq!(shown["global"], "Answer in French.");

    let (agent, _) = root(&user, None);
    let (before, after) = instructions(&user, &agent);

    // Its own rules differ from one project to the next: after the boundary, so that the shared
    // part stays the same for every project.
    assert!(after.contains("# How to work, in this project\n\nAnswer in Spanish."));
    assert!(!before.contains("How to work") && !before.contains("French"));
    assert!(!after.contains("French"));
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(file.starts_with("# The user's own words.\n"), "{file}");
    assert!(file.contains("rules ="), "{file}");
}

#[test]
fn reset_to_default_restores_the_text() {
    let user = User::with_fake_agent();
    project(&user);
    let default = default_rules(&user);
    user.call("rules.set", json!({"text": "Answer in French."}))
        .unwrap();
    user.call(
        "rules.set",
        json!({"project": "app", "text": "Answer in Spanish."}),
    )
    .unwrap();

    // The project goes back to following the user's rules, then the user's go back to Flottille's.
    let shown = user.call("rules.reset", json!({"project": "app"})).unwrap();
    assert_eq!(
        (shown["source"].as_str(), shown["text"].as_str()),
        (Some("global"), Some("Answer in French."))
    );
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!file.contains("rules"), "{file}");
    assert!(file.starts_with("# The user's own words.\n"), "{file}");
    let shown = user.call("rules.reset", json!({})).unwrap();
    assert_eq!(shown["source"], "default");
    assert_eq!(shown["text"], default);
    assert!(!user.path("config/flottille/rules.md").exists());

    let (agent, _) = root(&user, None);
    let (before, _) = instructions(&user, &agent);
    assert!(before.contains(default.trim()) && !before.contains("French"));
    // Resetting what was never set is no error.
    user.call("rules.reset", json!({})).unwrap();
}

#[test]
fn rules_longer_than_the_limit_are_refused() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);

    let error = user
        .call("rules.set", json!({"text": "x".repeat(8_001)}))
        .unwrap_err();
    assert!(error.1.contains("8000"), "{error:?}");
    let refused = mcp.call(
        "project",
        json!({"action": "set_rules", "text": "x".repeat(8_001)}),
    );
    assert!(
        refused.error && refused.text.contains("8000"),
        "{}",
        refused.text
    );
    let nothing = mcp.call("project", json!({"action": "set_rules"}));
    assert!(
        nothing.error && nothing.text.contains("needs text"),
        "{}",
        nothing.text
    );
    assert!(!user.path("config/flottille/rules.md").exists());
}

#[test]
fn rules_md_edited_by_hand_beyond_the_limit_is_cut_and_the_cut_is_said() {
    let user = User::with_fake_agent();
    project(&user);
    let long = format!("{}TAIL", "x".repeat(8_000));
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(user.path("config/flottille/rules.md"), &long).unwrap();

    let (agent, _) = root(&user, None);
    let (before, _) = instructions(&user, &agent);
    assert!(
        before.contains(&"x".repeat(8_000)),
        "the first 8000 are given"
    );
    assert!(!before.contains("TAIL"), "past the limit is cut");
    let shown = user.call("rules.show", json!({})).unwrap();
    let cut = shown["cut"].as_str().unwrap_or_else(|| panic!("{shown}"));
    assert!(
        cut.contains("8004 characters") && cut.contains("8000") && cut.contains("rules.md"),
        "{cut}"
    );
    // The file is the user's: it is read, never rewritten.
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/rules.md")).unwrap(),
        long
    );
}

#[test]
fn a_project_rules_key_edited_by_hand_beyond_the_limit_is_cut_and_the_cut_is_said() {
    let user = User::with_fake_agent();
    project(&user);
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!("rules = \"{}TAIL\"\n{text}", "y".repeat(8_000)),
    )
    .unwrap();

    let (agent, mcp) = root(&user, None);
    let (_, after) = instructions(&user, &agent);
    assert!(
        after.contains(&"y".repeat(8_000)),
        "the first 8000 are given"
    );
    assert!(!after.contains("TAIL"), "past the limit is cut");
    let shown = user.call("rules.show", json!({"project": "app"})).unwrap();
    let cut = shown["cut"].as_str().unwrap_or_else(|| panic!("{shown}"));
    assert!(
        cut.contains("project app") && cut.contains("8004 characters") && cut.contains("8000"),
        "{cut}"
    );
    // The agent reading them is told too.
    let read = done(mcp.call("project", json!({"action": "rules"})));
    assert!(read.contains(cut), "{read}");
    assert!(!read.contains("TAIL"), "{read}");
    // Within the limit, nothing is said.
    let fine = user.call("rules.show", json!({})).unwrap();
    assert!(fine["cut"].is_null(), "{fine}");
}

#[test]
fn an_agent_reads_the_rules_and_its_set_rules_is_held_and_applied_on_approval() {
    let user = User::with_fake_agent();
    project(&user);
    let (running, mcp) = root(&user, None);
    let default = default_rules(&user);

    let read = done(mcp.call("project", json!({"action": "rules"})));
    assert!(read.contains("Flottille's default rules"), "{read}");
    assert!(read.contains(default.trim()), "{read}");

    let number = held(mcp.call(
        "project",
        json!({"action": "set_rules", "text": "Answer in French.\nCommit when asked."}),
    ));
    let asked = user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap();
    assert_eq!(asked["kind"], "action");
    assert_eq!(
        asked["says"],
        json!([
            "Replace the rules it follows for project app by:",
            "Answer in French.\nCommit when asked."
        ])
    );
    // Nothing is written while it waits.
    assert!(
        !std::fs::read_to_string(user.project_file("app"))
            .unwrap()
            .contains("rules")
    );
    cli(&user, &["approve", &number.to_string()]);
    let body = settled(&user, number).body;
    assert!(
        body.contains("Saved. The new rules reach the agents created from now on"),
        "{body}"
    );

    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(text.starts_with("# The user's own words.\n"), "{text}");
    let read = done(mcp.call("project", json!({"action": "rules"})));
    assert!(read.contains("this project's own rules"), "{read}");
    assert!(read.contains("Commit when asked."), "{read}");
    // Frozen per session: the one that asked keeps its rules, the next one gets the new ones.
    let (kept, _) = instructions(&user, &running);
    assert!(kept.contains(default.trim()));
    let (next, _) = root(&user, None);
    let (_, after) = instructions(&user, &next);
    assert!(
        after.contains("Answer in French.\nCommit when asked."),
        "{after}"
    );
}

#[test]
fn a_child_has_the_rules_in_its_instructions_but_no_tool_to_change_them() {
    let user = User::with_fake_agent();
    project(&user);
    user.call(
        "rules.set",
        json!({"project": "app", "text": "Answer in Spanish."}),
    )
    .unwrap();
    let (_, mcp) = root(&user, None);
    let opened = user.received_requests("session/new").len();
    let launched = mcp.call("agent_launch", json!({"title": "t", "brief": "Do it."}));
    assert!(!launched.error, "{}", launched.text);
    let id = launched
        .text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no agent id in {}", launched.text))
        .to_owned();
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);

    assert!(!child.names().contains(&"project".to_owned()));
    let (_, after) = instructions(&user, &id);
    assert!(after.contains("Answer in Spanish."), "{after}");
}

/// The request a held call made, as `approval.list` gives it.
fn request(user: &User, answer: Answer) -> Value {
    no_escalation(user);
    let number = held(answer);
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap()
}

/// No fake `sudo`, `doas`, `pkexec`, `run0` or `su` ran: the gate held the call.
fn no_escalation(user: &User) {
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

/// How many prompts the judge was given.
fn judged(user: &User) -> usize {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .count()
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_enforced_text_matches_the_gate_s_rules() {
    let user = User::with_fake_agent()
        .with_fake_escalation()
        .with_allowing_judge();
    project(&user);
    let (agent, mcp) = root(&user, Some("login"));
    let (before, _) = instructions(&user, &agent);
    let workspace = user
        .call("workspace.show", json!({"project": "app", "name": "login"}))
        .unwrap()["folder"]
        .as_str()
        .map(PathBuf::from)
        .unwrap();
    std::fs::write(workspace.join(".env"), "TOKEN=abc\n").unwrap();

    // Each sentence of the text, with the calls the gate answers it by.
    let claim = |sentence: &str| {
        assert!(
            before.contains(sentence),
            "the text no longer says: {sentence}"
        );
    };

    claim(
        "Harmless calls pass: reading where you work, writing in a workspace, commands known to only read",
    );
    for mode in ["ask", "auto"] {
        set_mode(&user, &agent, mode);
        done(mcp.call("fs_read", json!({"path": "README.md"})));
        done(mcp.call(
            "fs_write",
            json!({"path": "README.md", "content": "app, edited\n"}),
        ));
        done(mcp.call("run", json!({"line": "git status"})));
    }
    assert_eq!(judged(&user), 0, "a harmless call reached the judge");

    claim("Calls to judge are everything else");
    claim("In Ask a call to judge is held for the user.");
    set_mode(&user, &agent, "ask");
    let asked = request(&user, mcp.call("run", json!({"line": "touch made.txt"})));
    assert_eq!(asked["class"], "judge", "{asked}");
    assert_eq!(judged(&user), 0, "Ask reached the judge");

    claim("In Auto a judge model allows it or holds it for the user; it never refuses.");
    set_mode(&user, &agent, "auto");
    done(mcp.call("run", json!({"line": "touch made.txt"})));
    assert!(workspace.join("made.txt").exists());
    assert_eq!(judged(&user), 1);

    claim("one mode for the whole tree, and only the user sets it");
    assert!(
        !mcp.names()
            .iter()
            .any(|n| n.contains("mode") || n.contains("judge")),
        "{:?}",
        mcp.names()
    );

    claim("Base calls are always held for the user, in both modes, and never reach the judge");
    claim("a sensitive place (`.env` files");
    for mode in ["ask", "auto"] {
        set_mode(&user, &agent, mode);
        let base = request(&user, mcp.call("fs_read", json!({"path": ".env"})));
        assert_eq!(base["class"], "base", "{base}");
        let forced = request(
            &user,
            mcp.call("run", json!({"line": "git push --force origin login"})),
        );
        assert_eq!(forced["class"], "base", "{forced}");
        for line in ["git gc --prune=now", "git -c pager.log=./x log", "sudo ls"] {
            let base = request(&user, mcp.call("run", json!({ "line": line })));
            assert_eq!(base["class"], "base", "{line}: {base}");
        }
        let rules = request(
            &user,
            mcp.call("project", json!({"action": "set_rules", "text": "Push."})),
        );
        assert_eq!(rules["class"], "base", "{rules}");
        for (tool, args) in [
            (
                "tasks",
                json!({"action": "add", "name": "up", "line": "sudo make install"}),
            ),
            (
                "setup",
                json!({"action": "add", "step": {"link": "node_modules"}}),
            ),
            (
                "setup",
                json!({"action": "add", "step": {"copy": [".env"]}}),
            ),
            (
                "tasks",
                json!({"action": "add", "name": "ro", "line": "make", "read_only": true}),
            ),
        ] {
            let base = request(&user, mcp.call(tool, args));
            assert_eq!(base["class"], "base", "{tool}: {base}");
        }
    }
    claim("a task or setup step whose later run would be base");
    claim("a setup step that links the user's checkout into workspaces or copies a secret");
    claim("a task's asking turned off or its check and read-only flags set");
    claim("a forced push");
    claim("removing git's reflog or unreachable objects");
    claim("git settings that run programs");
    claim("`sudo` and its like");
    claim("a change of the project's tracker or rules");
    assert_eq!(judged(&user), 1, "a base call reached the judge");
    no_escalation(&user);

    claim("What is refused is a call that cannot be made as it is: a command with shell syntax");
    let refused = mcp.call("run", json!({"line": "ls | cat"}));
    assert!(
        refused.error && refused.text.contains("shell syntax"),
        "{}",
        refused.text
    );
    claim("a malformed call");
    let malformed = mcp.call("fs_read", json!({"path": "README.md", "unknown": 1}));
    assert!(
        malformed.error && malformed.text.starts_with("refused: "),
        "{}",
        malformed.text
    );
    claim("a command that prints a login");
    let login = mcp.call("run", json!({"line": "gh auth token"}));
    assert!(
        login.error && login.text.starts_with("refused: "),
        "{}",
        login.text
    );
    claim("`flottille` run as a command");
    let daemon = mcp.call("run", json!({"line": "flottille approvals"}));
    assert!(
        daemon.error && daemon.text.starts_with("refused: "),
        "{}",
        daemon.text
    );
    assert_eq!(judged(&user), 1, "a refused call reached the judge");

    claim("A held call has not happened: you are told at once that request #n waits");
    claim(
        "A refusal or a held request is an answer: do not retry it, and do not reach the same effect with another tool or another line.",
    );
}

#[test]
fn the_server_explains_flottille_on_both_protocol_revisions() {
    let user = User::with_fake_agent();
    project(&user);
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(&user, opened);

    let initialize = common::mcp::post(
        &url,
        &[("Authorization", &authorization)],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}}}),
    );
    let initialized: Value = serde_json::from_str(&initialize.1).unwrap();
    let text = initialized["result"]["instructions"].as_str().unwrap();
    assert!(text.starts_with("# How Flottille works"), "{text}");
    assert!(text.contains("fixed for this whole session"));
    assert!(text.split_whitespace().count() < 400);

    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let discover = common::mcp::post(
        &url,
        &[
            ("Authorization", &authorization),
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "server/discover"),
        ],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
            "params": {"_meta": meta}}),
    );
    let discovered: Value = serde_json::from_str(&discover.1).unwrap();
    assert_eq!(discovered["result"]["instructions"], text, "{discovered}");
}
