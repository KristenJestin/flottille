//! The sidebar's "Ended" section (#642), on GPUI's test platform in front of a real daemon whose
//! agents are the fake ACP agent: unfolded it takes a maximum height and scrolls inside itself, and
//! its rows come in the order the sessions ended. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::tree::{ended_id, ended_scroll_id, project_id, session_id};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// How many sessions end: more rows than the section shows.
const ENDED: usize = 12;

fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

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
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Starts a session and waits until its first turn ended.
fn start(user: &User, message: &str) -> String {
    let id = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": message}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the first turn ends", || state_of(user, &id) == "idle");
    id
}

/// Ends the sessions one after the other: the last one ended last.
fn end_in_order(user: &User, ids: &[&str]) {
    for id in ids {
        user.call("agent.stop", json!({"agent": id})).unwrap();
        eventually("the session ended", || state_of(user, id) == "ended");
        // The clock the daemon stamps an end with is in milliseconds.
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn window<'a>(
    user: &User,
    height: f32,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(height)));
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

/// The sessions the "Ended" group lists, in the order drawn.
fn ended_group(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.ended()
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

/// A window on a project that holds `ENDED` ended sessions, the "Ended" group unfolded.
fn unfolded<'a>(
    user: &User,
    height: f32,
    cx: &'a mut TestAppContext,
) -> (Vec<String>, Entity<Shell>, &'a mut VisualTestContext) {
    project(user);
    let ids: Vec<String> = (0..ENDED)
        .map(|n| start(user, &format!("session number {n}")))
        .collect();
    end_in_order(user, &ids.iter().map(String::as_str).collect::<Vec<_>>());
    let (shell, cx) = window(user, height, cx);
    until(&shell, cx, "the ended sessions", |cx| {
        ended_group(&shell, cx).len() == ENDED
    });
    settle(cx);
    cx.update(|window, cx| window.click(ended_id(), cx));
    settle(cx);
    (ids, shell, cx)
}

fn bounds(
    cx: &mut VisualTestContext,
    id: impl Into<gpui_kit::ElementId>,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let id = id.into();
    cx.update(|window, _| window.find(id).bounds())
}

#[gpui_kit::test]
fn the_ended_section_never_grows_past_its_maximum_height(cx: &mut TestAppContext) {
    let user = user();
    let (ids, _shell, cx) = unfolded(&user, 800., cx);

    // Eight rows at most, below 35% of the sidebar's height.
    let section = bounds(cx, ended_scroll_id());
    assert!(
        section.size.height > px(0.) && section.size.height <= px(8. * 28. + 8.),
        "about eight rows: {section:?}"
    );
    let first = bounds(cx, session_id(&ids[ENDED - 1]));
    let last = bounds(cx, session_id(&ids[0]));
    assert!(
        last.bottom() - first.top() > section.size.height,
        "the rows are more than the section shows"
    );

    // The window's own height decides when 35% of it is less than eight rows.
    cx.simulate_resize(size(px(1280.), px(400.)));
    settle(cx);
    let section = bounds(cx, ended_scroll_id());
    assert!(
        section.size.height > px(0.) && section.size.height <= px(400. * 0.35),
        "35% of the sidebar when that is less: {section:?}"
    );
}

#[gpui_kit::test]
fn the_ended_section_scrolls_inside_itself(cx: &mut TestAppContext) {
    let user = user();
    let (ids, _shell, cx) = unfolded(&user, 800., cx);

    let section = bounds(cx, ended_scroll_id());
    let project_before = bounds(cx, project_id("atlas"));
    let header_before = bounds(cx, ended_id());
    let oldest = session_id(&ids[0]);
    assert!(
        bounds(cx, oldest.clone()).top() >= section.bottom(),
        "the oldest ended session is beyond the section's edge"
    );

    cx.update(|window, cx| {
        window.scroll(
            ended_scroll_id(),
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-2000.))),
            cx,
        )
    });
    settle(cx);

    let after = bounds(cx, oldest);
    assert!(
        after.bottom() <= section.bottom() + px(4.) && after.top() >= section.top(),
        "scrolled into view inside the section: {after:?} in {section:?}"
    );
    assert_eq!(bounds(cx, ended_scroll_id()), section, "the section stays");
    assert_eq!(bounds(cx, ended_id()), header_before, "its header stays");
    assert_eq!(
        bounds(cx, project_id("atlas")),
        project_before,
        "the rest of the sidebar keeps its place"
    );
}

#[gpui_kit::test]
fn ended_sessions_are_listed_most_recent_first(cx: &mut TestAppContext) {
    let user = user();
    project(&user);
    let first = start(&user, "ended first");
    let second = start(&user, "ended second");
    let third = start(&user, "ended third");
    // Started in one order, ended in another: the row follows the end, not the start.
    end_in_order(&user, &[&second, &third, &first]);
    let (shell, cx) = window(&user, 800., cx);
    until(&shell, cx, "the three ended sessions", |cx| {
        ended_group(&shell, cx).len() == 3
    });
    assert_eq!(ended_group(&shell, cx), [first, third, second]);
}
