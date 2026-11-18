//! A model that left the agent's list of models (issue #1076): a family alias (`fable`) or an id
//! the agent's update dropped is resolved to the newest offered id of its family, else to the
//! provider's default, and the session goes on with its message; never ended for it. Against the
//! fake `claude` (`crates/flottille-fake-claude`), whose list of models the script gives. No real
//! agent ever runs here.

mod common;

use std::path::PathBuf;

use common::User;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository with one commit.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

/// Starts an agent in `app` on `model`; its id and the event number to follow it from.
fn start(user: &User, model: &str, message: &str) -> (String, u64) {
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": message, "model": model}),
        )
        .unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

fn of(agent: &str) -> impl Fn(&Value) -> bool + '_ {
    move |event: &Value| event["data"]["agent"] == agent
}

/// The agent's events from `after` up to its first turn's end or its failure.
fn until_settled(user: &User, agent: &str, after: u64) -> Vec<Value> {
    let watch = user.watch(&["--after", &after.to_string()]);
    watch
        .until(|e| {
            of(agent)(e)
                && matches!(
                    e["name"].as_str(),
                    Some("agent.turn_ended" | "agent.failed" | "agent.ended")
                )
        })
        .into_iter()
        .filter(of(agent))
        .collect()
}

fn names(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["name"].as_str().unwrap()).collect()
}

fn show(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone()
}

/// The models `set_model` asked of `claude`, in order (`default` when none was named).
fn set_models(user: &User) -> Vec<String> {
    user.claude_received()
        .into_iter()
        .filter_map(|e| e.get("received").cloned())
        .filter(|m| m["type"] == "control_request" && m["request"]["subtype"] == "set_model")
        .map(|m| {
            m["request"]["model"]
                .as_str()
                .unwrap_or("default")
                .to_owned()
        })
        .collect()
}

/// The prompts `claude` received, by their text.
fn prompts(user: &User) -> Vec<String> {
    user.claude_received()
        .into_iter()
        .filter_map(|e| e.get("received").cloned())
        .filter(|m| m["type"] == "user")
        .map(|m| m["message"]["content"].to_string())
        .collect()
}

/// What the conversation says of a model or an effort that was replaced at the start.
fn replaced(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["name"] == "agent.options_replaced")
        .map(|e| e["data"]["reason"].as_str().unwrap().to_owned())
        .collect()
}

