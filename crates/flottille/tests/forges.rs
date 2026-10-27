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
    for (repository, url) in [
        ("api", "git@github.com:acme/api.git"),
        ("web", "https://github.com/acme/web.git"),
        ("lib", "https://git.example.com/acme/lib.git"),
    ] {
        user.git(
            &folder.join(repository),
            &["remote", "set-url", "origin", url],
        );
    }
    folder
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
fn a_github_remote_over_ssh_and_https_is_github_and_another_host_is_no_forge_support_yet() {
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
    assert_eq!(
        lib["note"],
        "no forge support yet: origin is on git.example.com"
    );
    // The other host's repository is never asked of `gh`.
    assert!(
        !gh_calls(&user).iter().any(|call| call.contains("acme/lib")),
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
    assert!(
        text.contains("  lib: no forge support yet: origin is on git.example.com\n"),
        "{text}"
    );
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
        "lib: no forge support yet: origin is on git.example.com.",
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
