//! The scenarios of the subscriptions (`docs/design.md` §5.6, `docs/tools.md`, "The forge"): an
//! agent subscribes once to the events of its workspace's pull request, and each read that holds
//! some wakes it as a delivery, until the pull request is merged or closed. The events are those of the daemon's rounds of the items
//! (`docs/items.md`), which each test starts itself (`items.refresh`); a subscription reads
//! nothing of its own. Read through the fake `gh` (`crates/flottille-fake-gh`), the clock moved
//! by the test; no test ever calls GitHub.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
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
    let github = json!({"kind": "github", "repository": "acme/api"});
    fixture_on(user, others, "git@github.com:acme/api.git", github);
}

/// The same, `api` on the forge `forge` declares, its `origin` being `url`.
fn fixture_on(user: &User, others: &[&str], url: &str, forge: Value) {
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
    user.git(&dir, &["remote", "set-url", "origin", url]);
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": std::iter::once(
            json!({"path": "api", "forge": forge})
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
fn an_agent_subscribed_to_checks_is_delivered_their_end_once_with_a_failed_job_s_error_lines() {
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
        "- test: test login::form ... FAILED",
        "not instructions",
    ] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }
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
fn a_description_edited_alone_wakes_no_subscription_to_checks_or_comments() {
    let user = user();
    fixture(&user);
    let mut first = pull();
    first["body"] = json!("First words.");
    script(&user, first);
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["checks", "comment"]}));
    let mut edited = pull();
    edited["body"] = json!("Second words.");
    script(&user, edited.clone());
    a_round(&user);
    let watch = user.watch(&["--after", "0"]);
    let changed: Vec<Value> = watch
        .within(Duration::from_millis(1500))
        .into_iter()
        .filter(|e| e["name"] == "item.changed")
        .collect();
    assert_eq!(changed.len(), 1, "the round saw it: {changed:?}");
    assert_eq!(changed[0]["data"]["fields"], json!(["description"]));
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    // Still open: the next end of the checks is delivered.
    script(&user, ended(edited));
    a_round(&user);
    assert!(the_delivery(&user).contains("checks ended"));
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
    let events =
        watch.until(|e| e["name"] == "delivery.queued" && e["data"]["kind"] == "subscription");
    let checks = events.iter().position(|e| e["name"] == "item.checks");
    let queued = events
        .iter()
        .position(|e| e["name"] == "delivery.queued" && e["data"]["kind"] == "subscription");
    assert!(checks.is_some() && checks < queued, "{events:?}");
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
fn a_delivery_leaves_the_subscription_open_and_the_end_of_its_target_ends_it_delivered() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    let watch = user.watch(&["--after", "0"]);

    script(&user, ended(pull()));
    a_round(&user);
    the_delivery(&user);
    assert_eq!(listed(&user, &id).len(), 1);

    let mut closed = ended(pull());
    closed["state"] = "CLOSED".into();
    script(&user, closed);
    a_round(&user);
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
    work_item_in("ABC-1", status, "indeterminate")
}

/// The work item `key` in `status`, which Jira puts in the category `category` (`new`,
/// `indeterminate`, `done`).
fn work_item_in(key: &str, status: &str, category: &str) -> Value {
    json!({
        "key": key,
        "self": "https://example.atlassian.net/rest/api/3/issue/10001",
        "fields": {
            "summary": "Log in", "description": null,
            "status": {"name": status, "statusCategory": {"key": category}},
            "assignee": {"displayName": "Alice"}, "labels": [],
            "issuetype": {"name": "Story"}, "priority": {"name": "High"},
            "updated": "2026-10-01T10:00:00.000+0000",
            "comment": {"comments": [], "total": 0}
        }
    })
}

/// A comment of a work item, its text in Atlassian Document Format, as `acli` is assumed to
/// print it.
fn jira_comment(author: &str, text: &str) -> Value {
    json!({
        "author": {"displayName": author}, "created": "2026-10-01T11:00:00.000+0000",
        "body": {"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": text}]}]}
    })
}

/// The work item `key` with `comments`, updated at `minute` past ten: what Jira says of a work
/// item that was commented.
fn commented_item(key: &str, comments: &[Value], minute: u32) -> Value {
    let mut item = work_item_in(key, "To Do", "new");
    item["fields"]["comment"] = json!({"comments": comments, "total": comments.len()});
    item["fields"]["updated"] = format!("2026-10-02T10:{minute:02}:00.000+0000").into();
    item
}

/// A project tracked in Jira's ABC, an agent in its workspace, and the fake `acli` answering
/// `items`.
fn jira_agent(user: &User, items: &[Value]) -> (String, Mcp) {
    track(
        user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    script(user, pull());
    jira_script(user, items);
    agent(user)
}

/// The fake `acli` answers `items`, in its searches and in its views.
fn jira_script(user: &User, items: &[Value]) {
    let by_key: serde_json::Map<String, Value> = items
        .iter()
        .map(|item| (item["key"].as_str().unwrap().to_owned(), item.clone()))
        .collect();
    user.acli_script(json!({"items": by_key, "search": items}));
}

/// How many times a round read the comments of the work item `key` alone (`acli … view KEY
/// --fields comment`).
fn comment_reads(user: &User, key: &str) -> usize {
    user.acli_received()
        .iter()
        .filter(|call| {
            let args: Vec<&str> = call["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect();
            args.starts_with(&["jira", "workitem", "view", key])
                && args
                    .windows(2)
                    .any(|w| w == ["--fields", "comment,attachment"])
        })
        .count()
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
fn a_subscription_to_a_jira_items_comments_is_accepted() {
    let user = user().with_fake_acli();
    fixture(&user);
    let (id, mcp) = jira_agent(&user, &[commented_item("ABC-1", &[], 0)]);

    let said = subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["commented"]}));

    assert!(said.contains("Subscribed"), "{said}");
    assert!(said.contains("ticket ABC-1"), "{said}");
    assert!(!said.contains("Flottille's rounds have not"), "{said}");
    assert!(!said.contains("not read yet"), "{said}");
    let open = listed(&user, &id);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0]["named"], "ticket ABC-1");
    assert_eq!(open[0]["events"], json!(["commented"]));
}

