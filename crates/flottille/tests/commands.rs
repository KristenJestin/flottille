//! The scenarios of commands (`docs/commands.md`): named instructions of a project, sent to a
//! session as `/name`. A command grants nothing: what its agents write goes through the gate's
//! classes, held here for the user, who allows it. The agents are the fake agent's sessions, driven through
//! their MCP server as their client would; the forge is the fake `gh`; the remotes are throwaway
//! bare repositories.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
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

/// Adds the command `name`.
fn command(user: &User, name: &str) {
    user.call(
        "command.add",
        json!({"project": "app", "name": name, "description": "Ship the workspace",
            "instructions": "Push the branch, then open its pull request."}),
    )
    .unwrap();
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
    let store = || Store::open(&user.path("state/flottille/flottille.db")).unwrap();
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

/// `workspace` `remove`, allowed by the user.
fn remove(user: &User, mcp: &Mcp, mut args: Value) -> Answer {
    args["action"] = "remove".into();
    allowed(user, mcp.call("workspace", args))
}

/// A line, allowed by the user.
fn allowed_line(user: &User, mcp: &Mcp, text: &str) -> Answer {
    allowed(user, line(mcp, text))
}

/// Starts a session in `login` with the command as its first message, as the composer sends
/// `/name`: its root agent's id and MCP session.
fn run(user: &User, name: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": format!("/{name}")}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        started["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

/// A root agent started in `login` with no command.
fn plain_agent(user: &User) -> Mcp {
    plain_agent_with_id(user).1
}

fn line(mcp: &Mcp, line: &str) -> Answer {
    mcp.call("run", json!({"line": line, "timeout": 30}))
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
fn a_command_is_added_listed_shown_edited_and_removed() {
    let user = User::new();
    fixture(&user);
    // What the user wrote in the file stays around what the daemon edits.
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("# kept by hand\n{text}")).unwrap();

    let added = user.run(&[
        "command",
        "add",
        "app",
        "open-prs",
        "--description",
        "Open the pull requests",
        "--instructions",
        "Push, then open one pull request per repository.",
    ]);
    assert!(added.status.success(), "{}", stderr(&added));

    let listed = stdout(&user.run(&["command", "list", "app"]));
    assert!(
        listed.contains("/open-prs  Open the pull requests"),
        "{listed}"
    );
    let shown = user.run(&["command", "show", "app", "open-prs", "--json"]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert!(shown.get("grants").is_none(), "{shown}");
    assert_eq!(
        shown["instructions"],
        "Push, then open one pull request per repository."
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# kept by hand\n"), "{text}");
    assert!(text.contains("[commands.open-prs]"), "{text}");

    // A grant is no option any more.
    let refused = user.run(&[
        "command",
        "edit",
        "app",
        "open-prs",
        "--grant",
        "push=allow",
    ]);
    assert!(!refused.status.success());
    let edited = user.run(&[
        "command",
        "edit",
        "app",
        "open-prs",
        "--description",
        "Open them all",
    ]);
    assert!(edited.status.success(), "{}", stderr(&edited));
    let shown = user
        .call(
            "command.show",
            json!({"project": "app", "name": "open-prs"}),
        )
        .unwrap();
    assert_eq!(shown["description"], "Open them all");
    assert_eq!(
        user.call(
            "command.add",
            json!({"project": "app", "name": "open-prs", "description": "d", "instructions": "i"})
        )
        .unwrap_err()
        .0,
        "refused"
    );

    let removed = user.run(&["command", "remove", "app", "open-prs"]);
    assert!(removed.status.success(), "{}", stderr(&removed));
    let listed = user
        .call("command.list", json!({"project": "app"}))
        .unwrap();
    assert_eq!(listed["commands"], json!([]));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains("open-prs"), "{text}");
    assert!(text.starts_with("# kept by hand\n"), "{text}");
}

#[test]
fn slash_name_sends_the_instructions_with_the_trailing_text() {
    let user = User::with_fake_agent();
    fixture(&user);
    command(&user, "ship");

    let (agent, _) = run(&user, "ship and the tests too");
    let brief = brief_of(&user, "The command ship of project app");
    assert!(
        brief.contains("\nThe command ship of project app: Ship the workspace\n"),
        "{brief}"
    );
    assert!(
        brief.contains("in workspace login (branch app/login)"),
        "{brief}"
    );
    assert!(
        brief.ends_with(
            "\n---\n\nPush the branch, then open its pull request.\n\nand the tests too"
        ),
        "{brief}"
    );
    // A session like any other, in the tree, in Auto.
    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    assert_eq!(shown["agent"]["workspace"], "login");
    assert_eq!(shown["agent"]["gate_mode"], "auto");

    // To a session already going, and a word that names no command is sent as it is.
    eventually("the first turn", || {
        user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"] == "idle"
    });
    user.call(
        "agent.send",
        json!({"agent": agent, "message": "/deploy now"}),
    )
    .unwrap();
    eventually("the plain text", || {
        user.received_requests("session/prompt").iter().any(|p| {
            p["params"]["prompt"][0]["text"]
                .as_str()
                .is_some_and(|text| text.ends_with("\n/deploy now"))
        })
    });
}

#[test]
fn a_push_is_to_judge_for_any_agent_and_goes_through_once_the_user_allows_it() {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    command(&user, "ship");

    for mcp in [plain_agent(&user), run(&user, "ship").1] {
        let answer = line(&mcp, "git push origin app/login");
        assert!(
            answer.text.contains("Waiting for the user's approval"),
            "{}",
            answer.text
        );
        let requests = waiting(&user);
        assert_eq!(requests.last().unwrap()["class"], "judge");
        assert!(!remote_branches(&user, &remote).contains("app/login"));
        user.call(
            "approval.deny",
            json!({"number": requests.last().unwrap()["number"]}),
        )
        .unwrap();
    }

    let answer = allowed_line(&user, &plain_agent(&user), "git push origin app/login");
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("exited with code 0"),
        "{}",
        answer.text
    );
    assert!(remote_branches(&user, &remote).contains("app/login"));
}

#[test]
fn a_force_push_or_a_deletion_on_the_remote_is_base_and_a_plain_push_is_to_judge() {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    let mcp = plain_agent(&user);

    for base in [
        "git push --force origin app/login",
        "git push -f origin app/login",
        "git push origin +app/login",
        "git push --delete origin main",
        "git push --force-with-lease origin app/login",
    ] {
        let answer = line(&mcp, base);
        assert!(
            answer.text.contains("Waiting for the user's approval"),
            "{base}: {}",
            answer.text
        );
        assert_eq!(waiting(&user).last().unwrap()["class"], "base", "{base}");
    }
    for judged in ["git reset --hard HEAD", "git stash", "git push origin main"] {
        line(&mcp, judged);
        assert_eq!(waiting(&user).last().unwrap()["class"], "judge", "{judged}");
    }
    assert!(!remote_branches(&user, &remote).contains("app/login"));
    assert!(remote_branches(&user, &remote).contains("main"));
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
fn forge_write_creates_a_pull_request_with_the_fake_gh_once_allowed() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(json!({"created": 12}));
    let create = json!({"action": "create", "repository": ".", "title": "Add the login page",
        "body": "What it does"});

    // Listed to an agent of a workspace, held: never asked of the forge before the user allows it.
    let mcp = plain_agent(&user);
    assert!(mcp.names().contains(&"forge".to_owned()));
    let answer = mcp.call("forge", create.clone());
    assert!(
        answer.text.contains("Waiting for the user's approval"),
        "{}",
        answer.text
    );
    assert_eq!(creates(&user), 0);

    let answer = write(&user, &mcp, create);
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
fn an_allowed_commit_push_or_rebase_runs_none_of_the_hooks_the_agent_can_write() {
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
    let mcp = plain_agent(&user);

    let written = mcp.call(
        "fs_write",
        json!({"path": "change.txt", "content": "change\n"}),
    );
    assert!(!written.error, "{}", written.text);
    let answer = line(&mcp, "git add change.txt");
    assert!(
        answer.text.contains("exited with code 0"),
        "{}",
        answer.text
    );
    for done in [
        "git commit -m change",
        "git rebase main",
        "git push origin app/login",
    ] {
        let answer = allowed_line(&user, &mcp, done);
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
fn pointing_a_remote_elsewhere_is_to_judge_and_a_push_to_a_remote_changed_since_the_workspace_was_created_is_refused()
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
    let mcp = plain_agent(&user);

    let evil_url = evil.to_str().unwrap();
    for held in [
        format!("git remote set-url origin {evil_url}"),
        format!("git remote add evil {evil_url}"),
        format!("git config remote.origin.pushurl {evil_url}"),
    ] {
        let answer = line(&mcp, &held);
        assert!(
            answer.text.contains("Waiting for the user's approval"),
            "{held}: {}",
            answer.text
        );
    }
    // Moved anyway, by a way the gate does not see: the push the user allows is refused before
    // git runs, since neither the user nor the judge reads where the remote leads now.
    user.git(
        &folder,
        &["remote", "set-url", "--push", "origin", evil_url],
    );
    let answer = allowed_line(&user, &mcp, "git push origin app/login");

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
    command(&user, "open");
    let (_, open) = run(&user, "open");
    user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:evil/app.git"],
    );

    let answer = write(
        &user,
        &open,
        json!({"action": "create", "repository": ".", "title": "Add the login page"}),
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
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    let merge = json!({"action": "merge", "repository": "."});
    let merges = |user: &User| {
        gh_calls(user)
            .iter()
            .filter(|call| call.starts_with("pr merge"))
            .count()
    };
    let pull = |cross: bool, base: &str| {
        json!({"pulls": {"acme/app": {"app/login": {"number": 12, "title": "Login",
            "state": "OPEN", "isDraft": false, "baseRefName": base, "isCrossRepository": cross,
            "url": "https://github.com/acme/app/pull/12", "statusCheckRollup": passing()}}}})
    };

    // A fork's pull request of a branch of the same name.
    user.gh_script(pull(true, "main"));
    let answer = write(&user, &mcp, merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("from a fork"), "{}", answer.text);
    // One into another base.
    user.gh_script(pull(false, "release"));
    let answer = write(&user, &mcp, merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("into release, not main"),
        "{}",
        answer.text
    );
    assert_eq!(merges(&user), 0);

    user.gh_script(pull(false, "main"));
    let answer = write(&user, &mcp, merge);
    assert!(!answer.error, "{}", answer.text);
    assert!(
        gh_calls(&user).contains(&"pr merge 12 -R github.com/acme/app --squash".to_owned()),
        "{:?}",
        gh_calls(&user)
    );
}

/// The gate refuses `flottille`, and the daemon the clients its agents start (#93): without
/// them, an agent reaches the daemon's socket through `run`. The agent names the socket's
/// folder itself, which a run's environment no longer says.
#[test]
fn an_agent_s_run_of_flottille_command_add_adds_nothing() {
    let user = User::with_fake_agent();
    let (_, remote) = fixture(&user);
    command(&user, "plain");
    let (_, mcp) = run(&user, "plain");
    let before = agents(&user);
    let flottille = format!(
        "env XDG_RUNTIME_DIR={} {}",
        user.path("runtime").display(),
        env!("CARGO_BIN_EXE_flottille")
    );

    let done = format!("{flottille} command add app ship --description Ship --instructions Push");
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

    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(agents(&user), before);
    assert!(
        user.call("command.show", json!({"project": "app", "name": "ship"}))
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
        "run",
        json!({"line": line, "repository": repository, "timeout": 30}),
    )
}

/// Adopts the template with the CLI, as the user does.
fn adopt(user: &User, template: &str) {
    let added = user.run(&["command", "add", "app", "--template", template]);
    assert!(added.status.success(), "{}", stderr(&added));
}

/// The first prompt that holds `what`: a command's brief.
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
    let named = user.run(&["command", "add", "app", "finish", "--template", "close"]);
    assert!(named.status.success(), "{}", stderr(&named));
    assert!(
        stdout(&named).contains("Added command finish to project app"),
        "{}",
        stdout(&named)
    );
    adopt(&user, "merge-stack");

    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# kept by hand\n"), "{text}");
    for table in [
        "[commands.open-prs]",
        "[commands.finish]",
        "[commands.merge-stack]",
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
            "command.show",
            json!({"project": "app", "name": "open-prs"}),
        )
        .unwrap();
    assert!(shown.get("grants").is_none(), "{shown}");
    assert!(
        shown["instructions"]
            .as_str()
            .unwrap()
            .contains("Report before anything else"),
        "{shown}"
    );
    // Edited by hand, it is the project's own.
    let text = std::fs::read_to_string(&file)
        .unwrap()
        .replace("Report before anything else.", "Report first.");
    std::fs::write(&file, text).unwrap();
    let shown = user
        .call(
            "command.show",
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

    let unknown = user.run(&["command", "add", "app", "--template", "deploy"]);
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains(
            "there is no template deploy: the templates are open-prs, \
             merge-stack, close"
        ),
        "{}",
        stderr(&unknown)
    );
    let twice = user.run(&["command", "add", "app", "--template", "open-prs"]);
    assert!(!twice.status.success());
    assert!(
        stderr(&twice).contains("already has a command named open-prs"),
        "{}",
        stderr(&twice)
    );
    let mixed = user
        .call(
            "command.add",
            json!({"project": "app", "name": "other", "template": "close",
                "description": "Close it"}),
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

    let brief = brief_of(&user, "The command open-prs of project app");
    for said in [
        "in workspace login (branch app/login)",
        "- level 1: lib (base main, on origin)\n- level 2: api (base main, on origin), web (base \
         main, on origin)",
        "A pull request's title follows the project's format: type(TICKET): subject",
        "Flottille asks alice, bob to review each pull request it opens.",
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
    let read = mcp.call("forge", json!({"action": "show"}));
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
    let create = json!({"action": "create", "repository": "api",
        "title": "feat(LOGIN-1): add the login", "body": "Adds the login page."});
    let opened = mcp.call("forge", create);
    assert!(
        opened.text.contains("Waiting for the user's approval"),
        "{}",
        opened.text
    );
    assert_eq!(creates(&user), 0);
    let requests = waiting(&user);
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        requests.iter().all(|r| r["class"] == "judge"),
        "{requests:?}"
    );

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
    let brief = brief_of(&user, "The command merge-stack of project app");
    assert!(
        brief.contains("- level 1: lib (base main, on origin)\n- level 2: api"),
        "{brief}"
    );
    let merge = |repository: &str| {
        write(
            &user,
            &mcp,
            json!({"action": "merge", "repository": repository}),
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
        "forge",
        json!({"action": "merge", "repository": "web",
            "waive": "the build of web fails on a known flaky test"}),
    );
    assert!(
        waived.text.contains("Waiting for the user's approval"),
        "{}",
        waived.text
    );
    let requests = waiting(&user);
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0]["class"], "base", "a waived merge always asks");
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
    let brief = brief_of(&user, "The command close of project app");
    assert!(brief.contains("`workspace` `remove`"), "{brief}");

    // api's commit is on no remote: refused, and nothing is touched.
    let answer = remove(&user, &mcp, json!({}));
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
    let answer = remove(&user, &mcp, json!({}));
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
    let answer = remove(&user, &mcp, json!({}));
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

// Bitbucket, through the user's `bkt` (`docs/forges.md`): the same writes, the same classes, the
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
fn forge_write_creates_a_bitbucket_pull_request_with_the_fake_bkt_once_allowed() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    user.bkt_script(json!({"created": 12,
        "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]}}));
    let create = json!({"action": "create", "repository": ".", "title": "Add the login page",
        "body": "What it does"});

    // Held: never asked of the forge before the user allows it.
    let mcp = plain_agent(&user);
    let answer = mcp.call("forge", create.clone());
    assert!(
        answer.text.contains("Waiting for the user's approval"),
        "{}",
        answer.text
    );
    assert!(user.bkt_received().is_empty());

    let answer = write(&user, &mcp, create);
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
    command(&user, "open");
    let (_, open) = run(&user, "open");

    let draft = write(
        &user,
        &open,
        json!({"action": "create", "repository": ".", "title": "Login", "draft": true}),
    );
    let untitled = write(&user, &open, json!({"action": "create", "repository": "."}));

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
    command(&user, "open");
    let (_, open) = run(&user, "open");

    let answer = write(
        &user,
        &open,
        json!({"action": "create", "repository": ".", "title": "Login"}),
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
    let answer = write(
        &user,
        &open,
        json!({"action": "create", "repository": ".", "title": "Login"}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
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
    command(&user, "open");
    let (_, open) = run(&user, "open");

    let answer = write(
        &user,
        &open,
        json!({"action": "create", "repository": ".", "title": "Login"}),
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
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    let merge = json!({"action": "merge", "repository": "."});
    // Its own page names no reviewer, and its check passes: nothing blocks the merge.
    let pulls = |pulls: Value| {
        user.bkt_script(json!({"pulls": {"acme/app": pulls},
            "checks": {"acme/app": {"12": [{"state": "SUCCESSFUL", "name": "build"}]}},
            "api": {"/repositories/acme/app/pullrequests/12": cloud_page(&[])}}))
    };

    // None open.
    pulls(json!([]));
    let answer = write(&user, &mcp, merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("no open pull request"),
        "{}",
        answer.text
    );
    // A fork's pull request of a branch of the same name.
    pulls(json!([cloud_pull(12, "someone/app", "acme/app", "main")]));
    let answer = write(&user, &mcp, merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("from a fork"), "{}", answer.text);
    // One into another base.
    pulls(json!([cloud_pull(12, "acme/app", "acme/app", "release")]));
    let answer = write(&user, &mcp, merge.clone());
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
    let answer = write(&user, &mcp, merge);
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
    command(&user, "rerun");
    let (_, mcp) = run(&user, "rerun");
    let rerun = json!({"action": "checks", "repository": "."});
    let pipeline = |number: u64, branch: &str, result: &str| {
        json!({"uuid": format!("{{{number}}}"), "build_number": number,
            "state": {"name": "COMPLETED", "result": {"name": result}},
            "target": {"ref": {"name": branch}}})
    };
    let pipelines =
        |pipelines: Value| user.bkt_script(json!({"pipelines": {"acme/app": pipelines}}));

    // None for the branch.
    pipelines(json!([pipeline(9, "main", "FAILED")]));
    let answer = write(&user, &mcp, rerun.clone());
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
    let answer = write(&user, &mcp, rerun.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("nothing to re-run"), "{}", answer.text);
    assert_eq!(bkt_calls_starting(&user, "pipeline run"), 0);

    pipelines(json!([
        pipeline(9, "main", "SUCCESSFUL"),
        pipeline(8, "app/login", "FAILED"),
        pipeline(7, "app/login", "SUCCESSFUL")
    ]));
    let answer = write(&user, &mcp, rerun);
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
    command(&user, "rerun");
    let (_, mcp) = run(&user, "rerun");

    let answer = write(&user, &mcp, json!({"action": "checks", "repository": "."}));

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
    command(&user, "open");
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

    let answer = write(
        &user,
        &open,
        json!({"action": "create", "repository": ".", "title": "Login"}),
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
    command(&user, "open");
    let (_, open) = run(&user, "open");

    let answer = write(
        &user,
        &open,
        json!({"action": "create", "repository": ".", "title": "Login", "body": "Adds it"}),
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
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    let merge = json!({"action": "merge", "repository": "."});
    let page = "/repositories/acme/app/pullrequests/12";
    let script = |checks: Value, reviewers: &[&str]| {
        user.bkt_script(json!({
            "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]},
            "checks": {"acme/app": {"12": checks}},
            "api": {page: cloud_page(reviewers)}}));
    };
    let refused = |what: &str| {
        let answer = write(&user, &mcp, merge.clone());
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
        "forge",
        json!({"action": "merge", "repository": ".",
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

    // Passing checks and approving reviewers: merged once the user allows it.
    script(
        json!([{"state": "SUCCESSFUL", "name": "build"}]),
        &["approved"],
    );
    let answer = write(&user, &mcp, merge);
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 2);
}

/// What `bkt pr list --json` prints of the Data Center pull request #5 of `APP/app`, its one
/// reviewer's `status`.
fn dc_pull(status: &str) -> Value {
    json!({"id": 5, "title": "Login", "state": "OPEN",
        "fromRef": {"displayId": "app/login",
            "repository": {"slug": "app", "project": {"key": "APP"}}},
        "toRef": {"displayId": "main",
            "repository": {"slug": "app", "project": {"key": "APP"}}},
        "participants": [{"role": "REVIEWER", "status": status, "approved": false,
            "user": {"displayName": "Alice"}}],
        "links": {"self": [{"href": "https://bitbucket.example.com/projects/APP/repos/app/pull-requests/5"}]}})
}

#[test]
fn a_data_center_pull_request_a_reviewer_marked_needs_work_is_not_merged() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "ssh://git@bitbucket.example.com:7999/APP/app.git");
    let script = |status: &str| {
        user.bkt_script(json!({
            "contexts": [{"name": "work", "host": "bitbucket.example.com:7990"}],
            "pulls": {"APP/app": [dc_pull(status)]},
            "checks": {"APP/app": {"5": [{"state": "SUCCESSFUL", "name": "build"}]}}}));
    };
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    let merge = json!({"action": "merge", "repository": "."});

    script("NEEDS_WORK");
    let answer = write(&user, &mcp, merge.clone());
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("a review asks for changes"),
        "{}",
        answer.text
    );
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 0);

    script("APPROVED");
    let answer = write(&user, &mcp, merge);
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 1);
}

/// The fake `gh`'s pull request #12 of `app/login` into `main` in `acme/app`, with `checks`
/// (`statusCheckRollup`), and the merge methods the repository allows (all when none is given).
fn github_merge(user: &User, checks: Value, methods: Option<&[&str]>) {
    user.gh_script(github_script(checks, methods));
}

fn github_script(checks: Value, methods: Option<&[&str]>) -> Value {
    let mut script = json!({"pulls": {"acme/app": {"app/login": {"number": 12, "title": "Login",
        "state": "OPEN", "isDraft": false, "baseRefName": "main", "isCrossRepository": false,
        "url": "https://github.com/acme/app/pull/12", "statusCheckRollup": checks}}}});
    if let Some(methods) = methods {
        script["merge_methods"] = json!({"acme/app": methods});
    }
    script
}

fn passing() -> Value {
    json!([{"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
        "conclusion": "SUCCESS"}])
}

fn gh_merges(user: &User) -> Vec<String> {
    gh_calls(user)
        .into_iter()
        .filter(|call| call.starts_with("pr merge"))
        .collect()
}

#[test]
fn a_pull_request_with_no_check_is_refused_and_merged_after_the_user_s_explicit_words() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    github_merge(&user, json!([]), None);

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "the pull request #12 of app/login in acme/app is not merged: no check ran on this \
             pull request"
        ),
        "{}",
        answer.text
    );
    assert_eq!(gh_merges(&user), Vec::<String>::new());

    // The user's own words say to merge it without checks: held for them, and merged.
    let waived = mcp.call(
        "forge",
        json!({"action": "merge", "repository": ".",
            "waive": "this repository has no CI, merge it as it is"}),
    );
    assert!(
        waived.text.contains("Waiting for the user's approval"),
        "{}",
        waived.text
    );
    let requests = waiting(&user);
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0]["class"], "base", "a waived merge always asks");
    approve(&user, &requests[0]);
    eventually("the waived merge", || gh_merges(&user).len() == 1);
    assert_eq!(
        gh_merges(&user),
        ["pr merge 12 -R github.com/acme/app --squash"]
    );
}

#[test]
fn a_bitbucket_pull_request_with_no_check_is_refused_and_merged_after_the_user_s_explicit_words() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    user.bkt_script(json!({
        "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]},
        "api": {"/repositories/acme/app/pullrequests/12": cloud_page(&["approved"])}}));

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("no check ran on this pull request"),
        "{}",
        answer.text
    );
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 0);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "merge", "repository": ".",
            "waive": "no pipeline here, merge it anyway"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 1);
}

