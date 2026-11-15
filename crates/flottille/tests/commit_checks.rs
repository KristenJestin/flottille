//! The scenarios of a repository's commit checks (`docs/projects.md`, "Commit checks"): the lines
//! the user saves for a repository, run before an agent's `git commit` in it, `pre-commit` in its
//! folder and `commit-msg` on the commit's message; the lines its hooks propose, which nothing
//! runs until they are saved; and the hooks themselves, which never run. The checks are fake
//! programs first in the daemon's `PATH`, the agent the fake ACP agent, in throwaway repositories
//! and temporary XDG folders.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// The fake check programs, each recording in `checks/<name>.ran` the folder it ran in and its
/// arguments: `pre-lint` passes, `pre-fail` says what it found and fails, `msg-lint` keeps the
/// message it is given (its last argument) in `checks/seen` and fails on one holding `bad`.
fn with_checks() -> User {
    let user = User::with_fake_agent().with_fake_escalation();
    let bin = user.path("checks-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let checks = user.path("checks");
    std::fs::create_dir_all(&checks).unwrap();
    let record = |name: &str| format!("echo \"$(pwd) $*\" >> '{}/{name}.ran'\n", checks.display());
    let programs = [
        ("pre-lint", format!("#!/bin/sh\n{}", record("pre-lint"))),
        (
            "pre-fail",
            format!(
                "#!/bin/sh\n{}echo 'lint: change.txt is not formatted'\nexit 1\n",
                record("pre-fail")
            ),
        ),
        (
            "msg-lint",
            format!(
                "#!/bin/sh\n{}for last; do :; done\ncat \"$last\" >> '{seen}'\n\
                 if grep -q bad \"$last\"; then echo \"msg: the subject says bad\"; exit 1; fi\n",
                record("msg-lint"),
                seen = checks.join("seen").display()
            ),
        ),
    ];
    for (name, text) in programs {
        let path = bin.join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // In front of the fake escalation programs, in front of the test's own.
    let path = format!(
        "{}:{}:{}",
        bin.display(),
        user.path("escalation-bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    user.with_env("PATH", &path)
}

/// A project `app` over a throwaway repository pushed to a throwaway remote, `prepare` given its
/// folder before its first commit, and the workspace `login` made (offline). The folder.
fn prepared(user: &User, prepare: impl FnOnce(&Path)) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    prepare(&folder);
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

/// Saves the repository's commit checks, as the project's Settings does.
fn declare(user: &User, pre_commit: &[&str], commit_msg: &[&str]) -> Value {
    user.call(
        "project.set_commit_checks",
        json!({
            "name": "app",
            "repository": ".",
            "pre_commit": pre_commit,
            "commit_msg": commit_msg,
        }),
    )
    .unwrap()
}

/// What a fake check recorded of its runs, one line each.
fn ran(user: &User, check: &str) -> String {
    std::fs::read_to_string(user.path(&format!("checks/{check}.ran"))).unwrap_or_default()
}

/// The messages `msg-lint` was given, each in turn.
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
    write(user, mcp, "change.txt", "change\n");
    let added = line(user, mcp, "git add change.txt");
    assert!(added.text.contains("exited with code 0"), "{}", added.text);
}

/// The subjects of the workspace's branch, newest first.
fn subjects(user: &User) -> String {
    user.git_output(&worktree(user), &["log", "--format=%s"])
}

/// The message files the checks were given, left in the daemon's state folder.
fn message_files(user: &User) -> usize {
    std::fs::read_dir(user.path("state/flottille/messages")).map_or(0, |files| files.count())
}

/// A hooks folder in the repository whose `pre-commit` and `commit-msg` would mark `hook-ran` if
/// git ran them; `hooks_on` makes it the repository's own (`core.hooksPath`).
fn hooks_folder(user: &User, folder: &Path) {
    let hooks = folder.join(".githooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for hook in ["pre-commit", "commit-msg"] {
        let path = hooks.join(hook);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho {hook} >> '{}'\n",
                user.path("hook-ran").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The user's hooks folder of `hooks_folder` turned on, after the user's own commits.
fn hooks_on(user: &User) {
    user.git(
        &user.path("work/app"),
        &["config", "core.hooksPath", ".githooks"],
    );
}

/// A file the agent writes in the workspace, allowed by the user when it is held.
fn write(user: &User, mcp: &Mcp, path: &str, content: &str) {
    let answer = mcp.call("fs_write", json!({"path": path, "content": content}));
    let answer = match answer.text.starts_with("Waiting for the user's approval") {
        true => allowed(user, answer),
        false => answer,
    };
    assert!(!answer.error, "{}", answer.text);
}

fn hook_ran(user: &User) -> String {
    std::fs::read_to_string(user.path("hook-ran")).unwrap_or_default()
}

#[test]
fn a_failing_pre_commit_check_refuses_the_agent_s_commit_with_its_output() {
    let user = with_checks();
    prepared(&user, |_| {});
    declare(&user, &["pre-lint", "pre-fail --strict"], &[]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit -m 'a change'");

    assert!(answer.error, "{}", answer.text);
    for said in [
        "the pre-commit check `pre-fail --strict` of the repository . refused the commit (it \
         exited with code 1)",
        "nothing was committed",
        "lint: change.txt is not formatted",
    ] {
        assert!(answer.text.contains(said), "{said}: {}", answer.text);
    }
    assert!(!subjects(&user).contains("a change"), "{}", subjects(&user));
    // Each ran in the repository's folder of the workspace, in their order.
    let folder = worktree(&user).canonicalize().unwrap();
    assert_eq!(ran(&user, "pre-lint"), format!("{} \n", folder.display()));
    assert_eq!(
        ran(&user, "pre-fail"),
        format!("{} --strict\n", folder.display())
    );
}

#[test]
fn a_failing_commit_msg_check_refuses_the_commit_and_sees_the_message() {
    let user = with_checks();
    prepared(&user, |_| {});
    declare(&user, &[], &["msg-lint --edit {message_file}"]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit -m 'bad change' -m 'Its body.'");

    assert!(answer.error, "{}", answer.text);
    for said in [
        "the commit-msg check `msg-lint --edit {message_file}` of the repository . refused the \
         commit message (it exited with code 1)",
        "nothing was committed",
        "msg: the subject says bad",
    ] {
        assert!(answer.text.contains(said), "{said}: {}", answer.text);
    }
    assert!(
        !subjects(&user).contains("bad change"),
        "{}",
        subjects(&user)
    );
    // The message as git joins its -m, in a file of Flottille's outside the repository, removed.
    assert_eq!(seen(&user), "bad change\n\nIts body.\n");
    let given = ran(&user, "msg-lint");
    let file = given.trim().rsplit(' ').next().unwrap();
    assert!(
        file.starts_with(user.path("state/flottille/messages").to_str().unwrap()),
        "{given}"
    );
    assert_eq!(message_files(&user), 0);

    // A message read from -F is seen as well, and one that passes is committed.
    write(&user, &mcp, "message.txt", "a good change\n");
    let answer = line(&user, &mcp, "git commit -F message.txt");
    assert!(!answer.error, "{}", answer.text);
    assert!(
        subjects(&user).starts_with("a good change"),
        "{}",
        subjects(&user)
    );
    assert!(seen(&user).ends_with("a good change\n"), "{}", seen(&user));
}

#[test]
fn passing_checks_let_the_commit_run_with_no_hook() {
    let user = with_checks();
    prepared(&user, |folder| hooks_folder(&user, folder));
    hooks_on(&user);
    declare(&user, &["pre-lint"], &["msg-lint {message_file}"]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit -m 'a good change'");

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("exited with code 0"),
        "{}",
        answer.text
    );
    assert!(
        subjects(&user).starts_with("a good change"),
        "{}",
        subjects(&user)
    );
    assert_eq!(ran(&user, "pre-lint").lines().count(), 1);
    assert_eq!(seen(&user), "a good change\n");
    assert_eq!(hook_ran(&user), "", "a hook ran");
}

#[test]
fn a_check_with_shell_syntax_is_refused_when_saved() {
    let user = with_checks();
    prepared(&user, |_| {});
    let before = std::fs::read_to_string(user.project_file("app")).unwrap();

    for (pre_commit, commit_msg) in [
        (vec!["pre-lint && pre-fail"], vec![]),
        (vec![], vec!["msg-lint {message_file} | tee out"]),
    ] {
        let (code, message) = user
            .call(
                "project.set_commit_checks",
                json!({"name": "app", "repository": ".", "pre_commit": pre_commit,
                       "commit_msg": commit_msg}),
            )
            .unwrap_err();
        assert_eq!(code, "refused", "{message}");
        assert!(message.contains("cannot be used"), "{message}");
        assert!(message.contains("nothing was saved"), "{message}");
        // The sentence every command line gets for its shell syntax.
        assert!(message.contains("is shell syntax"), "{message}");
    }
    // A commit-msg check that names no message, and a pre-commit one that does, are refused too.
    for (pre_commit, commit_msg, said) in [
        (vec![], vec!["msg-lint"], "must name {message_file}"),
        (
            vec!["pre-lint {message_file}"],
            vec![],
            "names {message_file}",
        ),
    ] {
        let (_, message) = user
            .call(
                "project.set_commit_checks",
                json!({"name": "app", "repository": ".", "pre_commit": pre_commit,
                       "commit_msg": commit_msg}),
            )
            .unwrap_err();
        assert!(message.contains(said), "{message}");
    }
    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        before
    );

    // Through the command line as well.
    let refused = user.run(&[
        "project",
        "set-commit-checks",
        "app",
        ".",
        "--pre-commit",
        "pre-lint; pre-fail",
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("nothing was saved"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn checks_are_proposed_from_the_repository_s_hooks_and_run_only_once_saved() {
    let user = with_checks();
    prepared(&user, |folder| {
        let husky = folder.join(".husky");
        std::fs::create_dir_all(&husky).unwrap();
        std::fs::write(
            husky.join("pre-commit"),
            "# lint what is staged\npre-lint --staged\npre-fail && echo done\n",
        )
        .unwrap();
        std::fs::write(husky.join("commit-msg"), "msg-lint --edit \"$1\"\n").unwrap();
    });
    let mcp = agent(&user);

    let checks = user
        .call("project.commit_checks", json!({"name": "app"}))
        .unwrap();
    let repository = &checks["repositories"][0];
    assert_eq!(repository["path"], ".");
    assert_eq!(repository["pre_commit"], json!([]));
    assert_eq!(
        repository["proposed"],
        json!([
            {"hook": "pre-commit", "line": "pre-lint --staged", "from": ".husky/pre-commit"},
            {"hook": "commit-msg", "line": "msg-lint --edit {message_file}",
             "from": ".husky/commit-msg"},
        ])
    );
    let left = &repository["left_out"];
    assert_eq!(left.as_array().unwrap().len(), 1, "{left}");
    assert_eq!(left[0]["line"], "pre-fail && echo done");
    assert_eq!(left[0]["from"], ".husky/pre-commit");
    assert!(left[0]["why"].as_str().unwrap().contains("&&"), "{left}");

    // Proposed is not saved: the agent's commit runs none of them.
    staged(&user, &mcp);
    let answer = line(&user, &mcp, "git commit -m 'bad but unchecked'");
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(ran(&user, "pre-lint"), "");
    assert_eq!(seen(&user), "");

    // Saved from the command line, as proposed: they run, and are no longer proposed.
    let saved = user.run(&[
        "project",
        "set-commit-checks",
        "app",
        ".",
        "--pre-commit",
        "pre-lint --staged",
        "--commit-msg",
        "msg-lint --edit {message_file}",
    ]);
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    let shown = String::from_utf8_lossy(&saved.stdout);
    assert!(
        shown.contains("pre-commit:\n    pre-lint --staged"),
        "{shown}"
    );
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(
        file.contains(
            "commit_checks = { pre_commit = [\"pre-lint --staged\"], commit_msg = [\"msg-lint \
             --edit {message_file}\"] }"
        ),
        "{file}"
    );
    let checks = user
        .call("project.commit_checks", json!({"name": "app"}))
        .unwrap();
    assert_eq!(checks["repositories"][0]["proposed"], json!([]));
    write(&user, &mcp, "second.txt", "second\n");
    line(&user, &mcp, "git add second.txt");
    let answer = line(&user, &mcp, "git commit -m 'bad and checked'");
    assert!(answer.error, "{}", answer.text);
    assert_eq!(ran(&user, "pre-lint").lines().count(), 1);
    assert_eq!(seen(&user), "bad and checked\n");
}

#[test]
fn a_hook_file_the_agent_wrote_is_never_run() {
    let user = with_checks();
    prepared(&user, |folder| hooks_folder(&user, folder));
    hooks_on(&user);
    declare(&user, &["pre-lint"], &[]);
    let mcp = agent(&user);

    // The agent rewrites the repository's hook to mark something of its own, and stages it.
    let marker = user.path("agent-hook-ran");
    let read = mcp.call("fs_read", json!({"path": ".githooks/pre-commit"}));
    assert!(!read.error, "{}", read.text);
    let hook = format!("#!/bin/sh\necho agent >> '{}'\n", marker.display());
    write(&user, &mcp, ".githooks/pre-commit", &hook);
    line(&user, &mcp, "git add .githooks/pre-commit");
    let answer = line(&user, &mcp, "git commit -m 'a new hook'");

    assert!(!answer.error, "{}", answer.text);
    assert!(
        subjects(&user).starts_with("a new hook"),
        "{}",
        subjects(&user)
    );
    assert_eq!(ran(&user, "pre-lint").lines().count(), 1);
    assert!(!marker.exists(), "the agent's hook ran");
    assert_eq!(hook_ran(&user), "", "a hook ran");
}

#[test]
fn no_verify_on_an_agent_s_commit_is_refused() {
    let user = with_checks();
    prepared(&user, |_| {});
    declare(&user, &["pre-lint"], &[]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    for commit in [
        "git commit --no-verify -m 'skipped'",
        "git commit -n -m 'skipped'",
        "git commit -anm 'skipped'",
        "git commit --no-verif -m 'skipped'",
    ] {
        let answer = line(&user, &mcp, commit);
        assert!(answer.error, "{commit}: {}", answer.text);
        assert!(
            answer.text.contains(
                "--no-verify skips the commit checks of the repository ., which are the user's"
            ),
            "{commit}: {}",
            answer.text
        );
        assert!(
            answer.text.contains("nothing was committed"),
            "{}",
            answer.text
        );
    }
    assert!(!subjects(&user).contains("skipped"), "{}", subjects(&user));
    assert_eq!(ran(&user, "pre-lint"), "");
    assert!(user.escalations().is_empty());
}

#[test]
fn a_repository_without_checks_commits_as_today() {
    let user = with_checks();
    prepared(&user, |folder| hooks_folder(&user, folder));
    hooks_on(&user);
    let checks = user
        .call("project.commit_checks", json!({"name": "app"}))
        .unwrap();
    assert_eq!(checks["repositories"][0]["pre_commit"], json!([]));
    assert_eq!(checks["repositories"][0]["commit_msg"], json!([]));
    let mcp = agent(&user);
    staged(&user, &mcp);

    let answer = line(&user, &mcp, "git commit --no-verify -m 'as today'");

    assert!(!answer.error, "{}", answer.text);
    assert!(
        subjects(&user).starts_with("as today"),
        "{}",
        subjects(&user)
    );
    assert_eq!(hook_ran(&user), "", "a hook ran");
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!file.contains("commit_checks"), "{file}");
}

#[test]
fn a_hook_manager_s_entry_point_is_proposed_as_one_check() {
    let user = with_checks();
    let folder = user.path("work/many");
    let repository = |name: &str, files: &[(&str, &str, bool)]| {
        let path = folder.join(name);
        user.git_init(&path);
        // git's own hooks (`hooks`) once the user's commit is made, which would run them.
        let write = |hooks: bool| {
            for (file, text, executable) in files {
                if file.starts_with(".git/") != hooks {
                    continue;
                }
                let file = path.join(file);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, text).unwrap();
                if *executable {
                    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
            }
        };
        write(false);
        user.git(&path, &["add", "."]);
        user.git(&path, &["commit", "--quiet", "-m", "start"]);
        write(true);
    };
    // lefthook, with the shim `lefthook install` writes in .git/hooks, which is not proposed.
    repository(
        "lefty",
        &[
            (
                "lefthook.yml",
                "pre-commit:\n  commands:\n    lint:\n      run: pre-lint {staged_files}\n\
                 commit-msg:\n  commands:\n    lint:\n      run: msg-lint {1}\n",
                false,
            ),
            (
                ".git/hooks/pre-commit",
                "#!/bin/sh\nlefthook run pre-commit \"$@\"\n",
                true,
            ),
        ],
    );
    repository(
        "snake",
        &[(
            ".pre-commit-config.yaml",
            "repos:\n- repo: local\n  hooks:\n  - id: lint\n    entry: pre-lint\n    \
             language: system\n  - id: message\n    entry: msg-lint\n    language: system\n    \
             stages: [commit-msg]\n",
            false,
        )],
    );
    // An older husky's hook file is a program of the repository: run itself, as it was written.
    repository(
        "husk",
        &[(
            ".husky/commit-msg",
            "#!/usr/bin/env sh\n. \"$(dirname -- \"$0\")/_/husky.sh\"\nmsg-lint --edit \"$1\"\n",
            true,
        )],
    );
    user.call("project.add", json!({"folder": folder, "name": "many"}))
        .unwrap();

    let checks = user
        .call("project.commit_checks", json!({"name": "many"}))
        .unwrap();
    let proposed = |path: &str| {
        checks["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|repository| repository["path"] == path)
            .map(|repository| repository["proposed"].clone())
            .unwrap()
    };
    assert_eq!(
        proposed("lefty"),
        json!([
            {"hook": "pre-commit", "line": "lefthook run pre-commit", "from": "lefthook.yml"},
            {"hook": "commit-msg", "line": "lefthook run commit-msg {message_file}",
             "from": "lefthook.yml"},
        ])
    );
    assert_eq!(
        proposed("snake"),
        json!([
            {"hook": "pre-commit", "line": "pre-commit run --hook-stage pre-commit",
             "from": ".pre-commit-config.yaml"},
            {"hook": "commit-msg",
             "line": "pre-commit run --hook-stage commit-msg --commit-msg-filename {message_file}",
             "from": ".pre-commit-config.yaml"},
        ])
    );
    assert_eq!(
        proposed("husk"),
        json!([
            {"hook": "commit-msg", "line": "./.husky/commit-msg {message_file}",
             "from": ".husky/commit-msg"},
        ])
    );
    // Reading them ran nothing.
    assert_eq!(ran(&user, "pre-lint"), "");
    assert_eq!(seen(&user), "");
}

#[test]
fn a_repository_script_runs_as_a_check() {
    let user = with_checks();
    let record = user.path("checks/script.ran");
    prepared(&user, |folder| {
        let script = folder.join("scripts/check-commit.sh");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$(pwd)\" >> '{}'\n\
                 if grep -q wip \"$1\"; then echo 'no wip commit, please'; exit 3; fi\n",
                record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    });
    declare(&user, &[], &["./scripts/check-commit.sh {message_file}"]);
    let mcp = agent(&user);
    staged(&user, &mcp);

    let refused = line(&user, &mcp, "git commit -m 'wip'");
    assert!(refused.error, "{}", refused.text);
    assert!(
        refused.text.contains("(it exited with code 3)"),
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains("no wip commit, please"),
        "{}",
        refused.text
    );
    let done = line(&user, &mcp, "git commit -m 'a finished change'");
    assert!(!done.error, "{}", done.text);
    assert!(
        subjects(&user).starts_with("a finished change"),
        "{}",
        subjects(&user)
    );
    // The workspace's own script, in its checkout, each time.
    let folder = worktree(&user).canonicalize().unwrap();
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        format!("{0}\n{0}\n", folder.display())
    );
}

/// A project `app` whose folder holds the repositories `api` and `web`, each pushed to its
/// throwaway remote, and its workspace `login` made (offline). The workspace's folder.
fn two_repositories(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    for name in ["api", "web"] {
        let repository = folder.join(name);
        user.git_init(&repository);
        std::fs::write(repository.join("README.md"), format!("{name}\n")).unwrap();
        user.git(&repository, &["add", "."]);
        user.git(&repository, &["commit", "--quiet", "-m", "start"]);
        let remote = user.path(&format!("remotes/{name}.git"));
        let remote = remote.to_str().unwrap();
        user.git(&user.path(""), &["init", "--quiet", "--bare", remote]);
        user.git(&repository, &["remote", "add", "origin", remote]);
        user.git(&repository, &["push", "--quiet", "origin", "main"]);
    }
    user.write_project_file(
        "app",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \"api\"\n\n[[repositories]]\npath = \
             \"web\"\n",
            folder.display()
        ),
    );
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    PathBuf::from(workspace["folder"].as_str().unwrap())
}

/// Saves the commit checks of one repository of `app`.
fn declare_in(user: &User, repository: &str, pre_commit: &[&str], commit_msg: &[&str]) {
    user.call(
        "project.set_commit_checks",
        json!({
            "name": "app",
            "repository": repository,
            "pre_commit": pre_commit,
            "commit_msg": commit_msg,
        }),
    )
    .unwrap();
}

/// A line the agent runs in one repository of the workspace (`None`: its folder), allowed by the
/// user when it is held.
fn line_in(user: &User, mcp: &Mcp, repository: Option<&str>, line: &str) -> Answer {
    let mut asked = json!({"line": line, "timeout": 30});
    if let Some(repository) = repository {
        asked["repository"] = json!(repository);
    }
    let answer = mcp.call("run", asked);
    if answer.text.starts_with("Waiting for the user's approval") {
        allowed(user, answer)
    } else {
        answer
    }
}

/// A change staged in `web`, by the user.
fn staged_in_web(user: &User, workspace: &Path) {
    let web = workspace.join("web");
    std::fs::write(web.join("change.txt"), "change\n").unwrap();
    user.git(&web, &["add", "change.txt"]);
}

fn subjects_of(user: &User, folder: &Path) -> String {
    user.git_output(folder, &["log", "--format=%s"])
}

#[test]
fn a_commit_with_dash_c_into_another_repository_runs_that_repository_s_checks() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-fail"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);

    let answer = line_in(
        &user,
        &mcp,
        Some("api"),
        "git -C ../web commit -m 'from api'",
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("the pre-commit check `pre-fail` of the repository web refused the commit"),
        "{}",
        answer.text
    );
    let web = workspace.join("web");
    assert!(!subjects_of(&user, &web).contains("from api"));
    let folder = web.canonicalize().unwrap();
    assert_eq!(ran(&user, "pre-fail"), format!("{} \n", folder.display()));
}

#[test]
fn a_commit_under_env_dash_c_runs_the_checks_of_that_folder() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-fail"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);

    let answer = line_in(
        &user,
        &mcp,
        None,
        "env -C web git commit -m 'from the root'",
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("the pre-commit check `pre-fail` of the repository web refused the commit"),
        "{}",
        answer.text
    );
    let web = workspace.join("web");
    assert!(!subjects_of(&user, &web).contains("from the root"));
    assert_eq!(ran(&user, "pre-fail").lines().count(), 1);
}

#[test]
fn no_verify_through_dash_c_is_refused_where_checks_exist() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-lint"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);

    for (repository, line) in [
        (Some("api"), "git -C ../web commit -n -m 'skipped'"),
        (None, "env -C web git commit --no-verify -m 'skipped'"),
    ] {
        let answer = line_in(&user, &mcp, repository, line);
        assert!(answer.error, "{line}: {}", answer.text);
        assert!(
            answer
                .text
                .contains("--no-verify skips the commit checks of the repository web"),
            "{line}: {}",
            answer.text
        );
    }
    assert!(!subjects_of(&user, &workspace.join("web")).contains("skipped"));
    assert_eq!(ran(&user, "pre-lint"), "");
}

#[test]
fn a_commit_with_git_dir_is_refused_where_checks_exist() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-lint"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);

    for (line, pointer) in [
        (
            "git --git-dir ../web/.git --work-tree ../web commit -m 'elsewhere'",
            "--git-dir",
        ),
        (
            "git --work-tree=../web commit -m 'elsewhere'",
            "--work-tree",
        ),
        (
            "env GIT_DIR=../web/.git git commit -m 'elsewhere'",
            "GIT_DIR",
        ),
        (
            "env GIT_WORK_TREE=../web git commit -m 'elsewhere'",
            "GIT_WORK_TREE",
        ),
    ] {
        let answer = line_in(&user, &mcp, Some("api"), line);
        assert!(answer.error, "{line}: {}", answer.text);
        assert!(
            answer.text.contains(&format!(
                "refused: {pointer} points the commit at a repository other than its folder's"
            )),
            "{line}: {}",
            answer.text
        );
        assert!(
            answer.text.contains("nothing was committed"),
            "{}",
            answer.text
        );
    }
    assert!(!subjects_of(&user, &workspace.join("web")).contains("elsewhere"));
    assert!(!subjects_of(&user, &workspace.join("api")).contains("elsewhere"));
    assert_eq!(ran(&user, "pre-lint"), "");
}

#[test]
fn a_message_file_is_read_from_the_folder_git_commits_in() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &[], &["msg-lint {message_file}"]);
    // The project's message checks read it too.
    user.call(
        "project.set_message_checks",
        json!({"name": "app", "checks": ["msg-lint {message}"]}),
    )
    .unwrap();
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);
    std::fs::write(workspace.join("msg.txt"), "bad, from the root\n").unwrap();
    std::fs::write(workspace.join("web/msg.txt"), "from web\n").unwrap();

    let answer = line_in(&user, &mcp, None, "env -C web git commit -F msg.txt");

    assert!(!answer.error, "{}", answer.text);
    assert_eq!(seen(&user), "from web\nfrom web\n");
    assert!(
        subjects_of(&user, &workspace.join("web")).starts_with("from web"),
        "{}",
        subjects_of(&user, &workspace.join("web"))
    );
}

#[test]
fn a_fifo_named_lefthook_yml_is_not_read() {
    use std::io::{BufRead, BufReader, Write};
    let user = with_checks();
    let folder = prepared(&user, |_| {});
    let made = std::process::Command::new("mkfifo")
        .arg(folder.join("lefthook.yml"))
        .status()
        .unwrap();
    assert!(made.success());
    user.status();

    // A FIFO with no writer would hold its read, and the daemon's answers with it, forever.
    let mut stream =
        std::os::unix::net::UnixStream::connect(user.path("runtime/flottille/daemon.sock"))
            .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let request = json!({"id": "1", "method": "project.commit_checks", "params": {"name": "app"}});
    writeln!(stream, "{request}").unwrap();
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .expect("the daemon answers");
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        answer["result"]["repositories"][0]["proposed"],
        json!([]),
        "{answer}"
    );
}

/// Each line, run by the agent in the repository `from` (`None`: the workspace's folder), is
/// refused with `said`, and commits nothing into `web`.
fn each_refused(
    user: &User,
    mcp: &Mcp,
    workspace: &Path,
    from: Option<&str>,
    lines: &[&str],
    said: &str,
) {
    for line in lines {
        let answer = line_in(user, mcp, from, line);
        assert!(answer.error, "{line}: {}", answer.text);
        assert!(answer.text.contains(said), "{line}: {}", answer.text);
        assert!(
            answer.text.contains("nothing was committed"),
            "{line}: {}",
            answer.text
        );
    }
    assert!(!subjects_of(user, &workspace.join("web")).contains("into web"));
}

#[test]
fn env_s_abbreviated_and_grouped_options_move_the_commit_into_their_folder() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-lint"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);

    each_refused(
        &user,
        &mcp,
        &workspace,
        Some("api"),
        &[
            "env --c=../web git commit -n -m 'into web'",
            "env --ch ../web git commit -n -m 'into web'",
            "env -vC ../web git commit -n -m 'into web'",
            "env -ivC../web git commit -n -m 'into web'",
            "env -C ../nowhere --chdir=../web git commit -n -m 'into web'",
            "env -S '-C ../web git commit -n' -m 'into web'",
            "env -vS'-v -C ../web git' commit -n -m 'into web'",
        ],
        "--no-verify skips the commit checks of the repository web",
    );
    assert_eq!(ran(&user, "pre-lint"), "");
}