fn row(value: &str, resolved: &str, name: &str) -> Value {
    json!({"value": value, "resolvedModel": resolved, "displayName": name,
        "description": name, "supportsEffort": true,
        "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]})
}

/// The list of models of the report: Claude Code dropped its `fable` alias and lists the family's
/// ids instead, not in the order of their versions; `opus`, `sonnet` and `haiku` stay.
fn without_fable() -> Value {
    json!([
        row("default", "claude-fable-5-2", "Default (recommended)"),
        row("opus", "claude-opus-5-5", "Opus 5.5"),
        row("claude-fable-5-1", "claude-fable-5-1", "Fable 5.1"),
        row("claude-fable-5-2", "claude-fable-5-2", "Fable 5.2"),
        row("sonnet", "claude-sonnet-5-5", "Sonnet 5.5"),
        row("haiku", "claude-haiku-5-5", "Haiku 5.5"),
        row("claude-fable-5", "claude-fable-5", "Fable 5"),
    ])
}

/// The list before: the alias was there.
fn with_fable() -> Value {
    let mut models = without_fable();
    models
        .as_array_mut()
        .unwrap()
        .push(row("fable", "claude-fable-5-2", "Fable"));
    models
}

/// No model of the `fable` family at all: `default` runs `opus`.
fn no_fable_family() -> Value {
    json!([
        row("default", "claude-opus-5-5", "Default (recommended)"),
        row("opus", "claude-opus-5-5", "Opus 5.5"),
        row("sonnet", "claude-sonnet-5-5", "Sonnet 5.5"),
    ])
}

#[test]
fn an_alias_that_left_the_list_runs_on_the_newest_of_its_family() {
    let user = User::with_fake_claude();
    user.claude_script(json!({ "models": without_fable() }));
    project(&user);

    let (id, after) = start(&user, "fable", "hello");
    let events = until_settled(&user, &id, after);

    assert!(
        names(&events).contains(&"agent.turn_ended"),
        "{:?}",
        names(&events)
    );
    assert_eq!(set_models(&user), ["claude-fable-5-2"]);
    assert_eq!(
        replaced(&events),
        ["the model fable is no longer offered by the agent: running claude-fable-5-2"]
    );
    let agent = show(&user, &id);
    assert_eq!(agent["chosen"]["model"], "claude-fable-5-2");
    assert_eq!(agent["taken"]["model"], "claude-fable-5-2");
}

#[test]
fn a_model_id_that_left_the_list_runs_on_the_newest_of_its_family() {
    let user = User::with_fake_claude();
    let mut models = without_fable();
    models
        .as_array_mut()
        .unwrap()
        .retain(|m| m["value"] != "claude-fable-5-1");
    user.claude_script(json!({ "models": models }));
    project(&user);

    let (id, after) = start(&user, "claude-fable-5-1", "hello");
    let events = until_settled(&user, &id, after);

    assert!(
        names(&events).contains(&"agent.turn_ended"),
        "{:?}",
        names(&events)
    );
    assert_eq!(set_models(&user), ["claude-fable-5-2"]);
    assert_eq!(
        replaced(&events),
        ["the model claude-fable-5-1 is no longer offered by the agent: running claude-fable-5-2"]
    );
    assert_eq!(show(&user, &id)["chosen"]["model"], "claude-fable-5-2");
}

#[test]
fn a_model_with_no_family_offered_starts_on_the_default_and_says_so() {
    let user = User::with_fake_claude();
    user.claude_script(json!({ "models": no_fable_family() }));
    project(&user);

    let (id, after) = start(&user, "fable", "hello");
    let events = until_settled(&user, &id, after);

    assert!(
        names(&events).contains(&"agent.turn_ended"),
        "{:?}",
        names(&events)
    );
    assert_eq!(
        set_models(&user),
        ["opus"],
        "the model `default` stands for"
    );
    assert_eq!(
        replaced(&events),
        ["the agent does not offer the model fable: running opus, its default"]
    );
    let agent = show(&user, &id);
    assert_eq!(agent["chosen"]["model"], "opus");
    assert_eq!(agent["taken"]["model"], "opus");
}

#[test]
fn a_refused_model_never_ends_the_session_and_its_message_is_sent() {
    let user = User::with_fake_claude();
    user.claude_script(json!({
        "models": no_fable_family(),
        "turns": [{"when": "hello", "do": [{"say": "Hello there"}]}]
    }));
    project(&user);

    let (id, after) = start(&user, "fable", "hello");
    let events = until_settled(&user, &id, after);

    assert!(
        !names(&events).contains(&"agent.failed"),
        "{:?}",
        names(&events)
    );
    assert!(
        prompts(&user).iter().any(|p| p.contains("hello")),
        "the message was sent: {:?}",
        prompts(&user)
    );
    assert!(
        events.iter().any(|e| e["data"]["text"] == "Hello there"),
        "{:?}",
        names(&events)
    );
    assert_eq!(show(&user, &id)["state"], "idle");
}

#[test]
fn a_session_ended_by_a_refused_model_resumes() {
    let user = User::with_fake_claude();
    user.claude_script(json!({ "models": with_fable() }));
    project(&user);
    let (id, after) = start(&user, "fable", "hello");
    until_settled(&user, &id, after);
    user.call("agent.stop", json!({"agent": id})).unwrap();
    let watch = user.watch(&["--after", &after.to_string()]);
    watch.until(|e| of(&id)(e) && e["name"] == "agent.ended");
    // The agent's update dropped the alias the session was stored on.
    user.claude_script(json!({ "models": without_fable() }));

    let resumed = user.resume(&id);
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    let from = resumed["after"]
        .as_u64()
        .or(sent["after"].as_u64())
        .unwrap();
    let events = until_settled(&user, &id, from);

    assert!(
        names(&events).contains(&"agent.turn_ended"),
        "{:?}",
        names(&events)
    );
    assert!(
        !names(&events).contains(&"agent.failed"),
        "{:?}",
        names(&events)
    );
    assert_eq!(set_models(&user).last().unwrap(), "claude-fable-5-2");
    assert_eq!(show(&user, &id)["chosen"]["model"], "claude-fable-5-2");
}

/// A session on `fable` that ran, then a second session that learned the list without it: the
/// daemon knows the alias left, the first session's store still holds it.
fn a_session_on_the_alias_that_left(user: &User) -> String {
    user.claude_script(json!({ "models": with_fable() }));
    project(user);
    let (id, after) = start(user, "fable", "hello");
    until_settled(user, &id, after);
    user.claude_script(json!({ "models": without_fable() }));
    let (other, after) = start(user, "sonnet", "hello");
    until_settled(user, &other, after);
    assert_eq!(show(user, &id)["chosen"]["model"], "fable");
    id
}

#[test]
fn an_effort_change_on_a_model_that_left_the_list_is_not_refused() {
    let user = User::with_fake_claude();
    let id = a_session_on_the_alias_that_left(&user);

    let answer = user.call("agent.options", json!({"agent": id, "effort": "max"}));

    let answer = answer.unwrap_or_else(|(_, why)| panic!("refused: {why}"));
    assert_eq!(answer["agent"]["pending"]["effort"], "max");
}

#[test]
fn picking_an_offered_model_on_a_session_whose_model_left_the_list_is_not_refused() {
    let user = User::with_fake_claude();
    let id = a_session_on_the_alias_that_left(&user);

    for model in ["claude-fable-5-2", "sonnet"] {
        let answer = user.call("agent.options", json!({"agent": id, "model": model}));
        let answer = answer.unwrap_or_else(|(_, why)| panic!("{model} refused: {why}"));
        assert_eq!(answer["agent"]["pending"]["model"], model);
    }
    // A model and an effort together, as the effort picker sends once a model was picked.
    let answer = user.call(
        "agent.options",
        json!({"agent": id, "model": "claude-fable-5-2", "effort": "max"}),
    );
    answer.unwrap_or_else(|(_, why)| panic!("refused: {why}"));
}

#[test]
fn an_explicit_pick_of_a_model_not_offered_is_refused_naming_what_is_offered() {
    let user = User::with_fake_claude();
    let id = a_session_on_the_alias_that_left(&user);

    // Neither the alias that left nor an id of its family the list does not hold is replaced by
    // another model: what the user asks is refused, as before.
    for model in ["fable", "claude-fable-5-3"] {
        let (code, why) = user
            .call("agent.options", json!({"agent": id, "model": model}))
            .unwrap_err();
        assert_eq!(code, "refused", "{why}");
        assert!(
            why.contains(&format!("offers no model {model}: it offers default, opus")),
            "{why}"
        );
        let (code, why) = user
            .call(
                "agent.run",
                json!({"project": "app", "message": "hello", "model": model}),
            )
            .unwrap_err();
        assert_eq!(code, "refused", "{why}");
        assert!(why.contains(&format!("offers no model {model}")), "{why}");
    }
}
