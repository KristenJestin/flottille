//! The statistics of what agents used (`docs/usage.md`): weighted tokens, and the cost of a task
//! that ends merged. The turns are rows written in the test's own store; the workspaces are made
//! and removed by the daemon, and their pull requests are read through the fake `gh`. No agent
//! runs here.

mod common;

use common::User;
use flottille_core::agent::{TurnUsage, Used};
use flottille_core::store::{Store, now_ms};
use serde_json::{Value, json};

/// A user whose daemon reads pull requests through the fake `gh`.
fn user() -> User {
    User::new().with_fake_gh().with_fetcher()
}

/// A project `app` of one repository, `api`, whose `origin` is on GitHub; its forge declared when
/// `forge` is.
fn project(user: &User, forge: bool) {
    let dir = user.path("work/app/api");
    user.git_init(&dir);
    std::fs::write(dir.join("README.md"), "api\n").unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &dir,
        &["remote", "add", "origin", "git@github.com:acme/api.git"],
    );
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
    if forge {
        user.call(
            "project.set_settings",
            json!({"name": "app", "repositories": [{"path": "api",
                "forge": {"kind": "github", "repository": "acme/api"}}]}),
        )
        .unwrap();
    }
}

/// Makes the workspace `name` of `app`, without fetching: its branch.
fn workspace(user: &User, name: &str) -> String {
    let made = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": name, "offline": true}),
        )
        .unwrap();
    made["branch"]
        .as_str()
        .unwrap_or_else(|| panic!("{made}"))
        .to_owned()
}

fn remove(user: &User, name: &str) {
    user.call("workspace.remove", json!({"project": "app", "name": name}))
        .unwrap();
}

/// What `gh pr list --json` answers of the pull request `number` of `acme/api`, on `head`.
fn pull(number: u64, head: &str, state: &str) -> Value {
    json!({
        "number": number, "title": "Work", "state": state, "isDraft": false,
        "headRefName": head, "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": format!("https://github.com/acme/api/pull/{number}"),
        "author": {"login": "someone"}, "assignees": [], "labels": [],
        "updatedAt": "2020-01-01T00:00:00Z", "mergeable": "MERGEABLE",
        "reviewDecision": "APPROVED", "statusCheckRollup": [], "comments": [], "reviews": []
    })
}

/// The fake `gh` lists these pull requests of `acme/api`, and the daemon reads them.
fn pulls(user: &User, pulls: &[Value]) {
    let by_head: serde_json::Map<String, Value> = pulls
        .iter()
        .map(|p| (p["headRefName"].as_str().unwrap().to_owned(), p.clone()))
        .collect();
    user.gh_script(json!({"pulls": {"acme/api": by_head}}));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

/// One turn of one model, as the daemon records it.
struct Turn<'a> {
    agent: &'a str,
    session: &'a str,
    workspace: Option<&'a str>,
    model: &'a str,
    tuning: &'a str,
    /// Input, cache write, cache read, output.
    counts: (Option<u64>, Option<u64>, Option<u64>, Option<u64>),
}

impl Default for Turn<'_> {
    fn default() -> Self {
        Turn {
            agent: "root00000001",
            session: "root00000001",
            workspace: None,
            model: "model-a",
            tuning: "",
            counts: (Some(0), Some(0), Some(0), Some(0)),
        }
    }
}

/// A turn that weighs `weighted` tokens: its input alone.
fn weighing(weighted: u64) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
    (Some(weighted), Some(0), Some(0), Some(0))
}

/// Writes the turns in the store, as having just ended.
fn record(user: &User, turns: &[Turn]) {
    let now = now_ms();
    let rows: Vec<TurnUsage> = turns
        .iter()
        .map(|turn| TurnUsage {
            agent: turn.agent.into(),
            title: None,
            session: turn.session.into(),
            project: "app".into(),
            workspace: turn.workspace.map(Into::into),
            provider: "claude".into(),
            role: "lead".into(),
            started_at: now,
            ended_at: now,
            stop_reason: "end_turn".into(),
            tuning: turn.tuning.into(),
            used: Used {
                model: Some(turn.model.into()),
                input: turn.counts.0,
                cache_write: turn.counts.1,
                cache_read: turn.counts.2,
                output: turn.counts.3,
                reasoning: None,
                cost_usd: None,
            },
        })
        .collect();
    Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .record_turn_usage(&rows)
        .unwrap();
}

