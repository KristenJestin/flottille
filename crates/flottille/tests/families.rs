//! The complete families of the agents' tools (#141, `docs/tools.md`, "The forge", "The tickets",
//! "Sessions an agent starts"): every read of `forge` and `tickets` answers at once, every write is
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
/// What Flottille ended the comments and bodies it posted for an agent with before it recorded
/// them (`docs/forges.md`): the old posts are still read as an agent's.
const MARKER: &str = "<!-- flottille -->";
/// The same on Jira.
const JIRA_FOOTER: &str = "— posted by a Flottille agent";

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
fn the_list_of_all_pull_requests_says_no_state() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls());
    let (_, mcp) = root_in(&user, None);

    let all = read(
        &user,
        mcp.call("forge", json!({"action": "list", "state": "all"})),
    );
    assert!(all.starts_with("The pull requests:\n"), "{all}");
    assert!(!all.contains("The all"), "{all}");

    let branch = read(
        &user,
        mcp.call(
            "forge",
            json!({"action": "list", "state": "all", "branch": "app/old"}),
        ),
    );
    assert!(
        branch.starts_with("The pull requests of app/old:\n"),
        "{branch}"
    );
    assert!(!branch.contains("The all"), "{branch}");

    let open = read(
        &user,
        mcp.call("forge", json!({"action": "list", "state": "open"})),
    );
    assert!(open.starts_with("The open pull requests:\n"), "{open}");
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
fn updating_a_merged_pull_request_is_refused_before_the_forge_is_called() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls_to_close());
    let (_, mcp) = root_in(&user, None);

    let merged = allowed_then_refused(
        &user,
        mcp.call(
            "forge",
            json!({"action": "edit", "number": 7, "title": "feat: new title"}),
        ),
    );
    let closed = allowed_then_refused(
        &user,
        mcp.call(
            "forge",
            json!({"action": "edit", "number": 9, "body": "A new description"}),
        ),
    );

    assert!(
        merged.contains("the pull request #7 of acme/app is merged, not open")
            && merged.contains("nothing was written"),
        "{merged}"
    );
    assert!(
        closed.contains("the pull request #9 of acme/app is closed, not open")
            && closed.contains("nothing was written"),
        "{closed}"
    );
    // Its state was read, and nothing was written.
    let calls = words(user.gh_received());
    assert!(calls.iter().any(|call| call.starts_with("pr view 7 ")));
    assert!(!calls.iter().any(|call| call.starts_with("pr edit")));
}

/// A Cloud pull request of `acme/app` as `bkt pr list` prints it.
fn cloud_pull(id: u64, state: &str) -> Value {
    json!({"id": id, "title": "Work", "state": state,
        "source": {"branch": {"name": format!("app/b{id}")},
            "repository": {"full_name": "acme/app"}},
        "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}},
        "links": {"html": {"href": format!("https://bitbucket.org/acme/app/pull-requests/{id}")}}})
}

/// The page of a Cloud pull request of `acme/app`, whose `state` is what `forge edit` reads.
fn cloud_page(id: u64, state: &str) -> (String, Value) {
    (
        format!("/repositories/acme/app/pullrequests/{id}"),
        json!({"id": id, "state": state}),
    )
}

#[test]
fn updating_a_declined_bitbucket_pull_request_is_refused_before_bkt_is_called() {
    let user = user();
    on_bitbucket(&user);
    let pages: serde_json::Map<String, Value> = [
        cloud_page(2, "MERGED"),
        cloud_page(3, "DECLINED"),
        cloud_page(4, "OPEN"),
        cloud_page(5, "SUPERSEDED"),
    ]
    .into_iter()
    .collect();
    user.bkt_script(json!({"pulls": {"acme/app": [
        cloud_pull(2, "MERGED"), cloud_pull(3, "DECLINED"), cloud_pull(4, "OPEN"),
        cloud_pull(5, "SUPERSEDED")]}, "api": pages}));
    let (_, mcp) = root_in(&user, None);
    let edit = |number: u64| {
        allowed_then_refused(
            &user,
            mcp.call(
                "forge",
                json!({"action": "edit", "number": number, "title": "feat: new title"}),
            ),
        )
    };

    let (merged, declined, superseded) = (edit(2), edit(3), edit(5));

    for (text, number, state) in [
        (&merged, 2, "merged"),
        (&declined, 3, "declined"),
        (&superseded, 5, "superseded"),
    ] {
        assert!(
            text.contains(&format!(
                "the pull request #{number} of acme/app is {state}, not open"
            )) && text.contains("nothing was written"),
            "{text}"
        );
    }
    assert!(
        !words(user.bkt_received())
            .iter()
            .any(|call| call.starts_with("pr edit"))
    );
}

#[test]
fn editing_an_open_pull_request_outside_the_newest_page_still_works() {
    let user = user();
    on_bitbucket(&user);
    // Fifty newer pull requests fill the page `bkt pr list` reads: #3 is not in it, #4 neither and
    // its page cannot be read.
    let mut pulls: Vec<Value> = (100..150).map(|id| cloud_pull(id, "OPEN")).collect();
    for id in [3, 4] {
        let mut older = cloud_pull(id, "OPEN");
        older["unlisted"] = json!(true);
        pulls.push(older);
    }
    let pages: serde_json::Map<String, Value> = [cloud_page(3, "OPEN")].into_iter().collect();
    user.bkt_script(json!({"pulls": {"acme/app": pulls}, "api": pages}));
    let (_, mcp) = root_in(&user, None);
    let calls = || words(user.bkt_received());

    for number in [3, 4] {
        let edit = mcp.call(
            "forge",
            json!({"action": "edit", "number": number, "body": "Adds the login page"}),
        );
        let done = judged_then_done(&user, edit, calls, &format!("pr edit {number} "));
        assert!(
            done.contains(&format!("Pull request #{number} of acme/app was changed.")),
            "{done}"
        );
    }

    let edits: Vec<String> = calls()
        .into_iter()
        .filter(|call| call.starts_with("pr edit"))
        .collect();
    assert_eq!(edits.len(), 2, "{edits:?}");
}

/// `pulls_to_close()` with the comments of a merged pull request (#7, merged by bob, who commented
/// a first time long before, and a second time as he merged), of a closed one (#9, closed by carol
/// with a comment, the issue's events saying who closed it), of one closed by dave, whose only
/// comment is of the day before (#8), of one closed by an app (#6), and of a merged one whose
/// merger only said "LGTM" hours earlier (#10).
fn closed_pulls() -> Value {
    let mut script = pulls_to_close();
    let comment = |login: &str, body: &str, at: &str| json!({"author": {"login": login}, "body": body, "createdAt": at});
    let held = &mut script["pulls"]["acme/app"];
    held["app/old"]["comments"] = json!([
        comment("bob", "LGTM", "2026-01-01T08:00:00Z"),
        comment("alice", "Looks good", "2026-01-01T09:00:00Z"),
        comment("bob", "Merging after the freeze", "2026-01-01T10:00:00Z"),
        comment("alice", "Thanks", "2026-01-01T11:00:00Z")
    ]);
    held["app/old"]["mergedBy"] = json!({"login": "bob"});
    held["app/old"]["mergedAt"] = json!("2026-01-01T10:00:01Z");
    held["app/closed"]["comments"] = json!([
        comment("carol", "Superseded by #20", "2026-01-02T09:59:59Z"),
        comment("alice", "Fine", "2026-01-02T12:00:00Z")
    ]);
    held["app/dropped"] = json!({"number": 8, "title": "Dropped too", "state": "CLOSED",
        "isDraft": false, "baseRefName": "main", "headRefName": "app/dropped",
        "url": "https://github.com/acme/app/pull/8",
        "comments": [comment("dave", "I will look at it", "2026-01-02T10:00:00Z"),
                     comment("alice", "Why?", "2026-01-03T09:00:00Z")]});
    held["app/stale"] = json!({"number": 6, "title": "Stale work", "state": "CLOSED",
        "isDraft": false, "baseRefName": "main", "headRefName": "app/stale",
        "url": "https://github.com/acme/app/pull/6",
        "comments": [comment("app/stale", "Closing for inactivity", "2026-01-04T09:59:30Z")]});
    held["app/lgtm"] = json!({"number": 10, "title": "Quiet merge", "state": "MERGED",
        "isDraft": false, "baseRefName": "main", "headRefName": "app/lgtm",
        "url": "https://github.com/acme/app/pull/10",
        "mergedBy": {"login": "bob"}, "mergedAt": "2026-01-05T10:00:00Z",
        "comments": [comment("bob", "LGTM", "2026-01-05T08:00:00Z")]});
    let event = |event: &str, login: &str, at: &str| json!({"event": event, "actor": {"login": login}, "created_at": at});
    script["events"] = json!({"acme/app": {
        "9": [event("labeled", "alice", "2026-01-02T08:00:00Z"),
              event("closed", "carol", "2026-01-02T10:00:00Z")],
        "8": [event("closed", "dave", "2026-01-03T10:00:00Z")],
        "6": [event("closed", "stale[bot]", "2026-01-04T10:00:00Z")]}});
    script
}

#[test]
fn show_of_a_closed_pull_request_has_its_closing_comment() {
    let user = user();
    on_github(&user);
    user.gh_script(closed_pulls());
    let (_, mcp) = root_in(&user, None);

    let show = |number: u64, detail: &str| {
        read(
            &user,
            mcp.call(
                "forge",
                json!({"action": "show", "number": number, "detail": detail}),
            ),
        )
    };
    let merged = show(7, "full");
    let closed = show(9, "short");
    let by_an_app = show(6, "full");
    let silent = show(8, "full");

    // The comment of whoever merged it as they did: not their old "LGTM", nor alice's thanks.
    assert!(
        merged.contains("closing comment (bob): Merging after the freeze"),
        "{merged}"
    );
    assert!(!merged.contains("closing comment (alice)"), "{merged}");
    assert!(
        closed.contains("closing comment (carol): Superseded by #20"),
        "{closed}"
    );
    // `gh` names an app `app/stale` where the events say `stale[bot]`.
    assert!(
        by_an_app.contains("closing comment (app/stale): Closing for inactivity"),
        "{by_an_app}"
    );
    // The forge gives none: nothing is said, not an empty line.
    assert!(!silent.contains("closing comment"), "{silent}");
    // The merger of a merged one is in the pull request itself: no event is read for it.
    let calls = words(user.gh_received());
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("api repos/acme/app/issues/9/events"))
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("api repos/acme/app/issues/7/"))
    );
}

#[test]
fn an_old_comment_of_the_merger_is_not_its_closing_comment() {
    let user = user();
    on_github(&user);
    user.gh_script(closed_pulls());
    let (_, mcp) = root_in(&user, None);

    let merged = read(
        &user,
        mcp.call(
            "forge",
            json!({"action": "show", "number": 10, "detail": "full"}),
        ),
    );

    assert!(merged.contains("bob: LGTM"), "{merged}");
    assert!(!merged.contains("closing comment"), "{merged}");
}

#[test]
fn show_of_an_open_pull_request_has_no_closing_comment_and_asks_no_event() {
    let user = user();
    on_github(&user);
    user.gh_script(closed_pulls());
    let (_, mcp) = root_in(&user, None);

    let open = read(
        &user,
        mcp.call("forge", json!({"action": "show", "number": 12})),
    );

    assert!(!open.contains("closing comment"), "{open}");
    assert!(
        !words(user.gh_received())
            .iter()
            .any(|call| call.starts_with("api repos"))
    );
}

#[test]
fn show_of_a_closed_pull_request_has_its_comment_count() {
    let user = user();
    on_github(&user);
    user.gh_script(closed_pulls());
    let (_, mcp) = root_in(&user, None);

    let short = read(
        &user,
        mcp.call("forge", json!({"action": "show", "number": 7})),
    );
    let full = read(
        &user,
        mcp.call(
            "forge",
            json!({"action": "show", "number": 7, "detail": "full"}),
        ),
    );

    assert!(short.contains("comments: 4, read them with"), "{short}");
    assert!(full.contains("comments (4):"), "{full}");
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
    assert!(
        calls.contains(
            &"pr edit 3 --context cloud --workspace acme --repo app --description Adds the login \
         page"
                .to_owned()
        )
    );
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
fn tickets_list_header_says_the_label_and_the_assignee_it_was_filtered_by() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    let listed = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "list", "label": "bug", "assignee": "ana"}),
        ),
    );

    let header = listed.lines().next().unwrap();
    assert!(
        header.starts_with("The open tickets of GitHub issues"),
        "{listed}"
    );
    assert!(
        header.ends_with(" labelled bug, assigned to ana: 1"),
        "{listed}"
    );
}

