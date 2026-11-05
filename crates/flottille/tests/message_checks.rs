//! The scenarios of the message checks (`docs/projects.md`, "Message checks"): the command lines
//! the user declares on a project, run on an agent's commit message before the commit, on a pull
//! request's title before it is opened and on a squash message before the merge; and the squash
//! message `forge merge` gives the forge. The checks are scripts of the test, never of the
//! repository; the agent is the fake ACP agent, the forges the fake `gh` and `bkt`, in throwaway
//! repositories and temporary XDG folders.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A check that refuses a message holding the word `bad`, saying its first line, and writes
/// every message it is given to `seen`.
const REFUSES_BAD: &str = "#!/bin/sh\ncat \"$1\" >> \"$(dirname \"$0\")/seen\"\n\
    if grep -q bad \"$1\"; then echo \"the message says bad: $(head -n 1 \"$1\")\"; exit 1; fi\n\
    echo fine\n";

/// A check that passes every message, and says it ran in `passed`.
const PASSES: &str = "#!/bin/sh\necho ran >> \"$(dirname \"$0\")/passed\"\n";

/// A project `app` over a throwaway repository pushed to a throwaway remote, `prepare` given its
/// folder before the workspace `login` is made (offline: nothing is fetched). The folder.
fn prepared(user: &User, prepare: impl FnOnce(&Path)) -> PathBuf {
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
    prepare(&folder);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
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

/// The workspace's checkout.
fn worktree(user: &User) -> PathBuf {
    user.path("data/flottille/workspaces/app/login")
}

/// A check script of the test, outside every repository: its path.
fn script(user: &User, name: &str, text: &str) -> PathBuf {
    let path = user.path(&format!("checks/{name}"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Declares the project's message checks, as the project's Settings does.
fn declare(user: &User, checks: &[String]) {
    user.call(
        "project.set_message_checks",
        json!({"name": "app", "checks": checks}),
    )
    .unwrap();
}

/// The check refusing `bad`, then the passing one: their lines.
fn two_checks(user: &User) -> (String, String) {
    let refusing = script(user, "refuses-bad.sh", REFUSES_BAD);
    let passing = script(user, "passes.sh", PASSES);
    (
        format!("{} {{message}}", passing.display()),
        format!("{} {{message}}", refusing.display()),
    )
}

/// What the check refusing `bad` was given, every message in turn.
fn seen(user: &User) -> String {
    std::fs::read_to_string(user.path("checks/seen")).unwrap_or_default()
}

/// A root agent started in `login`.
fn agent(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "workspace": "login", "message": "hello"}),
    )
    .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The number of a request a held call's answer names.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// A call the gate holds, allowed by the user: the answer the agent is handed for it, an error
/// when it was not done.
fn allowed(user: &User, answer: Answer) -> Answer {
    let number = held(&answer);
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let store = || Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || store().delivery(&id).unwrap().is_some());
    let text = store().delivery(&id).unwrap().unwrap().body;
    Answer {
        error: !text.contains("and Flottille has now done it"),
        text,
    }
}

/// A line the agent runs in the workspace: harmless ones answer at once, the others are allowed
/// by the user.
fn line(user: &User, mcp: &Mcp, line: &str) -> Answer {
    let answer = mcp.call("run", json!({"line": line, "timeout": 30}));
    if answer.text.starts_with("Waiting for the user's approval") {
        allowed(user, answer)
    } else {
        answer
    }
}

/// A file changed and staged in the workspace, ready to commit.
fn staged(user: &User, mcp: &Mcp) {
    let written = mcp.call(
        "fs_write",
        json!({"path": "change.txt", "content": "change\n"}),
    );
    assert!(!written.error, "{}", written.text);
    let added = line(user, mcp, "git add change.txt");
    assert!(added.text.contains("exited with code 0"), "{}", added.text);
}

/// The subjects of the workspace's branch, newest first.
fn subjects(user: &User) -> String {
    user.git_output(&worktree(user), &["log", "--format=%s"])
}

#[test]
fn a_commit_whose_message_a_check_refuses_is_not_made() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit -m 'bad change'");

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("nothing was committed"),
        "{}",
        answer.text
    );
    assert!(
        !subjects(&user).contains("bad change"),
        "{}",
        subjects(&user)
    );
    assert_eq!(seen(&user), "bad change\n");
}

