//! A stacked workspace (#1323, #1245): started from another ticket's branch, it opens and merges
//! its pull request once that branch is merged and deleted (`docs/forges.md`, "A workspace's
//! base"). The forge is the fake `gh` and `bkt`; the remotes are throwaway bare repositories; every
//! write is held for the user and done once they allow it.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

const STACKED: &str = "chore/2-start";

/// A project `app` of one repository on `origin` (a throwaway bare remote whose address is then
/// the forge's), with `main` and the branch `chore/2-start` on it, and its ready workspace `login`
/// started from `chore/2-start` on the branch `app/login`, pushed.
fn project(user: &User, origin: &str, forge: &str) {
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
    user.git(&folder, &["switch", "--quiet", "-c", STACKED]);
    std::fs::write(folder.join("parent.md"), "the parent\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "the parent"]);
    user.git(&folder, &["push", "--quiet", "origin", STACKED]);
    user.git(&folder, &["switch", "--quiet", "main"]);
    user.git(&folder, &["remote", "set-url", "origin", origin]);
    user.write_project_file(
        "app",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \".\"\nbase = [\"main\"]\nforge = \
             {forge}\n",
            folder.display()
        ),
    );
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true,
                "bases": {".": STACKED}}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    assert_eq!(base_of(user), STACKED);
    let worktree = user.path("data/flottille/workspaces/app/login");
    user.git(
        &worktree,
        &["update-ref", "refs/remotes/origin/app/login", "HEAD"],
    );
}

fn on_github(user: &User) {
    project(
        user,
        "git@github.com:acme/app.git",
        "{ kind = \"github\", repository = \"acme/app\" }",
    );
}

fn on_bitbucket(user: &User) {
    project(
        user,
        "git@bitbucket.org:acme/app.git",
        "{ kind = \"bitbucket-cloud\", repository = \"acme/app\" }",
    );
}

