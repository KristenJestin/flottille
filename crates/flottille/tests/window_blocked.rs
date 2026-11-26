//! A session blocked by someone else in the window (#1451, #1530, `docs/design.md` §10, "Blocked
//! by others"), on GPUI's test platform in front of a real daemon whose agents are the fake ACP
//! agent and whose forge is the fake `gh`: a root at rest whose subscription waits on someone else
//! stays a visible row of its project, slid to the foot of the project's sessions, and slides back
//! to its place when the wait ends; its page says the wait in the Workspace section, under its
//! ticket. No window opens on a screen.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::state::AgentState;
use flottille_window::tree::{
    session_id, session_key_beside_id, session_place_id, session_waits_id,
};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn user() -> User {
    let user = User::with_fake_agent().with_fake_gh();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A project `atlas` that is one repository on GitHub `acme/atlas`, its pull request #12 asking a
/// review of Maya.
fn project(user: &User) {
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/atlas.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
    user.git(
        &folder,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:acme/atlas.git",
        ],
    );
    user.call(
        "project.set_settings",
        json!({"name": "atlas", "repositories": [
            {"path": ".", "forge": {"kind": "github", "repository": "acme/atlas"}}
        ]}),
    )
    .unwrap();
    script(
        user,
        json!([{"__typename": "User", "login": "maya", "name": "Maya"}]),
    );
    a_round(user);
}

/// The fake `gh` answers the pull request #12, a review asked of `requested`: Maya's, given,
/// once it is asked of no one.
fn script(user: &User, requested: Value) {
    let reviews = match requested.as_array().is_some_and(Vec::is_empty) {
        true => json!([{"author": {"login": "maya"}, "state": "APPROVED", "body": "Fine"}]),
        false => json!([]),
    };
    answer(user, requested, reviews);
}

/// The fake `gh` answers the pull request #12, a review asked of `requested`, `reviews` given.
fn answer(user: &User, requested: Value, reviews: Value) {
    answer_on(user, "feat/docs", requested, reviews)
}

/// The same, the pull request #12 being the one of `branch`.
fn answer_on(user: &User, branch: &str, requested: Value, reviews: Value) {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    user.gh_script(json!({"pulls": {"acme/atlas": {branch: {
        "number": 12, "title": "Write the docs", "state": "OPEN", "isDraft": false,
        "headRefOid": "aaaaaaa1", "baseRefName": "main",
        "url": "https://github.com/acme/atlas/pull/12", "author": {"login": "someone"},
        "assignees": [], "labels": [], "reviewRequests": requested,
        "statusCheckRollup": [], "comments": [], "reviews": reviews,
        "updatedAt": format!("2999-01-01T00:{:02}:{:02}Z", n / 60 % 60, n % 60)
    }}}}));
}

/// A round of the project's items, as the daemon makes every few minutes.
fn a_round(user: &User) {
    user.call("items.refresh", json!({"project": "atlas"}))
        .unwrap();
}

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

fn line(user: &User, id: &str) -> Value {
    agents(user)
        .into_iter()
        .find(|agent| agent["id"] == id)
        .unwrap()
}

/// A session started in `atlas`, at rest after its first turn: its id and its MCP session.
fn start(user: &User, message: &str) -> (String, Mcp) {
    start_in(user, message, None)
}

/// The same, in the workspace `workspace` of the project when one is given.
fn start_in(user: &User, message: &str, workspace: Option<&str>) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let mut params = json!({"project": "atlas", "message": message});
    if let Some(workspace) = workspace {
        params["workspace"] = json!(workspace);
    }
    let id = user.call("agent.start", params).unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || line(user, &id)["state"] == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

/// The session subscribes to the review of #12, which waits on Maya: it is blocked by her.
fn wait_for_maya(user: &User, id: &str, mcp: &Mcp) {
    let answer = mcp.call(
        "forge",
        json!({"action": "subscribe", "number": 12, "events": ["review"]}),
    );
    assert!(!answer.error, "{}", answer.text);
    eventually("the session blocked", || {
        !line(user, id)["blocked"].is_null()
    });
}

/// Maya reviews: the round wakes the session, which takes a turn and is no longer blocked.
fn maya_reviews(user: &User, id: &str) {
    script(user, json!([]));
    a_round(user);
    eventually("the session no longer blocked", || {
        let line = line(user, id);
        line["blocked"].is_null() && line["state"] == "idle"
    });
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
    cx.simulate_resize(size(px(1280.), px(900.)));
    (shell, cx)
}

/// Polls the daemon until `check` holds, the clock of the animations standing still.
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

fn after(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        after(cx, Duration::from_millis(16));
    }
    cx.run_until_parked();
}