#[test]
fn a_pull_request_gh_answers_without_its_number_is_not_merged_and_says_why() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    let mut script = github_script(passing(), None);
    script["view_omits"] = json!({"acme/app": ["number"]});
    user.gh_script(script);

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "the pull request #12 of app/login in acme/app is not merged: it could not be read, \
             so nothing verified it"
        ),
        "{}",
        answer.text
    );
    assert_eq!(gh_merges(&user), Vec::<String>::new());
}

#[test]
fn a_pull_request_whose_checks_were_all_skipped_is_refused_as_one_with_no_check() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    github_merge(
        &user,
        json!([
            {"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
                "conclusion": "SKIPPED"},
            {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED",
                "conclusion": "NEUTRAL"}
        ]),
        None,
    );

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "the pull request #12 of app/login in acme/app is not merged: all its checks were \
             skipped (build, lint), none passed"
        ),
        "{}",
        answer.text
    );
    assert_eq!(gh_merges(&user), Vec::<String>::new());
    // One check that passed beside a skipped one verifies it.
    github_merge(
        &user,
        json!([
            {"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
                "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED",
                "conclusion": "SKIPPED"}
        ]),
        None,
    );
    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));
    assert!(!answer.error, "{}", answer.text);
}

#[test]
fn a_bitbucket_pull_request_whose_checks_cannot_be_read_is_not_merged_and_says_so() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    user.bkt_script(json!({
        "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]},
        "checks": {"acme/app": {"12": "unreadable"}},
        "api": {"/repositories/acme/app/pullrequests/12": cloud_page(&["approved"])}}));

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("the checks of the pull request #12 could not be read"),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains("no check ran"), "{}", answer.text);
    assert_eq!(bkt_calls_starting(&user, "pr merge"), 0);
}

