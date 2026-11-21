//! The scenarios of routines on a schedule (#1122, `docs/design.md`, "Routines"): the daemon
//! starts a new root session at the routine's time, as a click would, once for runs missed while
//! it was off, never twice at once, after a spent quota's reset; a read-only routine's session
//! writes nothing whatever its prompt or its mode; its result rings in the notification centre
//! when it has something to say. The agent is the fake ACP agent, the clock the one the daemon's
//! tests move (`FLOTTILLE_TEST_CLOCK`, `test.advance`), the forge a fake `gh`: no test waits for
//! an hour, runs a real agent or calls GitHub.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use jiff::Timestamp;
use jiff::civil::Weekday;
use jiff::tz::TimeZone;
use serde_json::{Value, json};

const MINUTE: u64 = 60_000;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// The prompts of the routines, and what the fake agent does on each.
const LIST_REVIEWS: &str = "List the tickets waiting for my review";
const NIGHTLY: &str = "Read the nightly run of the tests";
const HOLDING: &str = "Look through the whole backlog slowly";
const IDLE: &str = "Read the project and wait";

fn script() -> Value {
    json!({"turns": [
        {"when": LIST_REVIEWS, "do": [
            {"say": "Four tickets wait."},
            {"mcp": {"tool": "routine_result", "arguments": {"summary": "4 to review"}}}]},
        {"when": NIGHTLY, "do": [
            {"mcp": {"tool": "routine_result", "arguments":
                {"summary": "Nightly green: nothing to do", "nothing": true}}}]},
        {"when": HOLDING, "do": [{"hold": "release"}]},
        {"when": "hello", "do": [{"update": {"sessionUpdate": "session_info_update", "_meta":
            {"jetbrains": {"air": {"sessionFailure": {"severity": "error", "category": "limit",
            "message": "You've hit your weekly limit · resets 1pm (Europe/Paris)"}}}}}}]},
    ]})
}