#[test]
fn the_agent_is_told_which_check_failed_and_why() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing.clone(), refusing.clone()]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit -m 'bad change' -m 'its body'");

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains(&refusing), "{}", answer.text);
    assert!(!answer.text.contains(&passing), "{}", answer.text);
    assert!(
        answer.text.contains("exited with code 1"),
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("the message says bad: bad change"),
        "{}",
        answer.text
    );
    // Both messages of `-m`, a paragraph each, as git writes them.
    assert_eq!(seen(&user), "bad change\n\nits body\n");
}

#[test]
fn a_commit_passes_when_every_check_passes() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing]);
    let mcp = agent(&user);
    staged(&user, &mcp);
    std::fs::write(worktree(&user).join("message.txt"), "fix: a good change\n").unwrap();

    let answer = line(&user, &mcp, "git commit -F message.txt");

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("exited with code 0"),
        "{}",
        answer.text
    );
    assert!(
        subjects(&user).starts_with("fix: a good change"),
        "{}",
        subjects(&user)
    );
    assert_eq!(seen(&user), "fix: a good change\n");
    assert_eq!(
        std::fs::read_to_string(user.path("checks/passed")).unwrap(),
        "ran\n"
    );
}

#[test]
fn a_check_that_cannot_run_refuses_the_commit() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    let missing = user.path("checks/missing-linter");
    declare(&user, &[format!("{} {{message}}", missing.display())]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit -m 'fix: a good change'");

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("could not run"), "{}", answer.text);
    assert!(
        answer.text.contains("nothing was committed"),
        "{}",
        answer.text
    );
    assert!(
        !subjects(&user).contains("good change"),
        "{}",
        subjects(&user)
    );
}

#[test]
fn a_commit_whose_message_cannot_be_known_is_refused_when_checks_are_declared() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    // An editor would write it, after the checks.
    let answer = line(&user, &mcp, "git commit");

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("-m"), "{}", answer.text);
    assert!(
        answer.text.contains("nothing was committed"),
        "{}",
        answer.text
    );
    assert_eq!(subjects(&user).lines().count(), 1, "{}", subjects(&user));
    assert_eq!(seen(&user), "");
}

#[test]
fn no_check_declared_changes_nothing() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit -m 'bad change'");

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("exited with code 0"),
        "{}",
        answer.text
    );
    assert!(
        subjects(&user).starts_with("bad change"),
        "{}",
        subjects(&user)
    );
    let shown = user.call("project.show", json!({"name": "app"})).unwrap();
    assert!(shown.get("message_checks").is_none(), "{shown}");
}

#[test]
fn the_checks_come_from_the_project_not_the_repository() {
    let user = User::with_fake_agent();
    // The repository's own commit-msg hook, which the agent could write.
    prepared(&user, |folder| {
        let hooks = folder.join(".githooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("commit-msg");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\necho hook >> '{}'\n",
                user.path("marks").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        user.git(folder, &["add", ".githooks"]);
        user.git(folder, &["commit", "--quiet", "-m", "hooks"]);
        user.git(folder, &["push", "--quiet", "origin", "main"]);
        user.git(folder, &["config", "core.hooksPath", ".githooks"]);
    });
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing]);
    let shown = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(
        shown["message_checks"].as_array().unwrap().len(),
        2,
        "{shown}"
    );
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit -m 'fix: a good change'");

    assert!(!answer.error, "{}", answer.text);
    assert!(
        subjects(&user).starts_with("fix: a good change"),
        "{}",
        subjects(&user)
    );
    assert_eq!(seen(&user), "fix: a good change\n");
    assert!(
        !user.path("marks").exists(),
        "{}",
        std::fs::read_to_string(user.path("marks")).unwrap_or_default()
    );
    // An agent's settings change never reaches them.
    let tried = mcp.call(
        "project",
        json!({"action": "set_settings", "message_checks": []}),
    );
    assert!(tried.error, "{}", tried.text);
    let shown = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(
        shown["message_checks"].as_array().unwrap().len(),
        2,
        "{shown}"
    );
}

// The other ways to commit: the user's alias, a merge, a tree commit, a revert.

