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

/// A program standing for `claude` that only writes down that it was started: a test's daemon
/// must never start the agents of the machine.
#[test]
fn a_test_daemon_lists_nothing_unless_its_test_asks() {
    // A daemon that reads items, which sets a test variable of its own to nothing.
    let user = User::new().with_fetcher();
    std::fs::create_dir(user.path("bin")).unwrap();
    let marker = user.path("claude-started");
    let program = user.path("bin/claude");
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    std::fs::create_dir_all(user.path("home/.claude")).unwrap();
    std::fs::write(user.path("home/.claude/.credentials.json"), "{}").unwrap();
    // Nothing of the machine's own on its PATH but the system's commands.
    let path = format!("{}:/usr/bin:/bin", user.path("bin").display());
    let user = user.with_env("PATH", &path);

    let projects = user.call("project.list", Value::Null);
    assert!(projects.is_ok(), "{projects:?}");
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !marker.exists(),
        "the daemon started `claude`: {:?}",
        std::fs::read_to_string(&marker)
    );
}

#[test]
fn codex_s_models_are_listed_without_a_thread() {
    let user = User::with_fake_codex().with_env(ASKS.0, ASKS.1);
    user.write_providers("[claude]\nenabled = false\n[codex]\nenabled = true\n");
    assert_eq!(listed(&user, "codex")["ready"], true);
    eventually("the models of Codex", || {
        !ids(&listed(&user, "codex")).is_empty()
    });

    let codex = listed(&user, "codex");
    assert_eq!(ids(&codex), ["gpt-fake", "gpt-fake-mini"], "{codex}");
    assert_eq!(codex["models"][0]["default"], true, "{codex}");
    std::thread::sleep(Duration::from_millis(300));
    let methods: Vec<String> = user
        .codex_received()
        .iter()
        .filter_map(|entry| entry["received"]["method"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        methods,
        ["initialize", "initialized", "account/read", "model/list"],
        "no thread, no turn: {methods:?}"
    );
    assert!(
        std::fs::read_dir(user.path("codex/threads"))
            .map(|threads| threads.count() == 0)
            .unwrap_or(true),
        "a thread was made"
    );
}

#[test]
fn opencode_s_models_are_listed_and_its_empty_session_deleted() {
    let user = User::with_fake_providers().with_env(ASKS.0, ASKS.1);
    user.write_providers("[claude]\nenabled = false\n[opencode]\nenabled = true\n");
    user.provider_script(
        "opencode",
        json!({"models": ["fake-small", "fake-large"], "delete": true}),
    );
    assert_eq!(listed(&user, "opencode")["ready"], true);
    eventually("the models of OpenCode", || {
        !ids(&listed(&user, "opencode")).is_empty()
    });
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(
        ids(&listed(&user, "opencode")),
        ["fake-small", "fake-large"]
    );
    let methods: Vec<String> = user
        .provider_received("opencode")
        .iter()
        .filter_map(|entry| entry["received"]["method"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        methods,
        ["initialize", "session/new", "session/delete"],
        "no prompt: {methods:?}"
    );
    let sessions = std::fs::read_dir(user.path("fake-opencode/sessions"))
        .map(|sessions| sessions.count())
        .unwrap_or(0);
    assert_eq!(sessions, 0, "the empty session stays");
}

#[test]
fn the_listing_leaves_nothing_of_its_process() {
    let user = User::with_fake_claude().with_env(ASKS.0, ASKS.1);
    eventually("the models of Claude", || {
        !ids(&listed(&user, "claude")).is_empty()
    });
    std::thread::sleep(Duration::from_millis(300));

    let left = |sub: &str| -> Vec<String> {
        std::fs::read_dir(user.path(sub))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(left("runtime/flottille/bridge"), Vec::<String>::new());
    assert_eq!(left("state/flottille/bridge/claude"), Vec::<String>::new());
}

#[test]
fn the_models_of_a_program_replaced_while_it_lists_are_not_kept_as_current() {
    let user = User::with_fake_claude().with_env(ASKS.0, ASKS.1);
    // The first program: slow to start, with models of its own in a folder of its own.
    let old = user.path("claude-old");
    std::fs::create_dir(&old).unwrap();
    std::fs::write(
        old.join("script.json"),
        json!({"models": [
            {"value": "default", "displayName": "Default", "description": "Opus"},
            {"value": "old", "displayName": "Old", "description": "Old one"}]})
        .to_string(),
    )
    .unwrap();
    let started = user.path("claude-old-started");
    let bin = user.path("bin/claude");
    std::fs::remove_file(&bin).unwrap();
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version) exec '{fake}' \"$@\";; esac\n: > '{started}'\n\
             sleep 2\nFLOTTILLE_FAKE_CLAUDE_DIR='{old}' exec '{fake}' \"$@\"\n",
            fake = common::fake_claude().display(),
            started = started.display(),
            old = old.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    user.claude_script(json!({"models": [
        {"value": "default", "displayName": "Default", "description": "Opus"},
        {"value": "new", "displayName": "New", "description": "New one"}]}));

    assert_eq!(listed(&user, "claude")["ready"], true);
    eventually("the first listing starts", || started.exists());
    // Replaced while it lists.
    std::fs::remove_file(&bin).unwrap();
    std::fs::copy(common::fake_claude(), &bin).unwrap();

    eventually("the models of the new program", || {
        let claude = listed(&user, "claude");
        assert!(
            !(ids(&claude).contains(&"old".to_owned()) && claude["models_outdated"] == false),
            "the old program's models are taken for the new one's: {claude}"
        );
        ids(&claude).contains(&"new".to_owned()) && claude["models_outdated"] == false
    });
}

#[test]
fn the_efforts_of_every_model_are_learned_once_per_provider_version_without_a_prompt() {
    // The fake `claude`'s models: `default` and `fable` with efforts of their own, `haiku` with
    // none. Its answer to `initialize` says them all; no session ever runs on `fable`.
    let user = User::with_fake_claude().with_env(ASKS.0, ASKS.1);
    eventually("the models of Claude", || {
        ids(&listed(&user, "claude")).contains(&"haiku".to_owned())
    });

    let claude = listed(&user, "claude");
    let efforts = |id: &str| {
        claude["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == id)
            .unwrap()["efforts"]
            .clone()
    };
    assert_eq!(
        efforts("default"),
        json!(["default", "low", "medium", "high", "xhigh"])
    );
    assert_eq!(
        efforts("fable"),
        json!(["default", "low", "medium", "high", "xhigh", "max"]),
        "a model no session ran on has its efforts: {claude}"
    );
    assert_eq!(
        efforts("haiku"),
        json!([]),
        "a model without effort: {claude}"
    );

    // Once per version of the provider: a daemon started again lists nothing more.
    std::thread::sleep(Duration::from_millis(300));
    assert!(user.run(&["daemon", "stop"]).status.success());
    assert_eq!(listed(&user, "claude")["models_outdated"], false);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(user.claude_starts().len(), 1, "one listing");

    // No model was set and nothing was prompted: only `initialize` reached `claude`.
    let received: Vec<Value> = user
        .claude_received()
        .into_iter()
        .filter_map(|entry| entry.get("received").cloned())
        .collect();
    let controls: Vec<&str> = received
        .iter()
        .filter(|m| m["type"] == "control_request")
        .filter_map(|m| m["request"]["subtype"].as_str())
        .collect();
    assert_eq!(controls, ["initialize"], "{received:?}");
    assert!(
        !received.iter().any(|m| m["type"] == "user"),
        "no prompt: {received:?}"
    );
}

/// What `extension.list` says a Review slot left on its default gives, for each ready provider.
fn review_default(user: &User, key: &str) -> Value {
    let listed = user.call("extension.list", json!({})).unwrap();
    let review = listed["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "review")
        .unwrap();
    let slot = review["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap();
    slot["default"].clone()
}

#[test]
fn a_default_slot_says_the_model_the_daemon_picks() {
    let user = User::with_fake_providers().with_env(ASKS.0, ASKS.1);
    user.write_providers("[opencode]\nenabled = false\n");
    user.provider_script("claude", json!({"models": ["opus", "sonnet"]}));
    eventually("the models of Claude", || {
        !ids(&listed(&user, "claude")).is_empty()
    });

    let strongest = review_default(&user, "bugs_model");
    assert_eq!(strongest["rule"], "Strongest of the session's provider");
    assert_eq!(strongest["gives"], json!({"claude": "opus"}));
    let smaller = review_default(&user, "verifier_model");
    assert_eq!(smaller["rule"], "One size smaller");
    assert_eq!(smaller["gives"], json!({"claude": "sonnet"}));
}

#[test]
fn a_default_slot_gives_nothing_where_the_provider_offers_no_such_model() {
    let user = User::with_fake_providers().with_env(ASKS.0, ASKS.1);
    user.write_providers("[opencode]\nenabled = false\n");
    user.provider_script("claude", json!({"models": ["fake-small", "fake-large"]}));
    eventually("the models of Claude", || {
        !ids(&listed(&user, "claude")).is_empty()
    });

    // The provider runs on its own default: the answer names the provider, with no model.
    assert_eq!(
        review_default(&user, "bugs_model")["gives"],
        json!({"claude": null})
    );
}

#[test]
fn a_default_slot_names_only_the_ready_providers() {
    let user = User::with_fake_providers().with_env(ASKS.0, ASKS.1);
    user.write_providers("[opencode]\nenabled = false\n[codex]\nenabled = false\n");
    user.provider_script("claude", json!({"models": ["opus"]}));
    eventually("the models of Claude", || {
        !ids(&listed(&user, "claude")).is_empty()
    });

    let gives = review_default(&user, "bugs_model")["gives"].clone();
    assert_eq!(gives.as_object().unwrap().len(), 1, "{gives}");
}
