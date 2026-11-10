//! The scenarios of a spent quota (`docs/design.md` §5.9): the provider's words recognised at the
//! end of a turn, the reset read with its zone and stored, the session resumed once at the reset
//! when the user allows it and never otherwise, a message of the user's ending the wait, and a
//! restart keeping it. The agent is the fake ACP agent, which reports a limit as the bridge does
//! (`agent.provider_limit`, Claude's words in it); the clock is the one the daemon's tests move (`FLOTTILLE_TEST_CLOCK`, `test.advance`), so no test
//! waits for a reset.

mod common;

use std::time::{Duration, Instant};

use common::User;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::{Value, json};

const RESUME: &str = "Your quota has reset: continue where you stopped.";
const MINUTE: u64 = 60_000;

/// The step in which the provider reports a spent limit, as the bridge does at the end of a turn.
fn limit(message: &str) -> Value {
    json!({"update": {"sessionUpdate": "session_info_update", "_meta": {"jetbrains": {"air":
        {"sessionFailure": {"severity": "error", "category": "limit", "message": message}}}}}})
}

fn user() -> User {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    user.fake_script(json!({"turns": [
        {"when": "hello", "do": [
            limit("You've hit your weekly limit · resets 1pm (Europe/Paris)")]},
        {"when": "unreadable", "do": [limit("You've hit your limit · resets whenever")]},
        {"when": "rejected", "do": [limit("Request rejected by the provider")]},
        {"when": "quoting", "do": [
            {"say": "You've hit your limit · resets 3pm (Europe/Paris)"}]},
    ]}));
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A project `app` over a throwaway repository and a root agent started in its folder with
/// `message`, idle once its first turn ended: its id.
fn session(user: &User, message: &str) -> String {
    move_clock_to(user, 8, 0);
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn", || {
        prompts(user).len() == 1 && show(user, &id)["state"] == "idle"
    });
    id
}

fn show(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone()
}

fn quota(user: &User, agent: &str) -> Value {
    show(user, agent)["quota"].clone()
}

fn listed_quota(user: &User, agent: &str) -> Value {
    let list = user.call("agent.list", json!({})).unwrap();
    list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == agent)
        .unwrap()["quota"]
        .clone()
}

/// The texts of the prompts the fake agent took.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|request| request["params"]["prompt"].to_string())
        .collect()
}

fn resumes(user: &User) -> usize {
    prompts(user)
        .iter()
        .filter(|prompt| prompt.contains(RESUME))
        .count()
}

fn now(user: &User) -> u64 {
    user.call("test.advance", json!({"ms": 0})).unwrap()["now"]
        .as_u64()
        .unwrap()
}

fn advance(user: &User, ms: u64) {
    user.call("test.advance", json!({"ms": ms})).unwrap();
}

/// Moves the daemon's clock to the next `hour`:`minute` in Paris, far from every reset the fake
/// agent says (1pm, 3pm), so that no test depends on the real time of day.
fn move_clock_to(user: &User, hour: i8, minute: i8) {
    let paris = TimeZone::get("Europe/Paris").unwrap();
    let here = Timestamp::from_millisecond(now(user) as i64)
        .unwrap()
        .to_zoned(paris.clone());
    let at = |date: jiff::civil::Date| date.at(hour, minute, 0, 0).to_zoned(paris.clone()).unwrap();
    let mut target = at(here.date());
    if target <= here {
        target = at(here.date().tomorrow().unwrap());
    }
    let wait = target.timestamp().as_millisecond() - here.timestamp().as_millisecond();
    advance(user, wait as u64);
}

fn resume_setting(user: &User, on: bool) {
    user.call("sessions.set", json!({"resume": on})).unwrap();
}

/// The reset the session waits for, after `advance`ing past it is due.
fn reset_at(user: &User, agent: &str) -> u64 {
    quota(user, agent)["reset_at"].as_u64().expect("a reset")
}

fn past_the_reset(user: &User, agent: &str) {
    let due = reset_at(user, agent) + MINUTE;
    advance(user, due.saturating_sub(now(user)) + 1_000);
}

