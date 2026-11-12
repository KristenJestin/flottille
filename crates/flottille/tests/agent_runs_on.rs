//! What an agent runs on is said, by name, in every place an agent reads about another (#786,
//! `docs/tools.md`, "What an agent runs on"): `agent_launch`'s answer, `agent_status`, what a
//! pilot reads of its led session, and `agent_send` when it changes the model or the effort. The
//! fake ACP agent, temporary folders, no real agent.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// A fake agent whose models carry a name of their own, as a provider's list names them.
fn script(turns: Value) -> Value {
    json!({
        "models": ["fake-small", "fake-large"],
        "model_rows": {
            "fake-small": {"name": "Fake Small 5.5"},
            "fake-large": {"name": "Fake Large 5.5"}
        },
        "efforts": ["low", "medium", "high"],
        "turns": turns
    })
}

/// A project `app` over a throwaway repository, and a root of it in Full (nothing is judged
/// here), its first turn ended: its id and its MCP session.
fn lead(user: &User) -> (String, Mcp) {
    lead_on(user, None)
}

/// The same, on the fake agent standing for `provider` (`with_fake_providers`).
fn lead_on(user: &User, provider: Option<&str>) -> (String, Mcp) {
    let opened_by = |user: &User| match provider {
        Some(provider) => user
            .provider_received(provider)
            .into_iter()
            .filter(|entry| entry["received"]["method"] == "session/new")
            .map(|entry| entry["received"].clone())
            .collect::<Vec<_>>(),
        None => user.received_requests("session/new"),
    };
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("its session", || !opened_by(user).is_empty());
    let (url, authorization) = match provider {
        Some(_) => {
            let server = opened_by(user)[0]["params"]["mcpServers"][0].clone();
            let authorization = server["headers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|h| h["name"] == "Authorization")
                .and_then(|h| h["value"].as_str())
                .unwrap()
                .to_owned();
            (server["url"].as_str().unwrap().to_owned(), authorization)
        }
        None => server_of(user, 0),
    };
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    eventually("its first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    (id, Mcp::initialize(&url, &authorization))
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn agent_of(user: &User, title: &str) -> Value {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == title)
        .unwrap()
        .clone()
}

/// Launches `arguments` as `title` and waits until its agent is at rest: its id.
fn launched(user: &User, mcp: &Mcp, title: &str, arguments: Value) -> (String, String) {
    let mut arguments = arguments;
    arguments["title"] = title.into();
    arguments["brief"] = "Do the work.".into();
    let answer = mcp.call("agent_launch", arguments);
    assert!(!answer.error, "{}", answer.text);
    let id = agent_of(user, title)["id"].as_str().unwrap().to_owned();
    eventually("the agent at rest", || {
        let state =
            user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"].clone();
        assert!(state != "failed", "{title} failed");
        state == "idle"
    });
    (id, answer.text)
}

/// The deliveries of `kind` stored for `agent`, newest first.
fn delivered(user: &User, agent: &str, kind: &str) -> Vec<String> {
    let store =
        flottille_core::store::Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    store
        .last_deliveries(agent, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == kind)
        .map(|delivery| delivery.body)
        .collect()
}

fn says_no_default(text: &str) {
    assert!(!text.to_lowercase().contains("default"), "{text}");
}

#[test]
fn agent_launch_says_the_model_and_effort_taken() {
    let user = User::with_fake_agent();
    user.fake_script(script(json!([])));
    let (_, mcp) = lead(&user);

    let (_, child) = launched(
        &user,
        &mcp,
        "worker",
        json!({"kind": "child", "model": "fake-large", "effort": "medium"}),
    );
    let (_, led) = launched(
        &user,
        &mcp,
        "Login",
        json!({"kind": "led", "model": "fake-small", "effort": "high"}),
    );
    let (_, handed) = launched(
        &user,
        &mcp,
        "Docs",
        json!({"kind": "handed", "model": "fake-small", "effort": "low"}),
    );

    assert!(
        child.contains("on Claude · Fake Large 5.5 · medium"),
        "{child}"
    );
    assert!(led.contains("on Claude · Fake Small 5.5 · high"), "{led}");
    assert!(
        handed.contains("on Claude · Fake Small 5.5 · low"),
        "{handed}"
    );
}

#[test]
fn a_launch_without_model_or_effort_says_the_ones_taken_by_name() {
    let user = User::with_fake_agent();
    user.fake_script(script(json!([])));
    // The models are known once a session of the provider ran: the lead's.
    let (_, mcp) = lead(&user);

    let (id, answer) = launched(&user, &mcp, "worker", json!({"kind": "child"}));

    says_no_default(&answer);
    let said = answer
        .split(" on ")
        .nth(1)
        .and_then(|rest| rest.split(", in ").next())
        .unwrap_or_default()
        .to_owned();
    assert!(said.starts_with("Claude · "), "{answer}");
    // What the answer said is what the agent then took.
    let taken = user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone();
    let model = taken["taken"]["model"].as_str().unwrap().to_owned();
    let effort = taken["taken"]["effort"].as_str().unwrap().to_owned();
    let named = |id: &str| match id {
        "fake-small" => "Fake Small 5.5".to_owned(),
        "fake-large" => "Fake Large 5.5".to_owned(),
        other => other.to_owned(),
    };
    assert!(
        said.starts_with(&format!("Claude · {} · {effort}", named(&model))),
        "{answer}: the child took {model} on {effort}"
    );
}

#[test]
fn agent_status_says_each_child_s_model_and_effort() {
    let user = User::with_fake_agent();
    user.fake_script(script(json!([])));
    let (_, mcp) = lead(&user);
    launched(
        &user,
        &mcp,
        "quick",
        json!({"kind": "child", "model": "fake-small", "effort": "low"}),
    );
    launched(
        &user,
        &mcp,
        "deep",
        json!({"kind": "child", "model": "fake-large", "effort": "high"}),
    );

    for arguments in [
        json!({}),
        json!({"detail": "full"}),
        json!({"detail": "short"}),
    ] {
        let status = mcp.call("agent_status", arguments.clone());
        assert!(!status.error, "{}", status.text);
        let line = |title: &str| {
            status
                .text
                .lines()
                .find(|line| line.contains(&format!("\"{title}\"")))
                .unwrap_or_else(|| panic!("{arguments}: no line of {title}: {}", status.text))
                .to_owned()
        };
        assert!(
            line("quick").contains("Claude · Fake Small 5.5 · low"),
            "{arguments}: {}",
            line("quick")
        );
        assert!(
            line("deep").contains("Claude · Fake Large 5.5 · high"),
            "{arguments}: {}",
            line("deep")
        );
    }
    let one = mcp.call(
        "agent_status",
        json!({"agent_id": agent_of(&user, "deep")["id"]}),
    );
    assert!(
        one.text.contains("Claude · Fake Large 5.5 · high"),
        "{}",
        one.text
    );
}

#[test]
fn a_led_session_s_status_says_its_model_and_effort() {
    let user = User::with_fake_agent();
    user.fake_script(script(json!([
        {"when": "Fix the login.", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "Login fixed."}}},
            {"say": "Reported."}]}
    ])));
    let (lead, mcp) = lead(&user);

    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "led", "title": "Login", "brief": "Fix the login.",
            "model": "fake-large", "effort": "high"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("on Claude · Fake Large 5.5 · high"),
        "{}",
        answer.text
    );
    let session = agent_of(&user, "Login")["id"].as_str().unwrap().to_owned();

    // What wakes the pilot says what the session runs on.
    eventually("the report's wake", || {
        !delivered(&user, &lead, "session-turn-ended").is_empty()
    });
    let woken = delivered(&user, &lead, "session-turn-ended").remove(0);
    assert!(woken.contains("Claude · Fake Large 5.5 · high"), "{woken}");
    // What it reads of the session says it too.
    let read = mcp.call("session_read", json!({"session": session}));
    assert!(!read.error, "{}", read.text);
    assert!(
        read.text.contains("Claude · Fake Large 5.5 · high"),
        "{}",
        read.text
    );
    // And once the user changed it, the next wake says the new one.
    user.call("agent.options", json!({"agent": session, "effort": "low"}))
        .unwrap();
    eventually("the effort taken", || {
        user.call("agent.show", json!({"agent": session})).unwrap()["agent"]["taken"]["effort"]
            == "low"
    });
    let read = mcp.call("session_read", json!({"session": session}));
    assert!(
        read.text.contains("Claude · Fake Large 5.5 · low"),
        "{}",
        read.text
    );
    user.call("agent.stop", json!({"agent": session})).unwrap();
    eventually("the end's wake", || {
        delivered(&user, &lead, "session-turn-ended")
            .iter()
            .any(|woken| woken.contains("Your following of it is over"))
    });
    let ended = delivered(&user, &lead, "session-turn-ended")
        .into_iter()
        .find(|woken| woken.contains("Your following of it is over"))
        .unwrap();
    assert!(ended.contains("Claude · Fake Large 5.5 · low"), "{ended}");
}

