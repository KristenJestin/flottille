//! What a new session starts on (`docs/agents.md`, "The options"): the provider, model and effort
//! the user chose for it in `providers.toml`, below the project's own and above what the provider
//! proposes. No real agent runs.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

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

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn agent(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

/// A session started with nothing asked, once it is at rest.
fn started(user: &User, params: Value) -> Value {
    let mut params = params;
    params["project"] = "app".into();
    if params.get("message").is_none() {
        params["message"] = "hello".into();
    }
    let id = user.call("agent.start", params).unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the session", || {
        let now = agent(user, &id);
        if now["state"] == "failed" || now["state"] == "stopped" {
            panic!("{now}");
        }
        now["state"] == "idle"
    });
    agent(user, &id)
}

/// A user whose Claude and OpenCode both run on a fake agent, each with its own models.
fn user() -> User {
    let user = User::with_fake_providers();
    user.write_providers("[opencode]\nenabled = true\n");
    user.provider_script(
        "claude",
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}),
    );
    user.provider_script(
        "opencode",
        json!({"models": ["open/small", "open/medium"], "efforts": ["low", "high"]}),
    );
    project(&user);
    user
}

fn set(user: &User, provider: &str, model: Option<&str>, effort: Option<&str>) {
    user.call(
        "provider.set_new_session",
        json!({"provider": provider, "model": model, "effort": effort}),
    )
    .unwrap();
}

#[test]
fn by_default_a_new_session_starts_on_what_the_provider_proposes() {
    let user = user();
    assert_eq!(
        user.call("provider.list", Value::Null).unwrap()["new_session"],
        Value::Null
    );

    let session = started(&user, json!({}));

    assert_eq!(session["provider"], "claude");
    assert_eq!(session["chosen"]["model"], Value::Null);
    assert_eq!(session["chosen"]["effort"], Value::Null);
}

#[test]
fn a_chosen_provider_model_and_effort_are_what_a_new_session_starts_on() {
    let user = user();
    // The models are known once a session of each ran.
    started(&user, json!({}));
    started(&user, json!({"provider": "opencode"}));

    set(&user, "opencode", Some("open/medium"), Some("high"));

    let text = std::fs::read_to_string(user.path("config/flottille/providers.toml")).unwrap();
    assert!(
        text.contains(
            "[new_session]\nprovider = \"opencode\"\nmodel = \"open/medium\"\neffort = \"high\""
        ),
        "{text}"
    );
    assert_eq!(
        user.call("provider.list", Value::Null).unwrap()["new_session"],
        json!({"provider": "opencode", "model": "open/medium", "effort": "high"})
    );
    let session = started(&user, json!({}));
    assert_eq!(session["provider"], "opencode");
    assert_eq!(session["chosen"]["model"], "open/medium");
    assert_eq!(session["chosen"]["effort"], "high");
    // What a session asks for itself wins, and the effort chosen for another model is not its.
    let asked = started(&user, json!({"model": "open/small"}));
    assert_eq!(asked["chosen"]["model"], "open/small");
    assert_eq!(asked["chosen"]["effort"], Value::Null);
    let other = started(&user, json!({"provider": "claude"}));
    assert_eq!(other["provider"], "claude");
    assert_eq!(other["chosen"]["model"], Value::Null, "another provider's");
    // The provider alone: its own model and effort are left to it. Clearing goes back.
    set(&user, "opencode", None, None);
    let bare = started(&user, json!({}));
    assert_eq!(bare["provider"], "opencode");
    assert_eq!(bare["chosen"]["model"], Value::Null);
    user.call("provider.set_new_session", json!({"provider": null}))
        .unwrap();
    assert!(
        !std::fs::read_to_string(user.path("config/flottille/providers.toml"))
            .unwrap()
            .contains("new_session")
    );
    assert_eq!(started(&user, json!({}))["provider"], "claude");
    // A provider Flottille does not know is refused.
    let (code, _) = user
        .call("provider.set_new_session", json!({"provider": "gemini"}))
        .unwrap_err();
    assert_eq!(code, "not-found");
}

