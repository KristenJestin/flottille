//! The scenarios of procedures (`docs/procedures.md`): named actions of a project with the grants
//! each run gives, shown before it runs and applied by the gate to the agents of that run only.
//! The agents are the fake agent's sessions, driven through their MCP server as their client
//! would; the forge is the fake `gh`; the remotes are throwaway bare repositories.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// A project `app` that is one repository, pushed to a bare remote `origin`, with its ready
/// workspace `login` on the branch `app/login`. Its folder and the remote's.
fn fixture(user: &User) -> (PathBuf, PathBuf) {
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
    (folder, remote)
}

/// Adds the procedure `ship` with these grants.
fn procedure(user: &User, name: &str, grants: Value, children: bool) {
    user.call(
        "procedure.add",
        json!({"project": "app", "name": name, "description": "Ship the workspace",
            "instructions": "Push the branch, then open its pull request.",
            "grants": grants, "children": children}),
    )
    .unwrap();
}

/// Runs the procedure in `login` as the CLI does once confirmed: its root agent's id and MCP
/// session.
fn run(user: &User, name: &str) -> (String, Mcp) {
    let shown = user
        .call(
            "procedure.run",
            json!({"project": "app", "name": name, "workspace": "login"}),
        )
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let started = user
        .call(
            "procedure.start",
            json!({"project": "app", "name": name, "workspace": "login",
                "grants": shown["grants"], "children": shown["children"]}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        started["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

/// A root agent started in `login` outside any procedure.
fn plain_agent(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "workspace": "login", "message": "hello"}),
    )
    .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn line(mcp: &Mcp, line: &str) -> Answer {
    mcp.call("commands_run", json!({"line": line, "timeout": 30}))
}

/// The branches the bare remote holds.
fn remote_branches(user: &User, remote: &Path) -> String {
    user.git_output(remote, &["branch", "--list"])
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn agents(user: &User) -> usize {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .len()
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_procedure_is_added_listed_shown_edited_and_removed() {
    let user = User::new();
    fixture(&user);
    // What the user wrote in the file stays around what the daemon edits.
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("# kept by hand\n{text}")).unwrap();

    let added = user.run(&[
        "procedure",
        "add",
        "app",
        "open-prs",
        "--description",
        "Open the pull requests",
        "--instructions",
        "Push, then open one pull request per repository.",
        "--grant",
        "push=allow",
        "--grant",
        "pr-create=ask",
    ]);
    assert!(added.status.success(), "{}", stderr(&added));

    let listed = stdout(&user.run(&["procedure", "list", "app"]));
    assert!(
        listed.contains("open-prs  Open the pull requests"),
        "{listed}"
    );
    assert!(listed.contains("push (allow), pr-create (ask)"), "{listed}");
    let shown = user.run(&["procedure", "show", "app", "open-prs", "--json"]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(
        shown["grants"],
        json!({"push": "allow", "pr-create": "ask"})
    );
    assert_eq!(
        shown["instructions"],
        "Push, then open one pull request per repository."
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# kept by hand\n"), "{text}");
    assert!(text.contains("[procedures.open-prs]"), "{text}");

    // A grant outside the vocabulary is refused, and nothing changes.
    let refused = user.run(&[
        "procedure",
        "edit",
        "app",
        "open-prs",
        "--grant",
        "shell=allow",
    ]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("there is no grant shell"),
        "{}",
        stderr(&refused)
    );
    let edited = user.run(&[
        "procedure",
        "edit",
        "app",
        "open-prs",
        "--grant",
        "push=ask",
        "--children",
        "true",
    ]);
    assert!(edited.status.success(), "{}", stderr(&edited));
    let shown = user
        .call(
            "procedure.show",
            json!({"project": "app", "name": "open-prs"}),
        )
        .unwrap();
    assert_eq!(shown["grants"], json!({"push": "ask"}));
    assert_eq!(shown["children"], true);
    assert_eq!(
        user.call(
            "procedure.add",
            json!({"project": "app", "name": "open-prs", "description": "d", "instructions": "i"})
        )
        .unwrap_err()
        .0,
        "refused"
    );

    let removed = user.run(&["procedure", "remove", "app", "open-prs"]);
    assert!(removed.status.success(), "{}", stderr(&removed));
    let listed = user
        .call("procedure.list", json!({"project": "app"}))
        .unwrap();
    assert_eq!(listed["procedures"], json!([]));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains("procedures"), "{text}");
    assert!(text.starts_with("# kept by hand\n"), "{text}");
}

#[test]
fn running_one_shows_its_grants_and_starts_nothing_until_confirmed() {
    let user = User::with_fake_agent();
    fixture(&user);
    procedure(
        &user,
        "ship",
        json!({"push": "allow", "pr-create": "ask"}),
        false,
    );

    // Refused, or no answer at all: nothing starts.
    for input in ["n\n", ""] {
        let output = user.run_with_input(
            &["procedure", "run", "app", "ship", "--workspace", "login"],
            input,
        );
        assert!(!output.status.success());
        let shown = stderr(&output);
        assert!(shown.contains("It grants, for this run only:"), "{shown}");
        assert!(
            shown.contains("  push (allow): push the workspace's branch"),
            "{shown}"
        );
        assert!(shown.contains("  pr-create (ask): open the workspace's pull requests"));
        assert!(shown.contains("Its children get none of them."), "{shown}");
        assert!(shown.contains("not started"), "{shown}");
    }
    assert_eq!(agents(&user), 0);
    // The protocol's first request shows them and starts nothing either.
    let shown = user
        .call(
            "procedure.run",
            json!({"project": "app", "name": "ship", "workspace": "login"}),
        )
        .unwrap();
    assert_eq!(shown["started"], false);
    assert_eq!(
        shown["grants"],
        json!({"push": "allow", "pr-create": "ask"})
    );
    assert_eq!(agents(&user), 0);
    // Grants that are no longer the procedure's are refused: what is shown is what is granted.
    let (code, message) = user
        .call(
            "procedure.start",
            json!({"project": "app", "name": "ship", "workspace": "login",
                "grants": {"push": "allow"}, "children": false}),
        )
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(
        message.contains("changed since they were shown"),
        "{message}"
    );
    assert_eq!(agents(&user), 0);

    let output = user.run_with_input(
        &["procedure", "run", "app", "ship", "--workspace", "login"],
        "y\n",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Started agent"),
        "{}",
        stdout(&output)
    );
    assert_eq!(agents(&user), 1);
    // A session like any other, in the tree, its instructions as its first message.
    let listed = user.call("agent.list", json!({})).unwrap();
    let agent = &listed["agents"][0];
    assert_eq!(agent["workspace"], "login");
    let id = agent["id"].as_str().unwrap();
    let tree = user.call("agent.tree", json!({"agent": id})).unwrap();
    assert_eq!(tree["agents"].as_array().unwrap().len(), 1);
    eventually("the brief", || {
        user.received_requests("session/prompt")
            .iter()
            .any(|prompt| prompt.to_string().contains("Push the branch, then open"))
    });
    let prompt = user.received_requests("session/prompt")[0].to_string();
    assert!(prompt.contains("You run the procedure ship"), "{prompt}");

    let yes = user.run(&[
        "procedure",
        "run",
        "app",
        "ship",
        "--workspace",
        "login",
        "--yes",
    ]);
    assert!(yes.status.success(), "{}", stderr(&yes));
    assert_eq!(agents(&user), 2);
}

#[test]
fn inside_a_run_a_granted_push_of_the_workspace_s_branch_goes_through_an_ask_one_is_held_an_ungranted_one_is_refused_and_outside_the_run_it_is_refused_as_today()
 {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    procedure(&user, "ship", json!({"push": "allow"}), false);
    procedure(&user, "careful", json!({"push": "ask"}), false);
    procedure(&user, "commits", json!({"commit": "allow"}), false);

    // Outside a run: refused, as today, and nothing reaches the remote.
    let outside = plain_agent(&user);
    let answer = line(&outside, "git push origin app/login");
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("git push is refused for agents"),
        "{}",
        answer.text
    );
    assert!(!remote_branches(&user, &remote).contains("app/login"));

    // Inside a run that does not grant it: refused.
    let (_, commits) = run(&user, "commits");
    let answer = line(&commits, "git push origin app/login");
    assert!(answer.error, "{}", answer.text);
    assert!(!remote_branches(&user, &remote).contains("app/login"));

    // Held when the run grants it with ask: nothing is pushed until the user allows it.
    let (_, careful) = run(&user, "careful");
    let answer = line(&careful, "git push origin app/login");
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("Waiting for the user's approval"),
        "{}",
        answer.text
    );
    let requests = user.call("approval.list", json!({})).unwrap()["requests"].clone();
    let request = &requests[0];
    assert_eq!(request["sensitive"], true, "never allowed for the tree");
    assert!(!remote_branches(&user, &remote).contains("app/login"));
    let number = request["number"].as_u64().unwrap();
    assert_eq!(
        user.call(
            "approval.approve",
            json!({"number": number, "for_tree": true})
        )
        .unwrap_err()
        .0,
        "refused"
    );
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the allowed push", || {
        remote_branches(&user, &remote).contains("app/login")
    });
    user.git(&remote, &["branch", "-D", "app/login"]);

    // Allowed at once when the run grants it.
    let (_, ship) = run(&user, "ship");
    let answer = line(&ship, "git push origin app/login");
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("exited with code 0"),
        "{}",
        answer.text
    );
    assert!(remote_branches(&user, &remote).contains("app/login"));

    // The same push by the agent outside the run is still refused.
    let answer = line(&outside, "git push origin app/login");
    assert!(answer.error, "{}", answer.text);
}

