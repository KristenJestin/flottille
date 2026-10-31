//! The scenarios of the forges (`docs/forges.md`): a workspace's pull requests and checks, read
//! through a fake `gh` (`crates/flottille-fake-gh`) on the daemon's `PATH`, in throwaway
//! repositories whose `origin` looks like GitHub's. No test ever calls GitHub.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// A project `app` of three repositories, `api` and `web` on GitHub (over SSH and over HTTPS) and
/// `lib` on another host, and its ready workspace `login` on the branch `app/login`. Each
/// repository is pushed to a bare remote of its own first, so that the workspace's fetch works;
/// its `origin` then says where the forge is.
fn fixture(user: &User) -> PathBuf {
    fixture_at(
        user,
        &[
            ("api", "git@github.com:acme/api.git"),
            ("web", "https://github.com/acme/web.git"),
            ("lib", "https://git.example.com/acme/lib.git"),
        ],
    )
}

/// The same project and workspace, each repository's `origin` set to the URL given.
fn fixture_at(user: &User, urls: &[(&str, &str)]) -> PathBuf {
    let folder = user.path("work/app");
    for repository in ["api", "web", "lib"] {
        let dir = folder.join(repository);
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), format!("{repository}\n")).unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
        let remote = user.path(&format!("remotes/{repository}.git"));
        user.git(
            &user.path(""),
            &["init", "--quiet", "--bare", remote.to_str().unwrap()],
        );
        user.git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        user.git(&dir, &["push", "--quiet", "origin", "main"]);
    }
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    for (repository, url) in urls {
        user.git(
            &folder.join(repository),
            &["remote", "set-url", "origin", url],
        );
    }
    declare_proposed(user);
    folder
}

/// What `lib` and every repository whose forge is not declared says.
const NO_FORGE: &str = "no forge set for this repository: choose it in the project's Settings \
    (the window), or write its `forge` in the project's file; Flottille never guesses it";

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

/// What `gh pr view --json` answers for a pull request of `acme/api`.
fn pull(number: u64, title: &str) -> Value {
    json!({
        "number": number,
        "title": title,
        "state": "OPEN",
        "isDraft": false,
        "baseRefName": "main",
        "url": format!("https://github.com/acme/api/pull/{number}"),
        "reviewDecision": "CHANGES_REQUESTED",
        "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "name": "lint", "status": "IN_PROGRESS", "conclusion": ""}
        ],
        "comments": [{"author": {"login": "alice"}, "body": "Looks good to me"}],
        "reviews": [{"author": {"login": "bob"}, "state": "CHANGES_REQUESTED", "body": "Rename the form"}]
    })
}

/// The fake `gh`'s script: the pull request of `app/login` in `acme/api`, none in `acme/web`.
fn one_pull_request(user: &User, pull: Value) {
    user.gh_script(json!({"pulls": {"acme/api": {"app/login": pull}}}));
}

fn forge(user: &User) -> Value {
    user.call(
        "workspace.forge",
        json!({"project": "app", "name": "login"}),
    )
    .unwrap()
}

fn repository<'a>(forge: &'a Value, path: &str) -> &'a Value {
    forge["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == path)
        .unwrap_or_else(|| panic!("no repository {path} in {forge}"))
}

/// The events of that name stored so far.
fn stored(user: &User, name: &str) -> Vec<Value> {
    let output = user
        .command(&["watch", "--after", "0"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // `watch` never ends by itself: what it printed of the stored events is read for a moment.
    let mut child = output;
    std::thread::sleep(Duration::from_millis(500));
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["name"] == name)
        .collect()
}

/// The calls of the fake `gh`, each as one line of its words.
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

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_repository_with_no_declared_forge_is_never_read_and_says_so_and_a_declared_one_is_read() {
    let user = User::new().with_fake_gh();
    fixture(&user);

    let forge = forge(&user);

    assert_eq!(forge["project"], "app");
    assert_eq!(forge["workspace"], "login");
    assert_eq!(forge["branch"], "app/login");
    assert_eq!(forge["problem"], Value::Null);
    let api = repository(&forge, "api");
    assert_eq!(api["forge"], "github");
    assert_eq!(api["repository"], "acme/api");
    let web = repository(&forge, "web");
    assert_eq!(web["forge"], "github");
    assert_eq!(web["repository"], "acme/web");
    let lib = repository(&forge, "lib");
    assert_eq!(lib["forge"], Value::Null, "{lib}");
    assert_eq!(lib["error"], Value::Null, "not an error: {lib}");
    assert_eq!(lib["note"], NO_FORGE);
    // The repository with none is never asked of `gh`.
    assert!(
        !gh_calls(&user).iter().any(|call| call.contains("acme/lib")),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_github_remote_is_not_read_until_its_forge_is_declared() {
    let user = User::new().with_fake_gh();
    let folder = fixture(&user);
    user.git(
        &folder.join("lib"),
        &["remote", "set-url", "origin", "git@github.com:acme/lib.git"],
    );
    let file = user.project_file("app");
    let before = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        before.replace(
            "forge = { kind = \"github\", repository = \"acme/api\" }\n",
            "",
        ),
    )
    .unwrap();

    let forge = forge(&user);

    // A remote on GitHub proposes, it never declares.
    assert_eq!(repository(&forge, "api")["note"], NO_FORGE);
    assert_eq!(repository(&forge, "lib")["note"], NO_FORGE);
    assert!(
        !gh_calls(&user)
            .iter()
            .any(|call| call.contains("acme/api") || call.contains("acme/lib")),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_declared_forge_whose_remote_leads_elsewhere_is_refused_and_never_read() {
    let user = User::with_fake_agent().with_fake_gh();
    let folder = fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));
    user.git(
        &folder.join("api"),
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:someone/api.git",
        ],
    );

    let forge = forge(&user);

    let api = repository(&forge, "api");
    assert_eq!(api["forge"], Value::Null, "{api}");
    let error = api["error"].as_str().unwrap();
    assert!(
        error.starts_with(
            "refused: api is declared on GitHub acme/api, and its remote origin: it leads to \
             github.com/someone/api, not to GitHub acme/api"
        ),
        "{error}"
    );
    assert!(
        !gh_calls(&user)
            .iter()
            .any(|call| call.contains("acme/api") || call.contains("someone/api")),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_branch_with_a_pull_request_shows_it_with_its_checks_and_one_without_says_no_pull_request() {
    let user = User::new().with_fake_gh();
    fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));

    let forge = forge(&user);

    assert_eq!(
        repository(&forge, "api")["pull_request"],
        json!({
            "number": 12,
            "title": "Add the login page",
            "state": "open",
            "head": "app/login",
            "base": "main",
            "mergeable": null,
            "url": "https://github.com/acme/api/pull/12",
            "review": "changes requested",
            "checks": [{"name": "build", "state": "passing"}, {"name": "lint", "state": "pending"}],
            "mark": "pending",
            "comments": 1
        })
    );
    let web = repository(&forge, "web");
    assert_eq!(web["pull_request"], Value::Null, "no pull request: {web}");
    assert_eq!(web["error"], Value::Null);
    // `-R` names the repository: the daemon never depends on its working folder.
    let calls = gh_calls(&user);
    assert!(
        calls.contains(
            &"pr list -R github.com/acme/api --head app/login --state all --limit 1 --json number"
                .to_owned()
        ),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("pr view 12 -R github.com/acme/api --json ")),
        "{calls:?}"
    );
}

