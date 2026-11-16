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
    /// How long before now it started, in milliseconds.
    ago: u64,
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
            ago: 0,
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
            started_at: now - turn.ago,
            ended_at: now - turn.ago,
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

const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

/// `usage.stats` of every project over today alone, by `by`.
fn today(user: &User, by: &str) -> Value {
    let today = jiff::Zoned::now().date().to_string();
    user.call(
        "usage.stats",
        json!({"since": today, "until": today, "by": by}),
    )
    .unwrap()
}

/// Moves the last event `name` of the workspace `workspace` to `ago` milliseconds before now: what
/// the daemon would have written then.
fn backdate(user: &User, name: &str, workspace: &str, ago: u64) {
    let store = rusqlite::Connection::open(user.path("state/flottille/flottille.db")).unwrap();
    let changed = store
        .execute(
            "UPDATE events SET at = ?1 WHERE seq = (SELECT max(seq) FROM events WHERE name = ?2 \
             AND json_extract(data, '$.workspace') = ?3)",
            rusqlite::params![now_ms() - ago, name, workspace],
        )
        .unwrap();
    assert_eq!(changed, 1, "no {name} of {workspace}");
}

/// The pull request, opened `ago` milliseconds before now.
fn opened(mut pull: Value, ago: u64) -> Value {
    let at = jiff::Timestamp::from_millisecond((now_ms() - ago) as i64).unwrap();
    pull["createdAt"] = json!(at.to_string());
    pull
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
    assert_eq!(total["rows"], 2, "{total}");
    assert_eq!(total["missing"], 1, "{total}");
    // The share of the input tokens, raw, that were read from the cache.
    assert_eq!(
        total["cache_read_share"],
        10_100.0 / (1040.0 + 10_100.0 + 200.0),
        "{total}"
    );
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

    let groups = stats(&user, "tuning")["groups"]["tuning"].clone();

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

    let groups = stats(&user, "day")["groups"]["day"].clone();

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
    assert_eq!(said["by"], json!(["day"]), "{said}");
    assert_eq!(said["groups"]["day"].as_array().unwrap().len(), 7, "{said}");
    assert_eq!(said["total"]["tasks"]["main"], 12.0, "{said}");
}

#[test]
fn a_reused_workspace_name_gives_the_merge_to_the_later_task_only() {
    let user = user();
    project(&user, true);
    workspace(&user, "fix-login");
    backdate(&user, "workspace.created", "fix-login", 3 * DAY);
    remove(&user, "fix-login");
    backdate(&user, "workspace.removed", "fix-login", 2 * DAY);
    let branch = workspace(&user, "fix-login");
    backdate(&user, "workspace.created", "fix-login", DAY);
    record(
        &user,
        &[
            Turn {
                workspace: Some("fix-login"),
                ago: 3 * DAY - HOUR,
                counts: weighing(400_000),
                ..Turn::default()
            },
            Turn {
                workspace: Some("fix-login"),
                ago: DAY - HOUR,
                counts: weighing(600_000),
                ..Turn::default()
            },
        ],
    );
    // Opened by the second task: the first one, removed before, merged nothing.
    pulls(&user, &[opened(pull(1, &branch, "MERGED"), HOUR)]);

    let tasks = stats(&user, "model")["total"]["tasks"].clone();
    let week = {
        let today = jiff::Zoned::now().date();
        user.call(
            "usage.stats",
            json!({"since": today.checked_sub(jiff::Span::new().days(6)).unwrap().to_string(),
                "until": today.to_string(), "by": "model"}),
        )
        .unwrap()["total"]["tasks"]
            .clone()
    };

    // Over the last days: the merged task brings its 600k, the failed one its 400k against it.
    assert_eq!(
        week["merged"],
        json!({"count": 1, "weighted": 600_000.0}),
        "{week}"
    );
    assert_eq!(
        week["failed"],
        json!({"count": 1, "weighted": 400_000.0}),
        "{week}"
    );
    assert_eq!(week["cost_per_merged_task"], 1_000_000.0, "{week}");
    // Since yesterday: the failure ended before, the merge in it.
    assert_eq!(tasks["failed"]["count"], 0, "{tasks}");
    assert_eq!(tasks["cost_per_merged_task"], 600_000.0, "{tasks}");
}

#[test]
fn a_merge_of_the_earlier_task_is_not_the_later_task_s() {
    let user = user();
    project(&user, true);
    let branch = workspace(&user, "fix-login");
    backdate(&user, "workspace.created", "fix-login", 3 * DAY);
    record(
        &user,
        &[Turn {
            workspace: Some("fix-login"),
            ago: 3 * DAY - HOUR,
            counts: weighing(400_000),
            ..Turn::default()
        }],
    );
    // Opened by the first task, before the name was made again.
    pulls(&user, &[opened(pull(1, &branch, "MERGED"), 2 * DAY)]);
    remove(&user, "fix-login");
    workspace(&user, "fix-login");
    record(
        &user,
        &[Turn {
            workspace: Some("fix-login"),
            counts: weighing(600_000),
            ..Turn::default()
        }],
    );
    pulls(&user, &[opened(pull(1, &branch, "MERGED"), 2 * DAY)]);

    let tasks = tasks(&user);

    assert_eq!(
        tasks["merged"],
        json!({"count": 1, "weighted": 400_000.0}),
        "{tasks}"
    );
    assert_eq!(
        tasks["in_progress"],
        json!({"count": 1, "weighted": 600_000.0}),
        "{tasks}"
    );
    assert_eq!(tasks["cost_per_merged_task"], 400_000.0, "{tasks}");
}