#[test]
fn reset_hard_stash_and_a_force_push_without_lease_are_refused_even_with_every_grant() {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    procedure(
        &user,
        "all",
        json!({"commit": "allow", "push": "allow", "push-force-with-lease": "allow",
            "rebase": "allow", "pr-create": "allow", "pr-merge-squash": "allow",
            "pipeline-rerun": "allow"}),
        false,
    );
    let (_, mcp) = run(&user, "all");

    for refused in [
        "git reset --hard HEAD",
        "git stash",
        "git push --force origin app/login",
        "git push -f origin app/login",
        "git push origin +app/login",
        "git push origin main",
        "git push --delete origin main",
    ] {
        let answer = line(&mcp, refused);
        assert!(answer.error, "{refused}: {}", answer.text);
        assert!(
            answer.text.starts_with("refused: "),
            "{refused}: {}",
            answer.text
        );
    }
    assert!(!remote_branches(&user, &remote).contains("app/login"));
    assert!(remote_branches(&user, &remote).contains("main"));
    // With a lease, it goes through.
    let answer = line(&mcp, "git push --force-with-lease origin app/login");
    assert!(!answer.error, "{}", answer.text);
    assert!(remote_branches(&user, &remote).contains("app/login"));
}

/// The fixture's workspace, its remote then made to look like GitHub's.
fn on_github(user: &User) {
    let (folder, _) = fixture(user);
    user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
}