#[test]
fn gh_missing_is_said_once_and_nothing_else_breaks() {
    // A PATH with git and nothing else: no `gh` at all, the machine's own included.
    let user = User::new();
    std::fs::create_dir(user.path("bin")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let path = user.path("bin");
    let user = user
        .with_env("PATH", path.to_str().unwrap())
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    fixture(&user);

    let first = forge(&user);
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    let second = forge(&user);

    let problem = first["problem"].as_str().expect("a problem is said");
    assert!(
        problem.contains("gh, GitHub's command line, is not on"),
        "{problem}"
    );
    assert!(problem.contains("gh auth login"), "what to do: {problem}");
    assert!(problem.contains("the rest of Flottille works"), "{problem}");
    assert!(
        second["read_at"].as_u64() > first["read_at"].as_u64(),
        "read again"
    );
    assert_eq!(second["problem"], first["problem"]);
    assert_eq!(
        repository(&first, "api")["error"],
        Value::Null,
        "said once, not per repository"
    );
    let said = stored(&user, "workspace.forge");
    assert_eq!(said.len(), 1, "said once: {said:?}");
    let log = std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap();
    assert_eq!(
        log.matches("is not on the daemon's PATH").count(),
        1,
        "{log}"
    );
    // The rest works: the workspace is shown, by the daemon and by the command line.
    assert_eq!(
        user.call("workspace.show", json!({"project": "app", "name": "login"}))
            .unwrap()["state"],
        "ready"
    );
    let output = user.run(&["workspace", "show", "app", "login"]);
    assert!(output.status.success());
    let shown = String::from_utf8_lossy(&output.stdout);
    assert!(
        shown.contains("Pull requests:\n  gh, GitHub's command line"),
        "{shown}"
    );
}

#[test]
fn gh_not_signed_in_is_said_and_nothing_else_is_asked_of_it() {
    let user = User::new().with_fake_gh();
    fixture(&user);
    user.gh_script(json!({"signed_in": false}));

    let forge = forge(&user);

    let problem = forge["problem"].as_str().expect("a problem is said");
    assert!(
        problem.contains("gh is not signed in to github.com"),
        "{problem}"
    );
    assert!(problem.contains("`gh auth login`"), "{problem}");
    assert_eq!(
        gh_calls(&user),
        ["auth status --hostname github.com"],
        "only its exit code is read"
    );
    assert_eq!(repository(&forge, "api")["pull_request"], Value::Null);
}

#[test]
fn the_state_changes_when_gh_answers_differently_and_an_event_says_so() {
    let user = User::new()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));

    // `watch --forge` watches the workspace: its first read says what is there.
    let watch = user.watch(&["--forge", "app", "login"]);
    let first = watch
        .until(|event| event["name"] == "workspace.forge")
        .pop()
        .unwrap();
    assert_eq!(
        repository(&first["data"], "api")["pull_request"]["mark"],
        "pending"
    );

    // The checks end: read again about a minute later, and said.
    let mut ended = pull(12, "Add the login page");
    ended["statusCheckRollup"][1]["status"] = "COMPLETED".into();
    ended["statusCheckRollup"][1]["conclusion"] = "FAILURE".into();
    one_pull_request(&user, ended);
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    let changed = watch
        .until(|event| event["name"] == "workspace.forge")
        .pop()
        .unwrap();
    let api = repository(&changed["data"], "api");
    assert_eq!(api["pull_request"]["mark"], "failing");
    assert_eq!(
        api["pull_request"]["checks"][1],
        json!({"name": "lint", "state": "failing"})
    );
    assert_eq!(changed["data"]["project"], "app");
    assert_eq!(changed["data"]["workspace"], "login");
    // The same answer again is no news.
    let calls = gh_calls(&user).len();
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    eventually("a third read", || gh_calls(&user).len() > calls);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(stored(&user, "workspace.forge").len(), 2);
}

#[test]
fn nobody_watching_means_no_read_and_a_failing_gh_is_read_less_and_less_often() {
    let user = User::new()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    fixture(&user);
    user.gh_script(json!({"fail": {"acme/api": "HTTP 502: Bad Gateway"}}));

    // Nobody watches: time passes, nothing is read.
    let read = forge(&user);
    assert_eq!(
        repository(&read, "api")["error"],
        "gh pr list failed: HTTP 502: Bad Gateway"
    );
    let calls = gh_calls(&user).len();
    user.call("test.advance", json!({"ms": 10 * 60_000}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        gh_calls(&user).len(),
        calls,
        "no timer runs when nobody looks"
    );

    // Watched while gh fails: the wait doubles at each failure, never a loop. The watch reads at
    // once, the state it had being old; the same failure again is no news.
    let reads = || {
        gh_calls(&user)
            .iter()
            .filter(|c| c.starts_with("auth status"))
            .count()
    };
    let before = reads();
    let watch = user.watch(&["--forge", "app", "login"]);
    eventually("the watch's own read", || reads() == before + 1);
    // Its second failure in a row: the next read waits 4 minutes.
    user.call("test.advance", json!({"ms": 2 * 60_000 + 1000}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        reads(),
        before + 1,
        "two minutes are not enough after two failures"
    );
    user.call("test.advance", json!({"ms": 2 * 60_000}))
        .unwrap();
    eventually("read four minutes after the failure", || {
        reads() == before + 2
    });

    // The watch ends: no more reads.
    drop(watch);
    std::thread::sleep(Duration::from_millis(300));
    let calls = gh_calls(&user).len();
    user.call("test.advance", json!({"ms": 60 * 60_000}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(gh_calls(&user).len(), calls);
}

#[test]
fn workspace_show_json_holds_the_pull_request() {
    let user = User::new().with_fake_gh();
    fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));

    let output = user.run(&["workspace", "show", "app", "login", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();
    let text = String::from_utf8_lossy(&user.run(&["workspace", "show", "app", "login"]).stdout)
        .into_owned();

    assert_eq!(shown["name"], "login");
    assert_eq!(
        repository(&shown["forge"], "api")["pull_request"]["number"],
        12
    );
    assert_eq!(
        repository(&shown["forge"], "api")["pull_request"]["mark"],
        "pending"
    );
    assert!(
        text.contains(
            "Pull requests:\n  api (acme/api): #12 open into main, checks pending, changes \
             requested, 1 comment: Add the login page\n    https://github.com/acme/api/pull/12\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("  web (acme/web): no pull request\n"),
        "{text}"
    );
    assert!(text.contains(&format!("  lib: {NO_FORGE}\n")), "{text}");
}

/// An agent started in the workspace (the project folder for none): its id and its MCP session.
fn agent_in(user: &User, workspace: Option<&str>) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": workspace, "message": "hello"}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

#[test]
fn forge_read_returns_its_workspace_s_pull_requests_and_refuses_another_repository() {
    let user = User::with_fake_agent().with_fake_gh();
    let folder = fixture(&user);
    // A repository of the project the workspace `narrow` does not take.
    // Offline: its remote is GitHub's now, and nothing is fetched from it.
    user.call(
        "workspace.create",
        json!({"project": "app", "name": "narrow", "repositories": ["web"], "offline": true}),
    )
    .unwrap();
    one_pull_request(&user, pull(12, "Add the login page"));
    let (_, mcp) = agent_in(&user, Some("login"));

    assert!(mcp.names().contains(&"forge".to_owned()));
    let answer = mcp.call("forge", json!({"action": "show"}));
    assert!(!answer.error, "{}", answer.text);
    let text = answer.text;
    for said in [
        "api (acme/api): pull request #12, open, app/login into main: Add the login page",
        "https://github.com/acme/api/pull/12",
        "review: changes requested",
        "checks: pending (build passing, lint pending)",
        "comments: 1, read them with forge {\"action\": \"show\", \"workspace\": \"login\", \"detail\": \"full\"}",
        "reviews: 1, read them with forge {\"action\": \"show\", \"workspace\": \"login\", \"detail\": \"full\"}",
        "web (acme/web): no pull request for app/login.",
        &format!("lib: {NO_FORGE}."),
    ] {
        assert!(text.contains(said), "{said} in {text}");
    }
    // The texts come only when asked for.
    assert!(!text.contains("Looks good to me"), "{text}");
    let full = mcp.call("forge", json!({"action": "show", "detail": "full"}));
    assert!(!full.error, "{}", full.text);
    for said in [
        "- alice: Looks good to me",
        "- bob, changes requested: Rename the form",
    ] {
        assert!(full.text.contains(said), "{said} in {}", full.text);
    }
    let refused = mcp.call("forge", json!({"action": "show", "detail": "all"}));
    assert!(
        refused.error && refused.text.contains("has no detail \"all\""),
        "{}",
        refused.text
    );

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));
    assert!(!answer.error, "{}", answer.text);
    assert!(!answer.text.contains("acme/web"), "{}", answer.text);

    // Outside its workspace: refused, and never asked of the forge.
    let calls = gh_calls(&user).len();
    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": "elsewhere"}),
    );
    assert!(answer.error);
    assert!(
        answer.text.starts_with(
            "refused: elsewhere is not a repository of this agent's workspace: it has"
        ),
        "{}",
        answer.text
    );
    assert_eq!(gh_calls(&user).len(), calls);
    let (_, narrow) = agent_in(&user, Some("narrow"));
    let answer = narrow.call("forge", json!({"action": "show", "repository": "api"}));
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("it has web"), "{}", answer.text);

    // Through the gate, allowed: nothing was held for the user.
    assert!(folder.is_dir());
    let listed = user.call("approval.list", json!({})).unwrap();
    assert!(listed["requests"].as_array().unwrap().is_empty());
}

#[test]
fn the_short_forge_read_of_a_named_repository_points_at_that_repository() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "comments: 1, read them with forge {\"action\": \"show\", \"workspace\": \"login\", \"repository\": \"api\", \"detail\": \"full\"}",
        "reviews: 1, read them with forge {\"action\": \"show\", \"workspace\": \"login\", \"repository\": \"api\", \"detail\": \"full\"}",
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
}

