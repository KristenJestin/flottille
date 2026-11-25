//! A pilot sums up and lists the sessions it leads (#1452): its row's second line says the summary
//! of `flottille_core::leads` and its led rows are folded by default; its page's Leads section
//! lists each led session with its ticket, workspace, pull request, checks and runs, and a launch
//! of its own held for the user is a row until it starts. On GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent and whose forge is the fake `gh`. No window
//! opens on a screen.

use crate::common;
use crate::kit_common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::leads::{
    Leads, LedLine, lead_item_id, lead_place_id, lead_pull_id, lead_row_id, lead_runs_id,
    lead_ticket_id, lead_waits_id, leads_id,
};
use flottille_window::link::Connect;
use flottille_window::session_link::PullRequest;
use flottille_window::state::AgentState;
use flottille_window::tree::{
    SessionRow, session_id, session_place_id, session_summary_id, session_summary_part_id,
    session_waits_id,
};
use gpui_design_system::omarchy::Environment;
use gpui_design_system::seen::seen;
use gpui_design_system::toggle_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, WeakEntity, Window, div, px, size,
};
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

/// A user whose fake agent runs `turns` and offers two models, `fake-small` and `fake-large`.
fn user_with_models(turns: Value) -> User {
    let user = user(json!([]));
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "model_rows": {
            "fake-small": {"name": "Fake Small 5.5"},
            "fake-large": {"name": "Fake Large 5.5"}
        },
        "efforts": ["low", "medium", "high"],
        "turns": turns
    }));
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

/// The element `id`, which the window draws: a short message when it does not, as the kit's own
/// lists every path the window registered.
fn found(
    window: &gpui_kit::Window,
    id: impl Into<gpui_kit::ElementId>,
) -> gpui_kit::test::ElementSnapshot {
    let id = id.into();
    window
        .try_find(id.clone())
        .unwrap_or_else(|| panic!("{id:?} is not drawn"))
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
            found(window, session_place_id(&lead)).bounds(),
            found(window, session_summary_id(&lead)).bounds(),
        )
    });
    assert!(place.right() <= summary.left(), "{place:?} {summary:?}");
    // Its first line keeps its own title: the summary speaks for what it leads.
    assert_eq!(label(cx, session_id(&lead)).as_deref(), Some(DOCS));
}

#[gpui_kit::test]
fn a_pilot_s_led_rows_are_folded_by_default(cx: &mut TestAppContext) {
    // The led session asks the user to allow a launch of its own: it needs them.
    let user = user(json!([
        launch_docs(),
        {"when": "Write the docs", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Proofs", "brief": "Proofread the docs", "kind": "handed"}}}]}
    ]));
    let (lead, led) = docs(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the pilot's summary says it needs you", |cx| {
        label(cx, session_summary_id(&lead)).as_deref() == Some("leads 1 · 1 needs you")
    });
    settle(cx);
    assert_eq!(listed(&shell, "atlas", cx), std::slice::from_ref(&lead));
    assert!(!present(cx, session_id(&led)), "the led row is folded");
    // Folded, its mark is the most urgent of what it hides, as before.
    let mark = shell.read_with(cx, |s, cx| {
        s.session(&lead).and_then(|row| row.read(cx).mark())
    });
    assert_eq!(mark, Some(AgentState::NeedsYou));

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
    // The words do not swap at once: they fade back in, partway, then rest whole.
    let words = ElementId::Name(format!("session-summary-words-{lead}").into());
    let fading = opacities(&words, cx);
    assert!(
        fading.iter().any(|&o| o > 0.0 && o < 1.0),
        "the words fade back in: {fading:?}"
    );
    assert_eq!(fading.last().copied(), Some(1.0), "then rest: {fading:?}");
}

/// The opacities the kit notes for `id` as the next frames draw, one of 16 ms after another.
fn opacities(id: &ElementId, cx: &mut VisualTestContext) -> Vec<f32> {
    let mut seen_at = vec![seen(id.clone()).unwrap_or(0.0)];
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        seen_at.push(seen(id.clone()).unwrap_or(0.0));
    }
    seen_at
}

