//! The Usage tab of Settings (`docs/usage.md`): on GPUI's test platform in front of a real
//! daemon, whose pull requests come from the fake `gh` and whose turns are rows written in its
//! own store. No window opens on a screen, and no agent runs.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_core::agent::{TurnUsage, Used};
use flottille_core::store::{Store, now_ms};
use flottille_window::Shell;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The project `app` of one repository, `api`, on GitHub, with two workspaces: `login`, whose
/// pull request was merged, and `old`, removed with none. `login` used 1,000 weighted tokens and
/// `old` 3,000: the cost per merged task is 4,000.
fn user() -> User {
    let user = User::new().with_fake_gh().with_fetcher();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
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
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": "api",
            "forge": {"kind": "github", "repository": "acme/api"}}]}),
    )
    .unwrap();
    let mut branch = String::new();
    for name in ["login", "old"] {
        let made = user
            .call(
                "workspace.create",
                json!({"project": "app", "name": name, "offline": true}),
            )
            .unwrap();
        if name == "login" {
            branch = made["branch"].as_str().unwrap().to_owned();
        }
    }
    record(&user, &[("login", 1000), ("old", 3000)]);
    user.call("workspace.remove", json!({"project": "app", "name": "old"}))
        .unwrap();
    let mut pulls = serde_json::Map::new();
    pulls.insert(branch.clone(), pull(&branch));
    user.gh_script(json!({"pulls": {"acme/api": pulls}}));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    user
}

fn pull(head: &str) -> Value {
    json!({
        "number": 1, "title": "Login", "state": "MERGED", "isDraft": false,
        "headRefName": head, "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": "https://github.com/acme/api/pull/1",
        "author": {"login": "someone"}, "assignees": [], "labels": [],
        "updatedAt": "2020-01-01T00:00:00Z", "mergeable": "MERGEABLE",
        "reviewDecision": "APPROVED", "statusCheckRollup": [], "comments": [], "reviews": []
    })
}

/// One turn per workspace, its input alone, as having just ended.
fn record(user: &User, turns: &[(&str, u64)]) {
    let now = now_ms();
    let rows: Vec<TurnUsage> = turns
        .iter()
        .map(|(workspace, input)| TurnUsage {
            agent: "root00000001".into(),
            title: None,
            session: "root00000001".into(),
            project: "app".into(),
            workspace: Some((*workspace).into()),
            provider: "claude".into(),
            role: "lead".into(),
            started_at: now,
            ended_at: now,
            stop_reason: "end_turn".into(),
            tuning: String::new(),
            used: Used {
                model: Some("model-a".into()),
                input: Some(*input),
                output: Some(0),
                cache_read: Some(0),
                cache_write: Some(0),
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

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

#[gpui_kit::test]
fn the_usage_tab_shows_the_cost_per_merged_task(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        cx.update(|window, _| window.try_find("settings-tabs-usage").is_some())
    });

    cx.update(|window, cx| window.click("settings-tabs-usage", cx));

    until(&shell, cx, "the cost per merged task is shown", |cx| {
        label(cx, "usage-cost").as_deref() == Some("4.0k weighted tokens")
    });
    assert_eq!(label(cx, "usage-merged").as_deref(), Some("1"));
    assert_eq!(label(cx, "usage-failed").as_deref(), Some("1"));
    assert_eq!(label(cx, "usage-in-progress").as_deref(), Some("0"));
    // Today's bar, and the model that worked.
    let today = jiff::Zoned::now().date().to_string();
    assert_eq!(
        label(cx, &format!("usage-day-{today}")),
        Some(format!("{today}: 4.0k"))
    );
    assert!(
        cx.update(|window, _| window.try_find("usage-models-list-model-a").is_some()),
        "the model's row"
    );
}
