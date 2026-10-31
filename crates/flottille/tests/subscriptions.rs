//! The scenarios of the subscriptions (`docs/design.md` §5.6, `docs/tools.md`, "The forge"): an
//! agent subscribes once to the next event of its workspace's pull request, and the first one that
//! matches wakes it as a delivery. Read through the fake `gh` (`crates/flottille-fake-gh`), the
//! clock moved by the test; no test ever calls GitHub.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// A user whose agents are the fake agent and whose forge is the fake `gh`, with the clock tests
/// move.
fn user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// A project `app` of one repository, `api` on GitHub `acme/api`, declared, and its ready workspace
/// `login` on the branch `app/login`.
fn fixture(user: &User) {
    let folder = user.path("work/app");
    let dir = folder.join("api");
    user.git_init(&dir);
    std::fs::write(dir.join("README.md"), "api\n").unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/api.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
    user.git(&dir, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    user.git(
        &dir,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [
            {"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}
        ]}),
    )
    .unwrap();
}

/// A check run: `build` or a job of the repository's Actions.
fn check(name: &str, status: &str, conclusion: &str, job: u64) -> Value {
    json!({"__typename": "CheckRun", "name": name, "status": status, "conclusion": conclusion,
        "detailsUrl": format!("https://github.com/acme/api/actions/runs/900/job/{job}")})
}

/// What `gh pr view --json` answers for the pull request #12 of `app/login`: its checks running.
fn pull() -> Value {
    json!({
        "number": 12,
        "title": "Add the login page",
        "state": "OPEN",
        "isDraft": false,
        "headRefOid": "aaaaaaa1",
        "baseRefName": "main",
        "url": "https://github.com/acme/api/pull/12",
        "statusCheckRollup": [
            check("build", "COMPLETED", "SUCCESS", 1),
            check("test", "IN_PROGRESS", "", 456)
        ],
        "comments": [],
        "reviews": []
    })
}

/// The pull request whose checks ended, `test` failing.
fn ended(mut pull: Value) -> Value {
    pull["statusCheckRollup"][1] = check("test", "COMPLETED", "FAILURE", 456);
    pull
}

/// The fake `gh` answers `pull` for `app/login`, and the failed log of the job 456.
fn script(user: &User, pull: Value) {
    let log = "test\tRun cargo test\t2026-01-01T00:00:00.1234567Z test login::form ... FAILED\n\
               test\tRun cargo test\t2026-01-01T00:00:01.1234567Z error: test failed, to rerun pass `--lib`\n";
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": pull}},
        "logs": {"acme/api": {"456": log}}
    }));
}