/// The ids of the rows of the project's level, in the order drawn.
fn listed(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.project("atlas")
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

/// The ids of the rows of the project's level that wait on someone else, in the order drawn.
fn blocked(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.project("atlas")
            .map(|node| {
                node.read(cx)
                    .sessions(cx)
                    .iter()
                    .filter(|row| row.read(cx).is_blocked())
                    .map(|row| row.read(cx).id().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn label(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Where the rows of the project are drawn.
const IN_PROJECT: &str = "fold-project-atlas";

/// Where a row of the project is drawn, `(top, height)` of its frame, `None` when it is not drawn:
/// the frame is what the list slides, rises and folds.
fn frame(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<(f32, f32)> {
    let row = shell.read_with(cx, |s, _| s.session(id).map(|row| row.entity_id()))?;
    let frame = gpui_kit::ElementId::Name(format!("tree-row-{}", row.as_u64()).into());
    cx.update(|window, _| {
        window.within(IN_PROJECT).try_find(frame).map(|found| {
            let bounds = found.bounds();
            (f32::from(bounds.origin.y), f32::from(bounds.size.height))
        })
    })
}

fn top(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> f32 {
    frame(shell, id, cx).expect("its row is drawn").0
}

/// Steps the window one frame at a time until the row has left `from`, and gives the top it is
/// drawn at then: a frame in the middle of its slide.
fn midway(shell: &Entity<Shell>, id: &str, from: f32, cx: &mut VisualTestContext) -> f32 {
    for _ in 0..60 {
        after(cx, Duration::from_millis(16));
        let now = top(shell, id, cx);
        if (now - from).abs() > 1. {
            return now;
        }
    }
    panic!("its row never moved from {from}");
}

/// The tracker is the project's issues: #5 is what the session takes in hand, assigned to
/// `assignees`.
fn with_issue(user: &User, assignees: Value) {
    user.call(
        "project.set_settings",
        json!({"name": "atlas",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/atlas"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    let mut script = serde_json::from_str::<Value>(
        &std::fs::read_to_string(user.path("gh/script.json")).unwrap(),
    )
    .unwrap();
    script["issues"] = json!({"acme/atlas": {"5": {
        "number": 5, "title": "The docs are missing", "state": "OPEN", "body": "What it is.",
        "url": "https://github.com/acme/atlas/issues/5", "createdAt": "2026-10-01T10:00:00Z",
        "updatedAt": "2999-01-01T00:00:00Z", "assignees": assignees, "labels": [], "comments": []
    }}});
    user.gh_script(script);
    a_round(user);
}

/// The session takes ticket #5 in hand (a comment, served at once in Full).
fn takes_the_ticket(user: &User, id: &str, mcp: &Mcp) {
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    let answer = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "#5", "body": "Taking it"}),
    );
    assert!(!answer.error, "{}", answer.text);
    eventually("its subject", || line(user, id)["subject"]["key"] == "#5");
}

/// The session subscribes to ticket #5, which waits on its assignee.
fn wait_for_the_assignee(user: &User, id: &str, mcp: &Mcp) {
    let answer = mcp.call(
        "tickets",
        json!({"action": "subscribe", "ticket": "#5", "events": ["commented", "status"]}),
    );
    assert!(!answer.error, "{}", answer.text);
    eventually("the session blocked", || {
        !line(user, id)["blocked"].is_null()
    });
}

/// The words of a wait as the Workspace line says them under the line of what it waits on, from
/// what the daemon says of it.
fn wait_said(user: &User, id: &str) -> String {
    wait_said_with(user, id, false)
}

/// The same; `what` when the wait is not under the line of what it waits on, which then says it.
fn wait_said_with(user: &User, id: &str, what: bool) -> String {
    let blocked = line(user, id)["blocked"].clone();
    let who: Vec<&str> = blocked["by"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let at = flottille_window::session::clock(blocked["since"].as_u64().unwrap());
    match what {
        true => format!(
            "Waits for {}: {} · since {at}",
            who.join(", "),
            blocked["why"].as_str().unwrap()
        ),
        false => format!("Waits for {} · since {at}", who.join(", ")),
    }
}

/// How high an element of the page is drawn, none while it is not.
fn drawn_height(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
}

/// Opens the session's page, the right panel with it.
fn open_page(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "its page", |cx| {
        label(cx, "session-title").is_some()
    });
}

/// Two sessions of the project, the one that waits on top of the other: the project lists the
/// newest first.
fn two_sessions(user: &User) -> (String, Mcp, String) {
    let (other, _) = start(user, "write the readme");
    let (id, mcp) = start(user, "open the docs pull request");
    (id, mcp, other)
}

#[gpui_kit::test]
fn a_blocked_session_moves_to_the_foot_of_its_project_and_stays_visible(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp, other) = two_sessions(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both rows", |cx| listed(&shell, cx).len() == 2);
    settle(cx);
    assert_eq!(listed(&shell, cx), [id.clone(), other.clone()]);
    let (before, full) = frame(&shell, &id, cx).expect("its row");

    wait_for_maya(&user, &id, &mcp);
    until(&shell, cx, "the row after the other", |cx| {
        listed(&shell, cx) == [other.clone(), id.clone()]
    });

    // It slides down over a few frames, drawn all along: it never leaves its project's list.
    let slid = midway(&shell, &id, before, cx);
    let (_, height) = frame(&shell, &id, cx).expect("still drawn while it slides");
    assert_eq!(height, full, "no fold: the row keeps its height");
    settle(cx);
    let after = top(&shell, &id, cx);
    assert!(
        before < slid && slid < after,
        "it slides to the foot: {before}, {slid}, {after}"
    );
    assert_eq!(after, before + full, "under the other row, which came up");
    assert_eq!(top(&shell, &other, cx), before);

    // Visible: a plain row of its project, its person and who it waits for.
    assert!(label(cx, session_id(&id)).is_some());
    let (mark, waits, scope) = shell.read_with(cx, |s, cx| {
        let row = s.session(&id).unwrap().read(cx);
        (
            row.mark(),
            row.waits().to_owned(),
            row.scope().map(str::to_owned),
        )
    });
    assert_eq!(mark, Some(AgentState::Blocked));
    assert_eq!(waits, "waits for Maya · review of #12");
    assert_eq!(
        scope, None,
        "it is in its project: it does not say whose it is"
    );
    assert_eq!(
        label(cx, session_waits_id(&id)).as_deref(),
        Some("waits for Maya · review of #12")
    );
    assert_eq!(blocked(&shell, cx), [id]);
}

#[gpui_kit::test]
fn a_session_whose_wait_ends_goes_back_to_its_place(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp, other) = two_sessions(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both rows", |cx| listed(&shell, cx).len() == 2);
    settle(cx);
    let place = top(&shell, &id, cx);
    wait_for_maya(&user, &id, &mcp);
    until(&shell, cx, "the row at the foot", |cx| {
        listed(&shell, cx) == [other.clone(), id.clone()]
    });
    settle(cx);
    let foot = top(&shell, &id, cx);
    assert!(foot > place);

    maya_reviews(&user, &id);
    // On top: it had the last exchange, its turn the review woke.
    until(&shell, cx, "the row back on top", |cx| {
        listed(&shell, cx) == [id.clone(), other.clone()]
    });

    // It slides back up over a few frames, drawn all along.
    let slid = midway(&shell, &id, foot, cx);
    assert!(
        frame(&shell, &id, cx).is_some(),
        "still drawn while it slides"
    );
    settle(cx);
    let back = top(&shell, &id, cx);
    assert!(
        back < slid && slid < foot,
        "it slides back: {foot}, {slid}, {back}"
    );
    assert_eq!(back, place, "in its place");
    assert!(blocked(&shell, cx).is_empty());
}

#[gpui_kit::test]
fn there_is_no_blocked_group_across_projects(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "its row", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    settle(cx);

    // Neither a group above "Ended" nor a fold nor a count in the project: the row is a row of
    // its project, and nothing of it is kept with the sidebar's folds.
    for gone in [
        "blocked",
        "blocked-who",
        "fold-blocked",
        "blocked-sessions",
        "sidebar-blocked",
        "blocked-footer",
        "project-blocked-atlas",
        "project-blocked-block-atlas",
    ] {
        assert!(label(cx, gone).is_none(), "{gone} is gone");
        assert_eq!(drawn_height(cx, gone), 0., "{gone} is not drawn");
    }
    assert!(frame(&shell, &id, cx).is_some_and(|(_, height)| height > 0.));
    assert!(
        user.call("window.get", json!({})).unwrap()["state"]["sidebar"]["blocked"].is_null(),
        "no fold kept for it"
    );
}

#[gpui_kit::test]
fn a_project_keeps_its_blocked_sessions_at_its_foot_in_its_order(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (live, _) = start(&user, "write the readme");
    let (first, first_mcp) = start(&user, "open the docs pull request");
    let (second, second_mcp) = start(&user, "open the docs pull request again");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the three rows", |cx| {
        listed(&shell, cx).len() == 3
    });
    let order = listed(&shell, cx);
    settle(cx);

    wait_for_maya(&user, &first, &first_mcp);
    wait_for_maya(&user, &second, &second_mcp);
    until(&shell, cx, "both at the foot", |cx| {
        blocked(&shell, cx).len() == 2
    });
    settle(cx);

    // The others first, in their order, then the blocked ones in the order the project had them.
    let kept: Vec<String> = order
        .into_iter()
        .filter(|id| *id == first || *id == second)
        .collect();
    assert_eq!(listed(&shell, cx)[0], live);
    assert_eq!(blocked(&shell, cx), kept);
    assert_eq!(listed(&shell, cx)[1..], kept[..]);
    for id in [&first, &second] {
        assert!(frame(&shell, id, cx).is_some_and(|(_, height)| height > 0.));
    }
}

#[gpui_kit::test]
fn a_folded_project_shows_its_strongest_mark_and_a_blocked_session_never_needs_you(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "its row", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    settle(cx);

    // Only a blocked session in it: the folded project says the person, never "needs you".
    cx.update(|window, cx| {
        window.click(
            gpui_design_system::toggle_id(flottille_window::tree::project_id("atlas")),
            cx,
        )
    });
    settle(cx);
    let held = shell.read_with(cx, |s, cx| s.project("atlas").unwrap().read(cx).held());
    assert_eq!(held, Some(AgentState::Blocked));
    let mark = shell.read_with(cx, |s, cx| s.project("atlas").unwrap().read(cx).mark());
    assert_eq!(mark, Some(AgentState::Blocked));

    // Maya answers: nothing is blocked, the mark is gone.
    maya_reviews(&user, &id);
    until(&shell, cx, "no blocked row", |cx| {
        blocked(&shell, cx).is_empty()
    });
    settle(cx);
    let held = shell.read_with(cx, |s, cx| s.project("atlas").unwrap().read(cx).held());
    assert_ne!(held, Some(AgentState::Blocked));
}

#[gpui_kit::test]
fn a_blocked_led_session_stays_under_its_pilot(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let lead = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": "begin the docs session"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let opened = user.received_requests("session/new").len();
    let _ = opened;
    eventually("the launch is held", || {
        !user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    let number = user.call("approval.list", json!({})).unwrap()["requests"][0]["number"]
        .as_u64()
        .unwrap();
    let opened = user.received_requests("session/new").len();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the led session", || agents(&user).len() == 2);
    let led = agents(&user)
        .into_iter()
        .find(|agent| agent["led_by"] == lead.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (url, authorization) = server_of(&user, opened);
    eventually("its first turn", || line(&user, &led)["state"] == "idle");
    eventually("its pilot at rest", || {
        line(&user, &lead)["state"] == "idle"
    });
    let mcp = Mcp::initialize(&url, &authorization);
    wait_for_maya(&user, &led, &mcp);

    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led row", |cx| {
        label(cx, session_id(&led)).is_some()
    });
    settle(cx);

    // Its pilot speaks for it: it stays under it, its mark and its line saying who.
    assert_eq!(listed(&shell, cx), [lead.clone(), led.clone()]);
    let (mark, waits) = shell.read_with(cx, |s, cx| {
        let row = s.session(&led).unwrap().read(cx);
        (row.mark(), row.waits().to_owned())
    });
    assert_eq!(mark, Some(AgentState::Blocked));
    assert_eq!(waits, "waits for Maya · review of #12");
}

#[gpui_kit::test]
fn a_withdrawn_review_request_brings_the_row_back(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp, other) = two_sessions(&user);
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row at the foot", |cx| {
        listed(&shell, cx) == [other.clone(), id.clone()]
    });
    let turns = user.received_requests("session/prompt").len();

    // The review is no longer asked of Maya, and no one reviewed: no event wakes the session,
    // and no subscription moves.
    answer(&user, json!([]), json!([]));
    a_round(&user);
    eventually("the session no longer blocked", || {
        line(&user, &id)["blocked"].is_null()
    });
    // Nothing happened to it but the end of the wait: it takes its place back, whatever it is.
    until(&shell, cx, "the row back in its place", |cx| {
        blocked(&shell, cx).is_empty()
    });
    assert_eq!(listed(&shell, cx).len(), 2);
    assert_eq!(
        user.received_requests("session/prompt").len(),
        turns,
        "no turn"
    );
}

#[gpui_kit::test]
fn a_blocked_pilot_stays_with_the_sessions_it_leads(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let opened = user.received_requests("session/new").len();
    let lead = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": "begin the docs session"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (url, authorization) = server_of(&user, opened);
    eventually("the launch is held", || {
        !user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    let number = user.call("approval.list", json!({})).unwrap()["requests"][0]["number"]
        .as_u64()
        .unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the led session", || agents(&user).len() == 2);
    let led = agents(&user)
        .into_iter()
        .find(|agent| agent["led_by"] == lead.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("its first turn", || line(&user, &led)["state"] == "idle");
    eventually("its pilot at rest", || {
        line(&user, &lead)["state"] == "idle"
    });
    // The pilot, which follows the session it leads, also waits on Maya's review.
    let mcp = Mcp::initialize(&url, &authorization);
    wait_for_maya(&user, &lead, &mcp);

    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led row", |cx| {
        label(cx, session_id(&led)).is_some()
    });
    settle(cx);

    // It stays in its project with the session it leads, its mark saying it is blocked.
    assert_eq!(listed(&shell, cx), [lead.clone(), led.clone()]);
    let mark = shell.read_with(cx, |s, cx| s.session(&lead).unwrap().read(cx).mark());
    assert_eq!(mark, Some(AgentState::Blocked));
}

#[gpui_kit::test]
fn a_blocked_row_says_what_its_session_is_about(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "its row", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    settle(cx);

    // Where the row sits is not what it says: the subject on its first line, the
    // key as the mono word on its second. It is in its project: it does not say whose it is.
    let (first, scope) = shell.read_with(cx, |s, cx| {
        let row = s.session(&id).unwrap().read(cx);
        (row.first_line(), row.scope().map(str::to_owned))
    });
    assert_eq!(first.as_ref(), "The docs are missing");
    assert_eq!(scope, None);
    let key = cx.update(|window, _| {
        window
            .within(IN_PROJECT)
            .try_find(session_key_beside_id(&id))
            .and_then(|found| found.label().map(str::to_owned))
    });
    assert_eq!(key.as_deref(), Some("#5"));
}

#[gpui_kit::test]
fn a_blocked_row_says_who_it_waits_for_on_its_second_line(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (other, _) = start(&user, "write the readme");
    let (id, mcp) = start(&user, "open the docs pull request");
    // A row of its project is two lines high: a blocked row keeps that height.
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the other row", |cx| {
        label(cx, session_id(&other)).is_some()
    });
    settle(cx);
    let (_, two_lines) = frame(&shell, &other, cx).expect("the other row");

    wait_for_maya(&user, &id, &mcp);
    until(&shell, cx, "its row", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    settle(cx);

    // Line 2 says who and why, as #1508 gave it.
    let says = cx.update(|window, _| {
        window
            .within(IN_PROJECT)
            .try_find(session_waits_id(&id))
            .and_then(|found| found.label().map(str::to_owned))
    });
    assert_eq!(says.as_deref(), Some("waits for Maya · review of #12"));
    assert_eq!(
        frame(&shell, &id, cx).map(|(_, height)| height),
        Some(two_lines)
    );
}

#[gpui_kit::test]
fn its_second_line_comes_back_when_it_is_no_longer_blocked(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "its row", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    settle(cx);
    assert!(
        label(cx, session_waits_id(&id)).is_some(),
        "it says who it waits for"
    );

    maya_reviews(&user, &id);
    until(&shell, cx, "the wait over", |cx| {
        blocked(&shell, cx).is_empty()
    });

    // Its usual line 2 is back, the words of the wait gone.
    let mut frames = 0;
    for _ in 0..60 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            break;
        }
        frames += 1;
    }
    assert!(frames >= 4, "the line changes over a few frames: {frames}");
    let within = |scope: &'static str, id: gpui_kit::ElementId, cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            window
                .within(scope)
                .try_find(id)
                .map(|found| found.label().map(str::to_owned))
        })
    };
    assert_eq!(
        within(IN_PROJECT, session_place_id(&id), cx),
        Some(Some("main".to_owned())),
        "its place, as every row of its project says it"
    );
    // The session waits for the review as any session does, no longer on a person.
    assert_eq!(
        within(IN_PROJECT, session_waits_id(&id), cx),
        Some(Some("waits for review of #12".to_owned())),
        "the words of a wait, not of a person"
    );
}

