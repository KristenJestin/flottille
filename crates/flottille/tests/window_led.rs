//! A session led by the agent that started it (#521) in the window, on GPUI's test platform in
//! front of a real daemon whose agents are the fake ACP agent: its row sits under its pilot's and
//! folds with it, and the user detaches it from its pilot from the row's menu. No window opens on a
//! screen.

mod common;

use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::tree::{session_detach_id, session_id, session_mark_id, session_menu_id};
use gpui_design_system::omarchy::Environment;
use gpui_design_system::{tie_below_id, tie_id, tie_through_id, tie_turn_id, toggle_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn project(user: &User) {
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "TOKEN=made-up-made-up\n").unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
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
}

fn agents(user: &User) -> Vec<serde_json::Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
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

fn label(cx: &mut VisualTestContext, id: gpui_kit::ElementId) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The lead the user started and the session it leads: their ids.
fn lead_and_led(user: &User) -> (String, String) {
    project(user);
    let lead = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": "begin the docs session"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The launch comes to the user; the session starts once they allow it.
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
    eventually("the session exists", || agents(user).len() == 2);
    let launched = agents(user)
        .into_iter()
        .find(|agent| agent["led_by"] == lead.as_str())
        .expect("a session led by the lead")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, launched)
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

fn led_by(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<String> {
    shell.read_with(cx, |s, cx| {
        s.session(id)
            .and_then(|row| row.read(cx).led_by().map(str::to_owned))
    })
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_sidebar_nests_a_led_session_under_its_pilot(cx: &mut TestAppContext) {
    let user = user();
    let (lead, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);

    until(&shell, cx, "the led session's row", |cx| {
        label(cx, session_id(&led)).is_some()
    });

    // Under its pilot: right after the pilot's row, in its own style, with who leads it.
    assert_eq!(listed(&shell, cx), [lead.clone(), led.clone()]);
    assert_eq!(
        led_by(&shell, &led, cx).as_deref(),
        Some("begin the docs session")
    );
    assert_eq!(led_by(&shell, &lead, cx), None);

    // It folds with its pilot's rows and keeps its own row meanwhile.
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    settle(cx);
    assert_eq!(listed(&shell, cx), std::slice::from_ref(&lead));
    assert!(shell.read_with(cx, |s, _| s.session(&led).is_some()));
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    settle(cx);
    assert_eq!(listed(&shell, cx), [lead.clone(), led.clone()]);
}

#[gpui_kit::test]
fn the_window_offers_detach_and_moves_the_session_under_its_workspace(cx: &mut TestAppContext) {
    let user = user();
    let (lead, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led session's row", |cx| {
        label(cx, session_id(&led)).is_some()
    });
    // The row unfolds as it comes: it is clicked once it is there in full.
    settle(cx);
    // Its page says who leads it, and that the user's messages go straight to it.
    cx.update(|window, cx| window.click(session_id(&led), cx));
    until(&shell, cx, "the page says who leads it", |cx| {
        label(cx, "session-led-by".into()).as_deref() == Some("led by begin the docs session")
    });
    let note = label(cx, "session-led-note".into()).expect("the note");
    assert!(note.contains("straight to this session"), "{note}");

    cx.update(|window, cx| window.hover(session_id(&led), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_menu_id(&led), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_detach_id(&led), cx));

    until(&shell, cx, "the session detached", |cx| {
        led_by(&shell, &led, cx).is_none()
    });
    let shown = user.call("agent.show", json!({"agent": led})).unwrap();
    assert_eq!(shown["agent"]["detached_from"], lead.as_str());
    // Back among the project's sessions, its own: no longer under its pilot.
    let rows = listed(&shell, cx);
    assert!(rows.contains(&led) && rows.contains(&lead), "{rows:?}");
    assert_eq!(
        shell.read_with(cx, |s, cx| s
            .toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect::<Vec<_>>()),
        ["Detached from begin the docs session"]
    );
    // The line that said who led it folds away, the note with it.
    settle(cx);
    until(&shell, cx, "the led line gone", |cx| {
        label(cx, "session-led-by".into()).is_none()
            && label(cx, "session-led-note".into()).is_none()
    });
}

/// A lead that launches two led sessions, one of which launches a third: the sessions' ids.
struct Family {
    lead: String,
    docs: String,
    spec: String,
    tests: String,
}

fn family(user: &User) -> Family {
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [
            {"mcp": {"tool": "agent_launch",
                "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}},
            {"mcp": {"tool": "agent_launch",
                "arguments": {"title": "Tests", "brief": "Run the tests", "kind": "led"}}}]},
        {"when": "Write the docs", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Spec", "brief": "Write the spec", "kind": "led"}}}]}
    ]}));
    project(user);
    user.call(
        "agent.start",
        json!({"project": "atlas", "message": "begin the docs session"}),
    )
    .unwrap();
    // Each launch comes to the user and the session starts once they allow it.
    eventually("the four sessions exist", || {
        let held = user.call("approval.list", json!({})).unwrap()["requests"].clone();
        for request in held.as_array().unwrap() {
            user.call("approval.approve", json!({"number": request["number"]}))
                .unwrap();
        }
        agents(user).len() == 4
    });
    let by_title = |title: &str| {
        agents(user)
            .into_iter()
            .find(|agent| agent["title"] == title)
            .unwrap_or_else(|| panic!("a session titled {title}"))["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let lead = agents(user)
        .into_iter()
        .find(|agent| agent["led_by"].is_null())
        .expect("the lead")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    Family {
        lead,
        docs: by_title("Docs"),
        spec: by_title("Spec"),
        tests: by_title("Tests"),
    }
}

/// The element id of the frame a session's row unfolds in.
fn frame_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> gpui_kit::ElementId {
    let row = shell.read_with(cx, |s, _| s.session(id).expect("the row").entity_id());
    gpui_kit::ElementId::Name(format!("tree-row-{}", row.as_u64()).into())
}

fn frame_bounds(
    shell: &Entity<Shell>,
    id: &str,
    cx: &mut VisualTestContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let frame = frame_of(shell, id, cx);
    bounds(cx, frame)
}

fn bounds(
    cx: &mut VisualTestContext,
    id: gpui_kit::ElementId,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update(|window, _| window.find(id).bounds())
}

/// One frame, `by` later.
fn after(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
}

fn the_family<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Family, Entity<Shell>, &'a mut VisualTestContext) {
    let family = family(user);
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the family's rows", |cx| {
        [&family.docs, &family.spec, &family.tests]
            .iter()
            .all(|id| label(cx, session_id(id)).is_some())
    });
    settle(cx);
    (family, shell, cx)
}