#[test]
fn tickets_list_with_the_state_all_reads_all_tickets_of() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    let listed = read(
        &user,
        mcp.call("tickets", json!({"action": "list", "state": "all"})),
    );

    let header = listed.lines().next().unwrap();
    assert!(
        header.starts_with("All tickets of GitHub issues"),
        "{listed}"
    );
    assert!(header.ends_with(": 45"), "{listed}");
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

    assert!(
        found.contains("matching \"login\" in the titles, descriptions and comments: 1"),
        "{found}"
    );
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
            "issue create -R github.com/acme/app --title Add a logout --body Users stay signed \
             in --label feature"
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
    // A list and a search ask nothing more; a show asks the work item with the check, and shows
    // nothing of it.
    assert!(
        user.acli_received().iter().all(|call| {
            call["args"] == json!(["jira", "auth", "status"])
                || call["args"].as_array().is_some_and(|args| {
                    args.starts_with(&[json!("jira"), json!("workitem"), json!("view")])
                })
        }),
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
        "types": {"ABC": ["Task", "Bug", "Story", {"name": "Sub-task", "subtask": true}],
            "DEF": ["Task", "Story", "Epic"]},
    }));
}

/// A write of `tickets` on Jira, held for the user then done once allowed: what it answered, and
/// the calls of `acli` that started with `verb`, as the fake recorded them.
fn jira_sent(args: Value, verb: &str) -> (String, Vec<Value>) {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);
    let calls = || words(user.acli_received());
    let done = judged_then_done(&user, mcp.call("tickets", args), calls, verb);
    let asked = user
        .acli_received()
        .into_iter()
        .filter(|call| words(vec![(*call).clone()])[0].starts_with(verb))
        .collect();
    // Every write is checked to go to the tracker's site first.
    assert_eq!(calls()[0], "jira auth status");
    (done, asked)
}

/// A write of `tickets` on Jira: what it answered, and the calls of `acli` that started with
/// `verb`, a file's path (which is a temporary one) written `<file>`.
fn jira_written(args: Value, verb: &str) -> (String, Vec<String>) {
    let (done, asked) = jira_sent(args, verb);
    let asked = asked
        .into_iter()
        .map(|mut call| {
            let args = call["args"].as_array_mut().unwrap();
            for at in 1..args.len() {
                if args[at - 1]
                    .as_str()
                    .is_some_and(|flag| flag.ends_with("-file"))
                {
                    args[at] = Value::from("<file>");
                }
            }
            words(vec![call]).remove(0)
        })
        .collect();
    (done, asked)
}

/// The read of the status of ABC-1 a move starts with.
const VIEW_OF_ABC_1: &str = "jira workitem view ABC-1 --json --fields status";

/// The search that finds the statuses the work items of ABC are in.
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
            "jira workitem create --project DEF --summary Add a logout --description-file <file> \
             --type Story --label feature,auth --json"
        ]
    );
}

/// The calls of `acli` that read a project's types.
fn project_views(user: &User) -> Vec<String> {
    words(user.acli_received())
        .into_iter()
        .filter(|call| call.starts_with("jira project view"))
        .collect()
}

#[test]
fn types_lists_a_jira_project_s_work_item_types() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);

    let first = read(
        &user,
        mcp.call("tickets", json!({"action": "types", "project": "abc"})),
    );
    let again = read(
        &user,
        mcp.call("tickets", json!({"action": "types", "project": "ABC"})),
    );

    assert!(
        first.contains("The work item types of ABC on example.atlassian.net are Task, Bug, Story and Sub-task (a sub-task)"),
        "{first}"
    );
    assert_eq!(again, first);
    // Cached for a few minutes: the second answer asked Jira nothing.
    assert_eq!(project_views(&user), ["jira project view --key ABC --json"]);
    // The tracker has two projects: which one is not guessed.
    let unsaid = mcp.call("tickets", json!({"action": "types"}));
    assert!(unsaid.error, "{}", unsaid.text);
    assert!(
        unsaid
            .text
            .contains("need project, one of the tracker's: ABC, DEF"),
        "{}",
        unsaid.text
    );
    assert_eq!(project_views(&user).len(), 1);
}

#[test]
fn types_on_github_says_issues_have_no_type_and_asks_nothing() {
    let user = user();
    on_github(&user);
    many_issues(&user);
    let (_, mcp) = root_in(&user, None);

    let said = read(&user, mcp.call("tickets", json!({"action": "types"})));

    assert!(said.contains("GitHub issues have no type"), "{said}");
    assert!(user.gh_received().is_empty(), "{:?}", user.gh_received());
}

#[test]
fn a_create_refused_for_its_type_answers_with_the_types() {
    let user = user();
    on_jira(&user);
    jira_items(&user);

    let (answer, calls) = jira_asked(
        &user,
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Tâche"}),
    );

    assert!(
        answer.contains(
            "\"Tâche\" is not a work item type of ABC: its types are Task, Bug, Story and Sub-task"
        ),
        "{answer}"
    );
    assert!(answer.contains("nothing was written"), "{answer}");
    // One attempt, then one read of the types: no other type was tried.
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("jira workitem create"))
            .count(),
        1
    );
    assert_eq!(calls.last().unwrap(), "jira project view --key ABC --json");
}

#[test]
fn create_assigns_a_jira_work_item_to_the_user() {
    let (done, asked) = jira_written(
        json!({"action": "create", "project": "def", "title": "Add a logout", "type": "Story",
            "assignee": "@me"}),
        "jira workitem create",
    );
    assert!(
        done.contains("The ticket DEF-7 was opened in DEF"),
        "{done}"
    );
    assert_eq!(
        asked,
        [
            "jira workitem create --project DEF --summary Add a logout --assignee @me --type Story \
          --json"
        ]
    );
    // Nobody is assigned unless the agent asks.
    let (_, unasked) = jira_written(
        json!({"action": "create", "project": "def", "title": "Add a logout"}),
        "jira workitem create",
    );
    assert!(!unasked[0].contains("--assignee"), "{unasked:?}");
}

#[test]
fn edit_assigns_and_unassigns_on_jira() {
    let (done, asked) = jira_written(
        json!({"action": "edit", "ticket": "ABC-1", "assignee": "ana@example.org"}),
        "jira workitem edit",
    );
    assert!(done.contains("ABC-1 was changed."), "{done}");
    assert_eq!(
        asked,
        ["jira workitem edit --key ABC-1 --assignee ana@example.org --yes"]
    );
    let (done, asked) = jira_written(
        json!({"action": "edit", "ticket": "ABC-1", "assignee": null}),
        "jira workitem edit",
    );
    assert!(done.contains("ABC-1 was changed."), "{done}");
    assert_eq!(
        asked,
        ["jira workitem edit --key ABC-1 --remove-assignee --yes"]
    );
}

#[test]
fn github_accepts_at_me_as_an_assignee() {
    let (done, asked) = ticket_written(
        json!({"action": "edit", "ticket": "#2", "assignees": ["@me"]}),
        "issue edit",
    );
    assert!(done.contains("#2 of acme/app was changed."), "{done}");
    assert_eq!(
        asked,
        ["issue edit 2 -R github.com/acme/app --add-assignee @me"]
    );
}

#[test]
fn the_card_of_a_ticket_write_says_the_assignee() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);
    let says = |args: Value| {
        held(&mcp.call("tickets", args));
        let requests = user.call("approval.list", json!({})).unwrap()["requests"].clone();
        requests.as_array().unwrap().last().unwrap()["says"].clone()
    };

    let assigned = says(json!({"action": "edit", "ticket": "ABC-1", "assignee": "@me"}));
    let removed = says(json!({"action": "edit", "ticket": "ABC-1", "assignee": null}));

    assert!(
        assigned.to_string().contains("It assigns it to you"),
        "{assigned}"
    );
    assert!(
        removed.to_string().contains("It removes the assignee"),
        "{removed}"
    );
}

