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
            "base": "main",
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
            &"pr list -R acme/api --head app/login --state all --limit 1 --json number".to_owned()
        ),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("pr view 12 -R acme/api --json ")),
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

    assert!(mcp.names().contains(&"forge_read".to_owned()));
    let answer = mcp.call("forge_read", json!({}));
    assert!(!answer.error, "{}", answer.text);
    let text = answer.text;
    for said in [
        "api (acme/api): pull request #12, open, into main: Add the login page",
        "https://github.com/acme/api/pull/12",
        "review: changes requested",
        "checks: pending (build passing, lint pending)",
        "- alice: Looks good to me",
        "- bob, changes requested: Rename the form",
        "web (acme/web): no pull request for app/login.",
        &format!("lib: {NO_FORGE}."),
    ] {
        assert!(text.contains(said), "{said} in {text}");
    }

    let answer = mcp.call("forge_read", json!({"repository": "api"}));
    assert!(!answer.error, "{}", answer.text);
    assert!(!answer.text.contains("acme/web"), "{}", answer.text);

    // Outside its workspace: refused, and never asked of the forge.
    let calls = gh_calls(&user).len();
    let answer = mcp.call("forge_read", json!({"repository": "elsewhere"}));
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
    let answer = narrow.call("forge_read", json!({"repository": "api"}));
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("it has web"), "{}", answer.text);

    // Through the gate, allowed: nothing was held for the user.
    assert!(folder.is_dir());
    let listed = user.call("approval.list", json!({})).unwrap();
    assert!(listed["requests"].as_array().unwrap().is_empty());
}

#[test]
fn an_agent_in_the_project_folder_has_no_forge_read() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    let (_, mcp) = agent_in(&user, None);

    assert!(!mcp.names().contains(&"forge_read".to_owned()));
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

    let read = mcp.call("forge_read", json!({}));
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
            "base": "main",
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
        "pr list --context cloud --workspace acme --repo api --state all --limit 100 --json",
        "pr checks 12 --context cloud --workspace acme --repo api --json",
        "api /repositories/acme/api/pullrequests/12 --context cloud",
        "api /repositories/acme/api/pullrequests/12/comments --context cloud --param pagelen=100",
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
            "base": "main",
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
        "pr list --context work --project APP --repo web --state all --limit 100 --json",
        "pr checks 7 --context work --project APP --repo web --json",
        "api /rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities --context work --param limit=100",
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

    let answer = mcp.call("forge_read", json!({}));

    assert!(!answer.error, "{}", answer.text);
    for said in [
        "api (acme/api): pull request #12, open, into main: Add the login page",
        "https://bitbucket.org/acme/api/pull-requests/12",
        "review: changes requested",
        "reviewers: Bob changes requested",
        "checks: pending (build passing, lint pending)",
        "- Alice: Looks good to me",
        "web (APP/web): pull request #7, open, into main: Add the login form",
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

    let read = mcp.call("forge_read", json!({}));
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
