//! The scenarios of routines that run on an event (#1125, `docs/design.md`, "Routines"): a
//! workflow run that ends, a review asked from the user, a mention of the user, a ticket assigned
//! to the user, each read from what the daemon's rounds of the items already read; held until
//! `not_before`, at most once a day or an hour, across a restart of the daemon; delivered to the
//! session of the last run while it is open. The forge and the tracker are the fake `gh` and
//! `acli`, the agent the fake ACP agent, the clock the one the daemon's tests move: no test waits
//! for an hour, runs a real agent or calls a forge.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::{Value, json};

const MINUTE: u64 = 60_000;
const HOUR: u64 = 60 * MINUTE;

/// The prompts of the routines, and what the fake agent does on each.
const NIGHTLY: &str = "Read why the nightly failed";
const BRANCH: &str = "Open a branch for the fix";
const IDLE: &str = "Read the project and wait";

fn script() -> Value {
    json!({"turns": [
        {"when": NIGHTLY, "do": [
            {"mcp": {"tool": "routine_result", "arguments":
                {"summary": "The login test fails", "nothing": false}}}]},
        {"when": BRANCH, "do": [
            {"mcp": {"tool": "routine_result", "arguments":
                {"summary": "fix/7 opened", "goes_on": true}}}]},
    ]})
}