#[gpui_kit::test]
fn the_pilot_s_page_lists_each_led_session_with_ticket_workspace_pull_request_checks_and_runs(
    cx: &mut TestAppContext,
) {
    let user = user_with_models(json!([
        {"when": "lead the login work", "do": [
            {"mcp": {"tool": "agent_launch",
                "arguments": {"title": "Login", "brief": "Fix the login form", "kind": "led",
                    "workspace": "login", "model": "fake-large", "effort": "high"}}},
            {"mcp": {"tool": "agent_launch",
                "arguments": {"title": "Notes", "brief": "Write the release notes",
                    "kind": "led"}}}]},
        {"when": "Fix the login form", "do": [
            {"mcp": {"tool": "forge", "arguments": {"action": "follow_run", "repository": "api",
                "run": 900}}},
            {"say": "Following the run."}]},
        {"when": "Write the release notes", "do": [{"hang": true}]}
    ]));
    app(&user);
    forge(&user);
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "login", "ticket": "#5"}),
    )
    .unwrap();
    let lead = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "lead the login work"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Both launches are held: the user allows them in the order they were asked.
    eventually("both launches are held", || requests(&user).len() == 2);
    let mut numbers: Vec<u64> = requests(&user)
        .iter()
        .map(|r| r["number"].as_u64().unwrap())
        .collect();
    numbers.sort();
    let led = allow(&user, &lead, numbers[0]);
    user.call("approval.approve", json!({"number": numbers[1]}))
        .unwrap();
    eventually("the second led session exists", || {
        agents(&user).iter().filter(|a| a["led_by"] == lead).count() == 2
    });
    let notes = agents(&user)
        .into_iter()
        .find(|a| a["led_by"] == lead && a["id"] != led)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
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
    until(&shell, cx, "it waits for its run", |cx| {
        label(cx, lead_waits_id(&led)).as_deref() == Some("waits for run 900")
    });
    assert_eq!(label(cx, leads_id()).as_deref(), Some("Leads 2"));
    // Its ticket, key and title; its workspace and what it runs on.
    assert_eq!(
        label(cx, lead_row_id(&led)).as_deref(),
        Some("The login fails")
    );
    assert_eq!(label(cx, lead_ticket_id(&led)).as_deref(), Some("#5"));
    assert_eq!(
        label(cx, lead_place_id(&led)).as_deref(),
        Some("login · Fake Large 5.5 · high")
    );
    // The other, working in the project's folder, with no pull request yet.
    until(&shell, cx, "the second led session works", |cx| {
        label(cx, lead_waits_id(&notes)).as_deref() == Some("working")
    });
    assert_eq!(
        label(cx, lead_pull_id(&notes)).as_deref(),
        Some("no pull request yet")
    );
    // In launch order.
    settle(cx);
    let (first, second) = cx.update(|window, _| {
        (
            found(window, lead_row_id(&led)).bounds(),
            found(window, lead_row_id(&notes)).bounds(),
        )
    });
    assert!(first.bottom() <= second.top(), "{first:?} {second:?}");
    // The pilot's row counts its red check.
    until(&shell, cx, "the pilot's summary", |cx| {
        label(cx, session_summary_id(&lead)).as_deref() == Some("leads 2 · 1 red check")
    });
    // Leads comes under the header, before the conversation.
    let (header, section) = cx.update(|window, _| {
        (
            found(window, "session-header".to_owned()).bounds(),
            found(window, leads_id()).bounds(),
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
    let full = cx.update(|window, _| f32::from(found(window, item.clone()).bounds().size.height));

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
    // The session's row says its title as the sidebar does: the first line of its brief.
    assert_eq!(
        label(cx, lead_row_id(&led)).as_deref(),
        Some("Write the docs")
    );
    assert!(
        cx.update(|window, _| found(window, lead_item_id(&led)).bounds().size.height) > px(0.),
        "the session's row unfolded"
    );
}

#[gpui_kit::test]
fn the_summary_counts_the_red_check_its_leads_row_shows(cx: &mut TestAppContext) {
    let user = user(json!([
        {"when": "lead the login work", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Login", "brief": "Fix the login form", "kind": "led",
                "workspace": "login"}}}]},
        {"when": "Fix the login form", "do": [{"hang": true}]}
    ]));
    app(&user);
    // The forge has the red pull request, but no round of the items read it: the tree's lines
    // say no pull request of the session yet.
    let branch = branch(&user);
    user.gh_script(json!({
        "issues": {"acme/api": {}},
        "pulls": {"acme/api": {branch.clone(): red_pull(&branch)}},
        "runs": {"acme/api": []}
    }));
    let (lead, number) = pilot(&user, "app", "lead the login work");
    let led = allow(&user, &lead, number);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the pilot's summary", |cx| {
        label(cx, session_summary_id(&lead)).is_some()
    });
    // The project's page reads the workspace's pull request from the forge itself.
    shell.update_in(cx, |shell, window, cx| {
        shell.open(
            flottille_window::shell::Selection::Project("app".into()),
            window,
            cx,
        )
    });
    for _ in 0..40 {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        settle(cx);
        std::thread::sleep(Duration::from_millis(25));
    }
    open(&shell, &lead, cx);
    until(&shell, cx, "the led session's row of Leads", |cx| {
        present(cx, lead_pull_id(&led))
    });
    settle(cx);
    // What the pilot counts is what its Leads row says, as every client's summary.
    assert_eq!(
        label(cx, lead_pull_id(&led)).as_deref(),
        Some("no pull request yet")
    );
    assert_eq!(
        label(cx, session_summary_id(&lead)).as_deref(),
        Some("leads 1")
    );
}