#[test]
fn an_assignee_where_the_tracker_or_the_action_takes_none_is_refused_before_anything_is_asked() {
    let jira = user();
    on_jira(&jira);
    jira_items(&jira);
    let (_, mcp) = root_in(&jira, None);
    let github = user();
    on_github(&github);
    many_issues(&github);
    let (_, on_github) = root_in(&github, None);

    for (mcp, args, refusal) in [
        (
            &mcp,
            json!({"action": "close", "ticket": "ABC-1", "assignee": "@me"}),
            "refused: tickets close takes no assignee: assignee is for create and edit",
        ),
        (
            &on_github,
            json!({"action": "edit", "ticket": "#1", "assignee": "ana"}),
            "refused: tickets edit on GitHub takes assignees, a list of logins, not assignee",
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
    assert!(no_request(&jira) && no_request(&github));
    assert!(jira.acli_received().is_empty() && github.gh_received().is_empty());
}

/// The Jira tracker with its plans: ABC's board 7 with a closed, an active and a future sprint,
/// ABC-4 in the active one (on the site's Sprint field, `customfield_10020`), assigned and under
/// the epic ABC-10, and the one user `acli` knows besides the signed-in one.
fn jira_planned(user: &User) {
    jira_planned_with(user, |_| {});
}

/// [`jira_planned`], its script changed by `change` first.
fn jira_planned_with(user: &User, change: impl FnOnce(&mut Value)) {
    let site = "example.atlassian.net";
    let todo = in_status(work_item("ABC-1", "The login fails", site), "To Do", "new");
    let mut planned = work_item("ABC-4", "Remember the user", site);
    planned["fields"]["assignee"] = json!({"displayName": "Anne Example"});
    planned["fields"]["parent"] = json!({"key": "ABC-10", "fields": {"summary": "Sign-in"}});
    planned["fields"]["customfield_10020"] = json!([
        {"id": 41, "name": "Sprint 41", "state": "closed", "boardId": 7},
        {"id": 42, "name": "Sprint 42", "state": "active", "boardId": 7}]);
    let mut script = json!({
        "items": {"ABC-1": todo, "ABC-4": planned,
            "ABC-10": work_item("ABC-10", "Sign-in", site)},
        "created": "ABC-11",
        "users": ["ana@example.org"],
        "boards": {"ABC": [{"id": 7, "name": "ABC board", "type": "scrum"}]},
        "sprints": {"7": [
            {"id": 41, "name": "Sprint 41", "state": "closed"},
            {"id": 42, "name": "Sprint 42", "state": "active",
                "endDate": "2026-11-20T10:00:00.000Z"},
            {"id": 43, "name": "Sprint 43", "state": "future"}]},
    });
    change(&mut script);
    user.acli_script(script);
}

#[test]
fn a_jira_item_is_assigned_to_me() {
    let (done, asked) = jira_written(
        json!({"action": "edit", "ticket": "ABC-1", "assignee": "me"}),
        "jira workitem edit",
    );
    assert!(done.contains("ABC-1 was changed."), "{done}");
    assert_eq!(
        asked,
        ["jira workitem edit --key ABC-1 --assignee @me --yes"]
    );
}

#[test]
fn a_jira_item_gets_and_loses_labels() {
    let (done, asked) = jira_written(
        json!({"action": "edit", "ticket": "ABC-1", "labels": ["bug", "p1"],
            "remove_labels": ["wontfix"]}),
        "jira workitem edit",
    );
    assert!(done.contains("ABC-1 was changed."), "{done}");
    // `--labels` adds, as measured on a real site: the labels the item had stay.
    assert_eq!(
        asked,
        ["jira workitem edit --key ABC-1 --labels bug,p1 --remove-labels wontfix --yes"]
    );
}

#[test]
fn an_unknown_assignee_is_refused() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    // A display name: acli takes none, so nothing is asked of anyone.
    for action in ["create", "edit"] {
        let refused = mcp.call(
            "tickets",
            json!({"action": action, "ticket": "ABC-1", "project": "ABC", "title": "Add a logout",
                "assignee": "Kristen"}),
        );
        assert!(refused.error, "{}", refused.text);
        assert!(
            refused.text.contains(
                "assignee is me, an email or an account id; a display name cannot be looked up \
                 with acli"
            ),
            "{}",
            refused.text
        );
    }
    assert!(no_request(&user));
    assert!(user.acli_received().is_empty());

    // An email the site does not know: the tracker's words, and nothing was created.
    let (created, calls) = jira_asked(
        &user,
        json!({"action": "create", "project": "ABC", "title": "Add a logout",
            "assignee": "bob@example.org"}),
    );
    assert!(
        created.contains("acli refused the creation assigned to \"bob@example.org\""),
        "{created}"
    );
    assert!(
        created.contains("User not found for email: bob@example.org"),
        "{created}"
    );
    assert!(created.contains("nothing was created"), "{created}");
    let creates = |calls: &[String]| {
        calls
            .iter()
            .filter(|call| call.starts_with("jira workitem create"))
            .count()
    };
    // Not tried again without its assignee.
    assert_eq!(creates(&calls), 1, "{calls:?}");

    // An edit: acli ends with 0 and says its failure, which is not a change.
    let (edited, _) = jira_asked(
        &user,
        json!({"action": "edit", "ticket": "ABC-1", "assignee": "bob@example.org"}),
    );
    assert!(!edited.contains("ABC-1 was changed"), "{edited}");
    assert!(edited.contains("can't be edited"), "{edited}");
    assert!(edited.contains("bob@example.org"), "{edited}");
}

#[test]
fn a_github_issue_is_assigned_and_labelled() {
    let (done, asked) = ticket_written(
        json!({"action": "edit", "ticket": "#2", "assignees": ["me"], "labels": ["bug"]}),
        "issue edit",
    );
    assert!(done.contains("#2 of acme/app was changed."), "{done}");
    assert_eq!(
        asked,
        ["issue edit 2 -R github.com/acme/app --add-label bug --add-assignee @me"]
    );
}

/// The card the user is shown for the last request held.
fn last_card(user: &User) -> String {
    let requests = user.call("approval.list", json!({})).unwrap()["requests"].clone();
    requests.as_array().unwrap().last().unwrap()["says"].to_string()
}

#[test]
fn a_jira_item_is_created_assigned_under_a_parent_in_a_sprint() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "create", "project": "ABC", "title": "Add a logout",
            "body": "Users stay signed in", "type": "Story", "labels": ["auth"],
            "assignee": "me", "parent": "abc-10", "sprint": "sprint 42"}),
    ));
    let card = last_card(&user);
    assert!(card.contains("It assigns it to you"), "{card}");
    // The sprint as the site names it, found before the user is asked.
    assert!(
        card.contains("It puts it in sprint \\\"Sprint 42\\\" (board \\\"ABC board\\\")"),
        "{card}"
    );
    assert!(card.contains("It puts it under ABC-10"), "{card}");
    let not_auth = |calls: Vec<String>| -> Vec<String> {
        calls
            .into_iter()
            .filter(|call| call != "jira auth status")
            .collect()
    };
    let before = not_auth(words(user.acli_received()));
    assert_eq!(
        before,
        [
            "jira board search --project ABC --json",
            "jira board list-sprints --id 7 --state active,future --json",
            "jira workitem search --jql project in (\"ABC\", \"DEF\") AND sprint is not EMPTY \
             ORDER BY updated DESC --fields key,summary --json --limit 1",
            "jira workitem view ABC-4 --json --fields *navigable",
        ]
    );
    let done = allow(&user, number);

    assert!(
        done.contains(
            "The ticket ABC-11 was opened in ABC, in sprint \"Sprint 42\", under ABC-10, \
             assigned to you: https://example.atlassian.net/browse/ABC-11"
        ),
        "{done}"
    );
    let calls = not_auth(words(user.acli_received()));
    let after = &calls[before.len()..];
    // Read again once allowed (it may have closed since); the Sprint field is kept.
    assert_eq!(
        after[..2],
        [
            "jira board search --project ABC --json",
            "jira board list-sprints --id 7 --state active,future --json",
        ],
        "{calls:?}"
    );
    assert!(
        after[2].starts_with("jira workitem create --from-json "),
        "{calls:?}"
    );
    // `@me` is acli's flag's and not its JSON's: the user is assigned right after, by its flag.
    assert_eq!(
        after[3],
        "jira workitem edit --key ABC-11 --assignee @me --yes"
    );
    assert_eq!(after.len(), 4, "{calls:?}");
    let created = user
        .acli_received()
        .into_iter()
        .find(|call| call["args"][2] == "create")
        .unwrap();
    let item = &created["files"][0]["json"];
    assert_eq!(item["projectKey"], "ABC");
    assert_eq!(item["summary"], "Add a logout");
    assert_eq!(item["type"], "Story");
    assert_eq!(item["labels"], json!(["auth"]));
    assert_eq!(item["parentIssueId"], "ABC-10");
    assert_eq!(
        item["additionalAttributes"],
        json!({"customfield_10020": 42})
    );
    assert_eq!(item["description"]["type"], "doc");
    assert!(item.get("assignee").is_none(), "{item}");
    // The file is private, as a description's is.
    assert_eq!(created["files"][0]["mode"], 0o600);

    // Under a parent alone, acli's flags do it.
    let (_, parented) = jira_written(
        json!({"action": "create", "project": "def", "title": "Add a logout",
            "parent": "DEF-2"}),
        "jira workitem create",
    );
    assert_eq!(
        parented,
        ["jira workitem create --project DEF --summary Add a logout --parent DEF-2 --json"]
    );
}

/// A `tickets` call refused before the user is asked: its refusal.
fn refused_unasked(user: &User, mcp: &Mcp, args: Value) -> String {
    let refused = mcp.call("tickets", args.clone());
    assert!(refused.error, "{args}: {}", refused.text);
    assert!(no_request(user), "{args}");
    assert!(
        !words(user.acli_received())
            .iter()
            .any(|call| call.starts_with("jira workitem create")
                || call.starts_with("jira workitem edit")),
        "{args}"
    );
    refused.text
}

#[test]
fn a_sprint_that_is_not_open_on_the_project_s_boards_is_refused_naming_those_that_are() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    let answer = refused_unasked(
        &user,
        &mcp,
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Task",
            "sprint": "Sprint 41"}),
    );

    assert!(
        answer.contains("\"Sprint 41\" is no open sprint of ABC"),
        "{answer}"
    );
    assert!(
        answer.contains("Sprint 42") && answer.contains("Sprint 43"),
        "{answer}"
    );
    assert!(answer.contains("nothing was written"), "{answer}");
}

#[test]
fn the_card_names_the_sprint_an_id_resolves_to_and_an_unknown_one_is_refused_unasked() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    let unknown = refused_unasked(
        &user,
        &mcp,
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Task",
            "sprint": "57"}),
    );
    held(&mcp.call(
        "tickets",
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Task",
            "sprint": "43"}),
    ));
    let card = last_card(&user);

    assert!(
        unknown.contains("\"57\" is no open sprint of ABC"),
        "{unknown}"
    );
    assert!(
        card.contains("It puts it in sprint \\\"Sprint 43\\\" (board \\\"ABC board\\\")"),
        "{card}"
    );
    assert!(!card.contains("sprint \\\"43\\\""), "{card}");
}

#[test]
fn a_sprint_two_boards_share_is_refused_unasked_asking_for_its_id() {
    let user = user();
    on_jira(&user);
    jira_planned_with(&user, |script| {
        script["boards"]["ABC"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": 9, "name": "Ops board", "type": "scrum"}));
        script["sprints"]["9"] = json!([{"id": 90, "name": "Sprint 42", "state": "active"}]);
    });
    let (_, mcp) = root_in(&user, None);

    let answer = refused_unasked(
        &user,
        &mcp,
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Task",
            "sprint": "Sprint 42"}),
    );

    assert!(
        answer.contains("names several open sprints of ABC"),
        "{answer}"
    );
    assert!(answer.contains("give its id as sprint"), "{answer}");
}

#[test]
fn a_creation_in_a_sprint_on_a_site_whose_sprint_field_is_unknown_is_refused_unasked() {
    let user = user();
    on_jira(&user);
    jira_planned_with(&user, |script| {
        // No work item has been in a sprint: the field cannot be found.
        script["items"]["ABC-4"]["fields"]
            .as_object_mut()
            .unwrap()
            .remove("customfield_10020");
    });
    let (_, mcp) = root_in(&user, None);

    let answer = refused_unasked(
        &user,
        &mcp,
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Task",
            "sprint": "Sprint 42"}),
    );

    assert!(
        answer.contains("the Sprint field of example.atlassian.net is not known"),
        "{answer}"
    );
    assert!(answer.contains("nothing was written"), "{answer}");
}

#[test]
fn a_creation_acli_answers_without_its_key_says_what_may_not_be_applied() {
    let user = user();
    on_jira(&user);
    jira_planned_with(&user, |script| {
        script["created_said"] = Value::from("✓ Work item created");
    });

    let (answer, calls) = jira_asked(
        &user,
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Task",
            "assignee": "me", "parent": "ABC-10", "sprint": "Sprint 42"}),
    );

    assert!(!answer.contains("The ticket was opened"), "{answer}");
    assert!(
        answer.contains("without the key of the new work item"),
        "{answer}"
    );
    assert!(answer.contains("sprint \"Sprint 42\""), "{answer}");
    assert!(answer.contains("parent ABC-10"), "{answer}");
    assert!(answer.contains("not assigned to you"), "{answer}");
    assert!(answer.contains("✓ Work item created"), "{answer}");
    // No key, nothing to assign.
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("jira workitem edit")),
        "{calls:?}"
    );
}

#[test]
fn a_created_item_acli_does_not_assign_is_said_opened_without_its_assignee() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);

    let (answer, _) = jira_asked(
        &user,
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Task",
            "assignee": "bob@example.org", "sprint": "Sprint 42"}),
    );

    assert!(
        answer.contains(
            "The ticket ABC-11 was opened in ABC, in sprint \"Sprint 42\", without its assignee"
        ),
        "{answer}"
    );
    assert!(
        answer.contains("acli did not assign it to \"bob@example.org\""),
        "{answer}"
    );
    assert!(answer.contains("can't be edited"), "{answer}");
    assert!(
        answer.contains("https://example.atlassian.net/browse/ABC-11"),
        "{answer}"
    );
}

#[test]
fn the_card_of_a_jira_edit_says_the_labels_added_and_removed() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    held(&mcp.call(
        "tickets",
        json!({"action": "edit", "ticket": "ABC-1", "labels": ["bug", "p1"],
            "remove_labels": ["wontfix"]}),
    ));
    let card = last_card(&user);

    assert!(card.contains("It adds the labels bug, p1"), "{card}");
    assert!(card.contains("It removes the labels wontfix"), "{card}");
}

#[test]
fn a_label_holding_a_comma_is_refused_before_anything_is_asked() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    for args in [
        json!({"action": "edit", "ticket": "ABC-1", "labels": ["front,back"]}),
        json!({"action": "edit", "ticket": "ABC-1", "remove_labels": ["a,b"]}),
        json!({"action": "create", "project": "ABC", "title": "T", "labels": ["front,back"]}),
    ] {
        let refused = refused_unasked(&user, &mcp, args);
        assert!(refused.contains("a label holds no comma"), "{refused}");
    }
}

#[test]
fn blank_labels_are_left_out_on_every_path() {
    let (_, flags) = jira_written(
        json!({"action": "create", "project": "def", "title": "Add a logout",
            "labels": ["auth", " ", ""]}),
        "jira workitem create",
    );
    assert_eq!(
        flags,
        ["jira workitem create --project DEF --summary Add a logout --label auth --json"]
    );
    let (_, edited) = jira_written(
        json!({"action": "edit", "ticket": "ABC-1", "labels": [" ", "bug"],
            "remove_labels": [""]}),
        "jira workitem edit",
    );
    assert_eq!(
        edited,
        ["jira workitem edit --key ABC-1 --labels bug --yes"]
    );

    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, calls) = jira_asked(
        &user,
        json!({"action": "create", "project": "ABC", "title": "Add a logout", "type": "Task",
            "labels": ["auth", " "], "sprint": "Sprint 42"}),
    );
    assert!(
        calls.iter().any(|c| c.starts_with("jira workitem create")),
        "{calls:?}"
    );
    let created = user
        .acli_received()
        .into_iter()
        .find(|call| call["args"][2] == "create")
        .unwrap();
    assert_eq!(created["files"][0]["json"]["labels"], json!(["auth"]));
}