/// A user whose daemon reads the items of its projects, with the fake `gh` and `acli`, on a test
/// clock.
fn user() -> User {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_acli()
        .with_fetcher()
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

/// A project `app` over a throwaway repository whose `origin` is GitHub's `acme/app`, its forge
/// declared, and `tracker` its tracker.
fn project(user: &User, tracker: Value) {
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
    let mut settings = json!({"name": "app",
        "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}]});
    if !tracker.is_null() {
        settings["tracker"] = tracker;
    }
    user.call("project.set_settings", settings).unwrap();
}

fn now(user: &User) -> u64 {
    user.call("test.advance", json!({"ms": 0})).unwrap()["now"]
        .as_u64()
        .unwrap()
}

fn advance(user: &User, ms: u64) {
    user.call("test.advance", json!({"ms": ms})).unwrap();
}

/// Moves the daemon's clock to the next `hour`:`minute` UTC.
fn move_clock_to(user: &User, hour: i8, minute: i8) {
    let here = Timestamp::from_millisecond(now(user) as i64)
        .unwrap()
        .to_zoned(TimeZone::UTC);
    let mut date = here.date();
    loop {
        let at = date.at(hour, minute, 0, 0).to_zoned(TimeZone::UTC).unwrap();
        if at > here {
            let wait = at.timestamp().as_millisecond() - here.timestamp().as_millisecond();
            advance(user, wait as u64);
            return;
        }
        date = date.tomorrow().unwrap();
    }
}

/// A round of every source of the project now.
fn refresh(user: &User) {
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

fn add(user: &User, name: &str, prompt: &str, event: Value) -> Value {
    user.call(
        "routine.add",
        json!({"name": name, "project": "app", "prompt": prompt, "event": event,
            "zone": "UTC"}),
    )
    .unwrap()["routine"]
        .clone()
}

/// The runs of the routine, the last first.
fn runs(user: &User, name: &str) -> Vec<Value> {
    user.call("routine.show", json!({"routine": name})).unwrap()["runs"]
        .as_array()
        .unwrap()
        .clone()
}

/// The runs of the routine that started a session.
fn started(user: &User, name: &str) -> Vec<Value> {
    runs(user, name)
        .into_iter()
        .filter(|run| !run["session"].is_null())
        .collect()
}

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

/// A run of GitHub Actions as `gh run list --json` says it; `path` is the workflow's file, which
/// the fake `gh` filters `--workflow` by.
fn workflow_run(id: u64, event: &str, conclusion: Option<&str>) -> Value {
    json!({"databaseId": id, "workflowName": "Nightly", "path": ".github/workflows/nightly.yml",
        "displayTitle": "Nightly tests", "headBranch": "dev", "event": event,
        "status": if conclusion.is_some() { "completed" } else { "in_progress" },
        "conclusion": conclusion.unwrap_or(""),
        "url": format!("https://github.com/acme/app/actions/runs/{id}"),
        "createdAt": "2026-11-20T02:00:00Z", "updatedAt": "2026-11-20T02:30:00Z"})
}

fn runs_are(user: &User, runs: Vec<Value>) {
    user.gh_script(json!({"runs": {"acme/app": runs}}));
}

/// The trigger of the nightly workflow on `dev`, started by its schedule, when it fails.
fn nightly(extra: Value) -> Value {
    let mut event = json!({"kind": "workflow_run", "workflow": "nightly.yml", "branch": "dev",
        "started_by": "schedule", "outcome": "failed"});
    event
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    event
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
fn an_event_routine_has_an_event_and_no_schedule() {
    let user = user();
    project(&user, Value::Null);
    let routine = add(&user, "Nightly fix", NIGHTLY, nightly(json!({})));
    assert!(routine["schedule"].is_null(), "{routine}");
    assert!(routine["next_at"].is_null(), "{routine}");
    assert_eq!(
        routine["event"],
        json!({"kind": "workflow_run", "repository": null, "workflow": "nightly.yml",
            "branch": "dev", "started_by": "schedule", "outcome": "failed",
            "not_before": null, "at_most_once": null, "new_session": false}),
        "the fields left out are their defaults"
    );
    let defaults = add(
        &user,
        "Any review",
        NIGHTLY,
        json!({"kind": "review_requested"}),
    );
    assert_eq!(
        defaults["event"],
        json!({"kind": "review_requested", "repository": null, "not_before": null,
            "at_most_once": null, "new_session": false})
    );

    let invalid = |params: Value| {
        let mut params = params;
        params["project"] = json!("app");
        params["prompt"] = json!(NIGHTLY);
        let (code, message) = user.call("routine.add", params).unwrap_err();
        assert_eq!(code, "invalid-params", "{message}");
    };
    invalid(json!({"name": "Both", "schedule": "every day 07:00", "event": nightly(json!({}))}));
    invalid(json!({"name": "Neither"}));
    invalid(json!({"name": "Not its field",
        "event": {"kind": "mentioned", "workflow": "nightly.yml"}}));
    invalid(json!({"name": "No kind", "event": {"kind": "sometimes"}}));
    invalid(json!({"name": "Bad hour", "event": nightly(json!({"not_before": "8h"}))}));
    invalid(json!({"name": "Bad once", "event": nightly(json!({"at_most_once": "week"}))}));
    invalid(json!({"name": "Bad start", "event": nightly(json!({"started_by": "cron"}))}));

    // A schedule given on edit replaces the event, and an event the schedule.
    let edited = user
        .call(
            "routine.edit",
            json!({"routine": "Nightly fix", "schedule": "every day 07:00"}),
        )
        .unwrap()["routine"]
        .clone();
    assert_eq!(edited["schedule"], "every day 07:00", "{edited}");
    assert!(edited["event"].is_null(), "{edited}");
    assert!(edited["next_at"].is_u64(), "{edited}");
    let back = user
        .call(
            "routine.edit",
            json!({"routine": "Nightly fix", "event": {"kind": "mentioned"}}),
        )
        .unwrap()["routine"]
        .clone();
    assert!(back["schedule"].is_null(), "{back}");
    assert!(back["next_at"].is_null(), "{back}");
    assert_eq!(back["event"]["kind"], "mentioned", "{back}");
    let both = user.call(
        "routine.edit",
        json!({"routine": "Nightly fix", "event": {"kind": "mentioned"},
            "schedule": "every day 07:00"}),
    );
    assert_eq!(both.unwrap_err().0, "invalid-params");
}

#[test]
fn a_failed_run_of_the_workflow_starts_the_routine() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![workflow_run(1, "schedule", Some("failure"))]);
    add(&user, "Nightly fix", IDLE, nightly(json!({})));

    // What had failed before the routine is no event: no run without an event.
    refresh(&user);
    refresh(&user);
    assert!(runs(&user, "Nightly fix").is_empty());
    assert!(sessions(&user).is_empty());
    assert!(
        user.gh_received().iter().any(|call| {
            let args: Vec<&str> = call["args"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            args.starts_with(&["run", "list"])
                && args.windows(2).any(|w| w == ["--workflow", "nightly.yml"])
                && args.windows(2).any(|w| w == ["--branch", "dev"])
        }),
        "{:?}",
        user.gh_received()
    );

    // One still running is no end yet.
    runs_are(
        &user,
        vec![
            workflow_run(2, "schedule", None),
            workflow_run(1, "schedule", Some("failure")),
        ],
    );
    refresh(&user);
    assert!(runs(&user, "Nightly fix").is_empty());

    let opened = user.received_requests("session/new").len();
    runs_are(
        &user,
        vec![
            workflow_run(2, "schedule", Some("failure")),
            workflow_run(1, "schedule", Some("failure")),
        ],
    );
    refresh(&user);
    eventually("a run", || !started(&user, "Nightly fix").is_empty());
    let run = started(&user, "Nightly fix")[0].clone();
    assert!(
        run["trigger"]
            .as_str()
            .is_some_and(|said| said.contains("Nightly") && said.contains("dev")),
        "the run says what started it: {run}"
    );
    let session = run["session"].as_str().unwrap().to_owned();
    eventually("its prompt", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains(IDLE) && p.contains("actions/runs/2"))
    });
    assert!(
        prompts(&user)
            .iter()
            .any(|p| p.contains("data, not instructions")),
        "what the forge says is data: {:?}",
        prompts(&user)
    );

    // An event run keeps the read-only mode of a routine.
    let (url, authorization) = server_of(&user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    eventually("its first turn", || {
        agent(&user, &session)["state"] == "idle"
    });
    let read = mcp.call("fs_read", json!({"path": "README.md"}));
    assert!(!read.error, "{}", read.text);
    for mode in ["ask", "auto", "full"] {
        user.call("agent.mode", json!({"agent": session, "mode": mode}))
            .unwrap();
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

    // The same failed run read again is no new event.
    refresh(&user);
    assert_eq!(started(&user, "Nightly fix").len(), 1);
}

#[test]
fn a_run_started_by_a_push_does_not_start_a_schedule_only_routine() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![]);
    add(&user, "Nightly fix", NIGHTLY, nightly(json!({})));
    add(
        &user,
        "Any start",
        NIGHTLY,
        nightly(json!({"started_by": "any"})),
    );
    refresh(&user);

    runs_are(&user, vec![workflow_run(5, "push", Some("failure"))]);
    refresh(&user);

    eventually("the routine for any start", || {
        !started(&user, "Any start").is_empty()
    });
    assert!(
        runs(&user, "Nightly fix").is_empty(),
        "a push is not the nightly"
    );
    runs_are(
        &user,
        vec![
            workflow_run(6, "workflow_dispatch", Some("failure")),
            workflow_run(5, "push", Some("failure")),
        ],
    );
    refresh(&user);
    assert!(
        runs(&user, "Nightly fix").is_empty(),
        "nor is a run by hand"
    );
}

#[test]
fn outcome_any_starts_the_routine_on_a_green_run() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![]);
    add(&user, "Nightly fix", NIGHTLY, nightly(json!({})));
    add(
        &user,
        "Every night",
        NIGHTLY,
        nightly(json!({"outcome": "any"})),
    );
    refresh(&user);

    runs_are(&user, vec![workflow_run(3, "schedule", Some("success"))]);
    refresh(&user);

    eventually("the run on any outcome", || {
        !started(&user, "Every night").is_empty()
    });
    assert!(
        runs(&user, "Nightly fix").is_empty(),
        "a green night leaves no row"
    );
}

