//! The scenarios of #1229 (second part of #1181): the projects' list is the user's to arrange. A
//! project row is dragged to its place and the order is kept; groups are made, folded and dragged,
//! and projects go into and out of them; a new project goes at the end; the arrangement is the
//! daemon's, so every window of the machine shows it (`docs/design.md` §10, `docs/protocol.md`,
//! "The draft and window methods"). On GPUI's test platform in front of a real daemon whose agents
//! are the fake ACP agent, with a throwaway user. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::tree::{
    group_count_id, group_header_id, new_group_id, project_id, project_slot_id, projects_menu_id,
};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Bounds, Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext,
    point, px, size,
};
use serde_json::{Value, json};

const FRAME: Duration = Duration::from_millis(16);
const NONE: Modifiers = Modifiers {
    control: false,
    alt: false,
    shift: false,
    platform: false,
    function: false,
};

fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the daemon keeps of the sidebar.
fn kept(user: &User) -> Value {
    user.call("window.get", json!({})).unwrap()["state"]["sidebar"].clone()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|name| (*name).to_owned()).collect()
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
    open(user, cx)
}

/// Another window on the user's daemon.
fn open<'a>(user: &User, cx: &'a mut TestAppContext) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) -> usize {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    asked
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        step(shell, FRAME, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The clock and the frames of a window to the last frame of any motion.
fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..60 {
        step(shell, FRAME, cx);
    }
}

fn present(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn bounds(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> Bounds<Pixels> {
    let id = id.into();
    cx.update(|window, _| window.find(id).bounds())
}

/// A window whose list shows the projects `names`.
fn with_projects<'a>(
    user: &User,
    names: &[&str],
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    for name in names {
        project(user, name);
    }
    let (shell, cx) = window(user, cx);
    let last = names.last().unwrap().to_string();
    until(&shell, cx, "the rows", |cx| present(cx, project_id(&last)));
    settle(&shell, cx);
    (shell, cx)
}

/// The projects outside any group, in the order drawn.
fn loose(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |shell, _| shell.listing().loose)
}

/// The projects in the group called `name`, in the order drawn.
fn held(shell: &Entity<Shell>, name: &str, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |shell, _| {
        shell
            .listing()
            .groups
            .into_iter()
            .find(|group| group.name == name)
            .map(|group| group.projects)
            .unwrap_or_default()
    })
}

fn group_id(shell: &Entity<Shell>, name: &str, cx: &mut VisualTestContext) -> String {
    shell.read_with(cx, |shell, _| {
        shell
            .listing()
            .groups
            .into_iter()
            .find(|group| group.name == name)
            .unwrap_or_else(|| panic!("no group called {name}"))
            .id
    })
}

fn press(at: Point<Pixels>, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.simulate_mouse_move(at, None, NONE);
    cx.simulate_mouse_down(at, MouseButton::Left, NONE);
    step(shell, FRAME, cx);
}

/// Moves the held pointer to `to` in small steps, a frame after each.
fn pull(from: Point<Pixels>, to: Point<Pixels>, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let steps = 8;
    for n in 1..=steps {
        let t = n as f32 / steps as f32;
        let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        cx.simulate_mouse_move(at, Some(MouseButton::Left), NONE);
        step(shell, FRAME, cx);
    }
}

fn release(at: Point<Pixels>, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.simulate_mouse_up(at, MouseButton::Left, NONE);
    step(shell, FRAME, cx);
}

/// Drags the row `from` until the pointer is `below` pixels under the centre of the element `to`
/// (above it when negative), still held.
fn lift(
    from: impl Into<gpui_kit::ElementId>,
    to: impl Into<gpui_kit::ElementId>,
    below: f32,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> (Point<Pixels>, Point<Pixels>) {
    let start = bounds(cx, from).center();
    let target = bounds(cx, to).center();
    press(start, shell, cx);
    let end = point(start.x, target.y + px(below));
    pull(start, end, shell, cx);
    (start, end)
}

fn drag(
    from: impl Into<gpui_kit::ElementId>,
    to: impl Into<gpui_kit::ElementId>,
    below: f32,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) {
    let (_, end) = lift(from, to, below, shell, cx);
    release(end, shell, cx);
}

/// A group made from the Projects header's menu: New group…, its name, Create.
fn new_group(name: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(projects_menu_id(), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(new_group_id(), cx));
    settle(shell, cx);
    assert!(shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()));
    let field = shell.read_with(cx, |s, cx| s.group_form().read(cx).field().clone());
    let name = name.to_owned();
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(&name, window, cx)));
    settle(shell, cx);
    cx.update(|window, cx| window.click("group-name-save", cx));
    settle(shell, cx);
}