#[test]
fn board_is_refused_on_another_action_than_sprints() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    for args in [
        json!({"action": "show", "ticket": "ABC-1", "board": "ABC board"}),
        json!({"action": "edit", "ticket": "ABC-1", "title": "T", "board": "ABC board"}),
    ] {
        let refused = mcp.call("tickets", args.clone());
        assert!(refused.error, "{args}: {}", refused.text);
        assert!(
            refused.text.contains(&format!(
                "tickets {} takes no board: board is for sprints",
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
fn a_creation_in_a_sprint_without_its_type_is_refused_before_anything_is_asked() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    let refused = mcp.call(
        "tickets",
        json!({"action": "create", "project": "ABC", "title": "Add a logout",
            "sprint": "Sprint 42"}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(refused.text.contains("needs type"), "{}", refused.text);
    assert!(no_request(&user));
    assert!(user.acli_received().is_empty());
}

#[test]
fn the_sprints_of_the_project_s_board_are_listed() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    let said = read(
        &user,
        mcp.call("tickets", json!({"action": "sprints", "project": "abc"})),
    );

    assert!(
        said.contains("The open sprints of ABC on example.atlassian.net"),
        "{said}"
    );
    assert!(said.contains("Board \"ABC board\" (7):"), "{said}");
    assert!(
        said.contains("- Sprint 42 [id 42], active, ends 2026-11-20"),
        "{said}"
    );
    assert!(said.contains("- Sprint 43 [id 43], future"), "{said}");
    assert!(!said.contains("Sprint 41"), "{said}");
    let calls = words(user.acli_received());
    assert!(
        calls.contains(&"jira board list-sprints --id 7 --state active,future --json".to_owned()),
        "{calls:?}"
    );

    // A project whose board has no sprint says so, in acli's words.
    user.acli_script(
        json!({"boards": {"ABC": [{"id": 1, "name": "ABC board", "type": "simple"}]}}),
    );
    let none = read(
        &user,
        mcp.call("tickets", json!({"action": "sprints", "project": "ABC"})),
    );
    assert!(none.contains("does not support sprints"), "{none}");
}

#[test]
fn changing_the_parent_of_an_existing_item_is_refused_saying_why() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    let refused = mcp.call(
        "tickets",
        json!({"action": "edit", "ticket": "ABC-1", "parent": "ABC-10"}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused.text.contains("acli's workitem edit has no parent"),
        "{}",
        refused.text
    );
    assert!(refused.text.contains("Jira's REST API"), "{}", refused.text);
    assert!(
        refused.text.contains("nothing was written"),
        "{}",
        refused.text
    );
    assert!(no_request(&user));
    assert!(user.acli_received().is_empty());
}

#[test]
fn moving_an_existing_item_to_a_sprint_is_refused_saying_why() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    let refused = mcp.call(
        "tickets",
        json!({"action": "edit", "ticket": "ABC-1", "sprint": "Sprint 42"}),
    );
    // The action that was always refused is gone.
    let gone = mcp.call(
        "tickets",
        json!({"action": "sprint", "ticket": "ABC-1", "sprint": "Sprint 42"}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused
            .text
            .contains("acli's workitem edit takes no sprint"),
        "{}",
        refused.text
    );
    assert!(refused.text.contains("on the board"), "{}", refused.text);
    assert!(
        refused.text.contains("nothing was written"),
        "{}",
        refused.text
    );
    assert!(gone.error, "{}", gone.text);
    assert!(
        gone.text.contains("tickets has no action \"sprint\""),
        "{}",
        gone.text
    );
    assert!(no_request(&user));
    assert!(user.acli_received().is_empty());
}

#[test]
fn show_says_the_assignee_parent_and_sprint() {
    let user = user();
    on_jira(&user);
    jira_planned(&user);
    let (_, mcp) = root_in(&user, None);

    let shown = read(
        &user,
        mcp.call("tickets", json!({"action": "show", "ticket": "ABC-4"})),
    );
    let unplanned = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "ABC-10", "fresh": true}),
        ),
    );

    assert!(shown.contains("Assignee: Anne Example"), "{shown}");
    assert!(shown.contains("Parent: ABC-10 Sign-in"), "{shown}");
    assert!(shown.contains("Sprint: Sprint 42 (active)"), "{shown}");
    assert!(!shown.contains("Sprint 41"), "{shown}");
    assert!(unplanned.contains("Assignee: Ana"), "{unplanned}");
    assert!(!unplanned.contains("Parent:"), "{unplanned}");
    assert!(!unplanned.contains("Sprint:"), "{unplanned}");
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
        ["jira workitem comment create --key ABC-1 --body-file <file>"]
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
            "jira workitem edit --key ABC-1 --summary The logout fails --description-file \
             <file> --yes"
        ]
    );
}

/// A Markdown with a heading, marks, a list and a code block: what an agent writes.
const MARKDOWN: &str = "## Goal\n\nLet users **log in** with `code`.\n\n- a form\n- a session\n\n```rust\nfn main() {}\n```";

/// The types of the blocks of an ADF document.
fn block_types(adf: &Value) -> Vec<&str> {
    adf["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["type"].as_str().unwrap())
        .collect()
}

/// The one file a call of the fake `acli` was handed: it is private, and gone once the call is
/// done, the fake having read it (and checked it is valid ADF, ending with 1 if not).
fn file_sent<'a>(call: &'a Value, flag: &str) -> &'a Value {
    let args: Vec<&str> = call["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(args.contains(&flag), "{flag} not in {args:?}");
    assert!(
        !args
            .iter()
            .any(|a| matches!(*a, "--description" | "--body")),
        "the text must not be on the command line: {args:?}"
    );
    let file = &call["files"][0];
    assert_eq!(file["flag"], flag);
    assert_eq!(file["mode"], 0o600, "{file}");
    assert_eq!(file["dir_mode"].as_u64().unwrap() & 0o077, 0, "{file}");
    assert!(
        !Path::new(file["path"].as_str().unwrap()).exists(),
        "{file}"
    );
    file
}

#[test]
fn a_jira_description_written_in_markdown_is_sent_as_adf() {
    let (done, asked) = jira_sent(
        json!({"action": "create", "project": "def", "title": "Add a logout", "body": MARKDOWN}),
        "jira workitem create",
    );
    assert!(
        done.contains("The ticket DEF-7 was opened in DEF"),
        "{done}"
    );
    let adf = &file_sent(&asked[0], "--description-file")["adf"];
    assert_eq!(
        block_types(adf),
        ["heading", "paragraph", "bulletList", "codeBlock"]
    );
    assert_eq!(adf["content"][0]["attrs"]["level"], 2);
    assert_eq!(
        adf["content"][1]["content"][1],
        json!({"type": "text", "text": "log in", "marks": [{"type": "strong"}]})
    );
    assert_eq!(adf["content"][3]["attrs"]["language"], "rust");
}

#[test]
fn a_jira_edit_and_a_jira_comment_are_sent_as_adf() {
    let (done, asked) = jira_sent(
        json!({"action": "edit", "ticket": "ABC-1", "title": "The logout fails", "body": MARKDOWN}),
        "jira workitem edit",
    );
    assert!(done.contains("ABC-1 was changed."), "{done}");
    let adf = &file_sent(&asked[0], "--description-file")["adf"];
    assert_eq!(
        block_types(adf),
        ["heading", "paragraph", "bulletList", "codeBlock"]
    );

    let (done, asked) = jira_sent(
        json!({"action": "comment", "ticket": "ABC-1", "body": MARKDOWN}),
        "jira workitem comment",
    );
    assert!(done.contains("The comment was posted on ABC-1."), "{done}");
    let adf = &file_sent(&asked[0], "--body-file")["adf"];
    assert_eq!(
        block_types(adf),
        ["heading", "paragraph", "bulletList", "codeBlock"]
    );
}

#[test]
fn plain_text_is_sent_as_one_paragraph_per_block() {
    let (_, asked) = jira_sent(
        json!({"action": "comment", "ticket": "ABC-1",
            "body": "Fixed on app/login\nthen merged\n\nNothing else to do"}),
        "jira workitem comment",
    );
    let adf = &file_sent(&asked[0], "--body-file")["adf"];
    assert_eq!(
        adf["content"],
        json!([
            {"type": "paragraph", "content": [
                {"type": "text", "text": "Fixed on app/login"}, {"type": "hardBreak"},
                {"type": "text", "text": "then merged"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "Nothing else to do"}]},
        ])
    );
}

#[test]
fn what_cannot_be_converted_is_kept_as_text() {
    let (_, asked) = jira_sent(
        json!({"action": "comment", "ticket": "ABC-1",
            "body": "<div class=\"x\">raw</div>\n\nA logo ![logo](https://example.com/l.png) here"}),
        "jira workitem comment",
    );
    let adf = &file_sent(&asked[0], "--body-file")["adf"];
    assert_eq!(
        adf["content"],
        json!([
            {"type": "paragraph", "content": [{"type": "text", "text": "<div class=\"x\">raw</div>"}]},
            {"type": "paragraph", "content": [{"type": "text",
                "text": "A logo ![logo](https://example.com/l.png) here"}]},
        ])
    );
}

/// A Markdown with one of each kind converted.
const EVERYTHING: &str = "## Plan\n\nDo **this** and *that*, not ~~those~~, see \
    [docs](https://example.com/a) and `code`.\nA second line.\n\n1. one\n2. two\n   - nested\n\n\
    > quoted\n\n---\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n```rust\nfn main() {}\n```";

#[test]
fn a_jira_description_read_back_is_the_markdown_written() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "ABC-1", "body": EVERYTHING}),
    );
    judged_then_done(
        &user,
        answer,
        || words(user.acli_received()),
        "jira workitem comment",
    );

    let shown = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
        ),
    );
    // `show` indents the lines that follow a comment's first.
    assert!(
        shown.replace("\n    ", "\n").contains(EVERYTHING),
        "{shown}"
    );
}

#[test]
fn a_jira_work_item_shows_its_attachments_and_names_the_images_of_its_texts() {
    let user = user();
    on_jira(&user);
    let mut item = in_status(
        work_item("ABC-1", "The login fails", "example.atlassian.net"),
        "To Do",
        "new",
    );
    item["fields"]["description"] = json!({"type": "doc", "version": 1, "content": [
        {"type": "mediaSingle", "content": [{"type": "media", "attrs": {
            "alt": "image-20260101-120000.png", "type": "file",
            "id": "00000000-0000-4000-8000-000000000001"}}]}]});
    item["fields"]["attachment"] = json!([{"id": "10001", "filename": "image-20260101-120000.png",
        "mimeType": "image/png", "size": 97717, "created": "2026-01-01T12:00:00.000+0100",
        "author": {"displayName": "Ana Example"},
        "content": "https://example.atlassian.net/rest/api/3/attachment/content/10001"}]);
    user.acli_script(json!({"items": {"ABC-1": item}}));
    let (_, mcp) = root_in(&user, None);

    let shown = read(
        &user,
        mcp.call("tickets", json!({"action": "show", "ticket": "ABC-1"})),
    );

    for part in [
        "[image: image-20260101-120000.png, not shown: ask the user to paste it if it matters]",
        "Attachments (1)",
        "- image-20260101-120000.png, image/png, 95.4 KiB, by Ana Example, 2026-01-01T12:00:00.000+0100",
    ] {
        assert!(shown.contains(part), "{part} in {shown}");
    }
    assert!(!shown.contains("attachment/content"), "{shown}");
    assert!(
        words(user.acli_received())
            .contains(&"jira workitem view ABC-1 --json --fields summary,status,description,comment,issuelinks,attachment,assignee,parent,*navigable".to_owned()),
        "{:?}",
        user.acli_received()
    );
}

#[test]
fn a_post_flottille_wrote_on_jira_is_still_recognised() {
    // The text of a comment that Jira keeps as a tree, with a link, a table and marks, is found
    // by the record after the round trip, not by the Markdown typed.
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "ABC-1", "body": EVERYTHING}),
    );
    judged_then_done(
        &user,
        answer,
        || words(user.acli_received()),
        "jira workitem comment",
    );

    let shown = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
        ),
    );
    assert!(
        shown.contains("Casey Example (posted by an agent)"),
        "{shown}"
    );
}