#[test]
fn a_bitbucket_repository_that_allows_only_its_own_strategies_names_them_as_they_are() {
    // Cloud: `fast_forward` is a strategy of its own that no method here reaches.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    let mut page = cloud_page(&["approved"]);
    page["destination"] = json!({"branch": {"name": "main",
        "merge_strategies": ["fast_forward"], "default_merge_strategy": "fast_forward"}});
    user.bkt_script(json!({
        "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]},
        "checks": {"acme/app": {"12": [{"state": "SUCCESSFUL", "name": "build"}]}},
        "strategies": {"acme/app": ["fast_forward"]},
        "api": {"/repositories/acme/app/pullrequests/12": page}}));

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "it allows none of squash, merge and rebase, only its own strategies fast_forward \
             (no method reaches them)"
        ),
        "{}",
        answer.text
    );

    // Data Center: the ones that are reached are named, then the others.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "ssh://git@bitbucket.example.com:7999/APP/app.git");
    user.bkt_script(json!({
    "contexts": [{"name": "work", "host": "bitbucket.example.com:7990"}],
    "pulls": {"APP/app": [dc_pull("APPROVED")]},
    "checks": {"APP/app": {"5": [{"state": "SUCCESSFUL", "name": "build"}]}},
    "strategies": {"APP/app": ["ff", "rebase-no-ff", "squash-ff-only", "no-ff"]},
    "api": {"/rest/api/1.0/projects/APP/repos/app/settings/pull-requests": {
        "mergeConfig": {"defaultStrategy": {"id": "ff"}, "strategies": [
            {"id": "ff", "enabled": true},
            {"id": "rebase-no-ff", "enabled": true},
            {"id": "squash-ff-only", "enabled": true},
            {"id": "no-ff", "enabled": true},
            {"id": "squash", "enabled": false}
        ]}}}}));
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "it allows merge (method), and its own strategies ff, rebase-no-ff and \
             squash-ff-only (no method reaches them)"
        ),
        "{}",
        answer.text
    );
}

