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
    prepared(user, |_| {}, false)
}

/// The same, `prepare` given the project's folder before the workspace is created (`offline`
/// when the remote it leaves cannot be fetched).
fn prepared(user: &User, prepare: impl FnOnce(&Path), offline: bool) -> (PathBuf, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = bare(user, "app");
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
            json!({"project": "app", "name": "login", "offline": offline}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    (folder, remote)
}

/// Saves the forges the project's settings propose from the remotes, as the user does in the
/// window: nothing is declared otherwise.
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

/// A throwaway bare repository `remotes/<name>.git`.
fn bare(user: &User, name: &str) -> PathBuf {
    let remote = user.path(&format!("remotes/{name}.git"));
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    remote
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
    plain_agent_with_id(user).1
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
        json!({"push": "allow", "commit": "ask"}),
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
        assert!(shown.contains("  commit (ask): make commits"));
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
    assert_eq!(shown["grants"], json!({"push": "allow", "commit": "ask"}));
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
fn a_procedure_that_writes_to_the_forge_refuses_to_start_on_a_repository_with_none() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    procedure(&user, "open", json!({"pr-create": "ask"}), false);
    procedure(&user, "push", json!({"push": "ask"}), false);

    let shown = user
        .call(
            "procedure.run",
            json!({"project": "app", "name": "open", "workspace": "login"}),
        )
        .unwrap_err();
    let started = user
        .call(
            "procedure.start",
            json!({"project": "app", "name": "open", "workspace": "login",
                "grants": {"pr-create": "ask"}, "children": false}),
        )
        .unwrap_err();

    for (code, message) in [shown, started] {
        assert_eq!(code, "refused");
        assert!(
            message.starts_with(
                "procedure open writes to the forge (pr-create), and no forge is set for . of \
                 workspace login"
            ),
            "{message}"
        );
    }
    assert_eq!(agents(&user), 0);
    assert!(user.gh_received().is_empty());
    // A procedure that writes nothing to the forge needs none.
    user.call(
        "procedure.run",
        json!({"project": "app", "name": "push", "workspace": "login"}),
    )
    .unwrap();
}

#[test]
fn reset_hard_stash_and_a_force_push_without_lease_are_refused_even_with_every_grant() {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    procedure(
        &user,
        "all",
        // Every grant of git lines: those of the forge need a declared forge, and run no line.
        json!({"commit": "allow", "push": "allow", "push-force-with-lease": "allow",
            "rebase": "allow"}),
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

/// The fixture's workspace, its remote GitHub's `acme/app` (created offline: nothing is fetched
/// from it). The project's folder.
fn on_github(user: &User) -> PathBuf {
    let (folder, _) = prepared(
        user,
        |folder| {
            user.git(
                folder,
                &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
            )
        },
        true,
    );
    declare_proposed(user);
    folder
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

    // Outside a run, the tool is not listed and a call of it is refused; in a run that grants
    // another action it is listed and the gate refuses this one: never asked of the forge.
    let outside = plain_agent(&user);
    assert!(!outside.names().contains(&"forge_write".to_owned()));
    let answer = outside.call("forge_write", create.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("has no tool forge_write"),
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
        "pr create -R github.com/acme/app --head app/login --base main --title Add the login page --body \
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
fn a_run_s_children_get_its_grants_when_the_file_says_nothing_and_none_when_it_says_children_false()
{
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    // Written by hand: one says nothing of its children, the other keeps its grants from them.
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    let procedure = |name: &str, more: &str| {
        format!(
            "\n[procedures.{name}]\ndescription = \"Ship the workspace\"\n\
             instructions = \"Push the branch.\"\n{more}[procedures.{name}.grants]\n\
             push = \"allow\"\n"
        )
    };
    std::fs::write(
        &file,
        format!(
            "{text}{}{}",
            procedure("silent", ""),
            procedure("alone", "children = false\n")
        ),
    )
    .unwrap();

    for (name, inherits) in [("alone", false), ("silent", true)] {
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
    // Editing the one that said `children = false` keeps it.
    user.call(
        "procedure.edit",
        json!({"project": "app", "name": "alone", "description": "Ship it"}),
    )
    .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("children = false"), "{text}");
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

#[test]
fn a_run_s_grants_are_read_from_its_agent_while_it_goes_on_and_none_for_another() {
    let user = User::with_fake_agent();
    fixture(&user);
    procedure(
        &user,
        "ship",
        json!({"push": "allow", "commit": "ask"}),
        true,
    );
    let (agent, root) = run(&user, "ship");

    let held = user.call("procedure.of", json!({"agent": agent})).unwrap();

    assert_eq!(held["run"]["procedure"], "ship");
    assert_eq!(held["run"]["workspace"], "login");
    assert_eq!(held["run"]["children"], true);
    assert_eq!(held["run"]["ended"], false);
    assert_eq!(
        held["run"]["grants"],
        json!({"push": "allow", "commit": "ask"})
    );
    assert_eq!(held["run"]["says"].as_array().unwrap().len(), 2);
    let plain = user
        .call("procedure.of", json!({"agent": "no-such-agent"}))
        .unwrap();
    assert!(plain["run"].is_null(), "{plain}");

    root.call("report", json!({"text": "Done."}));
    let ended = user.call("procedure.of", json!({"agent": agent})).unwrap();
    assert_eq!(ended["run"]["ended"], true);
}

/// A hook that leaves a mark in `marks` each time it runs.
fn marking_hook(user: &User, folder: &Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let hooks = folder.join(".githooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join(name);
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\necho {name} >> '{}'\n",
            user.path("marks").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_granted_commit_push_or_rebase_runs_none_of_the_hooks_the_agent_can_write() {
    let user = User::with_fake_agent();
    // The repository's hooks are files of its tree, which the agent writes.
    let (folder, remote) = prepared(
        &user,
        |folder| {
            for hook in ["pre-commit", "pre-push", "pre-rebase"] {
                marking_hook(&user, folder, hook);
            }
            user.git(folder, &["add", ".githooks"]);
            user.git(folder, &["commit", "--quiet", "-m", "hooks"]);
            user.git(folder, &["push", "--quiet", "origin", "main"]);
            user.git(folder, &["config", "core.hooksPath", ".githooks"]);
        },
        false,
    );
    procedure(
        &user,
        "ship",
        json!({"commit": "allow", "push": "allow", "rebase": "allow"}),
        false,
    );
    let (_, mcp) = run(&user, "ship");

    let written = mcp.call(
        "fs_write",
        json!({"path": "change.txt", "content": "change\n"}),
    );
    assert!(!written.error, "{}", written.text);
    for done in [
        "git add change.txt",
        "git commit -m change",
        "git rebase main",
        "git push origin app/login",
    ] {
        let answer = line(&mcp, done);
        assert!(
            !answer.error && answer.text.contains("exited with code 0"),
            "{done}: {}",
            answer.text
        );
    }

    assert!(remote_branches(&user, &remote).contains("app/login"));
    assert!(
        !user.path("marks").exists(),
        "{}",
        std::fs::read_to_string(user.path("marks")).unwrap_or_default()
    );
    // The same hooks run for the user.
    std::fs::write(folder.join("other.txt"), "other\n").unwrap();
    user.git(&folder, &["add", "other.txt"]);
    user.git(&folder, &["commit", "--quiet", "-m", "other"]);
    assert_eq!(
        std::fs::read_to_string(user.path("marks")).unwrap(),
        "pre-commit\n"
    );
}

#[test]
fn an_agent_cannot_point_a_remote_elsewhere_and_a_push_to_a_remote_changed_since_the_workspace_was_created_is_refused()
 {
    let user = User::with_fake_agent();
    let (folder, remote) = fixture(&user);
    let evil = bare(&user, "evil");
    let shown = user
        .call("workspace.show", json!({"project": "app", "name": "login"}))
        .unwrap();
    assert_eq!(
        shown["repositories"][0]["base"]["urls"],
        json!([remote.to_str().unwrap()]),
        "{shown}"
    );
    procedure(&user, "ship", json!({"push": "allow"}), false);
    let (_, mcp) = run(&user, "ship");

    let evil_url = evil.to_str().unwrap();
    for refused in [
        format!("git remote set-url origin {evil_url}"),
        format!("git remote add evil {evil_url}"),
        format!("git config remote.origin.pushurl {evil_url}"),
    ] {
        let answer = line(&mcp, &refused);
        assert!(answer.error, "{refused}: {}", answer.text);
    }
    // Moved anyway, by a way the gate does not see: the push is refused before git runs.
    user.git(
        &folder,
        &["remote", "set-url", "--push", "origin", evil_url],
    );
    let answer = line(&mcp, "git push origin app/login");

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("no longer leads where it led"),
        "{}",
        answer.text
    );
    assert!(!remote_branches(&user, &evil).contains("app/login"));
    assert!(!remote_branches(&user, &remote).contains("app/login"));
}

#[test]
fn forge_write_refuses_a_remote_changed_since_the_workspace_was_created() {
    let user = User::with_fake_agent().with_fake_gh();
    let folder = on_github(&user);
    procedure(&user, "open", json!({"pr-create": "allow"}), false);
    let (_, open) = run(&user, "open");
    user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:evil/app.git"],
    );

    let answer = open.call(
        "forge_write",
        json!({"action": "pr-create", "repository": ".", "title": "Add the login page"}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("no longer leads where it led"),
        "{}",
        answer.text
    );
    assert_eq!(creates(&user), 0);
}

#[test]
fn pr_merge_squash_merges_only_the_workspace_s_own_pull_request_into_its_base() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    procedure(&user, "merge", json!({"pr-merge-squash": "allow"}), false);
    let (_, mcp) = run(&user, "merge");
    let merge = json!({"action": "pr-merge-squash", "repository": "."});
    let merges = |user: &User| {
        gh_calls(user)
            .iter()
            .filter(|call| call.starts_with("pr merge"))
            .count()
    };
    let pull = |cross: bool, base: &str| {
        json!({"pulls": {"acme/app": {"app/login": {"number": 12, "title": "Login",
            "state": "OPEN", "isDraft": false, "baseRefName": base, "isCrossRepository": cross,
            "url": "https://github.com/acme/app/pull/12"}}}})
    };

    // A fork's pull request of a branch of the same name.
    user.gh_script(pull(true, "main"));
    let answer = mcp.call("forge_write", merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("from a fork"), "{}", answer.text);
    // One into another base.
    user.gh_script(pull(false, "release"));
    let answer = mcp.call("forge_write", merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("into release, not main"),
        "{}",
        answer.text
    );
    assert_eq!(merges(&user), 0);

    user.gh_script(pull(false, "main"));
    let answer = mcp.call("forge_write", merge);
    assert!(!answer.error, "{}", answer.text);
    assert!(
        gh_calls(&user).contains(&"pr merge 12 -R github.com/acme/app --squash".to_owned()),
        "{:?}",
        gh_calls(&user)
    );
}

/// The gate refuses `flottille`, and the daemon the clients its agents start (#93): without
/// them, an agent reaches the daemon's socket through `commands_run`. The agent names the socket's
/// folder itself, which a run's environment no longer says.
#[test]
fn an_agent_s_commands_run_of_flottille_procedure_start_starts_no_run() {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    procedure(&user, "plain", json!({}), false);
    let (_, mcp) = run(&user, "plain");
    let before = agents(&user);
    let flottille = format!(
        "env XDG_RUNTIME_DIR={} {}",
        user.path("runtime").display(),
        env!("CARGO_BIN_EXE_flottille")
    );

    for done in [
        format!(
            "{flottille} procedure add app ship --description Ship --instructions Push \
             --grant push=allow"
        ),
        format!("{flottille} procedure run app ship --workspace login --yes"),
    ] {
        let answer = line(&mcp, &done);
        // Approved by the user, the line changes nothing either.
        if answer.text.contains("Waiting for the user's approval") {
            let requests = user.call("approval.list", json!({})).unwrap()["requests"].clone();
            let number = requests.as_array().unwrap().last().unwrap()["number"].clone();
            user.call("approval.approve", json!({"number": number}))
                .unwrap();
            user.watch(&["--after", "0"])
                .until(|e| e["name"] == "approval.settled" && e["data"]["number"] == number);
        }
    }

    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(agents(&user), before);
    assert!(
        user.call("procedure.show", json!({"project": "app", "name": "ship"}))
            .is_err()
    );
    assert!(!remote_branches(&user, &remote).contains("app/login"));
}

/// A project `app` whose folder holds the repositories `lib`, `api` and `web`, `api` and `web`
/// built on `lib`, each pushed to its bare remote `remotes/<name>.git`, with `more` at the end of
/// its file; its ready workspace `login`, on `app/login`. On GitHub's `acme/<name>` when `github`
/// (the bare remote still taking the pushes, through the user's fake `ssh`), the workspace then
/// made offline.
/// The workspace's folder.
fn stack(user: &User, github: bool, more: &str) -> PathBuf {
    let folder = user.path("work/app");
    for name in ["lib", "api", "web"] {
        let repository = folder.join(name);
        user.git_init(&repository);
        std::fs::write(repository.join("README.md"), format!("{name}\n")).unwrap();
        user.git(&repository, &["add", "."]);
        user.git(&repository, &["commit", "--quiet", "-m", "start"]);
        let remote = bare(user, name);
        let remote = remote.to_str().unwrap();
        user.git(&repository, &["remote", "add", "origin", remote]);
        user.git(&repository, &["push", "--quiet", "origin", "main"]);
        if github {
            let url = format!("git@github.com:acme/{name}.git");
            user.git(&repository, &["remote", "set-url", "origin", &url]);
        }
    }
    // On GitHub, each repository's forge is declared, as the user does in the settings.
    let forge = |name: &str| match github {
        true => format!("forge = {{ kind = \"github\", repository = \"acme/{name}\" }}\n"),
        false => String::new(),
    };
    user.write_project_file(
        "app",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \"lib\"\n{}\n[[repositories]]\npath = \
             \"api\"\ndepends_on = [\"lib\"]\n{}\n[[repositories]]\npath = \"web\"\ndepends_on = \
             [\"lib\"]\n{}{more}",
            folder.display(),
            forge("lib"),
            forge("api"),
            forge("web"),
        ),
    );
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": github}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    PathBuf::from(workspace["folder"].as_str().unwrap())
}

/// A commit of the user's in that worktree.
fn committed(user: &User, worktree: &Path, file: &str) {
    std::fs::write(worktree.join(file), "change\n").unwrap();
    user.git(worktree, &["add", file]);
    user.git(
        worktree,
        &["commit", "--quiet", "-m", &format!("add {file}")],
    );
}

/// A line run in one repository of the workspace.
fn line_in(mcp: &Mcp, repository: &str, line: &str) -> Answer {
    mcp.call(
        "commands_run",
        json!({"line": line, "repository": repository, "timeout": 30}),
    )
}

/// Adopts the template with the CLI, as the user does.
fn adopt(user: &User, template: &str) {
    let added = user.run(&["procedure", "add", "app", "--template", template]);
    assert!(added.status.success(), "{}", stderr(&added));
}

/// The first prompt of the run's root: its brief.
fn brief_of(user: &User, what: &str) -> String {
    eventually("the brief", || {
        user.received_requests("session/prompt")
            .iter()
            .any(|prompt| prompt.to_string().contains(what))
    });
    let prompts = user.received_requests("session/prompt");
    let prompt = prompts
        .iter()
        .find(|prompt| prompt.to_string().contains(what))
        .unwrap();
    prompt["params"]["prompt"][0]["text"]
        .as_str()
        .map_or_else(|| prompt.to_string(), str::to_owned)
}

/// The requests waiting for the user, oldest first.
fn waiting(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

fn approve(user: &User, request: &Value) {
    user.call("approval.approve", json!({"number": request["number"]}))
        .unwrap();
}

#[test]
fn a_template_is_adopted_with_one_command_into_the_project_s_file_where_the_user_edits_it() {
    let user = User::new();
    fixture(&user);
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("# kept by hand\n{text}")).unwrap();

    adopt(&user, "open-prs");
    let named = user.run(&["procedure", "add", "app", "finish", "--template", "close"]);
    assert!(named.status.success(), "{}", stderr(&named));
    assert!(
        stdout(&named).contains(
            "Added procedure finish to project app: it grants \
             workspace-remove (allow)"
        ),
        "{}",
        stdout(&named)
    );
    adopt(&user, "merge-stack");

    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# kept by hand\n"), "{text}");
    for table in [
        "[procedures.open-prs]",
        "[procedures.finish]",
        "[procedures.merge-stack]",
    ] {
        assert!(text.contains(table), "{text}");
    }
    // The instructions are written over several lines, as the user would edit them.
    assert!(
        text.contains("\"\"\"\nOpen the pull requests of this workspace"),
        "{text}"
    );
    let shown = user
        .call(
            "procedure.show",
            json!({"project": "app", "name": "open-prs"}),
        )
        .unwrap();
    assert_eq!(shown["grants"], json!({"push": "ask", "pr-create": "ask"}));
    assert!(
        shown["instructions"]
            .as_str()
            .unwrap()
            .contains("Report before anything else"),
        "{shown}"
    );
    let merge = user
        .call(
            "procedure.show",
            json!({"project": "app", "name": "merge-stack"}),
        )
        .unwrap();
    assert_eq!(
        merge["grants"],
        json!({"rebase": "allow", "push-force-with-lease": "allow",
            "pr-merge-squash": "allow", "pipeline-rerun": "allow"})
    );

    // Edited by hand, it is the project's own.
    let text = std::fs::read_to_string(&file)
        .unwrap()
        .replace("Report before anything else.", "Report first.");
    std::fs::write(&file, text).unwrap();
    let shown = user
        .call(
            "procedure.show",
            json!({"project": "app", "name": "open-prs"}),
        )
        .unwrap();
    assert!(
        shown["instructions"]
            .as_str()
            .unwrap()
            .contains("Report first."),
        "{shown}"
    );

    let unknown = user.run(&["procedure", "add", "app", "--template", "deploy"]);
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains(
            "there is no template deploy: the templates are open-prs, \
             merge-stack, close"
        ),
        "{}",
        stderr(&unknown)
    );
    let twice = user.run(&["procedure", "add", "app", "--template", "open-prs"]);
    assert!(!twice.status.success());
    assert!(
        stderr(&twice).contains("already has a procedure named open-prs"),
        "{}",
        stderr(&twice)
    );
    let mixed = user
        .call(
            "procedure.add",
            json!({"project": "app", "name": "other", "template": "close",
                "grants": {"push": "allow"}}),
        )
        .unwrap_err();
    assert!(
        mixed.1.contains("a template is added as it is"),
        "{mixed:?}"
    );
}

#[test]
fn open_the_pull_requests_reports_first_opens_nothing_before_the_confirmation_then_one_pull_request_per_repository_with_commits()
 {
    let user = User::with_fake_agent().with_fake_gh().with_fake_ssh(&[]);
    let workspace = stack(
        &user,
        true,
        "\n[pull_requests]\ntitle = \"type(TICKET): subject\"\nreviewers = [\"alice\", \"bob\"]\n",
    );
    user.gh_script(json!({"created": 31}));
    // The work of the workspace: commits in api only.
    committed(&user, &workspace.join("api"), "login.ts");
    adopt(&user, "open-prs");

    let (_, mcp) = run(&user, "open-prs");

    let brief = brief_of(&user, "You run the procedure open-prs");
    for said in [
        "in workspace login (branch app/login)",
        "- level 1: lib (base main, on origin)\n- level 2: api (base main, on origin), web (base \
         main, on origin)",
        "A pull request's title follows the project's format: type(TICKET): subject",
        "Flottille asks alice, bob to review each pull request it opens.",
        "- push: push the workspace's branch to its remote (git push) (each one held for the \
         user's approval)",
        "Report before anything else.",
    ] {
        assert!(brief.contains(said), "{said}\n---\n{brief}");
    }

    // The report: read only, and allowed at once.
    for repository in ["lib", "api", "web"] {
        for read in [
            "git status --porcelain",
            "git log --oneline HEAD --not --remotes",
            "git log --oneline origin/main..HEAD",
        ] {
            let answer = line_in(&mcp, repository, read);
            assert!(
                !answer.error && answer.text.contains("exited with code 0"),
                "{repository}: {read}: {}",
                answer.text
            );
        }
    }
    let ahead = line_in(&mcp, "api", "git log --oneline origin/main..HEAD");
    assert!(ahead.text.contains("add login.ts"), "{}", ahead.text);
    let read = mcp.call("forge_read", json!({}));
    assert!(!read.error, "{}", read.text);
    let asked = mcp.call(
        "ask_user",
        json!({"question": "api: 1 commit ahead of main, none pushed. lib, web: nothing. I will \
            push app/login in api and open its pull request \"feat(LOGIN-1): add the login\"."}),
    );
    assert!(!asked.error, "{}", asked.text);

    // Nothing leaves before the user's confirmation of each operation.
    let pushed = line_in(&mcp, "api", "git push origin app/login");
    assert!(
        pushed.text.contains("Waiting for the user's approval"),
        "{}",
        pushed.text
    );
    assert!(!remote_branches(&user, &user.path("remotes/api.git")).contains("app/login"));
    let create = json!({"action": "pr-create", "repository": "api",
        "title": "feat(LOGIN-1): add the login", "body": "Adds the login page."});
    let opened = mcp.call("forge_write", create);
    assert!(
        opened.text.contains("Waiting for the user's approval"),
        "{}",
        opened.text
    );
    assert_eq!(creates(&user), 0);
    let requests = waiting(&user);
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests.iter().all(|r| r["sensitive"] == true));

    for request in &requests {
        approve(&user, request);
    }
    eventually("the push", || {
        remote_branches(&user, &user.path("remotes/api.git")).contains("app/login")
    });
    eventually("the pull request", || creates(&user) == 1);
    let calls = gh_calls(&user);
    let created = calls.iter().find(|c| c.starts_with("pr create")).unwrap();
    assert_eq!(
        created,
        "pr create -R github.com/acme/api --head app/login --base main --title feat(LOGIN-1): add the login \
         --body Adds the login page. --reviewer alice --reviewer bob"
    );
    // The repositories with no commit of their own get nothing.
    for name in ["lib", "web"] {
        assert!(
            !remote_branches(&user, &user.path(&format!("remotes/{name}.git")))
                .contains("app/login")
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.starts_with(&format!("pr create -R github.com/acme/{name} ")))
        );
    }
}

/// A pull request of `app/login` into `main` in the repository, with these checks.
fn pull(number: u64, name: &str, checks: Value) -> Value {
    json!({"number": number, "title": format!("Change {name}"), "state": "OPEN",
        "isDraft": false, "baseRefName": "main", "isCrossRepository": false,
        "url": format!("https://github.com/acme/{name}/pull/{number}"),
        "reviewDecision": "APPROVED", "statusCheckRollup": checks})
}

fn check(conclusion: &str) -> Value {
    match conclusion {
        "RUNNING" => json!([{"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS"}]),
        done => json!([{"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
            "conclusion": done}]),
    }
}

fn pulls(lib: &str, api: &str, web: &str) -> Value {
    json!({"pulls": {
        "acme/lib": {"app/login": pull(1, "lib", check(lib))},
        "acme/api": {"app/login": pull(2, "api", check(api))},
        "acme/web": {"app/login": pull(3, "web", check(web))},
    }})
}

fn merged(user: &User) -> Vec<String> {
    gh_calls(user)
        .into_iter()
        .filter(|call| call.starts_with("pr merge"))
        .collect()
}

#[test]
fn merge_a_stack_refuses_while_a_check_is_failing_and_merges_the_levels_in_order() {
    let user = User::with_fake_agent().with_fake_gh().with_fake_ssh(&[]);
    stack(&user, true, "");
    adopt(&user, "merge-stack");
    user.gh_script(pulls("FAILURE", "SUCCESS", "FAILURE"));
    let (_, mcp) = run(&user, "merge-stack");
    let brief = brief_of(&user, "You run the procedure merge-stack");
    assert!(
        brief.contains("- level 1: lib (base main, on origin)\n- level 2: api"),
        "{brief}"
    );
    let merge = |repository: &str| {
        mcp.call(
            "forge_write",
            json!({"action": "pr-merge-squash", "repository": repository}),
        )
    };

    // The first level's check fails, then runs: nothing is merged.
    let answer = merge("lib");
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "the pull request #1 of app/login in acme/lib is not merged: its checks \
                 fail (build)"
        ),
        "{}",
        answer.text
    );
    user.gh_script(pulls("RUNNING", "SUCCESS", "FAILURE"));
    let answer = merge("lib");
    assert!(
        answer.error && answer.text.contains("its checks are still running (build)"),
        "{}",
        answer.text
    );
    assert_eq!(merged(&user), Vec::<String>::new());

    // Passing, the levels are merged in order, the second one's repositories together.
    user.gh_script(pulls("SUCCESS", "SUCCESS", "FAILURE"));
    let answer = merge("lib");
    assert!(!answer.error, "{}", answer.text);
    let answer = merge("api");
    assert!(!answer.error, "{}", answer.text);
    // web's check still fails: refused, until the user waives it and allows that very merge.
    let answer = merge("web");
    assert!(answer.error, "{}", answer.text);
    let waived = mcp.call(
        "forge_write",
        json!({"action": "pr-merge-squash", "repository": "web",
            "waive": "the build of web fails on a known flaky test"}),
    );
    assert!(
        waived.text.contains("Waiting for the user's approval"),
        "{}",
        waived.text
    );
    let requests = waiting(&user);
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0]
            .to_string()
            .contains("waived: the build of web fails"),
        "{requests:?}"
    );
    assert_eq!(merged(&user).len(), 2);
    approve(&user, &requests[0]);
    eventually("the waived merge", || merged(&user).len() == 3);

    assert_eq!(
        merged(&user),
        [
            "pr merge 1 -R github.com/acme/lib --squash",
            "pr merge 2 -R github.com/acme/api --squash",
            "pr merge 3 -R github.com/acme/web --squash",
        ]
    );
}