/// `usage.stats` of every project over yesterday, today and tomorrow, by `by`.
fn stats(user: &User, by: &str) -> Value {
    let today = jiff::Zoned::now().date();
    user.call(
        "usage.stats",
        json!({"since": today.yesterday().unwrap().to_string(),
            "until": today.tomorrow().unwrap().to_string(), "by": by}),
    )
    .unwrap()
}

fn tasks(user: &User) -> Value {
    stats(user, "model")["total"]["tasks"].clone()
}

#[test]
fn a_turn_s_weighted_tokens_follow_the_written_weights() {
    let user = User::new();
    project(&user, false);
    record(
        &user,
        &[
            Turn {
                counts: (Some(1000), Some(200), Some(10_000), Some(300)),
                ..Turn::default()
            },
            // A count the agent did not give weighs nothing, and the row says it was missing.
            Turn {
                counts: (Some(40), None, Some(100), None),
                ..Turn::default()
            },
        ],
    );

    let total = stats(&user, "model")["total"].clone();

    // 1000 + 1.25 × 200 + 0.1 × 10000 + 5 × 300, then 40 + 0.1 × 100.
    assert_eq!(total["weighted"], 3750.0 + 50.0, "{total}");
    assert_eq!(total["input"], 1040, "{total}");
    assert_eq!(total["cache_write"], 200, "{total}");
    assert_eq!(total["cache_read"], 10_100, "{total}");
    assert_eq!(total["output"], 300, "{total}");
    assert_eq!(total["turns"], 2, "{total}");
    assert_eq!(total["missing"], 1, "{total}");
    // What cache reads weigh, of the whole.
    assert_eq!(total["cache_read_share"], 1010.0 / 3800.0, "{total}");
}

#[test]
fn a_task_s_cost_sums_its_whole_tree() {
    let user = user();
    project(&user, true);
    workspace(&user, "login");
    let login = Some("login");
    record(
        &user,
        &[
            // The root, its child, and a session it leads: one tree, one workspace.
            Turn {
                workspace: login,
                counts: weighing(100),
                ..Turn::default()
            },
            Turn {
                agent: "child0000001",
                workspace: login,
                counts: weighing(20),
                ..Turn::default()
            },
            Turn {
                agent: "led000000001",
                session: "led000000001",
                workspace: login,
                counts: weighing(3),
                ..Turn::default()
            },
            // In `main`: in the totals, in no task.
            Turn {
                counts: weighing(5000),
                ..Turn::default()
            },
        ],
    );

    let tasks = tasks(&user);

    assert_eq!(
        tasks["in_progress"],
        json!({"count": 1, "weighted": 123.0}),
        "{tasks}"
    );
    assert_eq!(tasks["main"], 5000.0, "{tasks}");
}

#[test]
fn a_merged_workspace_is_a_success_and_a_removed_one_a_failure() {
    let user = user();
    project(&user, true);
    let merged = workspace(&user, "login");
    workspace(&user, "old");
    let wip = workspace(&user, "wip");
    record(
        &user,
        &[
            Turn {
                workspace: Some("login"),
                counts: weighing(1),
                ..Turn::default()
            },
            Turn {
                workspace: Some("old"),
                counts: weighing(10),
                ..Turn::default()
            },
            Turn {
                workspace: Some("wip"),
                counts: weighing(100),
                ..Turn::default()
            },
        ],
    );
    remove(&user, "old");
    // A closed pull request is no success.
    pulls(
        &user,
        &[pull(1, &merged, "MERGED"), pull(2, &wip, "CLOSED")],
    );

    let tasks = tasks(&user);

    assert_eq!(
        tasks["merged"],
        json!({"count": 1, "weighted": 1.0}),
        "{tasks}"
    );
    assert_eq!(
        tasks["failed"],
        json!({"count": 1, "weighted": 10.0}),
        "{tasks}"
    );
    assert_eq!(
        tasks["in_progress"],
        json!({"count": 1, "weighted": 100.0}),
        "{tasks}"
    );
}