#[test]
fn each_merge_method_reaches_the_forge_squash_by_default() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    github_merge(&user, passing(), None);

    for method in [None, Some("squash"), Some("merge"), Some("rebase")] {
        let mut args = json!({"action": "merge", "repository": "."});
        if let Some(method) = method {
            args["method"] = method.into();
        }
        let answer = write(&user, &mcp, args);
        assert!(!answer.error, "{method:?}: {}", answer.text);
        let by = method.unwrap_or("squash");
        assert!(
            answer
                .text
                .contains(&format!("was merged by {by} into main")),
            "{}",
            answer.text
        );
    }
    assert_eq!(
        gh_merges(&user),
        [
            "pr merge 12 -R github.com/acme/app --squash",
            "pr merge 12 -R github.com/acme/app --squash",
            "pr merge 12 -R github.com/acme/app --merge",
            "pr merge 12 -R github.com/acme/app --rebase",
        ]
    );

    // Another method is refused before anything is asked of the forge, and so is a method on
    // another action.
    let calls = gh_calls(&user).len();
    for args in [
        json!({"action": "merge", "repository": ".", "method": "octopus"}),
        json!({"action": "create", "repository": ".", "title": "T", "method": "merge"}),
    ] {
        let answer = mcp.call("forge", args);
        assert!(answer.error, "{}", answer.text);
        assert!(answer.text.contains("method"), "{}", answer.text);
    }
    assert!(
        mcp.call(
            "forge",
            json!({"action": "merge", "repository": ".", "method": "octopus"})
        )
        .text
        .contains("its methods are squash (the default), merge and rebase")
    );
    assert_eq!(gh_calls(&user).len(), calls);
}

