//! The scenarios of the subscriptions (`docs/design.md` §5.6, `docs/tools.md`, "The forge"): an
//! agent subscribes once to the next event of its workspace's pull request, and the first one that
//! matches wakes it as a delivery. The events are those of the daemon's rounds of the items
//! (`docs/items.md`), which each test starts itself (`items.refresh`); a subscription reads
//! nothing of its own. Read through the fake `gh` (`crates/flottille-fake-gh`), the clock moved
//! by the test; no test ever calls GitHub.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// What Flottille ends every comment and body it posts for an agent with (`docs/forges.md`).
const MARKER: &str = "<!-- flottille -->";

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
    fixture_with(user, &[]);
}

/// The same, the project holding the repositories `others` beside `api`.
fn fixture_with(user: &User, others: &[&str]) {
    let folder = user.path("work/app");
    for other in others {
        let dir = folder.join(other);
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), format!("{other}\n")).unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
        let remote = user.path(&format!("remotes/{other}.git"));
        user.git(
            &user.path(""),
            &["init", "--quiet", "--bare", remote.to_str().unwrap()],
        );
        user.git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        user.git(&dir, &["push", "--quiet", "origin", "main"]);
    }
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
        json!({"name": "app", "repositories": std::iter::once(
            json!({"path": "api", "forge": {"kind": "github", "repository": "acme/api"}})
        ).chain(others.iter().map(|other| json!({"path": other}))).collect::<Vec<_>>()}),
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

/// The fake `gh` answers `pull` for `app/login`, and the failed log of the job 456. Each script
/// is a change of the pull request, which GitHub says by a newer `updatedAt`.
fn script(user: &User, pull: Value) {
    let pull = changed(pull);
    let log = "test\tRun cargo test\t2026-01-01T00:00:00.1234567Z test login::form ... FAILED\n\
               test\tRun cargo test\t2026-01-01T00:00:01.1234567Z error: test failed, to rerun pass `--lib`\n";
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": pull}},
        "logs": {"acme/api": {"456": log}}
    }));
}

/// `pull` updated now: later than any change before it.
fn changed(mut pull: Value) -> Value {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    pull["updatedAt"] = format!("2999-01-01T{:02}:{:02}:00Z", n / 60 % 24, n % 60).into();
    pull
}

/// An agent started in the workspace `login`, once the daemon read the project's pull requests:
/// its id and its MCP session.
fn agent(user: &User) -> (String, Mcp) {
    a_round(user);
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

/// A round of the project's items, as the daemon makes every few minutes.
fn a_round(user: &User) {
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    // What the round found is delivered after it.
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

    a_round(&user);
    assert!(delivered(&user).is_empty(), "still running is no news");
    script(&user, ended(pull()));
    a_round(&user);
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

    // Over once delivered: the next end is not.
    let mut again = pull();
    again["headRefOid"] = "bbbbbbb2".into();
    script(&user, again.clone());
    a_round(&user);
    script(&user, ended(again));
    a_round(&user);
    assert_eq!(delivered(&user).len(), 1);
}

#[test]
fn checks_already_ended_when_it_subscribes_are_not_delivered_and_the_next_end_is() {
    let user = user();
    fixture(&user);
    script(&user, ended(pull()));
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["checks"]}));
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    // A new commit: its checks run, then end.
    let mut running = pull();
    running["headRefOid"] = "bbbbbbb2".into();
    script(&user, running.clone());
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
    script(&user, ended(running));
    a_round(&user);
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
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    let mut reviewed = commented;
    reviewed["reviews"] = json!([{"author": {"login": "bob"}, "state": "CHANGES_REQUESTED",
        "body": "Rename the form"}]);
    script(&user, reviewed);
    a_round(&user);
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
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    commented["comments"]
        .as_array_mut()
        .unwrap()
        .push(json!({"author": {"login": "alice"}, "body": "Why a new form?"}));
    script(&user, commented);
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("comment by alice: Why a new form?"),
        "{delivery}"
    );
    assert!(!delivery.contains("Coverage"), "{delivery}");
}