#[test]
fn a_task_that_started_before_the_period_brings_its_whole_cost() {
    let user = user();
    project(&user, true);
    let branch = workspace(&user, "login");
    backdate(&user, "workspace.created", "login", 3 * DAY);
    record(
        &user,
        &[
            Turn {
                workspace: Some("login"),
                ago: 3 * DAY - HOUR,
                counts: weighing(1000),
                ..Turn::default()
            },
            Turn {
                workspace: Some("login"),
                counts: weighing(200),
                ..Turn::default()
            },
        ],
    );
    pulls(&user, &[opened(pull(1, &branch, "MERGED"), 2 * DAY)]);

    let today = today(&user, "model");

    // The totals are the period's turns; the task merged in it, all of its own.
    assert_eq!(today["total"]["weighted"], 200.0, "{today}");
    let tasks = &today["total"]["tasks"];
    assert_eq!(
        tasks["merged"],
        json!({"count": 1, "weighted": 1200.0}),
        "{tasks}"
    );
    assert_eq!(tasks["cost_per_merged_task"], 1200.0, "{tasks}");
}

#[test]
fn a_task_is_in_one_tuning_group() {
    let user = user();
    project(&user, true);
    let mixed = workspace(&user, "mixed");
    let alone = workspace(&user, "alone");
    record(
        &user,
        &[
            Turn {
                workspace: Some("mixed"),
                counts: weighing(100),
                ..Turn::default()
            },
            Turn {
                workspace: Some("mixed"),
                tuning: "unchanged",
                counts: weighing(300),
                ..Turn::default()
            },
            Turn {
                workspace: Some("alone"),
                tuning: "unchanged",
                counts: weighing(50),
                ..Turn::default()
            },
        ],
    );
    pulls(
        &user,
        &[pull(1, &mixed, "MERGED"), pull(2, &alone, "MERGED")],
    );

    let stats = user
        .call(
            "usage.stats",
            json!({"since": jiff::Zoned::now().date().to_string(),
                "until": jiff::Zoned::now().date().to_string(), "by": ["tuning", "model"]}),
        )
        .unwrap();

    let groups: Vec<(Value, Value, Value)> = stats["groups"]["tuning"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["key"].clone(), g["weighted"].clone(), g["tasks"].clone()))
        .collect();
    let merged = |count: u64, weighted: f64| {
        json!({"merged": {"count": count, "weighted": weighted},
            "failed": {"count": 0, "weighted": 0.0},
            "cost_per_merged_task": if count == 0 { Value::Null } else { json!(weighted / count as f64) }})
    };
    // The tokens by what was on during each turn; each task once, in its own group.
    assert_eq!(
        groups,
        [
            (json!("unchanged"), json!(350.0), merged(1, 50.0)),
            (json!(""), json!(100.0), merged(0, 0.0)),
            (json!("mixed"), json!(0.0), merged(1, 400.0)),
        ],
        "{stats}"
    );
    // By model, the tokens alone: the ratio is the whole period's.
    assert_eq!(stats["groups"]["model"][0]["tasks"], Value::Null, "{stats}");
    assert_eq!(
        stats["total"]["tasks"]["cost_per_merged_task"], 225.0,
        "{stats}"
    );
}

/// A project `app` of two repositories: `api`, whose forge is declared, and `web`, without one.
fn two_repositories(user: &User) {
    for name in ["api", "web"] {
        let dir = user.path(&format!("work/app/{name}"));
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), format!("{name}\n")).unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    }
    user.git(
        &user.path("work/app/api"),
        &["remote", "add", "origin", "git@github.com:acme/api.git"],
    );
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
    forge(user, true);
}

/// Declares the forge of `api`, or takes it away.
fn forge(user: &User, declared: bool) {
    let api = match declared {
        true => json!({"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}),
        false => json!({"path": "api"}),
    };
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [api, {"path": "web"}]}),
    )
    .unwrap();
}

/// Makes the workspace `name` of `app` on `repositories` alone.
fn workspace_on(user: &User, name: &str, repositories: &[&str]) -> String {
    let made = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": name, "offline": true,
                "repositories": repositories}),
        )
        .unwrap();
    made["branch"]
        .as_str()
        .unwrap_or_else(|| panic!("{made}"))
        .to_owned()
}

#[test]
fn a_task_in_a_repository_without_a_forge_is_left_out_of_the_ratio_in_a_mixed_project() {
    let user = user();
    two_repositories(&user);
    let api = workspace_on(&user, "api-work", &["api"]);
    workspace_on(&user, "web-work", &["web"]);
    workspace_on(&user, "api-lost", &["api"]);
    record(
        &user,
        &[
            Turn {
                workspace: Some("api-work"),
                counts: weighing(1000),
                ..Turn::default()
            },
            Turn {
                workspace: Some("web-work"),
                counts: weighing(70),
                ..Turn::default()
            },
            Turn {
                workspace: Some("api-lost"),
                counts: weighing(500),
                ..Turn::default()
            },
        ],
    );
    remove(&user, "web-work");
    remove(&user, "api-lost");
    pulls(&user, &[pull(1, &api, "MERGED")]);

    let tasks = tasks(&user);

    // Removed in a repository no forge says anything of: no failure, its cost alone.
    assert_eq!(
        tasks["no_forge"],
        json!({"count": 1, "weighted": 70.0}),
        "{tasks}"
    );
    assert_eq!(
        tasks["failed"],
        json!({"count": 1, "weighted": 500.0}),
        "{tasks}"
    );
    assert_eq!(tasks["cost_per_merged_task"], 1500.0, "{tasks}");

    // The forge taken away later, its items forgotten: what was known stays.
    forge(&user, false);
    let after = self::tasks(&user);
    assert_eq!(after, tasks);
}