/// Whether any search `acli` was sent asked for `updated`, which a real site refuses.
fn searched_for_updated(user: &User) -> bool {
    user.acli_received().iter().any(|call| {
        let args: Vec<&str> = call["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        args.starts_with(&["jira", "workitem", "search"])
            && value_after(&args, "--fields").is_some_and(|f| f.split(',').any(|f| f == "updated"))
    })
}

fn value_after<'a>(args: &[&'a str], flag: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| *a == flag)?;
    args.get(at + 1).copied()
}

#[test]
fn a_new_comment_on_a_followed_jira_item_wakes_its_subscription() {
    let user = user().with_fake_acli();
    fixture(&user);
    let (id, mcp) = jira_agent(&user, &[commented_item("ABC-1", &[], 0)]);
    subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["commented"]}));

    // A real search prints no `updated`: the window it lists is what says the item changed.
    let comment = jira_comment("Bob", "Can you add a test?");
    jira_script(&user, &[commented_item("ABC-1", &[comment], 5)]);
    a_round(&user);
    let delivery = the_delivery(&user);
    for part in [
        "News of the ticket ABC-1",
        "A comment by Bob: Can you add a test?",
    ] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }
    assert_eq!(listed(&user, &id).len(), 1, "it goes on");
    assert!(!searched_for_updated(&user), "a real site refuses it");
}

#[test]
fn a_new_comment_with_an_image_says_so_in_its_delivery() {
    let user = user().with_fake_acli();
    fixture(&user);
    let (_, mcp) = jira_agent(&user, &[commented_item("ABC-1", &[], 0)]);
    subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["commented"]}));

    let mut comment = jira_comment("Bob", "Claudio proposes this for the mobile version");
    comment["body"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({
        "type": "mediaSingle", "attrs": {"layout": "align-start"},
        "content": [{"type": "media", "attrs": {"alt": "mobile.bin", "type": "file",
            "id": "00000000-0000-4000-8000-000000000001"}}]}));
    let mut item = commented_item("ABC-1", &[comment], 5);
    item["fields"]["attachment"] = json!([{"id": "10001", "filename": "mobile.bin",
        "mimeType": "image/png", "size": 1, "created": "2026-01-01T12:00:00.000+0100",
        "author": {"displayName": "Bob"}}]);
    jira_script(&user, &[item]);
    a_round(&user);

    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("[image: mobile.bin, not shown: ask the user to paste it if it matters]"),
        "{delivery}"
    );
}

#[test]
fn a_followed_jira_item_the_search_window_does_not_list_is_not_read_again() {
    let user = user().with_fake_acli();
    fixture(&user);
    let (_, mcp) = jira_agent(&user, &[commented_item("ABC-1", &[], 0)]);
    subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["commented"]}));
    let before = comment_reads(&user, "ABC-1");

    // Nothing changed since the last round: Jira lists nothing in its window.
    user.acli_script(json!({
        "items": {"ABC-1": commented_item("ABC-1", &[], 0)}, "search": []}));
    a_round(&user);

    assert_eq!(
        comment_reads(&user, "ABC-1"),
        before,
        "no read for no change"
    );
    assert!(delivered(&user).is_empty());
}

#[test]
fn a_jira_comment_flottille_posted_is_marked_posted_by_an_agent() {
    let user = user().with_fake_acli();
    fixture(&user);
    let (_, mcp) = jira_agent(&user, &[commented_item("ABC-1", &[], 0)]);
    subscribe_ticket(
        &mcp,
        json!({"ticket": "ABC-1", "events": ["commented"], "from": "people"}),
    );

    // The footer Flottille ends what an agent posts with: no wake for `people`.
    let posted = jira_comment(
        "Casey",
        "Done on the branch.\n\n— posted by a Flottille agent",
    );
    jira_script(
        &user,
        &[commented_item("ABC-1", std::slice::from_ref(&posted), 5)],
    );
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    // And it is said so to the agent that hears everyone.
    subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["commented"]}));
    let person = jira_comment("Bob", "Thanks");
    jira_script(
        &user,
        &[commented_item("ABC-1", &[posted.clone(), person], 10)],
    );
    a_round(&user);
    let delivery = the_delivery(&user);
    assert!(delivery.contains("A comment by Bob: Thanks"), "{delivery}");
    assert!(!delivery.contains("Casey"), "{delivery}");

    subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["commented"]}));
    let again = jira_comment("Casey", "Pushed.\n\n— posted by a Flottille agent");
    jira_script(
        &user,
        &[commented_item(
            "ABC-1",
            &[posted, jira_comment("Bob", "Thanks"), again],
            15,
        )],
    );
    a_round(&user);
    eventually("the second delivery", || delivered(&user).len() == 2);
    let delivery = delivered(&user).pop().unwrap();
    assert!(
        delivery.contains("A comment by Casey (posted by an agent): Pushed."),
        "{delivery}"
    );
    assert!(
        !delivery.contains("— posted by a Flottille agent"),
        "{delivery}"
    );
}