#[test]
fn a_project_s_model_wins_over_the_setting() {
    let user = user();
    started(&user, json!({"provider": "opencode"}));
    set(&user, "opencode", Some("open/small"), Some("high"));
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!("provider = \"opencode\"\nmodel = \"open/medium\"\n{text}"),
    )
    .unwrap();

    let session = started(&user, json!({}));

    assert_eq!(session["provider"], "opencode");
    assert_eq!(session["chosen"]["model"], "open/medium");
    assert_eq!(
        session["chosen"]["effort"],
        Value::Null,
        "the effort chosen went with another model"
    );
    // A project that gives its provider only leaves the model to the setting.
    std::fs::write(&file, format!("provider = \"opencode\"\n{text}")).unwrap();
    let only = started(&user, json!({}));
    assert_eq!(only["chosen"]["model"], "open/small");
    assert_eq!(only["chosen"]["effort"], "high");
}

#[test]
fn a_model_no_longer_offered_falls_back_to_what_the_provider_proposes() {
    let user = user();
    started(&user, json!({}));
    set(&user, "claude", Some("fake-gone"), Some("high"));

    let session = started(&user, json!({}));

    assert_eq!(session["chosen"]["model"], Value::Null);
    assert_eq!(session["chosen"]["effort"], Value::Null);
    // An effort the model does not offer (its efforts are known once a session ran on it): the
    // model stays, the effort goes.
    started(&user, json!({"model": "fake-large"}));
    set(&user, "claude", Some("fake-large"), Some("extreme"));
    let kept = started(&user, json!({}));
    assert_eq!(kept["chosen"]["model"], "fake-large");
    assert_eq!(kept["chosen"]["effort"], Value::Null);
    // A request of the session itself is refused, not bent.
    let (code, _) = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "hello", "model": "fake-gone"}),
        )
        .unwrap_err();
    assert_eq!(code, "refused");
}

#[test]
fn an_agent_launch_without_model_or_effort_follows_the_new_session_setting() {
    let user = user();
    user.provider_script(
        "claude",
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"], "turns": [
            {"when": "LEAD", "do": [
                {"mcp": {"tool": "agent_launch", "arguments": {"kind": "child",
                    "title": "other", "brief": "CHILD work"}}}
            ]}
        ]}),
    );
    started(&user, json!({}));
    set(&user, "claude", Some("fake-large"), Some("high"));

    let lead = started(&user, json!({"message": "LEAD go"}))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut child = None;
    eventually("the child", || {
        child = user.call("agent.list", Value::Null).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["parent"] == lead.as_str())
            .cloned();
        child.is_some()
    });
    let child = agent(&user, child.unwrap()["id"].as_str().unwrap());

    assert_eq!(child["chosen"]["model"], "fake-large");
    assert_eq!(child["chosen"]["effort"], "high");
}

#[test]
fn the_provider_command_reads_and_sets_what_a_new_session_starts_on() {
    let user = user();
    let said = |args: &[&str]| {
        let output = user.run(args);
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert!(said(&["provider", "list"]).ends_with("New sessions: what the provider proposes.\n"));

    assert_eq!(
        said(&[
            "provider",
            "new-session",
            "opencode",
            "--model",
            "open/medium",
            "--effort",
            "high"
        ]),
        "New sessions start on opencode, open/medium, high.\n"
    );
    assert!(said(&["provider", "list"]).ends_with("New sessions: opencode, open/medium, high.\n"));
    assert_eq!(
        said(&["provider", "new-session", "claude"]),
        "New sessions start on claude, the model and the effort it proposes.\n"
    );
    assert_eq!(
        said(&["provider", "new-session", "--clear"]),
        "New sessions start on what the provider proposes.\n"
    );
    let output = user.run(&["provider", "new-session", "gemini"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("there is no provider gemini"),
        "{output:?}"
    );
}