#[test]
fn each_merge_method_reaches_bitbucket_cloud_and_data_center() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    user.bkt_script(json!({
        "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]},
        "checks": {"acme/app": {"12": [{"state": "SUCCESSFUL", "name": "build"}]}},
        "api": {"/repositories/acme/app/pullrequests/12": cloud_page(&["approved"])}}));
    let merge = |method: &str| {
        write(
            &user,
            &mcp,
            json!({"action": "merge", "repository": ".", "method": method}),
        )
    };

    for method in ["squash", "merge"] {
        let answer = merge(method);
        assert!(!answer.error, "{method}: {}", answer.text);
    }
    // `bkt` merges a Cloud pull request by merge commit, squash or fast-forward, never by
    // rebase: refused before it is asked.
    let answer = merge("rebase");
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("rebase"), "{}", answer.text);
    let calls: Vec<String> = bkt_calls(&user)
        .into_iter()
        .filter(|call| call.starts_with("pr merge"))
        .collect();
    assert_eq!(
        calls,
        [
            "pr merge 12 --context cloud --workspace acme --repo app --strategy squash \
             --close-source=false",
            "pr merge 12 --context cloud --workspace acme --repo app --strategy merge_commit \
             --close-source=false",
        ]
    );
}

#[test]
fn each_merge_method_reaches_bitbucket_data_center() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "ssh://git@bitbucket.example.com:7999/APP/app.git");
    user.bkt_script(json!({
        "contexts": [{"name": "work", "host": "bitbucket.example.com:7990"}],
        "pulls": {"APP/app": [dc_pull("APPROVED")]},
        "checks": {"APP/app": {"5": [{"state": "SUCCESSFUL", "name": "build"}]}}}));
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");

    for method in ["squash", "merge", "rebase"] {
        let answer = write(
            &user,
            &mcp,
            json!({"action": "merge", "repository": ".", "method": method}),
        );
        assert!(!answer.error, "{method}: {}", answer.text);
    }
    let calls: Vec<String> = bkt_calls(&user)
        .into_iter()
        .filter(|call| call.starts_with("pr merge"))
        .collect();
    assert_eq!(
        calls,
        [
            "pr merge 5 --context work --project APP --repo app --strategy squash \
             --close-source=false",
            "pr merge 5 --context work --project APP --repo app --strategy no-ff \
             --close-source=false",
            "pr merge 5 --context work --project APP --repo app --strategy rebase-ff-only \
             --close-source=false",
        ]
    );
}