#[test]
fn a_comment_of_the_user_without_the_marker_wakes_a_subscription_for_people_only() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["comment"], "from": "people"}));
    let mut commented = pull();
    // `kris` is the account every agent writes through: a comment of theirs is a person's.
    commented["comments"] = json!([{"author": {"login": "kris"}, "body": "Why a new form?"}]);
    script(&user, commented);
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("comment by kris: Why a new form?"),
        "{delivery}"
    );
}

#[test]
fn from_people_still_drops_a_marked_comment_and_keeps_the_user_s() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["comment"], "from": "people"}));
    let mut commented = pull();
    // Another agent's, in the same account: the marker, not the author, says who wrote it.
    commented["comments"] = json!([
        {"author": {"login": "kris"}, "body": format!("Rebased on main\n\n{MARKER}")}
    ]);
    script(&user, commented.clone());
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    commented["comments"]
        .as_array_mut()
        .unwrap()
        .push(json!({"author": {"login": "kris"}, "body": "Thanks"}));
    script(&user, commented);
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(delivery.contains("comment by kris: Thanks"), "{delivery}");
    assert!(!delivery.contains("Rebased"), "{delivery}");
}

#[test]
fn a_marked_comment_is_shown_to_an_agent_as_posted_by_an_agent_without_its_marker() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["comment"]}));
    let mut commented = pull();
    commented["comments"] = json!([
        {"author": {"login": "kris"}, "body": format!("Rebased on main\n\n{MARKER}")}
    ]);
    script(&user, commented);
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("comment by kris (posted by an agent): Rebased on main"),
        "{delivery}"
    );
    assert!(!delivery.contains("flottille -->"), "{delivery}");
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
    a_round(&user);
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
    a_round(&user);
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
    a_round(&user);
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
fn a_subscription_asked_for_100_hours_is_accepted() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    let answer = subscribe(&mcp, json!({"events": ["checks"], "minutes": 6000}));
    assert!(answer.contains("within 6000 min"), "{answer}");
}

#[test]
fn a_subscription_asked_for_more_than_100_hours_is_refused_and_says_6000_is_the_most() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);

    let answer = mcp.call(
        "forge",
        json!({"action": "subscribe", "events": ["checks"], "minutes": 6001}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("from 1 to 6000"), "{}", answer.text);
}

#[test]
fn a_forge_subscription_is_delivered_from_the_fetcher_s_checks_event_with_no_watch_of_its_own() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));

    // Minutes pass with the subscription open: it reads nothing of its own.
    let calls = user.gh_received().len();
    user.call("test.advance", json!({"ms": 10 * 60_000}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(user.gh_received().len(), calls, "a watch of its own");

    let watch = user.watch(&["--after", "0"]);
    script(&user, ended(pull()));
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("checks ended: 1 passed, 1 failed"),
        "{delivery}"
    );
    let events = watch.until(|e| e["name"] == "subscription.ended");
    let checks = events.iter().position(|e| e["name"] == "item.checks");
    let ended = events
        .iter()
        .position(|e| e["name"] == "subscription.ended");
    assert!(checks.is_some() && checks < ended, "{events:?}");
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

    // The checks ended while it was down: the next round compares them with what it stored.
    script(&user, ended(pull()));
    a_round(&user);
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

    // Over for good: the checks end, nothing is read for it, nothing is delivered.
    let calls = user.gh_received().len();
    script(&user, ended(pull()));
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(user.gh_received().len(), calls);
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(!shown.text.contains("Your subscriptions"), "{}", shown.text);
}

/// What the tree says an agent does.
fn doing(user: &User, agent: &str) -> String {
    let tree = user.call("agent.tree", json!({"agent": agent})).unwrap();
    tree["agents"]
        .as_array()
        .and_then(|agents| agents.iter().find(|a| a["agent"]["id"] == agent))
        .map(|a| a["doing"].as_str().unwrap_or_default().to_owned())
        .unwrap_or_else(|| panic!("{tree}"))
}

#[test]
fn what_an_agent_waits_for_names_the_repository_only_when_the_project_has_several() {
    let alone = user();
    fixture(&alone);
    script(&alone, pull());
    let (id, mcp) = agent(&alone);
    subscribe(&mcp, json!({"events": ["checks"]}));
    assert_eq!(doing(&alone, &id), "waiting for #12: checks");

    let several = user();
    fixture_with(&several, &["web"]);
    script(&several, pull());
    let (id, mcp) = agent(&several);
    subscribe(&mcp, json!({"events": ["checks"], "repository": "api"}));
    assert_eq!(doing(&several, &id), "waiting for #12 of api: checks");
}

