//! The scenarios of #1229 (second part of #1181): the projects' list is the user's to arrange. A
//! project row is dragged to its place and the order is kept; groups are made, folded and dragged,
//! and projects go into and out of them; a new project goes at the end; the arrangement is the
//! daemon's, so every window of the machine shows it (`docs/design.md` §10, `docs/protocol.md`,
//! "The draft and window methods"). On GPUI's test platform in front of a real daemon whose agents
//! are the fake ACP agent, with a throwaway user. No window opens on a screen.

use crate::common;

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::state::{AgentState, icon_id};
use flottille_window::tree::{
    by_machine_id, group_count_id, group_header_id, group_mark_id, group_menu_id, group_remove_id,
    group_rename_id, new_group_id, project_id, project_slot_id, projects_menu_id, session_id,
};
use gpui_design_system::motion::PANEL;
use gpui_design_system::omarchy::Environment;
use gpui_design_system::toggle_id;
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

/// What the daemon keeps of the user's arrangement, under its own key (the sidebar's folds are
/// another's: a window that only folds a project never writes it).
fn kept(user: &User) -> Value {
    user.call("window.get", json!({})).unwrap()["state"]["arrangement"].clone()
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
    window_through(user, Arc::new(user.connector()), cx)
}

/// A window whose connections to the daemon are `connect`'s.
fn window_through<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    open_through(connect, cx)
}

/// Another window, on the daemon `connect` reaches.
fn open_through(
    connect: Connect,
    cx: &mut TestAppContext,
) -> (Entity<Shell>, &mut VisualTestContext) {
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

/// The dialog of New group… asked for `name` and Create pressed, nothing settled after it. A
/// dialog that is open already (a refusal keeps it) is not opened again.
fn ask_new_group(name: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    if !shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()) {
        cx.update(|window, cx| window.click(projects_menu_id(), cx));
        settle(shell, cx);
        cx.update(|window, cx| window.click(new_group_id(), cx));
        settle(shell, cx);
    }
    assert!(shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()));
    let field = shell.read_with(cx, |s, cx| s.group_form().read(cx).field().clone());
    let name = name.to_owned();
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(&name, window, cx)));
    settle(shell, cx);
    cx.update(|window, cx| window.click("group-name-save", cx));
}