#[test]
fn not_before_holds_the_run_until_its_hour() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![]);
    move_clock_to(&user, 2, 0);
    add(
        &user,
        "Morning fix",
        NIGHTLY,
        nightly(json!({"not_before": "08:00"})),
    );
    refresh(&user);

    runs_are(&user, vec![workflow_run(7, "schedule", Some("failure"))]);
    refresh(&user);
    assert!(runs(&user, "Morning fix").is_empty(), "held until 08:00");

    advance(&user, 5 * HOUR + 59 * MINUTE);
    std::thread::sleep(Duration::from_millis(300));
    assert!(runs(&user, "Morning fix").is_empty(), "07:59: still held");

    advance(&user, MINUTE);
    let run = ended_run(&user, "Morning fix");
    assert_eq!(run["outcome"], "something", "{run}");
    assert_eq!(started(&user, "Morning fix").len(), 1);
}

#[test]
fn at_most_once_a_day() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![]);
    move_clock_to(&user, 9, 0);
    add(
        &user,
        "Once a day",
        NIGHTLY,
        nightly(json!({"at_most_once": "day"})),
    );
    refresh(&user);

    runs_are(&user, vec![workflow_run(1, "schedule", Some("failure"))]);
    refresh(&user);
    ended_run(&user, "Once a day");

    advance(&user, HOUR);
    runs_are(
        &user,
        vec![
            workflow_run(2, "schedule", Some("failure")),
            workflow_run(1, "schedule", Some("failure")),
        ],
    );
    refresh(&user);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(started(&user, "Once a day").len(), 1, "not twice in a day");

    // The next day runs again.
    move_clock_to(&user, 9, 0);
    runs_are(
        &user,
        vec![
            workflow_run(3, "schedule", Some("failure")),
            workflow_run(2, "schedule", Some("failure")),
            workflow_run(1, "schedule", Some("failure")),
        ],
    );
    refresh(&user);
    eventually("the next day's run", || {
        started(&user, "Once a day").len() == 2
    });
}

