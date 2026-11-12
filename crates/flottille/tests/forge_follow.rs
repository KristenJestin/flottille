//! The scenarios of a run an agent starts (`docs/forges.md`, "The CI family", "Following a run"):
//! `forge` `run` follows the run it started unless told `follow: false`, and the agent is woken
//! when it ends or waits on a manual step. The run is read by the daemon's rounds of the items
//! (`docs/items.md`), which each test starts itself (`items.refresh`). Through the fake `gh` and
//! the fake `bkt`; no test ever calls GitHub nor Bitbucket.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// What `bitbucket-pipelines.yml` holds on every branch of `api`.
const BITBUCKET_PIPELINES: &str = "\
pipelines:
  custom:
    nightly:
      - step: {name: e2e, script: [make e2e]}
";

/// A workflow of `web` that can be dispatched.
const E2E_WORKFLOW: &str = "\
name: E2E
on:
  workflow_dispatch:
    inputs:
      suite: {type: string}
jobs:
  e2e: {runs-on: ubuntu-latest, steps: [{run: make e2e}]}
";

/// A project `app` of two repositories, `api` on Bitbucket Cloud `acme/api` and `web` on GitHub
/// `acme/web`, each declared and holding its CI's files, and its ready workspace `login` on the
/// branch `app/login`.
fn fixture(user: &User) {
    let folder = user.path("work/app");
    let repositories = [
        (
            "api",
            "git@bitbucket.org:acme/api.git",
            json!({"kind": "bitbucket-cloud", "repository": "acme/api"}),
        ),
        (
            "web",
            "git@github.com:acme/web.git",
            json!({"kind": "github", "repository": "acme/web"}),
        ),
    ];
    for (path, _, _) in &repositories {
        let dir = folder.join(path);
        user.git_init(&dir);
        match *path {
            "web" => {
                let workflows = dir.join(".github/workflows");
                std::fs::create_dir_all(&workflows).unwrap();
                std::fs::write(workflows.join("e2e.yml"), E2E_WORKFLOW).unwrap();
            }
            _ => std::fs::write(dir.join("bitbucket-pipelines.yml"), BITBUCKET_PIPELINES).unwrap(),
        }
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
    for (path, url, _) in &repositories {
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
    user.bkt_script(bkt_script());
    user.gh_script(gh_script());
}

/// A step of a Bitbucket run, as `bkt pipeline view --json` prints it.
fn step(uuid: &str, name: &str, state: &str, result: &str) -> Value {
    json!({"uuid": uuid, "name": name, "state": {"name": state, "result": {"name": result}}})
}

/// The fake `bkt`: `pipeline run` starts the run 43.
fn bkt_script() -> Value {
    json!({
        "contexts": [{"name": "cloud", "host": "bitbucket.org", "active": true}],
        "pipelines": {"acme/api": []},
        "steps": {"acme/api": {}},
        "triggered": 43
    })
}

/// The fake `gh`: `workflow run` starts the run 78.
fn gh_script() -> Value {
    json!({"runs": {"acme/web": []}, "run_views": {"acme/web": {}}, "dispatched": 78})
}

/// An agent started in the workspace `login`: its id and session.
fn agent(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "hello"}),
        )
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