#[gpui_kit::test]
fn the_session_header_holds_no_ticket_line(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the ticket said in Workspace", |cx| {
        label(cx, "rail-about-ticket").is_some()
    });
    settle(cx);

    // The header keeps the title (and where it works): no chip, no "What it is about" line.
    assert!(label(cx, "session-title").is_some());
    for gone in [
        "session-about-line",
        "session-about-ticket",
        "session-about-from-words",
        "session-about-pull",
    ] {
        assert!(label(cx, gone).is_none(), "{gone} left the header");
        assert_eq!(drawn_height(cx, gone), 0., "{gone} is not drawn");
    }
}

#[gpui_kit::test]
fn the_ticket_and_its_wait_are_said_in_workspace(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([{"login": "maya", "name": "Maya"}]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    wait_for_the_assignee(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the wait said in Workspace", |cx| {
        label(cx, "rail-wait").is_some()
    });
    settle(cx);

    // Under the workspace line: the key, its title, an icon button that opens it.
    assert_eq!(
        label(cx, "rail-about-ticket").as_deref(),
        Some("#5 The docs are missing")
    );
    assert_eq!(label(cx, "rail-about-open").as_deref(), Some("Open #5"));
    // Under the ticket it waits on: who and since when.
    assert_eq!(label(cx, "rail-wait"), Some(wait_said(&user, &id)));
    let (ticket_top, wait_top) = cx.update(|window, _| {
        let top = |id: &str| f32::from(window.find(id.to_owned()).bounds().origin.y);
        (top("rail-about-ticket"), top("rail-wait"))
    });
    assert!(wait_top > ticket_top, "the wait is under its ticket");
    // The Agents row takes the person, not the clock.
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("its page");
    assert_eq!(
        page.read_with(cx, |p, cx| p.state(&id, cx)),
        Some(AgentState::Blocked)
    );
}