#[test]
fn the_comments_of_a_jira_item_no_subscription_follows_are_not_read() {
    let user = user().with_fake_acli();
    fixture(&user);
    let (_, mcp) = jira_agent(
        &user,
        &[
            commented_item("ABC-1", &[], 0),
            commented_item("ABC-2", &[], 0),
        ],
    );
    subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["commented"]}));

    let said = jira_comment("Bob", "Over here");
    jira_script(
        &user,
        &[
            commented_item("ABC-1", std::slice::from_ref(&said), 5),
            commented_item("ABC-2", &[said], 5),
        ],
    );
    a_round(&user);

    let delivery = the_delivery(&user);
    assert!(
        delivery.contains("A comment by Bob: Over here"),
        "{delivery}"
    );
    assert_eq!(
        comment_reads(&user, "ABC-1"),
        1,
        "the round read the followed one"
    );
    assert_eq!(comment_reads(&user, "ABC-2"), 0, "and not the other");
}

#[test]
fn the_live_read_of_a_jira_item_has_its_status_category() {
    let user = user().with_fake_acli();
    fixture(&user);
    // The rounds read the tracker and do not list ABC-1: made since, or too old to be listed.
    let (_, mcp) = jira_agent(&user, &[]);
    user.acli_script(json!({
        "items": {"ABC-1": work_item_in("ABC-1", "Done", "done")}, "search": []}));

    // A closed ticket cannot be subscribed to (nothing would end it): `show` reads it live.
    let shown = mcp.call("tickets", json!({"action": "show", "ticket": "ABC-1"}));
    assert!(!shown.error, "{}", shown.text);

    // Stored by that read, closed because its status is in the done category.
    let stored = user
        .call("items.list", json!({"project": "app", "kind": "ticket"}))
        .unwrap()["items"]
        .clone();
    let stored = stored.as_array().unwrap();
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert_eq!(stored[0]["key"], "ABC-1");
    assert_eq!(stored[0]["state"], "closed");
    assert_eq!(stored[0]["details"]["status"], "Done");
    assert_eq!(stored[0]["details"]["category"], "done");

    // So its reopening, the next round finds, is a change from there.
    let watch = user.watch(&["--after", "0"]);
    let mut reopened = work_item_in("ABC-1", "In Progress", "indeterminate");
    reopened["fields"]["updated"] = "2026-10-02T10:05:00.000+0000".into();
    jira_script(&user, &[reopened]);
    a_round(&user);
    let events = watch.until(|e| e["name"] == "item.changed" && e["data"]["key"] == "ABC-1");
    let changes = &events.last().unwrap()["data"]["changes"];
    assert_eq!(
        changes["state"],
        json!({"from": "closed", "to": "open"}),
        "{changes}"
    );
    assert_eq!(
        changes["status"],
        json!({"from": "Done", "to": "In Progress"}),
        "{changes}"
    );
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
    assert!(delivery.contains("Nothing came in 5 min"), "{delivery}");
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
    // The rounds read the tracker, and not ABC-1: created since, as `tickets create` does. Its
    // status has no category, so its read says no state and stores nothing.
    let mut item = work_item("To Do");
    item["fields"]["status"] = json!({"name": "To Do"});
    user.acli_script(json!({"items": {"ABC-1": item}, "search": []}));
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
    // Slower than the 6 s a subscription is given, so one that waited for the round fails; yet
    // well within the 10 s the client waits for `items.refresh`, which answers after the round.
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": changed(ended(pull()))}},
        "logs": {"acme/api": {"456": "test\tRun cargo test\t2026-01-01T00:00:00.0000000Z failed\n"}},
        "listing_delay_ms": 8_000
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
    // A comment comes while the log is read: the checks ended first, and are delivered with it
    // after them, as one.
    let mut commented = ended(pull());
    commented["comments"] = json!([{"author": {"login": "alice"}, "body": "Why a new form?"}]);
    user.gh_script(slowly(commented));
    a_round(&user);

    let delivery = the_delivery(&user);
    let checks = delivery.find("checks ended: 1 passed, 1 failed");
    let comment = delivery.find("A comment by alice: Why a new form?");
    assert!(checks.is_some() && checks < comment, "{delivery}");
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

/// A check run of the repository's Actions that took `seconds`, `job` its job.
fn timed(name: &str, conclusion: &str, job: u64, seconds: u64) -> Value {
    let mut run = check(name, "COMPLETED", conclusion, job);
    run["startedAt"] = "2026-01-01T10:00:00Z".into();
    run["completedAt"] = format!("2026-01-01T10:{:02}:{:02}Z", seconds / 60, seconds % 60).into();
    run
}

/// The pull request whose checks ended with `failed` jobs failing and `build` passing: the jobs
/// are `job-1` and so on, each took 2 min 5 s more than the one before.
fn ended_with(failed: u64) -> Value {
    let mut pull = pull();
    let mut checks = vec![timed("build", "SUCCESS", 1, 30)];
    for n in 1..=failed {
        checks.push(timed(&format!("job-{n}"), "FAILURE", 500 + n, 125 * n));
    }
    pull["statusCheckRollup"] = checks.into();
    pull
}

/// The failed log of each job of `ended_with`: a long output (set up, built, then failed) whose
/// error line is the job's own.
fn failed_logs(failed: u64) -> Value {
    let mut logs = serde_json::Map::new();
    for n in 1..=failed {
        let stamp = "2026-01-01T00:00:00.1234567Z";
        let mut lines = vec![
            format!("job-{n}\tRun cargo test\t{stamp} ##[group]Run cargo test"),
            format!("job-{n}\tRun cargo test\t{stamp} cargo test"),
            format!("job-{n}\tRun cargo test\t{stamp} ##[endgroup]"),
        ];
        lines.extend(
            (1..=30).map(|k| format!("job-{n}\tRun cargo test\t{stamp} compiling crate {n}-{k}")),
        );
        lines.push(format!(
            "job-{n}\tRun cargo test\t{stamp} error[E0425]: cannot find value `x{n}` in this scope"
        ));
        lines.push(format!(
            "job-{n}\tRun cargo test\t{stamp} ##[error]Process completed with exit code 101."
        ));
        logs.insert((500 + n).to_string(), (lines.join("\n") + "\n").into());
    }
    json!({"acme/api": logs})
}

/// The daemon subscribed to the checks of `app/login`, which end with `failed` jobs failing:
/// the delivery the agent is woken by.
fn woken_by(user: &User, failed: u64) -> (String, Mcp) {
    fixture(user);
    script(user, pull());
    let (_, mcp) = agent(user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": changed(ended_with(failed))}},
        "logs": failed_logs(failed)
    }));
    a_round(user);
    (the_delivery(user), mcp)
}