fn gh_calls(user: &User) -> Vec<String> {
    user.gh_received()
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

fn creates(user: &User) -> usize {
    gh_calls(user)
        .iter()
        .filter(|call| call.starts_with("pr create"))
        .count()
}

#[test]
fn forge_write_creates_a_pull_request_with_the_fake_gh_only_when_granted() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(json!({"created": 12}));
    procedure(&user, "open", json!({"pr-create": "allow"}), false);
    procedure(&user, "merge", json!({"pr-merge-squash": "allow"}), false);
    let create = json!({"action": "pr-create", "repository": ".", "title": "Add the login page",
        "body": "What it does"});

    // Outside a run, and in a run that does not grant it: refused, never asked of the forge.
    let outside = plain_agent(&user);
    assert!(outside.names().contains(&"forge_write".to_owned()));
    let answer = outside.call("forge_write", create.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("only inside a procedure"),
        "{}",
        answer.text
    );
    let (_, merge) = run(&user, "merge");
    let answer = merge.call("forge_write", create.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("does not grant pr-create"),
        "{}",
        answer.text
    );
    assert_eq!(creates(&user), 0);

    let (_, open) = run(&user, "open");
    let answer = open.call("forge_write", create);
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("https://github.com/acme/app/pull/12"),
        "{}",
        answer.text
    );
    let calls = gh_calls(&user);
    let created = calls.iter().find(|c| c.starts_with("pr create")).unwrap();
    assert_eq!(
        created,
        "pr create -R acme/app --head app/login --base main --title Add the login page --body \
         What it does"
    );
    assert_eq!(creates(&user), 1);
    // The daemon's own sign-in: the agent never had a token.
    assert!(!calls.iter().any(|call| call.contains("auth token")));
}

#[test]
fn a_child_of_the_run_has_no_grant_unless_the_procedure_gives_it() {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    procedure(&user, "alone", json!({"push": "allow"}), false);
    procedure(&user, "together", json!({"push": "allow"}), true);

    for (name, inherits) in [("alone", false), ("together", true)] {
        let (_, root) = run(&user, name);
        let opened = user.received_requests("session/new").len();
        let launched = root.call(
            "agent_launch",
            json!({"title": "push", "brief": "Push the branch."}),
        );
        assert!(!launched.error, "{}", launched.text);
        let (url, authorization) = server_of(&user, opened);
        let child = Mcp::initialize(&url, &authorization);

        let answer = line(&child, "git push origin app/login");

        assert_eq!(!answer.error, inherits, "{name}: {}", answer.text);
        assert_eq!(
            remote_branches(&user, &remote).contains("app/login"),
            inherits,
            "{name}"
        );
    }
}

#[test]
fn the_grants_end_with_the_run() {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    procedure(&user, "ship", json!({"push": "allow"}), false);
    let (_, root) = run(&user, "ship");

    let reported = root.call("report", json!({"text": "Done."}));
    assert!(!reported.error, "{}", reported.text);
    let answer = line(&root, "git push origin app/login");

    assert!(answer.error, "{}", answer.text);
    assert!(!remote_branches(&user, &remote).contains("app/login"));
}