fn setup() -> (User, String, Mcp) {
    let user = User::with_fake_agent().with_fake_gh().with_fake_bkt();
    fixture(&user);
    let (id, mcp) = agent(&user);
    (user, id, mcp)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A read of `forge`, which the gate lets through in every mode.
fn read(mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("forge", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
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

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A Bitbucket run of `custom: nightly` on `app/login` numbered 43, as Bitbucket's API gives it
/// in `state`.
fn bitbucket_run(state: Value) -> Value {
    json!({"uuid": "{43}", "build_number": 43, "state": state,
        "trigger": {"name": "MANUAL"},
        "created_on": "2026-11-01T11:00:00.000000+00:00",
        "completed_on": "2026-11-01T11:04:05.000000+00:00",
        "target": {"type": "pipeline_ref_target", "ref_type": "branch",
            "ref_name": "app/login", "selector": {"type": "custom", "pattern": "nightly"}}})
}

/// The fake `bkt` with the run 43 in `state`, its steps `steps`.
fn bkt_with_run(user: &User, state: Value, steps: Value) {
    let mut script = bkt_script();
    script["pipelines"]["acme/api"]
        .as_array_mut()
        .unwrap()
        .insert(0, bitbucket_run(state));
    script["steps"]["acme/api"]["43"] = steps;
    user.bkt_script(script);
}

fn running() -> Value {
    json!({"name": "IN_PROGRESS"})
}

fn completed(result: &str) -> Value {
    json!({"name": "COMPLETED", "result": {"name": result}})
}

/// A round of the project's items, as the daemon makes every few minutes: what it found of the
/// runs followed is stored as deliveries by the time it answers.
fn a_round(user: &User) {
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

/// The bodies of the subscription deliveries made to the agent, oldest first, as stored.
fn delivered(user: &User, agent: &str) -> Vec<String> {
    let store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let mut bodies: Vec<String> = store
        .last_deliveries_of(agent, "subscription", 100)
        .unwrap()
        .into_iter()
        .map(|delivery| delivery.body)
        .collect();
    bodies.reverse();
    bodies
}

/// The agent's open subscriptions, as the window lists them.
fn subscriptions(user: &User) -> Vec<Value> {
    user.call("subscription.list", json!({})).unwrap()["subscriptions"]
        .as_array()
        .unwrap()
        .clone()
}

fn idle(user: &User, agent: &str) {
    eventually("its turn to end", || {
        user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"] == "idle"
    });
}

/// An agent in Full mode that started the run 43 of `custom: nightly`, running: the answer.
fn started_run(user: &User, id: &str, mcp: &Mcp, args: Value) -> String {
    set_mode(user, id, "full");
    idle(user, id);
    let mut args = args;
    args["action"] = "run".into();
    args["repository"] = "api".into();
    args["pipeline"] = "custom: nightly".into();
    let answer = mcp.call("forge", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

#[test]
fn a_run_started_by_an_agent_wakes_it_when_it_ends() {
    let (user, id, mcp) = setup();
    bkt_with_run(
        &user,
        running(),
        json!([step("{step-e2e}", "e2e", "IN_PROGRESS", "")]),
    );

    let answer = started_run(&user, &id, &mcp, json!({}));
    assert!(answer.contains("#43"), "{answer}");
    for said in ["Followed until it ends", "no wake is needed"] {
        assert!(answer.contains(said), "{said} in {answer}");
    }
    let listed = subscriptions(&user);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["source"], "run");
    assert!(
        listed[0]["named"]
            .as_str()
            .unwrap()
            .contains("run #43 of api"),
        "{listed:?}"
    );

    a_round(&user);
    assert!(delivered(&user, &id).is_empty(), "still running is no news");

    bkt_with_run(
        &user,
        completed("SUCCESSFUL"),
        json!([step("{step-e2e}", "e2e", "COMPLETED", "SUCCESSFUL")]),
    );
    a_round(&user);
    assert!(
        !delivered(&user, &id).is_empty(),
        "the end of the run delivered"
    );
    let delivery = delivered(&user, &id).remove(0);
    for part in [
        "run #43 of api",
        "custom: nightly",
        "app/login",
        "https://bitbucket.org/acme/api/pipelines/results/43",
        "successful",
        "4 min 5 s",
        "not instructions",
    ] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }
    // Over with the run.
    assert!(subscriptions(&user).is_empty());
    a_round(&user);
    assert_eq!(delivered(&user, &id).len(), 1);
}

#[test]
fn a_failed_run_names_its_failed_steps() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));
    started_run(&user, &id, &mcp, json!({}));

    bkt_with_run(
        &user,
        completed("FAILED"),
        json!([
            step("{step-build}", "build", "COMPLETED", "SUCCESSFUL"),
            step("{step-e2e}", "e2e", "COMPLETED", "FAILED"),
            step("{step-lint}", "lint", "COMPLETED", "ERROR")
        ]),
    );
    a_round(&user);
    assert!(!delivered(&user, &id).is_empty(), "the failure delivered");
    let delivery = delivered(&user, &id).remove(0);
    assert!(delivery.contains("failed"), "{delivery}");
    assert!(delivery.contains("Failed steps: e2e, lint"), "{delivery}");
    assert!(!delivery.contains("build,"), "{delivery}");
    assert!(delivery.contains("run_logs"), "{delivery}");
    // No log is read: run_logs reads it.
    assert!(
        !bkt_calls(&user)
            .iter()
            .any(|c| c.starts_with("pipeline logs")),
        "{:?}",
        bkt_calls(&user)
    );
}

#[test]
fn a_run_waiting_on_a_manual_step_wakes_the_agent() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));
    started_run(&user, &id, &mcp, json!({}));

    let paused = json!({"name": "IN_PROGRESS", "stage": {"name": "PAUSED"}});
    let steps = json!([
        step("{step-build}", "build", "COMPLETED", "SUCCESSFUL"),
        step("{step-deploy}", "deploy", "PENDING", "")
    ]);
    bkt_with_run(&user, paused, steps);
    a_round(&user);
    assert!(!delivered(&user, &id).is_empty(), "the wait delivered");
    let delivery = delivered(&user, &id).remove(0);
    assert!(delivery.contains("manual step deploy"), "{delivery}");
    // Still followed, and the same wait is not said again.
    assert_eq!(subscriptions(&user).len(), 1);
    a_round(&user);
    assert_eq!(delivered(&user, &id).len(), 1);

    bkt_with_run(&user, completed("SUCCESSFUL"), json!([]));
    a_round(&user);
    assert!(delivered(&user, &id).len() == 2, "the end delivered");
    assert!(subscriptions(&user).is_empty());
}