#[test]
fn a_method_the_repository_forbids_is_refused_naming_the_allowed_ones() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    github_merge(&user, passing(), Some(&["merge", "rebase"]));

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "acme/app does not allow merging by squash: it allows merge and rebase (method)"
        ),
        "{}",
        answer.text
    );
    let answer = write(
        &user,
        &mcp,
        json!({"action": "merge", "repository": ".", "method": "rebase"}),
    );
    assert!(!answer.error, "{}", answer.text);
}

#[test]
fn a_method_a_bitbucket_repository_forbids_is_refused_naming_the_allowed_ones() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "git@bitbucket.org:acme/app.git");
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");
    let mut page = cloud_page(&["approved"]);
    page["destination"] = json!({"branch": {"name": "main",
        "merge_strategies": ["merge_commit", "fast_forward"],
        "default_merge_strategy": "merge_commit"}});
    user.bkt_script(json!({
        "pulls": {"acme/app": [cloud_pull(12, "acme/app", "acme/app", "main")]},
        "checks": {"acme/app": {"12": [{"state": "SUCCESSFUL", "name": "build"}]}},
        "strategies": {"acme/app": ["merge_commit", "fast_forward"]},
        "api": {"/repositories/acme/app/pullrequests/12": page}}));

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("acme/app does not allow merging by squash: it allows merge (method)"),
        "{}",
        answer.text
    );
}

