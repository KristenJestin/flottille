//! The complete families of the agents' tools (#141, `docs/tools.md`, "The forge", "The tickets",
//! "Sessions for the user"): every read of `forge` and `tickets` answers at once, every write is
//! held for the user and done once they allow it, `tickets list` gives every open ticket in one
//! call, Jira is written as GitHub is, and a root starts a session for the user that is the
//! user's. The forge and the trackers are the fake `gh`, `bkt` and `acli`; the remotes are
//! throwaway bare repositories; the escalating programs are fakes, first on the daemon's `PATH`,
//! and none runs.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::agent::GateMode;
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` of one repository, its `origin` GitHub's `acme/app` (set before the workspace
/// is made, which is made offline: nothing is fetched from it), its forge declared, its tracker
/// `tracker`, and its ready workspace `login` on the branch `app/login`.
fn fixture(user: &User, origin: &str, forge: Value, tracker: Value) -> PathBuf {
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
    user.git(&folder, &["remote", "set-url", "origin", origin]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": ".", "forge": forge}],
            "tracker": tracker}),
    )
    .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    folder
}

/// The project on GitHub, its issues its tracker.
fn on_github(user: &User) -> PathBuf {
    fixture(
        user,
        "git@github.com:acme/app.git",
        json!({"kind": "github", "repository": "acme/app"}),
        json!({"kind": "github", "repository": "."}),
    )
}

/// The project on GitHub, Jira on the site the fake `acli` is signed in to its tracker.
fn on_jira(user: &User) -> PathBuf {
    fixture(
        user,
        "git@github.com:acme/app.git",
        json!({"kind": "github", "repository": "acme/app"}),
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC", "DEF"]}),
    )
}

/// The project on Bitbucket Cloud, with no tracker.
fn on_bitbucket(user: &User) -> PathBuf {
    fixture(
        user,
        "git@bitbucket.org:acme/app.git",
        json!({"kind": "bitbucket-cloud", "repository": "acme/app"}),
        Value::Null,
    )
}

fn user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_fake_bkt()
        .with_fake_acli()
        .with_fake_escalation()
}

/// A root started by the user in the workspace (in `main` for none): its id and its MCP session.
fn root_in(user: &User, workspace: Option<&str>) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": workspace, "message": "hello"}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        started["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A read: answered at once, nothing held for the user.
fn read(user: &User, answer: Answer) -> String {
    assert!(!answer.error, "{}", answer.text);
    assert!(!answer.text.contains("Waiting for"), "{}", answer.text);
    assert!(no_request(user));
    answer.text
}

fn no_request(user: &User) -> bool {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .is_empty()
}

/// A call held for the user: its request's number.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The user allows request `number`: the text the agent is handed for it.
fn allow(user: &User, number: u64) -> String {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

/// A write: held, nothing asked of `calls` before the user allows it, then done. What the agent
/// is handed.
fn judged_then_done(
    user: &User,
    answer: Answer,
    calls: impl Fn() -> Vec<String>,
    verb: &str,
) -> String {
    let number = held(&answer);
    assert!(
        !calls().iter().any(|call| call.starts_with(verb)),
        "{verb} before the user allowed it: {:?}",
        calls()
    );
    let done = allow(user, number);
    assert!(done.contains("and Flottille has now done it"), "{done}");
    done
}

fn words(received: Vec<Value>) -> Vec<String> {
    received
        .iter()
        .map(|call| {
            call["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn pulls() -> Value {
    json!({"pulls": {"acme/app": {
        "app/login": {"number": 12, "title": "Add the login page", "state": "OPEN",
            "isDraft": false, "baseRefName": "main",
            "url": "https://github.com/acme/app/pull/12", "reviewDecision": "APPROVED",
            "statusCheckRollup": [{"__typename": "CheckRun", "name": "build",
                "status": "COMPLETED", "conclusion": "SUCCESS"}],
            "comments": [{"author": {"login": "alice"}, "body": "Nice"}], "reviews": []},
        "app/old": {"number": 7, "title": "Old work", "state": "MERGED", "isDraft": false,
            "baseRefName": "main", "url": "https://github.com/acme/app/pull/7"}}}})
}

// The forge.

#[test]
fn forge_list_lists_the_pull_requests_by_state_and_branch() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls());
    let (_, mcp) = root_in(&user, None);

    let open = read(&user, mcp.call("forge", json!({"action": "list"})));
    assert!(
        open.contains("- #12 open, app/login into main: Add the login page"),
        "{open}"
    );
    assert!(!open.contains("#7"), "{open}");
    let merged = read(
        &user,
        mcp.call("forge", json!({"action": "list", "state": "merged"})),
    );
    assert!(
        merged.contains("- #7 merged, app/old into main"),
        "{merged}"
    );
    let branch = read(
        &user,
        mcp.call(
            "forge",
            json!({"action": "list", "state": "all", "branch": "app/old"}),
        ),
    );
    assert!(branch.contains("#7") && !branch.contains("#12"), "{branch}");
    let calls = words(user.gh_received());
    assert!(
        calls.contains(
            &"pr list -R github.com/acme/app --state merged --limit 100 --json \
              number,title,state,isDraft,headRefName,baseRefName,url"
                .to_owned()
        ),
        "{calls:?}"
    );
    let refused = mcp.call(
        "forge",
        json!({"action": "list", "repository": "elsewhere"}),
    );
    assert!(
        refused.error
            && refused
                .text
                .contains("not a repository of this agent's place")
    );
    assert!(user.escalations().is_empty());
}

#[test]
fn forge_show_reads_a_pull_request_by_its_number_or_the_workspace_s_branch() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls());
    let (_, main) = root_in(&user, None);
    let (_, inside) = root_in(&user, Some("login"));

    let numbered = read(
        &user,
        main.call("forge", json!({"action": "show", "number": 7})),
    );
    assert!(
        numbered.contains(". (acme/app): pull request #7, merged, app/old into main: Old work"),
        "{numbered}"
    );
    let own = read(&user, inside.call("forge", json!({"action": "show"})));
    assert!(own.contains("pull request #12, open"), "{own}");
    assert!(own.contains("checks: passing (build passing)"), "{own}");
    let full = read(
        &user,
        inside.call(
            "forge",
            json!({"action": "show", "number": 12, "detail": "full"}),
        ),
    );
    assert!(full.contains("- alice: Nice"), "{full}");
    let missing = main.call("forge", json!({"action": "show", "number": 99}));
    assert!(missing.error, "{}", missing.text);
}

#[test]
fn forge_comment_is_judged_and_posted_once_allowed() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls());
    let (_, mcp) = root_in(&user, None);

    let answer = mcp.call(
        "forge",
        json!({"action": "comment", "number": 12, "body": "Rebased on main"}),
    );
    let done = judged_then_done(&user, answer, || words(user.gh_received()), "pr comment");

    assert!(
        done.contains("The comment was posted on pull request #12 of acme/app."),
        "{done}"
    );
    assert!(
        words(user.gh_received())
            .contains(&"pr comment 12 -R github.com/acme/app --body Rebased on main".to_owned())
    );
    // Refused before anything is asked: no body, no number.
    for args in [
        json!({"action": "comment", "number": 12}),
        json!({"action": "comment", "body": "x"}),
    ] {
        let refused = mcp.call("forge", args);
        assert!(
            refused.error || refused.text.contains("Waiting"),
            "{}",
            refused.text
        );
    }
    assert!(user.escalations().is_empty());
}

#[test]
fn forge_edit_is_judged_and_changes_the_pull_request_once_allowed() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls());
    let (_, mcp) = root_in(&user, Some("login"));

    let answer = mcp.call(
        "forge",
        json!({"action": "edit", "number": 12, "title": "feat: add the login page"}),
    );
    let done = judged_then_done(&user, answer, || words(user.gh_received()), "pr edit");

    assert!(
        done.contains("Pull request #12 of acme/app was changed."),
        "{done}"
    );
    assert!(words(user.gh_received()).contains(
        &"pr edit 12 -R github.com/acme/app --title feat: add the login page".to_owned()
    ));
}

/// `pulls()` with an open pull request of a branch no workspace is on (#15), and a closed one (#9).
fn pulls_to_close() -> Value {
    let mut script = pulls();
    let held = &mut script["pulls"]["acme/app"];
    held["app/gone"] = json!({"number": 15, "title": "Gone work", "state": "OPEN",
        "isDraft": false, "baseRefName": "main", "headRefName": "app/gone",
        "isCrossRepository": false, "url": "https://github.com/acme/app/pull/15"});
    held["app/closed"] = json!({"number": 9, "title": "Dropped work", "state": "CLOSED",
        "isDraft": false, "baseRefName": "main", "headRefName": "app/closed",
        "isCrossRepository": false, "url": "https://github.com/acme/app/pull/9"});
    script
}

/// What the user is told once they allowed a call the forge then refused.
fn allowed_then_refused(user: &User, answer: Answer) -> String {
    let body = allow(user, held(&answer));
    assert!(body.contains("refused:"), "{body}");
    body
}

#[test]
fn forge_close_closes_a_github_pull_request_with_a_comment_once_allowed() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls_to_close());
    let (_, mcp) = root_in(&user, None);

    let answer = mcp.call(
        "forge",
        json!({"action": "close", "number": 12, "comment": "Superseded by #20"}),
    );
    let done = judged_then_done(&user, answer, || words(user.gh_received()), "pr close");

    assert!(
        done.contains("The pull request #12 of acme/app was closed."),
        "{done}"
    );
    let calls = words(user.gh_received());
    assert!(
        calls
            .contains(&"pr close 12 -R github.com/acme/app --comment Superseded by #20".to_owned())
    );
    // Its state was read first, and the branch is left to the workspace that holds it.
    assert!(calls.iter().any(|call| call.starts_with("pr view 12 ")));
    assert!(user.escalations().is_empty());
}

#[test]
fn forge_close_declines_a_bitbucket_pull_request_with_a_comment() {
    let user = user();
    on_bitbucket(&user);
    user.bkt_script(
        json!({"pulls": {"acme/app": [{"id": 3, "title": "Work", "state": "OPEN",
        "source": {"branch": {"name": "app/gone"}, "repository": {"full_name": "acme/app"}},
        "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}},
        "links": {"html": {"href": "https://bitbucket.org/acme/app/pull-requests/3"}}}]}}),
    );
    let (_, mcp) = root_in(&user, None);

    let answer = mcp.call(
        "forge",
        json!({"action": "close", "number": 3, "comment": "Superseded"}),
    );
    let done = judged_then_done(&user, answer, || words(user.bkt_received()), "pr decline");

    assert!(
        done.contains("The pull request #3 of acme/app was declined."),
        "{done}"
    );
    assert!(words(user.bkt_received()).contains(
        &"pr decline 3 --context cloud --workspace acme --repo app --comment Superseded".to_owned()
    ));
}

#[test]
fn forge_close_deletes_the_branch_of_a_pull_request_no_workspace_is_on() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls_to_close());
    let (_, mcp) = root_in(&user, None);

    let answer = mcp.call(
        "forge",
        json!({"action": "close", "number": 15, "delete_branch": true}),
    );
    let done = judged_then_done(&user, answer, || words(user.gh_received()), "pr close");

    assert!(
        done.contains("and its branch app/gone was deleted"),
        "{done}"
    );
    assert!(
        words(user.gh_received())
            .contains(&"pr close 15 -R github.com/acme/app --delete-branch".to_owned())
    );
}

#[test]
fn forge_close_leaves_the_branch_a_workspace_is_still_on() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls_to_close());
    let (_, mcp) = root_in(&user, None);

    let answer = mcp.call(
        "forge",
        json!({"action": "close", "number": 12, "delete_branch": true}),
    );
    let done = judged_then_done(&user, answer, || words(user.gh_received()), "pr close");

    assert!(
        done.contains("its branch app/login was left: the workspace login is on it"),
        "{done}"
    );
    assert!(words(user.gh_received()).contains(&"pr close 12 -R github.com/acme/app".to_owned()));
}

#[test]
fn forge_close_of_a_merged_or_closed_pull_request_is_refused_with_its_state() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls_to_close());
    let (_, mcp) = root_in(&user, None);

    let merged = allowed_then_refused(
        &user,
        mcp.call("forge", json!({"action": "close", "number": 7})),
    );
    assert!(
        merged.contains("the pull request #7 of acme/app is merged, not open"),
        "{merged}"
    );
    let closed = allowed_then_refused(
        &user,
        mcp.call("forge", json!({"action": "close", "number": 9})),
    );
    assert!(
        closed.contains("the pull request #9 of acme/app is closed, not open"),
        "{closed}"
    );
    assert!(
        !words(user.gh_received())
            .iter()
            .any(|call| call.starts_with("pr close"))
    );
}

#[test]
fn forge_reopen_reopens_a_closed_github_pull_request() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls_to_close());
    let (_, mcp) = root_in(&user, None);

    let answer = mcp.call(
        "forge",
        json!({"action": "reopen", "number": 9, "comment": "Needed again"}),
    );
    let done = judged_then_done(&user, answer, || words(user.gh_received()), "pr reopen");

    assert!(
        done.contains("The pull request #9 of acme/app was reopened."),
        "{done}"
    );
    assert!(
        words(user.gh_received())
            .contains(&"pr reopen 9 -R github.com/acme/app --comment Needed again".to_owned())
    );
    // Not one that is open, nor one that was merged.
    let open = allowed_then_refused(
        &user,
        mcp.call("forge", json!({"action": "reopen", "number": 12})),
    );
    assert!(
        open.contains("the pull request #12 of acme/app is open, not closed"),
        "{open}"
    );
    let merged = allowed_then_refused(
        &user,
        mcp.call("forge", json!({"action": "reopen", "number": 7})),
    );
    assert!(
        merged.contains("the pull request #7 of acme/app is merged, not closed"),
        "{merged}"
    );
}

#[test]
fn forge_reopen_on_bitbucket_is_refused_saying_it_cannot() {
    let user = user();
    on_bitbucket(&user);
    user.bkt_script(
        json!({"pulls": {"acme/app": [{"id": 3, "title": "Work", "state": "DECLINED",
        "source": {"branch": {"name": "app/gone"}, "repository": {"full_name": "acme/app"}},
        "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}},
        "links": {"html": {"href": "https://bitbucket.org/acme/app/pull-requests/3"}}}]}}),
    );
    let (_, mcp) = root_in(&user, None);

    let refused = allowed_then_refused(
        &user,
        mcp.call("forge", json!({"action": "reopen", "number": 3})),
    );

    assert!(
        refused.contains("Bitbucket cannot reopen a declined pull request"),
        "{refused}"
    );
    assert!(
        !words(user.bkt_received())
            .iter()
            .any(|call| call.starts_with("pr reopen") || call.starts_with("pr decline"))
    );
}

#[test]
fn forge_close_in_ask_mode_is_held_and_nothing_is_closed_before_the_user_allows_it() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls_to_close());
    let (agent, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": agent, "mode": "ask"}))
        .unwrap();

    let answer = mcp.call("forge", json!({"action": "close", "number": 15}));
    let number = held(&answer);

    let request = user.call("approval.list", json!({})).unwrap()["requests"][0].clone();
    assert!(
        request["reasons"]
            .to_string()
            .contains("it closes a pull request on the forge"),
        "{request}"
    );
    assert!(
        !words(user.gh_received())
            .iter()
            .any(|call| call.starts_with("pr close"))
    );
    allow(&user, number);
    assert!(words(user.gh_received()).contains(&"pr close 15 -R github.com/acme/app".to_owned()));
}

#[test]
fn forge_list_comment_and_edit_go_through_bkt_on_bitbucket() {
    let user = user();
    on_bitbucket(&user);
    let pull = |id: u64, branch: &str, state: &str| {
        json!({"id": id, "title": format!("Work on {branch}"), "state": state,
            "source": {"branch": {"name": branch}, "repository": {"full_name": "acme/app"}},
            "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}},
            "links": {"html": {"href": format!("https://bitbucket.org/acme/app/pull-requests/{id}")}}})
    };
    user.bkt_script(json!({"pulls": {"acme/app": [
        pull(3, "app/login", "OPEN"), pull(2, "app/old", "MERGED")]}}));
    let (_, mcp) = root_in(&user, None);

    let open = read(&user, mcp.call("forge", json!({"action": "list"})));
    assert!(open.contains("- #3 open, app/login into main"), "{open}");
    assert!(!open.contains("#2"), "{open}");
    let calls = || words(user.bkt_received());
    let comment = mcp.call(
        "forge",
        json!({"action": "comment", "number": 3, "body": "Ready"}),
    );
    judged_then_done(&user, comment, calls, "pr comment");
    let edit = mcp.call(
        "forge",
        json!({"action": "edit", "number": 3, "body": "Adds the login page"}),
    );
    judged_then_done(&user, edit, calls, "pr edit");

    let calls = calls();
    assert!(
        calls.contains(
            &"pr list --context cloud --workspace acme --repo app --state OPEN --limit 50 --json"
                .to_owned()
        )
    );
    assert!(calls.contains(
        &"pr comment 3 --context cloud --workspace acme --repo app --text Ready".to_owned()
    ));
    assert!(calls.contains(
        &"pr edit 3 --context cloud --workspace acme --repo app --description Adds the login page"
            .to_owned()
    ));
    assert!(user.escalations().is_empty());
}

// The tickets.

fn issue(number: u64, title: &str, state: &str, labels: &[&str], assignees: &[&str]) -> Value {
    json!({"number": number, "title": title, "state": state, "body": format!("About {title}"),
        "url": format!("https://github.com/acme/app/issues/{number}"),
        "labels": labels.iter().map(|l| json!({"name": l})).collect::<Vec<_>>(),
        "assignees": assignees.iter().map(|a| json!({"login": a})).collect::<Vec<_>>(),
        "comments": []})
}

/// 40 open issues and 5 closed ones; #1 a bug of ana's.
fn many_issues(user: &User) {
    let mut issues = serde_json::Map::new();
    for number in 1..=45u64 {
        let state = if number > 40 { "CLOSED" } else { "OPEN" };
        let (labels, assignees): (&[&str], &[&str]) = match number {
            1 => (&["bug"], &["ana"]),
            2 => (&["bug"], &[]),
            _ => (&[], &[]),
        };
        let title = match number {
            1 => "The login fails".to_owned(),
            n => format!("Ticket {n}"),
        };
        issues.insert(
            number.to_string(),
            issue(number, &title, state, labels, assignees),
        );
    }
    user.gh_script(json!({"issues": {"acme/app": issues}, "created_issue": 46}));
}

fn issue_lists(user: &User) -> Vec<String> {
    words(user.gh_received())
        .into_iter()
        .filter(|call| call.starts_with("issue list"))
        .collect()
}

#[test]
fn tickets_list_returns_every_open_ticket_in_one_call() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    let listed = read(&user, mcp.call("tickets", json!({"action": "list"})));

    assert!(
        listed.starts_with("The open tickets of GitHub issues"),
        "{listed}"
    );
    assert!(listed.contains(": 40\n"), "{listed}");
    for number in 1..=40 {
        assert!(
            listed.contains(&format!("- #{number} open: ")),
            "#{number} in {listed}"
        );
    }
    assert!(!listed.contains("#41"), "{listed}");
    assert!(
        listed.contains(
            "- #1 open: The login fails [bug] (ana)  https://github.com/acme/app/issues/1"
        ),
        "{listed}"
    );
    assert_eq!(
        issue_lists(&user),
        [
            "issue list -R github.com/acme/app --state open --limit 200 --json \
          number,title,state,labels,assignees,url"
        ]
    );
}

#[test]
fn tickets_list_filters_by_state_label_and_assignee() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, Some("login"));

    let bugs = read(
        &user,
        mcp.call("tickets", json!({"action": "list", "label": "bug"})),
    );
    assert!(
        bugs.contains("#1 ") && bugs.contains("#2 ") && !bugs.contains("#3 "),
        "{bugs}"
    );
    let ana = read(
        &user,
        mcp.call("tickets", json!({"action": "list", "assignee": "ana"})),
    );
    assert!(ana.contains("#1 ") && !ana.contains("#2 "), "{ana}");
    let closed = read(
        &user,
        mcp.call("tickets", json!({"action": "list", "state": "closed"})),
    );
    assert!(
        closed.contains(": 5\n") && closed.contains("#45 closed"),
        "{closed}"
    );
    assert!(issue_lists(&user)[0].contains("--label bug"));
    assert!(issue_lists(&user)[1].contains("--assignee ana"));
    let refused = mcp.call("tickets", json!({"action": "list", "state": "done"}));
    assert!(refused.error, "{}", refused.text);
}

#[test]
fn tickets_search_finds_the_tickets_holding_its_words() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    let found = read(
        &user,
        mcp.call("tickets", json!({"action": "search", "query": "login"})),
    );

    assert!(found.contains("matching \"login\": 1"), "{found}");
    assert!(found.contains("- #1 open: The login fails"), "{found}");
    assert!(issue_lists(&user)[0].contains("--search login"));
    let refused = mcp.call("tickets", json!({"action": "search"}));
    assert!(
        refused.error && refused.text.contains("needs query"),
        "{}",
        refused.text
    );
}

#[test]
fn tickets_show_reads_a_ticket_by_its_key() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    let shown = read(
        &user,
        mcp.call("tickets", json!({"action": "show", "ticket": "#1"})),
    );

    assert!(
        shown.starts_with("Ticket #1 of acme/app (GitHub), open: The login fails"),
        "{shown}"
    );
}

/// A write of `tickets` on GitHub, judged then done: what `gh` was asked.
fn ticket_written(args: Value, verb: &str) -> (String, Vec<String>) {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);
    let calls = || words(user.gh_received());
    let done = judged_then_done(&user, mcp.call("tickets", args), calls, verb);
    assert!(user.escalations().is_empty());
    let asked = calls()
        .into_iter()
        .filter(|c| c.starts_with(verb))
        .collect();
    (done, asked)
}

#[test]
fn tickets_create_is_judged_and_opens_an_issue_once_allowed() {
    let (done, asked) = ticket_written(
        json!({"action": "create", "title": "Add a logout", "body": "Users stay signed in",
            "labels": ["feature"]}),
        "issue create",
    );
    assert!(
        done.contains("The ticket was opened in acme/app: https://github.com/acme/app/issues/46"),
        "{done}"
    );
    assert_eq!(
        asked,
        [
            "issue create -R github.com/acme/app --title Add a logout --body Users stay signed in \
          --label feature"
        ]
    );
}

#[test]
fn tickets_comment_is_judged_and_posted_once_allowed() {
    let (done, asked) = ticket_written(
        json!({"action": "comment", "ticket": "#1", "body": "Fixed on app/login"}),
        "issue comment",
    );
    assert!(
        done.contains("The comment was posted on #1 of acme/app."),
        "{done}"
    );
    assert_eq!(
        asked,
        ["issue comment 1 -R github.com/acme/app --body Fixed on app/login"]
    );
}

#[test]
fn tickets_edit_is_judged_and_changes_the_issue_once_allowed() {
    let (done, asked) = ticket_written(
        json!({"action": "edit", "ticket": "#2", "title": "The logout fails"}),
        "issue edit",
    );
    assert!(done.contains("#2 of acme/app was changed."), "{done}");
    assert_eq!(
        asked,
        ["issue edit 2 -R github.com/acme/app --title The logout fails"]
    );
}

#[test]
fn tickets_close_is_judged_and_closes_the_issue_once_allowed() {
    let (done, asked) = ticket_written(json!({"action": "close", "ticket": "#3"}), "issue close");
    assert!(done.contains("#3 of acme/app was closed."), "{done}");
    assert_eq!(asked, ["issue close 3 -R github.com/acme/app"]);
}

#[test]
fn tickets_reopen_is_judged_and_reopens_the_issue_once_allowed() {
    let (done, asked) =
        ticket_written(json!({"action": "reopen", "ticket": "#41"}), "issue reopen");
    assert!(done.contains("#41 of acme/app was reopened."), "{done}");
    assert_eq!(asked, ["issue reopen 41 -R github.com/acme/app"]);
}

/// A write of `tickets` on GitHub whose boards (`projectItems` of #1) and fields are scripted:
/// held, then what the agent is handed once the user allows it, and the calls `gh` received.
fn status_asked(items: Value, args: Value) -> (String, Vec<String>) {
    let user = user();
    on_github(&user);
    let mut issues = serde_json::Map::new();
    let mut ticket = issue(1, "The login fails", "OPEN", &[], &[]);
    ticket["projectItems"] = items;
    issues.insert("1".into(), ticket);
    let status = |options: &[&str]| {
        json!([{"id": "F_t", "name": "Title", "type": "ProjectV2Field"},
            {"id": "F_s", "name": "Status", "type": "ProjectV2SingleSelectField",
             "options": options.iter().enumerate()
                .map(|(at, name)| json!({"id": format!("o{at}"), "name": name}))
                .collect::<Vec<_>>()}])
    };
    user.gh_script(json!({"issues": {"acme/app": issues},
        "projects": {"acme": [
            {"id": "PVT_a", "number": 3, "title": "Roadmap"},
            {"id": "PVT_b", "number": 4, "title": "Bugs"}]},
        "project_fields": {
            "acme/3": status(&["Todo", "In Progress", "Done"]),
            "acme/4": status(&["Triage", "Fixing", "Done"])}}));
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call("tickets", args);
    let number = held(&answer);
    let done = allow(&user, number);
    (done, words(user.gh_received()))
}

fn on_board(item: &str, board: &str, id: &str, status: &str) -> Value {
    json!({"id": item, "project": {"id": id, "title": board},
        "status": {"optionId": "o0", "name": status}})
}

fn edits(calls: &[String]) -> Vec<&String> {
    calls
        .iter()
        .filter(|call| call.starts_with("project item-edit"))
        .collect()
}

#[test]
fn tickets_status_sets_the_status_of_an_issue_on_its_board() {
    let (done, calls) = status_asked(
        json!([on_board("PVTI_1", "Roadmap", "PVT_a", "Todo")]),
        json!({"action": "status", "ticket": "#1", "status": "in progress"}),
    );

    assert!(
        done.contains("#1 of acme/app is now In Progress on Roadmap"),
        "{done}"
    );
    assert_eq!(
        edits(&calls),
        [
            "project item-edit --id PVTI_1 --project-id PVT_a --field-id F_s \
          --single-select-option-id o1"
        ]
    );
    assert!(
        calls
            .iter()
            .any(|call| call == "issue view 1 -R github.com/acme/app --json projectItems"),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("project list --owner acme")),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("project field-list 3 --owner acme")),
        "{calls:?}"
    );
}

#[test]
fn tickets_status_that_is_no_option_of_the_board_is_refused_with_the_options() {
    let (done, calls) = status_asked(
        json!([on_board("PVTI_1", "Roadmap", "PVT_a", "Todo")]),
        json!({"action": "status", "ticket": "#1", "status": "Blocked"}),
    );

    assert!(
        done.contains("Blocked is no status of Roadmap: its statuses are Todo, In Progress, Done"),
        "{done}"
    );
    assert!(edits(&calls).is_empty(), "{calls:?}");
}

#[test]
fn tickets_status_of_an_issue_on_no_board_says_it_has_no_status_to_set() {
    let (done, calls) = status_asked(
        json!([]),
        json!({"action": "status", "ticket": "#1", "status": "Done"}),
    );

    assert!(
        done.contains("#1 of acme/app is on no project board: it has no status to set"),
        "{done}"
    );
    assert!(edits(&calls).is_empty(), "{calls:?}");
}

#[test]
fn tickets_status_of_an_issue_on_two_boards_without_project_lists_the_boards() {
    let items = json!([
        on_board("PVTI_1", "Roadmap", "PVT_a", "Todo"),
        on_board("PVTI_9", "Bugs", "PVT_b", "Triage")
    ]);

    let (done, calls) = status_asked(
        items.clone(),
        json!({"action": "status", "ticket": "#1", "status": "Done"}),
    );
    assert!(
        done.contains(
            "#1 of acme/app is on 2 project boards: Roadmap, Bugs: name one with project"
        ),
        "{done}"
    );
    assert!(edits(&calls).is_empty(), "{calls:?}");

    let (done, calls) = status_asked(
        items,
        json!({"action": "status", "ticket": "#1", "status": "Fixing", "project": "bugs"}),
    );
    assert!(
        done.contains("#1 of acme/app is now Fixing on Bugs"),
        "{done}"
    );
    assert_eq!(
        edits(&calls),
        [
            "project item-edit --id PVTI_9 --project-id PVT_b --field-id F_s \
          --single-select-option-id o1"
        ]
    );
}

#[test]
fn tickets_status_with_a_board_the_issue_is_not_on_lists_its_boards() {
    let (done, calls) = status_asked(
        json!([on_board("PVTI_1", "Roadmap", "PVT_a", "Todo")]),
        json!({"action": "status", "ticket": "#1", "status": "Done", "project": "Nowhere"}),
    );

    assert!(
        done.contains(
            "#1 of acme/app is not on a project board named Nowhere: its boards are Roadmap"
        ),
        "{done}"
    );
    assert!(edits(&calls).is_empty(), "{calls:?}");
}

#[test]
fn tickets_status_without_the_project_scope_says_how_to_add_it() {
    let user = user();
    on_github(&user);
    user.gh_script(json!({"missing_scope": true,
        "issues": {"acme/app": {"1": issue(1, "The login fails", "OPEN", &[], &[])}}}));
    let (_, mcp) = root_in(&user, None);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "status", "ticket": "#1", "status": "Done"}),
    ));
    let done = allow(&user, number);

    assert!(done.contains("nothing was done"), "{done}");
    assert!(
        done.contains("gh has no project scope: run `gh auth refresh -s project` yourself"),
        "{done}"
    );
    assert!(
        words(user.gh_received())
            .iter()
            .all(|call| !call.starts_with("auth refresh") && !call.starts_with("project item-edit")),
        "{:?}",
        user.gh_received()
    );
}

#[test]
fn tickets_close_as_not_planned_is_judged_and_closes_for_that_reason() {
    let (done, asked) = ticket_written(
        json!({"action": "close", "ticket": "#3", "reason": "not planned"}),
        "issue close",
    );

    assert!(
        done.contains("#3 of acme/app was closed as not planned."),
        "{done}"
    );
    assert_eq!(
        asked,
        ["issue close 3 -R github.com/acme/app --reason not planned"]
    );
}

#[test]
fn tickets_edit_adds_and_removes_labels_and_assignees() {
    let (done, asked) = ticket_written(
        json!({"action": "edit", "ticket": "#2", "labels": ["bug", "p1"],
            "remove_labels": ["wontfix"], "assignees": ["ana"],
            "remove_assignees": ["bob"]}),
        "issue edit",
    );

    assert!(done.contains("#2 of acme/app was changed."), "{done}");
    assert_eq!(
        asked,
        [
            "issue edit 2 -R github.com/acme/app --add-label bug --add-label p1 --remove-label \
          wontfix --add-assignee ana --remove-assignee bob"
        ]
    );
}

#[test]
fn tickets_arguments_of_another_action_are_refused_before_anything_is_asked() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    for (args, refusal) in [
        (
            json!({"action": "edit", "ticket": "#1", "reason": "completed", "title": "x"}),
            "refused: tickets edit takes no reason: reason is for close",
        ),
        (
            json!({"action": "close", "ticket": "#1", "remove_labels": ["bug"]}),
            "refused: tickets close takes no remove_labels: remove_labels is for edit",
        ),
        (
            json!({"action": "status", "ticket": "#1"}),
            "refused: tickets status needs status, the name of the status to move to",
        ),
        (
            json!({"action": "close", "ticket": "#1", "reason": "duplicate"}),
            "refused: tickets close has no reason \"duplicate\": its reasons are completed and \
             not planned",
        ),
    ] {
        let refused = mcp.call("tickets", args.clone());
        assert!(refused.error, "{args}: {}", refused.text);
        assert!(
            refused.text.starts_with(refusal),
            "{args}: {}",
            refused.text
        );
    }
    assert!(no_request(&user));
    assert!(user.gh_received().is_empty(), "{:?}", user.gh_received());
}

#[test]
fn tickets_arguments_that_are_github_s_are_refused_on_jira_before_anything_is_asked() {
    let user = user();
    on_jira(&user);
    let (_, mcp) = root_in(&user, None);

    for (args, key) in [
        (
            json!({"action": "close", "ticket": "ABC-1", "reason": "completed"}),
            "reason",
        ),
        (
            json!({"action": "edit", "ticket": "ABC-1", "labels": ["bug"]}),
            "labels",
        ),
        (
            json!({"action": "edit", "ticket": "ABC-1", "assignees": ["ana"]}),
            "assignees",
        ),
    ] {
        let refused = mcp.call("tickets", args.clone());
        assert!(refused.error, "{args}: {}", refused.text);
        assert!(
            refused.text.starts_with(&format!(
                "refused: tickets {} on Jira takes no {key}",
                args["action"].as_str().unwrap()
            )),
            "{args}: {}",
            refused.text
        );
    }
    assert!(no_request(&user));
    assert!(user.acli_received().is_empty());
}

#[test]
fn tickets_edit_that_changes_nothing_is_refused_once_allowed_and_asks_nothing_of_gh() {
    for (args, refusal) in [
        (
            json!({"action": "edit", "ticket": "#1"}),
            "refused: tickets edit needs title, body, labels or assignees: what changes",
        ),
        (
            json!({"action": "edit", "ticket": "#1", "labels": [" "]}),
            "refused: tickets edit needs title, body, labels or assignees: what changes",
        ),
    ] {
        let user = user();
        on_github(&user);
        many_issues(&user);
        let (_, mcp) = root_in(&user, None);

        let number = held(&mcp.call("tickets", args.clone()));
        let done = allow(&user, number);

        assert!(done.contains(refusal), "{args}: {done}");
        assert!(
            words(user.gh_received())
                .iter()
                .all(|call| !call.starts_with("issue edit") && !call.starts_with("project")),
            "{:?}",
            user.gh_received()
        );
    }
}

/// A work item of Jira, from `site`.
fn work_item(key: &str, summary: &str, site: &str) -> Value {
    json!({"key": key, "self": format!("https://{site}/rest/api/3/issue/1"),
        "fields": {"summary": summary, "status": {"name": "To Do"},
            "assignee": {"displayName": "Ana"}, "labels": ["bug"]}})
}

#[test]
fn tickets_list_on_jira_is_one_search_of_the_tracker_s_projects() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"search": [
        work_item("ABC-2", "The login fails", "example.atlassian.net"),
        work_item("DEF-1", "Add search", "example.atlassian.net"),
        work_item("XYZ-9", "Elsewhere", "other.atlassian.net"),
    ]}));
    let (_, mcp) = root_in(&user, None);

    let listed = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "search", "query": "say \"hi\"", "label": "bug"}),
        ),
    );

    assert!(
        listed.contains(
            "- ABC-2 To Do: The login fails [bug] (Ana)  \
             https://example.atlassian.net/browse/ABC-2"
        ),
        "{listed}"
    );
    assert!(listed.contains("- DEF-1 To Do: Add search"), "{listed}");
    assert!(!listed.contains("XYZ-9"), "{listed}");
    assert!(listed.contains("1 answered from another site"), "{listed}");
    let searches: Vec<Value> = user
        .acli_received()
        .into_iter()
        .filter(|call| call["args"][2] == "search")
        .collect();
    assert_eq!(searches.len(), 1);
    assert_eq!(
        searches[0]["args"],
        json!([
            "jira",
            "workitem",
            "search",
            "--jql",
            "project in (\"ABC\", \"DEF\") AND statusCategory != Done AND labels = \"bug\" AND \
             text ~ \"say \\\"hi\\\"\" ORDER BY created DESC",
            "--fields",
            "key,summary,status,assignee,labels",
            "--json",
            "--limit",
            "200"
        ])
    );
}

#[test]
fn tickets_list_keeps_the_items_answered_from_an_atlassian_host() {
    let user = user();
    on_jira(&user);
    // With an OAuth sign-in, `acli` answers `self` links on Atlassian hosts, not on the site.
    let mut paas = work_item("ABC-2", "The login fails", "example.atlassian.net");
    paas["self"] = json!("https://jira-prod-xx-1-1.prod.atl-paas.net/rest/api/3/issue/2");
    let mut gateway = work_item("DEF-1", "Add search", "example.atlassian.net");
    gateway["self"] = json!(
        "https://api.atlassian.com/ex/jira/0f1e2d3c-aaaa-4bbb-8ccc-123456789abc/rest/api/3/issue/3"
    );
    user.acli_script(json!({"search": [
        paas,
        gateway,
        work_item("XYZ-9", "Elsewhere", "other.atlassian.net"),
    ]}));
    let (_, mcp) = root_in(&user, None);

    let listed = read(&user, mcp.call("tickets", json!({"action": "list"})));

    assert!(
        listed.contains(
            "- ABC-2 To Do: The login fails [bug] (Ana)  \
             https://example.atlassian.net/browse/ABC-2"
        ),
        "{listed}"
    );
    assert!(
        listed.contains(
            "- DEF-1 To Do: Add search [bug] (Ana)  https://example.atlassian.net/browse/DEF-1"
        ),
        "{listed}"
    );
    assert!(!listed.contains("XYZ-9"), "{listed}");
    assert!(listed.contains("1 answered from another site"), "{listed}");
}

#[test]
fn acli_signed_in_to_another_site_reads_nothing_for_a_listing_a_search_or_a_show() {
    let user = user();
    fixture(
        &user,
        "git@github.com:acme/app.git",
        json!({"kind": "github", "repository": "acme/app"}),
        json!({"kind": "jira", "site": "other.atlassian.net", "projects": ["ABC"]}),
    );
    // What acli would answer, signed in to example.atlassian.net, on an Atlassian host that does
    // not name the site: it must never be asked.
    let mut item = work_item("ABC-2", "Of the wrong site", "example.atlassian.net");
    item["self"] = json!("https://jira-prod-xx-1-1.prod.atl-paas.net/rest/api/3/issue/2");
    user.acli_script(json!({"items": {"ABC-2": item.clone()}, "search": [item]}));
    let (_, mcp) = root_in(&user, None);

    for args in [
        json!({"action": "list"}),
        json!({"action": "search", "query": "login"}),
        json!({"action": "show", "ticket": "ABC-2"}),
    ] {
        let answer = mcp.call("tickets", args.clone());
        assert!(
            answer.text.contains(
                "acli is signed in to example.atlassian.net, and this project's tracker is \
                 Jira on other.atlassian.net: nothing was read"
            ),
            "{args}: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("Of the wrong site"),
            "{}",
            answer.text
        );
    }
    assert!(
        user.acli_received()
            .iter()
            .all(|call| call["args"] == json!(["jira", "auth", "status"])),
        "{:?}",
        user.acli_received()
    );
}

/// `item` in the status `name`, of Jira's status category `category` (`new`, `indeterminate`,
/// `done`).
fn in_status(mut item: Value, name: &str, category: &str) -> Value {
    item["fields"]["status"] = json!({"name": name, "statusCategory": {"key": category}});
    item
}

/// The work items `acli` holds for the Jira tracker: ABC-1 to do, ABC-2 in progress, ABC-3 done,
/// what each can move to, and the key a creation answers.
fn jira_items(user: &User) {
    let site = "example.atlassian.net";
    let todo = in_status(work_item("ABC-1", "The login fails", site), "To Do", "new");
    let progress = in_status(
        work_item("ABC-2", "Add search", site),
        "In Progress",
        "indeterminate",
    );
    let done = in_status(work_item("ABC-3", "Shipped", site), "Done", "done");
    user.acli_script(json!({
        "items": {"ABC-1": todo, "ABC-2": progress, "ABC-3": done},
        "search": [todo, progress, done],
        "transitions": {"ABC-1": ["In Progress", "Done"], "ABC-3": ["To Do"]},
        "created": "DEF-7",
    }));
}

/// A write of `tickets` on Jira, held for the user then done once allowed: what it answered, and
/// the calls of `acli` that started with `verb`.
fn jira_written(args: Value, verb: &str) -> (String, Vec<String>) {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);
    let calls = || words(user.acli_received());
    let done = judged_then_done(&user, mcp.call("tickets", args), calls, verb);
    let asked = calls()
        .into_iter()
        .filter(|c| c.starts_with(verb))
        .collect();
    // Every write is checked to go to the tracker's site first.
    assert_eq!(calls()[0], "jira auth status");
    (done, asked)
}

/// The search that finds the statuses of the work items of ABC.
const STATUSES_OF_ABC: &str = "jira workitem search --jql project = \"ABC\" ORDER BY updated \
    DESC --fields key,status --json --limit 200";

#[test]
fn create_a_jira_work_item_in_a_tracker_s_project() {
    let (done, asked) = jira_written(
        json!({"action": "create", "project": "def", "title": "Add a logout",
            "body": "Users stay signed in", "type": "Story", "labels": ["feature", "auth"]}),
        "jira workitem create",
    );
    assert!(
        done.contains(
            "The ticket DEF-7 was opened in DEF: https://example.atlassian.net/browse/DEF-7"
        ),
        "{done}"
    );
    assert_eq!(
        asked,
        [
            "jira workitem create --project DEF --summary Add a logout --description Users stay \
             signed in --type Story --label feature,auth --json"
        ]
    );
}

#[test]
fn comment_on_a_jira_work_item() {
    let (done, asked) = jira_written(
        json!({"action": "comment", "ticket": "abc-1", "body": "Fixed on app/login"}),
        "jira workitem comment",
    );
    assert!(done.contains("The comment was posted on ABC-1."), "{done}");
    assert_eq!(
        asked,
        ["jira workitem comment create --key ABC-1 --body Fixed on app/login"]
    );
}

#[test]
fn edit_a_jira_work_item_s_summary_and_description() {
    let (done, asked) = jira_written(
        json!({"action": "edit", "ticket": "ABC-1", "title": "The logout fails",
            "body": "It hangs"}),
        "jira workitem edit",
    );
    assert!(done.contains("ABC-1 was changed."), "{done}");
    assert_eq!(
        asked,
        [
            "jira workitem edit --key ABC-1 --summary The logout fails --description It hangs \
             --yes"
        ]
    );
}

#[test]
fn close_a_jira_work_item_moves_it_to_a_done_transition() {
    let (done, asked) = jira_written(
        json!({"action": "close", "ticket": "ABC-1"}),
        "jira workitem transition",
    );
    assert!(done.contains("ABC-1 was closed: moved to Done."), "{done}");
    assert_eq!(
        asked,
        ["jira workitem transition --key ABC-1 --status Done --yes"]
    );
}

#[test]
fn reopen_a_jira_work_item() {
    let (done, asked) = jira_written(
        json!({"action": "reopen", "ticket": "ABC-3"}),
        "jira workitem transition",
    );
    assert!(
        done.contains("ABC-3 was reopened: moved to To Do."),
        "{done}"
    );
    assert_eq!(
        asked,
        ["jira workitem transition --key ABC-3 --status To Do --yes"]
    );
}

#[test]
fn move_a_jira_work_item_to_a_named_status() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);
    let calls = || words(user.acli_received());

    let done = judged_then_done(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "status", "ticket": "ABC-1", "status": "in progress"}),
        ),
        calls,
        "jira workitem transition",
    );

    assert!(done.contains("ABC-1 was moved to In Progress."), "{done}");
    assert_eq!(
        calls(),
        [
            "jira auth status",
            STATUSES_OF_ABC,
            "jira workitem transition --key ABC-1 --status In Progress --yes"
        ]
    );
}

#[test]
fn an_unknown_status_is_refused_with_the_statuses_it_can_move_to() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "status", "ticket": "ABC-1", "status": "Blocked"}),
    ));
    let answer = allow(&user, number);

    assert!(
        answer.contains(
            "refused: Blocked is not a status of ABC's work items, which are To Do, In Progress \
             and Done; nothing was written"
        ),
        "{answer}"
    );
    // A status the work item has no transition to: acli's refusal, and the statuses again.
    let number = held(&mcp.call(
        "tickets",
        json!({"action": "status", "ticket": "ABC-3", "status": "In Progress"}),
    ));
    let answer = allow(&user, number);
    assert!(
        answer.contains("acli could not move ABC-3 to In Progress"),
        "{answer}"
    );
    assert!(
        answer.contains("the statuses of ABC's work items are To Do, In Progress and Done"),
        "{answer}"
    );
    assert!(
        !words(user.acli_received())
            .iter()
            .any(|call| call.contains("--status Blocked")),
        "{:?}",
        user.acli_received()
    );
}

#[test]
fn a_write_with_acli_signed_in_to_another_site_writes_nothing() {
    let user = user();
    fixture(
        &user,
        "git@github.com:acme/app.git",
        json!({"kind": "github", "repository": "acme/app"}),
        json!({"kind": "jira", "site": "other.atlassian.net", "projects": ["ABC"]}),
    );
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);

    for args in [
        json!({"action": "create", "title": "x"}),
        json!({"action": "comment", "ticket": "ABC-1", "body": "x"}),
        json!({"action": "edit", "ticket": "ABC-1", "title": "x"}),
        json!({"action": "close", "ticket": "ABC-1"}),
        json!({"action": "reopen", "ticket": "ABC-3"}),
        json!({"action": "status", "ticket": "ABC-1", "status": "Done"}),
    ] {
        let number = held(&mcp.call("tickets", args.clone()));
        let answer = allow(&user, number);
        assert!(
            answer.contains(
                "acli is signed in to example.atlassian.net, and this project's tracker is Jira \
                 on other.atlassian.net: nothing was written"
            ),
            "{args}: {answer}"
        );
    }
    assert!(
        user.acli_received()
            .iter()
            .all(|call| call["args"] == json!(["jira", "auth", "status"])),
        "{:?}",
        user.acli_received()
    );
}

#[test]
fn a_write_to_a_key_of_a_project_outside_the_tracker_is_refused() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);

    for (args, said) in [
        (
            json!({"action": "comment", "ticket": "XYZ-1", "body": "x"}),
            "refused: XYZ-1 is not in a Jira project of this project's tracker",
        ),
        (
            json!({"action": "status", "ticket": "XYZ-1", "status": "Done"}),
            "refused: XYZ-1 is not in a Jira project of this project's tracker",
        ),
        (
            json!({"action": "create", "project": "XYZ", "title": "x"}),
            "refused: XYZ is not a project of this project's tracker (Jira on \
             example.atlassian.net (ABC, DEF))",
        ),
        (
            json!({"action": "create", "title": "x"}),
            "refused: tickets create on Jira needs project, one of the tracker's: ABC, DEF",
        ),
    ] {
        let refused = mcp.call("tickets", args.clone());
        assert!(refused.error, "{args}: {}", refused.text);
        assert!(refused.text.starts_with(said), "{args}: {}", refused.text);
    }
    assert!(no_request(&user));
    assert!(user.acli_received().is_empty());
}

#[test]
fn in_ask_mode_a_jira_write_is_held_like_a_github_write() {
    let request_of = |user: &User, mcp: &Mcp, ticket: &str| {
        held(&mcp.call(
            "tickets",
            json!({"action": "comment", "ticket": ticket, "body": "Fixed"}),
        ));
        let request = user.call("approval.list", json!({})).unwrap()["requests"][0].clone();
        (request["class"].clone(), request["reasons"].clone())
    };
    let github = user();
    on_github(&github);
    many_issues(&github);
    let (_, mcp) = root_in(&github, None);
    let held_on_github = request_of(&github, &mcp, "#1");
    let jira = user();
    on_jira(&jira);
    jira_items(&jira);
    let (_, mcp) = root_in(&jira, None);

    let held_on_jira = request_of(&jira, &mcp, "ABC-1");

    assert_eq!(held_on_jira, held_on_github);
    assert_eq!(held_on_jira.0, "judge");
    assert!(
        !words(jira.acli_received())
            .iter()
            .any(|call| call.starts_with("jira workitem")),
        "{:?}",
        jira.acli_received()
    );
}

// Sessions for the user.

fn agent(user: &User, id: &str) -> flottille_core::agent::Agent {
    store(user).agent(id).unwrap().unwrap()
}

fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_root_session_started_by_an_agent_appears_and_is_the_user_s() {
    let user = user();
    on_github(&user);
    let (lead, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "ask"}))
        .unwrap();
    let before = agents(&user).len();

    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Fix login", "brief": "The login fails: fix it.", "workspace": "login",
            "root": true}),
    );
    let number = held(&answer);
    assert_eq!(
        agents(&user).len(),
        before,
        "nothing started before the user allowed it"
    );
    let request = user.call("approval.list", json!({})).unwrap()["requests"][0].clone();
    assert_eq!(request["kind"], "action", "{request}");
    let says = request["says"].to_string();
    assert!(
        says.contains("Start a session for the user in the workspace login: \\\"Fix login\\\"")
            && says.contains("Its first message: The login fails: fix it."),
        "{says}"
    );
    let done = allow(&user, number);
    assert!(done.contains("Started a session for the user"), "{done}");
    assert!(done.contains("session_send"), "{done}");

    let all = agents(&user);
    assert_eq!(all.len(), before + 1);
    let started = all
        .iter()
        .find(|a| a["title"] == "Fix login")
        .unwrap_or_else(|| panic!("no new session in {all:?}"));
    let id = started["id"].as_str().unwrap();
    let new = agent(&user, id);
    // The user's: no parent, a root's depth and tools, in the workspace asked.
    assert_eq!(new.parent, None);
    assert_eq!(new.depth, 0);
    assert_eq!(new.workspace.as_deref(), Some("login"));
    assert!(new.tools.iter().any(|t| t.ends_with("__ask_user")));
    assert!(new.tools.iter().any(|t| t.ends_with("__agent_launch")));
    // It takes the tree's mode, Ask here, never the Auto a new root has by default.
    assert_eq!(new.gate_mode, GateMode::Ask);
    // Its first message is the launcher's, never the user's, and it keeps who started it.
    assert_eq!(new.launched_by.as_deref(), Some(lead.as_str()));
    eventually("its first message", || {
        user.received_requests("session/prompt")
            .iter()
            .any(|prompt| prompt.to_string().contains("The login fails: fix it."))
    });
    let first = store(&user)
        .delivery(&format!("brief-{id}-0"))
        .unwrap()
        .unwrap();
    assert_eq!(first.kind, "launched");
    assert!(
        first
            .body
            .starts_with(&format!("agent {lead} started this session")),
        "{}",
        first.body
    );
    // Not the caller's child: its agent tools do not reach it.
    let status = mcp.call("agent_status", json!({"agent_id": id}));
    assert!(status.error, "{}", status.text);
}

#[test]
fn a_root_session_in_auto_is_judged_by_the_judge_and_stays_in_auto() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    on_github(&user);
    let (lead, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "auto"}))
        .unwrap();

    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Docs", "brief": "Write the docs.", "root": true}),
    );

    assert!(
        answer.text.contains("Started a session for the user"),
        "{}",
        answer.text
    );
    let judged = Value::from(user.judge_received()).to_string();
    assert!(
        judged.contains("it starts a new session for the user"),
        "{judged}"
    );
    let started = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Docs")
        .unwrap();
    let new = agent(&user, started["id"].as_str().unwrap());
    assert_eq!(new.parent, None);
    assert_eq!(new.workspace, None, "the caller's own place, main");
    assert_eq!(new.gate_mode, GateMode::Auto);
}

#[test]
fn a_child_cannot_start_a_root_session() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, None);
    let opened = user.received_requests("session/new").len();
    let launched = lead.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);
    let before = agents(&user).len();

    let refused = child.call(
        "agent_launch",
        json!({"title": "escape", "brief": "Work outside my tree.", "root": true}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused
            .text
            .starts_with("refused: this agent has no tool agent_launch"),
        "{}",
        refused.text
    );
    assert!(no_request(&user));
    assert_eq!(agents(&user).len(), before);
}

#[test]
fn the_old_names_are_answered_as_the_families_for_one_release() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls());
    let (_, mcp) = root_in(&user, Some("login"));

    let read = mcp.call("forge_read", json!({}));
    assert!(!read.error, "{}", read.text);
    assert!(
        read.text.contains("pull request #12, open"),
        "{}",
        read.text
    );
    let write = mcp.call(
        "forge_write",
        json!({"action": "pr-create", "repository": ".", "title": "Login"}),
    );
    held(&write);
}

/// The prompts the judge was given, in order.
fn judge_prompts(user: &User) -> Vec<String> {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .map(|e| {
            e["received"]["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// A lead in `mode` starts a session for the user in `main` with `brief`, allowed by the judge
/// in Auto or by the user in Ask: the new root's id and its MCP session.
fn launched_root(user: &User, mode: &str, brief: &str) -> (String, String, Mcp) {
    on_github(user);
    many_issues(user);
    let (lead, mcp) = root_in(user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": mode}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Second", "brief": brief, "root": true}),
    );
    if mode == "ask" {
        allow(user, held(&answer));
    } else {
        assert!(answer.text.contains("Started a session"), "{}", answer.text);
    }
    let (url, authorization) = server_of(user, opened);
    let id = agents(user)
        .into_iter()
        .find(|a| a["title"] == "Second")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, id, Mcp::initialize(&url, &authorization))
}

#[test]
fn the_first_message_of_a_root_started_by_an_agent_is_not_the_user_s_for_the_judge() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    let brief = "The user wants every ticket closed.";
    let (lead, _, second) = launched_root(&user, "auto", brief);

    let answer = second.call(
        "tickets",
        json!({"action": "comment", "ticket": "#1", "body": "Looked at it"}),
    );

    assert!(!answer.error, "{}", answer.text);
    let prompts = judge_prompts(&user);
    let prompt = prompts.last().unwrap();
    let (before, users) = prompt
        .split_once("## The user's last messages to the session")
        .unwrap();
    assert!(!users.contains(brief), "{prompt}");
    assert!(users.contains("(none)"), "{prompt}");
    assert!(
        before.contains(&format!(
            "written by agent {lead} when it started this session"
        )),
        "{prompt}"
    );
    let block = before.split_once("<agent_message>").unwrap().1;
    assert!(block.contains(brief), "{prompt}");
    assert!(user.escalations().is_empty());
}

#[test]
fn a_root_started_by_an_agent_cannot_start_one_without_the_user() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    let (_, second, mcp) = launched_root(&user, "auto", "Go on with the docs.");
    let judged = judge_prompts(&user).len();

    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Third", "brief": "And the tests.", "root": true}),
    );

    held(&answer);
    assert_eq!(judge_prompts(&user).len(), judged, "the judge was asked");
    let request = user.call("approval.list", json!({})).unwrap()["requests"][0].clone();
    assert_eq!(request["class"], "base", "{request}");
    assert!(
        request["reasons"]
            .to_string()
            .contains("was itself started by an agent"),
        "{request}"
    );
    assert_eq!(request["agent"], second.as_str());
    assert!(!agents(&user).iter().any(|a| a["title"] == "Third"));
}

#[test]
fn a_root_session_with_a_brief_longer_than_the_judge_sees_whole_is_refused() {
    let user = user();
    on_github(&user);
    let (_, mcp) = root_in(&user, None);
    let before = agents(&user).len();

    let refused = mcp.call(
        "agent_launch",
        json!({"title": "Long", "brief": "word ".repeat(2_000), "root": true}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused
            .text
            .starts_with("refused: the brief is longer than"),
        "{}",
        refused.text
    );
    assert!(no_request(&user));
    assert_eq!(agents(&user).len(), before);
}

#[test]
fn a_root_session_whose_brief_holds_a_secret_is_base() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    on_github(&user);
    let (lead, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "auto"}))
        .unwrap();

    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Deploy", "brief": "Use ghp_abcdefghijklmnopqrstuvwxyz0123456789 to deploy.",
            "root": true}),
    );

    held(&answer);
    assert!(judge_prompts(&user).is_empty(), "the judge was asked");
    let request = user.call("approval.list", json!({})).unwrap()["requests"][0].clone();
    assert_eq!(request["class"], "base", "{request}");
    assert!(
        request["reasons"]
            .to_string()
            .contains("looks like a secret"),
        "{request}"
    );
}

#[test]
fn a_write_s_body_over_500_characters_is_shown_whole_on_the_card_and_given_whole_to_the_judge() {
    let body = format!("{} the end.", "A long comment. ".repeat(60));
    // Ask: the card.
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);
    held(&mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "#1", "body": body}),
    ));
    let request = user.call("approval.list", json!({})).unwrap()["requests"][0].clone();
    assert!(
        request["says"]
            .as_array()
            .unwrap()
            .iter()
            .any(|said| said.as_str().unwrap().ends_with(&body)),
        "{request}"
    );
    // Auto: the judge.
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    on_github(&user);
    user.gh_script(pulls());
    let (lead, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "auto"}))
        .unwrap();
    let answer = mcp.call(
        "forge",
        json!({"action": "comment", "number": 12, "body": body}),
    );
    assert!(!answer.error, "{}", answer.text);
    let prompts = judge_prompts(&user);
    assert!(prompts[0].contains(&body), "{}", prompts[0]);
    assert!(user.escalations().is_empty());
}

#[test]
fn a_write_whose_body_holds_a_token_is_base() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    on_github(&user);
    many_issues(&user);
    let (lead, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "auto"}))
        .unwrap();

    let answer = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "#1",
            "body": "The token is ghp_abcdefghijklmnopqrstuvwxyz0123456789"}),
    );

    held(&answer);
    assert!(judge_prompts(&user).is_empty(), "the judge was asked");
    let request = user.call("approval.list", json!({})).unwrap()["requests"][0].clone();
    assert_eq!(request["class"], "base", "{request}");
    assert!(
        request["reasons"]
            .to_string()
            .contains("looks like a secret"),
        "{request}"
    );
    assert!(
        !words(user.gh_received())
            .iter()
            .any(|c| c.starts_with("issue comment"))
    );
}

#[test]
fn a_write_longer_than_the_judge_sees_whole_is_refused() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    let refused = mcp.call(
        "tickets",
        json!({"action": "create", "title": "Long", "body": "word ".repeat(2_000)}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused.text.starts_with("refused: the body is longer than"),
        "{}",
        refused.text
    );
    assert!(no_request(&user));
}

#[test]
fn a_title_on_a_comment_close_or_reopen_is_refused_before_anything_is_asked() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    user.gh_script(pulls());
    let (_, mcp) = root_in(&user, None);

    for (tool, args) in [
        (
            "tickets",
            json!({"action": "comment", "ticket": "#1", "title": "T", "body": "b"}),
        ),
        (
            "tickets",
            json!({"action": "close", "ticket": "#1", "title": "T"}),
        ),
        (
            "tickets",
            json!({"action": "reopen", "ticket": "#41", "title": "T"}),
        ),
        (
            "forge",
            json!({"action": "comment", "number": 12, "title": "T", "body": "b"}),
        ),
    ] {
        let refused = mcp.call(tool, args);
        assert!(refused.error, "{}", refused.text);
        assert!(
            refused.text.contains("title is for edit"),
            "{}",
            refused.text
        );
    }
    assert!(no_request(&user));
}

#[test]
fn a_text_a_write_never_posts_is_refused_before_anything_is_asked() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    user.gh_script(pulls());
    let (_, mcp) = root_in(&user, None);

    for (tool, args, key) in [
        (
            "tickets",
            json!({"action": "close", "ticket": "#1", "body": "b"}),
            "body",
        ),
        (
            "tickets",
            json!({"action": "reopen", "ticket": "#41", "body": "b"}),
            "body",
        ),
        (
            "forge",
            json!({"action": "merge", "number": 12, "title": "T"}),
            "title",
        ),
        (
            "forge",
            json!({"action": "merge", "number": 12, "body": "b"}),
            "body",
        ),
        (
            "forge",
            json!({"action": "checks", "number": 12, "body": "b"}),
            "body",
        ),
        (
            "forge",
            json!({"action": "checks", "number": 12, "title": "T"}),
            "title",
        ),
    ] {
        let refused = mcp.call(tool, args);
        assert!(refused.error, "{}", refused.text);
        assert!(
            refused.text.contains(&format!("takes no {key}")),
            "{}",
            refused.text
        );
    }
    assert!(no_request(&user));
}

#[test]
fn a_session_in_a_new_workspace_with_a_brief_longer_than_the_judge_sees_whole_is_refused() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, None);
    let before = agents(&user).len();

    let refused = lead.call(
        "agent_launch",
        json!({"title": "Long", "brief": "word ".repeat(2_000), "new_workspace": "long",
            "root": true}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused
            .text
            .starts_with("refused: the brief is longer than"),
        "{}",
        refused.text
    );
    assert!(no_request(&user));
    assert_eq!(agents(&user).len(), before);
    assert!(!workspace_exists(&user, "long"));
}

#[test]
fn tickets_search_refuses_a_qualifier_that_reaches_another_repository() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    for query in [
        "repo:acme/secret token",
        "login ORG:acme",
        "user:ana",
        "is:pr login",
        "Type:PR",
        "owner:acme login",
        "login (repo:acme/secret)",
        "login -user:ana",
    ] {
        let refused = mcp.call("tickets", json!({"action": "search", "query": query}));
        assert!(refused.error, "{query}: {}", refused.text);
        assert!(
            refused
                .text
                .starts_with("refused: tickets search looks in the tracker's repository only"),
            "{}",
            refused.text
        );
    }
    assert!(issue_lists(&user).is_empty());
}

#[test]
fn tickets_search_passes_a_word_that_only_ends_in_a_qualifier() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    let found = mcp.call(
        "tickets",
        json!({"action": "search", "query": "the superuser:root cannot log in"}),
    );

    assert!(!found.error, "{}", found.text);
}

#[test]
fn tickets_search_refuses_a_label_or_assignee_that_carries_a_qualifier() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    for (field, value) in [
        ("label", "bug\" repo:acme/secret \""),
        ("label", "a:b"),
        ("assignee", "ana\" org:acme"),
        ("assignee", "owner:x"),
    ] {
        let refused = mcp.call(
            "tickets",
            json!({"action": "search", "query": "login", field: value}),
        );
        assert!(refused.error, "{field} {value}: {}", refused.text);
        assert!(
            refused.text.starts_with("refused: tickets search "),
            "{}",
            refused.text
        );
    }
    assert!(issue_lists(&user).is_empty());
}

#[test]
fn tickets_list_keeps_only_the_tracker_repository_s_issues() {
    let user = user();
    on_github(&user);
    let mut elsewhere = issue(7, "Not ours", "OPEN", &[], &[]);
    elsewhere["url"] = json!("https://github.com/acme/secret/issues/7");
    let mut pull = issue(8, "A pull request", "OPEN", &[], &[]);
    pull["url"] = json!("https://github.com/acme/app/pull/8");
    user.gh_script(json!({"issues": {"acme/app": {
        "1": issue(1, "Ours", "OPEN", &[], &[]),
        "7": elsewhere,
        "8": pull,
    }}}));
    let (_, mcp) = root_in(&user, None);

    let listed = read(&user, mcp.call("tickets", json!({"action": "list"})));

    assert!(listed.contains("- #1 open: Ours"), "{listed}");
    assert!(!listed.contains("Not ours"), "{listed}");
    assert!(!listed.contains("A pull request"), "{listed}");
    assert!(!listed.contains("left out"), "{listed}");
}

#[test]
fn jira_items_without_a_self_link_are_kept() {
    let user = user();
    on_jira(&user);
    let mut bare = work_item("ABC-3", "No link", "example.atlassian.net");
    bare.as_object_mut().unwrap().remove("self");
    user.acli_script(json!({"search": [
        bare,
        work_item("XYZ-9", "Elsewhere", "other.atlassian.net"),
    ]}));
    let (_, mcp) = root_in(&user, None);

    let listed = read(&user, mcp.call("tickets", json!({"action": "list"})));

    assert!(listed.contains("- ABC-3 To Do: No link"), "{listed}");
    assert!(!listed.contains("XYZ-9"), "{listed}");
    assert!(listed.contains("1 answered from another site"), "{listed}");
}

#[test]
fn jira_items_whose_self_link_names_another_site_or_none_that_parses_are_left_out() {
    let user = user();
    on_jira(&user);
    let mut plain = work_item("XYZ-1", "Plain http elsewhere", "other.atlassian.net");
    plain["self"] = json!("http://other.atlassian.net/rest/api/3/issue/1");
    let mut odd = work_item("XYZ-2", "Not a link", "example.atlassian.net");
    odd["self"] = json!("not a link");
    let mut here = work_item("ABC-4", "Plain http here", "example.atlassian.net");
    here["self"] = json!("http://Example.Atlassian.net/rest/api/3/issue/4");
    user.acli_script(json!({"search": [here, plain, odd]}));
    let (_, mcp) = root_in(&user, None);

    let listed = read(&user, mcp.call("tickets", json!({"action": "list"})));

    assert!(
        listed.contains("- ABC-4 To Do: Plain http here"),
        "{listed}"
    );
    assert!(!listed.contains("XYZ-1"), "{listed}");
    assert!(!listed.contains("XYZ-2"), "{listed}");
    assert!(listed.contains("2 answered from another site"), "{listed}");
}

fn workspace_exists(user: &User, name: &str) -> bool {
    store(user).workspace("app", name).unwrap().is_some()
}

#[test]
fn a_child_in_main_cannot_remove_a_workspace() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, None);
    let opened = user.received_requests("session/new").len();
    let launched = lead.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Read the code."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);

    let refused = child.call("workspace", json!({"action": "remove", "name": "login"}));

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused
            .text
            .starts_with("refused: the agent the user talks to manages the project's workspaces"),
        "{}",
        refused.text
    );
    assert!(no_request(&user));
    assert!(workspace_exists(&user, "login"));
}

#[test]
fn a_workspace_a_session_for_the_user_works_in_is_in_use() {
    let user = judged_user();
    let (_, root, session, _) = lead_and_session(&user);

    let answer = root.call("workspace", json!({"action": "remove", "name": "login"}));
    let done = allow(&user, held(&answer));

    assert!(
        done.contains(&format!(
            "the agent {session}, outside this agent's tree, works in it"
        )),
        "{done}"
    );
    assert!(workspace_exists(&user, "login"));
}

/// The request the user holds, the only one.
fn the_request(user: &User) -> Value {
    let requests = user.call("approval.list", json!({})).unwrap()["requests"].clone();
    assert_eq!(requests.as_array().unwrap().len(), 1, "{requests}");
    requests[0].clone()
}

#[test]
fn workspace_remove_is_held_for_the_user_in_ask_and_in_auto_never_judged() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    on_github(&user);
    let (lead, mcp) = root_in(&user, None);

    let asked = held(&mcp.call("workspace", json!({"action": "remove", "name": "login"})));
    assert_eq!(the_request(&user)["class"], "base");
    user.call("approval.deny", json!({"number": asked}))
        .unwrap();

    user.call("agent.mode", json!({"agent": lead, "mode": "auto"}))
        .unwrap();
    let answer = mcp.call("workspace", json!({"action": "remove", "name": "login"}));

    held(&answer);
    let request = the_request(&user);
    assert_eq!(request["class"], "base", "{request}");
    assert_eq!(request["judgement"], Value::Null, "{request}");
    assert!(
        judge_prompts(&user).is_empty(),
        "{:?}",
        judge_prompts(&user)
    );
    assert!(workspace_exists(&user, "login"));
}

#[test]
fn workspace_remove_allowed_on_a_clean_workspace_removes_it() {
    let user = user();
    on_github(&user);
    let (_, mcp) = root_in(&user, None);

    let number = held(&mcp.call("workspace", json!({"action": "remove", "name": "login"})));
    let says = the_request(&user)["says"].to_string();
    assert!(says.contains("Remove the workspace login"), "{says}");
    assert!(says.contains("Nothing would be lost"), "{says}");
    let done = allow(&user, number);

    assert!(done.contains("and Flottille has now done it"), "{done}");
    assert!(
        done.contains("Workspace login of project app is removed"),
        "{done}"
    );
    assert!(!workspace_exists(&user, "login"));
    assert!(!user.path("work/app/.flottille/workspaces/login").exists());
}

#[test]
fn workspace_remove_on_a_workspace_with_an_unpushed_commit_names_it_and_is_refused_without_the_discard_answer()
 {
    let user = user();
    on_github(&user);
    let folder = workspace_folder(&user);
    std::fs::write(folder.join("login.ts"), "login\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "add the login"]);
    let (_, mcp) = root_in(&user, None);

    let number = held(&mcp.call("workspace", json!({"action": "remove", "name": "login"})));
    let says = the_request(&user)["says"].to_string();
    assert!(says.contains("has commits on no remote"), "{says}");
    assert!(says.contains("add the login"), "{says}");
    assert!(!says.contains("Nothing would be lost"), "{says}");
    let answer = allow(&user, number);

    assert!(answer.contains("was not executed"), "{answer}");
    assert!(answer.contains("add the login"), "{answer}");
    assert!(
        answer.contains(
            "this request is decided: to remove it all the same, call workspace remove again, \
             and the user may answer that new request with discard"
        ),
        "{answer}"
    );
    assert!(workspace_exists(&user, "login"));
    assert!(folder.join("login.ts").exists());

    let number = held(&mcp.call("workspace", json!({"action": "remove", "name": "login"})));
    let approved = user.run(&["approve", &number.to_string(), "--discard"]);
    assert!(
        approved.status.success(),
        "{}",
        String::from_utf8_lossy(&approved.stderr)
    );
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(&user).delivery(&id).unwrap().is_some()
    });
    let done = store(&user).delivery(&id).unwrap().unwrap().body;

    assert!(done.contains("and Flottille has now done it"), "{done}");
    assert!(done.contains("discarded"), "{done}");
    assert!(!workspace_exists(&user, "login"));
    assert!(!folder.exists());
}

/// This user with a `git` first in its `PATH` that, before the `look`th `git status` of the
/// workspace `login`, writes an uncommitted file in it: work that appears while a removal is
/// under way. Everything else is the real `git`.
fn with_work_appearing_before_the_look(user: User, look: u32) -> User {
    let real = std::env::var("PATH")
        .unwrap()
        .split(':')
        .map(|folder| Path::new(folder).join("git"))
        .find(|candidate| candidate.is_file())
        .unwrap();
    let folder = user.path("data/flottille/workspaces/app/login");
    let bin = user.path("late-git-bin");
    std::fs::create_dir(&bin).unwrap();
    let counter = user.path("looks");
    let program = bin.join("git");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\ncase \"$*\" in *'-C {folder} '*' status '*)\n  n=$(cat '{counter}' 2>/dev/null || echo 0)\n  n=$((n + 1))\n  echo $n > '{counter}'\n  [ \"$n\" = {look} ] && echo late > '{folder}/late.txt'\n  ;;\nesac\nexec '{real}' \"$@\"\n",
            folder = folder.display(),
            counter = counter.display(),
            real = real.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    user.with_env("PATH", &path)
}

#[test]
fn work_appearing_after_the_user_s_answer_is_checked_is_not_discarded_by_the_removal() {
    // The work appears just before the fifth look at the workspace, the removal's own: the four
    // before are the card's and the answer's checks of it.
    let user = with_work_appearing_before_the_look(user(), 5);
    on_github(&user);
    let folder = workspace_folder(&user);
    std::fs::write(folder.join("login.ts"), "login\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "add the login"]);
    let (_, mcp) = root_in(&user, None);
    let number = held(&mcp.call("workspace", json!({"action": "remove", "name": "login"})));

    let approved = user.run(&["approve", &number.to_string(), "--discard"]);
    assert!(
        approved.status.success(),
        "{}",
        String::from_utf8_lossy(&approved.stderr)
    );
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(&user).delivery(&id).unwrap().is_some()
    });
    let answer = store(&user).delivery(&id).unwrap().unwrap().body;

    assert!(answer.contains("was not executed"), "{answer}");
    assert!(
        answer.contains("late.txt holds work that is not committed"),
        "{answer}"
    );
    assert!(
        answer.contains("is not what the user answered to discard"),
        "{answer}"
    );
    assert!(workspace_exists(&user, "login"));
    assert!(folder.join("late.txt").exists());
    assert!(folder.join("login.ts").exists());
}

#[test]
fn the_discard_answer_is_refused_for_a_request_that_removes_no_workspace() {
    let user = user();
    on_github(&user);
    let (_, mcp) = root_in(&user, None);
    let number = held(&mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "#1", "body": "Done."}),
    ));

    let refused = user.call(
        "approval.approve",
        json!({"number": number, "discard": true}),
    );

    let (code, message) = refused.unwrap_err();
    assert_eq!(code, "refused", "{message}");
    assert!(message.contains("discard"), "{message}");
    assert_eq!(the_request(&user)["state"], "pending");
}

#[test]
fn workspace_remove_called_from_inside_the_workspace_is_refused() {
    let user = user();
    on_github(&user);
    let (_, mcp) = root_in(&user, Some("login"));

    for args in [
        json!({"action": "remove"}),
        json!({"action": "remove", "name": "login"}),
    ] {
        let refused = mcp.call("workspace", args);
        assert!(refused.error, "{}", refused.text);
        assert!(
            refused
                .text
                .starts_with("refused: this agent works in the workspace login"),
            "{}",
            refused.text
        );
    }
    assert!(no_request(&user));
    assert!(workspace_exists(&user, "login"));
}

/// The folder of the workspace `login`.
fn workspace_folder(user: &User) -> PathBuf {
    PathBuf::from(
        user.call("workspace.show", json!({"project": "app", "name": "login"}))
            .unwrap()["folder"]
            .as_str()
            .unwrap(),
    )
}

// Sessions in workspaces, and sessions that talk to each other.

/// A lead in Auto with a judge that allows, in `main`, and the session it started for the user
/// in the workspace `login`: the lead's id and MCP session, the session's id and MCP session.
fn lead_and_session(user: &User) -> (String, Mcp, String, Mcp) {
    on_github(user);
    many_issues(user);
    let (lead, mcp) = root_in(user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "auto"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Login", "brief": "Fix the login.", "workspace": "login", "root": true}),
    );
    assert!(answer.text.contains("Started a session"), "{}", answer.text);
    let (url, authorization) = server_of(user, opened);
    let id = agents(user)
        .into_iter()
        .find(|a| a["title"] == "Login")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, mcp, id, Mcp::initialize(&url, &authorization))
}

fn judged_user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge()
}

/// The deliveries of `kind` handed to `agent`, oldest last.
fn delivered(user: &User, agent: &str, kind: &str) -> Vec<String> {
    store(user)
        .last_deliveries(agent, 32)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == kind)
        .map(|delivery| delivery.body)
        .collect()
}

#[test]
fn a_child_with_a_workspace_is_refused_naming_root_true() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, None);
    let before = agents(&user).len();

    for place in [
        json!({"workspace": "login"}),
        json!({"new_workspace": "fresh"}),
        json!({"new_workspace": "fresh", "repositories": ["."]}),
    ] {
        let mut call = json!({"title": "Fix login", "brief": "The login fails: fix it."});
        call.as_object_mut()
            .unwrap()
            .extend(place.as_object().unwrap().clone());
        let refused = lead.call("agent_launch", call);
        assert!(refused.error, "{place}: {}", refused.text);
        assert!(
            refused
                .text
                .starts_with("refused: a child works where you work")
                && refused.text.contains("\"root\": true"),
            "{place}: {}",
            refused.text
        );
    }
    assert!(no_request(&user));
    assert_eq!(agents(&user).len(), before);
    assert!(!workspace_exists(&user, "fresh"));
}

#[test]
fn a_child_naming_its_parent_s_own_workspace_works_there() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, Some("login"));

    let launched = lead.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests.", "workspace": "login"}),
    );

    assert!(!launched.error, "{}", launched.text);
    let child = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "worker")
        .unwrap();
    assert_eq!(child["workspace"], "login");
}

#[test]
fn agent_launch_leads_with_the_session_form_for_work_in_a_workspace() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, None);

    let listed = lead.tools();
    let launch = listed
        .iter()
        .find(|tool| tool["name"] == "agent_launch")
        .unwrap();
    let description = launch["description"].as_str().unwrap();
    assert!(
        description.starts_with("Work that belongs in a workspace is a session"),
        "{description}"
    );
    assert!(
        description.contains("\"root\": true, \"workspace\": \"login\""),
        "{description}"
    );
}

#[test]
fn a_session_started_in_a_workspace_is_listed_under_that_workspace() {
    let user = judged_user();
    let (lead, _, session, _) = lead_and_session(&user);

    let trees = user.call("agent.trees", json!({"project": "app"})).unwrap();
    let roots: Vec<&Value> = trees["trees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tree| &tree["status"]["agent"])
        .collect();
    let own = roots
        .iter()
        .find(|agent| agent["id"] == session.as_str())
        .unwrap_or_else(|| panic!("not a root of its own: {trees}"));
    assert_eq!(own["workspace"], "login", "{own}");
    assert_eq!(own["parent"], Value::Null, "{own}");
    assert_eq!(own["launched_by"], lead.as_str(), "{own}");
    // Not under the agent that started it.
    let lead_tree = trees["trees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tree| tree["status"]["agent"]["id"] == lead.as_str())
        .unwrap();
    assert!(
        !lead_tree["agents"].to_string().contains(&session),
        "{lead_tree}"
    );
}

#[test]
fn session_send_from_the_starter_reaches_the_session_labelled_and_its_answer_comes_back() {
    let user = judged_user();
    let (lead, mcp, session, second) = lead_and_session(&user);

    let sent = mcp.call(
        "session_send",
        json!({"session": session, "text": "Also cover the logout."}),
    );

    assert!(!sent.error, "{}", sent.text);
    eventually("the message in the session", || {
        !delivered(&user, &session, "session").is_empty()
    });
    let body = delivered(&user, &session, "session").remove(0);
    assert!(
        body.starts_with(&format!("agent {lead} wrote to this session:")),
        "{body}"
    );
    assert!(body.contains("Also cover the logout."), "{body}");
    assert!(delivered(&user, &session, "user").is_empty());
    eventually("the session prompted with it", || {
        user.received_requests("session/prompt")
            .iter()
            .any(|p| p.to_string().contains("Also cover the logout."))
    });

    let answered = second.call(
        "session_send",
        json!({"session": lead, "text": "Logout covered too."}),
    );

    assert!(!answered.error, "{}", answered.text);
    eventually("the answer delivered to the starter", || {
        delivered(&user, &lead, "session")
            .iter()
            .any(|body| body.contains("Logout covered too."))
    });
    let body = delivered(&user, &lead, "session").remove(0);
    assert!(
        body.starts_with(&format!(
            "\"Login\" (agent {session}) wrote to this session:"
        )),
        "{body}"
    );
}

#[test]
fn session_send_to_any_other_session_is_refused() {
    let user = judged_user();
    let (lead, mcp, session, second) = lead_and_session(&user);
    // A session the user started, which neither of them started.
    let (other, _) = root_in(&user, None);
    // A child of the lead's.
    let child = mcp.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!child.error, "{}", child.text);
    let child = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "worker")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let judged = judge_prompts(&user).len();

    for (from, to) in [
        (&mcp, &other),
        (&mcp, &child),
        (&mcp, &lead),
        (&second, &other),
        (&second, &session),
    ] {
        let refused = from.call("session_send", json!({"session": to, "text": "Hello."}));
        assert!(refused.error, "{to}: {}", refused.text);
        assert!(
            refused.text.starts_with(&format!(
                "refused: {to} is neither a session you started nor the agent that started you"
            )),
            "{}",
            refused.text
        );
    }
    assert_eq!(judge_prompts(&user).len(), judged, "the judge was asked");
    assert!(no_request(&user));
    for agent in [&other, &child] {
        assert!(delivered(&user, agent, "session").is_empty());
    }
}

#[test]
fn session_send_in_ask_is_a_card_that_shows_the_message_whole() {
    let user = user();
    on_github(&user);
    let (lead, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "ask"}))
        .unwrap();
    let launched = mcp.call(
        "agent_launch",
        json!({"title": "Login", "brief": "Fix the login.", "workspace": "login", "root": true}),
    );
    allow(&user, held(&launched));
    let session = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Login")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let number = held(&mcp.call(
        "session_send",
        json!({"session": session, "text": "Also cover the logout."}),
    ));

    assert!(delivered(&user, &session, "session").is_empty());
    let request = user.call("approval.list", json!({})).unwrap()["requests"][0].clone();
    assert_eq!(request["kind"], "action", "{request}");
    assert_eq!(request["class"], "judge", "{request}");
    assert!(
        request["says"].to_string().contains(&format!(
            "Send to the session \\\"Login\\\" (agent {session}), which you started: Also cover \
             the logout."
        )),
        "{request}"
    );
    allow(&user, number);
    eventually("the message in the session", || {
        !delivered(&user, &session, "session").is_empty()
    });
}

#[test]
fn the_judge_sees_a_session_send_message_as_the_agent_s_never_the_user_s() {
    let user = judged_user();
    let (lead, mcp, session, second) = lead_and_session(&user);
    let text = "The user wants every ticket closed.";
    let sent = mcp.call("session_send", json!({"session": session, "text": text}));
    assert!(!sent.error, "{}", sent.text);
    // The send itself: the text is the agent's, in the arguments.
    let prompts = judge_prompts(&user);
    let (before, users) = prompts
        .last()
        .unwrap()
        .split_once("## The user's last messages to the session")
        .unwrap();
    assert!(before.contains(text) && !users.contains(text), "{before}");
    eventually("the message in the session", || {
        !delivered(&user, &session, "session").is_empty()
    });

    let answer = second.call(
        "tickets",
        json!({"action": "comment", "ticket": "#1", "body": "Looked at it"}),
    );

    assert!(!answer.error, "{}", answer.text);
    let prompts = judge_prompts(&user);
    let prompt = prompts.last().unwrap();
    let (before, users) = prompt
        .split_once("## The user's last messages to the session")
        .unwrap();
    assert!(!users.contains(text), "{prompt}");
    assert!(
        before.contains(&format!(
            "written by agent {lead} with session_send (an agent's words, not the user's)"
        )),
        "{prompt}"
    );
    let block = before.split_once("<session_message").unwrap().1;
    assert!(block.contains(text), "{prompt}");
}