/// The base branch the workspace `login` records for its repository.
fn base_of(user: &User) -> String {
    let shown = user
        .call("workspace.show", json!({"project": "app", "name": "login"}))
        .unwrap();
    shown["repositories"][0]["base"]["branch"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A root agent started in the workspace `login`: its id and its tools.
fn agent(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "hello"}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        started["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A call the gate holds, allowed by the user: the answer the agent is handed for it, an error
/// when it was not done.
fn allowed(user: &User, answer: Answer) -> Answer {
    assert!(!answer.error, "{}", answer.text);
    let number: u64 = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let store = || Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || store().delivery(&id).unwrap().is_some());
    let text = store().delivery(&id).unwrap().unwrap().body;
    Answer {
        error: !text.contains("and Flottille has now done it"),
        text,
    }
}

/// A write of `forge`, allowed by the user.
fn write(user: &User, mcp: &Mcp, args: Value) -> Answer {
    allowed(user, mcp.call("forge", args))
}

/// What the agent was told by Flottille, every delivery's text.
fn told(user: &User, agent: &str) -> String {
    let store = Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap();
    store
        .last_deliveries(agent, 32)
        .unwrap()
        .into_iter()
        .map(|delivery| delivery.body)
        .collect::<Vec<_>>()
        .join("\n")
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

fn gh_starting(user: &User, start: &str) -> Vec<String> {
    calls(user.gh_received())
        .into_iter()
        .filter(|call| call.starts_with(start))
        .collect()
}

fn bkt_starting(user: &User, start: &str) -> Vec<String> {
    calls(user.bkt_received())
        .into_iter()
        .filter(|call| call.starts_with(start))
        .collect()
}

fn passing() -> Value {
    json!([{"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
        "conclusion": "SUCCESS"}])
}

/// GitHub's script: the pull request #12 of `app/login` into `base`, open and passing; the
/// branches the repository has; and the pull requests of other branches (`others`: by branch).
fn github(base: &str, branches: &[&str], others: Value) -> Value {
    let mut pulls = json!({"app/login": {"number": 12, "title": "Login", "state": "OPEN",
        "isDraft": false, "baseRefName": base, "isCrossRepository": false,
        "url": "https://github.com/acme/app/pull/12", "statusCheckRollup": passing()}});
    for (branch, pull) in others.as_object().into_iter().flatten() {
        pulls[branch] = pull.clone();
    }
    json!({"created": 12, "branches": {"acme/app": branches}, "pulls": {"acme/app": pulls}})
}

/// The pull request of the parent branch, merged into `base` and its branch deleted.
fn merged_parent(base: &str) -> Value {
    json!({STACKED: {"number": 9, "title": "The parent", "state": "MERGED",
        "isDraft": false, "baseRefName": base, "isCrossRepository": false,
        "url": "https://github.com/acme/app/pull/9", "statusCheckRollup": passing()}})
}

fn cloud_pull(id: u64, branch: &str, state: &str, base: &str) -> Value {
    json!({"id": id, "title": "Login", "state": state,
        "source": {"branch": {"name": branch}, "repository": {"full_name": "acme/app"}},
        "destination": {"branch": {"name": base}, "repository": {"full_name": "acme/app"}},
        "links": {"html": {"href": format!("https://bitbucket.org/acme/app/pull-requests/{id}")}}})
}

/// Bitbucket Cloud's script: the pull request #12 of `app/login` into `base`, open and passing,
/// `others` (more pull requests), and the branches the repository has.
fn cloud(base: &str, others: Vec<Value>, branches: &[&str]) -> Value {
    let mut pulls = vec![cloud_pull(12, "app/login", "OPEN", base)];
    pulls.extend(others);
    let mut api = json!({"/repositories/acme/app/pullrequests/12":
        {"id": 12, "state": "OPEN", "participants": []}});
    for branch in branches {
        api[format!("/repositories/acme/app/refs/branches/{branch}")] = json!({"name": branch});
    }
    json!({"created": 12, "pulls": {"acme/app": pulls},
        "checks": {"acme/app": {"12": [{"state": "SUCCESSFUL", "name": "build"}]}}, "api": api})
}

#[test]
fn a_workspace_whose_start_branch_was_deleted_opens_its_pull_request_into_the_project_s_base() {
    // GitHub.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github("main", &["main"], json!({})));
    let (id, mcp) = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Add the login page"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr create"),
        [
            "pr create -R github.com/acme/app --head app/login --base main --title Add the login \
          page --body "
        ]
    );
    // Said, not silent: the start branch is gone, and the workspace goes into the project's base.
    assert!(
        answer.text.contains(STACKED) && answer.text.contains("into main"),
        "{}",
        answer.text
    );
    assert_eq!(base_of(&user), "main");
    assert!(
        told(&user, &id).contains(&format!("moved from {STACKED} to main")),
        "{}",
        told(&user, &id)
    );

    // Bitbucket Cloud.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user);
    user.bkt_script(cloud("main", vec![], &["main"]));
    let (_, mcp) = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Add the login page"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr create"),
        [
            "pr create --context cloud --workspace acme --repo app --source app/login --target \
          main --title Add the login page --description "
        ]
    );
    assert!(answer.text.contains(STACKED), "{}", answer.text);
    assert_eq!(base_of(&user), "main");
}

#[test]
fn a_start_branch_that_is_still_there_is_the_base_of_a_pull_request_as_before() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github("main", &["main", STACKED], json!({})));
    let (_, mcp) = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Add the login page"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert!(
        gh_starting(&user, "pr create")[0].contains(&format!("--base {STACKED} ")),
        "{:?}",
        gh_starting(&user, "pr create")
    );
    assert_eq!(base_of(&user), STACKED);
}