#[gpui_kit::test]
fn a_project_is_dragged_to_its_place_and_the_order_is_kept(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie"], cx);
    assert_eq!(loose(&shell, cx), names(&["alpha", "bravo", "charlie"]));

    // alpha is let go a little below charlie: it takes the last place.
    drag(project_id("alpha"), project_id("charlie"), 6., &shell, cx);
    assert_eq!(loose(&shell, cx), names(&["bravo", "charlie", "alpha"]));
    settle(&shell, cx);
    let top = |name: &str, cx: &mut VisualTestContext| bounds(cx, project_slot_id(name)).origin.y;
    let (b, c, a) = (top("bravo", cx), top("charlie", cx), top("alpha", cx));
    assert!(b < c && c < a, "the rows are drawn in the new order");

    // The daemon keeps it: a click on a row after a drag is no click, and the order is written.
    eventually("the daemon keeps the order", || {
        strings(&kept(&user)["order"]) == ["bravo", "charlie", "alpha"]
    });
}

#[gpui_kit::test]
fn a_group_is_made_and_projects_are_dragged_into_it(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie"], cx);
    // Offered with one machine alone: the header's menu is not only for Group by machine.
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    assert!(present(cx, group_header_id(&id)), "the group is a heading");
    assert_eq!(label(cx, group_count_id(&id)).as_deref(), Some("0"));
    assert!(!shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()));

    // A project dropped on the heading is its first place.
    drag(project_id("bravo"), group_header_id(&id), 0., &shell, cx);
    assert_eq!(held(&shell, "Work", cx), names(&["bravo"]));
    assert_eq!(loose(&shell, cx), names(&["alpha", "charlie"]));
    settle(&shell, cx);
    assert_eq!(label(cx, group_count_id(&id)).as_deref(), Some("1"));
    assert!(
        present(cx, project_id("bravo")),
        "the row is under its heading"
    );

    eventually("the daemon keeps the group", || {
        let sidebar = kept(&user);
        sidebar["groups"][0]["name"] == "Work"
            && strings(&sidebar["groups"][0]["projects"]) == ["bravo"]
    });

    // And out of it again, below a project outside any group.
    drag(project_id("bravo"), project_id("charlie"), 6., &shell, cx);
    assert!(held(&shell, "Work", cx).is_empty());
    assert_eq!(loose(&shell, cx), names(&["alpha", "charlie", "bravo"]));
}

#[gpui_kit::test]
fn a_group_folds_and_unfolds_and_the_daemon_keeps_it(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);
    settle(&shell, cx);
    assert!(present(cx, project_id("alpha")));

    cx.update(|window, cx| window.click(group_header_id(&id), cx));
    settle(&shell, cx);
    assert!(!present(cx, project_id("alpha")), "its rows folded away");
    assert_eq!(label(cx, group_count_id(&id)).as_deref(), Some("1"));
    eventually("the daemon keeps the fold", || {
        kept(&user)["groups"][0]["open"] == false
    });

    cx.update(|window, cx| window.click(group_header_id(&id), cx));
    settle(&shell, cx);
    assert!(present(cx, project_id("alpha")), "its rows unfolded");
}

#[gpui_kit::test]
fn groups_are_dragged_among_themselves(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha"], cx);
    new_group("One", &shell, cx);
    new_group("Two", &shell, cx);
    let (one, two) = (group_id(&shell, "One", cx), group_id(&shell, "Two", cx));
    let order = |shell: &Entity<Shell>, cx: &mut VisualTestContext| -> Vec<String> {
        shell.read_with(cx, |s, _| {
            s.listing().groups.into_iter().map(|g| g.name).collect()
        })
    };
    assert_eq!(order(&shell, cx), names(&["One", "Two"]));

    drag(
        group_header_id(&two),
        group_header_id(&one),
        -4.,
        &shell,
        cx,
    );
    assert_eq!(order(&shell, cx), names(&["Two", "One"]));
    eventually("the daemon keeps the groups' order", || {
        kept(&user)["groups"][0]["name"] == "Two"
    });
}