#[test]
fn github_and_bitbucket_writes_are_not_converted() {
    let body = "## Done\n\n**Rebased** on `main`\n\n- a\n- b";
    let user = user();
    on_github(&user);
    user.gh_script(pulls());
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call(
        "forge",
        json!({"action": "comment", "number": 12, "body": body}),
    );
    judged_then_done(&user, answer, || words(user.gh_received()), "pr comment");
    assert!(words(user.gh_received()).contains(&format!(
        "pr comment 12 -R github.com/acme/app --body {body}"
    )));

    let user = self::user();
    on_bitbucket(&user);
    user.bkt_script(
        json!({"pulls": {"acme/app": [{"id": 3, "title": "Work", "state": "OPEN",
        "source": {"branch": {"name": "app/login"}, "repository": {"full_name": "acme/app"}},
        "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}}}]}}),
    );
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call(
        "forge",
        json!({"action": "comment", "number": 3, "body": body}),
    );
    judged_then_done(&user, answer, || words(user.bkt_received()), "pr comment");
    assert!(words(user.bkt_received()).contains(&format!(
        "pr comment 3 --context cloud --workspace acme --repo app --text {body}"
    )));
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

    // Jira's spelling of the status, read back, not the agent's.
    assert!(done.contains("ABC-1 was moved to In Progress."), "{done}");
    assert_eq!(
        calls(),
        [
            "jira auth status",
            VIEW_OF_ABC_1,
            "jira workitem transition --key ABC-1 --status in progress --yes",
            VIEW_OF_ABC_1
        ]
    );
}

/// A tracker where every work item is to do and the done status is no work item's yet; ABC-1 can
/// move to the given statuses (invented workflow names).
fn jira_all_to_do(user: &User, moves: Value) {
    jira_all_to_do_with(user, moves, json!({}));
}

/// [`jira_all_to_do`], `more` added to the script of the fake `acli`.
fn jira_all_to_do_with(user: &User, moves: Value, more: Value) {
    let site = "example.atlassian.net";
    let first = in_status(work_item("ABC-1", "The login fails", site), "To Do", "new");
    let second = in_status(work_item("ABC-2", "Add search", site), "To Do", "new");
    let mut script = json!({
        "items": {"ABC-1": first, "ABC-2": second},
        "search": [first, second],
        "transitions": {"ABC-1": moves},
    });
    for (name, value) in more.as_object().unwrap() {
        script[name] = value.clone();
    }
    user.acli_script(script);
}

/// `args` on Jira, held for the user then allowed: what the agent is handed, and the calls of
/// `acli`.
fn jira_asked(user: &User, args: Value) -> (String, Vec<String>) {
    let (_, mcp) = root_in(user, None);
    let answer = allow(user, held(&mcp.call("tickets", args)));
    (answer, words(user.acli_received()))
}

fn transitions_of(calls: &[String]) -> Vec<&String> {
    calls
        .iter()
        .filter(|call| call.starts_with("jira workitem transition"))
        .collect()
}

#[test]
fn a_jira_item_moves_to_a_status_no_item_has_yet() {
    let user = user();
    on_jira(&user);
    jira_all_to_do(&user, json!(["In Progress", "Done"]));

    let (answer, calls) = jira_asked(
        &user,
        json!({"action": "status", "ticket": "ABC-1", "status": "Done"}),
    );

    assert!(answer.contains("ABC-1 was moved to Done."), "{answer}");
    // Jira's transition decides: no guess among the statuses the items are in.
    assert_eq!(
        calls,
        [
            "jira auth status",
            VIEW_OF_ABC_1,
            "jira workitem transition --key ABC-1 --status Done --yes",
            VIEW_OF_ABC_1
        ]
    );
}

#[test]
fn close_takes_the_transition_to_a_done_status() {
    // The project's one done status seen is not named Done: it is the one taken.
    let user = user();
    on_jira(&user);
    let site = "example.atlassian.net";
    let todo = in_status(work_item("ABC-1", "The login fails", site), "To Do", "new");
    let shipped = in_status(work_item("ABC-3", "Shipped", site), "Shipped", "done");
    user.acli_script(json!({
        "items": {"ABC-1": todo, "ABC-3": shipped},
        "search": [todo, shipped],
        "transitions": {"ABC-1": ["In Progress", "Shipped"]},
    }));

    let (answer, calls) = jira_asked(&user, json!({"action": "close", "ticket": "ABC-1"}));

    assert!(
        answer.contains("ABC-1 was closed: moved to Shipped."),
        "{answer}"
    );
    assert_eq!(
        calls,
        [
            "jira auth status",
            VIEW_OF_ABC_1,
            STATUSES_OF_ABC,
            "jira workitem transition --key ABC-1 --status Shipped --yes",
            VIEW_OF_ABC_1
        ]
    );
}

#[test]
fn close_with_no_done_status_in_sight_asks_jira_for_done() {
    let user = user();
    on_jira(&user);
    jira_all_to_do(&user, json!(["Done"]));

    let (answer, calls) = jira_asked(&user, json!({"action": "close", "ticket": "ABC-1"}));

    assert!(
        answer.contains("ABC-1 was closed: moved to Done."),
        "{answer}"
    );
    assert_eq!(
        transitions_of(&calls),
        ["jira workitem transition --key ABC-1 --status Done --yes"]
    );
}

#[test]
fn close_on_a_closed_item_says_it_is_already_closed() {
    let user = user();
    on_jira(&user);
    jira_items(&user);

    let (answer, calls) = jira_asked(&user, json!({"action": "close", "ticket": "ABC-3"}));

    assert!(
        answer.contains("ABC-3 is already closed: it is in Done; nothing was written"),
        "{answer}"
    );
    assert!(transitions_of(&calls).is_empty(), "{calls:?}");
}

#[test]
fn a_status_acli_refuses_is_reported_with_its_line_and_the_statuses_seen_and_nothing_is_written() {
    let user = user();
    on_jira(&user);
    jira_items(&user);

    let (answer, calls) = jira_asked(
        &user,
        json!({"action": "status", "ticket": "ABC-1", "status": "Blocked"}),
    );

    assert!(
        answer.contains("acli could not move ABC-1 from To Do to Blocked"),
        "{answer}"
    );
    // acli's own line, and what the work items of ABC are in, said apart from the transitions
    // (acli lists none: `docs/tickets.md`).
    assert!(
        answer.contains("no transition to status Blocked for work item ABC-1"),
        "{answer}"
    );
    assert!(
        answer.contains("seen on ABC's work items: To Do, In Progress and Done"),
        "{answer}"
    );
    assert!(answer.contains("nothing was written"), "{answer}");
    // The one attempt Jira refused; no other status was tried.
    assert_eq!(
        transitions_of(&calls),
        ["jira workitem transition --key ABC-1 --status Blocked --yes"]
    );
}

#[test]
fn reopen_on_an_open_item_says_it_is_already_open() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);

    // To do, and in progress: both are open.
    for (key, status) in [("ABC-1", "To Do"), ("ABC-2", "In Progress")] {
        let answer = allow(
            &user,
            held(&mcp.call("tickets", json!({"action": "reopen", "ticket": key}))),
        );
        assert!(
            answer.contains(&format!(
                "{key} is already open: it is in {status}; nothing was written"
            )),
            "{answer}"
        );
    }
    assert!(
        transitions_of(&words(user.acli_received())).is_empty(),
        "{:?}",
        user.acli_received()
    );
}

/// A tracker whose work items are `seen` as (key, status, category); the one that moves is the first, which can go to `moves`.
fn jira_seen(user: &User, seen: &[(&str, &str, &str)], moves: Value) {
    let site = "example.atlassian.net";
    let items: Vec<Value> = seen
        .iter()
        .map(|(key, status, category)| in_status(work_item(key, "An item", site), status, category))
        .collect();
    let mut by_key = serde_json::Map::new();
    for item in &items {
        by_key.insert(item["key"].as_str().unwrap().to_owned(), item.clone());
    }
    user.acli_script(json!({
        "items": by_key,
        "search": items,
        "transitions": {seen[0].0: moves},
    }));
}

#[test]
fn close_picks_done_among_several_done_statuses_and_never_won_t_do() {
    // "Won't do" was set more recently than "Done": the search lists it first.
    let user = user();
    on_jira(&user);
    jira_seen(
        &user,
        &[
            ("ABC-1", "To Do", "new"),
            ("ABC-4", "Won't do", "done"),
            ("ABC-3", "Done", "done"),
        ],
        json!(["Won't do", "Done"]),
    );

    let (answer, calls) = jira_asked(&user, json!({"action": "close", "ticket": "ABC-1"}));

    assert!(
        answer.contains("ABC-1 was closed: moved to Done."),
        "{answer}"
    );
    assert_eq!(
        transitions_of(&calls),
        ["jira workitem transition --key ABC-1 --status Done --yes"]
    );
}

#[test]
fn close_with_several_done_statuses_and_none_named_done_is_refused_and_lists_them() {
    let user = user();
    on_jira(&user);
    jira_seen(
        &user,
        &[
            ("ABC-1", "To Do", "new"),
            ("ABC-4", "Won't do", "done"),
            ("ABC-3", "Shipped", "done"),
        ],
        json!(["Won't do", "Shipped"]),
    );

    let (answer, calls) = jira_asked(&user, json!({"action": "close", "ticket": "ABC-1"}));

    assert!(answer.contains("refused:"), "{answer}");
    assert!(
        answer.contains("several done statuses, Won't do and Shipped"),
        "{answer}"
    );
    assert!(answer.contains("move ABC-1 with status"), "{answer}");
    assert!(answer.contains("nothing was written"), "{answer}");
    assert!(transitions_of(&calls).is_empty(), "{calls:?}");
}

#[test]
fn reopen_with_two_new_statuses_and_none_named_to_do_is_refused_and_lists_them() {
    let user = user();
    on_jira(&user);
    jira_seen(
        &user,
        &[
            ("ABC-3", "Done", "done"),
            ("ABC-1", "Backlog", "new"),
            ("ABC-2", "Selected", "new"),
        ],
        json!(["Backlog", "Selected"]),
    );

    let (answer, calls) = jira_asked(&user, json!({"action": "reopen", "ticket": "ABC-3"}));

    assert!(answer.contains("refused:"), "{answer}");
    assert!(
        answer.contains("several to-do statuses, Backlog and Selected"),
        "{answer}"
    );
    assert!(answer.contains("nothing was written"), "{answer}");
    assert!(transitions_of(&calls).is_empty(), "{calls:?}");
}

#[test]
fn reopen_on_a_done_item_takes_the_transition_to_the_to_do_status_among_new_ones() {
    let user = user();
    on_jira(&user);
    jira_seen(
        &user,
        &[
            ("ABC-3", "Done", "done"),
            ("ABC-1", "Backlog", "new"),
            ("ABC-2", "To Do", "new"),
        ],
        json!(["Backlog", "To Do"]),
    );

    let (answer, calls) = jira_asked(&user, json!({"action": "reopen", "ticket": "ABC-3"}));

    assert!(
        answer.contains("ABC-3 was reopened: moved to To Do."),
        "{answer}"
    );
    assert_eq!(
        transitions_of(&calls),
        ["jira workitem transition --key ABC-3 --status To Do --yes"]
    );
}

#[test]
fn reopen_never_falls_back_to_a_started_status() {
    // Only a started status is seen: To Do is the one attempted, and Jira has no such transition.
    let user = user();
    on_jira(&user);
    jira_seen(
        &user,
        &[
            ("ABC-3", "Done", "done"),
            ("ABC-2", "In Progress", "indeterminate"),
        ],
        json!(["In Progress"]),
    );

    let (answer, calls) = jira_asked(&user, json!({"action": "reopen", "ticket": "ABC-3"}));

    assert!(answer.contains("acli could not move ABC-3"), "{answer}");
    assert!(answer.contains("nothing was written"), "{answer}");
    assert_eq!(
        transitions_of(&calls),
        ["jira workitem transition --key ABC-3 --status To Do --yes"]
    );
}

#[test]
fn a_transition_acli_answers_without_moving_is_not_reported_as_moved() {
    // The bulk command may end with 0 on a transition it did not take.
    let user = user();
    on_jira(&user);
    jira_all_to_do_with(&user, json!(["Done"]), json!({"no_move": ["ABC-1"]}));

    let (answer, _) = jira_asked(
        &user,
        json!({"action": "status", "ticket": "ABC-1", "status": "Done"}),
    );

    assert!(
        answer.contains("ABC-1 was not moved to Done: it is in To Do"),
        "{answer}"
    );
    assert!(
        !answer.contains("was moved") && !answer.contains("was closed"),
        "{answer}"
    );
}

#[test]
fn a_status_the_item_is_already_in_says_so_and_writes_nothing() {
    let user = user();
    on_jira(&user);
    jira_items(&user);

    let (answer, calls) = jira_asked(
        &user,
        json!({"action": "status", "ticket": "ABC-1", "status": "to do"}),
    );

    assert!(
        answer.contains("ABC-1 is already in To Do; nothing was written"),
        "{answer}"
    );
    assert!(transitions_of(&calls).is_empty(), "{calls:?}");
}

/// ABC-1 can move to Done and the fake `acli` is killed at the transition, `when` (`before` or
/// `after` the move), with `more` added to the script.
fn killed_during_a_transition(when: &str, more: Value) -> String {
    let user = user();
    on_jira(&user);
    let mut script = json!({"die": {"ABC-1": when}});
    for (name, value) in more.as_object().unwrap() {
        script[name] = value.clone();
    }
    jira_all_to_do_with(&user, json!(["Done"]), script);
    jira_asked(
        &user,
        json!({"action": "status", "ticket": "ABC-1", "status": "Done"}),
    )
    .0
}

#[test]
fn acli_killed_during_a_transition_that_took_is_reported_moved_from_the_status_read_back() {
    let answer = killed_during_a_transition("after", json!({}));

    assert!(answer.contains("ABC-1 was moved to Done"), "{answer}");
}

#[test]
fn acli_killed_during_a_transition_that_did_not_take_says_the_status_it_is_in() {
    let answer = killed_during_a_transition("before", json!({}));

    assert!(
        answer.contains("ABC-1 was not moved to Done: it is in To Do"),
        "{answer}"
    );
}

#[test]
fn acli_killed_during_a_transition_with_no_status_to_read_back_says_the_result_is_unknown() {
    let answer = killed_during_a_transition("after", json!({"unreadable_after_move": ["ABC-1"]}));

    assert!(answer.contains("result is unknown"), "{answer}");
    assert!(!answer.contains("nothing was written"), "{answer}");
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
            "refused: tickets create and types on Jira need project, one of the tracker's: ABC, DEF",
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
        json!({"title": "Fix login", "brief": "The login fails: fix it.", "workspace": "login", "kind": "handed"}),
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
        says.contains(
            "Start a session for the user, handed over, in the workspace login: \\\"Fix login\\\""
        ) && says.contains("Its first message: The login fails: fix it."),
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
    assert!(new.tools.iter().any(|t| t.ends_with("__ask")));
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
        json!({"title": "Docs", "brief": "Write the docs.", "kind": "handed"}),
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
        json!({"kind": "child", "title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);
    let before = agents(&user).len();

    let refused = child.call(
        "agent_launch",
        json!({"title": "escape", "brief": "Work outside my tree.", "kind": "handed"}),
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
        json!({"title": "Second", "brief": brief, "kind": "handed"}),
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
        json!({"title": "Third", "brief": "And the tests.", "kind": "handed"}),
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
fn a_handed_session_keeps_the_chain_rule_until_detached() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    let (lead, second, mcp) = launched_root(&user, "auto", "Go on with the docs.");
    let first = mcp.call(
        "agent_launch",
        json!({"title": "Third", "brief": "And the tests.", "kind": "handed"}),
    );
    held(&first);
    assert_eq!(
        user.call("approval.list", json!({})).unwrap()["requests"][0]["class"],
        "base"
    );
    user.call("approval.deny", json!({"number": held(&first)}))
        .unwrap();

    user.call("agent.detach", json!({"agent": second})).unwrap();

    // The user took it: what it starts is judged as any of the user's sessions' launches.
    let judged = judge_prompts(&user).len();
    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Fourth", "brief": "And the docs.", "kind": "handed"}),
    );
    assert!(answer.text.contains("Started a session"), "{}", answer.text);
    assert!(
        judge_prompts(&user).len() > judged,
        "the judge was not asked"
    );
    let fourth = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Fourth")
        .unwrap();
    assert_eq!(fourth["launched_by"], second.as_str());
    let detached = user.call("agent.show", json!({"agent": second})).unwrap();
    assert!(detached["agent"]["launched_by"].is_null(), "{detached}");
    assert_eq!(detached["agent"]["detached_from"], lead.as_str());
}

#[test]
fn a_detached_session_may_start_a_session_in_its_mode() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_escalation()
        .with_allowing_judge();
    let (_, second, mcp) = launched_root(&user, "auto", "Go on with the docs.");
    user.call("agent.detach", json!({"agent": second})).unwrap();

    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Third", "brief": "And the tests.", "kind": "led"}),
    );

    // Auto lets the judge decide it, as for any session of the user's: no chain is left.
    assert!(
        answer.text.contains("Started a session led by you"),
        "{}",
        answer.text
    );
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let third = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Third")
        .unwrap();
    assert_eq!(third["launched_by"], second.as_str());
}

#[test]
fn a_root_session_with_a_brief_longer_than_the_judge_sees_whole_is_refused() {
    let user = user();
    on_github(&user);
    let (_, mcp) = root_in(&user, None);
    let before = agents(&user).len();

    let refused = mcp.call(
        "agent_launch",
        json!({"title": "Long", "brief": "word ".repeat(2_000), "kind": "handed"}),
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
        json!({"title": "Deploy", "brief": "Use ghp_abcdefghijklmnopqrstuvwxyz0123456789 to deploy.", "kind": "handed"}),
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
        json!({"title": "Long", "brief": "word ".repeat(2_000), "new_workspace": "long", "kind": "handed"}),
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
        json!({"kind": "child", "title": "worker", "brief": "Read the code."}),
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
fn workspace_remove_called_from_inside_the_workspace_is_held() {
    let user = user();
    on_github(&user);
    let (_, mcp) = root_in(&user, Some("login"));

    for args in [
        json!({"action": "remove"}),
        json!({"action": "remove", "name": "login"}),
    ] {
        let asked = held(&mcp.call("workspace", args));
        assert_eq!(the_request(&user)["state"], "pending");
        assert!(workspace_exists(&user, "login"));
        user.call("approval.deny", json!({"number": asked}))
            .unwrap();
    }
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
        json!({"title": "Login", "brief": "Fix the login.", "workspace": "login", "kind": "handed"}),
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
fn a_child_with_a_workspace_is_refused_naming_led() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, None);
    let before = agents(&user).len();

    for place in [
        json!({"workspace": "login"}),
        json!({"new_workspace": "fresh"}),
        json!({"new_workspace": "fresh", "repositories": ["."]}),
    ] {
        let mut call =
            json!({"kind": "child", "title": "Fix login", "brief": "The login fails: fix it."});
        call.as_object_mut()
            .unwrap()
            .extend(place.as_object().unwrap().clone());
        let refused = lead.call("agent_launch", call);
        assert!(refused.error, "{place}: {}", refused.text);
        assert!(
            refused
                .text
                .starts_with("refused: a child works where you work")
                && refused.text.contains("\"kind\": \"led\""),
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
        json!({"kind": "child", "title": "worker", "brief": "Write the tests.", "workspace": "login"}),
    );

    assert!(!launched.error, "{}", launched.text);
    let child = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "worker")
        .unwrap();
    assert_eq!(child["workspace"], "login");
}

#[test]
fn agent_launch_names_each_kind_with_a_call_of_each() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, None);

    let listed = lead.tools();
    let launch = listed
        .iter()
        .find(|tool| tool["name"] == "agent_launch")
        .unwrap();
    let description = launch["description"].as_str().unwrap();
    assert!(description.starts_with("kind (required)"), "{description}");
    for said in [
        "child, a part of your own task",
        "led, an autonomous job of its own that you pilot",
        "handed, a session you start for the user and let go",
        "{\"kind\": \"child\", \"title\": \"Write the tests\"",
        "{\"kind\": \"led\", \"title\": \"Fix login\", \"brief\": \"...\", \"workspace\": \"login\"}",
        "{\"kind\": \"handed\", \"title\": \"Add API\"",
    ] {
        assert!(description.contains(said), "{said}: {description}");
    }
    assert_eq!(launch["inputSchema"]["required"][0], "kind");
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
fn session_send_to_a_child_or_to_itself_is_refused() {
    let user = judged_user();
    let (lead, mcp, session, second) = lead_and_session(&user);
    // A child of the lead's.
    let child = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "worker", "brief": "Write the tests."}),
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

    for (from, to, said) in [
        (&mcp, &child, "is one of your children"),
        (&second, &child, "is a child of another session"),
        (&mcp, &lead, "is your own session"),
        (&second, &session, "is your own session"),
    ] {
        let refused = from.call("session_send", json!({"session": to, "text": "Hello."}));
        assert!(refused.error, "{to}: {}", refused.text);
        assert!(
            refused.text.starts_with(&format!("refused: {to} {said}")),
            "{}",
            refused.text
        );
    }
    assert_eq!(judge_prompts(&user).len(), judged, "the judge was asked");
    assert!(no_request(&user));
    assert!(delivered(&user, &child, "session").is_empty());
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
        json!({"title": "Login", "brief": "Fix the login.", "workspace": "login", "kind": "handed"}),
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

// What an agent posts carries nothing of Flottille (#381): the daemon knows its posts by what it
// recorded when it posted them.

/// `pulls()` with the comments of the pull request `app/login` and of the issue #1, the next
/// comment posted being numbered 501 by GitHub.
fn github_with_comments(user: &User, pull: Value, issue_comments: Value) {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut script = pulls();
    script["pulls"]["acme/app"]["app/login"]["comments"] = pull;
    script["pulls"]["acme/app"]["app/login"]["updatedAt"] =
        format!("2999-01-01T{:02}:{:02}:00Z", n / 60 % 24, n % 60).into();
    script["issues"] = json!({"acme/app": {"1": {"number": 1, "title": "The login fails",
        "state": "OPEN", "body": "About it", "url": "https://github.com/acme/app/issues/1",
        "labels": [], "assignees": [], "comments": issue_comments}}});
    script["commented"] = json!(501);
    user.gh_script(script);
}

/// A comment as `gh` reads it back: its link names it, as the one `gh pr comment` printed did.
fn read_back(id: u64, author: &str, body: &str) -> Value {
    json!({"id": format!("IC_{id}"), "author": {"login": author}, "body": body,
        "url": format!("https://github.com/acme/app/pull/12#issuecomment-{id}"),
        "createdAt": "2026-10-01T12:00:00Z"})
}

/// The agent comments on the pull request `app/login` (held, then allowed).
fn comment_on_the_pull_request(user: &User, mcp: &Mcp, body: &str) {
    let answer = mcp.call(
        "forge",
        json!({"action": "comment", "number": 12, "body": body}),
    );
    judged_then_done(user, answer, || words(user.gh_received()), "pr comment");
}

/// GitHub: a pull request's comment and an issue's.
fn github_comments_are_posted_without_a_marker() {
    let user = user();
    on_github(&user);
    github_with_comments(&user, json!([]), json!([]));
    let (_, mcp) = root_in(&user, Some("login"));
    comment_on_the_pull_request(&user, &mcp, "Rebased on main");
    let answer = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "#1", "body": "Fixed on app/login"}),
    );
    judged_then_done(&user, answer, || words(user.gh_received()), "issue comment");
    let calls = words(user.gh_received());
    assert!(calls.contains(&"pr comment 12 -R github.com/acme/app --body Rebased on main".into()));
    assert!(
        calls.contains(&"issue comment 1 -R github.com/acme/app --body Fixed on app/login".into())
    );
}

