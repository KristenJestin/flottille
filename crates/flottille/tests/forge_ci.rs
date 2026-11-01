//! The scenarios of the CI durations (`docs/forges.md`, "How long CI takes"): `forge show` and
//! `forge subscribe` tell an agent how long the repository's recent CI runs took, and a
//! subscription asked for with no `minutes` is sized by it. Read through the fake `gh` and the
//! fake `bkt`; no test ever calls GitHub nor Bitbucket.

mod common;

use common::mcp::{Mcp, server_of};
use common::{User, bkt_pipeline, gh_run};
use serde_json::{Value, json};

/// The line `forge show` and `forge subscribe` give about the runs of a repository.
const LINE: &str = "Recent CI runs of";

/// A project `app` of repositories whose `origin` says where their forge is, each declared, and
/// its ready workspace `login` on the branch `app/login`.
fn fixture(user: &User, repositories: &[(&str, &str, Value)]) {
    let folder = user.path("work/app");
    for (path, _, _) in repositories {
        let dir = folder.join(path);
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), format!("{path}\n")).unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
        let remote = user.path(&format!("remotes/{path}.git"));
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
    for (path, url, _) in repositories {
        user.git(&folder.join(path), &["remote", "set-url", "origin", url]);
    }
    let declared: Vec<Value> = repositories
        .iter()
        .map(|(path, _, forge)| json!({"path": path, "forge": forge}))
        .collect();
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": declared}),
    )
    .unwrap();
}

/// `api` on GitHub `acme/api`.
fn github(user: &User) {
    fixture(
        user,
        &[(
            "api",
            "git@github.com:acme/api.git",
            json!({"kind": "github", "repository": "acme/api"}),
        )],
    );
}

/// What `gh pr view --json` answers for the pull request #12 of `app/login`.
fn pull() -> Value {
    json!({
        "number": 12,
        "title": "Add the login page",
        "state": "OPEN",
        "isDraft": false,
        "headRefOid": "aaaaaaa1",
        "baseRefName": "main",
        "url": "https://github.com/acme/api/pull/12",
        "statusCheckRollup": [],
        "comments": [],
        "reviews": []
    })
}

/// The fake `gh` answers the pull request, and `runs` for `gh run list`.
fn runs(user: &User, runs: Vec<Value>) {
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": pull()}},
        "runs": {"acme/api": runs}
    }));
}

/// `count` successful runs of `seconds` each.
fn each(count: usize, seconds: u64) -> Vec<Value> {
    (0..count).map(|_| gh_run("success", seconds)).collect()
}

/// An agent started in the workspace `login`.
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

fn gh_calls(user: &User) -> Vec<String> {
    calls(&user.gh_received())
}

fn bkt_calls(user: &User) -> Vec<String> {
    calls(&user.bkt_received())
}