#[test]
fn a_wake_with_three_failed_jobs_is_a_summary() {
    let user = user();
    let (delivery, _) = woken_by(&user, 3);

    assert!(
        delivery.contains("checks ended: 1 passed, 3 failed"),
        "{delivery}"
    );
    // It was about 130 lines, each job's log inlined.
    assert!(delivery.lines().count() <= 14, "{delivery}");
    assert!(!delivery.contains("compiling crate"), "{delivery}");
    assert!(!delivery.contains("the last"), "{delivery}");
    // It ends by saying where the details are.
    assert!(delivery.contains("`forge` `show`"), "{delivery}");
}

#[test]
fn each_failed_job_says_its_error_line() {
    let user = user();
    let (delivery, _) = woken_by(&user, 3);

    for (n, took) in [(1, "2 min 5 s"), (2, "4 min 10 s"), (3, "6 min 15 s")] {
        let line =
            format!("- job-{n} ({took}): error[E0425]: cannot find value `x{n}` in this scope");
        assert!(delivery.contains(&line), "{line} in {delivery}");
    }
    // The generic exit code of the runner says nothing of why.
    assert!(!delivery.contains("Process completed"), "{delivery}");
}

#[test]
fn the_full_log_is_still_in_show() {
    let user = user();
    let (delivery, mcp) = woken_by(&user, 3);
    assert!(!delivery.contains("compiling crate"), "{delivery}");

    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(!shown.error, "{}", shown.text);
    for part in [
        "failed jobs (3):",
        "- job-1: the last",
        "- job-3: the last",
        "compiling crate 1-30",
        "compiling crate 3-30",
        "error[E0425]: cannot find value `x2` in this scope",
        "Process completed with exit code 101.",
    ] {
        assert!(shown.text.contains(part), "{part} in {}", shown.text);
    }
}

#[test]
fn past_the_cap_the_failed_jobs_are_counted() {
    let user = user();
    let (delivery, _) = woken_by(&user, 8);

    assert!(
        delivery.contains("checks ended: 1 passed, 8 failed"),
        "{delivery}"
    );
    for listed in ["job-1 (", "job-2 (", "job-3 ("] {
        assert!(delivery.contains(listed), "{listed} in {delivery}");
    }
    assert!(
        delivery.contains("and 5 more failed, not listed"),
        "{delivery}"
    );
    for left_out in ["job-4", "job-8"] {
        assert!(!delivery.contains(left_out), "{left_out} in {delivery}");
    }
    assert!(delivery.lines().count() <= 14, "{delivery}");
    // Only the listed ones have their log read.
    let read = user
        .gh_received()
        .iter()
        .filter(|call| call["args"].to_string().contains("--log-failed"))
        .count();
    assert_eq!(read, 3);
}

#[test]
fn a_pull_request_subscription_wakes_at_each_checks_end_until_merged() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));

    script(&user, ended(pull()));
    a_round(&user);
    eventually("the first end", || delivered(&user).len() == 1);
    let first = delivered(&user).remove(0);
    assert!(first.contains("checks ended"), "{first}");
    assert!(first.contains("Your subscription goes on"), "{first}");

    // A new commit: its checks run, then end, and wake it again with no new subscription.
    let mut again = pull();
    again["headRefOid"] = "bbbbbbb2".into();
    script(&user, again.clone());
    a_round(&user);
    script(&user, ended(again.clone()));
    a_round(&user);
    eventually("the second end", || delivered(&user).len() == 2);
    assert_eq!(listed(&user, &id).len(), 1, "it goes on");

    let mut merged = ended(again);
    merged["state"] = "MERGED".into();
    script(&user, merged);
    a_round(&user);
    eventually("the merge", || delivered(&user).len() == 3);
    let last = delivered(&user).pop().unwrap();
    assert!(
        last.contains("was merged: your subscription to checks is over"),
        "{last}"
    );
    assert!(listed(&user, &id).is_empty());
}

#[test]
fn a_ticket_subscription_wakes_at_each_comment_until_the_ticket_is_closed() {
    let user = user();
    fixture(&user);
    track(&user, json!({"kind": "github", "repository": "api"}));
    script_issue(&user, issue(json!([])));
    let (id, mcp) = agent(&user);
    subscribe_ticket(&mcp, json!({"ticket": "#5", "events": ["commented"]}));

    let alice = json!({"author": {"login": "alice"}, "body": "I can reproduce"});
    let bob = json!({"author": {"login": "bob"}, "body": "Me too"});
    script_issue(&user, issue(json!([alice])));
    a_round(&user);
    eventually("the first comment", || delivered(&user).len() == 1);
    script_issue(&user, issue(json!([alice, bob])));
    a_round(&user);
    eventually("the second comment", || delivered(&user).len() == 2);
    let second = delivered(&user).pop().unwrap();
    assert!(second.contains("A comment by bob: Me too"), "{second}");
    assert!(!second.contains("alice"), "{second}");
    assert_eq!(listed(&user, &id).len(), 1, "it goes on");

    let mut closed = issue(json!([alice, bob]));
    closed["state"] = "CLOSED".into();
    script_issue(&user, closed);
    a_round(&user);
    eventually("the close", || delivered(&user).len() == 3);
    let last = delivered(&user).pop().unwrap();
    assert!(
        last.contains("was closed: your subscription to commented is over"),
        "{last}"
    );
    assert!(listed(&user, &id).is_empty());
}

