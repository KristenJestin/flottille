//! The complete families of the agents' tools (#141, `docs/tools.md`, "The forge", "The tickets",
//! "Sessions for the user"): every read of `forge` and `tickets` answers at once, every write is
//! held for the user and done once they allow it, `tickets list` gives every open ticket in one
//! call, Jira is read only, and a root starts a session for the user that is the user's. The
//! forge and the trackers are the fake `gh`, `bkt` and `acli`; the remotes are throwaway bare
//! repositories; the escalating programs are fakes, first on the daemon's `PATH`, and none runs.

mod common;

use std::path::PathBuf;
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
        numbered.contains(". (acme/app): pull request #7, merged, into main: Old work"),
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
            &"pr list --context cloud --workspace acme --repo app --state OPEN --limit 100 --json"
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
fn tickets_writes_on_jira_are_refused_before_anything_is_asked() {
    let user = user();
    on_jira(&user);
    let (_, mcp) = root_in(&user, None);

    for args in [
        json!({"action": "create", "title": "x"}),
        json!({"action": "comment", "ticket": "ABC-1", "body": "x"}),
        json!({"action": "edit", "ticket": "ABC-1", "title": "x"}),
        json!({"action": "close", "ticket": "ABC-1"}),
        json!({"action": "reopen", "ticket": "ABC-1"}),
    ] {
        let action = args["action"].as_str().unwrap().to_owned();
        let refused = mcp.call("tickets", args);
        assert!(refused.error, "{}", refused.text);
        assert!(
            refused.text.starts_with(&format!(
                "refused: tickets {action} is not supported on Jira: Flottille reads Jira \
                 tickets and writes nothing to Jira"
            )),
            "{}",
            refused.text
        );
    }
    assert!(no_request(&user));
    assert!(user.acli_received().is_empty());
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
        json!({"title": "worker", "brief": "Write the tests.", "workspace": "login"}),
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
            .starts_with("refused: only the agent the user talks to starts a session for the user"),
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
fn a_child_session_with_a_brief_longer_than_the_judge_sees_whole_is_refused() {
    let user = user();
    on_github(&user);
    let (_, lead) = root_in(&user, None);
    let before = agents(&user).len();

    let refused = lead.call(
        "agent_launch",
        json!({"title": "Long", "brief": "word ".repeat(2_000), "new_workspace": "long"}),
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
            .starts_with("refused: a child removes only its own workspace"),
        "{}",
        refused.text
    );
    assert!(no_request(&user));
    assert!(workspace_exists(&user, "login"));
}

#[test]
fn a_workspace_another_agent_of_the_tree_works_in_is_in_use() {
    let user = user();
    on_github(&user);
    let (_, root) = root_in(&user, Some("login"));
    let opened = user.received_requests("session/new").len();
    let launched = root.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    server_of(&user, opened);
    let child = agents(&user)
        .into_iter()
        .find(|a| a["title"] == "worker")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let answer = root.call("workspace", json!({"action": "remove"}));
    let done = allow(&user, held(&answer));

    assert!(
        done.contains(&format!(
            "the agent {child}, of this agent's tree, works in it"
        )),
        "{done}"
    );
    assert!(workspace_exists(&user, "login"));
}