#[test]
fn forge_show_prints_mergeable_review_and_checks() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    let mut conflicting = pull(12, "Add the login page");
    conflicting["headRefName"] = json!("app/login");
    conflicting["mergeable"] = json!("CONFLICTING");
    let bare = json!({"number": 14, "title": "Add the login form", "state": "OPEN",
        "isDraft": false, "baseRefName": "main", "headRefName": "app/form",
        "url": "https://github.com/acme/web/pull/14", "mergeable": "MERGEABLE",
        "reviewDecision": "", "statusCheckRollup": [], "comments": [], "reviews": []});
    user.gh_script(json!({"pulls": {"acme/api": {"app/login": conflicting},
        "acme/web": {"app/form": bare}}}));
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "api (acme/api): pull request #12, open, app/login into main: Add the login page",
        "mergeable: no, it conflicts with main",
        "review: changes requested",
        "checks: pending (build passing, lint pending)",
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
    // One by its number with no review and no check says so, rather than nothing.
    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": "web", "number": 14}),
    );
    assert!(!answer.error, "{}", answer.text);
    for said in [
        "web (acme/web): pull request #14, open, app/form into main: Add the login form",
        "mergeable: yes, no conflict with main",
        "review: none",
        "checks: none ran on this pull request",
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
    let asked = gh_calls(&user)
        .into_iter()
        .find(|call| call.starts_with("pr view 14 "))
        .unwrap();
    assert!(
        asked.contains("headRefName") && asked.contains("mergeable"),
        "{asked}"
    );
}

#[test]
fn bitbucket_forge_show_prints_mergeable_review_and_checks() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    // Cloud says a conflict in the pull request's diffstat, Data Center on its merge's page.
    script["api"]["/repositories/acme/api/pullrequests/12/diffstat"] = json!({"values": [
        {"status": "modified", "new": {"path": "src/a.rs"}},
        {"status": "merge conflict", "new": {"path": "src/b.rs"}}
    ]});
    script["api"]["/rest/api/1.0/projects/APP/repos/web/pull-requests/7/merge"] =
        json!({"canMerge": false, "conflicted": false, "outcome": "CLEAN", "vetoes": []});
    user.bkt_script(script);
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show"}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "api (acme/api): pull request #12, open, app/login into main: Add the login page",
        "mergeable: no, it conflicts with main",
        "review: changes requested",
        "checks: pending (build passing, lint pending)",
        "web (APP/web): pull request #7, open, app/login into main: Add the login form",
        "mergeable: yes, no conflict with main",
        "checks: failing (Jenkins failing)",
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
    let calls = bkt_calls(&user);
    for call in [
        "api /repositories/acme/api/pullrequests/12/diffstat --context cloud --param pagelen=50",
        "api /rest/api/1.0/projects/APP/repos/web/pull-requests/7/merge --context work",
    ] {
        assert!(calls.contains(&call.to_owned()), "{call} in {calls:?}");
    }
}

#[test]
fn the_json_read_says_the_head_and_whether_the_pull_request_merges() {
    let user = User::new().with_fake_gh();
    fixture(&user);
    let mut conflicting = pull(12, "Add the login page");
    conflicting["mergeable"] = json!("CONFLICTING");
    let mut clean = pull(14, "Add the login form");
    clean["mergeable"] = json!("MERGEABLE");
    user.gh_script(json!({"pulls": {"acme/api": {"app/login": conflicting},
        "acme/web": {"app/login": clean}}}));

    let forge = forge(&user);

    let api = &repository(&forge, "api")["pull_request"];
    assert_eq!(
        (&api["head"], &api["mergeable"]),
        (&json!("app/login"), &json!(false))
    );
    let web = &repository(&forge, "web")["pull_request"];
    assert_eq!(
        (&web["head"], &web["mergeable"]),
        (&json!("app/login"), &json!(true))
    );
}

#[test]
fn a_cloud_diffstat_with_more_pages_leaves_mergeable_not_known_unless_a_conflict_shows() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    let more =
        "https://api.bitbucket.org/2.0/repositories/acme/api/pullrequests/12/diffstat?page=2";
    script["api"]["/repositories/acme/api/pullrequests/12/diffstat"] = json!({"values": [
        {"status": "modified", "new": {"path": "src/a.rs"}}
    ], "next": more});
    user.bkt_script(script.clone());
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("mergeable: not known yet"),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains("mergeable: yes"), "{}", answer.text);

    // A conflict on the page read is a conflict, whatever follows.
    script["api"]["/repositories/acme/api/pullrequests/12/diffstat"] = json!({"values": [
        {"status": "merge conflict", "new": {"path": "src/a.rs"}}
    ], "next": more});
    user.bkt_script(script);
    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));
    assert!(
        answer
            .text
            .contains("mergeable: no, it conflicts with main"),
        "{}",
        answer.text
    );
}

#[test]
fn a_root_agent_in_main_with_a_forge_lists_the_forge_tools() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    let (_, main) = agent_in(&user, None);
    let (_, workspace) = agent_in(&user, Some("login"));

    // The same tools wherever it works: the place does not decide, the project's declarations
    // do. The fixture declares a forge and no tracker, so `tickets` could only refuse.
    for tool in ["forge", "workspace"] {
        assert!(main.names().contains(&tool.to_owned()), "{tool}");
    }
    assert!(!main.names().contains(&"tickets".to_owned()));
    assert_eq!(main.names(), workspace.names());
}

#[test]
fn in_main_forge_read_names_the_workspace_it_reads_and_in_a_workspace_only_its_own() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));
    let (_, main) = agent_in(&user, None);
    let (_, inside) = agent_in(&user, Some("login"));

    let nameless = main.call("forge", json!({"action": "show"}));
    assert!(nameless.error, "{}", nameless.text);
    assert!(
        nameless.text.contains("name the workspace forge acts on")
            && nameless.text.contains("its workspaces are login"),
        "{}",
        nameless.text
    );
    let unknown = main.call("forge", json!({"action": "show", "workspace": "elsewhere"}));
    assert!(unknown.error && unknown.text.contains("has no workspace named elsewhere"));
    let named = main.call("forge", json!({"action": "show", "workspace": "login"}));
    assert!(!named.error, "{}", named.text);
    assert!(named.text.contains("Add the login page"), "{}", named.text);
    // In a workspace its own, named or not, and no other.
    let own = inside.call("forge", json!({"action": "show", "workspace": "login"}));
    assert_eq!(own.text, named.text);
    let other = inside.call("forge", json!({"action": "show", "workspace": "elsewhere"}));
    assert!(other.error && other.text.contains("acts on this agent's own workspace"));
}

#[test]
fn no_token_reaches_an_agent_and_the_daemon_never_asks_gh_for_one() {
    let token = "ghp_fakefakefakefakefakefakefake0075";
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_env("GH_TOKEN", token);
    fixture(&user);
    // A title holding the token: what `gh` prints is masked before it is shown or stored.
    one_pull_request(&user, pull(12, &format!("Leak {token}")));
    // The agent says what its own process holds of the token.
    user.fake_script(json!({"turns": [{"when": "hello", "do": [
        {"say": "token=<"}, {"say_variable": "GH_TOKEN"}, {"say": ">"}
    ]}]}));
    let (id, mcp) = agent_in(&user, Some("login"));

    let read = mcp.call("forge", json!({"action": "show"}));
    let state = forge(&user);
    let watch = stored(&user, "workspace.forge");

    assert!(!read.text.contains(token), "{}", read.text);
    assert!(read.text.contains("Leak [masked]"), "{}", read.text);
    assert!(!state.to_string().contains(token), "{state}");
    assert!(!format!("{watch:?}").contains(token));
    // The daemon's `gh` is the user's own, signed in their way; the agent's process has no token
    // (it would show masked).
    eventually("the agent's turn", || {
        user.call("agent.read", json!({"agent": id})).unwrap()["lines"]
            .to_string()
            .contains(">")
    });
    let said = user.call("agent.read", json!({"agent": id})).unwrap()["lines"].to_string();
    assert!(said.contains("token=<"), "{said}");
    assert!(
        !said.contains("[masked]") && !said.contains(token),
        "{said}"
    );
    assert!(
        user.gh_received()
            .iter()
            .all(|call| call["env"]["GH_TOKEN"] == token),
        "the daemon runs gh with the user's own environment"
    );
    // Never a login asked of `gh`.
    let calls = gh_calls(&user);
    assert!(!calls.is_empty());
    for call in &calls {
        assert!(!call.contains("auth token"), "{call}");
        assert!(!call.contains("--show-token"), "{call}");
        assert!(!call.split(' ').any(|word| word == "-t"), "{call}");
    }
}