#[test]
fn a_jira_item_moved_to_a_done_status_ends_its_subscription() {
    let user = user().with_fake_acli();
    fixture(&user);
    track(
        &user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    script(&user, pull());
    user.acli_script(json!({"items": {"ABC-1": work_item("To Do")}}));
    let (id, mcp) = agent(&user);
    subscribe_ticket(&mcp, json!({"ticket": "ABC-1", "events": ["status"]}));

    user.acli_script(json!({"items": {"ABC-1": work_item("In Progress")}}));
    a_round(&user);
    eventually("the first status", || delivered(&user).len() == 1);
    assert_eq!(listed(&user, &id).len(), 1, "it goes on");

    // Done is Jira's status category, whatever the status is called.
    user.acli_script(json!({"items": {"ABC-1": work_item_in("ABC-1", "Shipped", "done")}}));
    a_round(&user);
    eventually("the end", || delivered(&user).len() == 2);
    let last = delivered(&user).pop().unwrap();
    assert!(last.contains("Shipped"), "{last}");
    assert!(
        last.contains("was closed: your subscription to status is over"),
        "{last}"
    );
    assert!(listed(&user, &id).is_empty());
}

#[test]
fn events_of_one_round_are_one_wake() {
    let user = user();
    fixture(&user);
    track(&user, json!({"kind": "github", "repository": "api"}));
    script_issue(&user, issue(json!([])));
    let (_, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["comment", "review"]}));
    subscribe_ticket(&mcp, json!({"ticket": "#5", "events": ["commented"]}));

    // One round finds a comment and a review on the pull request, and a comment on the ticket.
    let mut pull = pull();
    pull["comments"] = json!([{"author": {"login": "alice"}, "body": "Why a new form?"}]);
    pull["reviews"] = json!([{"author": {"login": "bob"}, "state": "CHANGES_REQUESTED",
        "body": "Rename the form"}]);
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": changed(pull)}},
        "issues": {"acme/api": {"5": issue(json!([{"author": {"login": "carol"}, "body": "Seen"}]))}}
    }));
    a_round(&user);

    let delivery = the_delivery(&user);
    for part in [
        "A comment by alice: Why a new form?",
        "review by bob, changes requested: Rename the form",
        "A comment by carol: Seen",
    ] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(delivered(&user).len(), 1, "{:?}", delivered(&user));
}

#[test]
fn events_during_a_turn_are_delivered_together_after_it() {
    let user = user();
    user.fake_script(json!({
        "turns": [{"when": "slow", "do": [{"hold": "release"}, {"say": "done"}]}]
    }));
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["comment"]}));
    user.call("agent.send", json!({"agent": id, "message": "slow"}))
        .unwrap();
    eventually("the slow turn", || state(&user, &id) == "working");

    // Two rounds, each with a comment, while the agent works.
    let alice = json!({"author": {"login": "alice"}, "body": "First"});
    let bob = json!({"author": {"login": "bob"}, "body": "Second"});
    let mut commented = pull();
    commented["comments"] = json!([alice]);
    script(&user, commented.clone());
    a_round(&user);
    commented["comments"] = json!([alice, bob]);
    script(&user, commented);
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    std::fs::write(user.path("fake/release"), "").unwrap();
    let delivery = the_delivery(&user);
    let (first, second) = (
        delivery.find("A comment by alice: First"),
        delivery.find("A comment by bob: Second"),
    );
    assert!(first.is_some() && first < second, "{delivery}");
}

#[test]
fn a_subscription_without_minutes_has_no_deadline() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    let row = listed(&user, &id).remove(0);
    assert!(
        row["expires_at"].as_u64().unwrap() >= i64::MAX as u64,
        "{row}"
    );

    // A week passes: the subscription is still there, and still says nothing.
    user.call("test.advance", json!({"ms": 7 * 24 * 3_600_000_u64}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(listed(&user, &id).len(), 1);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(
        shown.text.contains(
            "- pull request #12 of api, workspace login: checks, until it is merged or closed"
        ),
        "{}",
        shown.text
    );

    script(&user, ended(pull()));
    a_round(&user);
    assert!(the_delivery(&user).contains("checks ended"));

    // A deadline as long as the agent wants: there is no cap any more.
    let long = subscribe(&mcp, json!({"events": ["checks"], "minutes": 60_000}));
    assert!(
        long.contains("or for 60000 min, the deadline you gave"),
        "{long}"
    );
    let zero = mcp.call(
        "forge",
        json!({"action": "subscribe", "events": ["checks"], "minutes": 0}),
    );
    assert!(zero.error, "{}", zero.text);
}

#[test]
fn a_deadline_wakes_once_with_nothing_came_and_ends() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"], "minutes": 5}));
    let shown = mcp.call("forge", json!({"action": "show"}));
    assert!(
        shown.text.contains("checks, for 5 min more"),
        "{}",
        shown.text
    );

    user.call("test.advance", json!({"ms": 6 * 60_000}))
        .unwrap();
    let delivery = the_delivery(&user);
    assert!(delivery.contains("Nothing came in 5 min"), "{delivery}");
    assert!(delivery.contains("checks"), "{delivery}");
    assert!(listed(&user, &id).is_empty());

    user.call("test.advance", json!({"ms": 60 * 60_000}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(delivered(&user).len(), 1, "{:?}", delivered(&user));
}

#[test]
fn a_subscription_ends_with_its_agent() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));
    let watch = user.watch(&["--after", "0"]);

    user.call("agent.stop", json!({"agent": id})).unwrap();

    let seen = watch.until(|e| e["name"] == "subscription.ended");
    assert_eq!(
        seen.last().unwrap()["data"]["why"],
        "agent-ended",
        "{seen:?}"
    );
    assert!(listed(&user, &id).is_empty());
    script(&user, ended(pull()));
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
}