#[test]
fn a_method_a_data_center_repository_forbids_is_refused_naming_the_allowed_ones() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "ssh://git@bitbucket.example.com:7999/APP/app.git");
    user.bkt_script(json!({
    "contexts": [{"name": "work", "host": "bitbucket.example.com:7990"}],
    "pulls": {"APP/app": [dc_pull("APPROVED")]},
    "checks": {"APP/app": {"5": [{"state": "SUCCESSFUL", "name": "build"}]}},
    "strategies": {"APP/app": ["no-ff", "rebase-ff-only"]},
    "api": {"/rest/api/1.0/projects/APP/repos/app/settings/pull-requests": {
        "mergeConfig": {"defaultStrategy": {"id": "no-ff"}, "strategies": [
            {"id": "no-ff", "enabled": true},
            {"id": "squash", "enabled": false},
            {"id": "rebase-ff-only", "enabled": true}
        ]}}}}));
    command(&user, "merge");
    let (_, mcp) = run(&user, "merge");

    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "APP/app does not allow merging by squash: it allows merge and rebase (method)"
        ),
        "{}",
        answer.text
    );
}

/// A root agent started in `login` with no command: its id and MCP session.
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
fn sleeping(user: &User, mcp: &Mcp) -> String {
    let answer = allowed(
        user,
        mcp.call("run", json!({"line": "sleep 300", "timeout": 1})),
    );
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .split_once("\n\nrun ")
        .and_then(|(_, rest)| rest.split(':').next())
        .unwrap()
        .to_owned()
}