#[test]
fn a_commit_made_through_the_users_alias_is_checked() {
    let user = User::with_fake_agent();
    prepared(&user, |folder| {
        user.git(folder, &["config", "alias.ci", "commit"])
    });
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git ci -m 'bad change'");

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("nothing was committed"),
        "{}",
        answer.text
    );
    assert_eq!(seen(&user), "bad change\n");
    assert_eq!(subjects(&user).lines().count(), 1, "{}", subjects(&user));
}

/// A branch `other` with a commit of its own, beside the workspace's branch.
fn other_branch(user: &User) {
    let worktree = worktree(user);
    let branch = user.git_output(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    user.git(&worktree, &["checkout", "--quiet", "-b", "other"]);
    std::fs::write(worktree.join("other.txt"), "other\n").unwrap();
    user.git(&worktree, &["add", "other.txt"]);
    user.git(&worktree, &["commit", "--quiet", "-m", "other"]);
    user.git(&worktree, &["checkout", "--quiet", branch.trim()]);
}

#[test]
fn a_merge_message_is_checked_before_the_merge() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    other_branch(&user);
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing]);
    let mcp = agent(&user);

    let answer = line(&user, &mcp, "git merge --no-ff -m 'bad merge' other");
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("nothing was committed"),
        "{}",
        answer.text
    );
    assert!(
        !subjects(&user).contains("bad merge"),
        "{}",
        subjects(&user)
    );
    // Git would write its message itself.
    let answer = line(&user, &mcp, "git merge --no-ff other");
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("-m"), "{}", answer.text);

    let answer = line(&user, &mcp, "git merge --no-ff -m 'fix: merge other' other");
    assert!(!answer.error, "{}", answer.text);
    assert!(
        subjects(&user).starts_with("fix: merge other"),
        "{}",
        subjects(&user)
    );
    assert_eq!(seen(&user), "bad merge\nfix: merge other\n");
}

#[test]
fn a_tree_commit_message_is_checked() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing]);
    let mcp = agent(&user);

    let answer = line(
        &user,
        &mcp,
        "git commit-tree -m 'bad tree' -p HEAD HEAD^{tree}",
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("nothing was committed"),
        "{}",
        answer.text
    );
    assert_eq!(seen(&user), "bad tree\n");
}

#[test]
fn a_revert_is_refused_while_checks_are_declared() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing]);
    let mcp = agent(&user);
    staged(&user, &mcp);
    let answer = line(&user, &mcp, "git commit -m 'fix: a good change'");
    assert!(!answer.error, "{}", answer.text);

    // Git writes `Revert "…"`, which no check reads first.
    let answer = line(&user, &mcp, "git revert --no-edit HEAD");

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("--no-commit"), "{}", answer.text);
    assert!(answer.text.contains("git commit -m"), "{}", answer.text);
    assert!(
        subjects(&user).starts_with("fix: a good change"),
        "{}",
        subjects(&user)
    );
}

/// Whether the process `pid` still runs: a zombie has ended.
fn alive(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
}

#[test]
fn a_check_stops_with_the_call_that_is_given_up() {
    let user = User::with_fake_agent();
    prepared(&user, |_| {});
    // A check whose own child outlives it unless its whole group is stopped.
    let slow = script(
        &user,
        "slow.sh",
        "#!/bin/sh\nsleep 30 &\necho $$ $! > \"$(dirname \"$0\")/pids.tmp\"\n\
         mv \"$(dirname \"$0\")/pids.tmp\" \"$(dirname \"$0\")/pids\"\nwait\n",
    );
    declare(&user, &[format!("{} {{message}}", slow.display())]);
    let opened = user.received_requests("session/new").len();
    let started = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "hello"}),
        )
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(&user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    staged(&user, &mcp);
    // Full: the commit is not held, and the call is the agent's own until it ends.
    let output = user.run(&["agent", "mode", &id, "full"]);
    assert!(output.status.success(), "{output:?}");
    let pids = user.path("checks/pids");

    let given_up = Instant::now();
    mcp.call_given_up(
        "run",
        json!({"line": "git commit -m 'fix: a good change'", "timeout": 60}),
        || pids.exists(),
    );

    let pids = std::fs::read_to_string(&pids).unwrap();
    let pids: Vec<&str> = pids.split_whitespace().collect();
    assert_eq!(pids.len(), 2, "{pids:?}");
    eventually("the check's processes stop", || {
        pids.iter().all(|pid| !alive(pid))
    });
    assert!(given_up.elapsed() < Duration::from_secs(20));
    assert_eq!(subjects(&user).lines().count(), 1, "{}", subjects(&user));
}