#[test]
fn close_stops_on_unpushed_work_and_removes_a_workspace_whose_branches_are_merged_or_empty() {
    let user = User::with_fake_agent();
    let workspace = stack(&user, false, "");
    let main = user.path("work/app");
    committed(&user, &workspace.join("api"), "login.ts");
    adopt(&user, "close");
    let (_, mcp) = run(&user, "close");
    let brief = brief_of(&user, "You run the procedure close");
    assert!(brief.contains("call `workspace_remove`"), "{brief}");

    // api's commit is on no remote: refused, and nothing is touched.
    let answer = mcp.call("workspace_remove", json!({}));
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("api has commits on no remote"),
        "{}",
        answer.text
    );
    assert!(workspace.join("api/login.ts").exists());
    user.call("workspace.show", json!({"project": "app", "name": "login"}))
        .unwrap();
    let uncommitted = workspace.join("web/notes.txt");
    std::fs::write(&uncommitted, "notes\n").unwrap();
    let answer = mcp.call("workspace_remove", json!({}));
    assert!(
        answer.error
            && answer
                .text
                .contains("web/notes.txt holds work that is not committed"),
        "{}",
        answer.text
    );
    std::fs::remove_file(&uncommitted).unwrap();

    // Pushed (and its pull request merged on the forge): removed, the empty branches deleted.
    user.git(
        &workspace.join("api"),
        &["push", "--quiet", "origin", "app/login"],
    );
    let answer = mcp.call("workspace_remove", json!({}));
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("Its branch app/login, empty, was deleted in lib, web."),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("is kept in api"), "{}", answer.text);
    assert!(!workspace.exists());
    assert_eq!(
        user.call("workspace.show", json!({"project": "app", "name": "login"}))
            .unwrap_err()
            .0,
        "not-found"
    );
    for (name, kept) in [("lib", false), ("api", true), ("web", false)] {
        let branches = user.git_output(&main.join(name), &["branch", "--list", "app/login"]);
        assert_eq!(branches.contains("app/login"), kept, "{name}: {branches}");
    }
    // The run's agent still reports.
    let reported = mcp.call("report", json!({"text": "Closed."}));
    assert!(!reported.error, "{}", reported.text);
}