// Bitbucket, through the user's `bkt` (`docs/forges.md`): `api` on Bitbucket Cloud, `web` on a Data
// Center host the project names, `lib` elsewhere.

fn bitbucket_fixture(user: &User) -> PathBuf {
    fixture_at(
        user,
        &[
            ("api", "git@bitbucket.org:acme/api.git"),
            ("web", "ssh://git@bitbucket.example.com:7999/APP/web.git"),
            ("lib", "https://git.example.com/acme/lib.git"),
        ],
    )
}

/// The contexts of the user's `bkt`: one on Cloud, one on the Data Center host.
fn contexts() -> Value {
    json!([
        {"name": "cloud", "host": "bitbucket.org", "workspace": "acme", "active": true},
        {"name": "work", "host": "bitbucket.example.com:7990", "project_key": "APP", "active": false}
    ])
}

/// What `bkt pr list --json` prints of a Cloud pull request of `acme/api`.
fn cloud_pull(id: u64, branch: &str, state: &str) -> Value {
    json!({
        "id": id,
        "title": "Add the login page",
        "state": state,
        "author": {"display_name": "Alice", "username": "alice"},
        "source": {"branch": {"name": branch}, "commit": {"hash": "abc"},
            "repository": {"slug": "", "full_name": "acme/api"}},
        "destination": {"branch": {"name": "main"}, "repository": {"slug": "", "full_name": "acme/api"}},
        "links": {"html": {"href": format!("https://bitbucket.org/acme/api/pull-requests/{id}")}},
        "summary": {"raw": ""}
    })
}

/// What `bkt pr list --json` prints of a Data Center pull request of `APP/web`.
fn dc_pull(id: u64, branch: &str, state: &str) -> Value {
    let repository = json!({"slug": "web", "project": {"key": "APP"}});
    json!({
        "id": id,
        "title": "Add the login form",
        "state": state,
        "version": 3,
        "author": {"user": {"name": "alice", "displayName": "Alice"}},
        "fromRef": {"id": format!("refs/heads/{branch}"), "displayId": branch,
            "latestCommit": "abc", "repository": repository},
        "toRef": {"id": "refs/heads/main", "displayId": "main", "latestCommit": "def",
            "repository": repository},
        "reviewers": [],
        "participants": [
            {"user": {"name": "alice", "displayName": "Alice"}, "role": "AUTHOR",
             "status": "UNAPPROVED", "approved": false},
            {"user": {"name": "bob", "displayName": "Bob"}, "role": "REVIEWER",
             "status": "APPROVED", "approved": true},
            {"user": {"name": "carol", "displayName": "Carol"}, "role": "REVIEWER",
             "status": "UNAPPROVED", "approved": false}
        ],
        "links": {"self": [{"href": format!("https://bitbucket.example.com/projects/APP/repos/web/pull-requests/{id}")}]}
    })
}

/// The fake `bkt`'s script: a Cloud pull request of `app/login` (and others that are not its
/// own) in `acme/api`, a Data Center one in `APP/web`.
fn bitbucket_pull_requests(user: &User) {
    user.bkt_script(json!({
        "contexts": contexts(),
        "pulls": {
            "acme/api": [
                cloud_pull(9, "app/login", "MERGED"),
                cloud_pull(12, "app/login", "OPEN"),
                cloud_pull(13, "app/other", "OPEN")
            ],
            "APP/web": [dc_pull(7, "app/login", "OPEN"), dc_pull(8, "app/other", "OPEN")]
        },
        "checks": {
            "acme/api": {"12": [
                {"state": "SUCCESSFUL", "key": "build", "name": "build", "url": "https://x"},
                {"state": "INPROGRESS", "key": "lint", "name": "lint", "url": "https://x"}
            ]},
            "APP/web": {"7": [{"state": "FAILED", "key": "ci", "name": "Jenkins", "url": "https://x"}]}
        },
        "api": {
            "/repositories/acme/api/pullrequests/12": {"participants": [
                {"role": "REVIEWER", "approved": false, "state": "changes_requested",
                 "user": {"display_name": "Bob"}},
                {"role": "PARTICIPANT", "approved": false, "state": null,
                 "user": {"display_name": "Carol"}}
            ]},
            "/repositories/acme/api/pullrequests/12/comments": {"values": [
                {"content": {"raw": "Looks good to me"}, "user": {"display_name": "Alice"}, "deleted": false},
                {"content": {"raw": "Never mind"}, "user": {"display_name": "Bob"}, "deleted": true}
            ]},
            "/rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities": {"values": [
                {"action": "COMMENTED", "comment": {"text": "Rename the form",
                    "author": {"displayName": "Carol"},
                    "comments": [{"text": "Done", "author": {"displayName": "Alice"}, "comments": []}]}},
                {"action": "APPROVED"}
            ]}
        }
    }));
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

#[test]
fn a_declared_bitbucket_cloud_and_data_center_repository_is_read_and_an_old_forges_table_is_not() {
    let user = User::new()
        .with_fake_gh()
        .with_fake_bkt()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);

    let declared = forge(&user);
    // The table that named Data Center hosts before, in place of `web`'s declaration.
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    let line = "forge = { kind = \"bitbucket-data-center\", host = \"bitbucket.example.com\", \
        repository = \"APP/web\" }\n";
    assert!(text.contains(line), "{text}");
    let text = text.replace(line, "") + "\n[forges]\nbitbucket = [\"bitbucket.example.com\"]\n";
    std::fs::write(&file, text).unwrap();
    let shown = user.call("project.show", json!({"name": "app"})).unwrap();
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    let old = forge(&user);

    let api = repository(&declared, "api");
    assert_eq!(api["forge"], "bitbucket");
    assert_eq!(api["repository"], "acme/api");
    let web = repository(&declared, "web");
    assert_eq!(web["forge"], "bitbucket", "{web}");
    assert_eq!(web["repository"], "APP/web");
    let lib = repository(&declared, "lib");
    assert_eq!(lib["forge"], Value::Null);
    assert_eq!(lib["note"], NO_FORGE);
    // Neither is ever asked of `gh`.
    assert!(gh_calls(&user).is_empty(), "{:?}", gh_calls(&user));
    // An old `[forges]` table is read as nothing set, and said.
    assert!(
        shown["obsolete"][0]
            .as_str()
            .unwrap()
            .starts_with("forges: no longer read"),
        "{shown}"
    );
    let web = repository(&old, "web");
    assert_eq!(web["forge"], Value::Null, "{web}");
    assert_eq!(
        web["note"],
        format!("{NO_FORGE} (the project's [forges] table is no longer read)")
    );
}

#[test]
fn a_cloud_branch_shows_its_newest_own_pull_request_with_reviewers_checks_and_comments() {
    let user = User::new().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);

    let forge = forge(&user);

    assert_eq!(forge["problem"], Value::Null, "{forge}");
    let api = repository(&forge, "api");
    assert_eq!(api["error"], Value::Null, "{api}");
    assert_eq!(
        api["pull_request"],
        json!({
            "number": 12,
            "title": "Add the login page",
            "state": "open",
            "head": "app/login",
            "base": "main",
            "mergeable": null,
            "url": "https://bitbucket.org/acme/api/pull-requests/12",
            "review": "changes requested",
            "checks": [{"name": "build", "state": "passing"}, {"name": "lint", "state": "pending"}],
            "mark": "pending",
            "comments": 1,
            "reviewers": [{"name": "Bob", "state": "changes requested"}]
        })
    );
    // `--context` names the host and the repository is named by flags: the daemon depends on
    // neither its working folder nor the user's active context.
    let calls = bkt_calls(&user);
    for call in [
        "context list --json",
        "pr list --context cloud --workspace acme --repo api --state all --limit 50 --json",
        "pr checks 12 --context cloud --workspace acme --repo api --json",
        "api /repositories/acme/api/pullrequests/12 --context cloud",
        "api /repositories/acme/api/pullrequests/12/comments --context cloud --param pagelen=50",
    ] {
        assert!(calls.contains(&call.to_owned()), "{call} in {calls:?}");
    }
}