/// A group made from the Projects header's menu: New group…, its name, Create.
fn new_group(name: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    ask_new_group(name, shell, cx);
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
        kept(&user)["groups"].as_array().is_some_and(Vec::is_empty)
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
fn the_arrangement_survives_a_restart_of_the_window(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie"], first);
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

    // The first window is left as it is, never stepped again, and the app starts anew on the same
    // daemon (as `window_kept_state.rs` restarts one): the arrangement is the daemon's.
    let (again, cx) = window(&user, second);
    until(
        &again,
        cx,
        "the group and the rows of the new window",
        |cx| present(cx, group_header_id(&id)) && present(cx, project_id("charlie")),
    );
    settle(&again, cx);
    assert_eq!(
        held(&again, "Work", cx),
        names(&["alpha"]),
        "the daemon holds {}",
        kept(&user)
    );
    assert_eq!(loose(&again, cx), names(&["charlie", "bravo"]));
    assert!(
        !present(cx, project_id("alpha")),
        "the group is folded as it was left"
    );
}

#[gpui_kit::test]
fn a_new_groups_heading_unfolds_and_no_row_jumps(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo"], cx);
    let y = |cx: &mut VisualTestContext| f32::from(bounds(cx, project_slot_id("alpha")).origin.y);
    let before = y(cx);

    ask_new_group("Work", &shell, cx);
    let mut ys = Vec::new();
    for _ in 0..30 {
        step(&shell, FRAME, cx);
        ys.push(y(cx));
    }
    let id = group_id(&shell, "Work", cx);
    let heading = f32::from(bounds(cx, group_header_id(&id)).size.height);
    let end = *ys.last().unwrap();
    assert_eq!(end - before, heading, "the rows made room for the heading");
    assert!(
        ys.iter().any(|y| *y > before && *y < end),
        "the heading unfolds: {ys:?}"
    );
    assert!(
        ys.windows(2).all(|pair| pair[0] <= pair[1]),
        "the rows only go down: {ys:?}"
    );
    assert!(
        ys[0] - before < heading / 2.,
        "nothing jumps when the group comes: {ys:?}"
    );
}

#[gpui_kit::test]
fn the_arrangement_arriving_after_the_rows_slides_them_into_their_groups(cx: &mut TestAppContext) {
    let user = user();
    for name in ["alpha", "bravo", "charlie", "delta"] {
        project(&user, name);
    }
    // The daemon holds nothing yet: the window opens, and its rows are drawn and at rest.
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the rows", |cx| {
        present(cx, project_id("delta"))
    });
    settle(&shell, cx);
    assert!(held(&shell, "Work", cx).is_empty());

    // The arrangement comes later: another window made it, and this one reads it on its next
    // change (a change that changes nothing here, of a group it has never heard of).
    user.call(
        "window.set",
        json!({"key": "arrangement", "value": {
            "order": ["alpha", "delta"],
            "groups": [{"id": "g1", "name": "Work", "open": true,
                "projects": ["charlie", "bravo"]}]}}),
    )
    .unwrap();
    shell.update_in(cx, |shell, window, cx| {
        shell.group_opened("g1", true, window, cx)
    });

    // Every frame from then to the settled list: where each row is.
    let rows = ["alpha", "bravo", "charlie", "delta"];
    let mut seen: Vec<Vec<f32>> = Vec::new();
    let began = Instant::now();
    let mut arrived = None;
    loop {
        step(&shell, FRAME, cx);
        seen.push(
            rows.iter()
                .map(|name| f32::from(bounds(cx, project_slot_id(name)).origin.y))
                .collect(),
        );
        if arrived.is_none() && !held(&shell, "Work", cx).is_empty() {
            arrived = Some(seen.len());
        }
        if arrived.is_some_and(|at| seen.len() > at + 60)
            || began.elapsed() > Duration::from_secs(25)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(arrived.is_some(), "the arrangement never arrived");
    assert_eq!(held(&shell, "Work", cx), names(&["charlie", "bravo"]));
    let mut moved = 0;
    for (at, name) in rows.iter().enumerate() {
        let path: Vec<f32> = seen.iter().map(|frame| frame[at]).collect();
        let (from, to) = (path[0], *path.last().unwrap());
        let worst = path
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0., f32::max);
        if (to - from).abs() >= 10. {
            moved += 1;
            assert!(
                worst <= (to - from).abs() * 0.6,
                "{name} jumps ({from} to {to}, {worst} in one frame): {path:?}"
            );
        }
    }
    assert!(moved > 0, "the rows moved into their groups: {seen:?}");
}

#[gpui_kit::test]
fn groups_made_in_one_window_survive_a_sidebar_write_from_another(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "alpha");
    project(&user, "bravo");
    let (one, cx_one) = window(&user, first);
    until(&one, cx_one, "the rows of the window", |cx| {
        present(cx, project_id("bravo"))
    });
    let (two, cx_two) = window(&user, second);
    until(&two, cx_two, "the rows of the other window", |cx| {
        present(cx, project_id("bravo"))
    });

    new_group("Work", &one, cx_one);
    eventually("the daemon keeps the group", || {
        kept(&user)["groups"][0]["name"] == "Work"
    });

    // The other window, which never heard of the group, folds a project: it writes the sidebar.
    cx_two.update(|window, cx| window.click(toggle_id(project_id("alpha")), cx));
    settle(&two, cx_two);
    eventually("the daemon keeps the fold", || {
        user.call("window.get", json!({})).unwrap()["state"]["sidebar"]["projects"]["alpha"]
            == false
    });
    assert_eq!(
        kept(&user)["groups"][0]["name"],
        "Work",
        "a write of the folds never erases the groups"
    );
}

#[gpui_kit::test]
fn a_project_absent_while_the_user_drags_returns_to_its_place_and_group(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("bravo"), group_header_id(&id), 0., &shell, cx);
    assert_eq!(held(&shell, "Work", cx), names(&["bravo"]));

    // The project is gone for a while (as a machine's is while it is not reached).
    user.call("project.remove", json!({"name": "bravo"}))
        .unwrap();
    until(&shell, cx, "the row to go", |cx| {
        !present(cx, project_id("bravo"))
    });
    settle(&shell, cx);
    drag(project_id("alpha"), project_id("charlie"), 6., &shell, cx);
    assert_eq!(loose(&shell, cx), names(&["charlie", "alpha"]));

    user.call(
        "project.add",
        json!({"folder": user.path("work/bravo"), "name": "bravo"}),
    )
    .unwrap();
    until(&shell, cx, "the row to come back", |cx| {
        present(cx, project_id("bravo"))
    });
    settle(&shell, cx);
    assert_eq!(held(&shell, "Work", cx), names(&["bravo"]), "its group");
    assert_eq!(loose(&shell, cx), names(&["charlie", "alpha"]));
}

