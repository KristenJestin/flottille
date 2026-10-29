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

    // Since #141 every agent has the family: a child reads it and removes its own workspace.
    assert!(child.names().contains(&"workspace".to_owned()));
    let listed = child.call("workspace", json!({"action": "list"}));
    assert!(!listed.error, "{}", listed.text);
    for action in ["create", "prepare", "link_ticket"] {
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
        json!({"title": "worker", "brief": "Write the tests.", "new_workspace": "a"}),
    ));
    let (created, launched) = (request(&user, created), request(&user, launched));
    assert_eq!(launched["class"], "base", "{launched}");
    assert_eq!(launched["kind"], "action", "{launched}");
    assert_eq!(launched["reasons"], created["reasons"], "{launched}");
    assert!(
        launched["says"]
            .to_string()
            .contains("run `sudo make install`"),
        "{launched}"
    );
    assert!(workspace(&user, "a").is_err(), "made before the answer");
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}