#[test]
fn a_data_center_branch_shows_its_pull_request_with_reviewers_checks_and_comments() {
    let user = User::new().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);

    let forge = forge(&user);

    let web = repository(&forge, "web");
    assert_eq!(web["error"], Value::Null, "{web}");
    assert_eq!(
        web["pull_request"],
        json!({
            "number": 7,
            "title": "Add the login form",
            "state": "open",
            "head": "app/login",
            "base": "main",
            "mergeable": null,
            "url": "https://bitbucket.example.com/projects/APP/repos/web/pull-requests/7",
            "review": "review required",
            "checks": [{"name": "Jenkins", "state": "failing"}],
            "mark": "failing",
            "comments": 2,
            "reviewers": [
                {"name": "Bob", "state": "approved"},
                {"name": "Carol", "state": "pending"}
            ]
        })
    );
    let calls = bkt_calls(&user);
    for call in [
        "pr list --context work --project APP --repo web --state all --limit 50 --json",
        "pr checks 7 --context work --project APP --repo web --json",
        "api /rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities --context work --param limit=50",
    ] {
        assert!(calls.contains(&call.to_owned()), "{call} in {calls:?}");
    }
    // One list of contexts serves every repository of the read.
    assert_eq!(
        calls.iter().filter(|c| *c == "context list --json").count(),
        1,
        "{calls:?}"
    );
}

#[test]
fn a_branch_with_no_pull_request_or_only_closed_and_declined_ones_is_said_as_such() {
    let user = User::new().with_fake_bkt();
    bitbucket_fixture(&user);
    user.bkt_script(json!({
        "contexts": contexts(),
        "pulls": {"APP/web": [dc_pull(7, "app/login", "DECLINED")]},
        "api": {"/rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities": {"values": []}}
    }));

    let forge = forge(&user);

    assert_eq!(repository(&forge, "api")["pull_request"], Value::Null);
    assert_eq!(repository(&forge, "api")["error"], Value::Null);
    let web = repository(&forge, "web");
    assert_eq!(web["pull_request"]["state"], "closed", "{web}");
    assert_eq!(web["pull_request"]["review"], "review required");
}

#[test]
fn bkt_missing_is_said_once_for_bitbucket_and_github_is_still_read() {
    // A PATH with git and the fake `gh` and no `bkt` at all, the machine's own included.
    let user = User::new().with_fake_gh();
    std::fs::create_dir(user.path("bin")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let path = format!(
        "{}:{}",
        user.path("gh-bin").display(),
        user.path("bin").display()
    );
    let user = user.with_env("PATH", &path);
    fixture_at(
        &user,
        &[
            ("api", "git@github.com:acme/api.git"),
            ("web", "git@bitbucket.org:acme/web.git"),
            ("lib", "git@bitbucket.org:acme/lib.git"),
        ],
    );
    one_pull_request(&user, pull(12, "Add the login page"));

    let forge = forge(&user);

    let problem = forge["problem"].as_str().expect("a problem is said");
    assert!(
        problem.contains("bkt, Bitbucket's command line, is not on"),
        "{problem}"
    );
    assert!(problem.contains("bkt auth login"), "what to do: {problem}");
    assert_eq!(problem.matches("bkt, Bitbucket's").count(), 1, "{problem}");
    assert_eq!(
        repository(&forge, "api")["pull_request"]["number"],
        12,
        "GitHub is read all the same"
    );
    assert_eq!(repository(&forge, "web")["error"], Value::Null);
}

#[test]
fn a_host_bkt_has_no_context_for_is_said_with_what_to_do_and_nothing_else_is_asked() {
    let user = User::new().with_fake_bkt();
    bitbucket_fixture(&user);
    // Only the Cloud host is known to `bkt`.
    user.bkt_script(
        json!({"contexts": [{"name": "cloud", "host": "bitbucket.org"}],
        "pulls": {"acme/api": [cloud_pull(12, "app/login", "OPEN")]},
        "api": {"/repositories/acme/api/pullrequests/12": {"participants": []},
            "/repositories/acme/api/pullrequests/12/comments": {"values": []}}}),
    );

    let forge = forge(&user);

    let problem = forge["problem"].as_str().expect("a problem is said");
    assert!(
        problem.contains("bkt has no context for bitbucket.example.com"),
        "{problem}"
    );
    assert!(problem.contains("`bkt auth login"), "{problem}");
    assert_eq!(
        repository(&forge, "api")["pull_request"]["number"],
        12,
        "the host bkt knows is read"
    );
    assert!(
        !bkt_calls(&user).iter().any(|call| call.contains("APP")),
        "{:?}",
        bkt_calls(&user)
    );
}

#[test]
fn a_bkt_call_that_fails_is_the_repository_s_error_and_never_hides_the_others() {
    let user = User::new().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    script["fail"] = json!({"acme/api": "Error: 502 Bad Gateway"});
    user.bkt_script(script);

    let forge = forge(&user);

    let api = repository(&forge, "api");
    assert!(
        api["error"].as_str().unwrap().contains("502 Bad Gateway"),
        "{api}"
    );
    assert_eq!(repository(&forge, "web")["pull_request"]["number"], 7);
}

#[test]
fn forge_read_returns_the_bitbucket_pull_requests_of_the_workspace() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "detail": "full"}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "api (acme/api): pull request #12, open, app/login into main: Add the login page",
        "https://bitbucket.org/acme/api/pull-requests/12",
        "review: changes requested",
        "reviewers: Bob changes requested",
        "checks: pending (build passing, lint pending)",
        "- Alice: Looks good to me",
        "web (APP/web): pull request #7, open, app/login into main: Add the login form",
        "review: review required",
        "reviewers: Bob approved, Carol pending",
        "checks: failing (Jenkins failing)",
        "- Carol: Rename the form",
        "- Alice: Done",
        &format!("lib: {NO_FORGE}."),
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
    assert!(!answer.text.contains("Never mind"), "a deleted comment");
    // Through the gate, allowed: nothing was held for the user.
    let listed = user.call("approval.list", json!({})).unwrap();
    assert!(listed["requests"].as_array().unwrap().is_empty());
}

#[test]
fn workspace_show_prints_the_bitbucket_pull_request_as_it_does_a_github_one() {
    let user = User::new().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);

    let output = user.run(&["workspace", "show", "app", "login"]);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(output.status.success(), "{text}");
    assert!(
        text.contains(
            "Pull requests:\n  api (acme/api): #12 open into main, checks pending, changes \
             requested, 1 comment: Add the login page\n    https://bitbucket.org/acme/api/pull-requests/12\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("  web (APP/web): #7 open into main, checks failing, review required, 2 comments: Add the login form\n"),
        "{text}"
    );
}