#[gpui_kit::test]
fn a_pilot_leading_many_sessions_keeps_its_conversation_in_view(cx: &mut TestAppContext) {
    // Eight launches of led sessions, each held for the user: eight rows of Leads.
    let launches: Vec<Value> = (1..=8)
        .map(|n| {
            json!({"mcp": {"tool": "agent_launch", "arguments": {"title": format!("Part {n}"),
                "brief": format!("Write part {n}"), "kind": "led"}}})
        })
        .collect();
    let user = user(json!([{"when": "begin the parts", "do": launches}]));
    atlas(&user);
    let lead = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": "begin the parts"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the eight launches are held", || requests(&user).len() == 8);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the pilot's row", |cx| {
        present(cx, session_id(&lead))
    });
    open(&shell, &lead, cx);
    until(&shell, cx, "the eight rows of Leads", |cx| {
        label(cx, leads_id()).as_deref() == Some("Leads 8")
    });
    settle(cx);

    // The card scrolls past a few rows: the conversation, between it and the composer, keeps
    // most of the page, and the composer stays in the window.
    let (leads, composer) = cx.update(|window, _| {
        (
            found(window, leads_id()).bounds(),
            found(window, "reply".to_owned()).bounds(),
        )
    });
    // Its title and the card, capped at about four rows of the eight.
    assert!(leads.size.height <= px(340.), "{leads:?}");
    assert!(
        composer.top() - leads.bottom() >= px(250.),
        "the conversation stays in view: {leads:?} {composer:?}"
    );
    assert!(composer.bottom() <= px(800.), "{composer:?}");
}

/// The Leads section of a page, alone, on a page as wide as a narrow window's.
struct LeadsView(Leads);

impl Render for LeadsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(640.))
            .child(self.0.section(WeakEntity::new_invalid(), window, cx))
    }
}

/// A row of Leads of the session `k1`, working in `place`.
fn line(place: &str, pull: Option<u64>, runs: &[&str]) -> LedLine {
    LedLine {
        key: "k1".into(),
        session: Some("k1".into()),
        mark: Some(AgentState::Running),
        ticket: None,
        title: "Docs".into(),
        place: Some(place.into()),
        machine: None,
        known: true,
        pull: pull.map(|number| PullRequest {
            repository: "api".into(),
            number,
            state: "open".into(),
            mark: None,
            url: String::new(),
        }),
        runs: runs.iter().map(|run| run.to_string()).collect(),
        waits: "working".into(),
    }
}