#[gpui_kit::test]
fn unfolding_a_sessions_children_is_animated_like_folding_them(cx: &mut TestAppContext) {
    let user = user();
    let (family, shell, cx) = the_family(&user, cx);
    let full = frame_bounds(&shell, &family.docs, cx).size.height;

    cx.update(|window, cx| window.click(toggle_id(session_id(&family.lead)), cx));
    after(cx, Duration::from_millis(70));
    let folding = frame_bounds(&shell, &family.docs, cx).size.height;
    assert!(
        folding > px(0.) && folding < full,
        "the children fold away: {folding:?} of {full:?}"
    );
    settle(cx);
    assert_eq!(listed(&shell, cx), std::slice::from_ref(&family.lead));

    cx.update(|window, cx| window.click(toggle_id(session_id(&family.lead)), cx));
    after(cx, Duration::from_millis(70));
    let unfolding = frame_bounds(&shell, &family.docs, cx).size.height;
    assert!(
        unfolding > px(0.) && unfolding < full,
        "the children unfold from nothing, as they fold away: {unfolding:?} of {full:?}"
    );
    settle(cx);
    assert_eq!(frame_bounds(&shell, &family.docs, cx).size.height, full);
}

#[gpui_kit::test]
fn a_child_session_row_is_tied_to_its_parent_by_a_connector(cx: &mut TestAppContext) {
    let user = user();
    let (family, shell, cx) = the_family(&user, cx);

    // The first row has nothing above it to be tied to.
    assert_eq!(label(cx, tie_id(session_id(&family.lead))), None);
    for child in [&family.docs, &family.tests, &family.spec] {
        assert!(
            label(cx, tie_id(session_id(child))).is_some(),
            "{child} has its connector"
        );
    }

    // It comes down from under the parent's title, turns right at the middle of the row's first
    // line, and runs into the row.
    let parent = bounds(
        cx,
        gpui_kit::ElementId::Name(format!("session-{}-title", family.lead).into()),
    );
    let row = bounds(cx, session_id(&family.docs));
    let turn = bounds(cx, tie_turn_id(session_id(&family.docs)));
    let title = bounds(
        cx,
        gpui_kit::ElementId::Name(format!("session-{}-title", family.docs).into()),
    );
    let near = |a: gpui_kit::Pixels, b: gpui_kit::Pixels| f32::from(a - b).abs() <= 1.5;
    assert_eq!(turn.top(), row.top(), "it comes down from the row above");
    assert!(
        turn.left() > parent.left() && turn.left() < parent.left() + px(12.),
        "under the start of the parent's title: {turn:?} {parent:?}"
    );
    assert!(
        near(turn.bottom() - px(0.5), title.center().y),
        "at the middle of the first line: {turn:?} {title:?}"
    );
    assert!(
        turn.right() > row.left() && turn.right() <= title.left(),
        "it runs into the row: {turn:?} {row:?} {title:?}"
    );

    // A grandchild is tied one level deeper, under its pilot's icon.
    let docs_turn = bounds(cx, tie_turn_id(session_id(&family.docs)));
    let spec_turn = bounds(cx, tie_turn_id(session_id(&family.spec)));
    assert!(spec_turn.left() > docs_turn.left());
    let spec_row = bounds(cx, session_id(&family.spec));
    assert!(spec_row.left() > row.left(), "indented under its pilot");
    assert_eq!(
        listed(&shell, cx).len(),
        4,
        "no row was added for the connector"
    );
}