#[test]
fn no_token_reaches_an_agent_and_the_daemon_never_asks_bkt_for_one() {
    let token = "bkt_fakefakefakefakefakefake0078";
    let user = User::with_fake_agent()
        .with_fake_bkt()
        .with_env("BKT_TOKEN", token)
        .with_env("BKT_CONFIG_DIR", "/the/users/own/bkt");
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    // A title holding the token: what `bkt` prints is masked before it is shown or stored.
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    script["pulls"]["acme/api"][1]["title"] = json!(format!("Leak {token}"));
    user.bkt_script(script);
    user.fake_script(json!({"turns": [{"when": "hello", "do": [
        {"say": "token=<"}, {"say_variable": "BKT_TOKEN"}, {"say": ">"},
        {"say": "dir=<"}, {"say_variable": "BKT_CONFIG_DIR"}, {"say": ">"}
    ]}]}));
    let (id, mcp) = agent_in(&user, Some("login"));

    let read = mcp.call("forge", json!({"action": "show"}));
    let state = forge(&user);

    assert!(!read.text.contains(token), "{}", read.text);
    assert!(read.text.contains("Leak [masked]"), "{}", read.text);
    assert!(!state.to_string().contains(token), "{state}");
    // The daemon's `bkt` is the user's own, signed in their way.
    assert!(
        user.bkt_received()
            .iter()
            .all(|call| call["env"]["BKT_TOKEN"] == token
                && call["env"]["BKT_CONFIG_DIR"] == "/the/users/own/bkt"),
        "the daemon runs bkt with the user's own environment"
    );
    // The agent's process holds neither the token nor the user's configuration folder.
    eventually("the agent's turn", || {
        user.call("agent.read", json!({"agent": id})).unwrap()["lines"]
            .to_string()
            .contains("dir=<")
    });
    let said = user.call("agent.read", json!({"agent": id})).unwrap()["lines"].to_string();
    assert!(said.contains("token=<"), "{said}");
    assert!(!said.contains(token), "{said}");
    assert!(!said.contains("/the/users/own/bkt"), "{said}");
    // Never a token asked of `bkt`: of its sign-in, the daemon reads the contexts and no more.
    let calls = bkt_calls(&user);
    assert!(!calls.is_empty());
    for call in &calls {
        assert!(!call.starts_with("auth"), "{call}");
        assert!(!call.contains("token"), "{call}");
    }
}

#[test]
fn gh_host_in_the_daemon_s_environment_never_moves_a_github_call() {
    let user = User::new()
        .with_fake_gh()
        .with_env("GH_HOST", "ghe.example.com");
    fixture(&user);
    user.gh_script(json!({"pulls": {
        "acme/api": {"app/login": pull(12, "Add the login page")},
        "ghe.example.com/acme/api": {"app/login": pull(99, "Elsewhere")}
    }}));

    let forge = forge(&user);

    let api = repository(&forge, "api");
    assert_eq!(api["pull_request"]["number"], 12, "{api}");
    assert_eq!(api["repository"], "acme/api");
    let calls = gh_calls(&user);
    assert!(
        calls.iter().any(|call| call.starts_with("pr ")),
        "{calls:?}"
    );
    for call in calls.iter().filter(|call| !call.starts_with("auth status")) {
        assert!(call.contains(" -R github.com/acme/"), "{call}");
    }
}

#[test]
fn a_push_url_or_a_second_url_elsewhere_is_refused_with_its_reason() {
    let user = User::new().with_fake_gh();
    let folder = fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));
    user.git(
        &folder.join("api"),
        &[
            "config",
            "remote.origin.pushurl",
            "git@github.com:someone/api.git",
        ],
    );
    user.git(
        &folder.join("web"),
        &[
            "remote",
            "set-url",
            "--add",
            "origin",
            "https://github.com/other/web.git",
        ],
    );

    let forge = forge(&user);

    for (path, error) in [
        (
            "api",
            "refused: api is declared on GitHub acme/api, and its remote origin: it leads to \
             github.com/someone/api, not to GitHub acme/api; nothing was asked of the forge",
        ),
        (
            "web",
            "refused: web is declared on GitHub acme/web, and its remote origin: it leads to \
             github.com/other/web, not to GitHub acme/web; nothing was asked of the forge",
        ),
    ] {
        let found = repository(&forge, path);
        assert_eq!(found["forge"], Value::Null, "{found}");
        let said = found["error"].as_str().unwrap_or_default();
        assert!(said.starts_with(error), "{said}");
    }
    assert!(
        !gh_calls(&user).iter().any(|call| call.starts_with("pr ")),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_refusal_never_shows_a_remote_url_unmasked() {
    let user = User::new()
        .with_fake_gh()
        .with_env("ACME_TOKEN", "leakedvalue123");
    let folder = fixture(&user);
    // No host: the address is not on a forge, and is said without its credentials.
    user.git(
        &folder.join("api"),
        &[
            "remote",
            "set-url",
            "origin",
            "https://alice:hunter2secret@/acme/api.git",
        ],
    );
    // A host, and a secret of the daemon's environment in its path.
    user.git(
        &folder.join("web"),
        &[
            "remote",
            "set-url",
            "origin",
            "https://bob:pa55word99@github.com/leakedvalue123/web.git",
        ],
    );
    let secrets = [
        "alice",
        "hunter2secret",
        "bob",
        "pa55word99",
        "leakedvalue123",
    ];
    let clean = |text: &str| {
        for secret in secrets {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    };

    let forge = forge(&user);
    let settings = user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    let (code, saving) = user
        .call(
            "project.set_settings",
            json!({"name": "app", "repositories": [
                {"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}
            ]}),
        )
        .unwrap_err();

    for path in ["api", "web"] {
        let error = repository(&forge, path)["error"].as_str().unwrap();
        assert!(
            error.starts_with(&format!("refused: {path} is declared")),
            "{error}"
        );
        clean(error);
    }
    let api = repository(&forge, "api")["error"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        api.contains("it leads to https:///acme/api.git, which is not on a forge"),
        "{api}"
    );
    clean(&settings.to_string());
    assert!(
        settings.to_string().contains("which is not on a forge"),
        "{settings}"
    );
    assert_eq!(code, "refused");
    assert!(saving.starts_with("api cannot be declared"), "{saving}");
    clean(&saving);
}

#[test]
fn an_ssh_alias_is_resolved_by_ssh_and_its_real_host_checked() {
    let user = User::new()
        .with_fake_gh()
        .with_fake_ssh(&[("gh-work", "github.com"), ("gh-other", "gitlab.com")]);
    let folder = fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));
    user.git(
        &folder.join("api"),
        &["remote", "set-url", "origin", "git@gh-work:acme/api.git"],
    );
    user.git(
        &folder.join("web"),
        &["remote", "set-url", "origin", "git@gh-other:acme/web.git"],
    );
    user.git(
        &folder.join("lib"),
        &["remote", "set-url", "origin", "git@gh-unknown:acme/lib.git"],
    );
    let declare = |path: &str, repository: &str| {
        user.call(
            "project.set_settings",
            json!({"name": "app", "repositories": [
                {"path": path, "forge": {"kind": "github", "repository": repository}}
            ]}),
        )
    };

    let saved = declare("api", "acme/api");
    let (_, other) = declare("web", "acme/web").unwrap_err();
    let (_, unknown) = declare("lib", "acme/lib").unwrap_err();
    let forge = forge(&user);

    saved.unwrap();
    assert_eq!(
        other,
        "web cannot be declared on GitHub acme/web: its remote origin: it leads to \
         gitlab.com/acme/web, not to GitHub acme/web (gh-other is an SSH alias of gitlab.com); \
         nothing was saved"
    );
    assert!(
        unknown.starts_with(
            "lib cannot be declared on GitHub acme/lib: its remote origin: it leads to \
             gh-unknown/acme/lib, not to GitHub acme/lib (ssh -G gh-unknown did not say its \
             host: "
        ),
        "{unknown}"
    );
    let api = repository(&forge, "api");
    assert_eq!(api["error"], Value::Null, "{api}");
    assert_eq!(api["pull_request"]["number"], 12, "{api}");
    // Of `ssh`, only `-G` is asked, and only for a host that is not the declared one.
    let calls = user.ssh_calls();
    assert!(calls.contains(&"-G gh-work".to_owned()), "{calls:?}");
    assert!(
        calls.iter().all(|call| call.starts_with("-G gh-")),
        "{calls:?}"
    );
}

/// What `gh pr view --json` answers for a pull request of `acme/api` whose checks are `build`
/// (passing) and the failing `jobs`, each `(name, job id)` an Actions job of the repository.
fn pull_with_failed_jobs(jobs: &[(&str, u64)]) -> Value {
    let mut checks = vec![
        json!({"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
        "conclusion": "SUCCESS", "detailsUrl": "https://github.com/acme/api/actions/runs/900/job/1"}),
    ];
    for (name, job) in jobs {
        checks.push(
            json!({"__typename": "CheckRun", "name": name, "status": "COMPLETED",
            "conclusion": "FAILURE",
            "detailsUrl": format!("https://github.com/acme/api/actions/runs/900/job/{job}")}),
        );
    }
    let mut pull = pull(12, "Add the login page");
    pull["statusCheckRollup"] = checks.into();
    pull
}

/// A job's log as `gh run view --job <id> --log-failed` prints it: the job, the step and a
/// timestamp before each line.
fn gh_log(job: &str, lines: &[String]) -> String {
    lines
        .iter()
        .enumerate()
        .map(|(at, line)| {
            format!(
                "{job}\tRun cargo test\t2026-01-01T00:00:{:02}.1234567Z {line}\n",
                at % 60
            )
        })
        .collect()
}

fn script_logs(user: &User, pull: Value, logs: Value) {
    user.gh_script(json!({"pulls": {"acme/api": {"app/login": pull}}, "logs": {"acme/api": logs}}));
}

#[test]
fn a_failed_job_s_last_lines_are_returned() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    let lines: Vec<String> = [
        "\u{1b}[36;1mcargo test\u{1b}[0m",
        "test login::form ... \u{1b}[31mFAILED\u{1b}[0m",
        "thread 'login::form' panicked at src/login.rs:42:9:",
        "assertion `left == right` failed",
        "error: test failed, to rerun pass `--lib`",
    ]
    .map(String::from)
    .to_vec();
    script_logs(
        &user,
        pull_with_failed_jobs(&[("test", 456)]),
        json!({"456": gh_log("test", &lines)}),
    );
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "failed jobs (1):",
        "- test: the last 5 of 5 lines of its log:",
        "    test login::form ... FAILED",
        "    thread 'login::form' panicked at src/login.rs:42:9:",
        "    error: test failed, to rerun pass `--lib`",
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
    // Colours, timestamps and the job's and step's names before each line are noise.
    for noise in ["\u{1b}", "2026-01-01T", "Run cargo test"] {
        assert!(!answer.text.contains(noise), "{noise:?} in {}", answer.text);
    }
    // Only the failed job's log is asked, for the job the check's link names, in the repository.
    let calls = gh_calls(&user);
    let logs: Vec<&String> = calls.iter().filter(|c| c.starts_with("run view")).collect();
    assert_eq!(
        logs,
        ["run view --job 456 --log-failed -R github.com/acme/api"],
        "{calls:?}"
    );
    // The same by its number.
    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": "api", "number": 12}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("    error: test failed, to rerun pass `--lib`"),
        "{}",
        answer.text
    );
}