fn leads_view(
    lines: Vec<LedLine>,
    cx: &mut TestAppContext,
) -> (Entity<LeadsView>, &mut VisualTestContext) {
    kit_common::init(cx, kit_common::Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| LeadsView(Leads::default()));
    set_lines(&view, lines, cx);
    settle(cx);
    (view, cx)
}

fn set_lines(view: &Entity<LeadsView>, lines: Vec<LedLine>, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| {
        view.0.set(lines, cx);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
}

/// Whether the opacities of a fade went partway and ended whole.
fn faded(fading: &[f32]) -> bool {
    fading.iter().any(|&o| o > 0.0 && o < 1.0) && fading.last() == Some(&1.0)
}

#[gpui_kit::test]
fn the_words_of_a_row_of_leads_fade_as_they_change(cx: &mut TestAppContext) {
    let (view, cx) = leads_view(vec![line("login · Fake Small 5.5", None, &["run 900"])], cx);
    let id = |name: &str| ElementId::Name(format!("session-lead-{name}-change-k1").into());

    // What it runs on changes.
    set_lines(
        &view,
        vec![line("login · Fake Large 5.5", None, &["run 900"])],
        cx,
    );
    let fading = opacities(&id("place"), cx);
    assert!(faded(&fading), "the place and model fade: {fading:?}");
    assert_eq!(
        label(cx, lead_place_id("k1")).as_deref(),
        Some("login · Fake Large 5.5")
    );

    // Its pull request comes.
    set_lines(
        &view,
        vec![line("login · Fake Large 5.5", Some(12), &["run 900"])],
        cx,
    );
    let fading = opacities(&id("pull"), cx);
    assert!(faded(&fading), "the pull request's words fade: {fading:?}");
    assert_eq!(label(cx, lead_pull_id("k1")).as_deref(), Some("#12"));

    // It follows another run.
    set_lines(
        &view,
        vec![line(
            "login · Fake Large 5.5",
            Some(12),
            &["run 900", "run 901"],
        )],
        cx,
    );
    let fading = opacities(&id("runs"), cx);
    assert!(faded(&fading), "the runs fade: {fading:?}");
    assert_eq!(
        label(cx, lead_runs_id("k1")).as_deref(),
        Some("run 900 · run 901")
    );

    // It stops following them: the runs fade out rather than leave at once.
    set_lines(
        &view,
        vec![line("login · Fake Large 5.5", Some(12), &[])],
        cx,
    );
    let going = drawn(lead_runs_id("k1"), cx);
    assert!(going.first() == Some(&true), "the runs fade out: {going:?}");
    assert!(going.last() == Some(&false), "then leave: {going:?}");
}

/// Whether the element `id` is drawn, frame after frame of 16 ms.
fn drawn(id: impl Into<ElementId>, cx: &mut VisualTestContext) -> Vec<bool> {
    let id = id.into();
    let mut drawn = Vec::new();
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        drawn.push(present(cx, id.clone()));
    }
    drawn
}

#[gpui_kit::test]
fn the_leads_title_keeps_its_count_while_it_folds_away(cx: &mut TestAppContext) {
    let (view, cx) = leads_view(vec![line("login", None, &[])], cx);
    assert_eq!(label(cx, leads_id()).as_deref(), Some("Leads 1"));

    // A second row: the count's words fade to the new one.
    let mut second = line("main", None, &[]);
    second.key = "k2".into();
    second.session = Some("k2".into());
    set_lines(&view, vec![line("login", None, &[]), second], cx);
    let fading = opacities(&ElementId::Name("session-leads-count".into()), cx);
    assert!(faded(&fading), "the count fades: {fading:?}");
    assert_eq!(label(cx, leads_id()).as_deref(), Some("Leads 2"));

    // It leads nothing any more: the section folds away saying what it last said.
    set_lines(&view, Vec::new(), cx);
    let mut said = Vec::new();
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        said.extend(label(cx, leads_id()));
    }
    assert!(!said.is_empty(), "it folds away, not at once");
    assert!(
        said.iter().all(|said| said == "Leads 2"),
        "never a count of nothing: {said:?}"
    );
    assert!(!present(cx, leads_id()), "then it is gone");
}

