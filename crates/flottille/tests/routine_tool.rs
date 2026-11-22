//! The scenarios of the tool `routine` (#1129, `docs/tools.md`, "Routines"): an agent lists,
//! shows and reads the results of its project's routines at once, and creates, edits, pauses,
//! resumes, runs and removes them through the gate, as the other writes of the project; it never
//! gives a routine a mode above its own session's, a run's session is read with `session_read`,
//! and a routine an agent made says so. The agents are the fake ACP agent's sessions, driven
//! through their MCP server as their client would, on throwaway repositories and temporary XDG
//! folders: no test runs a real agent.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

const LIST: &str = "List the tickets waiting for my review";
const HOLD: &str = "Look through the whole backlog slowly";
const DAY: u64 = 86_400_000;

fn script() -> Value {
    json!({"turns": [
        {"when": HOLD, "do": [{"say": "Reading the backlog."}, {"hold": "release"}]},
    ]})
}

/// The fake agent's routine sessions end with a list of `items`.
fn listing(user: &User, summary: &str, items: Value) {
    user.fake_script(json!({"turns": [
        {"when": LIST, "do": [{"mcp": {"tool": "routine_result",
            "arguments": {"summary": summary, "items": items, "body": "Two block a release."}}}]},
        {"when": HOLD, "do": [{"say": "Reading the backlog."}, {"hold": "release"}]},
    ]}));
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

fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &[
            "remote",
            "add",
            "origin",
            &format!("git@github.com:acme/{name}.git"),
        ],
    );
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

/// A routine the user adds to `project`.
fn add(user: &User, project: &str, name: &str, extra: Value) {
    let mut params = json!({"name": name, "project": project, "prompt": LIST,
        "schedule": "every day 07:00", "zone": "UTC"});
    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }
    user.call("routine.add", params).unwrap();
}

fn show(user: &User, name: &str) -> Value {
    user.call("routine.show", json!({"routine": name})).unwrap()
}

fn runs(user: &User, name: &str) -> Vec<Value> {
    show(user, name)["runs"].as_array().unwrap().clone()
}

/// Starts a run by hand and waits for its result.
fn run_once(user: &User, name: &str) -> Value {
    let before = runs(user, name).len();
    user.call("routine.run", json!({"routine": name})).unwrap();
    eventually("the run's end", || {
        let all = runs(user, name);
        all.len() > before && all[0]["outcome"] != "running"
    });
    runs(user, name)[0].clone()
}

/// A root the user starts in `app`, its first turn ended, in `mode`: its id and MCP session.
fn root(user: &User, mode: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    user.call("agent.mode", json!({"agent": id, "mode": mode}))
        .unwrap();
    (id, Mcp::initialize(&url, &authorization))
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

fn json_of(answer: Answer) -> Value {
    let text = done(answer);
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}"))
}

fn held(answer: Answer) -> u64 {
    assert!(!answer.error, "an error: {}", answer.text);
    let rest = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    rest.split('.').next().unwrap().parse().unwrap()
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.starts_with("refused: "), "{}", answer.text);
    answer.text
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

fn request(user: &User, number: u64) -> Value {
    requests(user)
        .into_iter()
        .find(|r| r["number"] == number)
        .unwrap_or_else(|| panic!("no pending request #{number}"))
}