#[test]
fn a_subscription_cancelled_while_its_logs_are_read_wakes_nobody_and_ends_once() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    let subscription = listed(&user, &id)[0]["id"].as_str().unwrap().to_owned();
    let watch = user.watch(&["--after", "0"]);

    // The checks ended and the daemon is reading the failed job's log, slowly.
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": ended(pull())}},
        "logs": {"acme/api": {"456": "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z failed\n"}},
        "log_delay_ms": 1500
    }));
    a_round(&user);
    eventually("the log being read", || {
        user.gh_received()
            .iter()
            .any(|call| call["args"].to_string().contains("--log-failed"))
    });
    user.call("subscription.cancel", json!({"subscription": subscription}))
        .unwrap();

    let events = watch.within(Duration::from_secs(3));
    let ended: Vec<&Value> = events
        .iter()
        .filter(|e| e["name"] == "subscription.ended")
        .collect();
    assert_eq!(ended.len(), 1, "{events:?}");
    assert_eq!(ended[0]["data"]["why"], "cancelled", "{events:?}");
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
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
    a_round(&user);
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
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("News of the pull request #12 of app, workspace login."),
        "{delivery}"
    );
    assert!(!delivery.contains("of ."), "{delivery}");
}

#[test]
fn forge_unsubscribe_and_show_leave_the_agent_s_wake_alone() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    let wake = mcp.call("wake", json!({"minutes": 20, "why": "look again"}));
    assert!(!wake.error, "{}", wake.text);

    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(!shown.text.contains("look again"), "{}", shown.text);
    let answer = mcp.call("forge", json!({"action": "unsubscribe"}));
    assert!(!answer.error, "{}", answer.text);
    let left = listed(&user, &id);
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["source"], "timer");
}

/// The project's tracker, beside `api`'s forge: the issues of `api`, or Jira's ABC.
fn track(user: &User, tracker: Value) {
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}],
            "tracker": tracker}),
    )
    .unwrap();
}

/// The issue #5 of `acme/api` with `comments`, changed now.
fn issue(comments: Value) -> Value {
    changed(json!({
        "number": 5, "title": "Users cannot sign in", "state": "OPEN", "body": "",
        "url": "https://github.com/acme/api/issues/5", "author": {"login": "other"},
        "assignees": [], "labels": [], "comments": comments
    }))
}

/// The fake `gh` answers the pull request #12 and the issue #5.
fn script_issue(user: &User, issue: Value) {
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": changed(pull())}},
        "issues": {"acme/api": {"5": issue}}
    }));
}

/// The work item ABC-1 in `status`, as `acli` is assumed to print it.
fn work_item(status: &str) -> Value {
    json!({
        "key": "ABC-1",
        "self": "https://example.atlassian.net/rest/api/3/issue/10001",
        "fields": {
            "summary": "Log in", "description": null,
            "status": {"name": status, "statusCategory": {"key": "indeterminate"}},
            "assignee": {"displayName": "Alice"}, "labels": [],
            "issuetype": {"name": "Story"}, "priority": {"name": "High"},
            "updated": "2026-10-01T10:00:00.000+0000",
            "comment": {"comments": [], "total": 0}
        }
    })
}

fn subscribe_ticket(mcp: &Mcp, args: Value) -> String {
    let mut args = args;
    args["action"] = "subscribe".into();
    let answer = mcp.call("tickets", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

#[test]
fn a_ticket_subscription_wakes_the_agent_on_a_new_comment_of_the_ticket() {
    let user = user();
    fixture(&user);
    track(&user, json!({"kind": "github", "repository": "api"}));
    script_issue(&user, issue(json!([])));
    let (id, mcp) = agent(&user);

    let said = subscribe_ticket(&mcp, json!({"ticket": "#5", "events": ["commented"]}));
    assert!(said.contains("ticket #5"), "{said}");
    assert_eq!(listed(&user, &id)[0]["named"], "ticket #5 of api");

    script_issue(
        &user,
        issue(json!([{"author": {"login": "alice"}, "body": "I can reproduce"}])),
    );
    a_round(&user);
    let delivery = the_delivery(&user);
    for part in [
        "News of the ticket #5 of api",
        "comment by alice: I can reproduce",
    ] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }
}

#[test]
fn a_ticket_subscription_wakes_the_agent_on_a_status_change() {
    let user = user().with_fake_acli();
    fixture(&user);
    track(
        &user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    script(&user, pull());
    user.acli_script(json!({"items": {"ABC-1": work_item("To Do")}}));
    let (_, mcp) = agent(&user);

    let said = subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["status"]}));
    assert!(!said.contains("Flottille's rounds have not"), "{said}");
    user.acli_script(json!({"items": {"ABC-1": work_item("In Progress")}}));
    a_round(&user);
    let delivery = the_delivery(&user);
    for part in ["News of the ticket ABC-1", "To Do", "In Progress"] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }
}