fn bitbucket_comments_are_posted_without_a_marker() {
    let user = user();
    on_bitbucket(&user);
    user.bkt_script(
        json!({"pulls": {"acme/app": [{"id": 3, "title": "Work", "state": "OPEN",
        "source": {"branch": {"name": "app/login"}, "repository": {"full_name": "acme/app"}},
        "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}}}]}}),
    );
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call(
        "forge",
        json!({"action": "comment", "number": 3, "body": "Ready"}),
    );
    judged_then_done(&user, answer, || words(user.bkt_received()), "pr comment");
    let calls = words(user.bkt_received());
    assert!(
        calls.contains(
            &"pr comment 3 --context cloud --workspace acme --repo app --text Ready".into()
        ),
        "{calls:?}"
    );
}

#[test]
fn an_agent_s_comment_is_posted_without_a_marker() {
    github_comments_are_posted_without_a_marker();
    bitbucket_comments_are_posted_without_a_marker();
    let (_, asked) = jira_written(
        json!({"action": "comment", "ticket": "ABC-1", "body": "Fixed on app/login"}),
        "jira workitem comment",
    );
    assert_eq!(
        asked,
        ["jira workitem comment create --key ABC-1 --body-file <file>"]
    );
}

#[test]
fn show_says_posted_by_an_agent_from_the_record() {
    let user = user();
    on_github(&user);
    github_with_comments(&user, json!([]), json!([]));
    let (_, mcp) = root_in(&user, Some("login"));
    comment_on_the_pull_request(&user, &mcp, "Rebased on main");
    let answer = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "#1", "body": "Fixed on app/login"}),
    );
    judged_then_done(&user, answer, || words(user.gh_received()), "issue comment");

    // The forge answers with GitHub's own numbers: the user's, under the same account, is 502.
    github_with_comments(
        &user,
        json!([
            read_back(501, "kris", "Rebased on main"),
            read_back(502, "kris", "Rebased on main")
        ]),
        json!([
            {"author": {"login": "kris"}, "body": "Fixed on app/login",
             "url": "https://github.com/acme/app/issues/1#issuecomment-501"},
            {"author": {"login": "kris"}, "body": "Fixed on app/login",
             "url": "https://github.com/acme/app/issues/1#issuecomment-502"}
        ]),
    );
    let pull = read(
        &user,
        mcp.call("forge", json!({"action": "show", "detail": "full"})),
    );
    assert!(
        pull.contains("- kris (posted by an agent): Rebased on main\n"),
        "{pull}"
    );
    // By the record, not by the text: the same words under another number are a person's.
    assert!(pull.contains("- kris: Rebased on main\n"), "{pull}");
    let ticket = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "#1", "detail": "full"}),
        ),
    );
    assert!(ticket.contains("kris (posted by an agent)"), "{ticket}");
    assert!(ticket.contains("- kris: Fixed on app/login"), "{ticket}");
}