#[test]
fn follow_false_starts_the_run_without_following_it() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));

    let answer = started_run(&user, &id, &mcp, json!({"follow": false}));

    assert!(answer.contains("#43"), "{answer}");
    assert!(!answer.contains("Followed"), "{answer}");
    assert!(subscriptions(&user).is_empty());
    // A run is followed only when it is started.
    let refused = mcp.call("forge", json!({"action": "runs", "follow": false}));
    assert!(
        refused.text.contains("follow is for run"),
        "{}",
        refused.text
    );
}

#[test]
fn unsubscribe_ends_the_following_of_a_run() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));
    started_run(&user, &id, &mcp, json!({}));
    assert_eq!(subscriptions(&user).len(), 1);

    let answer = read(
        &mcp,
        json!({"action": "unsubscribe", "repository": "api", "run": 43}),
    );

    assert!(answer.contains("run #43 of api"), "{answer}");
    assert!(subscriptions(&user).is_empty());
    bkt_with_run(&user, completed("SUCCESSFUL"), json!([]));
    a_round(&user);
    assert!(
        delivered(&user, &id).is_empty(),
        "{:?}",
        delivered(&user, &id)
    );
}

#[test]
fn a_run_that_cannot_be_read_is_said_once() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));
    started_run(&user, &id, &mcp, json!({}));

    // The run is gone from what the forge answers: every read of it fails.
    let mut script = bkt_script();
    script["steps"]["acme/api"]
        .as_object_mut()
        .unwrap()
        .remove("43");
    user.bkt_script(script);
    a_round(&user);
    assert!(!delivered(&user, &id).is_empty(), "the failure said");
    a_round(&user);
    a_round(&user);
    let all = delivered(&user, &id);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(all[0].contains("cannot be read"), "{}", all[0]);
    // Still followed: it ends when it can be read and has ended.
    assert_eq!(subscriptions(&user).len(), 1);
    bkt_with_run(&user, completed("SUCCESSFUL"), json!([]));
    a_round(&user);
    assert!(delivered(&user, &id).len() == 2, "the end delivered");
}

