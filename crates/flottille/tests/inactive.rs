//! The scenarios of ending inactive sessions (`docs/design.md` §5.7): off
//! by default, a root with no activity for the days the user set ended as "End session" does it,
//! and never one with work waiting. The clock is the one the daemon's tests move
//! (`FLOTTILLE_TEST_CLOCK`, `test.advance`). The agent is the fake ACP agent, in a throwaway
//! repository and temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

const DAY: u64 = 24 * 3_600_000;

fn user() -> User {
    User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1")
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

/// A project `app` over a throwaway repository holding a `.env` (a sensitive place, so a read of
/// it is held for the user) and a `server.sh` task, and the session of a root agent working in
/// its folder, idle once its first turn ended: its id and its MCP session.
fn session(user: &User) -> (String, Mcp, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "DB_PASSWORD=hunter2-hunter2\n").unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[commands.server]\nline = \"./server.sh\"\nserve = true\n");
    std::fs::write(&file, text).unwrap();
    let script = folder.join("server.sh");
    std::fs::write(&script, "#!/bin/sh\necho listening\nsleep 300\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    eventually("the first turn", || state(user, &id) == "idle");
    (id, mcp, folder)
}

fn state(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn show(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone()
}

fn days(user: &User, days: Value) {
    user.call("sessions.set", json!({"days": days})).unwrap();
}

fn advance(user: &User, ms: u64) {
    user.call("test.advance", json!({"ms": ms})).unwrap();
}

#[test]
fn no_session_is_ended_while_the_setting_is_off() {
    let user = user();
    let (id, _mcp, _) = session(&user);
    assert_eq!(
        user.call("sessions.show", json!({})).unwrap()["days"],
        "off"
    );

    advance(&user, 400 * DAY);

    assert_eq!(state(&user, &id), "idle");
}

#[test]
fn the_setting_is_off_by_default_and_is_kept_in_a_file() {
    let user = user();
    assert_eq!(
        user.call("sessions.show", json!({})).unwrap()["days"],
        "off"
    );

    assert_eq!(
        user.call("sessions.set", json!({"days": 7})).unwrap()["days"],
        7
    );
    assert_eq!(user.call("sessions.show", json!({})).unwrap()["days"], 7);
    let file = user.path("config/flottille/sessions.toml");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "end_inactive_days = 7\n"
    );

    days(&user, json!("off"));
    assert_eq!(
        user.call("sessions.show", json!({})).unwrap()["days"],
        "off"
    );
}

#[test]
fn the_days_are_from_1_to_90() {
    let user = user();
    for wrong in [json!(0), json!(91), json!(-1), json!(1.5), json!("often")] {
        let refused = user
            .call("sessions.set", json!({"days": wrong}))
            .unwrap_err();
        assert_eq!(refused.0, "invalid-params", "{wrong}: {refused:?}");
    }
    assert_eq!(
        user.call("sessions.set", json!({})).unwrap_err().0,
        "invalid-params"
    );
    for right in [1, 90] {
        days(&user, json!(right));
    }
    assert_eq!(user.call("sessions.show", json!({})).unwrap()["days"], 90);
}

#[test]
fn a_file_with_a_word_it_does_not_know_is_said() {
    let user = user();
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/sessions.toml"),
        "end_inactive_days = 200\n",
    )
    .unwrap();

    let refused = user.call("sessions.show", json!({})).unwrap_err();

    assert_eq!(refused.0, "invalid-config", "{refused:?}");
    assert!(refused.1.contains("sessions.toml"), "{refused:?}");
}

#[test]
fn an_inactive_session_is_ended_after_the_days_set() {
    let user = user();
    let (id, _mcp, _) = session(&user);
    days(&user, json!(3));

    advance(&user, 2 * DAY);
    assert_eq!(state(&user, &id), "idle", "2 days are not 3");

    advance(&user, 2 * DAY);
    eventually("the end", || state(&user, &id) == "ended");
}

#[test]
fn a_session_with_activity_inside_the_days_is_not_ended() {
    let user = user();
    let (id, _mcp, _) = session(&user);
    days(&user, json!(3));
    advance(&user, 2 * DAY);
    user.call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    eventually("the second turn", || {
        user.received_requests("session/prompt").len() == 2 && state(&user, &id) == "idle"
    });

    advance(&user, 2 * DAY);

    assert_eq!(
        state(&user, &id),
        "idle",
        "4 days since the start, 2 since the message"
    );
    advance(&user, 2 * DAY);
    eventually("the end", || state(&user, &id) == "ended");
}

#[test]
fn a_session_with_a_running_command_is_not_ended() {
    let user = user();
    let (id, mcp, _) = session(&user);
    ok(mcp.call("run", json!({"task": "server", "timeout": 1})));
    days(&user, json!(3));

    advance(&user, 10 * DAY);

    assert_eq!(state(&user, &id), "idle");
}

#[test]
fn a_session_waiting_for_the_user_is_not_ended() {
    let user = user();
    let (id, mcp, _) = session(&user);
    let held = mcp.call("fs_read", json!({"path": ".env"}));
    assert!(
        held.text.starts_with("Waiting for the user's approval"),
        "{}",
        held.text
    );
    days(&user, json!(3));

    advance(&user, 10 * DAY);

    assert_eq!(state(&user, &id), "idle");
}

#[test]
fn an_ended_session_says_it_was_inactive() {
    let user = user();
    let (id, _mcp, _) = session(&user);
    days(&user, json!(3));
    let watch = user.watch(&["--after", "0"]);

    advance(&user, 4 * DAY);

    let ended = watch.until(|e| e["name"] == "agent.ended");
    let ended = ended.last().unwrap();
    assert_eq!(ended["data"]["agent"], id.as_str());
    assert_eq!(ended["data"]["reason"], "inactive for 3 days");
    let agent = show(&user, &id);
    assert_eq!(agent["state"], "ended");
    assert_eq!(agent["state_reason"], "inactive for 3 days");
}

#[test]
fn an_inactive_session_is_resumed_as_any_ended_session_is() {
    let user = user();
    let (id, _mcp, _) = session(&user);
    days(&user, json!(1));
    advance(&user, 2 * DAY);
    eventually("the end", || state(&user, &id) == "ended");

    let resumed = user.resume(&id);

    assert_ne!(resumed["agent"]["state"], "ended", "{resumed}");
}