fn names(listed: &Value) -> Vec<String> {
    listed["routines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|routine| routine["name"].as_str().unwrap().to_owned())
        .collect()
}

fn ids(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

/// The day of `ms` in UTC, `YYYY-MM-DD`.
fn day_of(ms: u64) -> String {
    let days = (ms / DAY) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[test]
fn an_agent_lists_and_shows_routines() {
    let user = user();
    project(&user, "app");
    project(&user, "kit");
    add(&user, "app", "Tickets to review", json!({}));
    add(
        &user,
        "app",
        "Nightly",
        json!({"schedule": "weekdays 08:30", "mode": "ask"}),
    );
    add(&user, "kit", "Kit releases", json!({}));
    listing(
        &user,
        "2 to review",
        json!([{"id": "ABC-1", "title": "Login"}]),
    );
    let run = run_once(&user, "Tickets to review");
    let (_, mcp) = root(&user, "ask");

    let listed = json_of(mcp.call("routine", json!({"action": "list"})));
    assert_eq!(
        names(&listed),
        ["Nightly", "Tickets to review"],
        "its own project's alone: {listed}"
    );
    let tickets = listed["routines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Tickets to review")
        .unwrap();
    assert_eq!(tickets["last"]["summary"], "2 to review");
    assert_eq!(tickets["schedule"], "every day 07:00");

    let shown = json_of(mcp.call(
        "routine",
        json!({"action": "show", "routine": "Tickets to review"}),
    ));
    assert_eq!(shown["routine"]["prompt"], LIST);
    assert_eq!(shown["routine"]["mode"], "read_only");
    assert!(
        shown["routine"]["next_at"].is_u64(),
        "its next run: {shown}"
    );
    assert_eq!(shown["runs"][0]["session"], run["session"], "its history");
    assert_eq!(shown["runs"][0]["outcome"], "something");

    let other = refused(mcp.call(
        "routine",
        json!({"action": "show", "routine": "Kit releases"}),
    ));
    assert!(other.contains("Kit releases"), "{other}");
    assert!(requests(&user).is_empty(), "reads are asked of no one");
}

#[test]
fn an_agent_reads_the_last_result_with_its_items() {
    let user = user();
    project(&user, "app");
    add(&user, "app", "Tickets to review", json!({}));
    // Paused, its schedule starts nothing while the clock moves a day: only the runs by hand.
    user.call("routine.pause", json!({"routine": "Tickets to review"}))
        .unwrap();
    listing(
        &user,
        "2 to review",
        json!([{"id": "ABC-1", "title": "Login"}, {"id": "ABC-2", "title": "Footer"}]),
    );
    let first = run_once(&user, "Tickets to review");
    user.call("test.advance", json!({"ms": DAY})).unwrap();
    listing(
        &user,
        "2 to review",
        json!([{"id": "ABC-1", "title": "Login"}, {"id": "ABC-3", "title": "Slow start",
            "tags": ["urgent"]}]),
    );
    let second = run_once(&user, "Tickets to review");
    let (_, mcp) = root(&user, "ask");

    let last = json_of(mcp.call(
        "routine",
        json!({"action": "result", "routine": "Tickets to review"}),
    ));
    let run = &last["run"];
    assert_eq!(run["id"], second["id"], "the last one by default: {last}");
    assert_eq!(run["summary"], "2 to review");
    assert_eq!(run["session"], second["session"], "its session's id");
    assert_eq!(ids(&run["items"]), ["ABC-1", "ABC-3"]);
    assert_eq!(run["items"][0]["new"], false);
    assert_eq!(run["items"][1]["new"], true, "new since the last run");
    assert_eq!(run["items"][1]["tags"], json!(["urgent"]));
    assert_eq!(run["left"], json!([{"id": "ABC-2", "title": "Footer"}]));
    assert_eq!(run["body"], "Two block a release.", "its report");

    let date = day_of(first["started_at"].as_u64().unwrap());
    let earlier = json_of(mcp.call(
        "routine",
        json!({"action": "result", "routine": "Tickets to review", "date": date}),
    ));
    assert_eq!(earlier["run"]["id"], first["id"], "by its date: {earlier}");
    assert_eq!(ids(&earlier["run"]["items"]), ["ABC-1", "ABC-2"]);

    let none = refused(mcp.call(
        "routine",
        json!({"action": "result", "routine": "Tickets to review", "date": "2001-01-01"}),
    ));
    assert!(none.contains("2001-01-01"), "{none}");
    assert!(requests(&user).is_empty(), "reads are asked of no one");
}

#[test]
fn creating_a_routine_is_asked_in_ask_mode_and_goes_through_in_full() {
    let user = user();
    project(&user, "app");
    let (id, mcp) = root(&user, "ask");

    let create = json!({"action": "create", "name": "Tickets to review", "prompt": LIST,
        "schedule": "weekdays 08:30"});
    let number = held(mcp.call("routine", create));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "action");
    assert_eq!(asked["class"], "judge");
    let says = asked["says"].to_string();
    assert!(says.contains("Tickets to review"), "{says}");
    assert!(says.contains("weekdays 08:30"), "{says}");
    assert!(says.contains(LIST), "the prompt whole: {says}");
    assert!(
        user.call("routine.list", json!({})).unwrap()["routines"]
            .as_array()
            .unwrap()
            .is_empty(),
        "nothing before the user's word"
    );
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the routine is made", || {
        user.call("routine.list", json!({})).unwrap()["routines"]
            .as_array()
            .unwrap()
            .len()
            == 1
    });
    let made = show(&user, "Tickets to review")["routine"].clone();
    assert_eq!(made["project"], "app", "its own project");
    assert_eq!(made["mode"], "read_only", "read only by default");
    assert_eq!(made["made_by"]["session"], id.as_str());

    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    let text = done(mcp.call(
        "routine",
        json!({"action": "create", "name": "Nightly", "prompt": LIST,
            "schedule": "every day 07:00", "mode": "auto"}),
    ));
    assert!(text.contains("Nightly"), "{text}");
    assert_eq!(show(&user, "Nightly")["routine"]["mode"], "auto");
    assert!(requests(&user).is_empty(), "nothing asked in Full");
}

