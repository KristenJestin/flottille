//! The scenarios of the `workspace` family (`docs/tools.md`, "The workspace"): a root reads the
//! project's workspaces at once; creating one, resuming its preparation and linking its ticket go
//! through the gate, creating and resuming judged as running each setup step they play (held for
//! the user in Ask, judged in Auto); a child has none of it; `agent_launch` with `new_workspace`
//! is judged the same. The agents are the fake agent's sessions, driven through their MCP server,
//! on throwaway repositories and temporary XDG folders; a fake of each escalating program stands
//! first on the runs' `PATH`.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::approval::Delivery;
use flottille_core::store::Store;
use serde_json::{Value, json};

fn user() -> User {
    User::with_fake_agent().with_fake_escalation()
}

/// A user whose sessions start in Auto with a judge that allows every call to judge: what is
/// still held is base.
fn judged_user() -> User {
    User::with_fake_agent()
        .with_fake_escalation()
        .with_allowing_judge()
}

/// A project `app` that is one repository holding `flaky.sh`, which fails until the file `ok`
/// of the user's folder exists, and `.env` left out of git.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    let flaky = folder.join("flaky.sh");
    std::fs::write(
        &flaky,
        format!("#!/bin/sh\ntest -f {}\n", user.path("ok").display()),
    )
    .unwrap();
    std::fs::set_permissions(&flaky, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    std::fs::write(folder.join(".env"), "TOKEN=invented\n").unwrap();
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
    folder
}

/// The user adds a step to the project's setup, as in the window.
fn step(user: &User, step: Value) {
    user.call("setup.add", json!({"project": "app", "step": step}))
        .unwrap();
}

/// A root agent the user started in the project folder: its id and MCP session.
fn root(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
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

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.starts_with("refused: "), "{}", answer.text);
    answer.text
}

fn request(user: &User, number: u64) -> Value {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap_or_else(|| panic!("no pending request #{number}"))
}

fn no_request(user: &User) -> bool {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .is_empty()
}