/// Stops the daemon and starts it again, its clock moved back to where it was, plus `ms`.
fn restart(user: &User, ms: u64) {
    let at = now(user);
    assert!(user.run(&["daemon", "stop"]).status.success());
    user.status();
    let back = now(user);
    advance(user, at - back + ms);
}

#[test]
fn a_night_of_failures_is_one_run_in_the_morning_across_a_restart() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![]);
    move_clock_to(&user, 1, 0);
    add(
        &user,
        "Morning fix",
        NIGHTLY,
        nightly(json!({"not_before": "08:00", "at_most_once": "day"})),
    );
    refresh(&user);
    let mut night = Vec::new();
    for id in 1..=2 {
        night.insert(0, workflow_run(id, "schedule", Some("failure")));
        runs_are(&user, night.clone());
        refresh(&user);
        advance(&user, HOUR);
    }
    // 03:00: the daemon restarts; two more failures come after it.
    restart(&user, 0);
    for id in 3..=4 {
        night.insert(0, workflow_run(id, "schedule", Some("failure")));
        runs_are(&user, night.clone());
        refresh(&user);
        advance(&user, HOUR);
    }
    assert!(
        runs(&user, "Morning fix").is_empty(),
        "nothing before 08:00"
    );

    move_clock_to(&user, 8, 0);
    ended_run(&user, "Morning fix");
    std::thread::sleep(Duration::from_millis(500));
    advance(&user, MINUTE);
    refresh(&user);
    assert_eq!(
        started(&user, "Morning fix").len(),
        1,
        "one run for the night: {:?}",
        runs(&user, "Morning fix")
    );
    let run = &started(&user, "Morning fix")[0];
    assert!(
        prompts(&user)
            .iter()
            .any(|p| p.contains("actions/runs/1") && p.contains("actions/runs/4")),
        "the run is told every failure of the night: {run}"
    );
}