#[test]
fn a_routine_cannot_get_a_mode_above_its_author() {
    let user = user();
    project(&user, "app");
    add(&user, "app", "Nightly", json!({"mode": "auto"}));
    let (_, mcp) = root(&user, "ask");

    let above = refused(mcp.call(
        "routine",
        json!({"action": "create", "name": "Fixer", "prompt": LIST,
            "schedule": "every day 07:00", "mode": "auto"}),
    ));
    assert!(above.contains("auto") && above.contains("ask"), "{above}");
    held(mcp.call(
        "routine",
        json!({"action": "create", "name": "Asker", "prompt": LIST,
            "schedule": "every day 07:00", "mode": "ask"}),
    ));
    held(mcp.call(
        "routine",
        json!({"action": "create", "name": "Reader", "prompt": LIST,
            "schedule": "every day 07:00"}),
    ));
    // An edit leaves a routine whose mode is above the session's: its words would run in Auto.
    let edit = refused(mcp.call(
        "routine",
        json!({"action": "edit", "routine": "Nightly", "prompt": "Push to main"}),
    ));
    assert!(edit.contains("auto"), "{edit}");
    // Lowering it is no more than the session's.
    held(mcp.call(
        "routine",
        json!({"action": "edit", "routine": "Nightly", "mode": "ask"}),
    ));

    // A read-only routine's run writes nothing, a routine included.
    add(&user, "app", "Reading", json!({"prompt": HOLD}));
    let opened = user.received_requests("session/new").len();
    user.call("routine.run", json!({"routine": "Reading"}))
        .unwrap();
    let (url, authorization) = server_of(&user, opened);
    let run = Mcp::initialize(&url, &authorization);
    let refusal = refused(run.call(
        "routine",
        json!({"action": "create", "name": "Copy", "prompt": LIST,
            "schedule": "every day 07:00"}),
    ));
    assert!(refusal.contains("read-only routine"), "{refusal}");
    std::fs::write(user.path("fake/release"), "").unwrap();
    assert!(
        user.call("routine.show", json!({"routine": "Copy"}))
            .is_err(),
        "never made"
    );
}

#[test]
fn a_request_above_the_session_s_mode_when_it_is_done_is_not_done() {
    let user = user();
    project(&user, "app");
    let (id, mcp) = root(&user, "auto");

    // In Auto with no judge chosen, the call waits for the user.
    let number = held(mcp.call(
        "routine",
        json!({"action": "create", "name": "Later", "prompt": LIST,
            "schedule": "every day 07:00", "mode": "auto"}),
    ));
    user.call("agent.mode", json!({"agent": id, "mode": "ask"}))
        .unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        user.call("routine.show", json!({"routine": "Later"}))
            .is_err(),
        "its session is Ask now"
    );
}