#[gpui_kit::test]
fn a_window_that_never_heard_of_a_group_does_not_erase_it(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = user();
    project(&user, "alpha");
    project(&user, "bravo");
    let (one, cx_one) = window(&user, first);
    until(&one, cx_one, "the rows of the window", |cx| {
        present(cx, project_id("bravo"))
    });
    // Open before the group is made: it reads the arrangement once, as it opens.
    let (two, cx_two) = window(&user, second);
    until(&two, cx_two, "the rows of the other window", |cx| {
        present(cx, project_id("bravo"))
    });
    settle(&two, cx_two);

    new_group("Work", &one, cx_one);
    eventually("the daemon keeps the group", || {
        kept(&user)["groups"][0]["name"] == "Work"
    });

    // The other window drags a project: what it writes is the daemon's arrangement with this
    // change, not its own copy.
    drag(project_id("alpha"), project_id("bravo"), 6., &two, cx_two);
    eventually("the daemon keeps the order", || {
        strings(&kept(&user)["order"]) == ["bravo", "alpha"]
    });
    assert_eq!(
        kept(&user)["groups"][0]["name"],
        "Work",
        "the group made elsewhere is kept"
    );
    until(&two, cx_two, "the group in the window that moved", |cx| {
        two.read_with(cx, |s, _| {
            s.listing().groups.iter().any(|group| group.name == "Work")
        })
    });
}

#[gpui_kit::test]
fn the_projects_menu_offers_group_by_machine_disabled_while_there_is_one_machine(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha"], cx);
    assert!(!shell.read_with(cx, |s, cx| s.offers_grouping(cx)));
    cx.update(|window, cx| window.click(projects_menu_id(), cx));
    settle(&shell, cx);
    assert!(
        present(cx, by_machine_id()),
        "the switch is in the menu from the start"
    );
    assert_eq!(
        label(cx, "projects-by-machine-detail").as_deref(),
        Some("One machine"),
        "and says why it is off"
    );
    cx.update(|window, cx| window.click(by_machine_id(), cx));
    settle(&shell, cx);
    assert!(
        !shell.read_with(cx, |s, _| s.is_by_machine()),
        "it does nothing"
    );
}

#[gpui_kit::test]
fn a_project_dropped_below_a_row_still_folding_away_lands_where_it_was_let_go(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie", "delta"], cx);
    // alpha is held a little below charlie.
    let (_, end) = lift(project_id("alpha"), project_id("charlie"), 6., &shell, cx);
    // bravo goes while it is held: its row folds away, still drawn between alpha and charlie.
    user.call("project.remove", json!({"name": "bravo"}))
        .unwrap();
    until(&shell, cx, "bravo to leave the list", |cx| {
        !shell.read_with(cx, |s, _| s.listing().loose.contains(&"bravo".to_owned()))
    });
    assert!(
        present(cx, project_slot_id("bravo")),
        "the row is still folding away"
    );
    release(end, &shell, cx);
    assert_eq!(loose(&shell, cx), names(&["charlie", "alpha", "delta"]));
}

#[gpui_kit::test]
fn a_press_on_a_session_row_never_drags_the_project(cx: &mut TestAppContext) {
    let user = user();
    for name in ["alpha", "bravo", "charlie"] {
        project(&user, name);
    }
    let session = user
        .call(
            "agent.start",
            json!({"project": "alpha", "message": "Say hello"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session's row", |cx| {
        present(cx, session_id(&session)) && present(cx, project_id("charlie"))
    });
    settle(&shell, cx);

    // A press on the session row, moved past a project below: the project is not moved with it.
    let start = bounds(cx, session_id(&session)).center();
    let end = point(
        start.x,
        bounds(cx, project_id("charlie")).center().y + px(6.),
    );
    press(start, &shell, cx);
    pull(start, end, &shell, cx);
    release(end, &shell, cx);
    settle(&shell, cx);
    assert_eq!(loose(&shell, cx), names(&["alpha", "bravo", "charlie"]));
}

/// The dialog of New group… refused `name`, with what it says, and made no group.
fn refused(name: &str, says: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let groups = shell.read_with(cx, |s, _| s.listing().groups.len());
    ask_new_group(name, shell, cx);
    settle(shell, cx);
    assert!(
        shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()),
        "the dialog stays open on {name:?}"
    );
    assert_eq!(
        shell
            .read_with(cx, |s, cx| s
                .group_form()
                .read(cx)
                .error()
                .map(str::to_owned))
            .as_deref(),
        Some(says)
    );
    assert!(present(cx, "group-name-error"), "and says it");
    assert_eq!(
        shell.read_with(cx, |s, _| s.listing().groups.len()),
        groups,
        "no group was made"
    );
}