#[test]
fn a_program_that_changes_folder_itself_cannot_commit_where_checks_exist() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-lint"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);

    each_refused(
        &user,
        &mcp,
        &workspace,
        Some("api"),
        &[
            "pnpm -C ../web exec git commit -m 'into web'",
            "pnpm exec -C ../web git commit -m 'into web'",
            "uv run --directory ../web git commit -m 'into web'",
            "find ../web/README.md -execdir git commit -m 'into web' ';'",
            "find ../web/README.md -okdir env git commit -m 'into web' ';'",
            "fd --base-directory ../web README -x git commit -m 'into web'",
            "fd --base-directory=../web README -x git ci -m 'into web'",
        ],
        "runs git in a folder it chooses itself",
    );
    assert_eq!(ran(&user, "pre-lint"), "");
}

#[test]
fn git_dir_in_any_assignment_word_of_any_layer_is_refused_where_checks_exist() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-lint"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);

    each_refused(
        &user,
        &mcp,
        &workspace,
        Some("api"),
        &[
            "sh -c \"A=1 GIT_DIR=../web/.git git commit -n -m 'into web'\"",
            "env -S 'GIT_DIR=../web/.git git commit' -m 'into web'",
            "env --split-string='A=1 GIT_DIR=../web/.git git' commit -m 'into web'",
            "env A=1 GIT_DIR=../web/.git git commit -m 'into web'",
        ],
        "GIT_DIR points the commit at a repository other than its folder's",
    );
    assert_eq!(ran(&user, "pre-lint"), "");
}