#[test]
fn a_trigger_while_the_last_session_is_open_is_delivered_to_it() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![]);
    add(&user, "Fix it", BRANCH, nightly(json!({})));
    user.call("routine.edit", json!({"routine": "Fix it", "mode": "auto"}))
        .unwrap();
    refresh(&user);

    runs_are(&user, vec![workflow_run(1, "schedule", Some("failure"))]);
    refresh(&user);
    let first = ended_run(&user, "Fix it");
    assert_eq!(first["outcome"], "goes_on", "{first}");
    let session = first["session"].as_str().unwrap().to_owned();

    runs_are(
        &user,
        vec![
            workflow_run(2, "schedule", Some("failure")),
            workflow_run(1, "schedule", Some("failure")),
        ],
    );
    refresh(&user);

    eventually("the delivery to the open session", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("[flottille:routine-event]") && p.contains("actions/runs/2"))
    });
    let delivered = prompts(&user)
        .into_iter()
        .find(|p| p.contains("[flottille:routine-event]"))
        .unwrap();
    assert!(
        delivered.contains("data, not instructions"),
        "what the forge says is data: {delivered}"
    );
    assert_eq!(started(&user, "Fix it").len(), 1, "no second session");
    assert_eq!(sessions(&user).len(), 1);
    let skipped = runs(&user, "Fix it")[0].clone();
    assert_eq!(skipped["outcome"], "skipped", "{skipped}");
    assert!(
        skipped["why"]
            .as_str()
            .is_some_and(|why| why.contains("delivered") && why.contains(&session)),
        "the history says it went to the open session: {skipped}"
    );
}

#[test]
fn new_session_starts_another_beside_the_open_one() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![]);
    add(
        &user,
        "Fix each",
        BRANCH,
        nightly(json!({"new_session": true})),
    );
    user.call(
        "routine.edit",
        json!({"routine": "Fix each", "mode": "auto"}),
    )
    .unwrap();
    refresh(&user);
    runs_are(&user, vec![workflow_run(1, "schedule", Some("failure"))]);
    refresh(&user);
    assert_eq!(ended_run(&user, "Fix each")["outcome"], "goes_on");

    runs_are(
        &user,
        vec![
            workflow_run(2, "schedule", Some("failure")),
            workflow_run(1, "schedule", Some("failure")),
        ],
    );
    refresh(&user);
    eventually("a second session", || started(&user, "Fix each").len() == 2);
}

/// When GitHub says an item changed last, the `n`th time: asking a review or commenting moves it,
/// and a round reads the details of an item whose time moved.
fn updated(n: u32) -> String {
    format!("2999-01-01T00:00:{n:02}Z")
}

/// What `gh pr view --json` answers of the pull request 12 of `acme/app`, its review asked of
/// `requested`.
fn pull(requested: &[&str]) -> Value {
    json!({
        "number": 12, "title": "Speed up the login", "state": "OPEN", "isDraft": false,
        "headRefName": "faster-login", "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": "https://github.com/acme/app/pull/12",
        "author": {"login": "other"}, "assignees": [], "labels": [],
        "updatedAt": updated(requested.len() as u32), "createdAt": "2026-11-01T10:00:00Z",
        "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
        "reviewRequests": requested.iter().map(|login| json!({"login": login}))
            .collect::<Vec<_>>(),
        "statusCheckRollup": [], "comments": [], "reviews": []
    })
}