#[gpui_kit::test]
fn the_wait_leaves_workspace_when_it_ends(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([{"login": "maya", "name": "Maya"}]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    wait_for_the_assignee(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the wait said", |cx| {
        label(cx, "rail-wait").is_some()
    });
    settle(cx);
    let full = drawn_height(cx, "rail-wait-block");
    assert!(full > 0.);

    // Maya answers: the ticket is hers no longer.
    with_issue(&user, json!([]));
    eventually("the session no longer blocked", || {
        line(&user, &id)["blocked"].is_null()
    });
    until(&shell, cx, "the wait over", |cx| {
        blocked(&shell, cx).is_empty()
    });
    after(cx, Duration::from_millis(70));
    let folding = drawn_height(cx, "rail-wait-block");
    assert!(
        folding > 0. && folding < full,
        "the wait folds away over a few frames: {folding} of {full}"
    );
    assert!(
        label(cx, "rail-wait").is_some(),
        "it still says its words while it folds"
    );
    settle(cx);
    assert!(label(cx, "rail-wait").is_none(), "gone once it ended");
    assert_eq!(drawn_height(cx, "rail-wait-block"), 0.);
    // The ticket stays: the session is still about it.
    assert!(label(cx, "rail-about-ticket").is_some());
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("its page");
    assert_ne!(
        page.read_with(cx, |p, cx| p.state(&id, cx)),
        Some(AgentState::Blocked),
        "the Agents row is no longer the person"
    );
}