#[test]
fn the_length_of_a_failed_job_s_log_is_bounded() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    let many: Vec<String> = (1..=200).map(|n| format!("line {n:03}")).collect();
    // Ten lines of 1,000 bytes: the bytes bound cuts before the lines' does.
    let wide: Vec<String> = (1..=10)
        .map(|n| format!("{n:02}{}", "w".repeat(998)))
        .collect();
    // Three jobs filling their bytes, then more than the total allows.
    let full: Vec<String> = (1..=30)
        .map(|n| format!("{n:02} {}", "f".repeat(117)))
        .collect();
    script_logs(
        &user,
        pull_with_failed_jobs(&[
            ("many", 10),
            ("wide", 11),
            ("full-a", 12),
            ("full-b", 13),
            ("full-c", 14),
            ("full-d", 15),
            ("full-e", 16),
        ]),
        json!({"10": gh_log("many", &many), "11": gh_log("wide", &wide),
            "12": gh_log("a", &full), "13": gh_log("b", &full), "14": gh_log("c", &full),
            "15": gh_log("d", &full), "16": gh_log("e", &full)}),
    );
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    let text = answer.text;
    // The last lines only, and how many there were.
    assert!(
        text.contains("- many: the last 40 of 200 lines of its log:"),
        "{text}"
    );
    assert!(text.contains("    line 200\n"), "{text}");
    assert!(text.contains("    line 161\n"), "{text}");
    assert!(!text.contains("line 160"), "{text}");
    // A job's bytes are bounded too: the oldest lines go first, whole.
    let wide_log = text
        .split("- wide:")
        .nth(1)
        .and_then(|rest| rest.split("\n  - ").next())
        .unwrap();
    assert!(
        wide_log.contains("the last 3 of 10 lines of its log"),
        "{wide_log}"
    );
    assert!(wide_log.contains("10www"), "{wide_log}");
    assert!(!wide_log.contains("06www"), "{wide_log}");
    assert!(wide_log.len() <= 4000 + 200, "{} bytes", wide_log.len());
    // The whole is bounded, and what it leaves out is said, never silently dropped.
    let jobs = text.split("failed jobs").nth(1).unwrap();
    assert!(jobs.len() <= 12_000 + 1_500, "{} bytes", jobs.len());
    for left_out in ["full-d", "full-e"] {
        let said = format!("- {left_out}: no log: at most 5 logs and 12000 bytes are given");
        assert!(text.contains(&said), "{said} in {text}");
    }
    // At most five logs are asked of `gh`; the others are named.
    let asked = gh_calls(&user)
        .into_iter()
        .filter(|call| call.starts_with("run view"))
        .count();
    assert_eq!(asked, 5, "{text}");
}

#[test]
fn a_secret_in_a_failed_job_s_log_is_masked() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    let token = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let lines: Vec<String> = [
        format!("curl -H 'Authorization: token {token}'"),
        "error: 401".into(),
    ]
    .to_vec();
    script_logs(
        &user,
        pull_with_failed_jobs(&[("deploy", 456)]),
        json!({"456": gh_log("deploy", &lines)}),
    );
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("error: 401"), "{}", answer.text);
    assert!(!answer.text.contains(token), "{}", answer.text);
    assert!(answer.text.contains("[masked]"), "{}", answer.text);
}

#[test]
fn a_check_that_has_no_log_to_read_says_why() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    let mut pull = pull_with_failed_jobs(&[("gone", 77)]);
    pull["statusCheckRollup"].as_array_mut().unwrap().extend([
        // A commit status of another CI, and one with no link at all.
        json!({"__typename": "StatusContext", "context": "ci/outside", "state": "FAILURE",
            "targetUrl": "https://ci.example.com/builds/5"}),
        json!({"__typename": "StatusContext", "context": "ci/bare", "state": "ERROR"}),
    ]);
    // The log of job 77 is gone: GitHub keeps them for a while only.
    script_logs(&user, pull, json!({}));
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "failed jobs (3):",
        "- gone: no log: gh run view failed: failed to get run log: log not found",
        "- ci/outside: no log: it is not a GitHub Actions job, see https://ci.example.com/builds/5",
        "- ci/bare: no log: it is not a GitHub Actions job",
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
}