#[test]
fn workspace_remove_is_refused_outside_a_run_that_grants_it_and_for_any_other_workspace() {
    let user = User::with_fake_agent();
    let workspace = stack(&user, false, "");
    procedure(&user, "push", json!({"push": "allow"}), false);
    procedure(&user, "tree", json!({"workspace-remove": "allow"}), true);

    let outside = plain_agent(&user);
    assert!(!outside.names().contains(&"workspace_remove".to_owned()));
    let answer = outside.call("workspace_remove", json!({}));
    assert!(
        answer.error && answer.text.contains("has no tool workspace_remove"),
        "{}",
        answer.text
    );
    let (_, push) = run(&user, "push");
    assert!(!push.names().contains(&"workspace_remove".to_owned()));
    let answer = push.call("workspace_remove", json!({}));
    assert!(
        answer.error && answer.text.contains("has no tool workspace_remove"),
        "{}",
        answer.text
    );

    // A child of a run that gives its grants, working in a workspace of its own: refused there.
    let (_, root) = run(&user, "tree");
    let opened = user.received_requests("session/new").len();
    let launched = root.call(
        "agent_launch",
        json!({"title": "elsewhere", "brief": "Remove your workspace.",
            "new_workspace": "other"}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);
    assert!(!child.names().contains(&"workspace_remove".to_owned()));
    let answer = child.call("workspace_remove", json!({}));
    assert!(
        answer.error && answer.text.contains("has no tool workspace_remove"),
        "{}",
        answer.text
    );
    assert!(workspace.exists());
    for name in ["login", "other"] {
        user.call("workspace.show", json!({"project": "app", "name": name}))
            .unwrap();
    }
}

// Bitbucket, through the user's `bkt` (`docs/forges.md`): the same writes, the same grants, the
// same checks.

/// The fixture's workspace, its remote `url` (created offline: nothing is fetched from it). The
/// project's folder.
fn on_bitbucket(user: &User, url: &str) -> PathBuf {
    let (folder, _) = prepared(
        user,
        |folder| user.git(folder, &["remote", "set-url", "origin", url]),
        true,
    );
    declare_proposed(user);
    folder
}

fn bkt_calls(user: &User) -> Vec<String> {
    user.bkt_received()
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

fn bkt_calls_starting(user: &User, start: &str) -> usize {
    bkt_calls(user)
        .iter()
        .filter(|call| call.starts_with(start))
        .count()
}

/// What `bkt pr list --json` prints of a Cloud pull request of `acme/app`.
fn cloud_pull(id: u64, source: &str, destination: &str, base: &str) -> Value {
    json!({"id": id, "title": "Login", "state": "OPEN",
        "source": {"branch": {"name": "app/login"}, "repository": {"full_name": source}},
        "destination": {"branch": {"name": base}, "repository": {"full_name": destination}},
        "links": {"html": {"href": format!("https://bitbucket.org/acme/app/pull-requests/{id}")}}})
}

#[test]
fn forge_write_creates_a_bitbucket_pull_request_with_the_fake_bkt_only_when_granted() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    user.bkt_script(json!({"created": 12,
        "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]}}));
    procedure(&user, "open", json!({"pr-create": "allow"}), false);
    procedure(&user, "merge", json!({"pr-merge-squash": "allow"}), false);
    let create = json!({"action": "pr-create", "repository": ".", "title": "Add the login page",
        "body": "What it does"});

    // Outside a run the tool is not listed; in a run that grants another action the gate refuses
    // this one: never asked of the forge.
    let outside = plain_agent(&user);
    assert!(!outside.names().contains(&"forge_write".to_owned()));
    let answer = outside.call("forge_write", create.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("has no tool forge_write"),
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
    assert!(user.bkt_received().is_empty());

    let (_, open) = run(&user, "open");
    let answer = open.call("forge_write", create);
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("https://bitbucket.org/acme/app/pull-requests/12"),
        "{}",
        answer.text
    );
    let calls = bkt_calls(&user);
    let created = calls.iter().find(|c| c.starts_with("pr create")).unwrap();
    assert_eq!(
        created,
        "pr create --context cloud --workspace acme --repo app --source app/login --target main \
         --title Add the login page --description What it does"
    );
    assert_eq!(bkt_calls_starting(&user, "pr create"), 1);
    // The daemon's own sign-in: of it, the contexts are all that was read.
    assert!(!calls.iter().any(|call| call.contains("token")));
    // No `gh` was asked: the fake `gh` is not even on this user's PATH.
}

#[test]
fn a_bitbucket_pull_request_is_never_a_draft_and_needs_its_title() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    procedure(&user, "open", json!({"pr-create": "allow"}), false);
    let (_, open) = run(&user, "open");

    let draft = open.call(
        "forge_write",
        json!({"action": "pr-create", "repository": ".", "title": "Login", "draft": true}),
    );
    let untitled = open.call(
        "forge_write",
        json!({"action": "pr-create", "repository": "."}),
    );

    assert!(draft.error, "{}", draft.text);
    assert!(draft.text.contains("draft"), "{}", draft.text);
    assert!(untitled.error, "{}", untitled.text);
    assert!(untitled.text.contains("needs a title"), "{}", untitled.text);
    assert_eq!(bkt_calls_starting(&user, "pr create"), 0);
}

#[test]
fn forge_write_names_a_data_center_repository_by_its_project() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "ssh://git@bitbucket.example.com:7999/APP/app.git");
    user.bkt_script(json!({"contexts": [
            {"name": "cloud", "host": "bitbucket.org"},
            {"name": "work", "host": "bitbucket.example.com:7990"}],
        "created": 4}));
    procedure(&user, "open", json!({"pr-create": "allow"}), false);
    let (_, open) = run(&user, "open");

    let answer = open.call(
        "forge_write",
        json!({"action": "pr-create", "repository": ".", "title": "Login"}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(
        bkt_calls(&user).contains(
            &"pr create --context work --project APP --repo app --source app/login --target main \
              --title Login --description "
                .to_owned()
        ),
        "{:?}",
        bkt_calls(&user)
    );
    // A declared host the remote does not lead to is refused, and nothing is written.
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        text.replace("bitbucket.example.com", "other.example.com"),
    )
    .unwrap();
    let answer = open.call(
        "forge_write",
        json!({"action": "pr-create", "repository": ".", "title": "Login"}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.starts_with(
            "refused: . is declared on Bitbucket Data Center APP/app on other.example.com, and \
             its remote origin: it leads to bitbucket.example.com/APP/app"
        ),
        "{}",
        answer.text
    );
    assert_eq!(bkt_calls_starting(&user, "pr create"), 1);
}

#[test]
fn a_host_bkt_has_no_context_for_is_a_failure_that_says_what_to_do() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    user.bkt_script(json!({"contexts": []}));
    procedure(&user, "open", json!({"pr-create": "allow"}), false);
    let (_, open) = run(&user, "open");

    let answer = open.call(
        "forge_write",
        json!({"action": "pr-create", "repository": ".", "title": "Login"}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("bkt has no context for bitbucket.org"),
        "{}",
        answer.text
    );
    assert_eq!(bkt_calls(&user), ["context list --json"]);
}

#[test]
fn bitbucket_pr_merge_squash_merges_only_the_workspace_s_own_pull_request_into_its_base() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    procedure(&user, "merge", json!({"pr-merge-squash": "allow"}), false);
    let (_, mcp) = run(&user, "merge");
    let merge = json!({"action": "pr-merge-squash", "repository": "."});
    // Its own page names no reviewer, and it has no check: nothing blocks the merge.
    let pulls = |pulls: Value| {
        user.bkt_script(json!({"pulls": {"acme/app": pulls},
            "api": {"/repositories/acme/app/pullrequests/12": cloud_page(&[])}}))
    };

    // None open.
    pulls(json!([]));
    let answer = mcp.call("forge_write", merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("no open pull request"),
        "{}",
        answer.text
    );
    // A fork's pull request of a branch of the same name.
    pulls(json!([cloud_pull(12, "someone/app", "acme/app", "main")]));
    let answer = mcp.call("forge_write", merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("from a fork"), "{}", answer.text);
    // One into another base.
    pulls(json!([cloud_pull(12, "acme/app", "acme/app", "release")]));
    let answer = mcp.call("forge_write", merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("into release, not main"),
        "{}",
        answer.text
    );
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 0);

    pulls(json!([
        cloud_pull(12, "acme/app", "acme/app", "main"),
        cloud_pull(11, "someone/app", "acme/app", "main")
    ]));
    let answer = mcp.call("forge_write", merge);
    assert!(!answer.error, "{}", answer.text);
    let calls = bkt_calls(&user);
    assert!(
        calls.contains(
            &"pr list --context cloud --workspace acme --repo app --state OPEN --limit 100 --json"
                .to_owned()
        ),
        "{calls:?}"
    );
    // By squash, and the source branch is left where it is, as `gh pr merge --squash` leaves it.
    assert!(
        calls.contains(
            &"pr merge 12 --context cloud --workspace acme --repo app --strategy squash \
              --close-source=false"
                .to_owned()
        ),
        "{calls:?}"
    );
}

#[test]
fn bitbucket_pipeline_rerun_starts_the_branch_s_pipeline_again_only_when_its_last_one_failed() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    procedure(&user, "rerun", json!({"pipeline-rerun": "allow"}), false);
    let (_, mcp) = run(&user, "rerun");
    let rerun = json!({"action": "pipeline-rerun", "repository": "."});
    let pipeline = |number: u64, branch: &str, result: &str| {
        json!({"uuid": format!("{{{number}}}"), "build_number": number,
            "state": {"name": "COMPLETED", "result": {"name": result}},
            "target": {"ref": {"name": branch}}})
    };
    let pipelines =
        |pipelines: Value| user.bkt_script(json!({"pipelines": {"acme/app": pipelines}}));

    // None for the branch.
    pipelines(json!([pipeline(9, "main", "FAILED")]));
    let answer = mcp.call("forge_write", rerun.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("no pipeline of app/login"),
        "{}",
        answer.text
    );
    // Its last one went through, after an older one that failed.
    pipelines(json!([
        pipeline(8, "app/login", "SUCCESSFUL"),
        pipeline(7, "app/login", "FAILED")
    ]));
    let answer = mcp.call("forge_write", rerun.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("nothing to re-run"), "{}", answer.text);
    assert_eq!(bkt_calls_starting(&user, "pipeline run"), 0);

    pipelines(json!([
        pipeline(9, "main", "SUCCESSFUL"),
        pipeline(8, "app/login", "FAILED"),
        pipeline(7, "app/login", "SUCCESSFUL")
    ]));
    let answer = mcp.call("forge_write", rerun);
    assert!(!answer.error, "{}", answer.text);
    let calls = bkt_calls(&user);
    assert!(
        calls.contains(
            &"pipeline list --context cloud --workspace acme --repo app --limit 50 --json"
                .to_owned()
        ),
        "{calls:?}"
    );
    assert!(
        calls.contains(
            &"pipeline run --context cloud --workspace acme --repo app --ref app/login".to_owned()
        ),
        "{calls:?}"
    );
    assert_eq!(bkt_calls_starting(&user, "pipeline run"), 1);
}

