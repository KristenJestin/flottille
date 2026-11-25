//! A session blocked by someone else in the window (#1451, `docs/design.md` §10, "Blocked by
//! others"), on GPUI's test platform in front of a real daemon whose agents are the fake ACP agent
//! and whose forge is the fake `gh`: a root at rest whose subscription waits on a review asked of
//! someone else leaves its project for one folded group above "Ended", and comes back by itself.
//! No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::state::AgentState;
use flottille_window::tree::{blocked_id, blocked_who_id, session_id};
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

/// The fake `gh` answers the pull request #12, a review asked of `requested`.
fn script(user: &User, requested: Value) {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let reviews = match requested.as_array().is_some_and(Vec::is_empty) {
        true => json!([{"author": {"login": "maya"}, "state": "APPROVED", "body": "Fine"}]),
        false => json!([]),
    };
    user.gh_script(json!({"pulls": {"acme/atlas": {"feat/docs": {
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
    let opened = user.received_requests("session/new").len();
    let id = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": message}),
        )
        .unwrap()["agent"]["id"]
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

/// The ids of the rows of the "Blocked by others" group.
fn blocked(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.blocked()
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

fn label(cx: &mut VisualTestContext, id: gpui_kit::ElementId) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Where a row is drawn: in its project's level, or in the "Blocked by others" group.
const IN_PROJECT: &str = "fold-project-atlas";
const IN_GROUP: &str = "blocked-sessions";

/// The height of the frame a row of the tree is drawn in within `scope`, `None` when it is not
/// drawn there.
fn frame_height(
    shell: &Entity<Shell>,
    id: &str,
    scope: &'static str,
    cx: &mut VisualTestContext,
) -> Option<f32> {
    let row = shell.read_with(cx, |s, _| s.session(id).map(|row| row.entity_id()))?;
    let frame = gpui_kit::ElementId::Name(format!("tree-row-{}", row.as_u64()).into());
    cx.update(|window, _| {
        window
            .within(scope)
            .try_find(frame)
            .map(|found| f32::from(found.bounds().size.height))
    })
}

/// Where the top of a session's row is drawn in its project's level.
fn row_top(id: &str, cx: &mut VisualTestContext) -> Option<f32> {
    cx.update(|window, _| {
        window
            .within(IN_PROJECT)
            .try_find(session_id(id))
            .map(|found| f32::from(found.bounds().origin.y))
    })
}

/// How high the "Blocked by others" group's fold is drawn: none while it is folded.
fn group_height(cx: &mut VisualTestContext) -> f32 {
    cx.update(|window, _| {
        window
            .try_find(gpui_kit::ElementId::Name("fold-blocked".into()))
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
}

#[gpui_kit::test]
fn a_blocked_root_leaves_its_project_for_the_blocked_group(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (other, _) = start(&user, "write the readme");
    let (id, mcp) = start(&user, "open the docs pull request");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both rows", |cx| {
        label(cx, session_id(&id)).is_some() && label(cx, session_id(&other)).is_some()
    });
    settle(cx);
    let full = frame_height(&shell, &id, IN_PROJECT, cx).expect("its row");

    wait_for_maya(&user, &id, &mcp);
    until(&shell, cx, "the row leaves its project", |cx| {
        !listed(&shell, cx).contains(&id)
    });

    // It folds away from its project, its gap with it, into the group, folded.
    assert_eq!(listed(&shell, cx), std::slice::from_ref(&other));
    assert_eq!(blocked(&shell, cx), std::slice::from_ref(&id));
    after(cx, Duration::from_millis(70));
    let folding =
        frame_height(&shell, &id, IN_PROJECT, cx).expect("still drawn while it folds away");
    assert!(
        folding > 0. && folding < full,
        "it folds away: {folding} of {full}"
    );
    settle(cx);
    assert_eq!(
        frame_height(&shell, &id, IN_PROJECT, cx),
        None,
        "gone from its project"
    );
    assert!(group_height(cx) < 1., "the group is folded");
    let mark = shell.read_with(cx, |s, cx| s.session(&id).unwrap().read(cx).mark());
    assert_eq!(mark, Some(AgentState::Blocked));
}

#[gpui_kit::test]
fn the_blocked_group_says_who_and_is_folded_by_default(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the group", |cx| {
        label(cx, blocked_who_id()).is_some()
    });
    settle(cx);

    assert_eq!(label(cx, blocked_who_id()).as_deref(), Some("1 · Maya"));
    let open = shell.read_with(cx, |s, cx| s.blocked().unwrap().read(cx).is_open());
    assert!(!open, "folded by default");
    assert!(group_height(cx) < 1., "its row is folded away");

    // Unfolded, the daemon keeps it with the sidebar's folds.
    cx.update(|window, cx| window.click(blocked_id(), cx));
    settle(cx);
    assert!(group_height(cx) > 1., "unfolded");
    assert!(frame_height(&shell, &id, IN_GROUP, cx).is_some_and(|h| h > 0.));
    eventually("the fold kept", || {
        user.call("window.get", json!({})).unwrap()["state"]["sidebar"]["blocked"] == true
    });
}

#[gpui_kit::test]
fn a_blocked_session_comes_back_on_top_of_its_project_by_itself(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    let (other, _) = start(&user, "write the readme");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the group", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    cx.update(|window, cx| window.click(blocked_id(), cx));
    settle(cx);
    assert!(
        frame_height(&shell, &id, IN_GROUP, cx).is_some(),
        "its row in the group"
    );
    // A row of its project is two lines high, the group's one.
    let full = frame_height(&shell, &other, IN_PROJECT, cx).expect("the other row");

    maya_reviews(&user, &id);
    until(&shell, cx, "the row back in its project", |cx| {
        listed(&shell, cx).contains(&id)
    });

    // On top: it had the last exchange, its turn the review woke. A row of the tree arrives at its
    // full size, rising into place as it fades in.
    assert_eq!(listed(&shell, cx), [id.clone(), other.clone()]);
    after(cx, Duration::from_millis(70));
    let rising = row_top(&id, cx).expect("drawn as it arrives");
    settle(cx);
    let placed = row_top(&id, cx).expect("in place");
    assert!(
        rising > placed,
        "it rises into place: {rising} then {placed}"
    );
    assert_eq!(frame_height(&shell, &id, IN_PROJECT, cx), Some(full));
    assert!(blocked(&shell, cx).is_empty());
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
    assert!(blocked(&shell, cx).is_empty());
    let (mark, waits) = shell.read_with(cx, |s, cx| {
        let row = s.session(&led).unwrap().read(cx);
        (row.mark(), row.waits().to_owned())
    });
    assert_eq!(mark, Some(AgentState::Blocked));
    assert_eq!(waits, "waits for Maya · review of #12");
}

#[gpui_kit::test]
fn the_blocked_group_is_absent_while_empty(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    settle(cx);
    assert!(
        label(cx, blocked_id()).is_none(),
        "no group while none is blocked"
    );

    wait_for_maya(&user, &id, &mcp);
    until(&shell, cx, "the group", |cx| {
        label(cx, blocked_id()).is_some()
    });
    maya_reviews(&user, &id);
    until(&shell, cx, "the row back", |cx| {
        listed(&shell, cx).contains(&id)
    });
    // The group folds away with its last row.
    settle(cx);
    assert!(label(cx, blocked_id()).is_none(), "gone with its last row");
}

#[gpui_kit::test]
fn a_blocked_session_s_page_says_who_and_since_when(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let since = line(&user, &id)["blocked"]["since"].as_u64().unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the group", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    cx.update(|window, cx| window.click(blocked_id(), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the banner", |cx| {
        label(cx, "session-blocked".into()).is_some()
    });

    let said = label(cx, "session-blocked".into()).unwrap();
    let at = flottille_window::session::clock(since);
    assert_eq!(
        said,
        format!(
            "Waits for Maya: review of #12, since {at}. It comes back by itself when they answer."
        )
    );
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
fn the_window_draws_no_frame_at_rest_on_a_blocked_session_s_page_and_its_group(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the group", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    cx.update(|window, cx| window.click(blocked_id(), cx));
    quiet(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the banner", |cx| {
        label(cx, "session-blocked".into()).is_some()
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