#[test]
fn a_ticket_subscription_s_expiry_and_cancel_are_those_of_a_forge_subscription() {
    let user = user();
    fixture(&user);
    track(&user, json!({"kind": "github", "repository": "api"}));
    script_issue(&user, issue(json!([])));
    let (id, mcp) = agent(&user);
    let watch = user.watch(&["--after", "0"]);

    subscribe_ticket(
        &mcp,
        json!({"ticket": "#5", "events": ["commented", "closed"], "minutes": 5}),
    );
    user.call("test.advance", json!({"ms": 6 * 60_000}))
        .unwrap();
    let ended = watch.until(|e| e["name"] == "subscription.ended");
    assert_eq!(ended.last().unwrap()["data"]["why"], "expired", "{ended:?}");
    let delivery = the_delivery(&user);
    assert!(delivery.contains("ended after 5 min"), "{delivery}");
    assert!(delivery.contains("ticket #5 of api"), "{delivery}");

    subscribe_ticket(&mcp, json!({"ticket": "#5", "events": ["commented"]}));
    let answer = mcp.call("tickets", json!({"action": "unsubscribe", "ticket": "#5"}));
    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("ticket #5 of api"), "{}", answer.text);
    let ended =
        watch.until(|e| e["name"] == "subscription.ended" && e["data"]["why"] == "cancelled");
    assert!(!ended.is_empty());
    assert!(listed(&user, &id).is_empty());
    script_issue(
        &user,
        issue(json!([{"author": {"login": "alice"}, "body": "I can reproduce"}])),
    );
    a_round(&user);
    assert_eq!(delivered(&user).len(), 1, "{:?}", delivered(&user));
}

/// The fake `gh` answers no pull request and no issue: what a round reads before the agent opens
/// one.
fn nothing_opened(user: &User) {
    user.gh_script(json!({"pulls": {"acme/api": {}}, "issues": {"acme/api": {}}}));
}

#[test]
fn checks_of_a_pull_request_no_round_stored_yet_are_delivered_to_its_subscription() {
    let user = user();
    fixture(&user);
    nothing_opened(&user);
    let (_, mcp) = agent(&user);
    // Opened since the last round, as `forge create` does.
    script(&user, pull());

    subscribe(&mcp, json!({"events": ["checks"]}));
    script(&user, ended(pull()));
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("checks ended: 1 passed, 1 failed"),
        "{delivery}"
    );
}

#[test]
fn a_comment_on_a_pull_request_no_round_stored_yet_is_delivered_to_its_subscription() {
    let user = user();
    fixture(&user);
    nothing_opened(&user);
    let (_, mcp) = agent(&user);
    script(&user, pull());

    subscribe(&mcp, json!({"events": ["comment"]}));
    let mut commented = pull();
    commented["comments"] = json!([{"author": {"login": "alice"}, "body": "Why a new form?"}]);
    script(&user, commented);
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("comment by alice: Why a new form?"),
        "{delivery}"
    );
}