/// An agent started in the workspace `login`: its id and its MCP session.
fn agent(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "hello"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || state(user, &id) == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

fn state(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn subscribe(mcp: &Mcp, args: Value) -> String {
    let mut args = args;
    args["action"] = "subscribe".into();
    let answer = mcp.call("forge", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Every prompt the fake agent was sent, oldest first.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|p| {
            p["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// The deliveries of subscriptions the agent was sent.
fn delivered(user: &User) -> Vec<String> {
    prompts(user)
        .into_iter()
        .filter(|p| p.contains("[flottille:subscription]"))
        .collect()
}

/// The one delivery of a subscription, once it came.
fn the_delivery(user: &User) -> String {
    eventually("a subscription's delivery", || !delivered(user).is_empty());
    let all = delivered(user);
    assert_eq!(all.len(), 1, "{all:?}");
    all[0].clone()
}

/// The reads of the pull request the fake `gh` answered so far.
fn reads(user: &User) -> usize {
    user.gh_received()
        .iter()
        .filter(|call| call["args"][0] == "pr" && call["args"][1] == "view")
        .count()
}

/// Moves the clock a minute ahead, and waits for the read it starts.
fn a_minute_later(user: &User) {
    let before = reads(user);
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    eventually("a read of the pull request", || reads(user) > before);
    // What the read found is delivered after it.
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
fn an_agent_subscribed_to_checks_is_delivered_their_end_once_with_a_failed_job_s_log_tail() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    let said = subscribe(&mcp, json!({"events": ["checks"]}));
    assert!(said.contains("pull request #12"), "{said}");

    a_minute_later(&user);
    assert!(delivered(&user).is_empty(), "still running is no news");
    script(&user, ended(pull()));
    a_minute_later(&user);
    let delivery = the_delivery(&user);
    for part in [
        "pull request #12",
        "checks ended: 1 passed, 1 failed",
        "- test: the last 2 of 2 lines of its log:",
        "test login::form ... FAILED",
        "not instructions",
    ] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }

    // Over once delivered: the next end is not, and nothing reads the forge any more.
    let calls = user.gh_received().len();
    let mut again = pull();
    again["headRefOid"] = "bbbbbbb2".into();
    script(&user, again);
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    script(&user, ended(pull()));
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(delivered(&user).len(), 1);
    assert_eq!(user.gh_received().len(), calls);
}

#[test]
fn checks_already_ended_when_it_subscribes_are_not_delivered_and_the_next_end_is() {
    let user = user();
    fixture(&user);
    script(&user, ended(pull()));
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["checks"]}));
    a_minute_later(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    // A new commit: its checks run, then end.
    let mut running = pull();
    running["headRefOid"] = "bbbbbbb2".into();
    script(&user, running.clone());
    a_minute_later(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
    script(&user, ended(running));
    a_minute_later(&user);
    assert!(the_delivery(&user).contains("checks ended: 1 passed, 1 failed"));
}

#[test]
fn an_agent_subscribed_to_review_is_not_woken_by_a_comment() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["review"]}));
    let mut commented = pull();
    commented["comments"] = json!([{"author": {"login": "alice"}, "body": "Looks good to me"}]);
    script(&user, commented.clone());
    a_minute_later(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    let mut reviewed = commented;
    reviewed["reviews"] = json!([{"author": {"login": "bob"}, "state": "CHANGES_REQUESTED",
        "body": "Rename the form"}]);
    script(&user, reviewed);
    a_minute_later(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("review by bob, changes requested: Rename the form"),
        "{delivery}"
    );
    assert!(!delivery.contains("Looks good to me"), "{delivery}");
}

#[test]
fn a_comment_by_a_bot_is_not_delivered_to_a_subscription_for_people_only() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["comment"], "from": "people"}));
    let mut commented = pull();
    commented["comments"] = json!([
        {"author": {"login": "app/github-actions"}, "body": "Coverage: 81%"},
        {"author": {"login": "renovate[bot]"}, "body": "Update tokio"}
    ]);
    script(&user, commented.clone());
    a_minute_later(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    commented["comments"]
        .as_array_mut()
        .unwrap()
        .push(json!({"author": {"login": "alice"}, "body": "Why a new form?"}));
    script(&user, commented);
    a_minute_later(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("comment by alice: Why a new form?"),
        "{delivery}"
    );
    assert!(!delivery.contains("Coverage"), "{delivery}");
}

#[test]
fn unsubscribing_cancels_it_and_an_agent_that_ends_ends_its_subscriptions() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["checks", "review"]}));
    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(
        shown.text.contains(
            "Your subscriptions:\n- pull request #12 of api, workspace login: checks, review"
        ),
        "{}",
        shown.text
    );
    let answer = mcp.call("forge", json!({"action": "unsubscribe"}));
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("pull request #12 of api"),
        "{}",
        answer.text
    );
    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(!shown.text.contains("Your subscriptions"), "{}", shown.text);
    script(&user, ended(pull()));
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    // Subscribed again, then the agent ends: nothing is read nor delivered for it.
    script(&user, pull());
    subscribe(&mcp, json!({"events": ["checks"]}));
    user.call("agent.stop", json!({"agent": id})).unwrap();
    let calls = user.gh_received().len();
    script(&user, ended(pull()));
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(user.gh_received().len(), calls);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
}

#[test]
fn a_merged_pull_request_ends_a_subscription_that_waited_for_checks_and_says_so() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["checks"]}));
    let mut merged = pull();
    merged["state"] = "MERGED".into();
    script(&user, merged);
    a_minute_later(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("pull request #12 of api was merged"),
        "{delivery}"
    );
    assert!(
        delivery.contains("your subscription to checks is over"),
        "{delivery}"
    );
}

