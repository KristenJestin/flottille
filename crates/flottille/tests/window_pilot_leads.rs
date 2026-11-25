//! A pilot sums up and lists the sessions it leads (#1452): its row's second line says the summary
//! of `flottille_core::leads` and its led rows are folded by default; its page's Leads section
//! lists each led session with its ticket, workspace, pull request, checks and runs, and a launch
//! of its own held for the user is a row until it starts. On GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent and whose forge is the fake `gh`. No window
//! opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::leads::{
    lead_item_id, lead_place_id, lead_pull_id, lead_row_id, lead_runs_id, lead_ticket_id,
    lead_waits_id, leads_id,
};
use flottille_window::link::Connect;
use flottille_window::tree::{
    session_id, session_place_id, session_summary_id, session_summary_part_id,
};
use gpui_design_system::omarchy::Environment;
use gpui_design_system::toggle_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

/// A user whose fake agent runs `turns`.
fn user(turns: Value) -> User {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    user.fake_script(json!({ "turns": turns }));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The project `atlas`, one repository with a remote.
fn atlas(user: &User) {
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
}

/// The pilot the user starts in `project` with `message`: its id once its launch is held.
fn pilot(user: &User, project: &str, message: &str) -> (String, u64) {
    let lead = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the launch is held", || !requests(user).is_empty());
    let number = requests(user)[0]["number"].as_u64().unwrap();
    (lead, number)
}

/// Allows the held launch `number` of `lead`: the id of the session it starts.
fn allow(user: &User, lead: &str, number: u64) -> String {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the led session exists", || {
        agents(user).iter().any(|a| a["led_by"] == lead)
    });
    agents(user)
        .into_iter()
        .find(|a| a["led_by"] == lead)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A pilot in `atlas` that leads the session `Docs`: their ids.
fn docs(user: &User) -> (String, String) {
    atlas(user);
    let (lead, number) = pilot(user, "atlas", "begin the docs session");
    let led = allow(user, &lead, number);
    (lead, led)
}

const DOCS: &str = "begin the docs session";