#[gpui_kit::test]
fn a_led_session_s_connector_ends_without_an_arrowhead(cx: &mut TestAppContext) {
    let user = user();
    let (lead, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led session's connector", |cx| {
        label(cx, tie_id(session_id(&led))).is_some()
    });
    settle(cx);

    // The row draws its connector and its turn, and nothing after the turn: the mark that ended
    // it (`↳`, with the tooltip "led by …") is gone, whoever leads the session being said by the
    // session's own header.
    cx.update(|window, _| {
        assert!(window.try_find(tie_turn_id(session_id(&led))).is_some());
        let arrowhead = gpui_kit::ElementId::Name(format!("session-led-{led}").into());
        assert!(window.try_find(arrowhead).is_none(), "no arrowhead");
    });
    assert_eq!(label(cx, tie_id(session_id(&lead))), None);
}

#[gpui_kit::test]
fn the_last_child_ends_the_connector(cx: &mut TestAppContext) {
    let user = user();
    let (family, shell, cx) = the_family(&user, cx);

    let children: Vec<String> = listed(&shell, cx)
        .into_iter()
        .filter(|id| led_by(&shell, id, cx).is_some() && *id != family.spec)
        .collect();
    let (last, others) = children.split_last().expect("two children");
    assert_eq!(
        label(cx, tie_id(session_id(last))).as_deref(),
        Some("Last branch"),
        "└"
    );
    cx.update(|window, _| assert!(window.try_find(tie_below_id(session_id(last))).is_none()));
    for other in others {
        assert_eq!(
            label(cx, tie_id(session_id(other))).as_deref(),
            Some("Branch"),
            "├"
        );
        cx.update(|window, _| assert!(window.try_find(tie_below_id(session_id(other))).is_some()));
    }

    // Its only child ends its own connector, and the line of its pilot goes on past it while the
    // pilot has a sibling below.
    assert_eq!(
        label(cx, tie_id(session_id(&family.spec))).as_deref(),
        Some("Last branch")
    );
    let docs_is_last = last == &family.docs;
    cx.update(|window, _| {
        assert_eq!(
            window
                .try_find(tie_through_id(session_id(&family.spec), 0))
                .is_some(),
            !docs_is_last
        );
    });
}