#[test]
fn a_data_center_repository_has_no_pipeline_bkt_can_run() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "ssh://git@bitbucket.example.com:7999/APP/app.git");
    user.bkt_script(json!({"contexts": [{"name": "work", "host": "bitbucket.example.com:7990"}]}));
    procedure(&user, "rerun", json!({"pipeline-rerun": "allow"}), false);
    let (_, mcp) = run(&user, "rerun");

    let answer = mcp.call(
        "forge_write",
        json!({"action": "pipeline-rerun", "repository": "."}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("Data Center has no pipelines"),
        "{}",
        answer.text
    );
    assert_eq!(bkt_calls_starting(&user, "pipeline"), 0);
}

#[test]
fn a_bitbucket_write_is_refused_when_the_remote_leads_elsewhere_than_when_the_workspace_was_made() {
    let user = User::with_fake_agent().with_fake_bkt();
    let folder = on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    procedure(&user, "open", json!({"pr-create": "allow"}), false);
    let (_, open) = run(&user, "open");
    user.git(
        &folder,
        &[
            "remote",
            "set-url",
            "origin",
            "git@bitbucket.org:evil/app.git",
        ],
    );

    let answer = open.call(
        "forge_write",
        json!({"action": "pr-create", "repository": ".", "title": "Login"}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("no longer leads where it led"),
        "{}",
        answer.text
    );
    assert!(user.bkt_received().is_empty());
}

/// `[pull_requests] reviewers` in the project's file.
fn name_the_reviewers(user: &User) {
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[pull_requests]\nreviewers = [\"alice\", \"bob\"]\n");
    std::fs::write(file, text).unwrap();
}

#[test]
fn bitbucket_pr_create_asks_the_project_s_reviewers() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    name_the_reviewers(&user);
    user.bkt_script(json!({"created": 12}));
    procedure(&user, "open", json!({"pr-create": "allow"}), false);
    let (_, open) = run(&user, "open");

    let answer = open.call(
        "forge_write",
        json!({"action": "pr-create", "repository": ".", "title": "Login", "body": "Adds it"}),
    );

    assert!(!answer.error, "{}", answer.text);
    let calls = bkt_calls(&user);
    let created = calls.iter().find(|c| c.starts_with("pr create")).unwrap();
    assert_eq!(
        created,
        "pr create --context cloud --workspace acme --repo app --source app/login --target main \
         --title Login --description Adds it --reviewer alice --reviewer bob"
    );
}

