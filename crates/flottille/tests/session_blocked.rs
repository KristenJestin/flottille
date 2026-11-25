//! The scenarios of a session blocked by someone else (#1451, `docs/design.md` §10): a session at
//! rest whose open subscription waits on a person (a review requested of someone else than the
//! user, a ticket assigned to someone else) says who on its line of the tree, `blocked`, derived
//! from the items the daemon keeps, and its changes are events. Read through the fake `gh` and
//! `acli`, the clock moved by the test; no test calls a forge or a tracker.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// A user whose agents are the fake agent, whose forge is the fake `gh` (signed in as
/// `someone`) and whose tracker the fake `acli`, with the clock tests move.
fn user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_fake_acli()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// A project `app` of one repository `api` on GitHub `acme/api`, its issues its tracker, and its
/// ready workspace `login` on the branch `app/login`.
fn fixture(user: &User) {
    let dir = user.path("work/app/api");
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
    user.git(
        &dir,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    track(user, json!({"kind": "github", "repository": "api"}));
}

/// The project's tracker is `tracker`.
fn track(user: &User, tracker: Value) {
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}],
            "tracker": tracker}),
    )
    .unwrap();
}

/// The pull request #12 of `app/login`, its checks passed, a review requested of `requested`.
fn pull(requested: Value) -> Value {
    changed(json!({
        "number": 12,
        "title": "Add the login page",
        "state": "OPEN",
        "isDraft": false,
        "headRefOid": "aaaaaaa1",
        "baseRefName": "main",
        "url": "https://github.com/acme/api/pull/12",
        "author": {"login": "someone"},
        "assignees": [],
        "labels": [],
        "reviewRequests": requested,
        "statusCheckRollup": [{"__typename": "CheckRun", "name": "build",
            "status": "COMPLETED", "conclusion": "SUCCESS",
            "detailsUrl": "https://github.com/acme/api/actions/runs/900/job/1"}],
        "comments": [],
        "reviews": []
    }))
}

/// The issue #5 of `acme/api`, assigned to `assignees`.
fn issue(assignees: Value) -> Value {
    changed(json!({
        "number": 5, "title": "Users cannot sign in", "state": "OPEN", "body": "",
        "url": "https://github.com/acme/api/issues/5", "author": {"login": "someone"},
        "assignees": assignees, "labels": [], "comments": []
    }))
}

/// The fake `gh` answers `pull` and `issue`.
fn script(user: &User, pull: Value, issue: Value) {
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": pull}},
        "issues": {"acme/api": {"5": issue}}
    }));
}

/// `value` updated now: later than any change before it.
fn changed(mut value: Value) -> Value {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    value["updatedAt"] = format!("2999-01-01T{:02}:{:02}:00Z", n / 60 % 24, n % 60).into();
    value
}

/// A round of the project's items, as the daemon makes every few minutes.
fn a_round(user: &User) {
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
}

/// An agent started in the workspace `login`, once the daemon read the project's items, at rest
/// after its first turn: its id and its MCP session.
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
    eventually("its first turn", || listed(user, &id)["state"] == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

/// The session's line as `agent.list` gives it.
fn listed(user: &User, id: &str) -> Value {
    let agents = user.call("agent.list", json!({})).unwrap()["agents"].clone();
    agents
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == id)
        .cloned()
        .unwrap()
}

fn subscribe(mcp: &Mcp, tool: &str, args: Value) {
    let mut args = args;
    args["action"] = "subscribe".into();
    let answer = mcp.call(tool, args);
    assert!(!answer.error, "{}", answer.text);
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The session's `blocked`, once it is not `null`.
fn blocked(user: &User, id: &str) -> Value {
    eventually("the session blocked", || {
        !listed(user, id)["blocked"].is_null()
    });
    listed(user, id)["blocked"].clone()
}

/// The item `key` of `kind` as the daemon stored it: a test that finds nothing blocked first
/// knows there was something to read.
fn stored(user: &User, kind: &str, key: &str) -> Value {
    let items = user
        .call("items.list", json!({"project": "app", "kind": kind}))
        .unwrap()["items"]
        .clone();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["key"] == key)
        .cloned()
        .unwrap_or_else(|| panic!("no {kind} {key} stored: {items}"))
}

