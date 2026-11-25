//! The scenarios of #1283: every fold of the sidebar starts in the frame of its click, without
//! waiting for the daemon. On GPUI's test platform in front of real daemons, each with its own
//! temporary HOME and XDG folders, fake agents and no keyring, and a hub served in the test on
//! 127.0.0.1. After a click the window only draws frames: it is not polled, so no event of the
//! daemon comes between the click and what it moves. Throwaway repositories. No window opens on a
//! screen.

use crate::common;
use crate::remote::{Machine, TestHub, pc_and_server_on};

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::model::elsewhere_id;
use flottille_window::tree::{
    by_machine_id, ended_id, machine_id, project_id, project_on_id, projects_menu_id, session_id,
};
use gpui_design_system::omarchy::Environment;
use gpui_design_system::toggle_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// How long a link, a session or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);

const APP: &str = "https://git.example/acme/app.git";

fn fake_user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The user of `pc`, whose lead launches a led session.
fn pc_user() -> User {
    let user = fake_user();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]}
    ]}));
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The project `app` on `machine`, over a throwaway repository whose remote is [`APP`].
fn add(machine: &Machine) {
    let user = &machine.user;
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(&folder, &["remote", "add", "origin", APP]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

fn agents(user: &User) -> Vec<serde_json::Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

/// A session the user starts in `app` on `machine`, its first turn ended: its id.
fn session(machine: &Machine, message: &str) -> String {
    let started = machine
        .user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        machine
            .user
            .call("agent.show", json!({"agent": id}))
            .is_ok_and(|shown| shown["agent"]["state"] == "idle")
    });
    id
}

/// The lead the user started in `app` on `pc` and the session it leads, which the user allowed.
fn lead_and_led(pc: &Machine) -> (String, String) {
    let user = &pc.user;
    let before = agents(user).len();
    let lead = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "begin the docs session"}),
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
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the led session exists", || {
        agents(user).len() == before + 2
    });
    let led = agents(user)
        .into_iter()
        .find(|agent| agent["led_by"] == lead.as_str())
        .expect("a session led by the lead")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, led)
}

/// `pc` and `server` both have `app`, `pc` told of `server`'s: on `pc` a lead and the session it
/// leads, and a session that ended; on `server` a session of `app`, which `pc` lists.
struct World {
    _hub: TestHub,
    pc: Machine,
    _server: Machine,
    lead: String,
    led: String,
    there: String,
}