#[gpui_kit::test]
fn removing_a_group_keeps_its_projects(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);
    assert_eq!(held(&shell, "Work", cx), names(&["alpha"]));

    shell.update_in(cx, |shell, window, cx| shell.remove_group(&id, window, cx));
    settle(&shell, cx);
    assert!(!present(cx, group_header_id(&id)), "the heading is gone");
    // The projects are back in the list, none removed.
    assert_eq!(loose(&shell, cx), names(&["bravo", "charlie", "alpha"]));
    assert!(present(cx, project_id("alpha")));
    assert_eq!(
        user.call("project.list", json!({})).unwrap()["projects"]
            .as_array()
            .unwrap()
            .len(),
        3,
        "no project was removed"
    );
    eventually("the daemon forgot the group", || {
        kept(&user)["groups"].is_null()
    });
}

#[gpui_kit::test]
fn a_new_project_goes_at_the_end_and_unfolds(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);
    settle(&shell, cx);
    let full = bounds(cx, project_slot_id("bravo")).size.height;

    project(&user, "delta");
    // From the moment its row is there: it is not full height yet, and grows to it.
    let first = std::cell::Cell::new(None);
    until(&shell, cx, "the new row", |cx| {
        if present(cx, project_slot_id("delta")) {
            first.set(Some(bounds(cx, project_slot_id("delta")).size.height));
            true
        } else {
            false
        }
    });
    assert!(
        first.get().unwrap() < full,
        "the new row unfolds ({:?} of {:?})",
        first.get(),
        full
    );
    settle(&shell, cx);
    assert_eq!(bounds(cx, project_slot_id("delta")).size.height, full);
    // At the end of the list, outside any group.
    assert_eq!(loose(&shell, cx), names(&["bravo", "delta"]));
    assert_eq!(held(&shell, "Work", cx), names(&["alpha"]));
    let y = |name: &str, cx: &mut VisualTestContext| bounds(cx, project_slot_id(name)).origin.y;
    assert!(y("bravo", cx) < y("delta", cx));
}

#[gpui_kit::test]
fn the_arrangement_survives_a_restart_of_the_window(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);
    drag(project_id("bravo"), project_id("charlie"), 6., &shell, cx);
    cx.update(|window, cx| window.click(group_header_id(&id), cx));
    settle(&shell, cx);
    eventually("the daemon keeps it", || {
        let sidebar = kept(&user);
        strings(&sidebar["order"]) == ["charlie", "bravo"] && sidebar["groups"][0]["open"] == false
    });

    // Another window, on the same daemon, shows the same arrangement.
    let (again, cx) = open(&user, cx);
    until(&again, cx, "the group of the new window", |cx| {
        present(cx, group_header_id(&id))
    });
    settle(&again, cx);
    assert_eq!(
        held(&again, "Work", cx),
        names(&["alpha"]),
        "the daemon holds {}, the window lists {:?}",
        kept(&user),
        again.read_with(cx, |s, _| s.listing())
    );
    assert_eq!(loose(&again, cx), names(&["charlie", "bravo"]));
    assert!(
        !present(cx, project_id("alpha")),
        "the group is folded as it was left"
    );
}

/// The frames the window draws, and the animation frames it asks, over ten seconds of its clock.
fn frames_at_rest(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (u64, usize) {
    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        asked += step(shell, FRAME, cx);
        gone += FRAME;
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    (drawn, asked)
}

/// Runs the window until it asked no animation frame for a second of its clock and two of real
/// time (`window_at_rest.rs`).
fn quiet(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let began = Instant::now();
    let mut last_asked = began;
    let mut still = Duration::ZERO;
    while still < Duration::from_secs(1) || last_asked.elapsed() < Duration::from_secs(2) {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "the window never stopped asking for animation frames"
        );
        if step(shell, FRAME, cx) == 0 {
            still += FRAME;
        } else {
            still = Duration::ZERO;
            last_asked = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_with_groups_and_a_finished_drag(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);
    drag(project_id("bravo"), project_id("charlie"), 6., &shell, cx);
    quiet(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "groups and a drag that ended: (drawn, asked)"
    );
}