/// The user answers request `number` with `answer` (`approve` or `deny`): the agent's delivery.
fn answer(user: &User, answer: &str, number: u64) -> Delivery {
    let output = user.run(&[answer, &number.to_string()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id = format!("approval-{number}");
    let store = || Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    eventually("the answer", || store().delivery(&id).unwrap().is_some());
    store().delivery(&id).unwrap().unwrap()
}

fn workspace(user: &User, name: &str) -> Result<Value, (String, String)> {
    user.call("workspace.show", json!({"project": "app", "name": name}))
}

/// The prompts the judge was given, in order.
fn judge_prompts(user: &User) -> Vec<String> {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .map(|e| {
            e["received"]["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn the_workspaces_are_listed_and_shown_at_once() {
    let user = user();
    project(&user);
    step(&user, json!({"copy": [".env"]}));
    let (_, mcp) = root(&user);

    let none = done(mcp.call("workspace", json!({"action": "list"})));
    assert!(none.contains("no workspace"), "{none}");
    user.call(
        "workspace.create",
        json!({"project": "app", "name": "login"}),
    )
    .unwrap();
    eventually("the setup's end", || {
        workspace(&user, "login").unwrap()["state"] == "ready"
    });

    let listed = done(mcp.call("workspace", json!({"action": "list"})));
    assert!(listed.contains("- login: ready"), "{listed}");
    let shown = done(mcp.call("workspace", json!({"action": "show", "name": "login"})));
    assert!(shown.contains("Workspace login: ready"), "{shown}");
    assert!(shown.contains("copy .env"), "{shown}");
    let unknown = refused(mcp.call("workspace", json!({"action": "show", "name": "nope"})));
    assert!(unknown.contains("login"), "{unknown}");
    assert!(no_request(&user));
}

#[test]
fn create_is_held_in_ask_done_on_approval_and_not_on_refusal() {
    let user = user();
    project(&user);
    step(&user, json!({"copy": [".env.example"]}));
    let (_, mcp) = root(&user);

    let number = held(mcp.call("workspace", json!({"action": "create", "name": "login"})));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "action", "{asked}");
    assert_eq!(asked["class"], "judge", "{asked}");
    let says = asked["says"].to_string();
    assert!(says.contains("Create the workspace login"), "{says}");
    assert!(says.contains("1. copy .env.example"), "{says}");
    assert!(workspace(&user, "login").is_err(), "made before the answer");
    let body = answer(&user, "deny", number).body;
    assert!(!body.contains("has now done it"), "{body}");
    assert!(workspace(&user, "login").is_err(), "made on a refusal");

    let number = held(mcp.call("workspace", json!({"action": "create", "name": "login"})));
    let body = answer(&user, "approve", number).body;
    assert!(body.contains("Workspace login created"), "{body}");
    eventually("the setup's end", || {
        workspace(&user, "login").unwrap()["state"] == "ready"
    });
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn create_is_judged_in_auto_with_the_setup_s_lines_in_the_judge_s_input() {
    let user = judged_user();
    project(&user);
    step(&user, json!({"run": "./flaky.sh --setup-line"}));
    std::fs::write(user.path("ok"), "").unwrap();
    let (_, mcp) = root(&user);

    let text = done(mcp.call("workspace", json!({"action": "create", "name": "login"})));
    assert!(text.contains("Workspace login created"), "{text}");
    let prompts = judge_prompts(&user);
    let prompt = prompts.last().expect("the judge was asked");
    let setup = prompt
        .split("<setup>")
        .nth(1)
        .and_then(|rest| rest.split("</setup>").next())
        .unwrap_or_else(|| panic!("no setup block: {prompt}"));
    assert!(
        setup.contains("1. run `./flaky.sh --setup-line`"),
        "{prompt}"
    );
    eventually("the setup's end", || {
        workspace(&user, "login").unwrap()["state"] == "ready"
    });
}

#[test]
fn a_setup_holding_a_base_line_makes_create_base() {
    let user = judged_user();
    project(&user);
    step(&user, json!({"copy": [".env.example"]}));
    step(&user, json!({"run": "sudo make install"}));
    let (_, mcp) = root(&user);

    let number = held(mcp.call("workspace", json!({"action": "create", "name": "login"})));
    let asked = request(&user, number);
    assert_eq!(asked["class"], "base", "{asked}");
    let reasons = asked["reasons"].to_string();
    assert!(reasons.contains("step 2 of the setup"), "{reasons}");
    assert!(reasons.contains("sudo"), "{reasons}");
    assert!(workspace(&user, "login").is_err(), "made before the answer");
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn a_setup_s_own_link_and_secret_copy_steps_do_not_make_create_base() {
    let user = judged_user();
    project(&user);
    step(&user, json!({"copy": [".env"]}));
    step(&user, json!({"root_entries": true}));
    let (_, mcp) = root(&user);

    let text = done(mcp.call("workspace", json!({"action": "create", "name": "login"})));
    assert!(text.contains("Workspace login created"), "{text}");
    assert!(no_request(&user));
    let prompts = judge_prompts(&user);
    let prompt = prompts.last().expect("the judge was asked");
    let setup = prompt
        .split("<setup>")
        .nth(1)
        .and_then(|rest| rest.split("</setup>").next())
        .unwrap_or_else(|| panic!("no setup block: {prompt}"));
    assert!(setup.contains("1. copy .env"), "{prompt}");
    assert!(setup.contains("2. link"), "{prompt}");
    assert!(prompt.contains("sensitive name"), "{prompt}");
    assert!(prompt.contains("user's checkout"), "{prompt}");
    eventually("the setup's end", || {
        workspace(&user, "login").unwrap()["state"] == "ready"
    });
}

#[test]
fn prepare_resumes_a_failed_preparation() {
    let user = user();
    project(&user);
    step(&user, json!({"run": "./flaky.sh"}));
    user.call(
        "workspace.create",
        json!({"project": "app", "name": "login"}),
    )
    .unwrap();
    eventually("the setup's failure", || {
        workspace(&user, "login").unwrap()["state"] == "failed"
    });
    let (_, mcp) = root(&user);

    let number = held(mcp.call("workspace", json!({"action": "prepare", "name": "login"})));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "action", "{asked}");
    let says = asked["says"].to_string();
    assert!(says.contains("run `./flaky.sh`"), "{says}");
    std::fs::write(user.path("ok"), "").unwrap();
    let body = answer(&user, "approve", number).body;
    assert!(body.contains("has now done it"), "{body}");
    eventually("the setup's end", || {
        workspace(&user, "login").unwrap()["state"] == "ready"
    });
    let ready = refused(mcp.call("workspace", json!({"action": "prepare", "name": "login"})));
    assert!(ready.contains("nothing to resume"), "{ready}");
}

#[test]
fn link_ticket_links_a_ticket() {
    let user = user().with_fake_acli();
    project(&user);
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [],
            "tracker": {"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}}),
    )
    .unwrap();
    user.call(
        "workspace.create",
        json!({"project": "app", "name": "login"}),
    )
    .unwrap();
    let (_, mcp) = root(&user);

    let wrong = refused(mcp.call(
        "workspace",
        json!({"action": "link_ticket", "name": "login", "ticket": "#12"}),
    ));
    assert!(wrong.contains("Jira"), "{wrong}");
    let number = held(mcp.call(
        "workspace",
        json!({"action": "link_ticket", "name": "login", "ticket": "ABC-12"}),
    ));
    let asked = request(&user, number);
    assert_eq!(asked["class"], "judge", "{asked}");
    assert!(asked["says"].to_string().contains("ABC-12"), "{asked}");
    answer(&user, "approve", number);
    assert_eq!(workspace(&user, "login").unwrap()["ticket"], "ABC-12");
}

#[test]
fn a_child_reads_the_workspaces_and_is_refused_the_root_s_writes() {
    let user = user();
    project(&user);
    let (_, lead) = root(&user);
    let opened = user.received_requests("session/new").len();
    let launched = lead.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);

    // Since #141 every agent has the family: a child reads it, and its writes are refused.
    assert!(child.names().contains(&"workspace".to_owned()));
    let listed = child.call("workspace", json!({"action": "list"}));
    assert!(!listed.error, "{}", listed.text);
    for action in ["create", "prepare", "link_ticket", "remove"] {
        let text = refused(child.call(
            "workspace",
            json!({"action": action, "name": "x", "ticket": "#1"}),
        ));
        assert!(text.contains("ask_parent"), "{text}");
    }
    assert!(no_request(&user));
}

#[test]
fn agent_launch_with_new_workspace_is_judged_the_same() {
    let user = judged_user();
    project(&user);
    step(&user, json!({"run": "sudo make install"}));
    let (_, mcp) = root(&user);

    let created = held(mcp.call("workspace", json!({"action": "create", "name": "a"})));
    let launched = held(mcp.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests.", "new_workspace": "a",
            "root": true}),
    ));
    let (created, launched) = (request(&user, created), request(&user, launched));
    assert_eq!(launched["class"], "base", "{launched}");
    assert_eq!(launched["kind"], "action", "{launched}");
    for reason in created["reasons"].as_array().unwrap() {
        assert!(
            launched["reasons"].as_array().unwrap().contains(reason),
            "{launched}"
        );
    }
    assert!(
        launched["says"]
            .to_string()
            .contains("run `sudo make install`"),
        "{launched}"
    );
    assert!(workspace(&user, "a").is_err(), "made before the answer");
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn an_agent_s_create_in_a_repository_with_no_remote_starts_from_the_local_branch() {
    let user = judged_user();
    let folder = project(&user);
    user.git(&folder, &["remote", "remove", "origin"]);
    let (_, mcp) = root(&user);

    let text = done(mcp.call("workspace", json!({"action": "create", "name": "login"})));

    assert!(text.contains("Workspace login created"), "{text}");
    assert!(
        text.contains(" In the project folder, no remote origin: started from the local main."),
        "{text}"
    );
    let made = workspace(&user, "login").unwrap();
    assert_eq!(
        made["repositories"][0]["base"]["note"],
        "no remote origin: started from the local main"
    );
    eventually("the setup's end", || {
        workspace(&user, "login").unwrap()["state"] == "ready"
    });
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

// An agent removes its own workspace (#375): always held for the user; once allowed, the tree's
// commands in it are stopped, the removal checks as the user's does, and the session goes on in
// `main`.

/// The ready workspace `login` of the project.
fn login(user: &User) -> PathBuf {
    let made = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    eventually("the setup's end", || {
        workspace(user, "login").unwrap()["state"] == "ready"
    });
    PathBuf::from(made["folder"].as_str().unwrap())
}

/// A root agent the user started in the workspace `login`: its id and MCP session.
fn root_in_login(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "hello"}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

fn agent(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

#[test]
fn an_agent_removes_its_own_workspace_once_the_user_allows_it() {
    let user = user();
    project(&user);
    let folder = login(&user);
    let (id, mcp) = root_in_login(&user);
    // Held whatever the mode: Full performs what it holds, but not this.
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();

    let number = held(mcp.call("workspace", json!({"action": "remove"})));

    let says = request(&user, number)["says"].to_string();
    assert!(says.contains("Remove the workspace login"), "{says}");
    assert!(says.contains("Nothing would be lost"), "{says}");
    assert!(
        says.contains("This session then goes on in the project's folder (main)"),
        "{says}"
    );
    assert!(folder.exists(), "removed before the answer");
    let body = answer(&user, "approve", number).body;
    assert!(body.contains("has now done it"), "{body}");
    assert!(
        body.contains(
            "The workspace login is removed; you now work in the project's folder (main)"
        ),
        "{body}"
    );
    assert!(!folder.exists());
    assert_eq!(workspace(&user, "login").unwrap_err().0, "not-found");
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn its_running_commands_are_stopped_first_and_the_card_names_them() {
    let user = user();
    project(&user);
    let folder = login(&user);
    let (id, mcp) = root_in_login(&user);
    let number = held(mcp.call("run", json!({"line": "sleep 300", "timeout": 1})));
    answer(&user, "approve", number);
    let running = || {
        store(&user)
            .running_runs()
            .unwrap()
            .into_iter()
            .filter(|run| run.agent.as_deref() == Some(id.as_str()))
            .collect::<Vec<_>>()
    };
    eventually("the run", || running().len() == 1);
    let run = running()[0].id.clone();

    let number = held(mcp.call("workspace", json!({"action": "remove"})));

    let says = request(&user, number)["says"].to_string();
    assert!(
        says.contains("Stop the commands of this session running in it: sleep 300"),
        "{says}"
    );
    let body = answer(&user, "approve", number).body;
    assert!(body.contains("has now done it"), "{body}");
    assert!(running().is_empty());
    assert_eq!(
        store(&user).run(&run).unwrap().unwrap().state,
        flottille_core::run::RunState::Stopped
    );
    assert!(!folder.exists());
}

#[test]
fn the_session_goes_on_in_main_after_the_removal() {
    let user = user();
    let main = project(&user);
    login(&user);
    let (id, mcp) = root_in_login(&user);
    let number = held(mcp.call("workspace", json!({"action": "remove"})));
    answer(&user, "approve", number);

    // The same session, its place now the project's folder.
    let moved = agent(&user, &id);
    assert_eq!(moved["workspace"], Value::Null, "{moved}");
    assert_eq!(moved["folder"], main.to_str().unwrap(), "{moved}");
    let events = store(&user).events_after(0, 100_000).unwrap();
    let event = events
        .iter()
        .find(|e| e.name == "agent.moved")
        .expect("agent.moved");
    assert_eq!(event.data["agent"], id.as_str());
    assert_eq!(event.data["from"], "login");
    assert_eq!(event.data["to"], Value::Null);
    let instructions = store(&user).agent(&id).unwrap().unwrap().instructions;
    assert!(
        instructions.contains(&format!(", in main: the folder {}.", main.display())),
        "{instructions}"
    );
    // Its grant works in main: the project folder's own files, at the top.
    let listed = done(mcp.call("fs_list", json!({"path": "."})));
    assert!(listed.contains("flaky.sh"), "{listed}");
    let shown = done(mcp.call("workspace", json!({"action": "list"})));
    assert!(shown.contains("no workspace"), "{shown}");
    // Its next process opens the session in the project's folder.
    user.call("agent.stop", json!({"agent": id})).unwrap();
    user.resume(&id);
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    eventually("the session reopened", || {
        ["session/resume", "session/load"]
            .iter()
            .any(|method| !user.received_requests(method).is_empty())
    });
    let reopened: Vec<Value> = ["session/resume", "session/load"]
        .iter()
        .flat_map(|method| user.received_requests(method))
        .collect();
    assert_eq!(reopened[0]["params"]["cwd"], main.to_str().unwrap());
}

#[test]
fn nothing_is_removed_when_work_would_be_lost_unless_discarded() {
    let user = user();
    project(&user);
    let folder = login(&user);
    std::fs::write(folder.join("notes.txt"), "notes\n").unwrap();
    let (id, mcp) = root_in_login(&user);

    let number = held(mcp.call("workspace", json!({"action": "remove"})));
    let says = request(&user, number)["says"].to_string();
    assert!(
        says.contains("notes.txt holds work that is not committed"),
        "{says}"
    );
    let body = answer(&user, "approve", number).body;
    assert!(
        body.contains("notes.txt holds work that is not committed"),
        "{body}"
    );
    assert!(folder.join("notes.txt").exists());
    assert_eq!(agent(&user, &id)["workspace"], "login");
    let listed = done(mcp.call("fs_list", json!({"path": "."})));
    assert!(listed.contains("notes.txt"), "{listed}");

    let number = held(mcp.call("workspace", json!({"action": "remove"})));
    let output = user.run(&["approve", &number.to_string(), "--discard"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id_of = format!("approval-{number}");
    eventually("the answer", || {
        store(&user).delivery(&id_of).unwrap().is_some()
    });
    let body = store(&user).delivery(&id_of).unwrap().unwrap().body;
    assert!(body.contains("it was discarded"), "{body}");
    assert!(!folder.exists());
    assert_eq!(agent(&user, &id)["workspace"], Value::Null);
}

#[test]
fn a_child_still_removes_no_workspace() {
    let user = user();
    project(&user);
    let folder = login(&user);
    let (_, lead) = root_in_login(&user);
    let opened = user.received_requests("session/new").len();
    let launched = lead.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);
    let child_id = launched
        .text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no agent id in {}", launched.text))
        .to_owned();

    let text = refused(child.call("workspace", json!({"action": "remove"})));
    assert!(text.contains("ask_parent"), "{text}");
    assert!(no_request(&user));

    // Its root's removal waits for it: nothing is touched while it works there.
    let number = held(lead.call("workspace", json!({"action": "remove"})));
    let body = answer(&user, "approve", number).body;
    assert!(body.contains("of this agent's tree, works in it"), "{body}");
    assert!(body.contains(&child_id), "{body}");
    assert!(folder.exists());
}

/// The agent's delivery answering request `number`, once there is one.
fn answer_of(user: &User, number: u64) -> Delivery {
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap()
}

/// The request `number` as the store holds it, whatever its state.
fn stored_request(user: &User, number: u64) -> flottille_core::approval::Request {
    store(user).request(number).unwrap().unwrap()
}

#[test]
fn a_request_held_before_the_move_is_not_run_in_main() {
    let user = user();
    let main = project(&user);
    login(&user);
    let (_, mcp) = root_in_login(&user);
    // Work of main's own, not committed: a reset run there would discard it.
    std::fs::write(main.join("flaky.sh"), "changed in main\n").unwrap();
    let reset = held(mcp.call("run", json!({"line": "git reset --hard"})));
    let removal = held(mcp.call("workspace", json!({"action": "remove"})));

    let body = answer(&user, "approve", removal).body;
    assert!(body.contains("has now done it"), "{body}");

    // Never performed after the move: answered to the agent, and the user has nothing to allow.
    let body = answer_of(&user, reset).body;
    assert!(
        body.contains(
            "not run: you moved from the workspace login to the project's folder (main); ask \
             again if it is still needed"
        ),
        "{body}"
    );
    assert_eq!(
        stored_request(&user, reset).state,
        flottille_core::approval::RequestState::Cancelled
    );
    let output = user.run(&["approve", &reset.to_string()]);
    assert!(!output.status.success(), "allowed after the move");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        std::fs::read_to_string(main.join("flaky.sh")).unwrap(),
        "changed in main\n"
    );
    assert!(
        store(&user)
            .events_after(0, 100_000)
            .unwrap()
            .iter()
            .any(|e| e.name == "approval.cancelled" && e.data["number"] == reset),
        "no approval.cancelled for the window"
    );
}

#[test]
fn a_held_write_with_a_relative_path_is_not_run_after_the_move() {
    let user = user();
    let main = project(&user);
    let folder = login(&user);
    let (_, mcp) = root_in_login(&user);
    let write = held(mcp.call("fs_write", json!({"path": "../y", "content": "y"})));
    let removal = held(mcp.call("workspace", json!({"action": "remove"})));

    answer(&user, "approve", removal);

    let body = answer_of(&user, write).body;
    assert!(body.contains("not run: you moved"), "{body}");
    let output = user.run(&["approve", &write.to_string()]);
    assert!(!output.status.success(), "allowed after the move");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !main.parent().unwrap().join("y").exists(),
        "written by main"
    );
    assert!(!folder.parent().unwrap().join("y").exists());
}

#[test]
fn a_run_started_while_the_workspace_is_removed_is_refused() {
    let user = user();
    let main = project(&user);
    // A command that does not stop at once: the removal waits for it, and meanwhile the
    // session tries to start another.
    let stubborn = main.join("stubborn.sh");
    std::fs::write(
        &stubborn,
        "#!/bin/sh\ntrap '' TERM\nwhile true; do sleep 0.1; done\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &stubborn,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    user.git(&main, &["add", "stubborn.sh"]);
    user.git(&main, &["commit", "--quiet", "-m", "stubborn"]);
    // A workspace starts from the remote's branch.
    user.git(&main, &["push", "--quiet", "origin", "main"]);
    let folder = login(&user);
    let (_, mcp) = root_in_login(&user);
    let number = held(mcp.call("run", json!({"line": "./stubborn.sh", "timeout": 1})));
    answer(&user, "approve", number);
    let removal = held(mcp.call("workspace", json!({"action": "remove"})));

    let output = user.run(&["approve", &removal.to_string()]);
    assert!(output.status.success());
    // The removal stops it first, which takes its whole grace (5 s).
    std::thread::sleep(Duration::from_secs(1));

    let text = refused(mcp.call("run", json!({"line": "true"})));
    assert!(
        text.contains("the workspace login is being removed"),
        "{text}"
    );
    let text = refused(mcp.call("fs_write", json!({"path": "x", "content": "x"})));
    assert!(
        text.contains("the workspace login is being removed"),
        "{text}"
    );
    let body = answer_of(&user, removal).body;
    assert!(body.contains("has now done it"), "{body}");
    assert!(!folder.exists(), "recreated by a write");
}

#[test]
fn a_command_ending_before_the_answer_does_not_refuse_the_removal() {
    let user = user();
    project(&user);
    let folder = login(&user);
    let (id, mcp) = root_in_login(&user);
    let number = held(mcp.call("run", json!({"line": "sleep 2", "timeout": 1})));
    answer(&user, "approve", number);
    let running = || {
        store(&user)
            .running_runs()
            .unwrap()
            .into_iter()
            .any(|run| run.agent.as_deref() == Some(id.as_str()))
    };
    eventually("the run", running);
    let removal = held(mcp.call("workspace", json!({"action": "remove"})));
    let says = request(&user, removal)["says"].to_string();
    assert!(says.contains("sleep 2"), "{says}");
    eventually("the run's end", || !running());

    let body = answer(&user, "approve", removal).body;
    assert!(body.contains("has now done it"), "{body}");
    assert!(!folder.exists());
}

#[test]
fn the_own_workspace_removal_is_held_in_ask_and_auto() {
    for user in [user(), judged_user()] {
        project(&user);
        let folder = login(&user);
        let (_, mcp) = root_in_login(&user);
        held(mcp.call("workspace", json!({"action": "remove"})));
        assert!(folder.exists(), "removed without the user");
    }
}

#[test]
fn a_request_asked_while_its_agent_leaves_is_not_run_in_main() {
    // A judge that answers "ask" only once the file `go` of its folder exists: the call it judges
    // is decided after its agent moved.
    let user = User::with_fake_agent()
        .with_fake_escalation()
        .with_judge(json!([
            {"hold": "go"},
            {"say": "{\"verdict\": \"ask\", \"reason\": \"The user decides.\"}"}
        ]));
    let main = project(&user);
    let folder = login(&user);
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "workspace": "login", "message": "hello"}),
    )
    .unwrap();
    let (url, authorization) = server_of(&user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    let removal = held(mcp.call("workspace", json!({"action": "remove"})));
    let writing = std::thread::spawn(move || {
        let other = Mcp::initialize(&url, &authorization);
        other.call("run", json!({"line": "touch made.txt"}))
    });
    eventually("the judgement", || !judge_prompts(&user).is_empty());

    let body = answer(&user, "approve", removal).body;
    assert!(body.contains("has now done it"), "{body}");
    std::fs::write(user.path("fake-judge/go"), "").unwrap();
    let written = writing.join().unwrap();

    // Refused when it is asked, or not run when it is allowed: never written from main.
    if written.text.starts_with("Waiting for") {
        let number = held(written);
        let output = user.run(&["approve", &number.to_string()]);
        if output.status.success() {
            let body = answer_of(&user, number).body;
            assert!(
                body.contains("asked in the workspace login, which you have left"),
                "{body}"
            );
        }
    } else {
        let text = refused(written);
        assert!(
            text.contains("asked in the workspace login, which you have left"),
            "{text}"
        );
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(!main.join("made.txt").exists(), "run in main");
    assert!(!folder.exists());
}