#[test]
fn every_git_variable_is_refused_on_a_commit_where_checks_exist() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-lint"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);

    for (line, name) in [
        (
            "env GIT_COMMON_DIR=../web/.git git commit -m 'into web'",
            "GIT_COMMON_DIR",
        ),
        (
            "env GIT_INDEX_FILE=../web/.git/index git commit -m 'into web'",
            "GIT_INDEX_FILE",
        ),
    ] {
        each_refused(
            &user,
            &mcp,
            &workspace,
            Some("api"),
            &[line],
            &format!("refused: {name} changes where or how git commits"),
        );
    }
    assert_eq!(ran(&user, "pre-lint"), "");
}

#[test]
fn a_worktree_of_web_inside_api_runs_web_s_checks() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-fail"], &[]);
    let mcp = agent(&user);
    let inside = workspace.join("api/web-tree");
    user.git(
        &workspace.join("web"),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "tree",
            inside.to_str().unwrap(),
        ],
    );
    std::fs::write(inside.join("change.txt"), "change\n").unwrap();
    user.git(&inside, &["add", "change.txt"]);

    let answer = line_in(
        &user,
        &mcp,
        Some("api"),
        "git -C web-tree commit -m 'into web'",
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("the pre-commit check `pre-fail` of the repository web refused the commit"),
        "{}",
        answer.text
    );
    assert!(!subjects_of(&user, &inside).contains("into web"));
    let folder = inside.canonicalize().unwrap();
    assert_eq!(ran(&user, "pre-fail"), format!("{} \n", folder.display()));
}

