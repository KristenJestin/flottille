//! A blocked agent asks the user for a one-off exception (#1220, `docs/design.md` §7.3): a call the
//! gate holds for a rule that protects the project's own process (a forced push of the agent's own
//! branch, a merge past failing checks or a branch protection, a forge action the rules hold) is
//! asked with `exception` and its reason; the user's yes lifts that one rule for the session that
//! asked until it ends or the user withdraws it, other sessions and its sub-agents still asking for
//! themselves. What can never be asked for is refused with no ask. Against the real daemon with
//! the fake ACP agent and the fake `gh`, throwaway repositories and temporary XDG folders.

use crate::common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

const WHY: &str = "a check refuses an old commit that holds sample data of a public standard; the \
                   squashed commit does not";

/// A project `app` over a throwaway repository pushed to a throwaway bare remote.
fn app(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
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
    folder
}

/// A ready workspace of `app`, on the branch `app/<name>`.
fn workspace(user: &User, name: &str) {
    user.call("workspace.create", json!({"project": "app", "name": name}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": name}))
            .unwrap()["state"]
            == "ready"
    });
}

/// A root session the user starts in the workspace `name`: its id and MCP session.
fn root(user: &User, name: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": name, "message": "ship the login page"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

/// A sub-agent `mcp`'s session starts: its MCP session.
fn child(user: &User, mcp: &Mcp) -> Mcp {
    let opened = user.received_requests("session/new").len();
    let launched = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "worker", "brief": "Push the branch."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn store(user: &User) -> Store {
    Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap()
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
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

/// A call that went through: neither held nor refused.
fn passed(user: &User, answer: &Answer) {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}; the requests: {:?}",
        answer.text,
        requests(user)
    );
}

/// A call refused at once, with no request made.
fn refused_with_no_ask(user: &User, answer: &Answer) {
    assert!(answer.error, "not refused: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    assert!(requests(user).is_empty(), "a request was made");
}

/// The user answers request `number` with `method`: the text the agent is handed for it.
fn answer(user: &User, number: u64, method: &str) -> String {
    user.call(method, json!({"number": number})).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

/// The rules the session `agent` holds an exception of, as `exception.list` lists them.
fn exceptions(user: &User, agent: &str) -> Vec<String> {
    user.call("exception.list", json!({"session": agent}))
        .unwrap()["exceptions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|exception| exception["rule"].as_str().unwrap().to_owned())
        .collect()
}

fn force_push(mcp: &Mcp, branch: &str) -> Answer {
    mcp.call(
        "run",
        json!({"line": format!("git push --force origin {branch}")}),
    )
}

fn ask_force_push(mcp: &Mcp, branch: &str) -> Answer {
    mcp.call(
        "run",
        json!({"line": format!("git push --force origin {branch}"), "exception": WHY}),
    )
}

#[test]
fn a_refused_action_asked_for_and_accepted_no_longer_stops_that_session() {
    let user = User::with_fake_agent().with_allowing_judge();
    app(&user);
    workspace(&user, "w");
    let (agent, mcp) = root(&user, "w");

    // Blocked by default: the forced push of its own branch is the user's, every time.
    let blocked = held(&force_push(&mcp, "app/w"));
    answer(&user, blocked, "approval.deny");

    // The ask says the refused action exactly, and the agent's reason; it is answered at once.
    let asked = held(&ask_force_push(&mcp, "app/w"));
    let request = requests(&user)
        .into_iter()
        .find(|request| request["number"] == asked)
        .unwrap();
    assert_eq!(request["class"], "base", "{request}");
    assert_eq!(request["call"]["line"], "git push --force origin app/w");
    let reasons: Vec<&str> = request["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reason| reason.as_str().unwrap())
        .collect();
    assert!(
        reasons[0].starts_with("it asks an exception for the rest of this session:"),
        "{reasons:?}"
    );
    assert!(reasons[0].contains("force-push"), "{reasons:?}");
    assert!(reasons[0].contains(WHY), "{reasons:?}");
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("forces the push")),
        "{reasons:?}"
    );

    // Accepted: the call is done, and the exception is the session's.
    let done = answer(&user, asked, "approval.approve");
    assert!(done.contains("Flottille has now done it"), "{done}");
    assert_eq!(exceptions(&user, &agent), ["force-push"]);

    // The same rule no longer stops it: the push is judged as any push is, and passes.
    passed(&user, &force_push(&mcp, "app/w"));
    assert!(requests(&user).is_empty(), "a request was made");

    // The exception and the answer stay in the session's history.
    let events = user.call("agent.show", json!({"agent": agent})).unwrap()["events"].clone();
    assert!(
        events
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["name"] == "exception.granted"),
        "no exception.granted: {events}"
    );
}

#[test]
fn an_accepted_exception_still_stops_another_session_and_a_sub_agent() {
    let user = User::with_fake_agent().with_allowing_judge();
    app(&user);
    workspace(&user, "w");
    workspace(&user, "v");
    let (agent, mcp) = root(&user, "w");
    let asked = held(&ask_force_push(&mcp, "app/w"));
    answer(&user, asked, "approval.approve");
    assert_eq!(exceptions(&user, &agent), ["force-push"]);

    // Another session asks for itself.
    let (_, other) = root(&user, "v");
    held(&force_push(&other, "app/v"));

    // So does the session's own sub-agent: its parent holds its request.
    let worker = child(&user, &mcp);
    let pushed = force_push(&worker, "app/w");
    assert!(!pushed.error, "{}", pushed.text);
    assert!(
        pushed.text.starts_with("Waiting for approval, request #"),
        "{}",
        pushed.text
    );
}

#[test]
fn a_withdrawn_exception_stops_the_session_again() {
    let user = User::with_fake_agent().with_allowing_judge();
    app(&user);
    workspace(&user, "w");
    let (agent, mcp) = root(&user, "w");
    let asked = held(&ask_force_push(&mcp, "app/w"));
    answer(&user, asked, "approval.approve");
    passed(&user, &force_push(&mcp, "app/w"));

    let withdrawn = user
        .call(
            "exception.withdraw",
            json!({"session": agent, "rule": "force-push"}),
        )
        .unwrap();
    assert_eq!(withdrawn["exception"]["rule"], "force-push");

    assert!(exceptions(&user, &agent).is_empty());
    held(&force_push(&mcp, "app/w"));
    // Withdrawing what is not there is said.
    let again = user.call(
        "exception.withdraw",
        json!({"session": agent, "rule": "force-push"}),
    );
    assert_eq!(again.unwrap_err().0, "not-found");
}

#[test]
fn a_refused_or_unanswered_ask_changes_nothing() {
    let user = User::with_fake_agent().with_allowing_judge();
    app(&user);
    workspace(&user, "w");
    let (agent, mcp) = root(&user, "w");

    // Refused: nothing was done, and the rule stops the session as before.
    let asked = held(&ask_force_push(&mcp, "app/w"));
    let denied = answer(&user, asked, "approval.deny");
    assert!(denied.contains("was refused by the user"), "{denied}");
    assert!(exceptions(&user, &agent).is_empty());
    held(&force_push(&mcp, "app/w"));

    // Unanswered: the same.
    held(&ask_force_push(&mcp, "app/w"));
    assert!(exceptions(&user, &agent).is_empty());
    held(&force_push(&mcp, "app/w"));
}

#[test]
fn an_action_that_can_never_be_asked_for_is_refused_with_no_ask() {
    let user = User::with_fake_agent().with_allowing_judge();
    let folder = app(&user);
    workspace(&user, "w");
    let (_, mcp) = root(&user, "w");
    let asking = |args: Value| {
        let mut args = args;
        args["exception"] = json!(WHY);
        args
    };

    for (tool, args) in [
        // A shell, or a command with shell syntax.
        (
            "run",
            json!({"line": "bash -c 'git push --force origin app/w'"}),
        ),
        (
            "run",
            json!({"line": "git push --force origin app/w && true"}),
        ),
        // The value of a secret or of a credential.
        ("run", json!({"line": "gh auth token"})),
        ("fs_read", json!({"path": "~/.ssh/id_ed25519"})),
        // Writing outside the workspace.
        (
            "fs_write",
            json!({"path": folder.join("notes.md"), "content": "x\n"}),
        ),
        // Rewriting the published history of a protected branch.
        ("run", json!({"line": "git push --force origin main"})),
        ("run", json!({"line": "git push --force origin HEAD:dev"})),
        (
            "run",
            json!({"line": "git push --force origin app/w:stable/1.2"}),
        ),
        ("run", json!({"line": "git push --force origin next/2.0"})),
        // Deleting a branch on the remote is no forced push of it.
        ("run", json!({"line": "git push --force origin :app/w"})),
        ("run", json!({"line": "git push --delete origin app/w"})),
        // A forced push of another session's branch.
        ("run", json!({"line": "git push --force origin app/v"})),
        // Changing the gate's own settings or the agent's own permissions.
        (
            "fs_write",
            json!({"path": "~/.config/flottille/providers.toml", "content": "x\n"}),
        ),
        // A call that the gate does not hold needs none.
        ("fs_read", json!({"path": "README.md"})),
    ] {
        let answer = mcp.call(tool, asking(args.clone()));
        refused_with_no_ask(&user, &answer);
        assert!(
            answer.text.contains("exception"),
            "{tool} {args}: {}",
            answer.text
        );
    }
}

/// GitHub's pull request #12 of `app/login` into `main`, a required check failing.
fn failing_pull() -> Value {
    json!({"pulls": {"acme/app": {"app/login": {"number": 12, "title": "Login", "state": "OPEN",
        "isDraft": false, "baseRefName": "main", "isCrossRepository": false,
        "url": "https://github.com/acme/app/pull/12", "reviewDecision": "APPROVED",
        "statusCheckRollup": [{"__typename": "CheckRun", "name": "secrets",
            "status": "COMPLETED", "conclusion": "FAILURE"}]}}}})
}

/// The project `app` on GitHub (`acme/app`), its ready workspace `login` pushed.
fn on_github(user: &User) {
    let folder = app(user);
    user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
    user.write_project_file(
        "app",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \".\"\nforge = {{ kind = \"github\", \
             repository = \"acme/app\" }}\n",
            folder.display()
        ),
    );
    let made = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(made["state"], "ready", "{made}");
    user.git(
        &user.path("data/flottille/workspaces/app/login"),
        &["update-ref", "refs/remotes/origin/app/login", "HEAD"],
    );
}

/// The `gh pr merge` lines the fake `gh` was run with.
fn merges(user: &User) -> Vec<String> {
    user.gh_received()
        .iter()
        .filter_map(|call| {
            let args: Vec<&str> = call["args"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            args.starts_with(&["pr", "merge"]).then(|| args.join(" "))
        })
        .collect()
}

#[test]
fn a_merge_past_failing_checks_asked_for_and_accepted_bypasses_the_protection_for_the_session() {
    let user = User::with_fake_agent().with_fake_gh().with_allowing_judge();
    on_github(&user);
    user.gh_script(failing_pull());
    let (agent, mcp) = root(&user, "login");
    let merge = json!({"action": "merge", "repository": "."});

    // Blocked: its check fails, and nothing was merged.
    let blocked = mcp.call("forge", merge.clone());
    assert!(blocked.error, "{}", blocked.text);
    assert!(blocked.text.contains("its checks fail"), "{}", blocked.text);
    assert!(merges(&user).is_empty());

    // Asked for, with the pull request and the check it skips on the card.
    let mut asking = merge.clone();
    asking["exception"] = json!(WHY);
    let asked = held(&mcp.call("forge", asking));
    let request = requests(&user)
        .into_iter()
        .find(|request| request["number"] == asked)
        .unwrap();
    assert!(
        request["reasons"][0]
            .as_str()
            .unwrap()
            .contains("merge-blocked"),
        "{request}"
    );
    let done = answer(&user, asked, "approval.approve");
    assert!(done.contains("was merged"), "{done}");
    assert_eq!(
        merges(&user),
        ["pr merge 12 -R github.com/acme/app --squash --admin"]
    );
    assert_eq!(exceptions(&user, &agent), ["merge-blocked"]);

    // The next merge of the session is no longer stopped by its checks.
    passed(&user, &mcp.call("forge", merge));
    assert_eq!(merges(&user).len(), 2);
    assert!(requests(&user).is_empty(), "a request was made");
}

#[test]
fn the_exceptions_are_listed_and_withdrawn_from_the_command_line() {
    let user = User::with_fake_agent().with_allowing_judge();
    app(&user);
    workspace(&user, "w");
    let (agent, mcp) = root(&user, "w");
    let asked = held(&ask_force_push(&mcp, "app/w"));
    answer(&user, asked, "approval.approve");

    let listed = user.run(&["exception", "list", &agent]);
    assert!(listed.status.success(), "{listed:?}");
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(text.contains("force-push"), "{text}");
    assert!(text.contains(&format!("request #{asked}")), "{text}");

    let json_listed = user.run(&["exception", "list", &agent, "--json"]);
    let parsed: Value = serde_json::from_slice(&json_listed.stdout).unwrap();
    assert_eq!(parsed["exceptions"][0]["rule"], "force-push");

    let withdrawn = user.run(&["exception", "withdraw", &agent, "force-push"]);
    assert!(withdrawn.status.success(), "{withdrawn:?}");
    assert!(exceptions(&user, &agent).is_empty());
    let text =
        String::from_utf8_lossy(&user.run(&["exception", "list", &agent]).stdout).into_owned();
    assert!(text.contains("holds no exception"), "{text}");
}