#[gpui_kit::test]
fn the_wait_unfolds_in_workspace_as_the_session_gets_blocked(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([{"login": "maya", "name": "Maya"}]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the ticket", |cx| {
        label(cx, "rail-about-ticket").is_some()
    });
    settle(cx);
    assert!(label(cx, "rail-wait").is_none(), "not blocked yet");
    assert_eq!(drawn_height(cx, "rail-wait-block"), 0.);

    wait_for_the_assignee(&user, &id, &mcp);
    until(&shell, cx, "the wait coming", |cx| {
        label(cx, "rail-wait").is_some()
    });
    // It unfolds and fades in: a frame between nothing and full.
    after(cx, Duration::from_millis(70));
    let coming = drawn_height(cx, "rail-wait-block");
    settle(cx);
    let full = drawn_height(cx, "rail-wait-block");
    assert!(
        coming > 0. && coming < full,
        "the wait unfolds over a few frames: {coming} of {full}"
    );
}

#[gpui_kit::test]
fn the_ticket_line_unfolds_in_workspace_as_the_session_takes_a_ticket(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([]));
    let (id, mcp) = start(&user, "open the docs pull request");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    // The section is mounted before the ticket comes: a line built with the section would be
    // there at its first frame, at full size, and nothing would unfold.
    until(&shell, cx, "its workspace", |cx| {
        drawn_height(cx, "rail-workspace-read") > 0.
    });
    settle(cx);
    assert!(label(cx, "rail-about-ticket").is_none(), "about no ticket");
    assert_eq!(drawn_height(cx, "rail-about-block"), 0.);

    takes_the_ticket(&user, &id, &mcp);
    until(&shell, cx, "the ticket coming", |cx| {
        label(cx, "rail-about-ticket").is_some()
    });
    after(cx, Duration::from_millis(70));
    let coming = drawn_height(cx, "rail-about-block");
    settle(cx);
    let full = drawn_height(cx, "rail-about-block");
    assert!(
        coming > 0. && coming < full,
        "the ticket unfolds over a few frames: {coming} of {full}"
    );
}

#[gpui_kit::test]
fn the_ticket_line_folds_away_in_workspace_as_the_session_becomes_free(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the ticket", |cx| {
        label(cx, "rail-about-ticket").is_some()
    });
    settle(cx);
    let full = drawn_height(cx, "rail-about-block");
    assert!(full > 0.);

    // The ticket is done: the session is about nothing, its line folds away over a few frames.
    let mut script = serde_json::from_str::<Value>(
        &std::fs::read_to_string(user.path("gh/script.json")).unwrap(),
    )
    .unwrap();
    script["issues"]["acme/atlas"]["5"]["state"] = json!("CLOSED");
    script["issues"]["acme/atlas"]["5"]["updatedAt"] = json!("2999-01-02T00:00:00Z");
    user.gh_script(script);
    a_round(&user);
    eventually("free", || line(&user, &id)["subject"]["kind"] == "free");
    // The row is no longer about it: the page's line folds away from there.
    until(&shell, cx, "its row about nothing", |cx| {
        shell.read_with(cx, |s, cx| {
            s.session(&id)
                .is_some_and(|row| row.read(cx).first_line().as_ref() != "The docs are missing")
        })
    });
    after(cx, Duration::from_millis(70));
    let going = drawn_height(cx, "rail-about-block");
    assert!(
        going > 0. && going < full,
        "it does not leave at once: {going} of {full}"
    );
    settle(cx);
    assert_eq!(drawn_height(cx, "rail-about-block"), 0.);
    assert!(label(cx, "rail-about-ticket").is_none());
}

#[gpui_kit::test]
fn nothing_is_drawn_above_the_composer_for_a_wait(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the wait said in Workspace", |cx| {
        label(cx, "rail-wait").is_some()
    });
    settle(cx);

    // The banner is gone: its words are the wait's tooltip, said once, in the rail.
    assert!(label(cx, "session-blocked").is_none(), "no banner");
    assert_eq!(drawn_height(cx, "session-blocked-line"), 0.);
    assert!(label(cx, "reply").is_some() || label(cx, "session-title").is_some());
}

/// One step of the window as the platform runs it; how many animation frames were asked.
fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) -> usize {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    asked
}

