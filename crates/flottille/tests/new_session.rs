//! What a new session starts on (`docs/agents.md`, "The options"): the provider, model and effort
//! the user chose for it in `providers.toml`, below the project's own and above what the provider
//! proposes. No real agent runs.

use crate::common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::Mcp;
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

    // The lead runs on another model and effort than the setting's: the child's can only come
    // from the setting.
    let lead = started(
        &user,
        json!({"message": "LEAD go", "model": "fake-small", "effort": "low"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let shown = agent(&user, &lead);
    assert_eq!(shown["chosen"]["model"], "fake-small");
    assert_eq!(shown["chosen"]["effort"], "low");
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

// ── The mode a new session starts in (#1325) ─────────────────────────────────────────────────

fn set_mode(user: &User, mode: &str) {
    user.call("provider.set_new_session_mode", json!({"mode": mode}))
        .unwrap();
}

fn default_mode(user: &User) -> Value {
    user.call("provider.list", Value::Null).unwrap()["new_session_mode"].clone()
}

#[test]
fn a_new_session_starts_in_the_default_mode() {
    let user = user();
    // Until the user chooses, Auto, as before.
    assert_eq!(default_mode(&user), "auto");
    assert_eq!(started(&user, json!({}))["gate_mode"], "auto");

    for mode in ["ask", "full", "auto"] {
        set_mode(&user, mode);
        assert_eq!(default_mode(&user), mode);
        assert_eq!(started(&user, json!({}))["gate_mode"], mode, "{mode}");
    }

    // What the session asks for itself (`flottille agent start --mode`, the composer's control)
    // wins over the default.
    set_mode(&user, "full");
    assert_eq!(
        started(&user, json!({"gate_mode": "ask"}))["gate_mode"],
        "ask"
    );
    // A mode that is none is refused, and the default stays.
    let (code, _) = user
        .call("provider.set_new_session_mode", json!({"mode": "yolo"}))
        .unwrap_err();
    assert_eq!(code, "invalid-params");
    assert_eq!(default_mode(&user), "full");
}

#[test]
fn the_default_mode_is_kept_per_machine() {
    let here = user();
    let there = user();
    // The choices beside it in the file are kept as the user wrote them.
    set(&here, "opencode", Some("open/medium"), None);
    set_mode(&here, "full");

    let text = std::fs::read_to_string(here.path("config/flottille/providers.toml")).unwrap();
    assert!(text.contains("new_session_mode = \"full\""), "{text}");
    assert!(
        text.contains("[new_session]\nprovider = \"opencode\""),
        "{text}"
    );
    assert_eq!(default_mode(&here), "full");
    // Another machine has its own.
    assert_eq!(default_mode(&there), "auto");
    set_mode(&there, "ask");
    assert_eq!(default_mode(&here), "full");
    assert_eq!(default_mode(&there), "ask");
    assert_eq!(started(&here, json!({}))["gate_mode"], "full");
    assert_eq!(started(&there, json!({}))["gate_mode"], "ask");
    // Choosing the provider again, or clearing it, leaves the mode alone.
    user_clear(&here);
    assert_eq!(default_mode(&here), "full");
    // Auto is what none says: the file does not keep it.
    set_mode(&here, "auto");
    let text = std::fs::read_to_string(here.path("config/flottille/providers.toml")).unwrap();
    assert!(!text.contains("new_session_mode"), "{text}");
}

fn user_clear(user: &User) {
    user.call("provider.set_new_session", json!({"provider": null}))
        .unwrap();
}

#[test]
fn a_child_still_follows_its_tree_s_mode() {
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
    // The tree's root starts in Ask; the default is Full by the time its child is launched.
    set_mode(&user, "ask");
    let lead = started(&user, json!({"message": "LEAD go"}))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    set_mode(&user, "full");
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
    assert_eq!(child["gate_mode"], "ask", "{child}");
    // And a session the user starts next is in the default.
    assert_eq!(started(&user, json!({}))["gate_mode"], "full");
}

#[test]
fn the_provider_command_sets_the_mode_a_new_session_starts_in() {
    let user = user();
    let said = |args: &[&str]| {
        let output = user.run(args);
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert!(said(&["provider", "list"]).contains("New session mode: Auto.\n"));

    assert_eq!(
        said(&["provider", "new-session", "--mode", "full"]),
        "New sessions start in Full.\n"
    );
    assert!(said(&["provider", "list"]).contains("New session mode: Full.\n"));
    assert_eq!(default_mode(&user), "full");
    // With a provider, both are set; without `--mode`, the mode stays.
    assert_eq!(
        said(&["provider", "new-session", "claude", "--mode", "ask"]),
        "New sessions start on claude, the model and the effort it proposes.\n\
         New sessions start in Ask.\n"
    );
    assert_eq!(
        said(&["provider", "new-session", "opencode"]),
        "New sessions start on opencode, the model and the effort it proposes.\n"
    );
    assert_eq!(default_mode(&user), "ask");
    assert_eq!(
        said(&["provider", "new-session", "--clear"]),
        "New sessions start on what the provider proposes.\n"
    );
    assert_eq!(default_mode(&user), "ask");
    let output = user.run(&["provider", "new-session", "--mode", "yolo"]);
    assert!(!output.status.success());
    assert_eq!(default_mode(&user), "ask");
}

#[test]
fn a_session_an_outside_agent_starts_through_the_pilot_door_is_in_auto_whatever_the_default() {
    let user = user();
    let pilot = Mcp::pilot(&user);
    let mut known: Vec<String> = Vec::new();
    for mode in ["full", "ask"] {
        set_mode(&user, mode);

        let answer = pilot.call("agent_start", json!({"project": "app", "message": "go"}));
        assert!(!answer.error, "{}", answer.text);

        // Only a session the user starts takes the default: an agent's door keeps Auto.
        let listed = user.call("agent.list", Value::Null).unwrap()["agents"].clone();
        let fresh: Vec<&str> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .filter(|id| !known.iter().any(|k| k == id))
            .collect();
        assert_eq!(fresh.len(), 1, "{listed}");
        assert_eq!(agent(&user, fresh[0])["gate_mode"], "auto", "{mode}");
        known.push(fresh[0].to_owned());
    }
}