#[test]
fn a_review_asked_from_the_user_starts_the_routine() {
    let user = user();
    project(&user, Value::Null);
    user.gh_script(json!({"pulls": {"acme/app": {"faster-login": pull(&[])}}}));
    add(
        &user,
        "Reviews",
        NIGHTLY,
        json!({"kind": "review_requested", "repository": "acme/app"}),
    );
    refresh(&user);

    // Asked of someone else: not the user's review.
    user.gh_script(json!({"pulls": {"acme/app": {"faster-login": pull(&["dana"])}}}));
    refresh(&user);
    assert!(runs(&user, "Reviews").is_empty());

    // The fake `gh` says the user is `someone`.
    user.gh_script(json!({"pulls": {"acme/app": {"faster-login": pull(&["dana", "someone"])}}}));
    refresh(&user);
    eventually("a run", || !started(&user, "Reviews").is_empty());
    eventually("its prompt", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains(NIGHTLY) && p.contains("Speed up the login"))
    });
    refresh(&user);
    assert_eq!(started(&user, "Reviews").len(), 1, "asked once, run once");
}

/// What `gh issue view --json` answers of the issue 7 of `acme/app`, with `comments`.
fn issue(comments: &[&str], assignees: &[&str]) -> Value {
    json!({
        "number": 7, "title": "Login fails", "state": "OPEN", "body": "It fails.",
        "url": "https://github.com/acme/app/issues/7",
        "author": {"login": "other"},
        "assignees": assignees.iter().map(|login| json!({"login": login})).collect::<Vec<_>>(),
        "labels": [], "updatedAt": updated(comments.len() as u32),
        "createdAt": "2026-11-01T10:00:00Z",
        "comments": comments.iter().enumerate().map(|(n, body)| json!({
            "author": {"login": "dana"}, "body": body, "createdAt": "2026-11-20T10:00:00Z",
            "url": format!("https://github.com/acme/app/issues/7#issuecomment-{}", n + 1)}))
            .collect::<Vec<_>>()
    })
}

#[test]
fn a_mention_of_the_user_starts_the_routine() {
    let user = user();
    project(&user, json!({"kind": "github", "repository": "."}));
    user.gh_script(json!({"issues": {"acme/app": {"7": issue(&[], &[])}}}));
    add(&user, "Mentions", NIGHTLY, json!({"kind": "mentioned"}));
    refresh(&user);

    user.gh_script(json!({"issues": {"acme/app": {"7": issue(&["It fails for me too."], &[])}}}));
    refresh(&user);
    assert!(runs(&user, "Mentions").is_empty(), "a comment of no one");

    user.gh_script(json!({"issues": {"acme/app": {"7": issue(
        &["It fails for me too.", "@someone could you look at the session cookie?"], &[])}}}));
    refresh(&user);
    eventually("a run", || !started(&user, "Mentions").is_empty());
    eventually("its prompt", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains(NIGHTLY) && p.contains("session cookie"))
    });
}

/// What `acli jira workitem search` prints of the work item ABC-1, assigned to `assignee`.
fn work_item(assignee: &str, kind: &str) -> Value {
    json!({
        "key": "ABC-1",
        "self": "https://example.atlassian.net/rest/api/3/issue/10001",
        "fields": {
            "summary": "Log in with a passkey",
            "status": {"name": "To Do", "statusCategory": {"key": "new"}},
            "assignee": {"displayName": assignee},
            "reporter": {"displayName": "Alice"},
            "labels": [],
            "issuetype": {"name": kind},
            "priority": {"name": "High"}
        }
    })
}

#[test]
fn a_ticket_assigned_to_the_user_starts_the_routine() {
    let user = user();
    project(
        &user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    user.acli_script(json!({"search": [work_item("Alice", "Story")]}));
    add(
        &user,
        "Stories for me",
        NIGHTLY,
        json!({"kind": "assigned", "tracker_project": "ABC", "type": "Story"}),
    );
    add(
        &user,
        "Bugs for me",
        NIGHTLY,
        json!({"kind": "assigned", "type": "Bug"}),
    );
    refresh(&user);
    assert!(runs(&user, "Stories for me").is_empty());

    // The fake `acli` says the user is Casey Example: the search of theirs finds it now.
    let mine = work_item("Casey Example", "Story");
    user.acli_script(json!({"search": [mine.clone()], "mine_search": [mine]}));
    refresh(&user);
    eventually("a run", || !started(&user, "Stories for me").is_empty());
    eventually("its prompt", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains(NIGHTLY) && p.contains("ABC-1"))
    });
    assert!(runs(&user, "Bugs for me").is_empty(), "not of its type");
}