// The forge: the title of a pull request, the message of a squash merge.

/// The fixture on GitHub (`acme/app`), its forge declared and the branch `app/login` as a push
/// leaves it: the agent's session.
fn on_github(user: &User) -> Mcp {
    prepared(user, |folder| {
        user.git(
            folder,
            &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
        )
    });
    declare_proposed(user);
    pushed(user);
    agent(user)
}

/// The same on Bitbucket Cloud (`acme/app`).
fn on_bitbucket(user: &User) -> Mcp {
    prepared(user, |folder| {
        user.git(
            folder,
            &[
                "remote",
                "set-url",
                "origin",
                "git@bitbucket.org:acme/app.git",
            ],
        )
    });
    declare_proposed(user);
    pushed(user);
    agent(user)
}

/// Saves the forges the project's settings propose from the remotes, as the user does.
fn declare_proposed(user: &User) {
    let settings = user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    let repositories: Vec<Value> = settings["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({"path": r["path"], "forge": r["proposal"]}))
        .collect();
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": repositories}),
    )
    .unwrap();
}

/// `app/login` as a push leaves it: its remote-tracking ref at its head.
fn pushed(user: &User) {
    user.git(
        &worktree(user),
        &["update-ref", "refs/remotes/origin/app/login", "HEAD"],
    );
}

fn calls(received: Vec<Value>) -> Vec<String> {
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

fn starting(calls: &[String], start: &str) -> usize {
    calls.iter().filter(|call| call.starts_with(start)).count()
}

/// The open pull request #12 of `app/login` into `main` on GitHub, its checks passing.
fn github_pull(user: &User) {
    user.gh_script(
        json!({"pulls": {"acme/app": {"app/login": {"number": 12, "title": "Login",
        "state": "OPEN", "isDraft": false, "baseRefName": "main", "isCrossRepository": false,
        "url": "https://github.com/acme/app/pull/12", "statusCheckRollup": [{
            "__typename": "CheckRun", "name": "build", "status": "COMPLETED",
            "conclusion": "SUCCESS"}]}}}}),
    );
}

#[test]
fn a_pull_request_title_is_checked_before_it_is_created() {
    let user = User::with_fake_agent().with_fake_gh();
    let mcp = on_github(&user);
    user.gh_script(json!({"created": 12}));
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing.clone()]);

    let answer = allowed(
        &user,
        mcp.call(
            "forge",
            json!({"action": "create", "repository": ".", "title": "bad title", "body": "x"}),
        ),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains(&refusing), "{}", answer.text);
    assert!(
        answer.text.contains("the message says bad: bad title"),
        "{}",
        answer.text
    );
    assert_eq!(starting(&calls(user.gh_received()), "pr create"), 0);

    let answer = allowed(
        &user,
        mcp.call(
            "forge",
            json!({"action": "create", "repository": ".", "title": "feat: add the login",
                "body": "x"}),
        ),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(starting(&calls(user.gh_received()), "pr create"), 1);
    assert_eq!(seen(&user), "bad title\nfeat: add the login\n");
}

