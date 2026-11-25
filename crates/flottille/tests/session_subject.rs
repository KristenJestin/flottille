//! The scenarios of a session's subject (`docs/design.md` §10, "What a session is about"): the
//! daemon derives what a session works on from what it took in hand, its workspace's ticket, a
//! pull request it opened, a ticket it changed, and gives it with the session's line
//! (`agent.list`, the tree), a change of it being `agent.updated`. Read through the fake `gh`, in
//! throwaway repositories; nothing is asked of a real forge.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

fn user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// A project `app` of one repository, `api` on GitHub `acme/api`, its issues the tracker, and its
/// ready workspace `login` on the branch `app/login`.
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
    user.git(
        &dir,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}],
            "tracker": {"kind": "github", "repository": "api"}}),
    )
    .unwrap();
}

/// An issue of `acme/api` as `gh` says it, `state` `OPEN` or `CLOSED`.
fn issue(number: u64, title: &str, state: &str) -> Value {
    json!({
        "number": number,
        "title": title,
        "state": state,
        "body": "What it is.",
        "url": format!("https://github.com/acme/api/issues/{number}"),
        "createdAt": "2026-10-01T10:00:00Z",
        "updatedAt": changed(),
        "comments": []
    })
}

/// The pull request #12 of `app/login`, `state` `OPEN` or `MERGED`.
fn pull(state: &str) -> Value {
    json!({
        "number": 12,
        "title": "Add the login page",
        "state": state,
        "isDraft": false,
        "headRefName": "app/login",
        "headRefOid": "aaaaaaa1",
        "baseRefName": "main",
        "url": "https://github.com/acme/api/pull/12",
        "statusCheckRollup": [],
        "comments": [],
        "reviews": [],
        "updatedAt": changed()
    })
}

/// Later than any time before it: each script is a change GitHub says by a newer `updatedAt`.
fn changed() -> String {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("2999-01-01T{:02}:{:02}:00Z", n / 60 % 24, n % 60)
}

/// The fake `gh` answers `issues` (by number) and, when given, the pull request of `app/login`.
fn script(user: &User, issues: &[Value], pull: Option<Value>) {
    let issues: serde_json::Map<String, Value> = issues
        .iter()
        .map(|issue| (issue["number"].to_string(), issue.clone()))
        .collect();
    let mut script = json!({"issues": {"acme/api": issues}, "pulls": {}});
    if let Some(pull) = pull {
        script["pulls"] = json!({"acme/api": {"app/login": pull}});
        script["created"] = 12.into();
    }
    user.gh_script(script);
}

/// A round of the project's items, as the daemon makes every few minutes.
fn a_round(user: &User) {
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

fn link(user: &User, ticket: &str) {
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "login", "ticket": ticket}),
    )
    .unwrap();
}

/// An agent started in the workspace `login`, in Full (its writes are not held), its first turn
/// over: its id and its MCP session.
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
    eventually("its first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    (id, Mcp::initialize(&url, &authorization))
}

/// The agent's subject, as `agent.list` gives it.
fn subject(user: &User, agent: &str) -> Value {
    let listed = user.call("agent.list", json!({})).unwrap();
    listed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == agent)
        .unwrap()["subject"]
        .clone()
}

