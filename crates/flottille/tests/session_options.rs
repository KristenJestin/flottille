//! A running session's model and effort changed between turns (`docs/agents.md`, "The options"),
//! against the fake agent, which records the `session/set_config_option` it receives. No real
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

/// Starts an agent in `app`; its id and the event number to follow it from.
fn run(user: &User, params: Value) -> (String, u64) {
    let mut params = params;
    params["project"] = "app".into();
    let answer = user.call("agent.run", params).unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

fn of(agent: &str) -> impl Fn(&Value) -> bool + '_ {
    move |event: &Value| event["data"]["agent"] == agent
}

/// The agent's events from `after`, up to the first one named `name`.
fn events_until(user: &User, agent: &str, after: u64, name: &str) -> Vec<Value> {
    let watch = user.watch(&["--after", &after.to_string()]);
    watch
        .until(|e| of(agent)(e) && e["name"] == name)
        .into_iter()
        .filter(of(agent))
        .collect()
}

fn show(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone()
}

/// What the fake agent received, in order, as `method` or `method:configId=value`, less the
/// handshake.
fn received(user: &User) -> Vec<String> {
    user.received()
        .iter()
        .filter_map(|entry| {
            let message = &entry["received"];
            let method = message["method"].as_str()?;
            Some(match method {
                "session/set_config_option" => format!(
                    "set:{}={}",
                    message["params"]["configId"].as_str()?,
                    message["params"]["value"].as_str()?
                ),
                "session/prompt" => "prompt".to_owned(),
                _ => return None,
            })
        })
        .collect()
}

const TWO_MODELS: fn() -> Value =
    || json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]});

#[test]
fn a_running_session_s_model_is_changed_before_its_next_prompt() {
    let user = User::with_fake_agent();
    user.fake_script(TWO_MODELS());
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let asked = user
        .call(
            "agent.options",
            json!({"agent": id, "model": "fake-large", "effort": "high"}),
        )
        .unwrap();
    events_until(
        &user,
        &id,
        asked["after"].as_u64().unwrap(),
        "agent.options_taken",
    );
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );

    let order = received(&user);
    assert_eq!(
        order[order.len() - 3..],
        ["set:model=fake-large", "set:effort=high", "prompt"],
        "{order:?}"
    );
    let agent = show(&user, &id);
    assert_eq!(agent["chosen"]["model"], "fake-large");
    assert_eq!(agent["chosen"]["effort"], "high");
    assert_eq!(agent["taken"]["model"], "fake-large");
    assert_eq!(agent["taken"]["effort"], "high");
    assert_eq!(agent["pending"]["model"], Value::Null);
}

#[test]
fn a_change_chosen_during_a_turn_waits_for_its_end() {
    let user = User::with_fake_agent();
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "turns": [{"when": "slow", "do": [{"hold": "release"}, {"say": "done"}]}]
    }));
    project(&user);
    let (id, after) = run(&user, json!({"message": "slow"}));
    events_until(&user, &id, after, "agent.prompted");

    let asked = user
        .call("agent.options", json!({"agent": id, "model": "fake-large"}))
        .unwrap();

    assert_eq!(asked["agent"]["pending"]["model"], "fake-large");
    assert_eq!(show(&user, &id)["pending"]["model"], "fake-large");
    assert!(
        !received(&user).contains(&"set:model=fake-large".to_owned()),
        "set during the turn: {:?}",
        received(&user)
    );
    std::fs::write(user.path("fake/release"), "").unwrap();
    let events = events_until(&user, &id, after, "agent.options_taken");
    let names: Vec<&str> = events.iter().map(|e| e["name"].as_str().unwrap()).collect();
    let ended = names.iter().position(|n| *n == "agent.turn_ended").unwrap();
    let pending = names
        .iter()
        .position(|n| *n == "agent.options_pending")
        .unwrap();
    assert!(pending < ended && ended < names.len() - 1, "{names:?}");
    assert_eq!(
        received(&user).last().map(String::as_str),
        Some("set:model=fake-large")
    );
    let agent = show(&user, &id);
    assert_eq!(agent["pending"]["model"], Value::Null);
    assert_eq!(agent["chosen"]["model"], "fake-large");
}

#[test]
fn a_model_the_provider_does_not_offer_is_refused_naming_what_it_offers() {
    let user = User::with_fake_agent();
    user.fake_script(TWO_MODELS());
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let (code, message) = user
        .call("agent.options", json!({"agent": id, "model": "fake-huge"}))
        .unwrap_err();
    let (_, effort) = user
        .call("agent.options", json!({"agent": id, "effort": "max"}))
        .unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("fake-huge") && message.contains("fake-small, fake-large"),
        "{message}"
    );
    assert!(
        effort.contains("max") && effort.contains("low, high"),
        "{effort}"
    );
    assert_eq!(show(&user, &id)["pending"]["model"], Value::Null);
}