/// A row of the sidebar alone, as wide as the sidebar.
struct RowView(Entity<SessionRow>);

impl Render for RowView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(280.)).child(self.0.clone())
    }
}

fn parts(needs: usize) -> Option<Vec<flottille_core::leads::Part>> {
    let led: Vec<flottille_core::leads::Led> = (0..2)
        .map(|at| flottille_core::leads::Led {
            needs_you: at < needs,
            red: false,
            blocked_by: Vec::new(),
        })
        .collect();
    flottille_core::leads::summary(&led, 0)
}

fn row_view(cx: &mut TestAppContext) -> (Entity<SessionRow>, &mut VisualTestContext) {
    kit_common::init(cx, kit_common::Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, cx| {
        RowView(cx.new(|cx| {
            let mut row = SessionRow::new("p1".into(), WeakEntity::new_invalid(), cx);
            row.set("Pilot", None, 0, 0, cx);
            row
        }))
    });
    settle(cx);
    let row = view.read_with(cx, |view, _| view.0.clone());
    (row, cx)
}

#[gpui_kit::test]
fn the_summary_s_words_fade_when_they_change(cx: &mut TestAppContext) {
    let (row, cx) = row_view(cx);
    row.update(cx, |row, cx| row.set_summary(parts(0), cx));
    settle(cx);
    assert_eq!(
        label(cx, session_summary_id("p1")).as_deref(),
        Some("leads 2")
    );

    row.update(cx, |row, cx| row.set_summary(parts(1), cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let fading = opacities(&ElementId::Name("session-summary-words-p1".into()), cx);
    assert!(faded(&fading), "the words fade back in: {fading:?}");
    assert_eq!(
        label(cx, session_summary_id("p1")).as_deref(),
        Some("leads 2 · 1 needs you")
    );
}

#[gpui_kit::test]
fn a_pilot_s_waits_fade_out_as_its_summary_comes(cx: &mut TestAppContext) {
    let (row, cx) = row_view(cx);
    row.update(cx, |row, cx| row.set_waits("waits for Docs", cx));
    settle(cx);
    assert_eq!(
        label(cx, session_waits_id("p1")).as_deref(),
        Some("waits for Docs")
    );

    // Its summary takes their place: the words fade out, never leave at once.
    row.update(cx, |row, cx| row.set_summary(parts(0), cx));
    let going = drawn(session_waits_id("p1"), cx);
    assert!(
        going.first() == Some(&true),
        "the waits fade out: {going:?}"
    );
    assert!(going.last() == Some(&false), "then leave: {going:?}");
}

#[gpui_kit::test]
fn the_title_and_ticket_of_a_row_of_leads_fade_as_they_come(cx: &mut TestAppContext) {
    let (view, cx) = leads_view(vec![line("login", None, &[])], cx);

    // Its ticket is read: its title and its key take the row's first line, fading in.
    let mut ticket = line("login", None, &[]);
    ticket.title = "The login fails".into();
    ticket.ticket = Some("#5".into());
    set_lines(&view, vec![ticket], cx);
    let mut widths = Vec::new();
    let mut fading = vec![seen(ElementId::Name("session-lead-k1-title".into())).unwrap_or(0.0)];
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        fading.push(seen(ElementId::Name("session-lead-k1-title".into())).unwrap_or(0.0));
        // The room it unfolds in, which clips it while it moves; its own once it rests.
        widths.push(cx.update(|window, _| {
            let id = lead_ticket_id("k1");
            window
                .try_find(ElementId::Name("session-lead-k1-tag-0-slot".into()))
                .or_else(|| window.try_find(id))
                .map_or(0.0, |found| f32::from(found.bounds().size.width))
        }));
    }
    assert!(faded(&fading), "the title fades: {fading:?}");
    let whole = *widths.last().unwrap();
    assert!(
        whole > 0.0 && widths.iter().any(|&w| w > 0.0 && w < whole),
        "the ticket's key unfolds: {widths:?}"
    );
    assert_eq!(
        label(cx, lead_row_id("k1")).as_deref(),
        Some("The login fails")
    );
}