#[test]
fn workspace_remove_is_refused_while_an_agent_outside_the_tree_or_a_command_not_the_tree_s_works_in_the_workspace()
 {
    let user = User::with_fake_agent();
    let workspace = stack(&user, false, "");
    adopt(&user, "close");
    let (outsider, outside) = plain_agent_with_id(&user);
    let theirs = sleeping(&user, &outside);
    let (_, mcp) = run(&user, "close");
    // The tree's own command run is no obstacle.
    let ours = sleeping(&user, &mcp);

    let answer = remove(&user, &mcp, json!({}));
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(&format!(
            "the agent {outsider}, outside this agent's tree, works in it"
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

    let stopped = outside.call("run_status", json!({"run": theirs, "action": "stop"}));
    assert!(!stopped.error, "{}", stopped.text);
    let answer = remove(&user, &mcp, json!({}));
    assert!(answer.error, "{}", answer.text);
    assert!(!answer.text.contains(&theirs), "{}", answer.text);
    assert!(workspace.exists());

    user.call("agent.stop", json!({"agent": outsider})).unwrap();
    let answer = remove(&user, &mcp, json!({}));
    assert!(!answer.error, "{}", answer.text);
    assert!(!workspace.exists());
    let stopped = mcp.call("run_status", json!({"run": ours, "action": "stop"}));
    assert!(!stopped.error, "{}", stopped.text);
}

/// The MCP session of the agent a root launched: the last session opened.
fn child_of(user: &User, root: &Mcp, launch: Value) -> Mcp {
    let opened = user.received_requests("session/new").len();
    let launched = root.call("agent_launch", launch);
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

#[test]
fn forge_write_and_workspace_remove_are_listed_to_every_agent_of_a_workspace() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    command(&user, "push");
    let listed = |mcp: &Mcp| -> (bool, bool) {
        let names = mcp.names();
        (
            names.contains(&"forge".to_owned()),
            names.contains(&"workspace".to_owned()),
        )
    };

    assert_eq!(listed(&plain_agent(&user)), (true, true), "a root");
    let (_, mcp) = run(&user, "push");
    assert_eq!(listed(&mcp), (true, true), "a root sent a command");
    let child = child_of(
        &user,
        &mcp,
        json!({"title": "same place", "brief": "Open the pull request."}),
    );
    assert_eq!(listed(&child), (true, true), "its child");
}

#[test]
fn an_inline_command_stays_inline_when_it_is_edited() {
    let user = User::new();
    fixture(&user);
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!(
            "{text}\n[commands]\nship = {{ description = \"Ship\", instructions = \"Push.\" }}\n"
        ),
    )
    .unwrap();

    user.call(
        "command.edit",
        json!({"project": "app", "name": "ship", "instructions": "Push it."}),
    )
    .unwrap();

    let edited = std::fs::read_to_string(&file).unwrap();
    assert!(
        edited.contains("ship = { description = \"Ship\", instructions = \"Push it.\" }"),
        "{edited}"
    );
    assert!(!edited.contains("[commands.ship]"), "{edited}");
}