#[test]
fn routine_event_commands_say_the_trigger_and_why_a_run_was_skipped() {
    let user = user();
    project(&user, Value::Null);
    runs_are(&user, vec![]);
    let ok = |args: &[&str]| -> String {
        let output = user.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let added: Value = serde_json::from_str(&ok(&[
        "routine",
        "add",
        "Nightly fix",
        "--project",
        "app",
        "--prompt",
        BRANCH,
        "--on",
        "workflow-run",
        "--workflow",
        "nightly.yml",
        "--branch",
        "dev",
        "--started-by",
        "schedule",
        "--outcome",
        "failed",
        "--not-before",
        "08:00",
        "--at-most-once",
        "day",
        "--mode",
        "auto",
        "--zone",
        "UTC",
        "--json",
    ]))
    .unwrap();
    assert_eq!(
        added["routine"]["event"],
        json!({"kind": "workflow_run", "repository": null, "workflow": "nightly.yml",
            "branch": "dev", "started_by": "schedule", "outcome": "failed",
            "not_before": "08:00", "at_most_once": "day", "new_session": false}),
        "{added}"
    );
    let text = ok(&["routine", "list"]);
    assert!(
        text.contains("when nightly.yml fails on dev, started by its schedule"),
        "{text}"
    );
    assert!(text.contains("not before 08:00"), "{text}");
    assert!(text.contains("at most once a day"), "{text}");

    ok(&[
        "routine",
        "edit",
        "Nightly fix",
        "--on",
        "assigned",
        "--tracker-project",
        "ABC",
        "--type",
        "Bug",
    ]);
    let shown: Value =
        serde_json::from_str(&ok(&["routine", "show", "Nightly fix", "--json"])).unwrap();
    assert_eq!(
        shown["routine"]["event"],
        json!({"kind": "assigned", "tracker_project": "ABC", "type": "Bug",
            "not_before": null, "at_most_once": null, "new_session": false}),
        "the event is replaced, its fields left out their defaults: {shown}"
    );
    let text = ok(&["routine", "show", "Nightly fix"]);
    assert!(
        text.contains("when a Bug of ABC is assigned to you"),
        "{text}"
    );

    // Back to the workflow, now run: the second failure goes to the open session, said.
    ok(&[
        "routine",
        "edit",
        "Nightly fix",
        "--on",
        "workflow-run",
        "--workflow",
        "nightly.yml",
    ]);
    refresh(&user);
    runs_are(&user, vec![workflow_run(1, "schedule", Some("failure"))]);
    refresh(&user);
    assert_eq!(ended_run(&user, "Nightly fix")["outcome"], "goes_on");
    runs_are(
        &user,
        vec![
            workflow_run(2, "schedule", Some("failure")),
            workflow_run(1, "schedule", Some("failure")),
        ],
    );
    refresh(&user);
    eventually("the skip", || {
        runs(&user, "Nightly fix")[0]["outcome"] == "skipped"
    });
    let text = ok(&["routine", "show", "Nightly fix"]);
    assert!(
        text.contains("skipped: delivered to its session still open"),
        "{text}"
    );
    let refused = user.run(&[
        "routine",
        "add",
        "Both",
        "--project",
        "app",
        "--prompt",
        NIGHTLY,
        "--schedule",
        "every day 07:00",
        "--on",
        "mentioned",
    ]);
    assert!(!refused.status.success());
}
