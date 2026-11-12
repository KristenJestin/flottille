//! A session is named by its title as well as its id (#776, `docs/tools.md`, "Sessions that read
//! each other", "Sessions that talk to each other"): `session_read`, `session_send` and
//! `agent_stop` take either, a title is matched among the roots of the caller's project, and a
//! name that picks no one is refused with the sessions it could have meant. Against the real
//! daemon with the fake ACP agent, throwaway repositories and temporary XDG folders.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

const TITLE: &str = "Analytics : sortir d'Umami";

/// A user whose agents answer every message, with a judge allowing every call.
fn user() -> User {
    let user = User::with_fake_agent().with_allowing_judge();
    project(&user, "app");
    user
}

/// A project over a throwaway repository.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

/// A session the user starts in the project, its first turn ended: its id and its MCP session.
fn root(user: &User, project: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call("agent.run", json!({"project": project, "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    idle(user, &id);
    (id, Mcp::initialize(&url, &authorization))
}

/// A session the user starts and titles.
fn titled(user: &User, project: &str, title: &str) -> (String, Mcp) {
    let (id, mcp) = root(user, project);
    user.call("agent.rename", json!({"agent": id, "title": title}))
        .unwrap();
    (id, mcp)
}

fn idle(user: &User, agent: &str) {
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"] == "idle"
    });
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    user.call("agent.mode", json!({"agent": agent, "mode": mode}))
        .unwrap();
}

/// What a request says, as the user reads it.
fn says(request: &Value) -> String {
    request["says"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

/// A call held for the user: its request's number.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The messages other sessions wrote to `agent`.
fn written_to(user: &User, agent: &str) -> Vec<String> {
    store(user)
        .last_deliveries(agent, 32)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == "session")
        .map(|delivery| delivery.body)
        .collect()
}

#[test]
fn session_read_takes_a_session_s_title() {
    let user = user();
    let (reader, mcp) = root(&user, "app");
    let (session, _) = titled(&user, "app", TITLE);
    set_mode(&user, &reader, "full");

    let by_title = mcp.call("session_read", json!({"session": TITLE}));
    let loose = mcp.call(
        "session_read",
        json!({"session": "  ANALYTICS : sortir D'umami "}),
    );

    for page in [&by_title, &loose] {
        assert!(!page.error, "{}", page.text);
        assert!(
            page.text.contains(&format!(
                "The conversation of the session \"{TITLE}\" (agent {session})"
            )) && page.text.contains("hello"),
            "{}",
            page.text
        );
    }
}

#[test]
fn session_send_takes_a_session_s_title() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = titled(&user, "app", TITLE);

    // The leave is asked as it is for the id: the request names the sessions, never the title
    // alone, and it is asked once for the pair, whichever name the next message carries.
    let number = held(&mcp.call("session_send", json!({"session": TITLE, "text": "First."})));
    let listed = requests(&user);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let says = says(&listed[0]);
    assert!(
        says.contains(&format!(
            "Let the session agent {a} write to the session \"{TITLE}\" (agent {x})"
        )),
        "{says}"
    );
    let again = mcp.call("session_send", json!({"session": x, "text": "Second."}));
    assert!(
        again.error
            && again
                .text
                .contains(&format!("request #{number} already asks")),
        "{}",
        again.text
    );

    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the first message", || {
        written_to(&user, &x)
            .iter()
            .any(|body| body.contains("First."))
    });
    let sent = mcp.call(
        "session_send",
        json!({"session": TITLE.to_uppercase(), "text": "Third."}),
    );
    assert!(sent.text.starts_with("Sent to "), "{}", sent.text);
    assert_eq!(requests(&user).len(), 0, "the leave was asked again");
}

#[test]
fn agent_stop_takes_a_session_s_title() {
    let user = user();
    let (_, mcp) = root(&user, "app");
    let (x, _) = titled(&user, "app", TITLE);

    held(&mcp.call("agent_stop", json!({"agent_id": TITLE})));

    let says = says(&requests(&user)[0]);
    assert!(
        says.contains(&format!("End the session \"{TITLE}\" (agent {x})")),
        "{says}"
    );
}

#[test]
fn a_title_two_sessions_share_is_refused_listing_both() {
    let user = user();
    let (_, mcp) = root(&user, "app");
    let (first, _) = titled(&user, "app", TITLE);
    let (second, _) = titled(&user, "app", TITLE);

    for (tool, field) in [
        ("session_read", "session"),
        ("session_send", "session"),
        ("agent_stop", "agent_id"),
    ] {
        let refused = mcp.call(
            tool,
            json!({field: TITLE, "text": "Hi."})
                .as_object()
                .map_or(json!({}), |all| {
                    let mut all = all.clone();
                    if tool != "session_send" {
                        all.remove("text");
                    }
                    Value::Object(all)
                }),
        );

        assert!(refused.error, "{tool}: {}", refused.text);
        for id in [&first, &second] {
            assert!(
                refused
                    .text
                    .contains(&format!("{TITLE} · {id} · main · idle, active ")),
                "{tool}: {}",
                refused.text
            );
        }
    }
    assert!(requests(&user).is_empty());
}

#[test]
fn an_unknown_session_is_refused_listing_the_project_s_sessions() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = titled(&user, "app", TITLE);
    let (y, _) = titled(&user, "app", "Login <b>&amp; · x");

    for (tool, args) in [
        ("session_read", json!({"session": "Nothing of the kind"})),
        (
            "session_send",
            json!({"session": "Nothing of the kind", "text": "Hi."}),
        ),
        ("agent_stop", json!({"agent_id": "Nothing of the kind"})),
    ] {
        let refused = mcp.call(tool, args);

        assert!(refused.error, "{tool}: {}", refused.text);
        assert!(
            refused.text.starts_with("refused: no session of your project has the id or the title \"Nothing of the kind\""),
            "{tool}: {}",
            refused.text
        );
        // The most recently active first, one line each, however odd the title.
        let at = |id: &String| refused.text.find(id.as_str()).unwrap_or(usize::MAX);
        assert!(at(&y) < at(&x), "{tool}: {}", refused.text);
        assert!(refused.text.contains(&a), "{tool}: {}", refused.text);
        assert_eq!(
            refused
                .text
                .lines()
                .filter(|line| line.contains(" · main · "))
                .count(),
            3,
            "{tool}: {}",
            refused.text
        );
    }
}

#[test]
fn a_title_of_another_project_s_session_is_not_found_in_this_one() {
    let user = user();
    project(&user, "other");
    let (_, mcp) = root(&user, "app");
    let (elsewhere, _) = titled(&user, "other", TITLE);

    for (tool, args) in [
        ("session_read", json!({"session": TITLE})),
        ("session_send", json!({"session": TITLE, "text": "Hi."})),
        ("agent_stop", json!({"agent_id": TITLE})),
    ] {
        let refused = mcp.call(tool, args);

        assert!(refused.error, "{tool}: {}", refused.text);
        assert!(
            refused.text.starts_with(&format!(
                "refused: no session of your project has the id or the title \"{TITLE}\""
            )),
            "{tool}: {}",
            refused.text
        );
        // It is no longer the other project's id that is told: the lines are this project's.
        assert!(
            !refused.text.contains(&elsewhere),
            "{tool}: {}",
            refused.text
        );
    }
    assert!(requests(&user).is_empty());
}