fn launch_docs() -> Value {
    json!({"when": DOCS, "do": [{"mcp": {"tool": "agent_launch",
        "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]})
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
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

fn label(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn present(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

/// How many frames the window asks for before it rests, one step of 16 ms each.
fn frames_until_rest(cx: &mut VisualTestContext) -> usize {
    let mut frames = 0;
    for _ in 0..60 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            break;
        }
        frames += 1;
    }
    frames
}

/// The ids of the rows of the project's level, in the order drawn.
fn listed(shell: &Entity<Shell>, project: &str, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.project(project)
            .map(|node| {
                node.read(cx)
                    .sessions(cx)
                    .iter()
                    .map(|row| row.read(cx).id().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Opens the page of `session` from its row.
fn open(shell: &Entity<Shell>, session: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(session_id(session), cx));
    until(shell, cx, "its page", |cx| {
        shell
            .read_with(cx, |s, cx| {
                s.main()
                    .read(cx)
                    .session_page()
                    .map(|page| page.read(cx).id().to_owned())
            })
            .as_deref()
            == Some(session)
    });
}

#[gpui_kit::test]
fn a_pilot_s_row_sums_up_the_sessions_it_leads(cx: &mut TestAppContext) {
    let user = user(json!([launch_docs(), {"when": "Write the docs", "do": [{"hang": true}]}]));
    let (lead, _led) = docs(&user);
    let (shell, cx) = window(&user, cx);

    until(&shell, cx, "the pilot's summary", |cx| {
        label(cx, session_summary_id(&lead)).as_deref() == Some("leads 1")
    });
    // Its second line: where it works, then the summary, whose one part is the count.
    assert_eq!(label(cx, session_place_id(&lead)).as_deref(), Some("main"));
    assert_eq!(
        label(cx, session_summary_part_id(&lead, "leads")).as_deref(),
        Some("leads 1")
    );
    assert!(!present(cx, session_summary_part_id(&lead, "needs")));
    assert!(!present(cx, session_summary_part_id(&lead, "red")));
    // The place comes before the summary, on the same line.
    let (place, summary) = cx.update(|window, _| {
        (
            window.find(session_place_id(&lead)).bounds(),
            window.find(session_summary_id(&lead)).bounds(),
        )
    });
    assert!(place.right() <= summary.left(), "{place:?} {summary:?}");
    // Its first line keeps its own title: the summary speaks for what it leads.
    assert_eq!(label(cx, session_id(&lead)).as_deref(), Some(DOCS));
}

#[gpui_kit::test]
fn a_pilot_s_led_rows_are_folded_by_default(cx: &mut TestAppContext) {
    let user = user(json!([launch_docs(), {"when": "Write the docs", "do": [{"hang": true}]}]));
    let (lead, led) = docs(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the pilot's summary", |cx| {
        label(cx, session_summary_id(&lead)).is_some()
    });
    settle(cx);

    assert_eq!(listed(&shell, "atlas", cx), std::slice::from_ref(&lead));
    assert!(!present(cx, session_id(&led)), "the led row is folded");

    // Unfolded by the user, its led row sits under it.
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    settle(cx);
    assert_eq!(listed(&shell, "atlas", cx), [lead.clone(), led.clone()]);
}

/// The project `app` of one repository on GitHub `acme/api`, its issues the tracker, and its
/// ready workspace `login`, linked to the ticket #5.
fn app(user: &User) {
    let folder = user.path("work/app");
    let dir = folder.join("api");
    user.git_init(&dir);
    std::fs::write(dir.join("README.md"), "api\n").unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/api.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
    user.git(&dir, &["push", "--quiet", "origin", "main"]);
    user.git(
        &dir,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}],
            "tracker": {"kind": "github", "repository": "api"}}),
    )
    .unwrap();
}

fn branch(user: &User) -> String {
    user.call("workspace.list", json!({"project": "app"}))
        .unwrap()["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "login")
        .unwrap()["branch"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn issue(number: u64, title: &str) -> Value {
    json!({
        "number": number, "title": title, "state": "OPEN", "body": "What it is.",
        "url": format!("https://github.com/acme/api/issues/{number}"),
        "createdAt": "2026-10-01T10:00:00Z", "updatedAt": "2999-01-01T00:00:00Z",
        "comments": []
    })
}

/// The pull request #12 of `acme/api` on `branch`, its one check failed.
fn red_pull(branch: &str) -> Value {
    json!({
        "number": 12, "title": "Fix the login", "state": "OPEN", "isDraft": false,
        "headRefName": branch, "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": "https://github.com/acme/api/pull/12",
        "author": {"login": "someone"}, "assignees": [], "labels": [],
        "updatedAt": "2999-02-01T00:00:02Z",
        "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
        "statusCheckRollup": [{"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
            "conclusion": "FAILURE",
            "detailsUrl": "https://github.com/acme/api/actions/runs/900/job/build"}],
        "comments": [], "reviews": [], "body": ""
    })
}

/// The fake `gh` answers the ticket #5, the red pull request on `login`'s branch and the run 900
/// in progress, and a round of the items reads them.
fn forge(user: &User) {
    let branch = branch(user);
    user.gh_script(json!({
        "issues": {"acme/api": {"5": issue(5, "The login fails")}},
        "pulls": {"acme/api": {branch.clone(): red_pull(&branch)}},
        "runs": {"acme/api": []},
        "run_views": {"acme/api": {"900": {
            "databaseId": 900, "workflowName": "CI", "status": "in_progress", "conclusion": "",
            "url": "https://github.com/acme/api/actions/runs/900",
            "startedAt": "2026-11-01T11:00:00Z", "updatedAt": "2026-11-01T11:02:05Z",
            "headBranch": branch,
            "jobs": [{"databaseId": 901, "name": "build", "status": "in_progress", "conclusion": ""}]
        }}}
    }));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

#[gpui_kit::test]
fn a_pilot_that_leads_nothing_shows_what_it_is_about(cx: &mut TestAppContext) {
    let user = user(json!([]));
    app(&user);
    forge(&user);
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "login", "ticket": "#5"}),
    )
    .unwrap();
    let id = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "fix the login form"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, cx) = window(&user, cx);

    until(&shell, cx, "the row says its ticket", |cx| {
        label(cx, session_id(&id)).as_deref() == Some("The login fails")
    });
    settle(cx);
    assert!(
        !present(cx, session_summary_id(&id)),
        "a session that leads nothing has no summary"
    );
}

#[gpui_kit::test]
fn the_summary_changes_when_a_led_session_needs_the_user(cx: &mut TestAppContext) {
    let user = user(json!([
        launch_docs(),
        {"when": "Write the docs", "do": [{"say": "Ready."}]},
        {"when": "Proofread now", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Proofs", "brief": "Proofread the docs", "kind": "handed"}}}]}
    ]));
    let (lead, led) = docs(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the pilot's summary", |cx| {
        label(cx, session_summary_id(&lead)).as_deref() == Some("leads 1")
    });
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing asks for a frame while the summary rests"
    );

    // The led session asks the user for a launch: the pilot's summary says it needs them.
    user.call(
        "agent.send",
        json!({"agent": led, "message": "Proofread now"}),
    )
    .unwrap();
    until(&shell, cx, "the summary says it needs you", |cx| {
        label(cx, session_summary_id(&lead)).as_deref() == Some("leads 1 · 1 needs you")
    });
    assert_eq!(
        label(cx, session_summary_part_id(&lead, "needs")).as_deref(),
        Some("1 needs you")
    );
    // The words do not swap at once: they fade back in over a few frames, then rest.
    let frames = frames_until_rest(cx);
    assert!(frames >= 4, "the words fade over HOVER, {frames} frames");
}

#[gpui_kit::test]
fn the_pilot_s_page_lists_each_led_session_with_ticket_workspace_pull_request_checks_and_runs(
    cx: &mut TestAppContext,
) {
    let user = user(json!([
        {"when": "lead the login work", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Login", "brief": "Fix the login form", "kind": "led",
                "workspace": "login"}}}]},
        {"when": "Fix the login form", "do": [
            {"mcp": {"tool": "forge", "arguments": {"action": "follow_run", "repository": "api",
                "run": 900}}},
            {"say": "Following the run."}]}
    ]));
    app(&user);
    forge(&user);
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "login", "ticket": "#5"}),
    )
    .unwrap();
    let (lead, number) = pilot(&user, "app", "lead the login work");
    let led = allow(&user, &lead, number);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the pilot's row", |cx| {
        present(cx, session_id(&lead))
    });
    open(&shell, &lead, cx);

    until(
        &shell,
        cx,
        "the led session's row of Leads, its run followed",
        |cx| label(cx, lead_runs_id(&led)).as_deref() == Some("run 900"),
    );
    until(&shell, cx, "its pull request", |cx| {
        label(cx, lead_pull_id(&led)).as_deref() == Some("#12, checks failing")
    });
    assert_eq!(label(cx, leads_id()).as_deref(), Some("Leads 1"));
    // Its ticket, key and title; its workspace and what it runs on.
    assert_eq!(
        label(cx, lead_row_id(&led)).as_deref(),
        Some("The login fails")
    );
    assert_eq!(label(cx, lead_ticket_id(&led)).as_deref(), Some("#5"));
    let place = label(cx, lead_place_id(&led)).unwrap_or_default();
    assert!(place.starts_with("login"), "{place}");
    let waits = label(cx, lead_waits_id(&led)).unwrap_or_default();
    assert!(!waits.is_empty(), "it says what it waits for");
    // The pilot's row counts its red check.
    until(&shell, cx, "the pilot's summary", |cx| {
        label(cx, session_summary_id(&lead)).as_deref() == Some("leads 1 · 1 red check")
    });
    // Leads comes under the header, before the conversation.
    let (header, section) = cx.update(|window, _| {
        (
            window.find("session-header".to_owned()).bounds(),
            window.find(leads_id()).bounds(),
        )
    });
    assert!(
        header.bottom() <= section.top() + px(1.),
        "{header:?} {section:?}"
    );

    // A click opens the led session.
    settle(cx);
    cx.update(|window, cx| window.click(lead_row_id(&led), cx));
    until(&shell, cx, "the led session's page", |cx| {
        shell
            .read_with(cx, |s, cx| {
                s.main()
                    .read(cx)
                    .session_page()
                    .map(|page| page.read(cx).id().to_owned())
            })
            .as_deref()
            == Some(led.as_str())
    });
}