#[test]
fn show_says_posted_by_an_agent_from_the_record_where_the_tracker_gives_no_id() {
    // Jira: `acli` prints no id for a comment, so the record keeps the text, and the comment read
    // back is known by it.
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "ABC-1", "body": "Fixed on app/login"}),
    );
    judged_then_done(
        &user,
        answer,
        || words(user.acli_received()),
        "jira workitem comment",
    );

    let shown = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
        ),
    );

    assert!(shown.contains("(posted by an agent)"), "{shown}");
    assert!(shown.contains("Fixed on app/login"), "{shown}");
    assert!(!shown.contains("posted by a Flottille agent"), "{shown}");
}

/// Every prompt the fake agent was sent that is a subscription's delivery.
fn deliveries(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .filter_map(|p| p["params"]["prompt"][0]["text"].as_str().map(str::to_owned))
        .filter(|p| p.contains("[flottille:subscription]"))
        .collect()
}

#[test]
fn from_people_leaves_out_a_comment_the_agent_posted() {
    let user = user();
    on_github(&user);
    github_with_comments(&user, json!([]), json!([]));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    let (_, mcp) = root_in(&user, Some("login"));
    let subscribed = mcp.call(
        "forge",
        json!({"action": "subscribe", "events": ["comment"], "from": "people"}),
    );
    assert!(!subscribed.error, "{}", subscribed.text);
    comment_on_the_pull_request(&user, &mcp, "Rebased on main");

    // The round finds the agent's comment (501) and the user's own, under the same account (502).
    github_with_comments(
        &user,
        json!([
            read_back(501, "kris", "Rebased on main"),
            read_back(502, "kris", "Why a new form?")
        ]),
        json!([]),
    );
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();

    eventually("the user's comment is delivered", || {
        !deliveries(&user).is_empty()
    });
    let all = deliveries(&user);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(all[0].contains("Why a new form?"), "{}", all[0]);
    assert!(!all[0].contains("Rebased"), "{}", all[0]);
}

#[test]
fn a_comment_with_the_old_marker_is_still_an_agent_s() {
    let user = user();
    on_github(&user);
    github_with_comments(
        &user,
        json!([read_back(
            7,
            "kris",
            &format!("Rebased on main\n\n{MARKER}")
        )]),
        json!([{"author": {"login": "kris"}, "body": format!("Fixed\n\n{MARKER}")}]),
    );
    let (_, mcp) = root_in(&user, Some("login"));

    let pull = read(
        &user,
        mcp.call("forge", json!({"action": "show", "detail": "full"})),
    );
    let ticket = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "#1", "detail": "full"}),
        ),
    );

    assert!(
        pull.contains("- kris (posted by an agent): Rebased on main\n"),
        "{pull}"
    );
    assert!(ticket.contains("kris (posted by an agent)"), "{ticket}");
    for shown in [&pull, &ticket] {
        assert!(!shown.contains("flottille -->"), "{shown}");
    }

    // Jira: the visible footer of before.
    let user = self::user();
    on_jira(&user);
    let mut item = work_item("ABC-1", "The login fails", "example.atlassian.net");
    item["fields"]["comment"] = json!({"comments": [{"author": {"displayName": "kris"},
        "created": "2026-10-01T12:00:00.000+0000",
        "body": format!("Started\n\n{JIRA_FOOTER}")}]});
    user.acli_script(json!({"items": {"ABC-1": item}}));
    let (_, mcp) = root_in(&user, None);
    let shown = read(
        &user,
        mcp.call(
            "tickets",
            json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
        ),
    );
    assert!(shown.contains("kris (posted by an agent)"), "{shown}");
    assert!(!shown.contains("posted by a Flottille agent"), "{shown}");
}

#[test]
fn what_an_agent_posted_is_still_known_after_a_daemon_restart() {
    let user = user();
    on_github(&user);
    github_with_comments(&user, json!([]), json!([]));
    let (_, mcp) = root_in(&user, Some("login"));
    comment_on_the_pull_request(&user, &mcp, "Rebased on main");
    drop(mcp);

    let daemon = user.status()["pid"].as_u64().unwrap();
    // SAFETY: kill has no memory precondition.
    unsafe { libc::kill(daemon as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Path::new(&format!("/proc/{daemon}")).exists() {
        assert!(Instant::now() < deadline, "the daemon did not die");
        std::thread::sleep(Duration::from_millis(20));
    }
    user.status();

    github_with_comments(
        &user,
        json!([read_back(501, "kris", "Rebased on main")]),
        json!([]),
    );
    let (_, mcp) = root_in(&user, Some("login"));
    let shown = read(
        &user,
        mcp.call("forge", json!({"action": "show", "detail": "full"})),
    );
    assert!(
        shown.contains("- kris (posted by an agent): Rebased on main\n"),
        "{shown}"
    );
}

/// A Cloud comment of the pull request #3 as `bkt api` reads it: its id and the time it was
/// written, which tell it from another of the same words.
fn cloud_said(id: u64, at: &str, author: &str, text: &str) -> Value {
    json!({"id": id, "created_on": at, "content": {"raw": text},
        "user": {"display_name": author}, "deleted": false})
}

/// A comment before the agent's post, and one after it, whatever the clock of the test.
const BEFORE: &str = "2020-01-01T00:00:00.000000+00:00";
const AFTER: &str = "2999-01-01T00:00:00.000000+00:00";

/// The pull request #3 of `acme/app` on Bitbucket Cloud, `state`, as `forge show` reads it: its
/// reason when declined, its comments.
fn cloud_pull_three(user: &User, state: &str, reason: Option<&str>, comments: Vec<Value>) {
    let mut page = json!({"participants": []});
    if let Some(reason) = reason {
        page["reason"] = json!(reason);
        page["closed_by"] = json!({"display_name": "Kris"});
    }
    // On the branch of the workspace `login`, whose pull request a subscription waits on; a round
    // reads its comments again when its count or its time changed.
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut pull = cloud_pull(3, state);
    pull["source"]["branch"]["name"] = json!("app/login");
    pull["comment_count"] = json!(comments.len());
    pull["updated_on"] = json!(format!(
        "2999-02-01T{:02}:{:02}:00+00:00",
        n / 60 % 24,
        n % 60
    ));
    user.bkt_script(json!({
        "pulls": {"acme/app": [pull.clone()]},
        "checks": {"acme/app": {}},
        "api": {
            "/repositories/acme/app/pullrequests": {"values": [pull]},
            "/repositories/acme/app/pullrequests/3": page,
            "/repositories/acme/app/pullrequests/3/comments": {"values": comments},
            "/repositories/acme/app/pullrequests/3/diffstat": {"values": []}
        }
    }));
}

/// The agent comments `body` on the Bitbucket pull request #3 (held, then allowed).
fn comment_on_bitbucket(user: &User, mcp: &Mcp, body: &str) {
    cloud_pull_three(user, "OPEN", None, Vec::new());
    let answer = mcp.call(
        "forge",
        json!({"action": "comment", "number": 3, "body": body}),
    );
    judged_then_done(user, answer, || words(user.bkt_received()), "pr comment");
}

fn show_bitbucket_three(user: &User, mcp: &Mcp) -> String {
    read(
        user,
        mcp.call(
            "forge",
            json!({"action": "show", "number": 3, "detail": "full"}),
        ),
    )
}

#[test]
fn an_agent_s_bitbucket_decline_comment_is_shown_as_the_agent_s() {
    let user = user();
    on_bitbucket(&user);
    cloud_pull_three(&user, "OPEN", None, Vec::new());
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call(
        "forge",
        json!({"action": "close", "number": 3, "comment": "Superseded, see https://example.com/new"}),
    );
    judged_then_done(&user, answer, || words(user.bkt_received()), "pr decline");

    // Cloud keeps the reason a pull request was declined with, and says who declined it.
    cloud_pull_three(
        &user,
        "DECLINED",
        Some("Superseded, see https://example.com/new"),
        Vec::new(),
    );
    let shown = show_bitbucket_three(&user, &mcp);

    assert!(
        shown.contains(
            "closing comment (Kris, posted by an agent): Superseded, see https://example.com/new"
        ),
        "{shown}"
    );
}

#[test]
fn show_says_a_bitbucket_comment_was_posted_by_an_agent() {
    let user = user();
    on_bitbucket(&user);
    let (_, mcp) = root_in(&user, None);
    comment_on_bitbucket(&user, &mcp, "Ready for review");

    cloud_pull_three(
        &user,
        "OPEN",
        None,
        vec![cloud_said(11, AFTER, "Kris", "Ready for review")],
    );
    let shown = show_bitbucket_three(&user, &mcp);

    assert!(
        shown.contains("- Kris (posted by an agent): Ready for review\n"),
        "{shown}"
    );
}

#[test]
fn a_person_s_same_words_on_bitbucket_stay_a_person_s() {
    let user = user();
    on_bitbucket(&user);
    let (_, mcp) = root_in(&user, None);
    comment_on_bitbucket(&user, &mcp, "Ready for review");

    // The user said it before, then the agent (12), then the user again (13): the record is one
    // post, so it is one comment.
    cloud_pull_three(
        &user,
        "OPEN",
        None,
        vec![
            cloud_said(10, BEFORE, "Kris", "Ready for review"),
            cloud_said(12, AFTER, "Kris", "Ready for review"),
            cloud_said(
                13,
                "2999-01-02T00:00:00.000000+00:00",
                "Kris",
                "Ready for review",
            ),
        ],
    );
    let shown = show_bitbucket_three(&user, &mcp);

    assert_eq!(
        shown
            .matches("(posted by an agent): Ready for review")
            .count(),
        1,
        "{shown}"
    );
    assert_eq!(
        shown.matches("- Kris: Ready for review").count(),
        2,
        "{shown}"
    );
    assert!(
        shown.find("- Kris: Ready for review") < shown.find("(posted by an agent)"),
        "the older one is the user's: {shown}"
    );
}

#[test]
fn an_agent_comment_holding_a_token_shape_is_still_the_agent_s() {
    let user = user();
    on_bitbucket(&user);
    let (_, mcp) = root_in(&user, None);
    let token = "ghp_0123456789abcdefghijklmnopqrstuvwxyz";
    comment_on_bitbucket(&user, &mcp, &format!("The key {token} was rotated"));

    // The read masks what it reads before it is matched.
    cloud_pull_three(
        &user,
        "OPEN",
        None,
        vec![cloud_said(
            11,
            AFTER,
            "Kris",
            &format!("The key {token} was rotated"),
        )],
    );
    let shown = show_bitbucket_three(&user, &mcp);

    assert!(
        shown.contains("- Kris (posted by an agent): The key"),
        "{shown}"
    );
    assert!(!shown.contains(token), "{shown}");
}

#[test]
fn from_people_leaves_out_the_agent_s_bitbucket_comment() {
    let user = user();
    on_bitbucket(&user);
    cloud_pull_three(&user, "OPEN", None, Vec::new());
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    let (_, mcp) = root_in(&user, Some("login"));
    let subscribed = mcp.call(
        "forge",
        json!({"action": "subscribe", "events": ["comment"], "from": "people"}),
    );
    assert!(!subscribed.error, "{}", subscribed.text);
    comment_on_bitbucket(&user, &mcp, "Rebased on main");

    cloud_pull_three(
        &user,
        "OPEN",
        None,
        vec![
            cloud_said(11, AFTER, "Kris", "Rebased on main"),
            cloud_said(
                12,
                "2999-01-02T00:00:00.000000+00:00",
                "Kris",
                "Why a new form?",
            ),
        ],
    );
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();

    eventually("the user's comment is delivered", || {
        !deliveries(&user).is_empty()
    });
    let all = deliveries(&user);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(all[0].contains("Why a new form?"), "{}", all[0]);
    assert!(!all[0].contains("Rebased"), "{}", all[0]);
}

#[test]
fn from_people_leaves_out_the_agent_s_jira_comment() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    let (_, mcp) = root_in(&user, None);
    let subscribed = mcp.call(
        "tickets",
        json!({"action": "subscribe", "ticket": "ABC-1", "events": ["commented"], "from": "people"}),
    );
    assert!(!subscribed.error, "{}", subscribed.text);
    let answer = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "ABC-1", "body": "Fixed on app/login"}),
    );
    judged_then_done(
        &user,
        answer,
        || words(user.acli_received()),
        "jira workitem comment",
    );

    let site = "example.atlassian.net";
    let mut item = in_status(work_item("ABC-1", "The login fails", site), "To Do", "new");
    let said = |at: &str, text: &str| {
        json!({"author": {"displayName": "Kris"}, "created": at,
            "body": {"type": "doc", "version": 1, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": text}]}]}})
    };
    item["fields"]["updated"] = json!("2999-01-02T00:00:00.000+0000");
    // The fake adds the comment the agent created, by its signed-in user, to what it holds.
    item["fields"]["comment"] = json!({"comments": [
        said("2999-01-02T00:00:00.000+0000", "Which commit?")
    ]});
    user.acli_script(json!({"items": {"ABC-1": item.clone()}, "search": [item]}));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();

    eventually("the user's comment is delivered", || {
        !deliveries(&user).is_empty()
    });
    let all = deliveries(&user);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(all[0].contains("Which commit?"), "{}", all[0]);
    assert!(!all[0].contains("Fixed on app/login"), "{}", all[0]);
}

/// What the store recorded as posted on an item, as (kind, id of the comment).
fn recorded(
    user: &User,
    source: &str,
    repository: &str,
    key: &str,
) -> Vec<(String, Option<String>)> {
    store(user)
        .posts("app", source, repository, key)
        .unwrap()
        .into_iter()
        .map(|post| (post.kind, post.comment_id))
        .collect()
}

fn body_of(id: Option<&str>) -> (String, Option<String>) {
    ("body".to_owned(), id.map(str::to_owned))
}

fn comment_of(id: Option<&str>) -> (String, Option<String>) {
    ("comment".to_owned(), id.map(str::to_owned))
}

#[test]
fn every_post_of_an_agent_on_github_is_recorded_with_the_id_gh_answers() {
    let user = user();
    on_github(&user);
    github_with_comments(&user, json!([]), json!([]));
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("gh/script.json")).unwrap())
            .unwrap();
    script["created_issue"] = json!(46);
    user.gh_script(script);
    let (_, mcp) = root_in(&user, Some("login"));
    let post = |tool: &str, args: Value| {
        let number = held(&mcp.call(tool, args));
        allow(&user, number);
    };

    post(
        "forge",
        json!({"action": "comment", "number": 12, "body": "Rebased"}),
    );
    post(
        "forge",
        json!({"action": "edit", "number": 12, "body": "What it does now"}),
    );
    post(
        "forge",
        json!({"action": "edit", "number": 12, "title": "Only a title"}),
    );
    post(
        "tickets",
        json!({"action": "create", "title": "Add a logout", "body": "Users stay in"}),
    );
    post("tickets", json!({"action": "create", "title": "No body"}));
    post(
        "tickets",
        json!({"action": "comment", "ticket": "#1", "body": "Fixed"}),
    );
    post(
        "tickets",
        json!({"action": "edit", "ticket": "#1", "body": "New text"}),
    );

    // `gh pr comment` and `gh issue comment` print the comment's link; the others print the item's.
    assert_eq!(
        recorded(&user, "github", ".", "12"),
        [comment_of(Some("issuecomment-501")), body_of(None)]
    );
    assert_eq!(recorded(&user, "github", ".", "46"), [body_of(None)]);
    assert_eq!(
        recorded(&user, "github", ".", "1"),
        [comment_of(Some("issuecomment-501")), body_of(None)]
    );
}