/// The subject once `check` holds of it.
fn subject_once(user: &User, agent: &str, check: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let subject = subject(user, agent);
        if check(&subject) {
            return subject;
        }
        assert!(
            Instant::now() < deadline,
            "the subject never came; last {subject}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn tickets(mcp: &Mcp, args: Value) {
    let answer = mcp.call("tickets", args);
    assert!(!answer.error, "{}", answer.text);
}

#[test]
fn a_session_in_a_workspace_linked_to_a_ticket_is_about_that_ticket() {
    let user = user();
    fixture(&user);
    script(&user, &[issue(5, "The login fails", "OPEN")], None);
    a_round(&user);
    link(&user, "#5");
    let (id, _mcp) = agent(&user);

    let subject = subject(&user, &id);
    assert_eq!(subject["kind"], "ticket", "{subject}");
    assert_eq!(subject["key"], "#5");
    assert_eq!(subject["title"], "The login fails");
    assert_eq!(subject["from"], "the workspace's linked ticket");
    // The tree says the same.
    let trees = user.call("agent.trees", json!({})).unwrap();
    assert_eq!(trees["trees"][0]["agents"][0]["agent"]["subject"], subject);
}

#[test]
fn a_session_that_opens_a_pull_request_is_about_it_when_it_has_no_ticket() {
    let user = user();
    fixture(&user);
    script(&user, &[], None);
    a_round(&user);
    let (id, mcp) = agent(&user);
    assert_eq!(subject(&user, &id), Value::Null, "nothing in hand yet");
    let worktree = user.path("data/flottille/workspaces/app/login/api");
    user.git(
        &worktree,
        &["update-ref", "refs/remotes/origin/app/login", "HEAD"],
    );

    script(&user, &[], Some(pull("OPEN")));
    let answer = mcp.call(
        "forge",
        json!({"action": "create", "title": "Add the login page", "body": "Adds it"}),
    );
    assert!(!answer.error, "{}", answer.text);
    a_round(&user);

    let subject = subject_once(&user, &id, |s| s["kind"] == "pull_request");
    assert_eq!(subject["key"], "#12", "{subject}");
    assert_eq!(subject["title"], "Add the login page");
    assert_eq!(subject["from"], "a pull request it opened");
    assert_eq!(subject["pull_request"]["key"], "#12");
}

#[test]
fn a_session_that_changes_another_ticket_follows_it() {
    let user = user();
    fixture(&user);
    let both = [
        issue(5, "The login fails", "OPEN"),
        issue(6, "The logout hangs", "OPEN"),
    ];
    script(&user, &both, None);
    a_round(&user);
    link(&user, "#5");
    let (id, mcp) = agent(&user);
    assert_eq!(subject(&user, &id)["key"], "#5");

    tickets(
        &mcp,
        json!({"action": "comment", "ticket": "#6", "body": "Taking it"}),
    );

    let subject = subject(&user, &id);
    assert_eq!(subject["kind"], "ticket", "{subject}");
    assert_eq!(subject["key"], "#6");
    assert_eq!(subject["title"], "The logout hangs");
    assert_eq!(subject["from"], "a ticket it changed");
}

#[test]
fn reading_a_ticket_does_not_move_the_subject_of_a_session_that_holds_one() {
    let user = user();
    fixture(&user);
    let both = [
        issue(5, "The login fails", "OPEN"),
        issue(6, "The logout hangs", "OPEN"),
    ];
    script(&user, &both, None);
    a_round(&user);
    link(&user, "#5");
    let (id, mcp) = agent(&user);

    tickets(&mcp, json!({"action": "show", "ticket": "#6"}));
    tickets(&mcp, json!({"action": "search", "query": "logout"}));

    let subject = subject(&user, &id);
    assert_eq!(subject["key"], "#5", "{subject}");
    assert_eq!(subject["from"], "the workspace's linked ticket");
}

#[test]
fn a_session_whose_ticket_closes_is_free_and_says_what_it_last_held() {
    let user = user();
    fixture(&user);
    script(&user, &[issue(5, "The login fails", "OPEN")], None);
    a_round(&user);
    link(&user, "#5");
    let (id, mcp) = agent(&user);
    let worktree = user.path("data/flottille/workspaces/app/login/api");
    user.git(
        &worktree,
        &["update-ref", "refs/remotes/origin/app/login", "HEAD"],
    );
    script(
        &user,
        &[issue(5, "The login fails", "OPEN")],
        Some(pull("OPEN")),
    );
    let answer = mcp.call(
        "forge",
        json!({"action": "create", "title": "Add the login page", "body": "Adds it"}),
    );
    assert!(!answer.error, "{}", answer.text);
    a_round(&user);
    // A pull request for its ticket: the ticket stays the subject, the pull request beside it.
    let subject = subject_once(&user, &id, |s| s["pull_request"]["key"] == "#12");
    assert_eq!(subject["key"], "#5", "{subject}");

    script(
        &user,
        &[issue(5, "The login fails", "CLOSED")],
        Some(pull("MERGED")),
    );
    a_round(&user);

    let subject = subject_once(&user, &id, |s| s["kind"] == "free");
    assert_eq!(subject["key"], Value::Null, "{subject}");
    assert_eq!(subject["last"], "#5 done · #12 merged");
}

#[test]
fn a_change_of_subject_is_an_event() {
    let user = user();
    fixture(&user);
    let both = [
        issue(5, "The login fails", "OPEN"),
        issue(6, "The logout hangs", "OPEN"),
    ];
    script(&user, &both, None);
    a_round(&user);
    let (id, mcp) = agent(&user);

    tickets(
        &mcp,
        json!({"action": "edit", "ticket": "#6", "labels": ["bug"]}),
    );

    let updated = |user: &User| -> Vec<Value> {
        let shown = user.call("agent.show", json!({"agent": id})).unwrap();
        shown["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["name"] == "agent.updated")
            .cloned()
            .collect()
    };
    eventually("agent.updated", || !updated(&user).is_empty());
    let events = updated(&user);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["data"]["agent"], id.as_str());
    assert_eq!(events[0]["data"]["subject"]["key"], "#6");
    assert_eq!(events[0]["data"]["subject"]["kind"], "ticket");

    // Read again, the same subject is no new event.
    tickets(
        &mcp,
        json!({"action": "comment", "ticket": "#6", "body": "Again"}),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(updated(&user).len(), 1);
}

#[test]
fn a_session_holding_a_ticket_before_the_first_round_shows_its_title_after_it() {
    let user = user();
    fixture(&user);
    link(&user, "#5");
    let (id, _mcp) = agent(&user);
    let subject = subject(&user, &id);
    assert_eq!(subject["key"], "#5", "{subject}");
    assert_eq!(subject["title"], Value::Null, "the tracker was never read");

    // The tracker's first round stores its items and tells no event of them.
    script(&user, &[issue(5, "The login fails", "OPEN")], None);
    a_round(&user);

    let subject = subject_once(&user, &id, |s| s["title"] == "The login fails");
    assert_eq!(subject["kind"], "ticket", "{subject}");
    assert_eq!(subject["key"], "#5");
}

#[test]
fn an_unset_forge_drops_the_pull_request_from_the_subject() {
    let user = user();
    fixture(&user);
    script(
        &user,
        &[issue(5, "The login fails", "OPEN")],
        Some(pull("OPEN")),
    );
    a_round(&user);
    link(&user, "#5");
    let (id, _mcp) = agent(&user);
    let subject = subject_once(&user, &id, |s| s["pull_request"]["key"] == "#12");
    assert_eq!(subject["key"], "#5", "{subject}");

    // The repository's forge unset: its pull requests are forgotten (`items.forgotten`).
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": "api"}], "tracker": null}),
    )
    .unwrap();

    let subject = subject_once(&user, &id, |s| s["pull_request"].is_null());
    assert_eq!(subject["key"], "#5", "{subject}");
}