/// Runs the window until it asked for no animation frame over a second of its clock and two of
/// real time, as `window_at_rest.rs` does.
fn quiet(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    const STEP: Duration = Duration::from_millis(16);
    let began = Instant::now();
    let mut last_asked = began;
    let mut still = Duration::ZERO;
    while still < Duration::from_secs(1) || last_asked.elapsed() < Duration::from_secs(2) {
        assert!(began.elapsed() < Duration::from_secs(60), "never at rest");
        if step(shell, STEP, cx) == 0 {
            still += STEP;
        } else {
            still = Duration::ZERO;
            last_asked = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_blocked_session_s_row_and_page(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "its row", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    quiet(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the wait said in Workspace", |cx| {
        label(cx, "rail-wait").is_some()
    });
    quiet(&shell, cx);

    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        asked += step(&shell, Duration::from_millis(16), cx);
        gone += Duration::from_millis(16);
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    assert_eq!((drawn, asked), (0, 0), "(frames drawn, frames asked)");
}

/// What the window opens when a link is pressed, the opener being the test's.
fn links(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Arc<Mutex<Vec<PathBuf>>> {
    let opened: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    opened
}

fn page_of(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::session::SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("its page")
}

/// The subject of a session that opened the pull request #12: as the daemon says it.
fn pull_subject() -> flottille_window::model::Subject {
    flottille_window::model::Subject {
        kind: "pull_request".to_owned(),
        key: Some("#12".to_owned()),
        title: Some("Write the docs".to_owned()),
        url: None,
        from: "a pull request it opened".to_owned(),
        last: None,
        pull: Some(flottille_window::session_link::PullRequest {
            repository: ".".to_owned(),
            number: 12,
            state: "open".to_owned(),
            mark: None,
            url: "https://github.com/acme/atlas/pull/12".to_owned(),
        }),
    }
}

#[gpui_kit::test]
fn a_pull_request_subject_is_said_in_workspace(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    // A session of the project folder itself: its workspace section holds no pull request line.
    let (id, _mcp) = start(&user, "open the docs pull request");
    let (shell, cx) = window(&user, cx);
    let opened = links(&shell, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "its workspace", |cx| {
        drawn_height(cx, "rail-workspace-read") > 0.
    });
    settle(cx);
    assert!(label(cx, "rail-about-pull").is_none(), "about nothing yet");

    page_of(&shell, cx).update(cx, |page, cx| page.set_subject(Some(pull_subject()), cx));
    settle(cx);

    // Its number and its title, and an icon button that opens it: the header says none of it.
    assert_eq!(
        label(cx, "rail-about-pull").as_deref(),
        Some("#12 Write the docs")
    );
    assert_eq!(label(cx, "rail-about-open").as_deref(), Some("Open #12"));
    assert!(label(cx, "session-about-line").is_none());
    cx.update(|window, cx| window.click("rail-about-open", cx));
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://github.com/acme/atlas/pull/12")]
    );
}

#[gpui_kit::test]
fn what_a_free_session_last_held_is_said_in_workspace(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the ticket", |cx| {
        label(cx, "rail-about-ticket").is_some()
    });
    settle(cx);
    assert!(label(cx, "rail-about-last").is_none(), "it holds a ticket");

    // The ticket is done: the session is free, and says what it last held.
    let mut script = serde_json::from_str::<Value>(
        &std::fs::read_to_string(user.path("gh/script.json")).unwrap(),
    )
    .unwrap();
    script["issues"]["acme/atlas"]["5"]["state"] = json!("CLOSED");
    script["issues"]["acme/atlas"]["5"]["updatedAt"] = json!("2999-01-02T00:00:00Z");
    user.gh_script(script);
    a_round(&user);
    eventually("free", || line(&user, &id)["subject"]["kind"] == "free");
    let last = line(&user, &id)["subject"]["last"]
        .as_str()
        .expect("what it last held")
        .to_owned();
    until(&shell, cx, "what it last held, said", |cx| {
        label(cx, "rail-about-last").is_some()
    });
    settle(cx);
    assert_eq!(label(cx, "rail-about-last"), Some(last));
    assert!(label(cx, "rail-about-ticket").is_none());
}

#[gpui_kit::test]
fn the_open_button_of_a_ticket_opens_its_page(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    with_issue(&user, json!([]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    let opened = links(&shell, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the ticket", |cx| {
        label(cx, "rail-about-open").is_some()
    });
    settle(cx);

    assert_eq!(label(cx, "rail-about-open").as_deref(), Some("Open #5"));
    cx.update(|window, cx| window.click("rail-about-open", cx));
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://github.com/acme/atlas/issues/5")]
    );
}

#[gpui_kit::test]
fn a_wait_not_under_its_pull_request_says_what_it_waits_on(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    // On the project folder: its section holds no line of the pull request #12.
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "the wait said in Workspace", |cx| {
        label(cx, "rail-wait").is_some()
    });
    settle(cx);

    assert_eq!(
        label(cx, "rail-wait"),
        Some(wait_said_with(&user, &id, true))
    );
    assert!(label(cx, "rail-wait").unwrap().contains("review of #12"));
}

/// A session in the workspace `docs`, whose branch has the pull request #12 asking a review of
/// Maya, and which waits on it.
fn blocked_in_a_workspace(user: &User) -> String {
    let made = user
        .call(
            "workspace.create",
            json!({"project": "atlas", "name": "docs", "offline": true}),
        )
        .unwrap();
    assert_eq!(made["state"], "ready", "{made}");
    let branch = user
        .call("workspace.list", json!({"project": "atlas"}))
        .unwrap()["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "docs")
        .unwrap()["branch"]
        .as_str()
        .unwrap()
        .to_owned();
    answer_on(
        user,
        &branch,
        json!([{"__typename": "User", "login": "maya", "name": "Maya"}]),
        json!([]),
    );
    a_round(user);
    let (id, mcp) = start_in(user, "open the docs pull request", Some("docs"));
    wait_for_maya(user, &id, &mcp);
    id
}

/// Whether the wait is drawn under the pull request's line of `atlas`. While it moves there, the
/// block it leaves at the top holds one too, and a plain search for it is ambiguous.
fn wait_under_pull(cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| {
        window
            .within("rail-wait-pull-block-atlas")
            .try_find("rail-wait")
            .is_some()
    })
}