#[test]
fn a_squash_message_is_given_to_the_forge() {
    let message = "feat: add the login\n\nThe page and its form.";
    // GitHub: its subject and its body.
    let user = User::with_fake_agent().with_fake_gh();
    let mcp = on_github(&user);
    github_pull(&user);
    let merge = json!({"action": "merge", "repository": ".", "message": message});
    let answer = mcp.call("forge", merge.clone());
    // The user reads it whole on the card.
    let number = held(&answer);
    let requests = user.call("approval.list", json!({})).unwrap()["requests"].clone();
    let request = requests
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .unwrap()
        .clone();
    assert!(
        request["says"]
            .to_string()
            .contains("Its message: feat: add the login\\n\\nThe page and its form."),
        "{request}"
    );
    user.call("approval.deny", json!({"number": number})).ok();
    let answer = allowed(&user, mcp.call("forge", merge));
    assert!(!answer.error, "{}", answer.text);
    let calls_made = calls(user.gh_received());
    assert!(
        calls_made.contains(
            &"pr merge 12 -R github.com/acme/app --squash --subject feat: add the login --body \
              The page and its form."
                .to_owned()
        ),
        "{calls_made:?}"
    );

    // Bitbucket: the whole message.
    let user = User::with_fake_agent().with_fake_bkt();
    let mcp = on_bitbucket(&user);
    user.bkt_script(
        json!({"pulls": {"acme/app": [{"id": 12, "title": "Login", "state": "OPEN",
            "source": {"branch": {"name": "app/login"}, "repository": {"full_name": "acme/app"}},
            "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}},
            "links": {"html": {"href": "https://bitbucket.org/acme/app/pull-requests/12"}}}]},
        "checks": {"acme/app": {"12": [{"state": "SUCCESSFUL", "name": "build"}]}},
        "api": {"/repositories/acme/app/pullrequests/12": {"id": 12, "participants": []}}}),
    );
    let answer = allowed(
        &user,
        mcp.call(
            "forge",
            json!({"action": "merge", "repository": ".", "message": message}),
        ),
    );
    assert!(!answer.error, "{}", answer.text);
    let made = user.bkt_received();
    let merged = made
        .iter()
        .find(|call| call["args"][1] == "merge")
        .unwrap_or_else(|| panic!("{made:?}"));
    let args: Vec<&str> = merged["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let at = args
        .iter()
        .position(|a| *a == "--message")
        .expect("--message");
    assert_eq!(args[at + 1], message, "{args:?}");
}

#[test]
fn a_squash_message_is_checked_before_the_merge() {
    let user = User::with_fake_agent().with_fake_gh();
    let mcp = on_github(&user);
    github_pull(&user);
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing.clone()]);

    let answer = allowed(
        &user,
        mcp.call(
            "forge",
            json!({"action": "merge", "repository": ".", "message": "bad squash\n\nbody"}),
        ),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains(&refusing), "{}", answer.text);
    assert!(
        answer.text.contains("nothing was merged"),
        "{}",
        answer.text
    );
    assert_eq!(starting(&calls(user.gh_received()), "pr merge"), 0);
    assert_eq!(seen(&user), "bad squash\n\nbody\n");
}

#[test]
fn a_squash_message_with_another_method_is_refused() {
    let user = User::with_fake_agent().with_fake_gh();
    let mcp = on_github(&user);
    github_pull(&user);

    for method in ["merge", "rebase"] {
        let answer = mcp.call(
            "forge",
            json!({"action": "merge", "repository": ".", "method": method,
                "message": "feat: add the login"}),
        );
        assert!(answer.error, "{method}: {}", answer.text);
        assert!(answer.text.contains("squash"), "{method}: {}", answer.text);
    }
    // A message is for a merge alone.
    let answer = mcp.call(
        "forge",
        json!({"action": "create", "repository": ".", "title": "x", "message": "y"}),
    );
    assert!(answer.error, "{}", answer.text);
    let made = calls(user.gh_received());
    assert_eq!(
        starting(&made, "pr merge") + starting(&made, "pr create"),
        0,
        "{made:?}"
    );
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_pull_request_title_is_checked_before_it_is_edited() {
    let user = User::with_fake_agent().with_fake_gh();
    let mcp = on_github(&user);
    github_pull(&user);
    let (passing, refusing) = two_checks(&user);
    declare(&user, &[passing, refusing.clone()]);

    let answer = allowed(
        &user,
        mcp.call(
            "forge",
            json!({"action": "edit", "repository": ".", "number": 12, "title": "bad title"}),
        ),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains(&refusing), "{}", answer.text);
    assert!(
        answer.text.contains("nothing was written to the forge"),
        "{}",
        answer.text
    );
    assert_eq!(starting(&calls(user.gh_received()), "pr edit"), 0);

    let answer = allowed(
        &user,
        mcp.call(
            "forge",
            json!({"action": "edit", "repository": ".", "number": 12,
                "title": "feat: add the login"}),
        ),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(starting(&calls(user.gh_received()), "pr edit"), 1);
    assert_eq!(seen(&user), "bad title\nfeat: add the login\n");
}