/// The Cloud pull request #12's own page, with its reviewers in `states`.
fn cloud_page(states: &[&str]) -> Value {
    let participants: Vec<Value> = states
        .iter()
        .enumerate()
        .map(|(at, state)| {
            json!({"role": "REVIEWER", "approved": *state == "approved", "state": state,
                "user": {"display_name": format!("Reviewer {at}")}})
        })
        .collect();
    json!({"id": 12, "participants": participants})
}

#[test]
fn bitbucket_pr_merge_squash_refuses_while_a_check_fails_or_runs_or_a_reviewer_asks_for_changes_until_the_user_waives_it()
 {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    procedure(&user, "merge", json!({"pr-merge-squash": "allow"}), false);
    let (_, mcp) = run(&user, "merge");
    let merge = json!({"action": "pr-merge-squash", "repository": "."});
    let page = "/repositories/acme/app/pullrequests/12";
    let script = |checks: Value, reviewers: &[&str]| {
        user.bkt_script(json!({
            "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]},
            "checks": {"acme/app": {"12": checks}},
            "api": {page: cloud_page(reviewers)}}));
    };
    let refused = |what: &str| {
        let answer = mcp.call("forge_write", merge.clone());
        assert!(answer.error, "{}", answer.text);
        assert!(answer.text.contains(what), "{what}: {}", answer.text);
    };

    script(json!([{"state": "FAILED", "name": "build"}]), &[]);
    refused("its checks fail (build)");
    script(json!([{"state": "INPROGRESS", "name": "build"}]), &[]);
    refused("its checks are still running (build)");
    script(
        json!([{"state": "SUCCESSFUL", "name": "build"}]),
        &["approved", "changes_requested"],
    );
    refused("a review asks for changes");
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 0);

    // Waived: held for the user, and merged once they allow that very merge.
    let waived = mcp.call(
        "forge_write",
        json!({"action": "pr-merge-squash", "repository": ".",
            "waive": "the reviewer's request is already answered"}),
    );
    assert!(
        waived.text.contains("Waiting for the user's approval"),
        "{}",
        waived.text
    );
    let requests = waiting(&user);
    assert_eq!(requests.len(), 1, "{requests:?}");
    approve(&user, &requests[0]);
    eventually("the waived merge", || {
        bkt_calls_starting(&user, "pr merge") == 1
    });

    // Passing checks and approving reviewers: merged without a word from the user.
    script(
        json!([{"state": "SUCCESSFUL", "name": "build"}]),
        &["approved"],
    );
    let answer = mcp.call("forge_write", merge);
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 2);
}