#[test]
fn agent_send_that_changes_the_effort_says_the_new_one() {
    let user = User::with_fake_agent();
    user.fake_script(script(json!([])));
    let (_, mcp) = lead(&user);
    let (id, _) = launched(
        &user,
        &mcp,
        "worker",
        json!({"kind": "child", "model": "fake-large", "effort": "high"}),
    );

    let effort = mcp.call(
        "agent_send",
        json!({"agent_id": id, "message": "Go on.", "effort": "low"}),
    );
    assert!(!effort.error, "{}", effort.text);
    assert!(
        effort.text.contains("Claude · Fake Large 5.5 · low"),
        "{}",
        effort.text
    );
    let model = mcp.call(
        "agent_send",
        json!({"agent_id": id, "message": "And on.", "model": "fake-small"}),
    );
    assert!(!model.error, "{}", model.text);
    assert!(
        model.text.contains("Claude · Fake Small 5.5 · low"),
        "{}",
        model.text
    );
    // A message that changes nothing says nothing of it.
    let plain = mcp.call("agent_send", json!({"agent_id": id, "message": "Thanks."}));
    assert!(!plain.text.contains("Claude ·"), "{}", plain.text);
}

#[test]
fn every_provider_says_what_its_agents_run_on() {
    let user = User::with_fake_providers();
    user.write_providers("[opencode]\nenabled = true\n");
    for provider in ["claude", "opencode"] {
        user.provider_script(provider, script(json!([])));
    }
    let (_, mcp) = lead_on(&user, Some("claude"));
    // The models of a provider are known once one of its agents ran.
    for provider in ["claude", "opencode"] {
        launched(
            &user,
            &mcp,
            &format!("{provider} first"),
            json!({"kind": "child", "provider": provider}),
        );
    }

    for (provider, title) in [("claude", "Claude"), ("opencode", "OpenCode")] {
        let answer = mcp.call(
            "agent_launch",
            json!({"kind": "child", "title": provider, "brief": "Do the work.",
                "provider": provider, "model": "fake-large", "effort": "high"}),
        );
        assert!(!answer.error, "{}", answer.text);
        assert!(
            answer
                .text
                .contains(&format!("on {title} · Fake Large 5.5 · high")),
            "{}",
            answer.text
        );
    }
    let status = mcp.call("agent_status", json!({}));
    for title in ["Claude", "OpenCode"] {
        assert!(
            status
                .text
                .contains(&format!("{title} · Fake Large 5.5 · high")),
            "{}",
            status.text
        );
    }
}

#[test]
fn a_provider_that_offers_no_effort_says_the_model_alone() {
    let user = User::with_fake_agent();
    user.fake_script(json!({
        "models": ["fake-small"],
        "model_rows": {"fake-small": {"name": "Fake Small 5.5"}},
        "turns": []
    }));
    let (_, mcp) = lead(&user);

    let (_, answer) = launched(&user, &mcp, "worker", json!({"kind": "child"}));

    assert!(
        answer.contains("on Claude · Fake Small 5.5, in "),
        "{answer}"
    );
    assert!(!answer.contains("effort not known"), "{answer}");
}