#[test]
fn forge_create_takes_a_base() {
    // GitHub.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github("main", &["main", "release", STACKED], json!({})));
    let (_, mcp) = agent(&user);
    let create = |base: &str| {
        json!({"action": "create", "repository": ".", "title": "Add the login page",
            "base": base})
    };
    let answer = write(&user, &mcp, create("release"));
    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("into release"), "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr create"),
        [
            "pr create -R github.com/acme/app --head app/login --base release --title Add the \
          login page --body "
        ]
    );
    // A base the forge has not got is refused before anything is opened.
    let unknown = write(&user, &mcp, create("nowhere"));
    assert!(
        unknown
            .text
            .contains("refused: acme/app has no branch nowhere on the forge; nothing was written"),
        "{}",
        unknown.text
    );
    assert_eq!(gh_starting(&user, "pr create").len(), 1);
    // A pull request opened into a base the user did not ask for does not move the workspace's.
    assert_eq!(base_of(&user), STACKED);

    // Bitbucket Cloud.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user);
    user.bkt_script(cloud("main", vec![], &["main", "release", STACKED]));
    let (_, mcp) = agent(&user);
    let answer = write(&user, &mcp, create("release"));
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr create"),
        [
            "pr create --context cloud --workspace acme --repo app --source app/login --target \
          release --title Add the login page --description "
        ]
    );
    let unknown = write(&user, &mcp, create("nowhere"));
    assert!(
        unknown
            .text
            .contains("refused: acme/app has no branch nowhere on the forge; nothing was written"),
        "{}",
        unknown.text
    );
    assert_eq!(bkt_starting(&user, "pr create").len(), 1);
}

#[test]
fn a_pull_request_retargeted_to_the_project_s_base_is_merged_by_its_workspace() {
    // GitHub: retargeted with `forge edit`, then merged.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github(STACKED, &["main", STACKED], json!({})));
    let (_, mcp) = agent(&user);
    let merge = json!({"action": "merge", "repository": "."});
    let edit = json!({"action": "edit", "repository": ".", "number": 12, "base": "main"});
    let moved = write(&user, &mcp, edit);
    assert!(!moved.error, "{}", moved.text);
    // The forge now has it into main.
    user.gh_script(github("main", &["main", STACKED], json!({})));
    let merged = write(&user, &mcp, merge.clone());
    assert!(!merged.error, "{}", merged.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash"]
    );

    // Bitbucket Cloud.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user);
    user.bkt_script(cloud(STACKED, vec![], &["main", STACKED]));
    let (_, mcp) = agent(&user);
    let edit = json!({"action": "edit", "repository": ".", "number": 12, "base": "main"});
    let moved = write(&user, &mcp, edit);
    assert!(!moved.error, "{}", moved.text);
    user.bkt_script(cloud("main", vec![], &["main", STACKED]));
    let merged = write(&user, &mcp, merge);
    assert!(!merged.error, "{}", merged.text);
    assert_eq!(
        bkt_starting(&user, "pr merge"),
        [
            "pr merge 12 --context cloud --workspace acme --repo app --strategy squash \
          --close-source=false"
        ]
    );
}

#[test]
fn a_retargeted_pull_request_moves_the_workspace_s_base_and_merges() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github(STACKED, &["main", STACKED], json!({})));
    let (id, mcp) = agent(&user);
    let seen = user.watch(&["--after", "0"]);

    let moved = write(
        &user,
        &mcp,
        json!({"action": "edit", "repository": ".", "number": 12, "base": "main"}),
    );
    assert!(!moved.error, "{}", moved.text);
    // The workspace's base follows the forge's.
    assert_eq!(base_of(&user), "main");
    // Said to the session in words, once, and to every client as an event.
    let said = told(&user, &id);
    assert!(
        said.contains(&format!("moved from {STACKED} to main")),
        "{said}"
    );
    seen.until(|e| {
        e["name"] == "workspace.base_changed"
            && e["data"]["workspace"] == "login"
            && e["data"]["base"] == "main"
            && e["data"]["was"] == STACKED
    });

    user.gh_script(github("main", &["main", STACKED], json!({})));
    let merged = write(
        &user,
        &mcp,
        json!({"action": "merge", "repository": ".", "method": "squash"}),
    );
    assert!(!merged.error, "{}", merged.text);
    assert!(merged.text.contains("into main"), "{}", merged.text);
    assert_eq!(gh_starting(&user, "pr merge").len(), 1);
}

