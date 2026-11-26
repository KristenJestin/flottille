//! A session blocked by someone else in the window (#1451, #1530, `docs/design.md` §10, "Blocked
//! by others"), on GPUI's test platform in front of a real daemon whose agents are the fake ACP
//! agent and whose forge is the fake `gh`: a root at rest whose subscription waits on someone else
//! stays in its project, at its foot under a folded `N blocked by others`, and comes back to its
//! place by itself; its page says the wait in the Workspace section, under its ticket. No window
//! opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::state::AgentState;
use flottille_window::tree::{
    project_blocked_id, session_id, session_key_beside_id, session_place_id, session_waits_id,
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
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CHANGES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

/// The ids of the rows of `atlas`'s foot, `N blocked by others`.
fn blocked(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.project("atlas")
            .map(|node| {
                node.read(cx)
                    .blocked(cx)
                    .iter()
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

/// Where a row is drawn: in its project's level, or in the project's foot.
const IN_PROJECT: &str = "fold-project-atlas";
const IN_GROUP: &str = "fold-blocked-atlas";
/// The foot's row, `N blocked by others`, unfolds as the project gets its first blocked session.
const FOOT_BLOCK: &str = "project-blocked-block-atlas";

/// The height of the frame a row of the tree is drawn in within `scope`, `None` when it is not
/// drawn there.
fn frame_height(
    shell: &Entity<Shell>,
    id: &str,
    scope: &'static str,
    cx: &mut VisualTestContext,
) -> Option<f32> {
    let row = shell.read_with(cx, |s, _| s.session(id).map(|row| row.entity_id()))?;
    // The foot's rows are framed under a name of their own: a project's fold holds both levels.
    let kind = if scope == IN_GROUP {
        "blocked-row"
    } else {
        "tree-row"
    };
    let frame = gpui_kit::ElementId::Name(format!("{kind}-{}", row.as_u64()).into());
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

/// How high the foot's fold is drawn: none while it is folded.
fn group_height(cx: &mut VisualTestContext) -> f32 {
    cx.update(|window, _| {
        window
            .try_find(gpui_kit::ElementId::Name(IN_GROUP.into()))
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
}

/// How high the foot's row is drawn with its fold: none while the project has no blocked session.
fn foot_height(cx: &mut VisualTestContext) -> f32 {
    cx.update(|window, _| {
        window
            .within(IN_PROJECT)
            .try_find(FOOT_BLOCK)
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
}

/// What the foot's row says: `1 blocked by others`.
fn foot_said(cx: &mut VisualTestContext) -> Option<String> {
    label(cx, project_blocked_id("atlas"))
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

/// The words of a wait as the Workspace line says them, from what the daemon says of it.
fn wait_said(user: &User, id: &str) -> String {
    let blocked = line(user, id)["blocked"].clone();
    let who: Vec<&str> = blocked["by"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let at = flottille_window::session::clock(blocked["since"].as_u64().unwrap());
    format!("Waits for {} · since {at}", who.join(", "))
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

#[gpui_kit::test]
fn a_blocked_session_stays_under_its_project(cx: &mut TestAppContext) {
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
    assert_eq!(foot_height(cx), 0., "no foot while none is blocked");

    wait_for_maya(&user, &id, &mcp);
    until(&shell, cx, "the row leaves the live list", |cx| {
        !listed(&shell, cx).contains(&id)
    });

    // Its project holds it still, at its foot: the live list folds the row away with its gap, and
    // the foot's row unfolds with the count it says.
    assert_eq!(listed(&shell, cx), std::slice::from_ref(&other));
    assert_eq!(blocked(&shell, cx), std::slice::from_ref(&id));
    after(cx, Duration::from_millis(70));
    let folding =
        frame_height(&shell, &id, IN_PROJECT, cx).expect("still drawn while it folds away");
    assert!(
        folding > 0. && folding < full,
        "it folds away: {folding} of {full}"
    );
    let unfolding = foot_height(cx);
    assert!(
        unfolding > 0.,
        "the foot's row unfolds as the row folds away: {unfolding}"
    );
    settle(cx);
    let foot = foot_height(cx);
    assert!(
        foot > unfolding,
        "it is still unfolding at 70 ms: {unfolding} then {foot}"
    );
    assert_eq!(
        frame_height(&shell, &id, IN_PROJECT, cx),
        None,
        "gone from the live list"
    );
    assert_eq!(foot_said(cx).as_deref(), Some("1 blocked by others"));
    // The foot is inside the project's fold: its row is drawn there.
    let inside = cx.update(|window, _| {
        window
            .within(IN_PROJECT)
            .try_find(project_blocked_id("atlas"))
            .is_some()
    });
    assert!(inside, "under its project, not apart");
    assert!(group_height(cx) < 1., "the foot is folded");
    let mark = shell.read_with(cx, |s, cx| s.session(&id).unwrap().read(cx).mark());
    assert_eq!(mark, Some(AgentState::Blocked));
}

#[gpui_kit::test]
fn there_is_no_blocked_group_across_projects(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the foot", |cx| foot_said(cx).is_some());
    settle(cx);

    // Neither the group above "Ended" nor its fold: only the project's own foot.
    assert!(label(cx, "blocked").is_none(), "no group row");
    assert!(label(cx, "blocked-who").is_none(), "no names heading");
    assert!(label(cx, "fold-blocked").is_none(), "no group fold");
    assert!(label(cx, "blocked-sessions").is_none(), "no group list");
    assert_eq!(foot_said(cx).as_deref(), Some("1 blocked by others"));
}

#[gpui_kit::test]
fn a_project_gathers_its_blocked_sessions_under_n_blocked_by_others(cx: &mut TestAppContext) {
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

    assert_eq!(listed(&shell, cx), std::slice::from_ref(&live));
    assert_eq!(foot_said(cx).as_deref(), Some("2 blocked by others"));
    // A session keeps its place in its project's order whatever its state: the foot holds the
    // blocked ones in the order the project had them.
    let kept: Vec<String> = order
        .into_iter()
        .filter(|id| *id == first || *id == second)
        .collect();
    assert_eq!(blocked(&shell, cx), kept);

    // Its rows are drawn once the foot is unfolded, each one in its place.
    cx.update(|window, cx| window.click(project_blocked_id("atlas"), cx));
    settle(cx);
    assert!(group_height(cx) > 1., "unfolded");
    for id in [&first, &second] {
        assert!(frame_height(&shell, id, IN_GROUP, cx).is_some_and(|h| h > 0.));
        assert_eq!(frame_height(&shell, id, IN_PROJECT, cx), None);
    }
}

#[gpui_kit::test]
fn n_blocked_by_others_is_folded_by_default_and_keeps_its_fold(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the foot", |cx| foot_said(cx).is_some());
    settle(cx);

    let open = shell.read_with(cx, |s, cx| {
        s.project("atlas").unwrap().read(cx).blocked_open()
    });
    assert!(!open, "folded by default");
    assert!(group_height(cx) < 1., "its rows are folded away");

    // The fold turns over a few frames, as the rail's `N ended` does.
    cx.update(|window, cx| window.click(project_blocked_id("atlas"), cx));
    after(cx, Duration::from_millis(70));
    let folding = group_height(cx);
    settle(cx);
    let full = group_height(cx);
    assert!(
        folding > 0. && folding < full,
        "it unfolds over a few frames: {folding} of {full}"
    );
    assert!(frame_height(&shell, &id, IN_GROUP, cx).is_some_and(|h| h > 0.));
    // The daemon keeps it with the sidebar's other folds, one key for each project.
    eventually("the fold kept", || {
        user.call("window.get", json!({})).unwrap()["state"]["sidebar"]["blocked"]
            == json!({"atlas": true})
    });

    // And folds again over a few frames, kept as folded.
    cx.update(|window, cx| window.click(project_blocked_id("atlas"), cx));
    after(cx, Duration::from_millis(70));
    let folding = group_height(cx);
    assert!(
        folding > 0. && folding < full,
        "it folds over a few frames: {folding} of {full}"
    );
    settle(cx);
    assert!(group_height(cx) < 1.);
    eventually("the fold kept folded", || {
        user.call("window.get", json!({})).unwrap()["state"]["sidebar"]["blocked"]
            == json!({"atlas": false})
    });
}

#[gpui_kit::test]
fn n_blocked_by_others_opens_as_the_user_left_it(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    // The window was closed with the foot of atlas unfolded.
    user.call(
        "window.set",
        json!({"key": "sidebar", "value": {
            "projects": {}, "leads": {}, "blocked": {"atlas": true}}}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the foot", |cx| foot_said(cx).is_some());

    // Made unfolded, it never animates from the other: its row is there at once.
    let open = shell.read_with(cx, |s, cx| {
        s.project("atlas").unwrap().read(cx).blocked_open()
    });
    assert!(open, "unfolded as it was left");
    assert!(group_height(cx) > 1., "its rows are drawn");
    assert!(frame_height(&shell, &id, IN_GROUP, cx).is_some_and(|h| h > 0.));
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
    until(&shell, cx, "the foot", |cx| foot_said(cx).is_some());
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
fn a_blocked_session_comes_back_in_its_place_by_itself(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    let (other, _) = start(&user, "write the readme");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the foot", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    cx.update(|window, cx| window.click(project_blocked_id("atlas"), cx));
    settle(cx);
    assert!(
        frame_height(&shell, &id, IN_GROUP, cx).is_some(),
        "its row at the foot"
    );
    // A row of its project is two lines high.
    let full = frame_height(&shell, &other, IN_PROJECT, cx).expect("the other row");
    assert!(foot_height(cx) > 0.);

    maya_reviews(&user, &id);
    until(&shell, cx, "the row back in its project", |cx| {
        listed(&shell, cx).contains(&id)
    });

    // On top: it had the last exchange, its turn the review woke. The row rises into place as it
    // fades in, and leaves the foot over a few frames, the foot's row folding away with its last.
    assert_eq!(listed(&shell, cx), [id.clone(), other.clone()]);
    after(cx, Duration::from_millis(70));
    let leaving = frame_height(&shell, &id, IN_GROUP, cx).expect("still drawn as it leaves");
    assert!(leaving > 0., "it slides out of the foot: {leaving}");
    let folding = foot_height(cx);
    assert!(folding > 0., "the foot's row folds away: {folding}");
    let rising = row_top(&id, cx).expect("drawn as it arrives");
    settle(cx);
    let placed = row_top(&id, cx).expect("in place");
    assert!(
        rising > placed,
        "it rises into place: {rising} then {placed}"
    );
    assert_eq!(frame_height(&shell, &id, IN_PROJECT, cx), Some(full));
    assert!(blocked(&shell, cx).is_empty());
    assert_eq!(foot_height(cx), 0., "the foot is gone with its last row");
    assert!(foot_said(cx).is_none());
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
    assert!(blocked(&shell, cx).is_empty());
    assert!(foot_said(cx).is_none(), "no foot");
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
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the foot", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    let turns = user.received_requests("session/prompt").len();

    // The review is no longer asked of Maya, and no one reviewed: no event wakes the session,
    // and no subscription moves.
    answer(&user, json!([]), json!([]));
    a_round(&user);
    eventually("the session no longer blocked", || {
        line(&user, &id)["blocked"].is_null()
    });
    until(&shell, cx, "the row back in its project", |cx| {
        listed(&shell, cx).contains(&id)
    });
    assert!(blocked(&shell, cx).is_empty());
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
    assert!(blocked(&shell, cx).is_empty());
    assert!(foot_said(cx).is_none(), "no foot");
    let mark = shell.read_with(cx, |s, cx| s.session(&lead).unwrap().read(cx).mark());
    assert_eq!(mark, Some(AgentState::Blocked));
}

#[gpui_kit::test]
fn the_foot_is_absent_while_none_is_blocked(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| {
        label(cx, session_id(&id)).is_some()
    });
    settle(cx);
    assert!(foot_said(cx).is_none(), "no foot while none is blocked");

    wait_for_maya(&user, &id, &mcp);
    until(&shell, cx, "the foot", |cx| foot_said(cx).is_some());
    maya_reviews(&user, &id);
    until(&shell, cx, "the row back", |cx| {
        listed(&shell, cx).contains(&id)
    });
    // The foot folds away with its last row.
    settle(cx);
    assert!(foot_said(cx).is_none(), "gone with its last row");
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
    until(&shell, cx, "the foot", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    cx.update(|window, cx| window.click(project_blocked_id("atlas"), cx));
    settle(cx);

    // The foot changes where the row sits, not what it says: the subject on its first line, the
    // key as the mono word on its second. It is in its project: it does not say whose it is.
    let (first, scope) = shell.read_with(cx, |s, cx| {
        let row = s.session(&id).unwrap().read(cx);
        (row.first_line(), row.scope().map(str::to_owned))
    });
    assert_eq!(first.as_ref(), "The docs are missing");
    assert_eq!(scope, None);
    let key = cx.update(|window, _| {
        window
            .within(IN_GROUP)
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
    // A row of its project is two lines high: the foot's row keeps that height.
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the other row", |cx| {
        label(cx, session_id(&other)).is_some()
    });
    settle(cx);
    let two_lines = frame_height(&shell, &other, IN_PROJECT, cx).expect("the other row");

    wait_for_maya(&user, &id, &mcp);
    until(&shell, cx, "the foot", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    cx.update(|window, cx| window.click(project_blocked_id("atlas"), cx));
    settle(cx);

    // Line 2 says who and why, as #1508 gave it.
    let says = cx.update(|window, _| {
        window
            .within(IN_GROUP)
            .try_find(session_waits_id(&id))
            .and_then(|found| found.label().map(str::to_owned))
    });
    assert_eq!(says.as_deref(), Some("waits for Maya · review of #12"));
    assert_eq!(frame_height(&shell, &id, IN_GROUP, cx), Some(two_lines));
}

#[gpui_kit::test]
fn its_second_line_comes_back_when_it_is_no_longer_blocked(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the foot", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    cx.update(|window, cx| window.click(project_blocked_id("atlas"), cx));
    settle(cx);
    assert!(
        label(cx, session_waits_id(&id)).is_some(),
        "it says who it waits for at the foot"
    );

    maya_reviews(&user, &id);
    until(&shell, cx, "the row back in its project", |cx| {
        listed(&shell, cx).contains(&id)
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
    until(&shell, cx, "the wait folding away", |cx| {
        drawn_height(cx, "rail-wait-block") < full
    });
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
fn the_window_draws_no_frame_at_rest_on_a_blocked_session_s_page_and_its_foot(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user);
    let (id, mcp) = start(&user, "open the docs pull request");
    wait_for_maya(&user, &id, &mcp);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the foot", |cx| {
        blocked(&shell, cx) == [id.clone()]
    });
    cx.update(|window, cx| window.click(project_blocked_id("atlas"), cx));
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