fn world() -> World {
    let (hub, pc, server) = pc_and_server_on(Machine::of(pc_user()), fake_user());
    add(&pc);
    add(&server);
    eventually("pc is told server's project", || {
        let lists = pc.user.call("remote.sessions", json!({})).unwrap();
        lists["machines"][0]["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|p| p["name"] == "app")
    });
    let (lead, led) = lead_and_led(&pc);
    let gone = session(&pc, "an ordinary one");
    pc.user.call("agent.stop", json!({"agent": gone})).unwrap();
    eventually("the session ended", || {
        pc.user.call("agent.show", json!({"agent": gone})).unwrap()["agent"]["state"] == "ended"
    });
    let there = elsewhere_id("server", &session(&server, "Ship the server part"));
    World {
        _hub: hub,
        pc,
        _server: server,
        lead,
        led,
        there,
    }
}

fn window<'a>(
    machine: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: machine.user.path("os-release"),
        state_home: machine.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(machine.user.connector());
    let device = Some(Device::in_folder(
        machine.user.path("state/flottille/remote"),
    ));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

/// One frame, `by` later, the daemon not polled: nothing it says reaches the window.
fn frame(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// Half a second of frames, the daemon not polled.
fn frames(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        frame(cx, Duration::from_millis(16));
    }
}

/// One step of the window with what the daemon said.
fn step(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    frame(cx, Duration::from_millis(16));
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(shell, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..40 {
        step(shell, cx);
    }
}

fn present(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

/// The height of `id`, nothing while it is not drawn: the tallest of them while a row that moves
/// between two levels is in both.
fn height(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> f32 {
    let id = id.into();
    cx.update(|window, _| {
        window
            .find_all(id)
            .iter()
            .map(|found| f32::from(found.bounds().size.height))
            .fold(0., f32::max)
    })
}

/// The frame a session's row unfolds in.
fn frame_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> gpui_kit::ElementId {
    let row = shell.read_with(cx, |s, _| s.session(id).expect("the row").entity_id());
    gpui_kit::ElementId::Name(format!("tree-row-{}", row.as_u64()).into())
}

/// The window on `pc`, the rows of both machines' sessions in its tree.
fn the_tree<'a>(
    world: &World,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(&world.pc, cx);
    until(&shell, cx, "the rows of both machines", |cx| {
        present(cx, session_id(&world.led))
            && present(cx, session_id(&world.there))
            && present(cx, ended_id())
    });
    settle(&shell, cx);
    (shell, cx)
}

/// Group by machine switched on from the Projects header's menu, its sections drawn.
fn by_machine(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the choice is offered", |cx| {
        present(cx, projects_menu_id())
    });
    cx.update(|window, cx| window.click(projects_menu_id(), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(by_machine_id(), cx));
    until(shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc")) && present(cx, machine_id("server"))
    });
    settle(shell, cx);
    assert!(shell.read_with(cx, |s, _| s.is_by_machine()));
}

/// The lead's led rows fold away from the frame of the click, and are gone once folded; then
/// unfold from it: the daemon is not heard in between.
fn the_led_fold_moves_at_once(
    world: &World,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    arrangement: &str,
) {
    let led = frame_of(shell, &world.led, cx);
    let full = height(cx, led.clone());
    assert!(full > 0., "{arrangement}: the led row is drawn");

    cx.update(|window, cx| window.click(toggle_id(session_id(&world.lead)), cx));
    frame(cx, Duration::from_millis(70));
    let folding = height(cx, led.clone());
    assert!(
        folding > 0. && folding < full,
        "{arrangement}: the led row folds away at once: {folding} of {full}"
    );
    frames(cx);
    assert!(
        !present(cx, session_id(&world.led)),
        "{arrangement}: the led row is gone once folded"
    );

    cx.update(|window, cx| window.click(toggle_id(session_id(&world.lead)), cx));
    frame(cx, Duration::from_millis(70));
    let unfolding = height(cx, led.clone());
    assert!(
        unfolding > 0. && unfolding < full,
        "{arrangement}: the led row unfolds at once: {unfolding} of {full}"
    );
    frames(cx);
    assert_eq!(height(cx, led), full, "{arrangement}: unfolded in full");
}

#[gpui_kit::test]
fn folding_a_session_s_led_sessions_moves_at_once(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = the_tree(&world, cx);
    // The project's row holds the sessions of both machines.
    assert!(present(cx, project_id("app")));

    the_led_fold_moves_at_once(&world, &shell, cx, "one list");

    by_machine(&shell, cx);
    the_led_fold_moves_at_once(&world, &shell, cx, "by machine");
}

/// A click on `click` moves the fold `fold` in the first frame after it, the daemon not polled;
/// a second click, once it is still, brings it back.
fn starts_at_once(
    cx: &mut VisualTestContext,
    what: &str,
    click: gpui_kit::ElementId,
    fold: gpui_kit::ElementId,
) {
    let before = height(cx, fold.clone());
    cx.update(|window, cx| window.click(click.clone(), cx));
    frame(cx, Duration::from_millis(16));
    let first = height(cx, fold.clone());
    assert_ne!(
        first, before,
        "{what}: the fold moves in the frame of its click"
    );
    frames(cx);
    cx.update(|window, cx| window.click(click, cx));
    frames(cx);
    assert_eq!(height(cx, fold), before, "{what}: back as it was");
}

#[gpui_kit::test]
fn every_fold_of_the_sidebar_starts_in_the_frame_of_its_click(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = the_tree(&world, cx);

    // One list: a project, a session's led sessions, Ended.
    starts_at_once(
        cx,
        "project",
        toggle_id(project_id("app")),
        "fold-project-app".into(),
    );
    let led = frame_of(&shell, &world.led, cx);
    starts_at_once(cx, "session", toggle_id(session_id(&world.lead)), led);
    starts_at_once(cx, "Ended", ended_id(), "fold-ended".into());

    // By machine: a machine, a project under it, a session's led sessions, Ended.
    by_machine(&shell, cx);
    starts_at_once(
        cx,
        "machine",
        toggle_id(machine_id("pc")),
        "fold-machine-pc".into(),
    );
    starts_at_once(
        cx,
        "project on a machine",
        toggle_id(project_on_id("app", "pc")),
        "fold-project-app-on-pc".into(),
    );
    let led = frame_of(&shell, &world.led, cx);
    starts_at_once(
        cx,
        "session on a machine",
        toggle_id(session_id(&world.lead)),
        led,
    );
    starts_at_once(cx, "Ended by machine", ended_id(), "fold-ended".into());
}