#[gpui_kit::test]
fn the_wait_goes_under_its_pull_request_line(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let id = blocked_in_a_workspace(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "its pull request and the wait under it", |cx| {
        label(cx, "rail-pull-atlas").is_some() && wait_under_pull(cx)
    });
    settle(cx);

    // Under the pull request's line, which says what it waits on: the wait does not.
    assert_eq!(label(cx, "rail-wait"), Some(wait_said(&user, &id)));
    let (pull_bottom, wait_top) = cx.update(|window, _| {
        let pull = window.find("rail-line-pull-atlas".to_owned()).bounds();
        let wait = window.find("rail-wait".to_owned()).bounds();
        (
            f32::from(pull.origin.y + pull.size.height),
            f32::from(wait.origin.y),
        )
    });
    assert!(
        wait_top >= pull_bottom - 1.,
        "{wait_top} under {pull_bottom}"
    );
    // One place, not two: the line at the top of the section is not drawn.
    assert_eq!(drawn_height(cx, "rail-wait-block"), 0.);
}

#[gpui_kit::test]
fn the_agents_row_keeps_what_it_waits_for_and_its_mark_cross_fades(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    // The ticket is the user's own: the session waits, it is blocked by no one.
    with_issue(&user, json!([]));
    let (id, mcp) = start(&user, "open the docs pull request");
    takes_the_ticket(&user, &id, &mcp);
    wait_for_ticket_unassigned(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    let waiting = format!("rail-waiting-{id}");
    until(&shell, cx, "what it waits for", |cx| {
        label(cx, waiting.clone()).is_some()
    });
    settle(cx);
    let words = label(cx, waiting.clone()).unwrap();
    assert!(words.contains("ticket"), "{words}");
    let mark_height = |cx: &mut VisualTestContext, shape: &str| {
        let (within, shape) = (format!("rail-mark-{id}"), shape.to_owned());
        cx.update(|window, _| {
            window
                .within(within)
                .try_find(shape)
                .map(|found| f32::from(found.bounds().size.height))
        })
    };
    assert!(mark_height(cx, "state-mark-clock").is_some(), "a clock");
    assert!(mark_height(cx, "state-mark-person").is_none());

    // Maya is assigned: the session is blocked by her, the words stay, the mark swaps.
    with_issue(&user, json!([{"login": "maya", "name": "Maya"}]));
    eventually("blocked", || !line(&user, &id)["blocked"].is_null());
    until(&shell, cx, "the person's mark", |cx| {
        mark_height(cx, "state-mark-person").is_some()
    });
    // In the middle of the swap it is smaller than at rest.
    let coming = mark_height(cx, "state-mark-person").unwrap();
    settle(cx);
    let full = mark_height(cx, "state-mark-person").unwrap();
    assert!(
        coming < full,
        "the mark fades in over a few frames: {coming} of {full}"
    );
    assert!(
        mark_height(cx, "state-mark-clock").is_none(),
        "the clock left"
    );
    assert_eq!(label(cx, waiting.clone()), Some(words), "its words stay");
}

/// The session subscribes to ticket #5, which is assigned to no one: it waits, blocked by no one.
fn wait_for_ticket_unassigned(user: &User, id: &str, mcp: &Mcp) {
    let answer = mcp.call(
        "tickets",
        json!({"action": "subscribe", "ticket": "#5", "events": ["commented", "status"]}),
    );
    assert!(!answer.error, "{}", answer.text);
    eventually("the session waiting", || {
        user.call("agent.tree", json!({"agent": id})).unwrap()["agents"][0]["waiting_on"]
            == "subscription"
    });
}

/// A wait of Maya's on `on`, as the daemon would say it.
fn waiting_on(on: flottille_window::model::WaitOn) -> flottille_window::model::Blocked {
    flottille_window::model::Blocked {
        by: vec!["Maya".to_owned()],
        why: match &on {
            flottille_window::model::WaitOn::PullRequest { number, .. } => {
                format!("review of #{number}")
            }
            flottille_window::model::WaitOn::Ticket(key) => format!("ticket {key}"),
        },
        on: Some(on),
        since: 1_700_000_000_000,
    }
}

fn review_of(repository: Option<&str>, number: u64) -> flottille_window::model::WaitOn {
    flottille_window::model::WaitOn::PullRequest {
        repository: repository.map(str::to_owned),
        number,
    }
}

fn pull_in(repository: &str, number: u64) -> flottille_window::session_link::PullRequest {
    flottille_window::session_link::PullRequest {
        repository: repository.to_owned(),
        number,
        state: "open".to_owned(),
        mark: None,
        url: format!("https://github.com/acme/{repository}/pull/{number}"),
    }
}

/// A place of these repositories (their paths in the project) and their pull requests.
fn place_of(
    repositories: &[&str],
    pulls: Vec<flottille_window::session_link::PullRequest>,
) -> flottille_window::session_link::PlaceFacts {
    flottille_window::session_link::PlaceFacts {
        repositories: repositories
            .iter()
            .map(|path| {
                (
                    (*path).to_owned(),
                    flottille_window::session_link::RepositoryFacts::Branch(Some(
                        "main".to_owned(),
                    )),
                )
            })
            .collect(),
        pulls,
        ..Default::default()
    }
}

/// The page of a session of the project folder, open, its place and its wait as the test says
/// them (nothing polls the daemon after).
fn page_with<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
    place: flottille_window::session_link::PlaceFacts,
    blocked: Option<flottille_window::model::Blocked>,
) -> (
    Entity<Shell>,
    &'a mut VisualTestContext,
    Entity<flottille_window::session::SessionPage>,
) {
    let (id, _mcp) = start(user, "open the docs pull request");
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "its workspace", |cx| {
        drawn_height(cx, "rail-workspace-read") > 0.
    });
    settle(cx);
    let page = page_of(&shell, cx);
    page.update(cx, |page, cx| {
        page.set_place(Some(place), cx);
        page.set_blocked(blocked, cx);
    });
    settle(cx);
    (shell, cx, page)
}

