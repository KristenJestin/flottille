//! A session's title (#589, `docs/tools.md`, "The session's title"): its root's agent names it
//! with `session_title` in its first turn and may correct it, the user renames it and then holds
//! it, and the title is checked and masked. Against the real daemon with the fake ACP agent,
//! throwaway repositories and temporary XDG folders.

mod common;

use std::process::Output;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// A user whose root titles its session in the turn of "rework the site".
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "rework the site", "do": [
            {"mcp": {"tool": "session_title", "arguments": {"title": "Rework the Flottille site"}}},
            {"say": "On it."}
        ]}
    ]}));
    user
}

fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn shown(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()
}

/// A root the user starts with `message`, its first turn ended: its id and its MCP session.
fn root(user: &User, message: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call("agent.start", json!({"project": "app", "message": message}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("the turn's end", || {
        shown(user, &id)["agent"]["state"] == "idle"
    });
    (id, Mcp::initialize(&url, &authorization))
}

/// The `agent.titled` events of the agent, their data.
fn titled(user: &User, agent: &str) -> Vec<Value> {
    shown(user, agent)["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["name"] == "agent.titled")
        .map(|event| event["data"].clone())
        .collect()
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    answer.text
}

fn title_of(user: &User, agent: &str) -> (Value, Value) {
    let agent = &shown(user, agent)["agent"];
    (agent["title"].clone(), agent["titled_by"].clone())
}

#[test]
fn a_root_agent_titles_its_session_in_its_first_turn() {
    let user = user();
    project(&user);
    let (id, _) = root(&user, "rework the site, all of it, page by page");

    assert_eq!(
        title_of(&user, &id),
        (json!("Rework the Flottille site"), json!("agent"))
    );
    assert_eq!(
        titled(&user, &id),
        [json!({"agent": id, "title": "Rework the Flottille site", "by": "agent"})]
    );
}

#[test]
fn a_title_too_long_or_on_several_lines_is_refused_with_the_reason() {
    let user = user();
    project(&user);
    let (id, mcp) = root(&user, "look around");

    let long = "Rework every page of the Flottille site with a brand new design system";
    let why = refused(mcp.call("session_title", json!({"title": long})));
    assert!(why.contains("at most 60 characters"), "{why}");
    let why = refused(mcp.call("session_title", json!({"title": "Rework the\nsite"})));
    assert!(why.contains("one line"), "{why}");
    let why = refused(mcp.call("session_title", json!({"title": "Site"})));
    assert!(why.contains("3 to 8 words"), "{why}");
    let why = refused(mcp.call("session_title", json!({"title": "a b c d e f g h i"})));
    assert!(why.contains("3 to 8 words"), "{why}");

    assert_eq!(title_of(&user, &id), (Value::Null, Value::Null));
    assert!(titled(&user, &id).is_empty());
}

/// The `agent.tool_served` events of the agent's `session_title` calls, their data.
fn served(user: &User, agent: &str) -> Vec<Value> {
    shown(user, agent)["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            event["name"] == "agent.tool_served" && event["data"]["tool"] == "session_title"
        })
        .map(|event| event["data"].clone())
        .collect()
}

#[test]
fn a_call_refused_for_its_input_says_so_at_its_end_and_another_refusal_does_not() {
    let user = user();
    project(&user);
    let (id, mcp) = root(&user, "look around");

    refused(mcp.call("session_title", json!({"title": "Site"})));
    refused(mcp.call("session_title", json!({"title": "Rework the\nsite"})));
    refused(mcp.call("session_title", json!({"title": 7})));
    let answer = mcp.call(
        "session_title",
        json!({"title": "Rework the Flottille site"}),
    );
    assert!(!answer.error, "{}", answer.text);
    user.call("agent.rename", json!({"agent": id, "title": "The site"}))
        .unwrap();
    refused(mcp.call("session_title", json!({"title": "Move the site to Astro"})));

    let ends: Vec<(Value, Value)> = served(&user, &id)
        .iter()
        .map(|end| (end["outcome"].clone(), end["input_refused"].clone()))
        .collect();
    assert_eq!(
        ends,
        [
            (json!("refused"), json!(true)),
            (json!("refused"), json!(true)),
            (json!("refused"), json!(true)),
            (json!("ok"), Value::Null),
            (json!("refused"), Value::Null),
        ]
    );
}

#[test]
fn an_agent_retitles_its_session_while_the_user_has_not_named_it() {
    let user = user();
    project(&user);
    let (id, mcp) = root(&user, "rework the site");

    let answer = mcp.call("session_title", json!({"title": "Move the site to Astro"}));
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        title_of(&user, &id),
        (json!("Move the site to Astro"), json!("agent"))
    );
    assert_eq!(titled(&user, &id).len(), 2);
}

#[test]
fn the_user_renames_a_session_and_the_agent_can_no_longer_retitle_it() {
    let user = user();
    project(&user);
    let (id, mcp) = root(&user, "rework the site");

    let renamed = user
        .call(
            "agent.rename",
            json!({"agent": id, "title": "Site, take two"}),
        )
        .unwrap();
    assert_eq!(renamed["title"], "Site, take two");
    assert_eq!(
        title_of(&user, &id),
        (json!("Site, take two"), json!("user"))
    );
    assert_eq!(
        titled(&user, &id).last().unwrap(),
        &json!({"agent": id, "title": "Site, take two", "by": "user"})
    );

    let why = refused(mcp.call("session_title", json!({"title": "Move the site to Astro"})));
    assert!(why.contains("the user named this session"), "{why}");
    assert_eq!(
        title_of(&user, &id),
        (json!("Site, take two"), json!("user"))
    );
    // The user renames it again as often as they like.
    user.call("agent.rename", json!({"agent": id, "title": "The site"}))
        .unwrap();
    assert_eq!(title_of(&user, &id).0, json!("The site"));
    // An empty title or one on two lines is no title.
    for title in ["  ", "The\nsite"] {
        let (code, _) = user
            .call("agent.rename", json!({"agent": id, "title": title}))
            .unwrap_err();
        assert_eq!(code, "invalid-params");
    }
}

#[test]
fn a_secret_in_a_title_is_masked() {
    let user = user();
    project(&user);
    let (id, mcp) = root(&user, "look around");
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

    let answer = mcp.call(
        "session_title",
        json!({"title": format!("Rotate {token} now")}),
    );
    assert!(!answer.error, "{}", answer.text);
    let (title, _) = title_of(&user, &id);
    assert_eq!(title, "Rotate [masked] now");
    assert!(!answer.text.contains(token));

    user.call(
        "agent.rename",
        json!({"agent": id, "title": format!("Token {token}")}),
    )
    .unwrap();
    assert_eq!(title_of(&user, &id).0, "Token [masked]");
    assert!(!shown(&user, &id).to_string().contains(token));
}

#[test]
fn a_child_cannot_be_renamed_by_the_user() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "plan it", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "planner", "brief": "Plan the work", "kind": "child"}}}]}
    ]}));
    project(&user);
    let (id, _) = root(&user, "plan it");
    eventually("the child", || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .len()
            == 2
    });
    let child = user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["parent"] == id.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, why) = user
        .call("agent.rename", json!({"agent": child, "title": "Other"}))
        .unwrap_err();
    assert!(why.contains("is a child"), "{why}");
    assert_eq!(title_of(&user, &child).0, "planner");
}

fn json_of(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn the_cli_renames_a_session() {
    let user = user();
    project(&user);
    let (id, _) = root(&user, "rework the site");

    let renamed = json_of(user.run(&["agent", "rename", &id, "Site, take two", "--json"]));
    assert_eq!(renamed["title"], "Site, take two");
    let said = user.run(&["agent", "rename", &id, "The site"]);
    assert!(said.status.success());
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        format!("Renamed agent {id}: \"The site\".")
    );
    assert_eq!(title_of(&user, &id), (json!("The site"), json!("user")));
}