#[test]
fn a_data_center_pull_request_a_reviewer_marked_needs_work_is_not_merged() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "ssh://git@bitbucket.example.com:7999/APP/app.git");
    let pull = |status: &str| {
        json!({"id": 5, "title": "Login", "state": "OPEN",
            "fromRef": {"displayId": "app/login",
                "repository": {"slug": "app", "project": {"key": "APP"}}},
            "toRef": {"displayId": "main",
                "repository": {"slug": "app", "project": {"key": "APP"}}},
            "participants": [{"role": "REVIEWER", "status": status, "approved": false,
                "user": {"displayName": "Alice"}}],
            "links": {"self": [{"href": "https://bitbucket.example.com/projects/APP/repos/app/pull-requests/5"}]}})
    };
    let script = |status: &str| {
        user.bkt_script(json!({
            "contexts": [{"name": "work", "host": "bitbucket.example.com:7990"}],
            "pulls": {"APP/app": [pull(status)]},
            "checks": {"APP/app": {"5": [{"state": "SUCCESSFUL", "name": "build"}]}}}));
    };
    procedure(&user, "merge", json!({"pr-merge-squash": "allow"}), false);
    let (_, mcp) = run(&user, "merge");
    let merge = json!({"action": "pr-merge-squash", "repository": "."});

    script("NEEDS_WORK");
    let answer = mcp.call("forge_write", merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("a review asks for changes"),
        "{}",
        answer.text
    );
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 0);

    script("APPROVED");
    let answer = mcp.call("forge_write", merge);
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 1);
}