#[gpui_kit::test]
fn the_connector_folds_away_with_the_children(cx: &mut TestAppContext) {
    let user = user();
    let (family, shell, cx) = the_family(&user, cx);
    let tie = tie_id(session_id(&family.docs));
    assert!(label(cx, tie.clone()).is_some());
    let full = frame_bounds(&shell, &family.docs, cx).size.height;

    cx.update(|window, cx| window.click(toggle_id(session_id(&family.lead)), cx));
    after(cx, Duration::from_millis(70));
    let folding = frame_bounds(&shell, &family.docs, cx).size.height;
    assert!(
        folding > px(0.) && folding < full,
        "the frame the connector is drawn in is folding: {folding:?} of {full:?}"
    );
    assert!(
        label(cx, tie.clone()).is_some(),
        "it is still drawn while the children fold away"
    );
    settle(cx);
    assert_eq!(label(cx, tie.clone()), None, "gone with the children");

    cx.update(|window, cx| window.click(toggle_id(session_id(&family.lead)), cx));
    settle(cx);
    assert!(label(cx, tie).is_some(), "back with them");
}

/// The family and a second root, which leads no one: its id.
fn the_family_and_a_lone_root<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Family, String, Entity<Shell>, &'a mut VisualTestContext) {
    let family = family(user);
    let lone = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": "a session of its own"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the family's rows and the lone root's", |cx| {
        [&family.docs, &family.spec, &family.tests, &lone]
            .iter()
            .all(|id| label(cx, session_id(id)).is_some())
    });
    settle(cx);
    (family, lone, shell, cx)
}

fn title_of(cx: &mut VisualTestContext, id: &str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    bounds(
        cx,
        gpui_kit::ElementId::Name(format!("session-{id}-title").into()),
    )
}

#[gpui_kit::test]
fn roots_with_and_without_children_share_their_title_offset(cx: &mut TestAppContext) {
    let user = user();
    let (family, lone, _shell, cx) = the_family_and_a_lone_root(&user, cx);

    let with = title_of(cx, &family.lead);
    let without = title_of(cx, &lone);
    assert_eq!(
        with.left(),
        without.left(),
        "two roots are one level: {with:?} {without:?}"
    );

    // The chevron is in the row's trailing area, after the title, not before it.
    let chevron = bounds(cx, toggle_id(session_id(&family.lead)));
    assert!(
        chevron.left() >= with.right(),
        "the chevron is after the title: {chevron:?} {with:?}"
    );
    let row = bounds(cx, session_id(&family.lead));
    assert!(chevron.right() <= row.right());
}

#[gpui_kit::test]
fn a_led_session_is_offset_one_step_and_draws_its_connector(cx: &mut TestAppContext) {
    let user = user();
    let (family, lone, _shell, cx) = the_family_and_a_lone_root(&user, cx);

    let lead = title_of(cx, &family.lead);
    let docs = title_of(cx, &family.docs);
    let spec = title_of(cx, &family.spec);
    let step = docs.left() - lead.left();
    assert!(
        step > px(0.),
        "a led session is indented: {lead:?} {docs:?}"
    );
    assert_eq!(
        spec.left() - docs.left(),
        step,
        "every level is indented by the same step"
    );
    assert_eq!(
        title_of(cx, &lone).left(),
        lead.left(),
        "only the led sessions are indented"
    );

    // The line from its parent turns into the row, ending before its title.
    let near = |a: gpui_kit::Pixels, b: gpui_kit::Pixels| f32::from(a - b).abs() <= 1.5;
    let turn = bounds(cx, tie_turn_id(session_id(&family.docs)));
    assert!(
        turn.left() >= lead.left() && turn.right() <= docs.left(),
        "it comes down inside the parent's title and ends before the child's: {turn:?} {lead:?} {docs:?}"
    );
    assert!(near(turn.bottom() - px(0.5), docs.center().y));
    assert!(label(cx, tie_id(session_id(&lone))).is_none());
}

fn mark_of(
    shell: &Entity<Shell>,
    id: &str,
    cx: &mut VisualTestContext,
) -> Option<flottille_window::state::AgentState> {
    shell.read_with(cx, |s, cx| {
        s.session(id).and_then(|row| row.read(cx).mark())
    })
}