#[test]
fn a_github_workflow_run_wakes_the_agent_when_it_ends() {
    let (user, id, mcp) = setup();
    let gh_with = |status: &str, conclusion: &str| {
        let mut script = gh_script();
        script["run_views"]["acme/web"]["78"] = json!({
            "databaseId": 78, "workflowName": "E2E", "status": status,
            "conclusion": conclusion, "url": "https://github.com/acme/web/actions/runs/78",
            "startedAt": "2026-11-01T11:00:00Z", "updatedAt": "2026-11-01T11:02:05Z",
            "jobs": [
                {"databaseId": 801, "name": "e2e", "status": status, "conclusion": conclusion},
                {"databaseId": 802, "name": "lint", "status": "completed", "conclusion": "success"}
            ]
        });
        user.gh_script(script);
    };
    gh_with("in_progress", "");
    set_mode(&user, &id, "full");
    idle(&user, &id);

    let answer = mcp.call(
        "forge",
        json!({"action": "run", "repository": "web", "pipeline": "e2e.yml",
            "variables": [{"key": "suite", "value": "smoke"}]}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("Followed until it ends"),
        "{}",
        answer.text
    );
    a_round(&user);
    assert!(delivered(&user, &id).is_empty(), "still running is no news");

    gh_with("completed", "failure");
    a_round(&user);
    assert!(!delivered(&user, &id).is_empty(), "the end delivered");
    let delivery = delivered(&user, &id).remove(0);
    for part in [
        "run 78 of web",
        "e2e.yml",
        "app/login",
        "https://github.com/acme/web/actions/runs/78",
        "failure",
        "2 min 5 s",
        "Failed steps: e2e",
    ] {
        assert!(delivery.contains(part), "{part} in {delivery}");
    }
    assert!(subscriptions(&user).is_empty());
}

#[test]
fn a_subscription_to_a_pull_request_says_its_running_checks_and_the_run_followed_on_its_branch() {
    let (user, id, mcp) = setup();
    let check = |name: &str, status: &str, conclusion: &str| {
        json!({"__typename": "CheckRun", "name": name, "status": status,
            "conclusion": conclusion})
    };
    let mut script = gh_script();
    script["pulls"] = json!({"acme/web": {"app/login": {
        "number": 12, "title": "Add the login page", "state": "OPEN", "isDraft": false,
        "headRefName": "app/login", "headRefOid": "aaaaaaa1", "baseRefName": "main",
        "url": "https://github.com/acme/web/pull/12",
        "statusCheckRollup": [check("build", "COMPLETED", "SUCCESS"),
            check("test", "IN_PROGRESS", "")],
        "comments": [], "reviews": []
    }}});
    script["run_views"]["acme/web"]["78"] = json!({"databaseId": 78, "status": "in_progress",
        "conclusion": "", "jobs": []});
    user.gh_script(script);
    set_mode(&user, &id, "full");
    idle(&user, &id);
    let started = mcp.call(
        "forge",
        json!({"action": "run", "repository": "web", "pipeline": "e2e.yml"}),
    );
    assert!(!started.error, "{}", started.text);

    let answer = read(
        &mcp,
        json!({"action": "subscribe", "repository": "web", "events": ["checks"]}),
    );

    for said in [
        "Its checks running as you subscribed: test.",
        "a run started on its branch outside them is followed by forge run, not by this \
         subscription",
        "You already follow the run 78 of web (e2e.yml on app/login)",
    ] {
        assert!(answer.contains(said), "{said:?} in {answer}");
    }
    assert!(
        !answer.contains("running as you subscribed: build"),
        "{answer}"
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

/// How many reads of a run the fakes were asked: `bkt pipeline view`, `gh run view`.
fn run_reads(user: &User) -> usize {
    let views =
        |calls: Vec<String>, head: &str| calls.iter().filter(|c| c.starts_with(head)).count();
    views(bkt_calls(user), "pipeline view") + views(gh_calls(user), "run view")
}

#[test]
fn a_round_with_no_run_followed_reads_no_run_and_one_followed_is_read_once() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));

    a_round(&user);
    a_round(&user);
    assert_eq!(run_reads(&user), 0, "{:?}", bkt_calls(&user));

    started_run(&user, &id, &mcp, json!({}));
    let before = run_reads(&user);
    a_round(&user);
    assert_eq!(run_reads(&user) - before, 1, "{:?}", bkt_calls(&user));
}