#[gpui_kit::test]
fn a_group_name_that_is_empty_is_refused(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha"], cx);
    refused("   ", "Give the group a name.", &shell, cx);
}

#[gpui_kit::test]
fn a_group_name_already_taken_is_refused_whatever_its_case(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha"], cx);
    new_group("Équipe", &shell, cx);
    // Beyond ASCII too: the same word in another case is the same name.
    refused("équipe", "A group of that name exists.", &shell, cx);
    refused("ÉQUIPE ", "A group of that name exists.", &shell, cx);
}

#[gpui_kit::test]
fn a_group_name_longer_than_sixty_characters_is_refused(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha"], cx);
    let long = "é".repeat(61);
    refused(
        &long,
        "A group's name is 60 characters at most.",
        &shell,
        cx,
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

/// The titles and second lines of the toasts shown.
fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<(String, Option<String>)> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| {
                (
                    toast.title().to_owned(),
                    toast.second_line().map(str::to_owned),
                )
            })
            .collect()
    })
}

/// A group's "⋯" menu opened and `entry` picked, nothing settled after it.
fn from_menu(
    group: &str,
    entry: gpui_kit::ElementId,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) {
    cx.update(|window, cx| window.click(group_menu_id(group), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(entry, cx));
}

fn is_open(shell: &Entity<Shell>, group: &str, cx: &mut VisualTestContext) -> bool {
    shell.read_with(cx, |shell, _| {
        shell
            .listing()
            .groups
            .into_iter()
            .find(|held| held.id == group)
            .is_some_and(|held| held.open)
    })
}

#[gpui_kit::test]
fn a_group_is_renamed_from_its_menu(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);

    from_menu(&id, group_rename_id(&id), &shell, cx);
    settle(&shell, cx);
    assert!(
        is_open(&shell, &id, cx),
        "a press on the menu does not fold the group"
    );
    // The New group dialog in a rename mode: its field holds the name the group has.
    assert!(shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()));
    let field = shell.read_with(cx, |s, cx| s.group_form().read(cx).field().clone());
    assert_eq!(
        field.read_with(cx, |field, _| field.value().to_string()),
        "Work"
    );
    assert_eq!(label(cx, "group-name-save").as_deref(), Some("Save"));
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value("Office", window, cx)));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("group-name-save", cx));
    settle(&shell, cx);

    assert!(!shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()));
    assert_eq!(group_id(&shell, "Office", cx), id, "the same group");
    assert_eq!(
        held(&shell, "Office", cx),
        names(&["alpha"]),
        "with its projects"
    );
    assert_eq!(
        label(
            cx,
            gpui_kit::ElementId::Name(format!("projects-list-group-{id}-title").into())
        )
        .as_deref(),
        Some("Office"),
        "its heading says it"
    );
    eventually("the daemon kept the name", || {
        kept(&user)["groups"][0]["name"] == "Office"
    });
}

#[gpui_kit::test]
fn a_group_cannot_take_the_name_of_another_but_may_change_its_own_case(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha"], cx);
    new_group("Work", &shell, cx);
    new_group("Home", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    let rename = |name: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext| {
        if !shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()) {
            from_menu(&id, group_rename_id(&id), shell, cx);
            settle(shell, cx);
        }
        let field = shell.read_with(cx, |s, cx| s.group_form().read(cx).field().clone());
        let name = name.to_owned();
        cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(&name, window, cx)));
        settle(shell, cx);
        cx.update(|window, cx| window.click("group-name-save", cx));
        settle(shell, cx);
    };
    rename("home", &shell, cx);
    assert_eq!(
        shell.read_with(cx, |s, cx| s
            .group_form()
            .read(cx)
            .error()
            .map(str::to_owned)),
        Some("A group of that name exists.".to_owned())
    );
    assert!(shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()));
    // Its own name, in another case, is its own to take.
    rename("WORK", &shell, cx);
    assert!(!shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()));
    assert_eq!(group_id(&shell, "WORK", cx), id);
}