#[test]
fn a_comment_on_a_ticket_no_round_stored_yet_is_delivered_to_its_subscription() {
    let user = user();
    fixture(&user);
    track(&user, json!({"kind": "github", "repository": "api"}));
    nothing_opened(&user);
    let (_, mcp) = agent(&user);
    let watch = user.watch(&["--after", "0"]);
    // Created since the last round, as `tickets create` does, assigned and labelled: what its
    // live read does not say.
    let assigned = |comments: Value| {
        let mut issue = issue(comments);
        issue["assignees"] = json!([{"login": "alice"}]);
        issue["labels"] = json!([{"name": "bug"}]);
        issue
    };
    script_issue(&user, assigned(json!([])));

    let said = subscribe_ticket(
        &mcp,
        json!({"ticket": "#5", "events": ["commented", "assigned"]}),
    );
    assert!(!said.contains("Flottille's rounds have not"), "{said}");
    script_issue(
        &user,
        assigned(json!([{"author": {"login": "alice"}, "body": "I can reproduce"}])),
    );
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("comment by alice: I can reproduce"),
        "{delivery}"
    );
    assert!(!delivery.contains("assignees"), "{delivery}");
    let changed: Vec<Value> = watch
        .within(Duration::from_secs(1))
        .into_iter()
        .filter(|e| e["name"] == "item.changed")
        .collect();
    assert!(changed.is_empty(), "{changed:?}");
}

#[test]
fn a_ticket_subscription_before_the_tracker_was_ever_read_says_what_is_not_seen() {
    let user = user().with_fake_acli();
    fixture(&user);
    track(
        &user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    script(&user, pull());
    // Every round of the tracker fails: none ever read it through.
    user.acli_script(json!({"items": {"ABC-1": work_item("To Do")},
        "fail": {"search": "Error: the search could not be run"}}));
    let (_, mcp) = agent(&user);

    let said = subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["status"]}));
    assert!(
        said.contains(
            "Flottille's rounds have not read the tracker once yet: what happens before their \
             first complete round is not seen."
        ),
        "{said}"
    );
}

#[test]
fn a_subscription_to_a_work_item_no_round_stored_says_what_is_not_seen() {
    let user = user().with_fake_acli();
    fixture(&user);
    track(
        &user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    script(&user, pull());
    // The rounds read the tracker, and not ABC-1: created since, as `tickets create` does.
    user.acli_script(json!({"items": {"ABC-1": work_item("To Do")}, "search": []}));
    let (_, mcp) = agent(&user);

    let said = subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["status"]}));
    assert!(
        said.contains(
            "Flottille's rounds have not stored this ticket yet: what happens to it before their \
             next round is not seen."
        ),
        "{said}"
    );
}

#[test]
fn a_merged_pull_request_no_round_listed_read_live_is_stored_and_not_said_opened() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);
    let watch = user.watch(&["--after", "0"]);
    // Too old to be in a round's list.
    let mut old = pull();
    old["number"] = 3.into();
    old["state"] = "MERGED".into();
    old["url"] = "https://github.com/acme/api/pull/3".into();
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": changed(pull()), "app/old": changed(old)}},
        "unlisted": {"acme/api": [3]}
    }));

    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": "api", "number": 3}),
    );
    assert!(!answer.error, "{}", answer.text);
    let item = user
        .call(
            "items.show",
            json!({"project": "app", "kind": "pull", "id": "3", "repository": "api"}),
        )
        .unwrap();
    assert_eq!(item["state"], "merged", "{item}");
    let opened: Vec<Value> = watch
        .within(Duration::from_secs(1))
        .into_iter()
        .filter(|e| e["name"] == "item.opened")
        .collect();
    assert!(opened.is_empty(), "{opened:?}");
}

#[test]
fn a_subscription_during_a_slow_round_answers_at_once_and_is_delivered_the_round_s_event() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);
    let listings = |user: &User| {
        user.gh_received()
            .iter()
            .filter(|call| {
                let args = call["args"].to_string();
                args.contains(r#""pr","list""#) && !args.contains("--head")
            })
            .count()
    };
    let before = listings(&user);
    // A round reads the checks ended, its list slowly, as the first round of a big project does.
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": changed(ended(pull()))}},
        "logs": {"acme/api": {"456": "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z failed\n"}},
        "listing_delay_ms": 10_000
    }));
    std::thread::scope(|scope| {
        let round = scope.spawn(|| a_round(&user));
        eventually("the round's list", || listings(&user) > before);

        let asked = Instant::now();
        subscribe(&mcp, json!({"events": ["checks"]}));
        assert!(
            asked.elapsed() < Duration::from_secs(6),
            "answered after {:?}",
            asked.elapsed()
        );
        round.join().unwrap();
    });
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("checks ended: 1 passed, 1 failed"),
        "{delivery}"
    );
}