#[test]
fn a_spent_quota_is_recognised_and_its_reset_read_in_its_zone() {
    let user = user();
    let id = session(&user, "hello");
    let said = now(&user);

    let wait = quota(&user, &id);

    let reset = wait["reset_at"].as_u64().expect("a reset read");
    assert!(reset > said && reset <= said + 24 * 60 * MINUTE, "{wait}");
    let paris = Timestamp::from_millisecond(reset as i64)
        .unwrap()
        .to_zoned(TimeZone::get("Europe/Paris").unwrap());
    assert_eq!(paris.strftime("%H:%M").to_string(), "13:00");
    assert_eq!(wait["resume"], false, "off by default");
    assert_eq!(listed_quota(&user, &id), wait, "the list says it too");
    assert_eq!(show(&user, &id)["state"], "idle");
}

#[test]
fn a_reset_that_cannot_be_read_is_stored_unknown_and_resumes_nothing() {
    let user = user();
    let id = session(&user, "unreadable");
    resume_setting(&user, true);

    let wait = quota(&user, &id);
    assert!(wait.is_object(), "limited: {wait}");
    assert_eq!(wait["reset_at"], Value::Null);
    assert_eq!(wait["resume"], false);

    advance(&user, 8 * 24 * 60 * MINUTE);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0);
    assert!(quota(&user, &id).is_object(), "still limited");
}

#[test]
fn an_agent_quoting_the_limit_message_does_not_wait_for_a_reset() {
    let user = user();
    resume_setting(&user, true);

    let id = session(&user, "quoting");

    assert_eq!(quota(&user, &id), Value::Null, "its own words are no limit");
    advance(&user, 8 * 24 * 60 * MINUTE);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0);
}

#[test]
fn a_rejected_rate_limit_without_a_known_sentence_waits_with_an_unknown_reset() {
    let user = user();
    let id = session(&user, "rejected");
    resume_setting(&user, true);

    let wait = quota(&user, &id);

    assert!(wait.is_object(), "limited: {wait}");
    assert_eq!(wait["reset_at"], Value::Null);
    assert_eq!(wait["resume"], false);
    advance(&user, 8 * 24 * 60 * MINUTE);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0);
    assert!(quota(&user, &id).is_object(), "still limited");
}

#[test]
fn a_turn_without_the_limit_is_no_wait() {
    let user = user();
    let id = session(&user, "work");

    assert_eq!(quota(&user, &id), Value::Null);
}

#[test]
fn with_the_setting_on_the_session_is_resumed_once_at_its_reset() {
    let user = user();
    let id = session(&user, "hello");
    resume_setting(&user, true);
    assert_eq!(quota(&user, &id)["resume"], true);

    // Before the reset and its margin: nothing.
    let due = reset_at(&user, &id) + MINUTE;
    advance(&user, due - now(&user) - 2_000);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0, "not before the reset");

    advance(&user, 3_000);
    eventually("the resume", || resumes(&user) == 1);
    eventually("the turn it starts ends", || {
        show(&user, &id)["state"] == "idle" && quota(&user, &id).is_null()
    });

    advance(&user, 7 * 24 * 60 * MINUTE);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 1, "never twice");
}

#[test]
fn with_the_setting_off_nothing_is_sent_at_the_reset() {
    let user = user();
    let id = session(&user, "hello");

    past_the_reset(&user, &id);
    eventually("the wait ends with its reset", || {
        quota(&user, &id).is_null()
    });

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0);
    assert_eq!(prompts(&user).len(), 1);
}

#[test]
fn the_session_s_own_choice_wins_over_the_setting() {
    let user = user();
    let id = session(&user, "hello");
    resume_setting(&user, true);

    let answer = user
        .call("agent.quota_resume", json!({"agent": id, "resume": false}))
        .unwrap();
    assert_eq!(answer["quota"]["resume"], false);
    assert_eq!(quota(&user, &id)["resume"], false);
    past_the_reset(&user, &id);
    eventually("the wait ends", || quota(&user, &id).is_null());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0, "off for this session");
}