#[test]
fn a_subscription_stored_with_a_deadline_keeps_it() {
    let user = user();
    fixture(&user);
    script(&user, pull());
    let (id, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["checks"]}));

    // What a subscription stored before it lasted until its target ended looks like: the hour it
    // was given by default.
    let store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let mut old = store.subscriptions().unwrap().remove(0);
    old.expires_at = old.created_at + 60 * 60_000;
    let mut store = store;
    store.subscribe(&old).unwrap();
    drop(store);
    let row = listed(&user, &id).remove(0);
    assert_eq!(
        row["expires_at"].as_u64().unwrap() - row["created_at"].as_u64().unwrap(),
        60 * 60_000,
        "{row}"
    );

    user.call("test.advance", json!({"ms": 61 * 60_000}))
        .unwrap();
    let delivery = the_delivery(&user);
    assert!(delivery.contains("Nothing came in 60 min"), "{delivery}");
    // The hour was the old default, not the agent's choice: it is not said to be.
    assert!(!delivery.contains("you gave"), "{delivery}");
    assert!(listed(&user, &id).is_empty());
}

#[test]
fn the_subscribe_answers_say_no_need_to_subscribe_again() {
    let user = user();
    fixture(&user);
    track(&user, json!({"kind": "github", "repository": "api"}));
    script_issue(&user, issue(json!([])));
    let (_, mcp) = agent(&user);

    let pull = subscribe(&mcp, json!({"events": ["checks", "review"]}));
    assert!(
        pull.contains(
            "it wakes you at each of these events until it is merged or closed; no need to \
             subscribe again"
        ),
        "{pull}"
    );
    let ticket = subscribe_ticket(&mcp, json!({"ticket": "#5", "events": ["commented"]}));
    assert!(
        ticket.contains(
            "it wakes you at each of these events until the ticket is closed; no need to \
             subscribe again"
        ),
        "{ticket}"
    );
    let shown = mcp.call("tickets", json!({"action": "show", "ticket": "#5"}));
    assert!(
        shown.text.contains(
            "Your subscriptions:\n- ticket #5 of api: commented, until the ticket is closed"
        ),
        "{}",
        shown.text
    );
}

#[test]
fn subscribing_to_a_closed_ticket_is_refused() {
    // A GitHub issue closed: no `item.closed` would ever come to end it.
    let user = user();
    fixture(&user);
    track(&user, json!({"kind": "github", "repository": "api"}));
    let mut closed = issue(json!([]));
    closed["state"] = "CLOSED".into();
    script_issue(&user, closed);
    let (id, mcp) = agent(&user);

    let answer = mcp.call(
        "tickets",
        json!({"action": "subscribe", "ticket": "#5", "events": ["commented"]}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("the ticket #5 is already closed"),
        "{}",
        answer.text
    );
    assert!(listed(&user, &id).is_empty());

    // A Jira work item in a status of the category done, whatever it is called.
    let user = self::user().with_fake_acli();
    fixture(&user);
    track(
        &user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    script(&user, pull());
    user.acli_script(json!({"items": {"ABC-1": work_item_in("ABC-1", "Shipped", "done")}}));
    let (id, mcp) = agent(&user);

    let answer = mcp.call(
        "tickets",
        json!({"action": "subscribe", "ticket": "ABC-1", "events": ["commented"]}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("the ticket ABC-1 is already closed"),
        "{}",
        answer.text
    );
    assert!(listed(&user, &id).is_empty());
}

// The `approved` event: the pull request's review decision becomes approved (#513).

/// `pull` with its review decision and the reviews that lead to it, as `gh pr view --json` says
/// them.
fn decided(mut pull: Value, decision: &str, reviews: &[(&str, &str)]) -> Value {
    pull["reviewDecision"] = decision.into();
    pull["reviews"] = reviews
        .iter()
        .map(|(author, state)| json!({"author": {"login": author}, "state": state, "body": ""}))
        .collect();
    pull
}

/// `pull` whose two checks passed.
fn passing(mut pull: Value) -> Value {
    pull["statusCheckRollup"][1] = check("test", "COMPLETED", "SUCCESS", 456);
    pull
}

/// The deliveries to the agent, once there are `count` of them.
fn deliveries(user: &User, count: usize) -> Vec<String> {
    eventually("the deliveries", || delivered(user).len() >= count);
    // What else came with them would show.
    std::thread::sleep(Duration::from_millis(300));
    let all = delivered(user);
    assert_eq!(all.len(), count, "{all:?}");
    all
}

#[test]
fn the_last_approval_on_github_wakes_an_approved_subscription() {
    let user = user();
    fixture(&user);
    script(&user, decided(passing(pull()), "REVIEW_REQUIRED", &[]));
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["approved"]}));
    // One approval of two: GitHub's decision is still to be taken.
    script(
        &user,
        decided(
            passing(pull()),
            "REVIEW_REQUIRED",
            &[("bob", "APPROVED"), ("carol", "COMMENTED")],
        ),
    );
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));

    script(
        &user,
        decided(
            passing(pull()),
            "APPROVED",
            &[("bob", "APPROVED"), ("carol", "APPROVED")],
        ),
    );
    a_round(&user);
    let delivery = the_delivery(&user);
    for part in ["pull request #12", "approved", "bob", "carol"] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }
}