#[test]
fn a_comment_that_closes_or_reopens_is_recorded_by_its_text_as_gh_and_bkt_give_no_id() {
    let user = user();
    on_github(&user);
    user.gh_script(pulls_to_close());
    let (_, mcp) = root_in(&user, None);
    let answer = mcp.call(
        "forge",
        json!({"action": "close", "number": 12, "comment": "Superseded by #20"}),
    );
    judged_then_done(&user, answer, || words(user.gh_received()), "pr close");
    assert_eq!(recorded(&user, "github", ".", "12"), [comment_of(None)]);
    let posts = store(&user).posts("app", "github", ".", "12").unwrap();
    assert!(posts[0].fingerprint.is_some());

    let user = self::user();
    on_bitbucket(&user);
    user.bkt_script(
        json!({"pulls": {"acme/app": [{"id": 3, "title": "Work", "state": "OPEN",
        "source": {"branch": {"name": "app/login"}, "repository": {"full_name": "acme/app"}},
        "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}}}]}}),
    );
    let (_, mcp) = root_in(&user, None);
    for args in [
        json!({"action": "comment", "number": 3, "body": "Ready"}),
        json!({"action": "edit", "number": 3, "body": "Adds the login page"}),
        json!({"action": "close", "number": 3, "comment": "Superseded"}),
    ] {
        let verb = format!(
            "pr {}",
            if args["action"] == "close" {
                "decline"
            } else {
                args["action"].as_str().unwrap()
            }
        );
        let answer = mcp.call("forge", args);
        judged_then_done(&user, answer, || words(user.bkt_received()), &verb);
    }
    assert_eq!(
        recorded(&user, "bitbucket", ".", "3"),
        [comment_of(None), body_of(None), comment_of(None)]
    );
}

#[test]
fn every_post_of_an_agent_on_jira_is_recorded_a_comment_by_the_id_read_after_it() {
    let user = user();
    on_jira(&user);
    jira_items(&user);
    let (_, mcp) = root_in(&user, None);
    let calls = || words(user.acli_received());
    for (args, verb) in [
        (
            json!({"action": "create", "title": "Add a logout", "body": "Users stay in", "project": "DEF"}),
            "jira workitem create",
        ),
        (
            json!({"action": "comment", "ticket": "ABC-1", "body": "Fixed"}),
            "jira workitem comment",
        ),
        (
            json!({"action": "edit", "ticket": "ABC-1", "body": "New text"}),
            "jira workitem edit",
        ),
    ] {
        let answer = mcp.call("tickets", args);
        judged_then_done(&user, answer, calls, verb);
    }

    assert_eq!(recorded(&user, "jira", "", "DEF-7"), [body_of(None)]);
    assert_eq!(
        recorded(&user, "jira", "", "ABC-1"),
        [comment_of(Some("10100")), body_of(None)]
    );
}

// A root starts a session for the user in main (#408).

/// The text of the one request held for the user.
fn says_of_the_request(user: &User) -> String {
    the_request(user)["says"].to_string()
}

#[test]
fn a_root_in_a_workspace_starts_a_session_in_main() {
    let user = user();
    on_github(&user);
    let (lead, mcp) = root_in(&user, Some("login"));
    user.call("agent.mode", json!({"agent": lead, "mode": "ask"}))
        .unwrap();

    let number = held(&mcp.call(
        "agent_launch",
        json!({"title": "Study", "brief": "Read the code.", "workspace": "main", "kind": "handed"}),
    ));
    let done = allow(&user, number);

    assert!(done.contains("Started a session for the user"), "{done}");
    assert!(done.contains("main, the project folder"), "{done}");
    let started = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Study")
        .unwrap_or_else(|| panic!("no session started"));
    let session = agent(&user, started["id"].as_str().unwrap());
    assert_eq!(session.workspace, None, "the project folder is main");
    assert_eq!(session.parent, None);
    assert_eq!(session.launched_by.as_deref(), Some(lead.as_str()));
    // No workspace was made for it.
    assert!(!workspace_exists(&user, "main"));
}

#[test]
fn the_gate_is_told_the_session_starts_in_main() {
    let user = user();
    on_github(&user);
    let (lead, mcp) = root_in(&user, Some("login"));
    user.call("agent.mode", json!({"agent": lead, "mode": "ask"}))
        .unwrap();

    held(&mcp.call(
        "agent_launch",
        json!({"title": "Study", "brief": "Read the code.", "workspace": "main", "kind": "handed"}),
    ));

    let says = says_of_the_request(&user);
    assert!(
        says.contains(
            "Start a session for the user, handed over, in main, the project folder: \\\"Study\\\""
        ),
        "{says}"
    );
    assert!(!says.contains("the workspace main"), "{says}");
}

#[test]
fn a_workspace_cannot_be_named_main() {
    let user = user();
    on_github(&user);
    let (lead, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "ask"}))
        .unwrap();
    let before = agents(&user).len();

    // The agent's tool, and a launch that makes one: refused before the user is asked.
    for (tool, call) in [
        ("workspace", json!({"action": "create", "name": "main"})),
        (
            "agent_launch",
            json!({"title": "Study", "brief": "Read.", "new_workspace": "main", "kind": "handed"}),
        ),
    ] {
        let refused = mcp.call(tool, call);
        assert!(refused.error, "{tool}: {}", refused.text);
        assert!(
            refused.text.contains("main is reserved")
                && refused.text.contains("the project folder"),
            "{tool}: {}",
            refused.text
        );
    }
    assert!(no_request(&user));
    assert_eq!(agents(&user).len(), before);

    // The protocol, which the window uses, and the command line.
    let failure = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "main"}),
        )
        .unwrap_err();
    assert!(failure.1.contains("main is reserved"), "{failure:?}");
    let output = user.run(&["workspace", "create", "app", "main"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("main is reserved"), "{stderr}");
    assert!(!workspace_exists(&user, "main"));
}

#[test]
fn a_project_with_an_old_workspace_named_main_keeps_it() {
    let user = user();
    on_github(&user);
    // A workspace made before the name was reserved: the login one, under another name.
    let mut old = store(&user).workspace("app", "login").unwrap().unwrap();
    old.name = "main".into();
    old.branch = "app/old-main".into();
    old.steps.clear();
    store(&user)
        .create_workspace(&old, ("workspace.created", json!({"name": "main"})))
        .unwrap();
    let (lead, mcp) = root_in(&user, None);
    user.call("agent.mode", json!({"agent": lead, "mode": "ask"}))
        .unwrap();

    let number = held(&mcp.call(
        "agent_launch",
        json!({"title": "Study", "brief": "Read the code.", "workspace": "main", "kind": "handed"}),
    ));

    let says = says_of_the_request(&user);
    assert!(says.contains("the workspace main"), "{says}");
    let done = allow(&user, number);
    assert!(done.contains("Started a session"), "{done}");
    let started = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "Study")
        .unwrap();
    assert_eq!(
        agent(&user, started["id"].as_str().unwrap())
            .workspace
            .as_deref(),
        Some("main"),
        "it names the workspace, not the project folder"
    );
}

#[test]
fn a_child_still_cannot_be_launched_in_main_from_a_workspace() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, Some("login"));
    let before = agents(&user).len();

    let refused = lead.call(
        "agent_launch",
        json!({"kind": "child", "title": "worker", "brief": "Read the code.", "workspace": "main"}),
    );

    assert!(refused.error, "{}", refused.text);
    assert!(
        refused
            .text
            .starts_with("refused: a child works where you work"),
        "{}",
        refused.text
    );
    assert_eq!(agents(&user).len(), before);
}