/// The heights the element `id` is drawn at, one frame of 16 ms after another, while the window
/// asks for frames: none once it is gone.
fn heights(cx: &mut VisualTestContext, id: &gpui_kit::ElementId) -> Vec<f32> {
    let mut seen = Vec::new();
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        let height = cx.update(|window, _| {
            window
                .try_find(id.clone())
                .map(|found| f32::from(found.bounds().size.height))
        });
        seen.push(height.unwrap_or(0.0));
    }
    seen
}

#[gpui_kit::test]
fn a_held_launch_is_a_row_of_leads_until_it_starts(cx: &mut TestAppContext) {
    let user = user(json!([launch_docs(), {"when": "Write the docs", "do": [{"hang": true}]}]));
    atlas(&user);
    let (lead, number) = pilot(&user, "atlas", DOCS);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the pilot's row", |cx| {
        present(cx, session_id(&lead))
    });
    open(&shell, &lead, cx);

    let held = format!("held-{number}");
    until(&shell, cx, "the held launch's row", |cx| {
        label(cx, lead_row_id(&held)).as_deref() == Some("Docs")
    });
    settle(cx);
    assert_eq!(label(cx, leads_id()).as_deref(), Some("Leads 1"));
    assert_eq!(
        label(cx, lead_place_id(&held)).as_deref(),
        Some("launch held")
    );
    assert_eq!(
        label(cx, lead_waits_id(&held)).as_deref(),
        Some("needs you: allow its launch")
    );
    let item = lead_item_id(&held);
    let full = cx.update(|window, _| f32::from(window.find(item.clone()).bounds().size.height));

    // Allowed, it starts: its row folds away and the session's unfolds.
    let led = allow(&user, &lead, number);
    until(&shell, cx, "the led session's row", |cx| {
        present(cx, lead_row_id(&led))
    });
    let going = heights(cx, &item);
    assert!(
        going.iter().any(|&h| h > 0.0 && h < full),
        "the held row folds away: {going:?}"
    );
    assert_eq!(*going.last().unwrap(), 0.0, "it ends gone: {going:?}");
    assert_eq!(label(cx, leads_id()).as_deref(), Some("Leads 1"));
    assert_eq!(label(cx, lead_row_id(&led)).as_deref(), Some("Docs"));
    assert!(
        cx.update(|window, _| window.find(lead_item_id(&led)).bounds().size.height) > px(0.),
        "the session's row unfolded"
    );
}