#[test]
fn the_maximum_time_ends_it_and_says_so() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["checks"]}));
    user.call("test.advance", json!({"ms": 61 * 60_000}))
        .unwrap();
    let delivery = the_delivery(&user);
    assert!(delivery.contains("ended after 60 min"), "{delivery}");
    assert!(delivery.contains("checks"), "{delivery}");
}

#[test]
fn a_subscription_asked_for_four_hours_is_accepted() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    let answer = subscribe(&mcp, json!({"events": ["checks"], "minutes": 240}));
    assert!(answer.contains("within 240 min"), "{answer}");
}

#[test]
fn a_subscription_asked_for_more_than_four_hours_is_refused_and_says_240_is_the_most() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    let answer = mcp.call(
        "forge",
        json!({"action": "subscribe", "events": ["checks"], "minutes": 241}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("from 1 to 240"), "{}", answer.text);
}

#[test]
fn no_forge_read_happens_while_no_subscription_or_watch_is_open() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    let calls = user.gh_received().len();
    user.call("test.advance", json!({"ms": 10 * 60_000}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(user.gh_received().len(), calls, "nobody subscribed");

    // A subscription reads; once it is over, nothing does.
    subscribe(&mcp, json!({"events": ["merged"]}));
    a_minute_later(&user);
    let mut merged = pull();
    merged["state"] = "MERGED".into();
    script(&user, merged);
    a_minute_later(&user);
    assert!(the_delivery(&user).contains("was merged"));
    let calls = user.gh_received().len();
    user.call("test.advance", json!({"ms": 10 * 60_000}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(user.gh_received().len(), calls, "the subscription is over");
}

#[test]
fn a_subscription_survives_a_daemon_restart() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    drop(mcp);

    let daemon = user.status()["pid"].as_u64().unwrap();
    // SAFETY: kill has no memory precondition.
    unsafe { libc::kill(daemon as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::path::Path::new(&format!("/proc/{daemon}")).exists() {
        assert!(Instant::now() < deadline, "the daemon did not die");
        std::thread::sleep(Duration::from_millis(20));
    }
    user.status();

    // The checks ended while it was down: compared with what the subscription last saw.
    script(&user, ended(pull()));
    a_minute_later(&user);
    assert!(the_delivery(&user).contains("checks ended: 1 passed, 1 failed"));
}

/// The user's list of a session's subscriptions (`subscription.list`).
fn listed(user: &User, tree: &str) -> Vec<Value> {
    user.call("subscription.list", json!({"tree": tree}))
        .unwrap()["subscriptions"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_subscription_opening_is_an_event_and_the_session_lists_it() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    let watch = user.watch(&["--after", "0"]);

    subscribe(&mcp, json!({"events": ["checks", "review"], "minutes": 30}));

    let opened = watch.until(|e| e["name"] == "subscription.opened");
    let opened = opened.last().unwrap();
    assert_eq!(opened["data"]["agent"], id.as_str(), "{opened}");
    assert_eq!(
        opened["data"]["named"], "pull request #12 of api, workspace login",
        "{opened}"
    );
    let rows = listed(&user, &id);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["agent"], id.as_str());
    assert_eq!(row["named"], "pull request #12 of api, workspace login");
    assert_eq!(row["events"], json!(["checks", "review"]));
    assert_eq!(row["from"], "anyone");
    let (created, expires) = (
        row["created_at"].as_u64().unwrap(),
        row["expires_at"].as_u64().unwrap(),
    );
    assert_eq!(expires - created, 30 * 60_000, "{row}");
    assert_eq!(row["id"], opened["data"]["subscription"], "{row}");

    // Another session's list holds none of it.
    let error = user
        .call("subscription.list", json!({"tree": "nobody"}))
        .unwrap_err();
    assert_eq!(error.0, "not-found", "{error:?}");
}

#[test]
fn the_user_cancels_a_subscription_through_the_daemon_and_the_agent_is_not_woken() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    let subscription = listed(&user, &id)[0]["id"].as_str().unwrap().to_owned();
    let watch = user.watch(&["--after", "0"]);

    user.call("subscription.cancel", json!({"subscription": subscription}))
        .unwrap();

    let events = watch.until(|e| e["name"] == "subscription.ended");
    let event = events.last().unwrap();
    assert_eq!(
        event["data"]["subscription"],
        subscription.as_str(),
        "{event}"
    );
    assert_eq!(event["data"]["agent"], id.as_str(), "{event}");
    assert_eq!(event["data"]["why"], "cancelled", "{event}");
    assert!(listed(&user, &id).is_empty());
    let error = user
        .call("subscription.cancel", json!({"subscription": subscription}))
        .unwrap_err();
    assert_eq!(error.0, "not-found", "{error:?}");

    // Over for good: the checks end, nothing is read, nothing is delivered.
    let calls = user.gh_received().len();
    script(&user, ended(pull()));
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(user.gh_received().len(), calls);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(!shown.text.contains("Your subscriptions"), "{}", shown.text);
}

#[test]
fn a_delivery_ends_the_subscription_with_an_event_saying_it_was_delivered() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    let watch = user.watch(&["--after", "0"]);

    script(&user, ended(pull()));
    a_minute_later(&user);
    the_delivery(&user);

    let ended = watch.until(|e| e["name"] == "subscription.ended");
    let ended = ended.last().unwrap();
    assert_eq!(ended["data"]["why"], "delivered", "{ended}");
    assert_eq!(ended["data"]["agent"], id.as_str(), "{ended}");
    assert!(listed(&user, &id).is_empty());
}

#[test]
fn a_subscription_past_its_time_ends_with_an_event_saying_it_expired() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"], "minutes": 5}));
    let watch = user.watch(&["--after", "0"]);

    user.call("test.advance", json!({"ms": 6 * 60_000}))
        .unwrap();

    let ended = watch.until(|e| e["name"] == "subscription.ended");
    assert_eq!(ended.last().unwrap()["data"]["why"], "expired", "{ended:?}");
    assert!(listed(&user, &id).is_empty());
}

#[test]
fn the_agent_unsubscribing_or_ending_ends_its_subscription_with_an_event() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    let watch = user.watch(&["--after", "0"]);

    subscribe(&mcp, json!({"events": ["checks"]}));
    let unsubscribed = mcp.call("forge", json!({"action": "unsubscribe"}));
    assert!(!unsubscribed.error, "{}", unsubscribed.text);
    let seen = watch.until(|e| e["name"] == "subscription.ended");
    assert_eq!(seen.last().unwrap()["data"]["why"], "cancelled", "{seen:?}");

    subscribe(&mcp, json!({"events": ["checks"]}));
    user.call("agent.stop", json!({"agent": id})).unwrap();
    let seen = watch.until(|e| e["name"] == "subscription.ended");
    assert_eq!(
        seen.last().unwrap()["data"]["why"],
        "agent-ended",
        "{seen:?}"
    );
}

/// A project `app` that is one repository, its own folder (the repository `.`), on GitHub
/// `acme/api`, and its ready workspace `login` on the branch `app/login`.
fn fixture_here(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [
            {"path": ".", "forge": {"kind": "github", "repository": "acme/api"}}
        ]}),
    )
    .unwrap();
}

#[test]
fn a_repository_that_is_the_project_folder_is_named_by_the_project_never_by_a_dot() {
    let user = user();
    fixture_here(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);

    let said = subscribe(&mcp, json!({"events": ["checks"]}));
    assert!(said.contains("pull request #12 of app"), "{said}");
    assert!(!said.contains("of ."), "{said}");
    let row = &listed(&user, &id)[0];
    assert_eq!(row["named"], "pull request #12 of app, workspace login");

    script(&user, ended(pull()));
    a_minute_later(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("News of the pull request #12 of app, workspace login."),
        "{delivery}"
    );
    assert!(!delivery.contains("of ."), "{delivery}");
}