#[test]
fn a_failed_task_s_cost_counts_against_the_cost_per_merged_task() {
    let user = user();
    project(&user, true);
    let first = workspace(&user, "first");
    let second = workspace(&user, "second");
    workspace(&user, "lost");
    workspace(&user, "wip");
    record(
        &user,
        &[
            Turn {
                workspace: Some("first"),
                counts: weighing(1000),
                ..Turn::default()
            },
            Turn {
                workspace: Some("second"),
                counts: weighing(2000),
                ..Turn::default()
            },
            Turn {
                workspace: Some("lost"),
                counts: weighing(3000),
                ..Turn::default()
            },
            Turn {
                workspace: Some("wip"),
                counts: weighing(50_000),
                ..Turn::default()
            },
            Turn {
                counts: weighing(70_000),
                ..Turn::default()
            },
        ],
    );
    remove(&user, "lost");
    pulls(
        &user,
        &[pull(1, &first, "MERGED"), pull(2, &second, "MERGED")],
    );

    let tasks = tasks(&user);

    // (1000 + 2000 + 3000) ÷ 2: the task in progress and `main` are left out.
    assert_eq!(tasks["cost_per_merged_task"], 3000.0, "{tasks}");
}

#[test]
fn a_project_without_a_forge_is_left_out_of_the_ratio() {
    let user = user();
    project(&user, false);
    workspace(&user, "login");
    workspace(&user, "old");
    record(
        &user,
        &[
            Turn {
                workspace: Some("login"),
                counts: weighing(7),
                ..Turn::default()
            },
            Turn {
                workspace: Some("old"),
                counts: weighing(30),
                ..Turn::default()
            },
        ],
    );
    remove(&user, "old");

    let tasks = tasks(&user);

    // Removed or not, a task with no forge to say has no known outcome: its cost alone.
    assert_eq!(
        tasks["no_forge"],
        json!({"count": 2, "weighted": 37.0}),
        "{tasks}"
    );
    assert_eq!(tasks["failed"]["count"], 0, "{tasks}");
    assert_eq!(tasks["cost_per_merged_task"], Value::Null, "{tasks}");
}

#[test]
fn usage_stats_splits_by_tuning() {
    let user = user();
    project(&user, true);
    let with = workspace(&user, "with");
    let without = workspace(&user, "without");
    record(
        &user,
        &[
            Turn {
                workspace: Some("with"),
                tuning: "unchanged",
                counts: weighing(400),
                ..Turn::default()
            },
            Turn {
                workspace: Some("without"),
                counts: weighing(900),
                ..Turn::default()
            },
            Turn {
                tuning: "unchanged",
                counts: weighing(5),
                ..Turn::default()
            },
        ],
    );
    pulls(
        &user,
        &[pull(1, &with, "MERGED"), pull(2, &without, "MERGED")],
    );

    let groups = stats(&user, "tuning")["groups"].clone();

    let keys: Vec<&Value> = groups
        .as_array()
        .unwrap()
        .iter()
        .map(|g| &g["key"])
        .collect();
    assert_eq!(keys, [&json!(""), &json!("unchanged")], "{groups}");
    let (off, on) = (&groups[0], &groups[1]);
    assert_eq!(off["weighted"], 900.0, "{groups}");
    assert_eq!(off["tasks"]["cost_per_merged_task"], 900.0, "{groups}");
    assert_eq!(on["weighted"], 405.0, "{groups}");
    assert_eq!(on["tasks"]["cost_per_merged_task"], 400.0, "{groups}");
    assert_eq!(on["tasks"]["main"], 5.0, "{groups}");
}

#[test]
fn usage_by_day_gives_every_day_of_the_period() {
    let user = User::new();
    project(&user, false);
    record(
        &user,
        &[Turn {
            counts: weighing(12),
            ..Turn::default()
        }],
    );

    let groups = stats(&user, "day")["groups"].clone();

    let today = jiff::Zoned::now().date();
    let days: Vec<(Value, Value)> = groups
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["key"].clone(), g["weighted"].clone()))
        .collect();
    assert_eq!(
        days,
        [
            (json!(today.yesterday().unwrap().to_string()), json!(0.0)),
            (json!(today.to_string()), json!(12.0)),
            (json!(today.tomorrow().unwrap().to_string()), json!(0.0)),
        ]
    );
}

#[test]
fn flottille_usage_json_says_the_tasks_of_the_last_days() {
    let user = User::new();
    project(&user, false);
    record(
        &user,
        &[Turn {
            counts: weighing(12),
            ..Turn::default()
        }],
    );

    let output = user.run(&["usage", "--since", "7d", "--json"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let said: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(said["by"], "day", "{said}");
    assert_eq!(said["groups"].as_array().unwrap().len(), 7, "{said}");
    assert_eq!(said["total"]["tasks"]["main"], 12.0, "{said}");
}