#[test]
fn a_comment_by_a_bot_on_a_ticket_is_not_delivered_to_a_subscription_for_people_only() {
    let user = user();
    fixture(&user);
    track(&user, json!({"kind": "github", "repository": "api"}));
    script_issue(&user, issue(json!([])));
    let (_, mcp) = agent(&user);

    subscribe_ticket(
        &mcp,
        json!({"ticket": "#5", "events": ["commented"], "from": "people"}),
    );
    let mut comments = json!([
        {"author": {"login": "github-actions[bot]"}, "body": "Stale for 30 days"},
        {"author": {"login": "app/dependabot"}, "body": "Bump tokio"}
    ]);
    script_issue(&user, issue(comments.clone()));
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    comments
        .as_array_mut()
        .unwrap()
        .push(json!({"author": {"login": "alice"}, "body": "I can reproduce"}));
    script_issue(&user, issue(comments));
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("comment by alice: I can reproduce"),
        "{delivery}"
    );
    assert!(!delivery.contains("Bump tokio"), "{delivery}");
}

#[test]
fn ended_checks_whose_logs_are_read_are_delivered_before_a_later_comment() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks", "comment"]}));

    let slowly = |pull: Value| {
        json!({
            "pulls": {"acme/api": {"app/login": changed(pull)}},
            "logs": {"acme/api": {"456": "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z failed\n"}},
            "log_delay_ms": 1500
        })
    };
    user.gh_script(slowly(ended(pull())));
    a_round(&user);
    eventually("the log being read", || {
        user.gh_received()
            .iter()
            .any(|call| call["args"].to_string().contains("--log-failed"))
    });
    // A comment comes while the log is read: the checks ended first, and are delivered.
    let mut commented = ended(pull());
    commented["comments"] = json!([{"author": {"login": "alice"}, "body": "Why a new form?"}]);
    user.gh_script(slowly(commented));
    a_round(&user);

    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("checks ended: 1 passed, 1 failed"),
        "{delivery}"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(delivered(&user).len(), 1, "{:?}", delivered(&user));
}

#[test]
fn a_refresh_during_a_live_read_waits_for_it_and_does_not_emit_its_change_twice() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);
    let watch = user.watch(&["--after", "0"]);
    let views = |user: &User| {
        user.gh_received()
            .iter()
            .filter(|call| call["args"].to_string().contains(r#""pr","view","12""#))
            .count()
    };
    let before = views(&user);

    // `forge show` reads the checks still running, slowly; a round reads them ended meanwhile.
    let mut slow = json!({"pulls": {"acme/api": {"app/login": changed(pull())}}});
    slow["view_delay_ms"] = 1500.into();
    user.gh_script(slow);
    let shown = std::thread::spawn(move || {
        let answer = mcp.call(
            "forge",
            json!({"action": "show", "repository": "api", "number": 12}),
        );
        (mcp, answer)
    });
    eventually("the live read", || views(&user) > before);
    script(&user, ended(pull()));
    a_round(&user);
    let (_mcp, answer) = shown.join().unwrap();
    assert!(!answer.error, "{}", answer.text);
    a_round(&user);

    let checks: Vec<Value> = watch
        .within(Duration::from_secs(1))
        .into_iter()
        .filter(|e| e["name"] == "item.checks")
        .collect();
    assert_eq!(checks.len(), 1, "{checks:?}");
}

#[test]
fn a_pull_request_read_live_again_is_no_longer_said_removed() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (_, mcp) = agent(&user);
    // Its page not found once: the round says it removed.
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": changed(pull())}},
        "view_fail": {"acme/api": {"12": "GraphQL: Could not resolve to a PullRequest with the number of 12. (repository.pullRequest)"}}
    }));
    a_round(&user);
    let item = || {
        user.call(
            "items.show",
            json!({"project": "app", "kind": "pull", "id": "12", "repository": "api"}),
        )
        .unwrap()
    };
    assert_eq!(item()["details"]["removed"], true, "{}", item());

    script(&user, pull());
    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": "api", "number": 12}),
    );
    assert!(!answer.error, "{}", answer.text);
    let item = item();
    assert_eq!(item["state"], "open", "{item}");
    assert_eq!(item["details"].get("removed"), None, "{item}");
}
