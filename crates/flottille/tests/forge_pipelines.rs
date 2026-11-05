//! The scenarios of the CI family of `forge` (`docs/forges.md`, "The CI family"): an agent lists
//! the pipelines it can run, reads the recent runs, a run's steps and a step's log, runs any
//! pipeline with its variables, stops a run and acts on one of its steps, every write under the
//! gate as the other forge writes are. Through the fake `gh` and the fake `bkt`; no test ever
//! calls GitHub nor Bitbucket.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// What `bitbucket-pipelines.yml` holds on every branch of `api`.
const BITBUCKET_PIPELINES: &str = "\
image: rust:1
pipelines:
  default:
    - step:
        name: test
        script: [cargo test]
  branches:
    main:
      - step: {name: build, script: [make]}
  pull-requests:
    '**':
      - step: {name: pr, script: [make]}
  custom:
    nightly:
      - step: {name: e2e, script: [make e2e]}
    deploy-preview:
      - variables:
          - name: PREVIEW_SEED
            default: \"42\"
          - name: PREVIEW_JOBS
            allowed-values: [\"1\", \"4\"]
      - step:
          name: deploy
          deployment: preview
          script: [./deploy]
";

/// A workflow of `web` that can be dispatched, with inputs.
const E2E_WORKFLOW: &str = "\
name: E2E
on:
  workflow_dispatch:
    inputs:
      suite:
        description: Which suite
        required: true
        type: choice
        options: [smoke, full]
      debug:
        type: boolean
        default: false
jobs:
  e2e:
    runs-on: ubuntu-latest
    steps: [{run: make e2e}]
";

/// One that runs on pushes alone: not one an agent can run.
const CI_WORKFLOW: &str = "\
name: CI
on: [push, pull_request]
jobs:
  test: {runs-on: ubuntu-latest, steps: [{run: make}]}
";

/// One dispatched with no input.
const DEPLOY_WORKFLOW: &str = "\
on: workflow_dispatch
jobs:
  deploy: {runs-on: ubuntu-latest, steps: [{run: ./deploy}]}
";

/// A project `app` of three repositories, `api` on Bitbucket Cloud `acme/api`, `web` on GitHub
/// `acme/web`, `dc` on a Data Center host, each declared and holding its CI's files, and its
/// ready workspace `login` on the branch `app/login`.
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
        (
            "dc",
            "ssh://git@bitbucket.example.com:7999/APP/dc.git",
            json!({"kind": "bitbucket-data-center", "host": "bitbucket.example.com",
                "repository": "APP/dc"}),
        ),
    ];
    for (path, _, _) in &repositories {
        let dir = folder.join(path);
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), format!("{path}\n")).unwrap();
        match *path {
            "web" => {
                let workflows = dir.join(".github/workflows");
                std::fs::create_dir_all(&workflows).unwrap();
                std::fs::write(workflows.join("e2e.yml"), E2E_WORKFLOW).unwrap();
                std::fs::write(workflows.join("ci.yml"), CI_WORKFLOW).unwrap();
                std::fs::write(workflows.join("deploy.yml"), DEPLOY_WORKFLOW).unwrap();
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

/// The fake `bkt`: the pipelines of `api`, the steps and logs of its run #7.
fn bkt_script() -> Value {
    json!({
        "contexts": [
            {"name": "cloud", "host": "bitbucket.org", "active": true},
            {"name": "work", "host": "bitbucket.example.com:7990", "active": false}
        ],
        "pipelines": {"acme/api": [
            {"uuid": "{0000-8}", "build_number": 8,
             "state": {"name": "IN_PROGRESS"},
             "trigger": {"name": "MANUAL"},
             "created_on": "2026-11-01T10:00:00.000000+00:00",
             "target": {"type": "pipeline_ref_target", "ref_type": "branch",
                "ref_name": "app/login", "selector": {"type": "custom", "pattern": "nightly"}}},
            {"uuid": "{0000-7}", "build_number": 7,
             "state": {"name": "COMPLETED", "result": {"name": "FAILED"}},
             "trigger": {"name": "PUSH"},
             "created_on": "2026-11-01T09:00:00.000000+00:00", "duration_in_seconds": 245,
             "target": {"type": "pipeline_ref_target", "ref_type": "branch",
                "ref_name": "app/login", "selector": {"type": "default"}}},
            {"uuid": "{0000-6}", "build_number": 6,
             "state": {"name": "COMPLETED", "result": {"name": "SUCCESSFUL"}},
             "trigger": {"name": "PUSH"},
             "created_on": "2026-11-01T08:00:00.000000+00:00", "duration_in_seconds": 100,
             "target": {"type": "pipeline_ref_target", "ref_type": "branch",
                "ref_name": "main", "selector": {"type": "branches", "pattern": "main"}}}
        ]},
        "steps": {"acme/api": {"7": [
            step("{step-seed}", "seed", "COMPLETED", "FAILED"),
            step("{step-test}", "test", "COMPLETED", "SUCCESSFUL"),
            step("{step-deploy}", "deploy", "PENDING", "")
        ]}},
        "logs": {"acme/api": {"7": {
            "{step-seed}": "+ ./seed\nconnecting with ghp_abcdefghijklmnopqrstuvwxyz0123456789\nerror: network rule\n",
            "{step-test}": "+ cargo test\nok\n"
        }}},
        "triggered": 43
    })
}

/// The fake `gh`: the runs of `web`, and its run 77 with its jobs.
fn gh_script() -> Value {
    json!({
        "runs": {"acme/web": [
            {"databaseId": 77, "workflowName": "E2E", "status": "completed",
             "conclusion": "failure", "event": "workflow_dispatch", "headBranch": "app/login",
             "createdAt": "2026-11-01T10:00:00Z", "startedAt": "2026-11-01T10:00:00Z",
             "updatedAt": "2026-11-01T10:03:00Z", "url": "https://github.com/acme/web/actions/runs/77"},
            {"databaseId": 70, "workflowName": "CI", "status": "completed",
             "conclusion": "success", "event": "push", "headBranch": "main",
             "createdAt": "2026-11-01T09:00:00Z", "startedAt": "2026-11-01T09:00:00Z",
             "updatedAt": "2026-11-01T09:01:00Z", "url": "https://github.com/acme/web/actions/runs/70"}
        ]},
        "run_views": {"acme/web": {"77": {
            "databaseId": 77, "workflowName": "E2E", "status": "completed",
            "conclusion": "failure", "url": "https://github.com/acme/web/actions/runs/77",
            "jobs": [
                {"databaseId": 501, "name": "e2e", "status": "completed", "conclusion": "failure"},
                {"databaseId": 502, "name": "lint", "status": "completed", "conclusion": "success"}
            ]
        }}},
        "logs": {"acme/web": {"501": "e2e\tRun\t2026-11-01T10:01:00.0000000Z login failed with ghp_abcdefghijklmnopqrstuvwxyz0123456789\n"}},
        "dispatched": 78
    })
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

/// The number of the request a held call waits on.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// A call the gate holds, allowed by the user: the answer the agent is handed for it, an error
/// when it was not done.
fn allowed(user: &User, answer: Answer) -> Answer {
    let number = held(&answer);
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

/// A read of `forge`, which the gate lets through in every mode.
fn read(mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("forge", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
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

fn bkt_calls(user: &User) -> Vec<String> {
    calls(&user.bkt_received())
}

fn gh_calls(user: &User) -> Vec<String> {
    calls(&user.gh_received())
}

/// The calls of `bkt` that start pipelines or act on one.
fn bkt_writes(user: &User) -> Vec<String> {
    bkt_calls(user)
        .into_iter()
        .filter(|call| call.starts_with("pipeline run") || call.contains("--method POST"))
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

/// Every byte the store holds, its journal included.
fn stored(user: &User) -> String {
    let folder = user.path("state/flottille");
    let mut all = Vec::new();
    for entry in std::fs::read_dir(&folder).unwrap().flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("flottille.db")
        {
            all.extend(std::fs::read(entry.path()).unwrap_or_default());
        }
    }
    String::from_utf8_lossy(&all).into_owned()
}

#[test]
fn the_pipelines_of_a_bitbucket_branch_list_its_custom_ones() {
    let (user, _, mcp) = setup();

    let text = read(&mcp, json!({"action": "pipelines", "repository": "api"}));

    for line in [
        "- default",
        "- branches: main",
        "- pull-requests: **",
        "- custom: nightly",
        "- custom: deploy-preview, variables: PREVIEW_SEED (default 42), PREVIEW_JOBS (one of 1, 4)",
    ] {
        assert!(text.contains(line), "{line:?} in {text}");
    }
    assert!(text.contains("app/login"), "{text}");
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_calls(&user));
}

#[test]
fn the_workflows_that_can_be_dispatched_are_listed_on_github() {
    let (_user, _, mcp) = setup();

    let text = read(&mcp, json!({"action": "pipelines", "repository": "web"}));

    assert!(
        text.contains(
            "- E2E (.github/workflows/e2e.yml), inputs: suite (required, one of smoke, full; \
             Which suite), debug (boolean, default false)"
        ),
        "{text}"
    );
    assert!(
        text.contains("- deploy.yml (.github/workflows/deploy.yml)"),
        "{text}"
    );
    assert!(!text.contains("ci.yml"), "{text}");
}

#[test]
fn the_recent_runs_of_a_branch_are_listed() {
    let (user, _, mcp) = setup();

    let text = read(&mcp, json!({"action": "runs", "repository": "api"}));
    assert!(
        text.contains("- #8 custom: nightly, in progress, manual, started 2026-11-01T10:00:00"),
        "{text}"
    );
    assert!(
        text.contains(
            "- #7 default, failed, push, started 2026-11-01T09:00:00.000000+00:00, 4 min 5 s"
        ),
        "{text}"
    );
    // Of the workspace's branch: main's is not.
    assert!(!text.contains("#6"), "{text}");
    assert!(
        bkt_calls(&user).contains(
            &"api /repositories/acme/api/pipelines/ --context cloud --param pagelen=50 --param \
              sort=-created_on"
                .to_owned()
        ),
        "{:?}",
        bkt_calls(&user)
    );

    let text = read(&mcp, json!({"action": "runs", "repository": "web"}));
    assert!(
        text.contains("- 77 E2E, failure, workflow_dispatch, started 2026-11-01T10:00:00Z, 3 min"),
        "{text}"
    );
    assert!(
        gh_calls(&user).iter().any(|call| call
            .starts_with("run list -R github.com/acme/web --branch app/login --limit 20 --json")),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_custom_bitbucket_pipeline_runs_on_the_workspace_branch() {
    let (user, _, mcp) = setup();

    let answer = write(
        &user,
        &mcp,
        json!({"action": "run", "repository": "api", "pipeline": "custom: nightly"}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_writes(&user),
        vec![
            "pipeline run --context cloud --workspace acme --repo api --ref app/login \
             --selector-type custom --selector-pattern nightly"
                .to_owned()
        ]
    );
    assert!(
        answer.text.contains("custom: nightly") && answer.text.contains("app/login"),
        "{}",
        answer.text
    );
}

#[test]
fn a_github_workflow_runs_with_its_inputs() {
    let (user, _, mcp) = setup();

    let answer = write(
        &user,
        &mcp,
        json!({"action": "run", "repository": "web", "pipeline": "e2e.yml",
            "variables": [{"key": "suite", "value": "smoke"}, {"key": "debug", "value": "true"}]}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(
        gh_calls(&user).contains(
            &"workflow run e2e.yml -R github.com/acme/web --ref app/login -f suite=smoke -f \
              debug=true"
                .to_owned()
        ),
        "{:?}",
        gh_calls(&user)
    );
    assert!(
        answer
            .text
            .contains("https://github.com/acme/web/actions/runs/78"),
        "{}",
        answer.text
    );
}

#[test]
fn a_run_s_steps_and_a_step_s_log_are_read_masked() {
    let (user, _, mcp) = setup();

    let text = read(
        &mcp,
        json!({"action": "run_view", "repository": "api", "run": 7}),
    );
    for line in [
        "- seed: completed, failed",
        "- test: completed, successful",
        "- deploy: pending",
    ] {
        assert!(text.contains(line), "{line:?} in {text}");
    }

    let text = read(
        &mcp,
        json!({"action": "run_logs", "repository": "api", "run": 7, "step": "seed"}),
    );
    assert!(text.contains("error: network rule"), "{text}");
    assert!(!text.contains("ghp_abcdef"), "{text}");
    assert!(text.contains("[masked]"), "{text}");
    assert!(
        bkt_calls(&user).contains(
            &"pipeline logs 7 --context cloud --workspace acme --repo api --step {step-seed}"
                .to_owned()
        ),
        "{:?}",
        bkt_calls(&user)
    );

    let text = read(
        &mcp,
        json!({"action": "run_view", "repository": "web", "run": 77}),
    );
    assert!(text.contains("- e2e (501): completed, failure"), "{text}");
    let text = read(
        &mcp,
        json!({"action": "run_logs", "repository": "web", "run": 77, "step": "e2e"}),
    );
    assert!(text.contains("login failed with [masked]"), "{text}");
    assert!(
        gh_calls(&user).contains(&"run view --job 501 -R github.com/acme/web --log".to_owned()),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_run_is_stopped() {
    let (user, _, mcp) = setup();

    let answer = write(
        &user,
        &mcp,
        json!({"action": "stop", "repository": "api", "run": 7}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_writes(&user),
        vec![
            "api /repositories/acme/api/pipelines/{0000-7}/stopPipeline --context cloud \
             --method POST"
                .to_owned()
        ]
    );

    let answer = write(
        &user,
        &mcp,
        json!({"action": "stop", "repository": "web", "run": 77}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert!(
        gh_calls(&user).contains(&"run cancel 77 -R github.com/acme/web".to_owned()),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_manual_step_is_started_and_a_step_run_again() {
    let (user, _, mcp) = setup();

    // GitHub runs one job again.
    let answer = write(
        &user,
        &mcp,
        json!({"action": "step", "repository": "web", "run": 77, "step": "e2e", "do": "rerun"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert!(
        gh_calls(&user).contains(&"run rerun 77 -R github.com/acme/web --job 501".to_owned()),
        "{:?}",
        gh_calls(&user)
    );

    // Where the forge allows neither, it is refused naming why, and nothing is asked of it.
    for (repository, run, verb, why) in [
        ("api", 7, "start", "Bitbucket Cloud's API has no call"),
        ("api", 7, "rerun", "Bitbucket Cloud's API has no call"),
        ("web", 77, "start", "GitHub has no manual step"),
    ] {
        let step = if repository == "api" { "deploy" } else { "e2e" };
        let answer = write(
            &user,
            &mcp,
            json!({"action": "step", "repository": repository, "run": run, "step": step,
                "do": verb}),
        );
        assert!(answer.error, "{}", answer.text);
        assert!(answer.text.contains(why), "{why:?} in {}", answer.text);
    }
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_calls(&user));
}

#[test]
fn a_custom_deploy_pipeline_runs_with_its_variables_in_full_without_asking() {
    let (user, id, mcp) = setup();
    set_mode(&user, &id, "full");
    let secret = "s3cr3t-preview-value";

    let answer = mcp.call(
        "forge",
        json!({"action": "run", "repository": "api", "pipeline": "custom: deploy-preview",
        "variables": [
            {"key": "PREVIEW_SEED", "value": "7"},
            {"key": "PREVIEW_JOBS", "value": "1,4"},
            {"key": "PREVIEW_TOKEN", "value": secret, "secured": true}
        ]}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(!answer.text.contains("Waiting"), "{}", answer.text);
    assert!(answer.text.contains("#43"), "{}", answer.text);
    assert!(!answer.text.contains(secret), "{}", answer.text);
    let posted: Vec<Value> = user
        .bkt_received()
        .into_iter()
        .filter(|call| {
            call["args"][0] == "api" && call["args"][1] == "/repositories/acme/api/pipelines/"
        })
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|a| a == "--input")?;
            serde_json::from_str(args.get(at + 1)?.as_str()?).ok()
        })
        .collect();
    assert_eq!(posted.len(), 1, "{:?}", bkt_calls(&user));
    assert_eq!(
        posted[0],
        json!({"target": {"type": "pipeline_ref_target", "ref_type": "branch",
            "ref_name": "app/login", "selector": {"type": "custom", "pattern": "deploy-preview"}},
        "variables": [
            {"key": "PREVIEW_SEED", "value": "7", "secured": false},
            {"key": "PREVIEW_JOBS", "value": "1,4", "secured": false},
            {"key": "PREVIEW_TOKEN", "value": secret, "secured": true}
        ]})
    );
    assert!(
        bkt_calls(&user)
            .iter()
            .any(|call| call.contains("--context cloud --method POST")),
        "{:?}",
        bkt_calls(&user)
    );
    // The secured value is in no record of the call.
    assert!(
        !stored(&user).contains(secret),
        "the store holds the secured value"
    );
}

#[test]
fn running_a_pipeline_goes_through_the_gate_like_any_forge_write() {
    let (user, _, mcp) = setup();

    // The reads pass in every mode.
    read(&mcp, json!({"action": "pipelines", "repository": "api"}));
    read(&mcp, json!({"action": "runs", "repository": "api"}));
    // Every write is held for the user, and nothing runs before they allow it.
    for args in [
        json!({"action": "run", "repository": "api", "pipeline": "default"}),
        json!({"action": "stop", "repository": "api", "run": 7}),
        json!({"action": "step", "repository": "web", "run": 77, "step": "e2e", "do": "rerun"}),
    ] {
        let answer = mcp.call("forge", args);
        held(&answer);
    }
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_calls(&user));
    assert!(
        !gh_calls(&user)
            .iter()
            .any(|call| call.starts_with("run rerun")),
        "{:?}",
        gh_calls(&user)
    );

    let answer = write(
        &user,
        &mcp,
        json!({"action": "run", "repository": "api", "pipeline": "default"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert!(
        bkt_writes(&user).contains(
            &"pipeline run --context cloud --workspace acme --repo api --ref app/login".to_owned()
        ),
        "{:?}",
        bkt_calls(&user)
    );
}

#[test]
fn a_data_center_repository_is_refused_for_pipelines_naming_why() {
    let (user, _, mcp) = setup();

    for action in ["pipelines", "runs", "run_view"] {
        let answer = mcp.call(
            "forge",
            json!({"action": action, "repository": "dc", "run": 3}),
        );
        assert!(answer.error, "{action}: {}", answer.text);
        assert!(
            answer
                .text
                .contains("Bitbucket Data Center has no pipelines"),
            "{action}: {}",
            answer.text
        );
    }
    let answer = write(
        &user,
        &mcp,
        json!({"action": "run", "repository": "dc", "pipeline": "default"}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("Bitbucket Data Center has no pipelines"),
        "{}",
        answer.text
    );
    assert!(
        !bkt_calls(&user).iter().any(|call| call.contains("APP")),
        "{:?}",
        bkt_calls(&user)
    );
}

/// A realistic `bitbucket-pipelines.yml`, as projects write them: anchors and aliases, merge keys,
/// `definitions` that are not pipelines, block scalars, comments with colons in them.
const ANCHORED_PIPELINES: &str = "\
# Build: test, then ship. Note: the deploy ones are manual.
image: atlassian/default-image:4

definitions:
  caches:
    cargo: ~/.cargo
  services:
    postgres:
      image: postgres:16
      variables:
        POSTGRES_DB: app
  steps:
    - step: &build
        name: Build and test
        caches: [cargo]
        services: [postgres]
        script:
          - cargo build --release
          - |
            if [ -n \"$CI\" ]; then
              echo \"ci: yes\"
            fi
          - >
            cargo test
            --workspace
    - step: &deploy
        name: Deploy
        deployment: staging
        trigger: manual
        script:
          - ./deploy.sh \"$TARGET\"
  inputs: &deploy_inputs
    variables:
      - name: TARGET
        default: staging
        allowed-values: [staging, production]
        description: Where to deploy
      - name: TAG
  shared: &shared
    deploy-again:
      - <<: *deploy_inputs
      - step: *deploy

pipelines:
  default:
    - step: *build
  branches:
    main:
      - step:
          <<: *build
          name: Build main
  pull-requests:
    '**':
      - step: *build
  custom:
    <<: *shared
    nightly:
      - step: *build
    deploy:
      - <<: *deploy_inputs
      - step: *deploy
";

/// The `api` repository's `branch` holds `text` as its `bitbucket-pipelines.yml`, as last fetched
/// from its remote, the way a branch of the user's Bitbucket does.
fn api_holds(user: &User, branch: &str, text: &str) {
    let dir = user.path("work/app/api");
    std::fs::write(dir.join("bitbucket-pipelines.yml"), text).unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "pipelines"]);
    user.git(
        &dir,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{branch}"),
            "HEAD",
        ],
    );
}

#[test]
fn a_bitbucket_file_with_anchors_and_merge_keys_lists_its_custom_pipelines() {
    let (user, _, mcp) = setup();
    api_holds(&user, "feature/x", ANCHORED_PIPELINES);

    let text = read(
        &mcp,
        json!({"action": "pipelines", "repository": "api", "branch": "feature/x"}),
    );

    for line in [
        "- default",
        "- branches: main",
        "- pull-requests: **",
        "- custom: nightly",
        "- custom: deploy-again, variables: ",
        "- custom: deploy, variables: ",
    ] {
        assert!(text.contains(line), "{line:?} in {text}");
    }
    assert!(!text.contains("<<"), "a merge key is no pipeline: {text}");
    assert!(
        !text.contains("definitions") && !text.contains("deploy_inputs"),
        "{text}"
    );
}

#[test]
fn a_custom_pipeline_from_definitions_lists_its_variables() {
    let (user, _, mcp) = setup();
    api_holds(&user, "feature/x", ANCHORED_PIPELINES);

    let text = read(
        &mcp,
        json!({"action": "pipelines", "repository": "api", "branch": "feature/x"}),
    );

    let want = "TARGET (default staging, one of staging, production, Where to deploy), TAG";
    for name in ["deploy", "deploy-again"] {
        assert!(
            text.contains(&format!("- custom: {name}, variables: {want}\n")),
            "{name} in {text}"
        );
    }
}

#[test]
fn a_large_realistic_file_is_read_whole() {
    let (user, _, mcp) = setup();
    let mut text = ANCHORED_PIPELINES.to_owned();
    for n in 0..60 {
        text.push_str(&format!(
            "    release-{n}:\n      - variables: *deploy_vars\n      - step:\n          <<: *deploy_base\n          name: Release {n}\n          script:\n            - echo \"release: {n}\"\n",
        ));
    }
    let text = text.replace(
        "      - name: TAG\n",
        "      - name: TAG\n  vars: &deploy_vars\n    - name: ROUND\n  base: &deploy_base\n    script: [./deploy.sh]\n",
    );
    assert!(text.lines().count() > 300, "{}", text.lines().count());
    api_holds(&user, "feature/x", &text);

    let listed = read(
        &mcp,
        json!({"action": "pipelines", "repository": "api", "branch": "feature/x"}),
    );

    assert!(
        listed.contains("- custom: release-59, variables: ROUND\n"),
        "{listed}"
    );
    assert!(
        listed.contains("- custom: deploy, variables: TARGET"),
        "{listed}"
    );
}

#[test]
fn a_file_that_cannot_be_parsed_says_why_and_where() {
    let (user, _, mcp) = setup();
    api_holds(
        &user,
        "feature/x",
        "pipelines:\n  default:\n    - step:\n        name: Deploy: prod\n        script: [a]\n",
    );

    let answer = mcp.call(
        "forge",
        json!({"action": "pipelines", "repository": "api", "branch": "feature/x"}),
    );

    assert!(answer.error, "{}", answer.text);
    for part in [
        "bitbucket-pipelines.yml",
        "cannot be parsed",
        "mapping values are not allowed",
        "line 4 column 21",
        "origin/feature/x",
    ] {
        assert!(answer.text.contains(part), "{part:?} in {}", answer.text);
    }
}

#[test]
fn a_missing_file_says_it_is_missing() {
    let (user, _, mcp) = setup();
    let dir = user.path("work/app/api");
    user.git(&dir, &["rm", "--quiet", "bitbucket-pipelines.yml"]);
    user.git(&dir, &["commit", "--quiet", "-m", "no pipelines"]);
    user.git(
        &dir,
        &["update-ref", "refs/remotes/origin/feature/x", "HEAD"],
    );

    let answer = mcp.call(
        "forge",
        json!({"action": "pipelines", "repository": "api", "branch": "feature/x"}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("has no bitbucket-pipelines.yml")
            && answer.text.contains("is not in the commit")
            && answer.text.contains("origin/feature/x"),
        "{}",
        answer.text
    );
    assert!(
        !answer.text.contains("cannot be parsed"),
        "a missing file is not a broken one: {}",
        answer.text
    );
}

#[test]
fn a_merge_key_that_cannot_be_applied_is_said_and_the_rest_is_listed() {
    let (user, _, mcp) = setup();
    api_holds(
        &user,
        "feature/x",
        "definitions:\n  list: &list [a, b]\npipelines:\n  custom:\n    broken:\n      - step:\n          <<: *list\n    fine:\n      - step: {script: [a]}\n",
    );

    let text = read(
        &mcp,
        json!({"action": "pipelines", "repository": "api", "branch": "feature/x"}),
    );

    assert!(text.contains("- custom: fine"), "{text}");
    assert!(text.contains("- custom: broken"), "{text}");
    assert!(
        text.contains("merge key") && text.contains("mapping"),
        "the merge that failed is said: {text}"
    );
}

#[test]
fn a_broken_workflow_is_named_and_the_others_are_listed() {
    let (user, _, mcp) = setup();
    let dir = user.path("work/app/web");
    std::fs::write(
        dir.join(".github/workflows/broken.yml"),
        "on: workflow_dispatch\njobs:\n  x:\n    name: a: b\n",
    )
    .unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "broken"]);
    user.git(
        &dir,
        &["update-ref", "refs/remotes/origin/feature/x", "HEAD"],
    );

    let text = read(
        &mcp,
        json!({"action": "pipelines", "repository": "web", "branch": "feature/x"}),
    );

    assert!(text.contains("- E2E (.github/workflows/e2e.yml)"), "{text}");
    assert!(
        text.contains("- deploy.yml (.github/workflows/deploy.yml)"),
        "{text}"
    );
    assert!(
        text.contains(".github/workflows/broken.yml")
            && text.contains("cannot be parsed")
            && text.contains("line 4"),
        "the workflow that cannot be read is named: {text}"
    );
}

#[test]
fn a_custom_pipeline_from_an_anchored_file_runs_through_bkt() {
    let (user, _, mcp) = setup();
    api_holds(&user, "feature/x", ANCHORED_PIPELINES);

    let answer = write(
        &user,
        &mcp,
        json!({"action": "run", "repository": "api", "branch": "feature/x",
            "pipeline": "custom: nightly"}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_writes(&user),
        vec![
            "pipeline run --context cloud --workspace acme --repo api --ref feature/x \
             --selector-type custom --selector-pattern nightly"
                .to_owned()
        ]
    );
}