#[test]
fn an_already_approved_pull_request_is_no_event() {
    let user = user();
    fixture(&user);
    script(
        &user,
        decided(passing(pull()), "APPROVED", &[("bob", "APPROVED")]),
    );
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["approved"]}));
    a_round(&user);
    // A round that reads it approved again, with more said: still no event.
    let mut commented = decided(passing(pull()), "APPROVED", &[("bob", "APPROVED")]);
    commented["comments"] = json!([{"author": {"login": "alice"}, "body": "Nice"}]);
    script(&user, commented);
    a_round(&user);
    // Another approval of a pull request that is approved already.
    script(
        &user,
        decided(
            passing(pull()),
            "APPROVED",
            &[("bob", "APPROVED"), ("carol", "APPROVED")],
        ),
    );
    a_round(&user);
    assert!(delivered(&user).is_empty(), "{:?}", delivered(&user));
}

#[test]
fn approved_after_changes_requested_is_a_new_event() {
    let user = user();
    fixture(&user);
    script(&user, decided(passing(pull()), "REVIEW_REQUIRED", &[]));
    let (_, mcp) = agent(&user);

    subscribe(&mcp, json!({"events": ["approved"]}));
    script(
        &user,
        decided(passing(pull()), "APPROVED", &[("bob", "APPROVED")]),
    );
    a_round(&user);
    assert!(deliveries(&user, 1)[0].contains("bob"));

    // A reviewer asks for changes: the decision goes back, and says nothing.
    script(
        &user,
        decided(
            passing(pull()),
            "CHANGES_REQUESTED",
            &[("bob", "APPROVED"), ("carol", "CHANGES_REQUESTED")],
        ),
    );
    a_round(&user);
    deliveries(&user, 1);

    // She approves it: approved again, a new event.
    script(
        &user,
        decided(
            passing(pull()),
            "APPROVED",
            &[("bob", "APPROVED"), ("carol", "APPROVED")],
        ),
    );
    a_round(&user);
    let all = deliveries(&user, 2);
    assert!(all[1].contains("carol"), "{}", all[1]);
}

#[test]
fn the_approved_delivery_says_the_checks() {
    let user = user();
    fixture(&user);
    script(&user, decided(pull(), "REVIEW_REQUIRED", &[]));
    let (_, mcp) = agent(&user);
    subscribe(&mcp, json!({"events": ["approved"]}));

    let approve = |pull: Value| {
        script(
            &user,
            decided(
                pull.clone(),
                "CHANGES_REQUESTED",
                &[("bob", "CHANGES_REQUESTED")],
            ),
        );
        a_round(&user);
        script(&user, decided(pull, "APPROVED", &[("bob", "APPROVED")]));
        a_round(&user);
    };
    // The test job still runs.
    script(&user, decided(pull(), "APPROVED", &[("bob", "APPROVED")]));
    a_round(&user);
    assert!(
        deliveries(&user, 1)[0].contains("checks still running"),
        "{:?}",
        delivered(&user)
    );
    // It failed.
    approve(ended(pull()));
    let all = deliveries(&user, 2);
    assert!(all[1].contains("checks: 1 failed"), "{}", all[1]);
    // It passed.
    approve(passing(pull()));
    let all = deliveries(&user, 3);
    assert!(all[2].contains("checks: 2 passed"), "{}", all[2]);
}

#[test]
fn subscribing_to_approved_on_github_with_no_required_review_says_it_will_not_come() {
    const LINE: &str =
        "This repository asks for no review, so approved will not come; subscribe to review.";
    let user = user();
    fixture(&user);
    // GitHub says no decision at all when the base branch requires no review.
    script(&user, passing(pull()));
    let (_, mcp) = agent(&user);

    let said = subscribe(&mcp, json!({"events": ["approved"]}));
    assert!(said.contains(LINE), "{said}");
    // Only `approved` waits for a decision: `review` and the others are not told.
    let said = subscribe(&mcp, json!({"events": ["review", "checks"]}));
    assert!(!said.contains(LINE), "{said}");

    // A repository that requires a review has a decision, even before anyone reviewed.
    script(&user, decided(passing(pull()), "REVIEW_REQUIRED", &[]));
    a_round(&user);
    let said = subscribe(&mcp, json!({"events": ["approved"]}));
    assert!(!said.contains(LINE), "{said}");
}

/// How a Bitbucket pull request is read: Cloud and Data Center say it in their own shapes.
#[derive(Clone, Copy, Debug)]
enum Bitbucket {
    Cloud,
    DataCenter,
}

impl Bitbucket {
    const ALL: [Bitbucket; 2] = [Bitbucket::Cloud, Bitbucket::DataCenter];

    /// A user whose forge is the fake `bkt`, with the clock tests move.
    fn user(self) -> User {
        let user = User::with_fake_agent()
            .with_fake_bkt()
            .with_env("FLOTTILLE_TEST_CLOCK", "1");
        let (url, forge) = match self {
            Bitbucket::Cloud => (
                "git@bitbucket.org:acme/api.git",
                json!({"kind": "bitbucket-cloud", "repository": "acme/api"}),
            ),
            Bitbucket::DataCenter => (
                "ssh://git@bitbucket.example.com:7999/APP/api.git",
                json!({"kind": "bitbucket-data-center", "host": "bitbucket.example.com",
                    "repository": "APP/api"}),
            ),
        };
        fixture_on(&user, &[], url, forge);
        user
    }