#[test]
fn a_pull_request_with_no_failed_job_asks_for_no_log() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    one_pull_request(&user, pull(12, "Add the login page"));
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(!answer.text.contains("failed jobs"), "{}", answer.text);
    assert!(
        gh_calls(&user)
            .iter()
            .all(|call| !call.starts_with("run view")),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_forge_without_logs_says_so() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call("forge", json!({"action": "show", "repository": "web"}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "checks: failing (Jenkins failing)",
        "failed jobs (1):",
        "- Jenkins: no log: bkt gives no job logs, read it on Bitbucket",
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
    // Nothing was asked of Bitbucket for it.
    assert!(
        bkt_calls(&user).iter().all(|call| !call.contains("log")),
        "{:?}",
        bkt_calls(&user)
    );
}

// Page sizes: Bitbucket Cloud refuses a `pagelen` above its endpoint's maximum (`400 Bad Request:
// Invalid pagelen`), and the fake `bkt` does the same above 50 (#209).

/// Adds the body `bkt api <path>` answers to the fake `bkt`'s script.
fn bkt_api(user: &User, path: &str, body: Value) {
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    script["api"][path] = body;
    user.bkt_script(script);
}

/// Every page size the daemon asked of `bkt`: `--limit`, `pagelen=` and `limit=`.
fn page_sizes(user: &User) -> Vec<usize> {
    bkt_calls(user)
        .iter()
        .flat_map(|call| call.split(' ').map(str::to_owned).collect::<Vec<_>>())
        .filter_map(|word| {
            word.strip_prefix("pagelen=")
                .or_else(|| word.strip_prefix("limit="))
                .and_then(|n| n.parse().ok())
        })
        .chain(bkt_calls(user).iter().filter_map(|call| {
            let words: Vec<&str> = call.split(' ').collect();
            let at = words.iter().position(|w| *w == "--limit")?;
            words.get(at + 1)?.parse().ok()
        }))
        .collect()
}

fn cloud_comments(count: usize) -> Value {
    let values: Vec<Value> = (1..=count)
        .map(|n| {
            json!({"content": {"raw": format!("Comment {n}")}, "user": {"display_name": "Alice"},
                "deleted": false})
        })
        .collect();
    json!({"values": values})
}

/// The pull request `workspace.forge` reads, the minute its cache holds having passed.
fn the_pull_request_of(user: &User, name: &str) -> Value {
    user.call("test.advance", json!({"ms": 61_000})).unwrap();
    repository(&forge(user), name)["pull_request"].clone()
}

#[test]
fn bitbucket_forge_list_answers_the_pull_requests_within_the_page_maximum() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let (_, mcp) = agent_in(&user, None);

    let answer = mcp.call("forge", json!({"action": "list"}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "- #12 open, app/login into main: Add the login page",
        "- #13 open, app/other into main: Add the login page",
        "- #7 open, app/login into main: Add the login form",
    ] {
        assert!(answer.text.contains(said), "{said} in {}", answer.text);
    }
    assert!(!answer.text.contains("Invalid pagelen"), "{}", answer.text);
    assert!(
        !answer.text.contains("Only the newest"),
        "a short list is not cut: {}",
        answer.text
    );
    assert!(
        page_sizes(&user).iter().all(|n| *n <= 50),
        "{:?}",
        page_sizes(&user)
    );
}

#[test]
fn bitbucket_forge_list_says_when_the_repository_holds_more_than_the_newest_it_reads() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    script["pulls"]["acme/api"] = (1..=50)
        .map(|id| cloud_pull(id, "app/login", "OPEN"))
        .collect();
    user.bkt_script(script);
    let (_, mcp) = agent_in(&user, None);

    let answer = mcp.call("forge", json!({"action": "list"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "Only the newest 50 pull requests are read; older ones, if any, are not listed."
        ),
        "{}",
        answer.text
    );
    // Data Center holds two: its list is not cut.
    assert_eq!(
        answer.text.matches("Only the newest").count(),
        1,
        "{}",
        answer.text
    );
}

#[test]
fn bitbucket_forge_show_of_a_pull_request_by_its_number_answers_it() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let (_, mcp) = agent_in(&user, None);

    let cloud = mcp.call(
        "forge",
        json!({"action": "show", "repository": "api", "number": 12}),
    );
    let data_center = mcp.call(
        "forge",
        json!({"action": "show", "repository": "web", "number": 7}),
    );

    assert!(!cloud.error, "{}", cloud.text);
    assert!(
        cloud
            .text
            .contains("pull request #12, open, app/login into main"),
        "{}",
        cloud.text
    );
    assert!(!data_center.error, "{}", data_center.text);
    assert!(
        data_center.text.contains("pull request #7, open"),
        "{}",
        data_center.text
    );
    assert!(
        page_sizes(&user).iter().all(|n| *n <= 50),
        "{:?}",
        page_sizes(&user)
    );
}

#[test]
fn bitbucket_forge_subscribe_to_a_pull_request_is_accepted() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call(
        "forge",
        json!({"action": "subscribe", "repository": "api", "events": ["checks"]}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.to_lowercase().contains("subscribed"),
        "{}",
        answer.text
    );
    assert!(
        page_sizes(&user).iter().all(|n| *n <= 50),
        "{:?}",
        page_sizes(&user)
    );
}

#[test]
fn bitbucket_forge_show_of_a_pull_request_with_no_build_status_answers_it_with_no_check() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    // The pull request 13 has no entry in `checks`: its commit has no status.
    script["api"]["/repositories/acme/api/pullrequests/13"] = json!({"participants": []});
    script["api"]["/repositories/acme/api/pullrequests/13/comments"] = json!({"values": []});
    user.bkt_script(script);
    let (_, mcp) = agent_in(&user, None);

    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": "api", "number": 13}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("pull request #13, open"),
        "{}",
        answer.text
    );
    assert!(
        answer
            .text
            .contains("checks: none ran on this pull request"),
        "{}",
        answer.text
    );
}

#[test]
fn bitbucket_forge_subscribe_to_a_pull_request_with_no_build_status_is_accepted() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    // The pull request 12 of `login` has no status either: its commit answers `null`.
    script["checks"]["acme/api"] = json!({});
    user.bkt_script(script);
    let (_, mcp) = agent_in(&user, Some("login"));

    let answer = mcp.call(
        "forge",
        json!({"action": "subscribe", "repository": "api", "events": ["checks"]}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.to_lowercase().contains("subscribed"),
        "{}",
        answer.text
    );
}

#[test]
fn bitbucket_forge_show_of_a_pull_request_whose_checks_cannot_be_read_is_refused() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    // `bkt` answers without its `statuses` key: that is not "no check ran".
    script["checks"]["acme/api"]["12"] = json!("unreadable");
    user.bkt_script(script);
    let (_, mcp) = agent_in(&user, None);

    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": "api", "number": 12}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("the checks of the pull request #12 could not be read"),
        "{}",
        answer.text
    );
}

#[test]
fn a_bitbucket_pull_request_with_more_comments_than_one_page_shows_them_all_up_to_the_total() {
    let user = User::new()
        .with_fake_bkt()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let comments = "/repositories/acme/api/pullrequests/12/comments";
    let activities = "/rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities";
    let said = |count: usize| {
        let activity = |n: usize| {
            json!({"action": "COMMENTED", "comment": {"text": format!("Comment {n}"),
                "author": {"displayName": "Carol"}, "comments": []}})
        };
        (
            cloud_comments(count),
            json!({"values": (1..=count).map(activity).collect::<Vec<_>>()}),
        )
    };

    // Seventy comments take two pages of 50, and every one is shown.
    let (cloud, dc) = said(70);
    bkt_api(&user, comments, cloud);
    bkt_api(&user, activities, dc);
    let shown = (
        the_pull_request_of(&user, "api")["comments"].clone(),
        the_pull_request_of(&user, "web")["comments"].clone(),
    );
    assert_eq!(shown, (json!(70), json!(70)));
    let calls = bkt_calls(&user);
    for call in [
        "api /repositories/acme/api/pullrequests/12/comments --context cloud --param pagelen=50",
        "api /2.0/repositories/acme/api/pullrequests/12/comments?pagelen=50&page=2 --context cloud",
        "api /rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities --context work --param limit=50",
        "api /rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities --context work --param limit=50 --param start=50",
    ] {
        assert!(calls.contains(&call.to_owned()), "{call} in {calls:?}");
    }

    // Up to the 100 it read before, and no more.
    let (cloud, dc) = said(150);
    bkt_api(&user, comments, cloud);
    bkt_api(&user, activities, dc);
    let shown = (
        the_pull_request_of(&user, "api")["comments"].clone(),
        the_pull_request_of(&user, "web")["comments"].clone(),
    );
    assert_eq!(shown, (json!(100), json!(100)));
    assert!(
        page_sizes(&user).iter().all(|n| *n <= 50),
        "{:?}",
        page_sizes(&user)
    );
}

#[test]
fn a_cloud_conflict_on_a_later_page_of_the_diffstat_is_found_up_to_five_hundred_files() {
    let user = User::new()
        .with_fake_bkt()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    bitbucket_fixture(&user);
    bitbucket_pull_requests(&user);
    let diffstat = "/repositories/acme/api/pullrequests/12/diffstat";
    let files = |count: usize, conflict_at: Option<usize>| {
        let file = |n: usize| {
            let status = if Some(n) == conflict_at {
                "merge conflict"
            } else {
                "modified"
            };
            json!({"status": status, "new": {"path": format!("src/{n}.rs")}})
        };
        json!({"values": (1..=count).map(file).collect::<Vec<_>>()})
    };
    let mergeable = |user: &User| the_pull_request_of(user, "api")["mergeable"].clone();

    bkt_api(&user, diffstat, files(120, None));
    let clean = mergeable(&user);
    bkt_api(&user, diffstat, files(480, Some(470)));
    let late_conflict = mergeable(&user);
    // Past the 500 files it reads, with no conflict among them, nothing is known.
    bkt_api(&user, diffstat, files(600, Some(550)));
    let unknown = mergeable(&user);

    assert_eq!(
        (clean, late_conflict, unknown),
        (json!(true), json!(false), Value::Null)
    );
    assert!(
        page_sizes(&user).iter().all(|n| *n <= 50),
        "{:?}",
        page_sizes(&user)
    );
}