fn calls(received: &[Value]) -> Vec<String> {
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

/// What `forge subscribe` answers for `args`, which is not refused.
fn subscribe(mcp: &Mcp, args: Value) -> String {
    let mut args = args;
    args["action"] = "subscribe".into();
    args["events"] = json!(["checks"]);
    let answer = mcp.call("forge", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

/// The longest a subscription can wait, as `forge subscribe` says it when it refuses more.
fn longest(mcp: &Mcp) -> u64 {
    let refused = mcp.call(
        "forge",
        json!({"action": "subscribe", "events": ["checks"], "minutes": 100_000_000}),
    );
    assert!(refused.error, "{}", refused.text);
    refused
        .text
        .split("minutes is from 1 to ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no longest in {}", refused.text))
}

#[test]
fn forge_show_on_github_states_the_average_shortest_and_longest_of_the_last_five_finished_runs() {
    let user = User::with_fake_agent().with_fake_gh();
    github(&user);
    // Newest first: a cancelled run and a sixth finished one are not counted; the five that are
    // took 3, 6, 4, 3 and 4 minutes.
    runs(
        &user,
        vec![
            gh_run("cancelled", 30),
            gh_run("success", 180),
            gh_run("failure", 360),
            gh_run("success", 240),
            gh_run("success", 180),
            gh_run("failure", 240),
            gh_run("success", 3600),
        ],
    );
    let mcp = agent(&user);

    let by_branch = mcp.call("forge", json!({"action": "show"}));
    let by_number = mcp.call("forge", json!({"action": "show", "number": 12}));

    for answer in [by_branch, by_number] {
        assert!(!answer.error, "{}", answer.text);
        assert!(answer.text.contains("pull request #12"), "{}", answer.text);
        assert_eq!(answer.text.matches(LINE).count(), 1, "{}", answer.text);
        assert!(
            answer
                .text
                .contains("Recent CI runs of acme/api took 4 min on average (3 to 6 min, last 5)."),
            "{}",
            answer.text
        );
    }
    let asked = gh_calls(&user)
        .into_iter()
        .find(|call| call.starts_with("run list "))
        .expect("a read of the runs");
    for part in [
        "-R github.com/acme/api",
        "--status completed",
        "--json conclusion,startedAt,updatedAt",
    ] {
        assert!(asked.contains(part), "{part} in {asked}");
    }
}

#[test]
fn forge_show_on_bitbucket_cloud_states_the_average_shortest_and_longest_of_its_pipelines() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket(&user);
    // Newest first: a pipeline still running and a stopped one are not counted, nor the sixth
    // finished one; the five that are took 3, 6, 4, 3 and 4 minutes.
    pipelines(
        &user,
        json!([
            bkt_pipeline("", 0),
            bkt_pipeline("STOPPED", 30),
            bkt_pipeline("SUCCESSFUL", 180),
            bkt_pipeline("FAILED", 360),
            bkt_pipeline("SUCCESSFUL", 240),
            bkt_pipeline("ERROR", 180),
            bkt_pipeline("SUCCESSFUL", 240),
            bkt_pipeline("SUCCESSFUL", 3600),
        ]),
    );
    let mcp = agent(&user);

    let answer = mcp.call("forge", json!({"action": "show"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("pull request #12"), "{}", answer.text);
    assert_eq!(answer.text.matches(LINE).count(), 1, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("Recent CI runs of acme/api took 4 min on average (3 to 6 min, last 5)."),
        "{}",
        answer.text
    );
    let calls = bkt_calls(&user);
    assert!(
        calls.iter().any(|call| call
            .starts_with("pipeline list --context cloud --workspace acme --repo api --limit 50")),
        "{calls:?}"
    );
}

#[test]
fn forge_show_on_bitbucket_data_center_says_nothing_of_ci_and_asks_for_no_pipeline() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket(&user);
    pipelines(&user, json!([bkt_pipeline("SUCCESSFUL", 240)]));
    let mcp = agent(&user);

    let answer = mcp.call("forge", json!({"action": "show", "repository": "web"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("pull request #7"), "{}", answer.text);
    assert!(!answer.text.contains(LINE), "{}", answer.text);
    let calls = bkt_calls(&user);
    assert!(
        calls
            .iter()
            .all(|call| !(call.starts_with("pipeline ") && call.contains("--project"))),
        "{calls:?}"
    );
}

#[test]
fn with_no_finished_run_forge_show_says_nothing_of_ci_and_does_not_fail() {
    let user = User::with_fake_agent().with_fake_gh();
    github(&user);
    let mcp = agent(&user);

    // Nothing at all, then only a run that was cancelled.
    for script in [vec![], vec![gh_run("cancelled", 120)]] {
        runs(&user, script);
        let answer = mcp.call("forge", json!({"action": "show"}));
        assert!(!answer.error, "{}", answer.text);
        assert!(answer.text.contains("pull request #12"), "{}", answer.text);
        assert!(!answer.text.contains(LINE), "{}", answer.text);
    }
}

#[test]
fn a_failure_to_read_the_runs_never_fails_forge_show_nor_forge_subscribe() {
    let user = User::with_fake_agent().with_fake_gh();
    github(&user);
    user.gh_script(json!({
        "pulls": {"acme/api": {"app/login": pull()}},
        "run_fail": {"acme/api": "HTTP 502: Bad Gateway"}
    }));
    let mcp = agent(&user);

    let shown = mcp.call("forge", json!({"action": "show"}));
    let subscribed = subscribe(&mcp, json!({}));

    assert!(!shown.error, "{}", shown.text);
    assert!(shown.text.contains("pull request #12"), "{}", shown.text);
    assert!(!shown.text.contains(LINE), "{}", shown.text);
    assert!(subscribed.contains("Subscribed"), "{subscribed}");
    assert!(!subscribed.contains(LINE), "{subscribed}");
    assert!(subscribed.contains("within 60 min"), "{subscribed}");
}

#[test]
fn a_failure_to_read_the_pipelines_never_fails_forge_show() {
    let user = User::with_fake_agent().with_fake_bkt();
    bitbucket(&user);
    pipelines(&user, json!([bkt_pipeline("SUCCESSFUL", 240)]));
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    script["pipeline_fail"] = json!({"acme/api": "Error: 502 Bad Gateway"});
    user.bkt_script(script);
    let mcp = agent(&user);

    let answer = mcp.call("forge", json!({"action": "show", "repository": "api"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("pull request #12"), "{}", answer.text);
    assert!(!answer.text.contains(LINE), "{}", answer.text);
}

#[test]
fn forge_subscribe_with_no_minutes_takes_twice_the_average_rounded_up_to_the_minute() {
    let user = User::with_fake_agent().with_fake_gh();
    github(&user);
    // 12 min 10 s each: twice that is 24 min 20 s.
    runs(&user, each(5, 730));
    let mcp = agent(&user);

    let said = subscribe(&mcp, json!({}));

    assert!(said.contains("within 25 min"), "{said}");
    assert!(
        said.contains("Recent CI runs of acme/api took 12 min on average (12 to 12 min, last 5)."),
        "{said}"
    );
    assert!(
        said.contains("Duration: 25 min, twice the average of the recent CI runs."),
        "{said}"
    );
}

#[test]
fn forge_subscribe_with_no_minutes_waits_at_least_15_minutes_when_the_runs_are_short() {
    let user = User::with_fake_agent().with_fake_gh();
    github(&user);
    runs(&user, each(5, 120));
    let mcp = agent(&user);

    let said = subscribe(&mcp, json!({}));

    assert!(said.contains("within 15 min"), "{said}");
    assert!(
        said.contains(
            "Duration: 15 min, the shortest: twice the average of the recent CI runs is 4 min."
        ),
        "{said}"
    );
}

#[test]
fn forge_subscribe_with_no_minutes_waits_at_most_the_longest_when_the_runs_are_long() {
    let user = User::with_fake_agent().with_fake_gh();
    github(&user);
    // 20 hours each: twice that is past any longest.
    runs(&user, each(5, 20 * 3600));
    let mcp = agent(&user);
    let longest = longest(&mcp);

    let said = subscribe(&mcp, json!({}));

    assert!(said.contains(&format!("within {longest} min")), "{said}");
    assert!(
        said.contains(&format!(
            "Duration: {longest} min, the longest: twice the average of the recent CI runs is {} \
             min.",
            2 * 20 * 60
        )),
        "{said}"
    );
}

#[test]
fn forge_subscribe_with_no_minutes_and_no_history_takes_60_minutes() {
    let user = User::with_fake_agent().with_fake_gh();
    github(&user);
    runs(&user, vec![]);
    let mcp = agent(&user);

    let said = subscribe(&mcp, json!({}));

    assert!(said.contains("within 60 min"), "{said}");
    assert!(
        said.contains("Duration: 60 min, the default: no recent CI run to measure it by."),
        "{said}"
    );
    assert!(!said.contains(LINE), "{said}");
}

#[test]
fn forge_subscribe_with_minutes_keeps_the_agent_s_choice_and_still_says_the_runs() {
    let user = User::with_fake_agent().with_fake_gh();
    github(&user);
    runs(&user, each(5, 730));
    let mcp = agent(&user);

    let said = subscribe(&mcp, json!({"minutes": 30}));

    assert!(said.contains("within 30 min"), "{said}");
    assert!(!said.contains("Duration:"), "{said}");
    assert!(said.contains("took 12 min on average"), "{said}");
}

#[test]
fn the_runs_are_read_by_forge_show_and_forge_subscribe_only_never_by_the_watch() {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    github(&user);
    runs(&user, each(5, 240));
    let mcp = agent(&user);
    let reads = |user: &User| {
        gh_calls(user)
            .iter()
            .filter(|call| call.starts_with("run list "))
            .count()
    };

    subscribe(&mcp, json!({}));
    let after_subscribing = reads(&user);
    // The subscription's watch reads the pull request again each minute, and not the runs.
    for _ in 0..3 {
        let before = gh_calls(&user).len();
        user.call("test.advance", json!({"ms": 61_000})).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while gh_calls(&user).len() == before {
            assert!(std::time::Instant::now() < deadline, "no read of the watch");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    assert_eq!(after_subscribing, 1);
    assert_eq!(reads(&user), 1, "{:?}", gh_calls(&user));
}

// Bitbucket: `api` on Cloud, `web` on a Data Center host.

fn bitbucket(user: &User) {
    fixture(
        user,
        &[
            (
                "api",
                "git@bitbucket.org:acme/api.git",
                json!({"kind": "bitbucket-cloud", "repository": "acme/api"}),
            ),
            (
                "web",
                "ssh://git@bitbucket.example.com:7999/APP/web.git",
                json!({"kind": "bitbucket-data-center", "host": "bitbucket.example.com",
                    "repository": "APP/web"}),
            ),
        ],
    );
}

/// The fake `bkt`'s script: the pull request #12 of `app/login` in `acme/api` (Cloud), #7 in
/// `APP/web` (Data Center), and `pipelines` for both repositories.
fn pipelines(user: &User, pipelines: Value) {
    let repository = json!({"slug": "web", "project": {"key": "APP"}});
    user.bkt_script(json!({
        "contexts": [
            {"name": "cloud", "host": "bitbucket.org", "workspace": "acme", "active": true},
            {"name": "work", "host": "bitbucket.example.com:7990", "project_key": "APP",
             "active": false}
        ],
        "pulls": {
            "acme/api": [{
                "id": 12, "title": "Add the login page", "state": "OPEN",
                "author": {"display_name": "Alice", "username": "alice"},
                "source": {"branch": {"name": "app/login"}, "commit": {"hash": "abc1234"},
                    "repository": {"slug": "", "full_name": "acme/api"}},
                "destination": {"branch": {"name": "main"},
                    "repository": {"slug": "", "full_name": "acme/api"}},
                "links": {"html": {"href": "https://bitbucket.org/acme/api/pull-requests/12"}},
                "summary": {"raw": ""}
            }],
            "APP/web": [{
                "id": 7, "title": "Add the login form", "state": "OPEN", "version": 3,
                "author": {"user": {"name": "alice", "displayName": "Alice"}},
                "fromRef": {"id": "refs/heads/app/login", "displayId": "app/login",
                    "latestCommit": "abc1234", "repository": repository},
                "toRef": {"id": "refs/heads/main", "displayId": "main",
                    "latestCommit": "def5678", "repository": repository},
                "reviewers": [], "participants": [],
                "links": {"self": [{"href":
                    "https://bitbucket.example.com/projects/APP/repos/web/pull-requests/7"}]}
            }]
        },
        "api": {
            "/repositories/acme/api/pullrequests/12": {"participants": []},
            "/repositories/acme/api/pullrequests/12/comments": {"values": []},
            "/rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities": {"values": []}
        },
        "pipelines": {"acme/api": pipelines.clone(), "APP/web": pipelines}
    }));
}