#[test]
fn run_pause_resume_and_remove_through_the_tool() {
    let user = user();
    project(&user, "app");
    add(&user, "app", "Tickets to review", json!({}));
    listing(
        &user,
        "1 to review",
        json!([{"id": "ABC-1", "title": "Login"}]),
    );
    let (id, mcp) = root(&user, "ask");

    for action in ["pause", "resume", "run", "remove"] {
        let number = held(mcp.call(
            "routine",
            json!({"action": action, "routine": "Tickets to review"}),
        ));
        assert_eq!(request(&user, number)["class"], "judge", "{action}");
        user.call("approval.deny", json!({"number": number}))
            .unwrap();
    }
    assert_eq!(show(&user, "Tickets to review")["routine"]["paused"], false);

    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    done(mcp.call(
        "routine",
        json!({"action": "pause", "routine": "Tickets to review"}),
    ));
    let paused = show(&user, "Tickets to review")["routine"].clone();
    assert_eq!(paused["paused"], true);
    assert_eq!(paused["next_at"], Value::Null);
    done(mcp.call(
        "routine",
        json!({"action": "resume", "routine": "Tickets to review"}),
    ));
    assert_eq!(show(&user, "Tickets to review")["routine"]["paused"], false);

    let started = done(mcp.call(
        "routine",
        json!({"action": "run", "routine": "Tickets to review"}),
    ));
    assert!(started.contains("session"), "its session: {started}");
    eventually("the run's end", || {
        runs(&user, "Tickets to review")
            .first()
            .is_some_and(|run| run["outcome"] == "something")
    });

    done(mcp.call(
        "routine",
        json!({"action": "remove", "routine": "Tickets to review"}),
    ));
    assert!(
        user.call("routine.show", json!({"routine": "Tickets to review"}))
            .is_err()
    );
    assert!(requests(&user).is_empty());
}

#[test]
fn a_running_routine_session_is_read_with_session_read() {
    let user = user();
    project(&user, "app");
    add(&user, "app", "Backlog", json!({"prompt": HOLD}));
    let (_, mcp) = root(&user, "full");
    user.call("routine.run", json!({"routine": "Backlog"}))
        .unwrap();
    eventually("the run's session says something", || {
        runs(&user, "Backlog")
            .first()
            .and_then(|run| run["session"].as_str().map(str::to_owned))
            .is_some_and(|session| {
                user.call("agent.show", json!({"agent": session})).unwrap()["agent"]["state"]
                    == "working"
            })
    });

    let shown = json_of(mcp.call("routine", json!({"action": "show", "routine": "Backlog"})));
    let session = shown["runs"][0]["session"].as_str().unwrap().to_owned();
    assert_eq!(shown["runs"][0]["outcome"], "running");
    let page = done(mcp.call("session_read", json!({"session": session})));
    assert!(page.contains(HOLD), "its prompt: {page}");

    std::fs::write(user.path("fake/release"), "").unwrap();
    eventually("the run's end", || {
        runs(&user, "Backlog")[0]["outcome"] != "running"
    });
    let page = done(mcp.call("session_read", json!({"session": session})));
    assert!(page.contains("Reading the backlog."), "ended too: {page}");
}

#[test]
fn a_routine_made_by_an_agent_says_so() {
    let user = user();
    project(&user, "app");
    add(&user, "app", "By hand", json!({}));
    let (id, mcp) = root(&user, "full");
    done(mcp.call("session_title", json!({"title": "Plan the mornings"})));

    done(mcp.call(
        "routine",
        json!({"action": "create", "name": "Mornings", "prompt": LIST,
            "schedule": "weekdays 08:30"}),
    ));

    let made = show(&user, "Mornings")["routine"].clone();
    assert_eq!(
        made["made_by"],
        json!({"session": id, "title": "Plan the mornings"})
    );
    let listed = user.call("routine.list", json!({})).unwrap();
    let mornings = listed["routines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Mornings")
        .unwrap();
    assert_eq!(mornings["made_by"]["title"], "Plan the mornings");
    assert_eq!(
        show(&user, "By hand")["routine"]["made_by"],
        Value::Null,
        "the user's own says nothing"
    );
    // The user edits and removes it as any other.
    user.call(
        "routine.edit",
        json!({"routine": "Mornings", "prompt": "List them all"}),
    )
    .unwrap();
    assert_eq!(
        show(&user, "Mornings")["routine"]["made_by"]["session"],
        id.as_str(),
        "an edit of the user's keeps who made it"
    );
    user.call("routine.remove", json!({"routine": "Mornings"}))
        .unwrap();
}