#[gpui_kit::test]
fn a_folded_root_shows_that_a_led_session_runs(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]},
        {"when": "Write the docs", "do": [{"hang": true}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (lead, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led session runs", |cx| {
        mark_of(&shell, &led, cx) == Some(flottille_window::state::AgentState::Running)
    });
    settle(cx);

    // Unfolded, each row shows its own: the led session spins, its pilot waits for it.
    assert_eq!(
        mark_of(&shell, &lead, cx),
        Some(flottille_window::state::AgentState::Waiting)
    );

    // Folded, the pilot's row shows what its hidden led session does.
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    settle(cx);
    assert_eq!(listed(&shell, cx), std::slice::from_ref(&lead));
    assert_eq!(
        mark_of(&shell, &lead, cx),
        Some(flottille_window::state::AgentState::Running),
        "the folded root spins for the session it hides"
    );

    // Unfolded again, nothing changes: the pilot waits, the led session spins.
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    settle(cx);
    assert_eq!(
        mark_of(&shell, &lead, cx),
        Some(flottille_window::state::AgentState::Waiting)
    );
    assert_eq!(
        mark_of(&shell, &led, cx),
        Some(flottille_window::state::AgentState::Running)
    );
}

const QUESTION: &str = "Cover the logout too?";

/// A lead that starts a led session, which asks its lead a question and goes on.
fn asking() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]},
        {"when": "Write the docs", "do": [
            {"mcp": {"tool": "ask", "arguments": {"question": QUESTION}}},
            {"say": "Asked."}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The page of the led session, opened from its row.
fn open_led(shell: &Entity<Shell>, led: &str, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    until(shell, cx, "the led session's row", |cx| {
        label(cx, session_id(led)).is_some()
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(led), cx));
    until(shell, cx, "the led session's page", |cx| {
        shell
            .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
            .is_some()
    });
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .unwrap()
}

/// The row of the first block of the conversation that `pick` finds.
fn row_of(
    page: &Entity<SessionPage>,
    pick: impl Fn(&Block) -> bool,
    cx: &mut VisualTestContext,
) -> Option<usize> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().iter().position(&pick)
    })
}

#[gpui_kit::test]
fn a_led_session_s_question_card_says_it_asks_its_pilot(cx: &mut TestAppContext) {
    let user = asking();
    let (_, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_led(&shell, &led, cx);

    let seen = Cell::new(None);
    until(&shell, cx, "the card of the question", |cx| {
        seen.set(row_of(
            &page,
            |block| matches!(block, Block::Asked { .. }),
            cx,
        ));
        seen.get()
            .is_some_and(|at| label(cx, format!("row-{at}-asked-title").into()).is_some())
    });
    let at = seen.get().unwrap();
    assert_eq!(
        label(cx, format!("row-{at}-asked-title").into()).as_deref(),
        Some("Write the docs asks its lead begin the docs session")
    );
    assert_eq!(
        label(cx, format!("row-{at}-asked-note").into()).as_deref(),
        Some("Its lead answers at the end of its turn; you can answer here too.")
    );
    // A card of the user's own would say it asks them, with a field.
    assert!(row_of(&page, |block| matches!(block, Block::Question { .. }), cx).is_none());
}

#[gpui_kit::test]
fn once_the_pilot_stops_following_the_card_says_it_asks_you(cx: &mut TestAppContext) {
    let user = asking();
    let (_, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_led(&shell, &led, cx);
    let seen = Cell::new(None);
    until(&shell, cx, "the card of the question", |cx| {
        seen.set(row_of(
            &page,
            |block| matches!(block, Block::Asked { .. }),
            cx,
        ));
        seen.get()
            .is_some_and(|at| label(cx, format!("row-{at}-asked-title").into()).is_some())
    });
    let asked = seen.get().unwrap();

    // The user takes the session from its pilot: its open question goes to them.
    user.call("agent.detach", json!({"agent": led})).unwrap();

    let seen = Cell::new(None);
    until(&shell, cx, "the question is the user's", |cx| {
        seen.set(row_of(
            &page,
            |block| matches!(block, Block::Question { .. }),
            cx,
        ));
        seen.get()
            .is_some_and(|at| label(cx, format!("row-{at}-question-title").into()).is_some())
    });
    let now = seen.get().unwrap();
    assert_eq!(
        label(cx, format!("row-{now}-question-title").into()).as_deref(),
        Some("Write the docs")
    );
    assert_eq!(
        label(cx, format!("row-{now}-question-tag").into()).as_deref(),
        Some("asks you")
    );
    // The card of the pilot folds away: nobody is waiting for it any more.
    settle(cx);
    assert!(label(cx, format!("row-{asked}-asked-title").into()).is_none());
    assert!(label(cx, format!("row-{asked}-asked-note").into()).is_none());
}

/// A lead with two led sessions that are in a turn, as the sidebar draws them: two-line rows.
fn two_in_a_turn(user: &User) -> Family {
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [
            {"mcp": {"tool": "agent_launch",
                "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}},
            {"mcp": {"tool": "agent_launch",
                "arguments": {"title": "Tests", "brief": "Run the tests", "kind": "led"}}}]},
        {"when": "Write the docs", "do": [{"hang": true}]},
        {"when": "Run the tests", "do": [{"hang": true}]}
    ]}));
    project(user);
    user.call(
        "agent.start",
        json!({"project": "atlas", "message": "begin the docs session"}),
    )
    .unwrap();
    eventually("the three sessions exist", || {
        let held = user.call("approval.list", json!({})).unwrap()["requests"].clone();
        for request in held.as_array().unwrap() {
            user.call("approval.approve", json!({"number": request["number"]}))
                .unwrap();
        }
        agents(user).len() == 3
    });
    let by_title = |title: &str| {
        agents(user)
            .into_iter()
            .find(|agent| agent["title"] == title)
            .unwrap_or_else(|| panic!("a session titled {title}"))["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let lead = agents(user)
        .into_iter()
        .find(|agent| agent["led_by"].is_null())
        .expect("the lead")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    Family {
        lead,
        docs: by_title("Docs"),
        spec: String::new(),
        tests: by_title("Tests"),
    }
}

fn the_led_in_a_turn<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Family, Entity<Shell>, &'a mut VisualTestContext) {
    let family = two_in_a_turn(user);
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "both led sessions spin", |cx| {
        [&family.docs, &family.tests]
            .iter()
            .all(|id| mark_of(&shell, id, cx) == Some(flottille_window::state::AgentState::Running))
    });
    settle(cx);
    (family, shell, cx)
}