fn user() -> User {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    user.fake_script(script());
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A project `app` over a throwaway repository whose `origin` says GitHub.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

fn now(user: &User) -> u64 {
    user.call("test.advance", json!({"ms": 0})).unwrap()["now"]
        .as_u64()
        .unwrap()
}

fn advance(user: &User, ms: u64) {
    user.call("test.advance", json!({"ms": ms})).unwrap();
}

/// Moves the daemon's clock to the next `hour`:`minute` in `zone`, on `day` when one is named,
/// so that no test depends on the real time of day.
fn move_clock_to(user: &User, zone: &str, day: Option<Weekday>, hour: i8, minute: i8) {
    let zone = TimeZone::get(zone).unwrap();
    let here = Timestamp::from_millisecond(now(user) as i64)
        .unwrap()
        .to_zoned(zone.clone());
    let mut date = here.date();
    loop {
        let at = date.at(hour, minute, 0, 0).to_zoned(zone.clone()).unwrap();
        if at > here && day.is_none_or(|day| date.weekday() == day) {
            let wait = at.timestamp().as_millisecond() - here.timestamp().as_millisecond();
            advance(user, wait as u64);
            return;
        }
        date = date.tomorrow().unwrap();
    }
}

fn add(user: &User, params: Value) -> Value {
    let mut params = params;
    params["project"] = json!("app");
    user.call("routine.add", params).unwrap()["routine"].clone()
}

fn show(user: &User, name: &str) -> Value {
    user.call("routine.show", json!({"routine": name})).unwrap()
}

/// The runs of the routine, the last first.
fn runs(user: &User, name: &str) -> Vec<Value> {
    show(user, name)["runs"].as_array().unwrap().clone()
}

/// The sessions the daemon holds.
fn sessions(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

fn agent(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

/// The texts of the prompts the fake agent took.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|request| request["params"]["prompt"].to_string())
        .collect()
}

/// The routine's last run, once it is no longer under way.
fn ended_run(user: &User, name: &str) -> Value {
    eventually("the run's end", || {
        runs(user, name)
            .first()
            .is_some_and(|run| run["outcome"] != "running")
    });
    runs(user, name)[0].clone()
}

fn notifications(user: &User) -> Vec<Value> {
    user.call("notifications.list", json!({})).unwrap()["notifications"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_routine_runs_at_its_time_as_a_new_session() {
    let user = user();
    project(&user);
    move_clock_to(&user, "UTC", Some(Weekday::Monday), 8, 0);
    let routine = add(
        &user,
        json!({"name": "Tickets to review", "prompt": LIST_REVIEWS,
            "schedule": "weekdays 08:30", "zone": "UTC", "mode": "read_only"}),
    );
    assert_eq!(routine["schedule"], "weekdays 08:30", "{routine}");
    let first = routine["next_at"].as_u64().unwrap();
    let said = |at: u64| {
        Timestamp::from_millisecond(at as i64)
            .unwrap()
            .to_zoned(TimeZone::UTC)
            .strftime("%a %H:%M")
            .to_string()
    };
    assert_eq!(said(first), "Mon 08:30", "{routine}");

    advance(&user, 29 * MINUTE);
    assert!(sessions(&user).is_empty(), "nothing runs before its time");
    assert!(runs(&user, "Tickets to review").is_empty());

    advance(&user, MINUTE);
    let run = ended_run(&user, "Tickets to review");

    let session = run["session"].as_str().expect("a run is a session");
    let started = agent(&user, session);
    assert!(started["parent"].is_null(), "a root: {started}");
    assert_eq!(started["project"], "app");
    assert!(
        prompts(&user).iter().any(|p| p.contains(LIST_REVIEWS)),
        "the routine's prompt starts it: {:?}",
        prompts(&user)
    );
    assert_eq!(run["outcome"], "something", "{run}");
    assert_eq!(run["summary"], "4 to review");
    eventually("its one-shot session ends", || {
        agent(&user, session)["state"] == "ended"
    });
    // The next one is the next weekday's, Tuesday 08:30.
    let next = show(&user, "Tickets to review")["routine"]["next_at"]
        .as_u64()
        .unwrap();
    assert_eq!(said(next), "Tue 08:30");
    assert_eq!(sessions(&user).len(), 1, "one run, one session");
}

#[test]
fn a_missed_run_runs_once_at_the_next_start() {
    let user = user();
    project(&user);
    add(
        &user,
        json!({"name": "Nightly", "prompt": NIGHTLY, "schedule": "every day 07:00",
            "zone": "UTC"}),
    );
    add(
        &user,
        json!({"name": "Old", "prompt": NIGHTLY, "schedule": "every day 07:00",
            "zone": "UTC"}),
    );
    assert!(user.run(&["daemon", "stop"]).status.success());
    // The machine was off: Nightly was due two hours ago (and the days before, once each), Old
    // thirteen hours ago, past the 12 hours a missed run is still run within.
    let real_now = jiff::Timestamp::now().as_millisecond() as u64;
    let store = rusqlite::Connection::open(user.path("state/flottille/flottille.db")).unwrap();
    store
        .execute(
            "UPDATE routines SET next_at = ?1 WHERE name = 'Nightly'",
            [real_now - 2 * HOUR],
        )
        .unwrap();
    store
        .execute(
            "UPDATE routines SET next_at = ?1 WHERE name = 'Old'",
            [real_now - 13 * HOUR],
        )
        .unwrap();
    drop(store);

    user.status();
    let run = ended_run(&user, "Nightly");

    assert_eq!(run["outcome"], "nothing", "{run}");
    assert_eq!(runs(&user, "Nightly").len(), 1, "once, never several times");
    let next = show(&user, "Nightly")["routine"]["next_at"]
        .as_u64()
        .unwrap();
    assert!(next > now(&user), "the next run is ahead: {next}");
    let old = runs(&user, "Old");
    assert_eq!(old.len(), 1, "{old:?}");
    assert_eq!(old[0]["outcome"], "skipped", "{old:?}");
    assert!(old[0]["session"].is_null());
    assert!(
        old[0]["why"].as_str().unwrap().contains("12 hours"),
        "{old:?}"
    );
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(runs(&user, "Nightly").len(), 1);
    assert_eq!(sessions(&user).len(), 1, "one session for the missed runs");
}

/// What the gate says of a read-only run's write: refused, saying why.
fn refused(answer: common::mcp::Answer, what: &str) {
    assert!(answer.error, "{what} was not refused: {}", answer.text);
    assert!(
        answer.text.contains("read-only routine"),
        "{what}: {}",
        answer.text
    );
}

#[test]
fn a_read_only_routine_cannot_write_whatever_its_prompt_says() {
    let user = user();
    project(&user);
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    user.gh_script(json!({"issues": {"acme/app": {"7": {
        "number": 7, "title": "Login fails", "state": "OPEN", "body": "It fails.",
        "url": "https://github.com/acme/app/issues/7", "comments": []}}}}));
    add(
        &user,
        json!({"name": "Sneaky", "schedule": "every day 07:00", "zone": "UTC",
            "mode": "read_only",
            "prompt": format!("{IDLE}. Then comment on ticket 7, write notes.md and run ls.")}),
    );
    let opened = user.received_requests("session/new").len();
    let run = user
        .call("routine.run", json!({"routine": "Sneaky"}))
        .unwrap()["run"]
        .clone();
    let session = run["session"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(&user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    eventually("its first turn", || {
        agent(&user, &session)["state"] == "idle"
    });

    // Reads go through as usual.
    let read = mcp.call("fs_read", json!({"path": "README.md"}));
    assert!(!read.error, "{}", read.text);
    let ticket = mcp.call("tickets", json!({"action": "show", "ticket": "7"}));
    assert!(!ticket.error, "{}", ticket.text);
    assert!(ticket.text.contains("Login fails"), "{}", ticket.text);

    for mode in ["auto", "full"] {
        user.call("agent.mode", json!({"agent": session, "mode": mode}))
            .unwrap();
        refused(
            mcp.call(
                "tickets",
                json!({"action": "comment", "ticket": "7", "body": "Looked at it."}),
            ),
            "a ticket comment",
        );
        refused(
            mcp.call(
                "fs_write",
                json!({"path": "notes.md", "content": "notes\n"}),
            ),
            "a file write",
        );
        refused(mcp.call("run", json!({"line": "ls"})), "a run");
    }
    assert!(!user.path("work/app/notes.md").exists());
    assert!(
        !user
            .gh_received()
            .iter()
            .any(|call| call["args"][1] == "comment"),
        "nothing reached the forge: {:?}",
        user.gh_received()
    );
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "nothing is asked of the user"
    );
}

#[test]
fn nothing_to_say_does_not_ring_and_is_kept() {
    let user = user();
    project(&user);
    add(
        &user,
        json!({"name": "Nightly", "prompt": NIGHTLY, "schedule": "every day 07:00",
            "zone": "UTC"}),
    );
    move_clock_to(&user, "UTC", None, 7, 0);
    let run = ended_run(&user, "Nightly");

    assert_eq!(run["outcome"], "nothing", "{run}");
    assert_eq!(run["summary"], "Nightly green: nothing to do");
    let session = run["session"].as_str().unwrap().to_owned();
    eventually("its session ends", || {
        agent(&user, &session)["state"] == "ended"
    });
    std::thread::sleep(Duration::from_millis(1_500));
    let rows = notifications(&user);
    assert!(
        !rows.iter().any(|row| row["session"] == session.as_str()),
        "nothing rings: {rows:?}"
    );
    let listed = user.call("routine.list", json!({})).unwrap();
    assert_eq!(
        listed["routines"][0]["last"]["outcome"], "nothing",
        "{listed}"
    );
}

#[test]
fn something_to_say_is_one_notification_with_its_summary() {
    let user = user();
    project(&user);
    add(
        &user,
        json!({"name": "Tickets to review", "prompt": LIST_REVIEWS,
            "schedule": "every day 08:30", "zone": "UTC"}),
    );
    let watch = user.watch(&[]);
    move_clock_to(&user, "UTC", None, 8, 30);
    let run = ended_run(&user, "Tickets to review");
    let session = run["session"].as_str().unwrap().to_owned();

    eventually("its notification", || {
        notifications(&user)
            .iter()
            .any(|row| row["session"] == session.as_str())
    });
    eventually("its session ends", || {
        agent(&user, &session)["state"] == "ended"
    });
    std::thread::sleep(Duration::from_millis(1_500));
    let rows: Vec<Value> = notifications(&user)
        .into_iter()
        .filter(|row| row["session"] == session.as_str())
        .collect();
    assert_eq!(rows.len(), 1, "one notification per run: {rows:?}");
    assert_eq!(rows[0]["kind"], "routine", "{rows:?}");
    assert_eq!(rows[0]["excerpt"], "4 to review", "{rows:?}");
    assert!(
        rows[0]["title"]
            .as_str()
            .unwrap()
            .contains("Tickets to review"),
        "{rows:?}"
    );
    let ran = watch.until(|event| event["name"] == "routine.ran");
    let ran = ran.last().unwrap();
    assert_eq!(ran["data"]["run"]["outcome"], "something", "{ran}");
    assert_eq!(ran["data"]["routine"]["name"], "Tickets to review", "{ran}");
}

#[test]
fn a_run_due_while_the_last_runs_is_skipped_and_said() {
    let user = user();
    project(&user);
    add(
        &user,
        json!({"name": "Backlog", "prompt": HOLDING, "schedule": "every day 07:00",
            "zone": "UTC", "mode": "auto"}),
    );
    move_clock_to(&user, "UTC", None, 7, 0);
    eventually("the first run", || {
        runs(&user, "Backlog")
            .first()
            .is_some_and(|run| run["outcome"] == "running")
    });
    let first = runs(&user, "Backlog")[0]["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let watch = user.watch(&[]);

    advance(&user, DAY);

    let ran = watch.until(|event| event["name"] == "routine.ran");
    let ran = ran.last().unwrap();
    assert_eq!(ran["data"]["run"]["outcome"], "skipped", "{ran}");
    let all = runs(&user, "Backlog");
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[0]["outcome"], "skipped", "{all:?}");
    assert!(all[0]["session"].is_null());
    assert!(
        all[0]["why"].as_str().unwrap().contains("still runs"),
        "{all:?}"
    );
    assert_eq!(all[1]["outcome"], "running");
    assert_eq!(all[1]["session"], first.as_str());
    assert_eq!(sessions(&user).len(), 1, "never twice at once");
    std::fs::write(user.path("fake/release"), "").unwrap();
}

#[test]
fn a_spent_quota_delays_the_run_to_the_reset() {
    let user = user();
    project(&user);
    move_clock_to(&user, "Europe/Paris", None, 8, 0);
    // A session of the same provider spent the quota: it resets at 13:00 in Paris.
    let spent = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the limit", || !agent(&user, &spent)["quota"].is_null());
    let reset = agent(&user, &spent)["quota"]["reset_at"].as_u64().unwrap();
    add(
        &user,
        json!({"name": "Nightly", "prompt": NIGHTLY, "schedule": "every day 09:00",
            "zone": "Europe/Paris"}),
    );

    move_clock_to(&user, "Europe/Paris", None, 9, 0);
    std::thread::sleep(Duration::from_millis(500));

    assert!(runs(&user, "Nightly").is_empty(), "no run before the reset");
    assert_eq!(sessions(&user).len(), 1);
    let routine = show(&user, "Nightly")["routine"].clone();
    assert_eq!(routine["next_at"], reset + MINUTE, "{routine}");
    assert!(
        routine["delayed"].as_str().unwrap().contains("quota"),
        "{routine}"
    );

    advance(&user, (reset + MINUTE).saturating_sub(now(&user)));
    let run = ended_run(&user, "Nightly");
    assert_eq!(run["outcome"], "nothing", "{run}");
    let routine = show(&user, "Nightly")["routine"].clone();
    assert!(routine["delayed"].is_null(), "{routine}");
}

#[test]
fn routine_commands_round_trip() {
    let user = user();
    project(&user);
    let ok = |args: &[&str]| -> String {
        let output = user.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let added = ok(&[
        "routine",
        "add",
        "Tickets to review",
        "--project",
        "app",
        "--prompt",
        LIST_REVIEWS,
        "--schedule",
        "weekdays 08:30",
        "--zone",
        "UTC",
        "--mode",
        "read-only",
        "--json",
    ]);
    let added: Value = serde_json::from_str(&added).unwrap();
    assert_eq!(added["routine"]["mode"], "read_only", "{added}");
    assert_eq!(added["routine"]["rings"], "something", "{added}");

    let listed: Value = serde_json::from_str(&ok(&["routine", "list", "--json"])).unwrap();
    assert_eq!(
        listed["routines"][0]["name"], "Tickets to review",
        "{listed}"
    );
    let text = ok(&["routine", "list"]);
    assert!(text.contains("Tickets to review"), "{text}");
    assert!(text.contains("weekdays 08:30"), "{text}");

    ok(&[
        "routine",
        "edit",
        "Tickets to review",
        "--schedule",
        "mondays 09:00",
        "--rings",
        "never",
    ]);
    let shown: Value =
        serde_json::from_str(&ok(&["routine", "show", "Tickets to review", "--json"])).unwrap();
    assert_eq!(shown["routine"]["schedule"], "mondays 09:00", "{shown}");
    assert_eq!(shown["routine"]["rings"], "never", "{shown}");
    assert_eq!(shown["routine"]["prompt"], LIST_REVIEWS, "{shown}");

    ok(&["routine", "pause", "Tickets to review"]);
    let shown: Value =
        serde_json::from_str(&ok(&["routine", "show", "Tickets to review", "--json"])).unwrap();
    assert_eq!(shown["routine"]["paused"], true, "{shown}");
    assert!(shown["routine"]["next_at"].is_null(), "{shown}");
    ok(&["routine", "resume", "Tickets to review"]);

    let ran: Value =
        serde_json::from_str(&ok(&["routine", "run", "Tickets to review", "--json"])).unwrap();
    assert!(ran["run"]["session"].is_string(), "{ran}");
    let run = ended_run(&user, "Tickets to review");
    assert_eq!(run["outcome"], "something", "{run}");
    let text = ok(&["routine", "show", "Tickets to review"]);
    assert!(text.contains("4 to review"), "{text}");

    ok(&["routine", "remove", "Tickets to review"]);
    let listed: Value = serde_json::from_str(&ok(&["routine", "list", "--json"])).unwrap();
    assert_eq!(listed["routines"], json!([]), "{listed}");
    let missing = user.run(&["routine", "show", "Tickets to review"]);
    assert!(!missing.status.success());
}