/// A root agent started in `login` outside any procedure: its id and MCP session.
fn plain_agent_with_id(user: &User) -> (String, Mcp) {
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

/// Starts `sleep 300` and answers once it is going: the command run's id.
fn sleeping(mcp: &Mcp) -> String {
    let answer = mcp.call("commands_run", json!({"line": "sleep 300", "timeout": 1}));
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("run ")
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .to_owned()
}

#[test]
fn workspace_remove_is_refused_while_an_agent_outside_the_run_or_a_command_not_the_run_s_works_in_the_workspace()
 {
    let user = User::with_fake_agent();
    let workspace = stack(&user, false, "");
    adopt(&user, "close");
    let (outsider, outside) = plain_agent_with_id(&user);
    let theirs = sleeping(&outside);
    let (_, mcp) = run(&user, "close");
    // The run's own command is no obstacle.
    let ours = sleeping(&mcp);

    let answer = mcp.call("workspace_remove", json!({}));
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(&format!(
            "the agent {outsider}, outside this run, works in it"
        )),
        "{}",
        answer.text
    );
    assert!(
        answer
            .text
            .contains(&format!("the command run {theirs} (sleep 300) runs in it")),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains(&ours), "{}", answer.text);
    assert!(workspace.exists());

    let stopped = outside.call("commands_status", json!({"run": theirs, "action": "stop"}));
    assert!(!stopped.error, "{}", stopped.text);
    let answer = mcp.call("workspace_remove", json!({}));
    assert!(answer.error, "{}", answer.text);
    assert!(!answer.text.contains(&theirs), "{}", answer.text);
    assert!(workspace.exists());

    user.call("agent.stop", json!({"agent": outsider})).unwrap();
    let answer = mcp.call("workspace_remove", json!({}));
    assert!(!answer.error, "{}", answer.text);
    assert!(!workspace.exists());
    let stopped = mcp.call("commands_status", json!({"run": ours, "action": "stop"}));
    assert!(!stopped.error, "{}", stopped.text);
}