#[test]
fn a_model_the_agent_does_not_offer_is_refused_and_the_agent_goes_on() {
    let user = User::with_fake_agent();
    // The provider lists `fake-large`, and the session refuses to take it.
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "refuse": ["fake-large"],
        "turns": [{"when": "again", "do": [{"say": "Still here."}]}]
    }));
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let asked = user
        .call("agent.options", json!({"agent": id, "model": "fake-large"}))
        .unwrap();
    let refused = events_until(
        &user,
        &id,
        asked["after"].as_u64().unwrap(),
        "agent.options_refused",
    );
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    let second = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );

    let reason = refused.last().unwrap()["data"]["reason"].as_str().unwrap();
    assert!(reason.contains("fake-large"), "{reason}");
    assert!(second.iter().any(|e| e["data"]["text"] == "Still here."));
    let agent = show(&user, &id);
    assert_eq!(agent["state"], "idle");
    assert_eq!(
        agent["chosen"]["model"],
        Value::Null,
        "the previous options stay"
    );
    assert_eq!(
        agent["pending"]["model"],
        Value::Null,
        "the pending mark is cleared"
    );
}

#[test]
fn the_new_options_are_kept_for_the_agent_s_next_process() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "400");
    user.fake_script(TWO_MODELS());
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");
    let asked = user
        .call(
            "agent.options",
            json!({"agent": id, "model": "fake-large", "effort": "high"}),
        )
        .unwrap();
    events_until(
        &user,
        &id,
        asked["after"].as_u64().unwrap(),
        "agent.released",
    );

    let sent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );

    let order = received(&user);
    assert_eq!(
        order[order.len() - 3..],
        ["set:model=fake-large", "set:effort=high", "prompt"],
        "the second process opens with the new options: {order:?}"
    );
    let sets = order
        .iter()
        .filter(|r| *r == "set:model=fake-large")
        .count();
    assert_eq!(sets, 2, "once for each process: {order:?}");
    assert_eq!(show(&user, &id)["chosen"]["model"], "fake-large");
}

#[test]
fn a_released_agent_keeps_the_change_pending_until_its_next_message() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    user.fake_script(TWO_MODELS());
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.released");

    let asked = user
        .call("agent.options", json!({"agent": id, "model": "fake-large"}))
        .unwrap();

    assert_eq!(asked["agent"]["pending"]["model"], "fake-large");
    assert_eq!(show(&user, &id)["state"], "idle");
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );
    let agent = show(&user, &id);
    assert_eq!(agent["chosen"]["model"], "fake-large");
    assert_eq!(agent["pending"]["model"], Value::Null);
}

#[test]
fn the_command_line_changes_the_options_and_shows_them() {
    let user = User::with_fake_agent();
    user.fake_script(TWO_MODELS());
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let output = user.run(&[
        "agent",
        "options",
        &id,
        "--model",
        "fake-large",
        "--effort",
        "high",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    events_until(&user, &id, after, "agent.options_taken");
    let shown = user.run(&["agent", "show", &id]);
    let shown = String::from_utf8_lossy(&shown.stdout).into_owned();
    let bare = user.run(&["agent", "options", &id]);

    assert!(stdout.contains("fake-large"), "{stdout}");
    assert!(
        shown.contains("fake-large") && shown.contains("high"),
        "{shown}"
    );
    assert!(!bare.status.success(), "nothing to change is refused");
}

/// Waits until the fake agent received `entry` (as `received` writes it).
fn until_received(user: &User, entry: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !received(user).iter().any(|r| r == entry) {
        assert!(
            std::time::Instant::now() < deadline,
            "{entry} never received"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn names(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["name"].as_str().unwrap()).collect()
}

#[test]
fn an_effort_chosen_while_the_model_is_being_set_is_set_after_it() {
    let user = User::with_fake_agent();
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "efforts": ["low", "high"],
        "set_wait": 1000
    }));
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let asked = user
        .call("agent.options", json!({"agent": id, "model": "fake-large"}))
        .unwrap();
    until_received(&user, "set:model=fake-large");
    user.call("agent.options", json!({"agent": id, "effort": "high"}))
        .unwrap();
    let watch = user.watch(&["--after", &asked["after"].as_u64().unwrap().to_string()]);
    watch.until(|e| {
        of(&id)(e) && e["name"] == "agent.options_taken" && e["data"]["effort"] == "high"
    });

    assert!(
        received(&user).contains(&"set:effort=high".to_owned()),
        "{:?}",
        received(&user)
    );
    let agent = show(&user, &id);
    assert_eq!(agent["chosen"]["model"], "fake-large");
    assert_eq!(agent["chosen"]["effort"], "high");
    assert_eq!(agent["pending"]["effort"], Value::Null);
}

#[test]
fn two_changes_asked_at_once_are_both_kept() {
    let user = User::with_fake_agent();
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "efforts": ["low", "high"],
        "turns": [{"when": "slow", "do": [{"wait": 60000}]}]
    }));
    project(&user);
    let (id, after) = run(&user, json!({"message": "slow"}));
    events_until(&user, &id, after, "agent.prompted");

    for (model, effort) in [("fake-large", "high"), ("fake-small", "low")].repeat(5) {
        std::thread::scope(|scope| {
            scope.spawn(|| {
                user.call("agent.options", json!({"agent": id, "model": model}))
                    .unwrap()
            });
            scope.spawn(|| {
                user.call("agent.options", json!({"agent": id, "effort": effort}))
                    .unwrap()
            });
        });

        let pending = &show(&user, &id)["pending"];
        assert_eq!(pending["model"], model, "{pending}");
        assert_eq!(pending["effort"], effort, "{pending}");
    }
}