#[gpui_kit::test]
fn removing_a_group_from_its_menu_keeps_its_projects_and_folds_it_away(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo", "charlie"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);
    drag(project_id("bravo"), group_header_id(&id), 0., &shell, cx);
    assert_eq!(held(&shell, "Work", cx), names(&["bravo", "alpha"]));
    settle(&shell, cx);
    let before = bounds(cx, group_header_id(&id)).size.height;

    from_menu(&id, group_remove_id(&id), &shell, cx);
    // Half-way: the heading is still there, folding away with its gap.
    step(&shell, PANEL / 2, cx);
    assert!(present(cx, group_header_id(&id)), "the heading folds away");
    let half = bounds(cx, group_header_id(&id)).size.height;
    assert!(half > px(0.) && half < before, "{half:?} of {before:?}");
    assert!(
        held(&shell, "Work", cx).is_empty(),
        "the arrangement has forgotten the group at once"
    );

    settle(&shell, cx);
    assert!(!present(cx, group_header_id(&id)), "the heading is gone");
    assert_eq!(loose(&shell, cx), names(&["charlie", "bravo", "alpha"]));
    assert!(present(cx, project_id("alpha")) && present(cx, project_id("bravo")));
    assert_eq!(
        user.call("project.list", json!({})).unwrap()["projects"]
            .as_array()
            .unwrap()
            .len(),
        3,
        "no project was removed"
    );
    // The result is a toast, that says nothing was removed.
    assert_eq!(
        toasts(&shell, cx),
        vec![(
            "Group removed".to_owned(),
            Some("Its 2 projects are back in the list.".to_owned())
        )]
    );
    eventually("the daemon forgot the group", || {
        kept(&user)["groups"].as_array().is_some_and(Vec::is_empty)
    });
}