/// The last `agent.blocked` of `id` among `events`.
fn last_blocked(events: &[Value], id: &str) -> Value {
    events
        .iter()
        .filter(|e| e["name"] == "agent.blocked" && e["data"]["agent"] == id)
        .next_back()
        .unwrap_or_else(|| panic!("no agent.blocked of {id}: {events:?}"))["data"]["blocked"]
        .clone()
}

#[test]
fn a_session_at_rest_waiting_on_a_requested_review_is_blocked_by_its_reviewer() {
    let user = user();
    fixture(&user);
    let maya = json!([{"__typename": "User", "login": "maya", "name": "Maya"}]);
    script(&user, pull(maya), issue(json!([])));
    let (id, mcp) = agent(&user);
    let watch = user.watch(&["--after", "0"]);

    subscribe(&mcp, "forge", json!({"events": ["review", "checks"]}));

    let blocked = blocked(&user, &id);
    assert_eq!(blocked["by"], json!(["Maya"]), "{blocked}");
    assert_eq!(blocked["why"], "review of #12", "{blocked}");
    assert!(blocked["since"].as_u64().is_some(), "{blocked}");
    let events = watch.until(|e| e["name"] == "agent.blocked");
    let event = events.last().unwrap();
    assert_eq!(event["data"]["agent"], id.as_str(), "{event}");
    assert_eq!(event["data"]["blocked"], blocked, "{event}");
    // The command line says it too.
    let cli = user.run(&["agent", "list", "--json"]);
    assert!(cli.status.success(), "{cli:?}");
    let cli: Value = serde_json::from_slice(&cli.stdout).unwrap();
    let line = cli["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == id.as_str())
        .unwrap();
    assert_eq!(line["blocked"], blocked, "{line}");
}

#[test]
fn a_session_waiting_on_a_ticket_assigned_to_someone_else_is_blocked_by_them() {
    let user = user();
    fixture(&user);
    script(&user, pull(json!([])), issue(json!([{"login": "noor"}])));
    let (id, mcp) = agent(&user);

    subscribe(
        &mcp,
        "tickets",
        json!({"ticket": "#5", "events": ["commented", "status"]}),
    );

    let blocked = blocked(&user, &id);
    assert_eq!(blocked["by"], json!(["noor"]), "{blocked}");
    assert_eq!(blocked["why"], "ticket #5", "{blocked}");
}

#[test]
fn a_session_waiting_on_checks_is_waiting_not_blocked() {
    let user = user();
    fixture(&user);
    // A review is asked of someone, but the session waits for the checks alone: they come by
    // themselves.
    let maya = json!([{"__typename": "User", "login": "maya", "name": "Maya"}]);
    script(&user, pull(maya), issue(json!([])));
    let (id, mcp) = agent(&user);

    subscribe(&mcp, "forge", json!({"events": ["checks"]}));
    a_round(&user);

    stored(&user, "pull", "12");
    let line = listed(&user, &id);
    assert!(line["blocked"].is_null(), "{line}");
    let tree = user.call("agent.tree", json!({"agent": id})).unwrap();
    assert_eq!(tree["agents"][0]["waiting_on"], "subscription", "{tree}");
}

#[test]
fn a_ticket_assigned_to_the_user_blocks_nothing() {
    let user = user();
    fixture(&user);
    // `someone` is whom the fake `gh` says the user is.
    script(&user, pull(json!([])), issue(json!([{"login": "someone"}])));
    let (id, mcp) = agent(&user);

    subscribe(
        &mcp,
        "tickets",
        json!({"ticket": "#5", "events": ["commented"]}),
    );
    a_round(&user);

    let issue = stored(&user, "ticket", "5");
    assert_eq!(issue["assignees"], json!(["someone"]), "{issue}");
    let line = listed(&user, &id);
    assert!(line["blocked"].is_null(), "{line}");
}

#[test]
fn a_blocked_session_is_no_longer_blocked_when_the_review_comes() {
    let user = user();
    fixture(&user);
    let maya = json!([{"__typename": "User", "login": "maya", "name": "Maya"}]);
    script(&user, pull(maya), issue(json!([])));
    let (id, mcp) = agent(&user);
    subscribe(&mcp, "forge", json!({"events": ["review"]}));
    blocked(&user, &id);
    let watch = user.watch(&["--after", "0"]);

    // Maya reviews: GitHub no longer lists her among the requested reviewers.
    let mut reviewed = pull(json!([]));
    reviewed["reviews"] = json!([{"author": {"login": "maya"}, "state": "APPROVED",
        "body": "Fine"}]);
    reviewed["reviewDecision"] = "APPROVED".into();
    script(&user, reviewed, issue(json!([])));
    a_round(&user);

    // The review wakes the agent, which is blocked by no one while it runs: it is back at rest,
    // its subscription still open, that says whether Maya still blocks it.
    let mut events = watch.until(|e| e["name"] == "agent.turn_ended" && e["data"]["agent"] == id);
    eventually("the session at rest again", || {
        listed(&user, &id)["state"] == "idle"
    });
    events.extend(watch.within(Duration::from_secs(1)));
    let open = user.call("subscription.list", json!({"tree": id})).unwrap();
    assert!(
        open.to_string().contains("\"forge\""),
        "the subscription ended: {open}"
    );
    let line = listed(&user, &id);
    assert!(line["blocked"].is_null(), "{line}");
    assert!(last_blocked(&events, &id).is_null(), "{events:?}");
}

#[test]
fn a_review_asked_after_subscribing_blocks_the_session() {
    let user = user();
    fixture(&user);
    script(&user, pull(json!([])), issue(json!([])));
    let (id, mcp) = agent(&user);
    subscribe(&mcp, "forge", json!({"events": ["review"]}));
    a_round(&user);
    stored(&user, "pull", "12");
    assert!(listed(&user, &id)["blocked"].is_null());
    let watch = user.watch(&["--after", "0"]);

    // The user asks Maya for a review: nothing else of the pull request changes.
    let maya = json!([{"__typename": "User", "login": "maya", "name": "Maya"}]);
    script(&user, pull(maya), issue(json!([])));
    a_round(&user);

    let events = watch.until(|e| {
        e["name"] == "agent.blocked" && e["data"]["agent"] == id && !e["data"]["blocked"].is_null()
    });
    let blocked = &events.last().unwrap()["data"]["blocked"];
    assert_eq!(blocked["by"], json!(["Maya"]), "{blocked}");
    assert_eq!(listed(&user, &id)["blocked"], *blocked);

    // The request is withdrawn: blocked by no one, and said.
    script(&user, pull(json!([])), issue(json!([])));
    a_round(&user);
    watch.until(|e| {
        e["name"] == "agent.blocked" && e["data"]["agent"] == id && e["data"]["blocked"].is_null()
    });
    assert!(listed(&user, &id)["blocked"].is_null());
}

#[test]
fn a_review_requested_of_the_user_blocks_nothing() {
    let user = user();
    fixture(&user);
    // `someone` is whom the fake `gh` says the user is, here by the name they set.
    let requested = json!([
        {"__typename": "User", "login": "someone", "name": "Some One"},
        {"__typename": "User", "login": "maya", "name": "Maya"}
    ]);
    script(&user, pull(requested), issue(json!([])));
    let (id, mcp) = agent(&user);

    subscribe(&mcp, "forge", json!({"events": ["review"]}));

    let blocked = blocked(&user, &id);
    assert_eq!(blocked["by"], json!(["Maya"]), "{blocked}");
}

#[test]
fn a_review_requested_of_a_bot_blocks_nothing() {
    let user = user();
    fixture(&user);
    // As gh asks `requestedReviewer`: a bot and a team are no person.
    let requested = json!([
        {"__typename": "Bot", "login": "copilot-pull-request-reviewer"},
        {"__typename": "Team", "name": "Backend", "slug": "backend"},
        {"__typename": "User", "login": "maya", "name": "Maya"}
    ]);
    script(&user, pull(requested), issue(json!([])));
    let (id, mcp) = agent(&user);

    subscribe(&mcp, "forge", json!({"events": ["review"]}));

    let blocked = blocked(&user, &id);
    assert_eq!(blocked["by"], json!(["Maya"]), "{blocked}");
}

#[test]
fn a_jira_ticket_the_user_reported_assigned_to_someone_else_blocks() {
    let user = user();
    fixture(&user);
    script(&user, pull(json!([])), issue(json!([])));
    track(
        &user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    let work_item = |key: &str, assignee: &str| {
        json!({
            "key": key,
            "self": "https://example.atlassian.net/rest/api/3/issue/10001",
            "fields": {
                "summary": "Log in", "description": null,
                "status": {"name": "To Do", "statusCategory": {"key": "new"}},
                "assignee": {"displayName": assignee},
                "reporter": {"displayName": "Some One"},
                "labels": [], "issuetype": {"name": "Story"}, "priority": {"name": "High"},
                "updated": "2026-10-01T10:00:00.000+0000",
                "comment": {"comments": [], "total": 0}
            }
        })
    };
    // The user filed both: ABC-1 is Noor's, ABC-2 the user's own.
    let (reported, theirs) = (
        work_item("ABC-1", "Noor Haddad"),
        work_item("ABC-2", "Some One"),
    );
    user.acli_script(json!({
        "items": {"ABC-1": reported, "ABC-2": theirs},
        "search": [reported, theirs],
        "mine_search": [reported, theirs],
        "assigned_search": [theirs]
    }));
    let (id, mcp) = agent(&user);
    stored(&user, "ticket", "ABC-1");

    subscribe(
        &mcp,
        "tickets",
        json!({"ticket": "ABC-1", "events": ["status"]}),
    );
    let blocked = blocked(&user, &id);
    assert_eq!(blocked["by"], json!(["Noor Haddad"]), "{blocked}");
    assert_eq!(blocked["why"], "ticket ABC-1", "{blocked}");

    // The one assigned to the user blocks nothing.
    let answer = mcp.call(
        "tickets",
        json!({"action": "unsubscribe", "ticket": "ABC-1"}),
    );
    assert!(!answer.error, "{}", answer.text);
    subscribe(
        &mcp,
        "tickets",
        json!({"ticket": "ABC-2", "events": ["status"]}),
    );
    eventually("no longer blocked", || {
        listed(&user, &id)["blocked"].is_null()
    });
    std::thread::sleep(Duration::from_millis(500));
    let line = listed(&user, &id);
    assert!(line["blocked"].is_null(), "{line}");
}

#[test]
fn no_account_id_or_e_mail_is_in_the_blocked_state() {
    let user = user();
    fixture(&user);
    // A person who set no name is said by their login; a name that is an e-mail is not said.
    let requested = json!([
        {"__typename": "User", "login": "maya", "name": "", "id": "MDQ6VXNlcjEyMzQ1",
            "email": "maya@example.com"},
        {"__typename": "User", "login": "noor", "name": "noor@example.com",
            "id": "MDQ6VXNlcjY3ODk="}
    ]);
    script(&user, pull(requested), issue(json!([])));
    let (id, mcp) = agent(&user);
    subscribe(&mcp, "forge", json!({"events": ["review"]}));
    let github = blocked(&user, &id);
    assert_eq!(github["by"], json!(["maya", "noor"]), "{github}");

    // A Jira work item assigned to someone, as `acli` prints its assignee.
    track(
        &user,
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}),
    );
    let item = json!({
        "key": "ABC-1",
        "self": "https://example.atlassian.net/rest/api/3/issue/10001",
        "fields": {
            "summary": "Log in", "description": null,
            "status": {"name": "To Do", "statusCategory": {"key": "new"}},
            "assignee": {"displayName": "Noor Haddad", "accountId": "5b10ac8d82e05b22cc7d4ef5",
                "emailAddress": "noor@example.com"},
            "reporter": {"displayName": "Someone"},
            "labels": [], "issuetype": {"name": "Story"}, "priority": {"name": "High"},
            "updated": "2026-10-01T10:00:00.000+0000",
            "comment": {"comments": [], "total": 0}
        }
    });
    user.acli_script(json!({"items": {"ABC-1": item}, "search": [item], "mine_search": []}));
    a_round(&user);
    let answer = mcp.call("forge", json!({"action": "unsubscribe"}));
    assert!(!answer.error, "{}", answer.text);
    subscribe(
        &mcp,
        "tickets",
        json!({"ticket": "ABC-1", "events": ["commented"]}),
    );
    eventually("blocked by the work item's assignee", || {
        listed(&user, &id)["blocked"]["why"] == "ticket ABC-1"
    });
    let jira = listed(&user, &id)["blocked"].clone();
    assert_eq!(jira["by"], json!(["Noor Haddad"]), "{jira}");

    for blocked in [github, jira] {
        let said = blocked.to_string();
        for never in ["MDQ6", "5b10ac8d", "@"] {
            assert!(!said.contains(never), "{never} in {said}");
        }
    }
}