#[gpui_kit::test]
fn a_led_row_s_tie_turns_at_the_middle_of_its_first_line(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (family, _shell, cx) = the_led_in_a_turn(&user, cx);
    cx.update(|window, cx| window.click(toggle_id(session_id(&family.lead)), cx));
    settle(cx);
    cx.update(|window, cx| window.click(toggle_id(session_id(&family.lead)), cx));
    settle(cx);

    let near = |a: gpui_kit::Pixels, b: gpui_kit::Pixels| f32::from(a - b).abs() <= 1.;
    for led in [&family.docs, &family.tests] {
        let row = bounds(cx, session_id(led));
        let turn = bounds(cx, tie_turn_id(session_id(led)));
        let mark = bounds(cx, session_mark_id(led));
        let title = title_of(cx, led);
        assert_eq!(turn.top(), row.top(), "it comes down from the row above");
        // The turn's line is its bottom border, the last pixel of its box.
        assert!(
            near(turn.bottom() - px(0.5), mark.center().y),
            "at the middle of the mark: {turn:?} {mark:?} {row:?}"
        );
        assert!(
            near(turn.bottom() - px(0.5), title.center().y),
            "at the middle of the title: {turn:?} {title:?} {row:?}"
        );
    }
}

#[gpui_kit::test]
fn the_tie_runs_on_between_two_rows_of_the_same_lead(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (family, shell, cx) = the_led_in_a_turn(&user, cx);

    let rows = listed(&shell, cx);
    let (first, second) = (rows[1].clone(), rows[2].clone());
    let below = bounds(cx, tie_below_id(session_id(&first)));
    let next = bounds(cx, tie_turn_id(session_id(&second)));
    let row = bounds(cx, session_id(&first));
    assert_eq!(
        below.left(),
        bounds(cx, tie_turn_id(session_id(&first))).left()
    );
    assert_eq!(below.left(), next.left(), "one line: {below:?} {next:?}");
    assert!(
        next.top() - below.bottom() <= px(0.5),
        "no gap between the first row's line and the second row's turn: {below:?} {next:?} {row:?}"
    );
    let _ = family;
}