#[test]
fn a_nested_git_file_in_api_pointing_at_web_runs_web_s_checks() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-fail"], &[]);
    let mcp = agent(&user);
    staged_in_web(&user, &workspace);
    let web_git = user.git_output(&workspace.join("web"), &["rev-parse", "--absolute-git-dir"]);
    let nested = workspace.join("api/nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join(".git"), format!("gitdir: {}\n", web_git.trim())).unwrap();

    let answer = line_in(
        &user,
        &mcp,
        Some("api"),
        "git -C nested commit -m 'into web'",
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("the pre-commit check `pre-fail` of the repository web refused the commit"),
        "{}",
        answer.text
    );
    assert!(!subjects_of(&user, &workspace.join("web")).contains("into web"));
}

#[test]
fn a_worktree_of_web_outside_the_workspace_runs_web_s_checks() {
    let user = with_checks();
    let workspace = two_repositories(&user);
    declare_in(&user, "web", &["pre-fail"], &[]);
    let mcp = agent(&user);
    let outside = user.path("elsewhere/web-tree");
    user.git(
        &workspace.join("web"),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "outside",
            outside.to_str().unwrap(),
        ],
    );
    std::fs::write(outside.join("change.txt"), "change\n").unwrap();
    user.git(&outside, &["add", "change.txt"]);

    let line = format!("git -C {} commit -m 'into web'", outside.display());
    let answer = line_in(&user, &mcp, Some("api"), &line);

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("the pre-commit check `pre-fail` of the repository web refused the commit"),
        "{}",
        answer.text
    );
    assert!(!subjects_of(&user, &outside).contains("into web"));
}