#[test]
fn a_run_cancelled_while_a_round_reads_it_wakes_nobody_and_stays_cancelled() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));
    started_run(&user, &id, &mcp, json!({}));
    let subscription = subscriptions(&user)[0]["id"].as_str().unwrap().to_owned();
    // Waiting on a manual step, which would wake the agent; every read held a while.
    let paused = json!({"name": "IN_PROGRESS", "stage": {"name": "PAUSED"}});
    bkt_with_run(
        &user,
        paused,
        json!([step("{step-deploy}", "deploy", "PENDING", "")]),
    );
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("bkt/script.json")).unwrap())
            .unwrap();
    script["read_delay_ms"] = json!(1500);
    user.bkt_script(script);
    let before = run_reads(&user);

    std::thread::scope(|scope| {
        let round = scope.spawn(|| a_round(&user));
        eventually("the round reading the run", || run_reads(&user) > before);
        user.call("subscription.cancel", json!({"subscription": subscription}))
            .unwrap();
        round.join().unwrap();
    });

    assert!(
        subscriptions(&user).is_empty(),
        "{:?}",
        subscriptions(&user)
    );
    assert!(
        delivered(&user, &id).is_empty(),
        "{:?}",
        delivered(&user, &id)
    );
}

#[test]
fn unsubscribing_a_run_names_its_repository_when_the_project_has_several() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));

    let answer = started_run(&user, &id, &mcp, json!({}));

    assert!(
        answer.contains(r#"forge unsubscribe {"run": 43, "repository": "api"}"#),
        "{answer}"
    );
    let refused = mcp.call("forge", json!({"action": "unsubscribe", "run": 43}));
    assert!(refused.error, "{}", refused.text);
    assert!(
        refused.text.contains("needs repository") && refused.text.contains("api, web"),
        "{}",
        refused.text
    );
    assert_eq!(subscriptions(&user).len(), 1);
}

#[test]
fn a_run_bkt_cannot_read_says_what_bkt_needs() {
    let (user, id, mcp) = setup();
    bkt_with_run(&user, running(), json!([]));
    started_run(&user, &id, &mcp, json!({}));

    // The user's bkt lost its sign-in to Bitbucket Cloud: it has a Data Center context alone.
    let mut script = bkt_script();
    script["contexts"] = json!([{"name": "work", "host": "bitbucket.example.com", "active": true}]);
    user.bkt_script(script);
    a_round(&user);

    let all = delivered(&user, &id);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(
        all[0].contains("bkt has no context for bitbucket.org: sign in with `bkt auth login"),
        "{}",
        all[0]
    );
}

#[test]
fn runs_and_the_end_of_a_run_say_its_branch() {
    let (user, id, mcp) = setup();
    bkt_with_run(
        &user,
        running(),
        json!([step("{step-e2e}", "e2e", "IN_PROGRESS", "")]),
    );
    started_run(&user, &id, &mcp, json!({}));

    // `runs` of the workspace's branch says each run's branch, not only a list of every branch.
    let runs = read(&mcp, json!({"action": "runs", "repository": "api"}));
    assert!(
        runs.contains("#43 custom: nightly") && runs.contains(", on app/login"),
        "{runs}"
    );
    let view = read(
        &mcp,
        json!({"action": "run_view", "repository": "api", "run": 43}),
    );
    assert!(view.contains("run #43 of acme/api on app/login"), "{view}");

    bkt_with_run(
        &user,
        completed("SUCCESSFUL"),
        json!([step("{step-e2e}", "e2e", "COMPLETED", "SUCCESSFUL")]),
    );
    a_round(&user);
    let delivery = delivered(&user, &id).remove(0);
    assert!(
        delivery.contains("(custom: nightly on app/login)"),
        "{delivery}"
    );

    // GitHub says it as well.
    let mut script = gh_script();
    script["runs"]["acme/web"] = json!([{"databaseId": 78, "workflowName": "E2E",
        "status": "in_progress", "conclusion": "", "event": "workflow_dispatch",
        "headBranch": "app/login", "createdAt": "2026-11-01T10:00:00Z",
        "url": "https://github.com/acme/web/actions/runs/78"}]);
    script["run_views"]["acme/web"]["78"] = json!({"databaseId": 78, "workflowName": "E2E",
        "status": "in_progress", "conclusion": "", "headBranch": "app/login",
        "url": "https://github.com/acme/web/actions/runs/78", "jobs": []});
    user.gh_script(script);
    let runs = read(&mcp, json!({"action": "runs", "repository": "web"}));
    assert!(runs.contains(", on app/login"), "{runs}");
    let view = read(
        &mcp,
        json!({"action": "run_view", "repository": "web", "run": 78}),
    );
    assert!(view.contains("E2E on app/login"), "{view}");
}