#[test]
fn the_retarget_of_a_pull_request_that_is_not_the_workspace_s_own_leaves_its_base() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let other = json!({"someone/else": {"number": 13, "title": "Else", "state": "OPEN",
        "isDraft": false, "baseRefName": STACKED, "isCrossRepository": false,
        "url": "https://github.com/acme/app/pull/13", "statusCheckRollup": passing()}});
    user.gh_script(github(STACKED, &["main", STACKED], other));
    let (_, mcp) = agent(&user);

    let moved = write(
        &user,
        &mcp,
        json!({"action": "edit", "repository": ".", "number": 13, "base": "main"}),
    );
    assert!(!moved.error, "{}", moved.text);
    assert_eq!(base_of(&user), STACKED);
}

#[test]
fn a_retarget_the_forge_refused_leaves_the_workspace_s_base() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    // `main` is not on the forge: nothing is written.
    user.gh_script(github(STACKED, &[STACKED], json!({})));
    let (_, mcp) = agent(&user);

    let moved = write(
        &user,
        &mcp,
        json!({"action": "edit", "repository": ".", "number": 12, "base": "main"}),
    );
    assert!(moved.text.contains("refused"), "{}", moved.text);
    assert_eq!(base_of(&user), STACKED);
}

#[test]
fn a_stacked_workspace_follows_its_parent_once_merged() {
    // GitHub retargets the pull request to where the parent went, when the parent is merged and
    // its branch deleted: the workspace's base follows, and the merge is done.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github("main", &["main"], merged_parent("main")));
    let (id, mcp) = agent(&user);
    let merged = write(
        &user,
        &mcp,
        json!({"action": "merge", "repository": ".", "method": "squash"}),
    );
    assert!(!merged.error, "{}", merged.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash"]
    );
    assert_eq!(base_of(&user), "main");
    assert!(
        told(&user, &id).contains(&format!("moved from {STACKED} to main")),
        "{}",
        told(&user, &id)
    );

    // Bitbucket Cloud, the parent merged into `release`, which the workspace now follows.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user);
    user.bkt_script(cloud(
        "release",
        vec![cloud_pull(9, STACKED, "MERGED", "release")],
        &["main", "release"],
    ));
    let (_, mcp) = agent(&user);
    let merged = write(&user, &mcp, json!({"action": "merge", "repository": "."}));
    assert!(!merged.error, "{}", merged.text);
    assert_eq!(bkt_starting(&user, "pr merge").len(), 1);
    assert_eq!(base_of(&user), "release");
}

#[test]
fn a_pull_request_into_another_base_is_still_refused() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let (_, mcp) = agent(&user);
    let merge = json!({"action": "merge", "repository": "."});
    let refused = |answer: &Answer, into: &str, not: &str| {
        assert!(answer.error, "{}", answer.text);
        assert!(
            answer
                .text
                .contains(&format!("into {into}, not {not}, the workspace's base")),
            "{}",
            answer.text
        );
    };

    // Nobody retargeted it, and the parent is still there.
    user.gh_script(github("release", &["main", "release", STACKED], json!({})));
    refused(&write(&user, &mcp, merge.clone()), "release", STACKED);
    // Into the project's base, with the parent still there: the text of a remote is no proof.
    user.gh_script(github("main", &["main", STACKED], merged_parent("main")));
    refused(&write(&user, &mcp, merge.clone()), "main", STACKED);
    // The parent is gone, merged into main, and the pull request goes somewhere else.
    user.gh_script(github(
        "release",
        &["main", "release"],
        merged_parent("main"),
    ));
    refused(&write(&user, &mcp, merge.clone()), "release", "main");
    // The parent is gone with no merged pull request: only the project's base is where it went.
    user.gh_script(github("release", &["main", "release"], json!({})));
    refused(&write(&user, &mcp, merge), "release", "main");

    assert!(gh_starting(&user, "pr merge").is_empty());
    assert_eq!(base_of(&user), STACKED);
}