#[test]
fn a_session_switched_on_alone_is_resumed_with_the_setting_off() {
    let user = user();
    let id = session(&user, "hello");

    user.call("agent.quota_resume", json!({"agent": id, "resume": true}))
        .unwrap();
    past_the_reset(&user, &id);

    eventually("the resume", || resumes(&user) == 1);
}

#[test]
fn a_session_that_does_not_wait_has_no_choice_to_make() {
    let user = user();
    let id = session(&user, "work");

    let refused = user
        .call("agent.quota_resume", json!({"agent": id, "resume": true}))
        .unwrap_err();

    assert_eq!(refused.0, "refused", "{refused:?}");
}

#[test]
fn a_message_of_the_user_s_clears_the_wait() {
    let user = user();
    let id = session(&user, "hello");
    resume_setting(&user, true);
    let watch = user.watch(&["--after", "0"]);

    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();

    let cleared = watch.until(|e| e["name"] == "agent.quota" && e["data"]["limited"] == false);
    assert_eq!(cleared.last().unwrap()["data"]["why"], "the user wrote");
    eventually("the second turn", || {
        prompts(&user).len() == 2 && show(&user, &id)["state"] == "idle"
    });
    assert_eq!(quota(&user, &id), Value::Null);

    advance(&user, 8 * 24 * 60 * MINUTE);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0, "nothing at the old reset");
}

#[test]
fn a_restart_keeps_the_wait() {
    let user = user();
    let id = session(&user, "hello");
    let before = quota(&user, &id);
    user.call("agent.quota_resume", json!({"agent": id, "resume": true}))
        .unwrap();

    assert!(user.run(&["daemon", "stop"]).status.success());
    user.status();

    let after = quota(&user, &id);
    assert_eq!(after["reset_at"], before["reset_at"]);
    assert_eq!(after["resume"], true, "the session's choice kept");
    assert_eq!(resumes(&user), 0);
}

/// The stored reset moved to an instant already past, the daemon stopped: as if it was down at
/// the reset.
fn reset_passed_while_stopped(user: &User, id: &str) {
    assert!(user.run(&["daemon", "stop"]).status.success());
    let db = rusqlite::Connection::open(user.path("state/flottille/flottille.db")).unwrap();
    let past = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 10 * MINUTE as i64;
    let moved = db
        .execute(
            "UPDATE quota_waits SET reset_at = ?1 WHERE agent = ?2",
            rusqlite::params![past, id],
        )
        .unwrap();
    assert_eq!(moved, 1);
}

#[test]
fn a_reset_already_past_at_start_resumes_at_once_when_allowed() {
    let user = user();
    let id = session(&user, "hello");
    resume_setting(&user, true);
    reset_passed_while_stopped(&user, &id);

    user.status();

    eventually("the resume at start", || resumes(&user) == 1);
}

#[test]
fn a_reset_already_past_at_start_resumes_nothing_when_not_allowed() {
    let user = user();
    let id = session(&user, "hello");
    reset_passed_while_stopped(&user, &id);

    user.status();

    eventually("the wait ends", || quota(&user, &id).is_null());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0);
}

#[test]
fn the_setting_is_kept_beside_the_days() {
    let user = user();
    assert_eq!(
        user.call("sessions.show", json!({})).unwrap()["resume"],
        false
    );
    user.call("sessions.set", json!({"days": 7})).unwrap();

    resume_setting(&user, true);

    let shown = user.call("sessions.show", json!({})).unwrap();
    assert_eq!(shown["resume"], true);
    assert_eq!(shown["days"], 7, "the days kept");
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/sessions.toml")).unwrap(),
        "end_inactive_days = 7\nresume_at_reset = true\n"
    );
    let refused = user
        .call("sessions.set", json!({"resume": "yes"}))
        .unwrap_err();
    assert_eq!(refused.0, "invalid-params");
}

#[test]
fn an_ended_session_waits_no_more() {
    let user = user();
    let id = session(&user, "hello");
    resume_setting(&user, true);

    user.call("agent.stop", json!({"agent": id})).unwrap();
    advance(&user, 8 * 24 * 60 * MINUTE);

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(resumes(&user), 0);
    assert_eq!(quota(&user, &id), Value::Null);
}