    /// The fake `bkt` holding the pull request #12 of `app/login`, its reviewers and their
    /// verdicts (`approved`, `changes requested`, `pending`) and its one check `state`. Each
    /// script is a change of the pull request, which Bitbucket says by a newer time.
    fn script(self, user: &User, reviewers: &[(&str, &str)], state: &str) {
        static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let at = format!("2999-01-01T{:02}:{:02}:00+00:00", n / 60 % 24, n % 60);
        let checks = json!([{"state": state, "name": "build"}]);
        user.bkt_script(match self {
            Bitbucket::Cloud => {
                let pull = json!({"id": 12, "title": "Add the login page", "state": "OPEN",
                    "author": {"display_name": "Alice"},
                    "source": {"branch": {"name": "app/login"}, "commit": {"hash": "abcdef1"},
                        "repository": {"full_name": "acme/api"}},
                    "destination": {"branch": {"name": "main"},
                        "repository": {"full_name": "acme/api"}},
                    "links": {"html": {"href": "https://bitbucket.org/acme/api/pull-requests/12"}},
                    "comment_count": 0, "updated_on": at,
                    "created_on": "2020-01-01T00:00:00+00:00"});
                let participants: Vec<Value> = reviewers
                    .iter()
                    .map(|(name, verdict)| {
                        let state = match *verdict {
                            "approved" => json!("approved"),
                            "changes requested" => json!("changes_requested"),
                            _ => Value::Null,
                        };
                        json!({"role": "REVIEWER", "approved": *verdict == "approved",
                            "state": state, "user": {"display_name": name}})
                    })
                    .collect();
                json!({"contexts": [{"name": "cloud", "host": "bitbucket.org", "active": true}],
                    "pulls": {"acme/api": [pull]},
                    "api": {
                        "/repositories/acme/api/pullrequests": {"values": [pull]},
                        "/repositories/acme/api/pullrequests/12": {"participants": participants},
                        "/repositories/acme/api/pullrequests/12/comments": {"values": []},
                        "/repositories/acme/api/pullrequests/12/diffstat": {"values": []}
                    },
                    "checks": {"acme/api": {"12": checks}}})
            }
            Bitbucket::DataCenter => {
                let repository = json!({"slug": "api", "project": {"key": "APP"}});
                let participants: Vec<Value> = reviewers
                    .iter()
                    .map(|(name, verdict)| {
                        let status = match *verdict {
                            "approved" => "APPROVED",
                            "changes requested" => "NEEDS_WORK",
                            _ => "UNAPPROVED",
                        };
                        json!({"role": "REVIEWER", "approved": *verdict == "approved",
                            "status": status, "user": {"name": name.to_lowercase(),
                                "displayName": name}})
                    })
                    .collect();
                let pull = json!({"id": 12, "title": "Add the login page", "state": "OPEN",
                    "version": n, "createdDate": 1_600_000_000_000u64,
                    "updatedDate": 1_600_000_000_000u64 + n,
                    "author": {"user": {"name": "alice", "displayName": "Alice"}},
                    "fromRef": {"displayId": "app/login", "latestCommit": "abcdef1",
                        "repository": repository},
                    "toRef": {"displayId": "main", "latestCommit": "abcdef2",
                        "repository": repository},
                    "participants": participants,
                    "links": {"self": [{"href":
                        "https://bitbucket.example.com/projects/APP/repos/api/pull-requests/12"}]}});
                let base = "/rest/api/1.0/projects/APP/repos/api/pull-requests";
                json!({"contexts": [{"name": "work", "host": "bitbucket.example.com:7990",
                        "active": true}],
                    "pulls": {"APP/api": [pull]},
                    "api": {
                        base: {"values": [pull]},
                        format!("{base}/12/activities"): {"values": []},
                        format!("{base}/12/merge"): {"conflicted": false}
                    },
                    "checks": {"APP/api": {"12": checks}}})
            }
        });
    }
}

#[test]
fn one_approval_of_two_bitbucket_reviewers_is_no_approved_event() {
    for shape in Bitbucket::ALL {
        let user = shape.user();
        shape.script(
            &user,
            &[("Alice", "pending"), ("Bob", "pending")],
            "SUCCESSFUL",
        );
        let (_, mcp) = agent(&user);

        subscribe(&mcp, json!({"events": ["approved"]}));
        shape.script(
            &user,
            &[("Alice", "approved"), ("Bob", "pending")],
            "SUCCESSFUL",
        );
        a_round(&user);
        assert!(
            delivered(&user).is_empty(),
            "{shape:?}: {:?}",
            delivered(&user)
        );
        // One approves, the other asks for changes: not approved either.
        shape.script(
            &user,
            &[("Alice", "approved"), ("Bob", "changes requested")],
            "SUCCESSFUL",
        );
        a_round(&user);
        assert!(
            delivered(&user).is_empty(),
            "{shape:?}: {:?}",
            delivered(&user)
        );
    }
}

#[test]
fn the_second_bitbucket_approval_wakes_an_approved_subscription() {
    for shape in Bitbucket::ALL {
        let user = shape.user();
        shape.script(
            &user,
            &[("Alice", "approved"), ("Bob", "pending")],
            "SUCCESSFUL",
        );
        let (_, mcp) = agent(&user);

        subscribe(&mcp, json!({"events": ["approved"]}));
        shape.script(
            &user,
            &[("Alice", "approved"), ("Bob", "approved")],
            "SUCCESSFUL",
        );
        a_round(&user);
        let delivery = the_delivery(&user);
        for part in [
            "pull request #12",
            "approved",
            "Alice",
            "Bob",
            "checks: 1 passed",
        ] {
            assert!(delivery.contains(part), "{shape:?}: {part} in {delivery}");
        }
    }
}

#[test]
fn a_bitbucket_pull_request_with_no_reviewer_is_never_approved() {
    for shape in Bitbucket::ALL {
        let user = shape.user();
        shape.script(&user, &[], "INPROGRESS");
        let (_, mcp) = agent(&user);

        subscribe(&mcp, json!({"events": ["approved"]}));
        shape.script(&user, &[], "SUCCESSFUL");
        a_round(&user);
        assert!(
            delivered(&user).is_empty(),
            "{shape:?}: {:?}",
            delivered(&user)
        );
    }
}