/// The MCP session of the agent a run's root launched: the last session opened.
fn child_of(user: &User, root: &Mcp, launch: Value) -> Mcp {
    let opened = user.received_requests("session/new").len();
    let launched = root.call("agent_launch", launch);
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

#[test]
fn forge_write_and_workspace_remove_are_listed_only_to_an_agent_whose_run_grants_them() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    procedure(&user, "push", json!({"push": "allow"}), false);
    procedure(&user, "open", json!({"pr-create": "ask"}), false);
    procedure(&user, "rerun", json!({"pipeline-rerun": "allow"}), false);
    procedure(&user, "tidy", json!({"workspace-remove": "allow"}), false);
    procedure(
        &user,
        "ship",
        json!({"pr-merge-squash": "allow", "workspace-remove": "ask"}),
        false,
    );
    let listed = |mcp: &Mcp| -> (bool, bool) {
        let names = mcp.names();
        (
            names.contains(&"forge_write".to_owned()),
            names.contains(&"workspace_remove".to_owned()),
        )
    };

    assert_eq!(listed(&plain_agent(&user)), (false, false), "outside a run");
    for (name, tools) in [
        ("push", (false, false)),
        ("open", (true, false)),
        ("rerun", (true, false)),
        ("tidy", (false, true)),
        ("ship", (true, true)),
    ] {
        let (_, mcp) = run(&user, name);
        assert_eq!(listed(&mcp), tools, "the run {name}");
        // The tools of the others are still there, in the same order.
        assert!(mcp.names().starts_with(&["fs_read".to_owned()]), "{name}");
    }
}

#[test]
fn a_child_of_a_run_lists_the_tools_its_run_gives_it_only_when_the_run_gives_its_grants() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    procedure(
        &user,
        "alone",
        json!({"pr-create": "allow", "workspace-remove": "allow"}),
        false,
    );
    procedure(
        &user,
        "together",
        json!({"pr-create": "allow", "workspace-remove": "allow"}),
        true,
    );
    let own = json!({"title": "same place", "brief": "Open the pull request."});

    let (_, alone) = run(&user, "alone");
    let child = child_of(&user, &alone, own.clone());
    assert!(!child.names().contains(&"forge_write".to_owned()));
    assert!(!child.names().contains(&"workspace_remove".to_owned()));

    let (_, together) = run(&user, "together");
    let child = child_of(&user, &together, own);
    assert!(child.names().contains(&"forge_write".to_owned()));
    assert!(child.names().contains(&"workspace_remove".to_owned()));
    // What the list shows is what the gate lets through: the same run decides both.
    let answer = child.call(
        "forge_write",
        json!({"action": "pipeline-rerun", "repository": "."}),
    );
    assert!(
        answer.error && answer.text.contains("does not grant pipeline-rerun"),
        "{}",
        answer.text
    );
}