#[test]
fn a_model_the_agent_replaces_with_another_is_recorded_as_taken() {
    let user = User::with_fake_agent();
    user.fake_script(json!({
        "models": ["fake-small", "fake-large", "fake-medium"],
        "takes": {"fake-large": "fake-medium"}
    }));
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let asked = user
        .call("agent.options", json!({"agent": id, "model": "fake-large"}))
        .unwrap();
    let events = events_until(
        &user,
        &id,
        asked["after"].as_u64().unwrap(),
        "agent.options_taken",
    );

    assert_eq!(events.last().unwrap()["data"]["model"], "fake-medium");
    let agent = show(&user, &id);
    assert_eq!(agent["taken"]["model"], "fake-medium");
    assert_eq!(agent["chosen"]["model"], "fake-medium");
    assert_eq!(agent["pending"]["model"], Value::Null);
}

#[test]
fn a_change_the_agent_could_not_answer_stays_pending_and_is_no_refusal() {
    let user = User::with_fake_agent();
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "broken": ["fake-large"],
        "turns": [{"when": "again", "do": [{"say": "Still here."}]}]
    }));
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let asked = user
        .call("agent.options", json!({"agent": id, "model": "fake-large"}))
        .unwrap();
    until_received(&user, "set:model=fake-large");
    user.call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    let events = events_until(
        &user,
        &id,
        asked["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );

    assert!(
        !names(&events).contains(&"agent.options_refused"),
        "{:?}",
        names(&events)
    );
    assert!(events.iter().any(|e| e["data"]["text"] == "Still here."));
    let agent = show(&user, &id);
    assert_eq!(agent["state"], "idle");
    assert_eq!(agent["pending"]["model"], "fake-large", "kept for a retry");
    let sets = received(&user)
        .iter()
        .filter(|r| *r == "set:model=fake-large")
        .count();
    assert!(
        sets >= 2,
        "retried before the next prompt: {:?}",
        received(&user)
    );
}

#[test]
fn a_pending_effort_the_new_model_lacks_is_dropped_with_a_word() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "efforts": ["low", "high", "max"],
        "model_efforts": {"fake-large": ["low"]}
    }));
    project(&user);
    // A session on each model: the provider knows the efforts of both.
    let (large, after) = run(&user, json!({"message": "hello", "model": "fake-large"}));
    events_until(&user, &large, after, "agent.turn_ended");
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.released");
    user.call("agent.options", json!({"agent": id, "effort": "max"}))
        .unwrap();

    let asked = user
        .call("agent.options", json!({"agent": id, "model": "fake-large"}))
        .unwrap();
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    let events = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );

    let dropped = asked["dropped"].as_str().unwrap_or_default();
    assert!(
        dropped.contains("max") && dropped.contains("fake-large"),
        "{dropped}"
    );
    assert_eq!(asked["agent"]["pending"]["effort"], Value::Null);
    assert!(
        !names(&events).contains(&"agent.options_refused"),
        "{:?}",
        names(&events)
    );
    assert!(!received(&user).contains(&"set:effort=max".to_owned()));
    assert_eq!(show(&user, &id)["taken"]["model"], "fake-large");
}

#[test]
fn a_pending_model_replaces_a_chosen_one_the_agent_no_longer_offers() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    user.fake_script(TWO_MODELS());
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello", "model": "fake-large"}));
    events_until(&user, &id, after, "agent.released");
    let asked = user
        .call("agent.options", json!({"agent": id, "model": "fake-small"}))
        .unwrap();
    assert_eq!(asked["agent"]["pending"]["model"], "fake-small");
    // The agent's catalogue lost the model its process was started on.
    user.fake_script(json!({"models": ["fake-small"], "efforts": ["low", "high"]}));

    let sent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    let events = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );

    assert!(
        !names(&events).contains(&"agent.failed"),
        "{:?}",
        names(&events)
    );
    let agent = show(&user, &id);
    assert_eq!(agent["state"], "idle");
    assert_eq!(agent["chosen"]["model"], "fake-small");
    assert_eq!(agent["pending"]["model"], Value::Null);
}
