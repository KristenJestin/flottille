//! A provider's models are read as soon as it is ready, with no session and no prompt
//! (`docs/agents.md`, "The models"): the fake ACP agent and the fake `claude` stand for the
//! programs, in a throwaway home. No real agent runs.

mod common;

use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// The daemon of a test lists the models only where the test asks for it, so that the fakes of
/// the other tests see no start of their own.
const ASKS: (&str, &str) = ("FLOTTILLE_TEST_LIST_MODELS", "1");

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The provider of that name, as `provider.list` gives it.
fn listed(user: &User, name: &str) -> Value {
    user.call("provider.list", Value::Null).unwrap()["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()
        .clone()
}

fn ids(provider: &Value) -> Vec<String> {
    provider["models"]
        .as_array()
        .map(|models| {
            models
                .iter()
                .map(|m| m["id"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_ready_provider_s_models_are_read_without_a_session() {
    let user = User::with_fake_providers().with_env(ASKS.0, ASKS.1);
    user.write_providers("[opencode]\nenabled = false\n");
    user.provider_script("claude", json!({"models": ["fake-small", "fake-large"]}));

    // The daemon starts with the first call; nobody started an agent.
    assert_eq!(listed(&user, "claude")["ready"], true);
    eventually("the models of Claude", || {
        !ids(&listed(&user, "claude")).is_empty()
    });

    let claude = listed(&user, "claude");
    assert_eq!(ids(&claude), ["fake-small", "fake-large"]);
    assert_eq!(claude["models"][0]["default"], true, "{claude}");
    assert_eq!(claude["models_outdated"], false);
    let agents = user.call("agent.list", Value::Null).unwrap();
    assert_eq!(agents["agents"], json!([]), "no agent was made");
    // OpenCode is disabled: it is not asked.
    assert_eq!(listed(&user, "opencode")["models"], Value::Null);
    assert!(user.provider_received("opencode").is_empty());
}

#[test]
fn listing_the_models_sends_no_prompt() {
    let user = User::with_fake_providers().with_env(ASKS.0, ASKS.1);
    user.provider_script("claude", json!({"models": ["fake-small", "fake-large"]}));
    assert_eq!(listed(&user, "claude")["ready"], true);
    eventually("the models of Claude", || {
        !ids(&listed(&user, "claude")).is_empty()
    });
    // Its process is over: the daemon let it go.
    std::thread::sleep(Duration::from_millis(300));

    let methods: Vec<String> = user
        .provider_received("claude")
        .iter()
        .filter_map(|entry| entry["received"]["method"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(methods, ["initialize", "session/new"], "{methods:?}");
}

#[test]
fn the_models_are_read_again_when_the_program_changes() {
    let user = User::with_fake_claude().with_env(ASKS.0, ASKS.1);
    user.claude_script(json!({"models": [
        {"value": "default", "displayName": "Default", "description": "Opus"},
        {"value": "old", "displayName": "Old", "description": "Old one"}]}));
    eventually("the first models", || {
        ids(&listed(&user, "claude")).contains(&"old".to_owned())
    });

    // A new version of the program: another file, another modification time.
    user.claude_script(json!({"models": [
        {"value": "default", "displayName": "Default", "description": "Opus"},
        {"value": "new", "displayName": "New", "description": "New one"}]}));
    let bin = user.path("bin/claude");
    std::fs::remove_file(&bin).unwrap();
    std::fs::copy(common::fake_claude(), &bin).unwrap();
    assert_eq!(listed(&user, "claude")["models_outdated"], true);

    eventually("the models of the new version", || {
        let claude = listed(&user, "claude");
        ids(&claude).contains(&"new".to_owned()) && claude["models_outdated"] == false
    });
    assert!(!ids(&listed(&user, "claude")).contains(&"old".to_owned()));
    // Still no agent, no prompt.
    let prompts = user
        .claude_received()
        .iter()
        .filter(|entry| entry["received"]["type"] == "user")
        .count();
    assert_eq!(prompts, 0);
}

#[test]
fn a_failed_listing_leaves_the_models_as_they_were() {
    let user = User::with_fake_providers().with_env(ASKS.0, ASKS.1);
    user.provider_script("claude", json!({"models": ["fake-small", "fake-large"]}));
    assert_eq!(listed(&user, "claude")["ready"], true);
    eventually("the models of Claude", || {
        !ids(&listed(&user, "claude")).is_empty()
    });
    assert!(user.run(&["developer", "on"]).status.success());
    assert!(user.run(&["daemon", "stop"]).status.success());

    // The daemon again with another key set, so that the models are read again, and a program
    // that ends at once.
    let user = user
        .with_env("ANTHROPIC_API_KEY", "sk-ant-invented-0123456789")
        .with_env("FLOTTILLE_AGENT_COMMAND_CLAUDE", "/bin/false");
    assert_eq!(listed(&user, "claude")["models_outdated"], true);
    let log = user.path("state/flottille/developer.log");
    eventually("the failure in the developer log", || {
        std::fs::read_to_string(&log).is_ok_and(|text| {
            text.lines().any(|line| {
                serde_json::from_str::<Value>(line)
                    .is_ok_and(|l| l["kind"] == "models" && l["outcome"] == "failed")
            })
        })
    });

    let claude = listed(&user, "claude");
    assert_eq!(ids(&claude), ["fake-small", "fake-large"]);
    assert_eq!(claude["models_outdated"], true, "changed nothing: {claude}");
}