#[gpui_kit::test]
fn a_folded_group_shows_its_strongest_mark(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let answer = user
        .call("agent.start", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let agent = answer["agent"]["id"].as_str().unwrap().to_owned();
    let state = |user: &User| {
        user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    eventually("the first turn ends", || state(&user) == "idle");
    user.call("agent.send", json!({"agent": agent, "message": "HANG"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the rows", |cx| present(cx, project_id("app")));
    until(&shell, cx, "the agent runs", |cx| {
        cx.update(|window, _| window.try_find(icon_id(AgentState::Running)).is_some())
    });
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("app"), group_header_id(&id), 0., &shell, cx);
    settle(&shell, cx);
    // The row shows it while the group is unfolded.
    assert!(
        !present(cx, group_mark_id(&id)),
        "unfolded, the group says nothing"
    );

    let in_heading = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            let header = window.find(group_header_id(&id)).bounds();
            window
                .find_all(icon_id(AgentState::Running))
                .iter()
                .any(|icon| header.contains(&icon.bounds().center()))
        })
    };
    cx.update(|window, cx| window.click(group_header_id(&id), cx));
    settle(&shell, cx);
    assert!(!is_open(&shell, &id, cx));
    assert!(present(cx, group_mark_id(&id)), "folded, it shows the mark");
    assert!(
        in_heading(cx),
        "the strongest mark of its projects: running"
    );

    cx.update(|window, cx| window.click(group_header_id(&id), cx));
    settle(&shell, cx);
    assert!(
        !present(cx, group_mark_id(&id)),
        "unfolded again, it is gone"
    );

    cx.update(|window, cx| window.click(group_header_id(&id), cx));
    settle(&shell, cx);
    assert!(in_heading(cx));
    user.call("agent.stop", json!({"agent": agent})).unwrap();
    until(&shell, cx, "the mark leaves", |cx| !in_heading(cx));
    settle(&shell, cx);
    assert!(
        !present(cx, group_mark_id(&id)),
        "nothing is held, nothing is said"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_with_a_group_menu_opened_and_closed(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = with_projects(&user, &["alpha", "bravo"], cx);
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);
    cx.update(|window, cx| window.click(group_menu_id(&id), cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click(group_menu_id(&id), cx));
    quiet(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "a group's menu closed: (drawn, asked)"
    );
}

/// The connections the window's keeper opens, through a relay a test can hold: from the next time
/// the keeper reads the arrangement, what it asks does not reach the daemon and no answer comes
/// back; then it drops the connections, so that what was asked is answered "retry" and the next ask opens another one.
#[derive(Clone, Default)]
struct Relay {
    /// The next read of the arrangement is the one to hold (other asks of the keeper, a fold
    /// written, go through).
    armed: Arc<AtomicBool>,
    held: Arc<AtomicBool>,
    open: Arc<Mutex<Vec<(UnixStream, UnixStream)>>>,
}

impl Relay {
    fn connector(&self, user: &User) -> Connect {
        let daemon = user.connector();
        let relay = self.clone();
        Arc::new(move || {
            let toward = daemon()?;
            if std::thread::current().name() != Some("flottille-keeper") {
                return Ok(toward);
            }
            let (keeper, outside) = UnixStream::pair()?;
            relay
                .open
                .lock()
                .unwrap()
                .push((toward.try_clone()?, outside.try_clone()?));
            // The asks, once the relay lets them go.
            let (mut from, mut to) = (outside.try_clone()?, toward.try_clone()?);
            let (armed, held) = (relay.armed.clone(), relay.held.clone());
            std::thread::spawn(move || {
                let mut buffer = [0u8; 8192];
                while let Ok(read) = from.read(&mut buffer) {
                    let asked = &buffer[..read];
                    let reads = asked.windows(10).any(|part| part == b"window.get");
                    if read > 0 && reads && armed.swap(false, Ordering::SeqCst) {
                        held.store(true, Ordering::SeqCst);
                    }
                    while read > 0 && held.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if read == 0 || to.write_all(asked).is_err() {
                        break;
                    }
                }
                let _ = to.shutdown(Shutdown::Both);
            });
            // The answers.
            let (mut from, mut to) = (toward, outside);
            std::thread::spawn(move || {
                let mut buffer = [0u8; 8192];
                while let Ok(read) = from.read(&mut buffer) {
                    if read == 0 || to.write_all(&buffer[..read]).is_err() {
                        break;
                    }
                }
                let _ = to.shutdown(Shutdown::Both);
            });
            Ok(keeper)
        })
    }

    /// The next time the keeper reads the arrangement, that ask and what follows stay in the
    /// relay.
    fn hold(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Whether an ask is stuck in the relay.
    fn holding(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }

    /// The connections are lost: what was held never reaches the daemon.
    fn drop_connections(&self) {
        for (toward, outside) in self.open.lock().unwrap().drain(..) {
            let _ = toward.shutdown(Shutdown::Both);
            let _ = outside.shutdown(Shutdown::Both);
        }
        self.armed.store(false, Ordering::SeqCst);
        self.held.store(false, Ordering::SeqCst);
    }
}

#[gpui_kit::test]
fn a_change_retried_after_a_lost_connection_keeps_the_user_s_order(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "alpha");
    project(&user, "bravo");
    let relay = Relay::default();
    let (shell, cx) = window_through(&user, relay.connector(&user), cx);
    until(&shell, cx, "the rows", |cx| {
        present(cx, project_id("bravo"))
    });
    settle(&shell, cx);

    // The user makes the group "Work" while the connection is stuck, and drags alpha into it
    // before the first request is answered: the connection drops then, so that it is asked again
    // later, and what comes after it must not pass it.
    relay.hold();
    new_group("Work", &shell, cx);
    let id = group_id(&shell, "Work", cx);
    drag(project_id("alpha"), group_header_id(&id), 0., &shell, cx);
    assert_eq!(held(&shell, "Work", cx), names(&["alpha"]));
    assert!(
        relay.holding(),
        "the creation of the group is stuck in the relay"
    );
    assert!(
        kept(&user)["groups"].as_array().is_none_or(Vec::is_empty),
        "nothing reached the daemon while the connection is held: {}",
        kept(&user)
    );
    relay.drop_connections();

    // A second by step, so that the wait before the asking again is over.
    let deadline = Instant::now() + Duration::from_secs(60);
    while strings(&kept(&user)["groups"][0]["projects"]) != ["alpha"] {
        assert!(
            Instant::now() < deadline,
            "the move reaches the daemon after the group it names"
        );
        step(&shell, Duration::from_secs(1), cx);
        std::thread::sleep(Duration::from_millis(25));
    }
    settle(&shell, cx);
    assert_eq!(
        held(&shell, "Work", cx),
        names(&["alpha"]),
        "the window keeps it too"
    );
}