#[gpui_kit::test]
fn the_wait_unfolds_under_its_pull_request_as_it_comes_there_and_folds_away_as_it_leaves(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user);
    // The pull request line is there; the wait is on a ticket, at the top.
    let (_shell, cx, page) = page_with(
        &user,
        cx,
        place_of(&["."], vec![pull_in(".", 12)]),
        Some(waiting_on(flottille_window::model::WaitOn::Ticket(
            "#5".to_owned(),
        ))),
    );
    const TOP: &str = "rail-wait-block";
    const UNDER: &str = "rail-wait-pull-block-atlas";
    assert!(drawn_height(cx, TOP) > 0., "at the top");
    assert_eq!(drawn_height(cx, UNDER), 0., "not under the pull request");
    let at_top = drawn_height(cx, TOP);

    // The wait is now on the review of #12: it goes under its line. The one under the line
    // unfolds, the one at the top folds, both over a few frames: neither pops.
    page.update(cx, |page, cx| {
        page.set_blocked(Some(waiting_on(review_of(Some("."), 12))), cx)
    });
    after(cx, Duration::from_millis(70));
    let (coming, leaving) = (drawn_height(cx, UNDER), drawn_height(cx, TOP));
    settle(cx);
    let under = drawn_height(cx, UNDER);
    assert!(
        coming > 0. && coming < under,
        "the wait unfolds under its pull request: {coming} of {under}"
    );
    assert!(
        leaving > 0. && leaving < at_top,
        "the wait folds away from the top: {leaving} of {at_top}"
    );
    assert_eq!(drawn_height(cx, TOP), 0.);

    // Back on a ticket (the pull request line left its wait): the reverse.
    page.update(cx, |page, cx| {
        page.set_blocked(
            Some(waiting_on(flottille_window::model::WaitOn::Ticket(
                "#5".to_owned(),
            ))),
            cx,
        )
    });
    after(cx, Duration::from_millis(70));
    let (leaving, coming) = (drawn_height(cx, UNDER), drawn_height(cx, TOP));
    assert!(
        leaving > 0. && leaving < under,
        "the wait folds away from under the pull request: {leaving} of {under}"
    );
    assert!(coming > 0. && coming < at_top, "and unfolds at the top");
}

#[gpui_kit::test]
fn a_wait_is_put_under_the_pull_request_of_its_own_repository(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    // Two repositories each have a #12: the wait is on the one of `web`.
    let (_shell, cx, _page) = page_with(
        &user,
        cx,
        place_of(
            &["api", "web"],
            vec![pull_in("api", 12), pull_in("web", 12)],
        ),
        Some(waiting_on(review_of(Some("web"), 12))),
    );

    let (api_bottom, web_top, web_bottom, wait) = cx.update(|window, _| {
        let bounds = |id: &str| window.find(id.to_owned()).bounds();
        let bottom = |id: &str| f32::from(bounds(id).origin.y + bounds(id).size.height);
        (
            bottom("rail-line-pull-api"),
            f32::from(bounds("rail-line-repository-web").origin.y),
            bottom("rail-line-pull-web"),
            f32::from(bounds("rail-wait").origin.y),
        )
    });
    assert!(wait >= web_bottom - 1., "{wait} under web's {web_bottom}");
    // Nothing between api's pull request and web's repository: no second line under api's.
    assert!(web_top - api_bottom < 2., "a wait stands under api's");
    // The line is under its pull request: it does not say what it waits on.
    assert_eq!(
        label(cx, "rail-wait").unwrap(),
        flottille_window::session::blocked_line(&waiting_on(review_of(Some("web"), 12)), false)
    );
}

#[gpui_kit::test]
fn a_wait_does_not_disappear_when_its_pull_request_is_not_the_one_drawn(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    // A repository has two pull requests; the section draws the first. The wait is on the other.
    let (_shell, cx, _page) = page_with(
        &user,
        cx,
        place_of(&["."], vec![pull_in(".", 11), pull_in(".", 12)]),
        Some(waiting_on(review_of(Some("."), 12))),
    );

    // At the top, saying what it waits on, since no line of #12 stands above it.
    assert_eq!(
        label(cx, "rail-wait"),
        Some(flottille_window::session::blocked_line(
            &waiting_on(review_of(Some("."), 12)),
            true
        ))
    );
    let (wait_bottom, repository_top) = cx.update(|window, _| {
        let wait = window.find("rail-wait".to_owned()).bounds();
        let repository = window
            .find("rail-line-repository-atlas".to_owned())
            .bounds();
        (
            f32::from(wait.origin.y + wait.size.height),
            f32::from(repository.origin.y),
        )
    });
    assert!(wait_bottom <= repository_top + 1., "at the top");
}

#[gpui_kit::test]
fn under_a_pull_request_subject_a_wait_does_not_repeat_its_review(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    // A session on the project folder: no pull request line, its subject says the pull request.
    let (_shell, cx, page) = page_with(
        &user,
        cx,
        place_of(&["."], vec![]),
        Some(waiting_on(review_of(Some("."), 12))),
    );
    page.update(cx, |page, cx| page.set_subject(Some(pull_subject()), cx));
    settle(cx);

    assert_eq!(
        label(cx, "rail-about-pull").as_deref(),
        Some("#12 Write the docs")
    );
    assert_eq!(
        label(cx, "rail-wait").unwrap(),
        flottille_window::session::blocked_line(&waiting_on(review_of(Some("."), 12)), false)
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_wait_under_its_pull_request_line(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user);
    let id = blocked_in_a_workspace(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "its row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    open_page(&shell, &id, cx);
    until(&shell, cx, "its pull request and the wait under it", |cx| {
        label(cx, "rail-pull-atlas").is_some() && wait_under_pull(cx)
    });
    quiet(&shell, cx);

    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        asked += step(&shell, Duration::from_millis(16), cx);
        gone += Duration::from_millis(16);
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    assert_eq!((drawn, asked), (0, 0), "(frames drawn, frames asked)");
}
