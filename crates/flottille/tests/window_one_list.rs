//! The scenarios of #1150, points 1 to 3: one list of the sessions of every machine
//! (`docs/remote.md`, "One list of every machine"). On GPUI's test platform in front of real
//! daemons, each with its own temporary HOME and XDG folders, fake agents and no keyring, and a
//! hub served in the test on 127.0.0.1: the window signs with the device key file of its
//! machine's key folder. A machine "off" is its daemon stopped, then started again on the same
//! folders. Throwaway repositories. No window opens on a screen.

use crate::common;
use crate::remote::{Machine, TestHub, pc_and_server_on};

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::model::elsewhere_id;
use flottille_window::tree::{session_end_id, session_id, session_machine_id, session_menu_id};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// How long a link, a session or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);

/// A machine whose agents are the fake agent.
fn fake_user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The projects `projects` on `machine`, each over a throwaway repository with the remote its
/// name says.
fn add(machine: &Machine, projects: &[&str]) {
    let user = &machine.user;
    for name in projects {
        let folder = user.path(&format!("work/{name}"));
        user.git_init(&folder);
        std::fs::write(folder.join("README.md"), "app\n").unwrap();
        user.git(&folder, &["add", "README.md"]);
        user.git(&folder, &["commit", "--quiet", "-m", "start"]);
        // The same remote on both machines: one project (`docs/remote.md`, "One list of every
        // machine").
        user.git(
            &folder,
            &[
                "remote",
                "add",
                "origin",
                &format!("https://git.example/acme/{name}.git"),
            ],
        );
        user.call("project.add", json!({"folder": folder, "name": name}))
            .unwrap();
    }
}

/// `pc` with the project `app`, `server` a runner with `app` and `site`, the two linked.
fn pc_and_server() -> (TestHub, Machine, Machine) {
    let (hub, pc, server) = pc_and_server_on(Machine::of(fake_user()), fake_user());
    add(&pc, &["app"]);
    add(&server, &["app", "site"]);
    (hub, pc, server)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn state(machine: &Machine, agent: &str) -> String {
    machine
        .user
        .call("agent.show", json!({"agent": agent}))
        .map(|shown| {
            shown["agent"]["state"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

/// A session the user starts in `project` on `machine`, its first turn ended: its id.
fn session(machine: &Machine, project: &str, message: &str) -> String {
    let started = machine
        .user
        .call("agent.run", json!({"project": project, "message": message}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || state(machine, &id) == "idle");
    id
}

/// Sends `message` to `agent` from `machine`'s own client: another device than the window's.
fn send(machine: &Machine, agent: &str, message: &str) {
    let turns = machine.user.received_requests("session/prompt").len();
    machine
        .user
        .call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
    eventually("the turn ends", || {
        machine.user.received_requests("session/prompt").len() > turns
            && state(machine, agent) == "idle"
    });
}

fn off(machine: &Machine) {
    let stopped = machine.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", machine.log());
}

fn on(machine: &Machine) {
    machine.user.status();
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

/// One step of the window as the platform would run it: how many animation frames were asked.
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
    let deadline = Instant::now() + WAIT;
    loop {
        step(shell, Duration::from_millis(16), cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..40 {
        step(shell, Duration::from_millis(16), cx);
    }
}

fn present(cx: &mut VisualTestContext, id: gpui_kit::ElementId) -> bool {
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(cx: &mut VisualTestContext, id: gpui_kit::ElementId) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The ids of the rows of the project `name`'s group, in order.
fn listed(shell: &Entity<Shell>, name: &str, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.project(name)
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

/// The window on `pc`, once the row `row` is in its tree.
fn with_row<'a>(
    pc: &Machine,
    row: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(pc, cx);
    until(&shell, cx, "the row of the session on server", |cx| {
        present(cx, session_id(row))
    });
    settle(&shell, cx);
    (shell, cx)
}

#[gpui_kit::test]
fn the_sidebar_lists_the_sessions_of_every_machine_by_project(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let here = session(&pc, "app", "Fix the pc part");
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = with_row(&pc, &row, cx);

    // One group, by last exchange: the server's session, started last, first.
    until(&shell, cx, "both sessions in app", |cx| {
        listed(&shell, "app", cx) == [row.clone(), here.clone()]
    });
    assert_eq!(
        label(cx, session_id(&row)).as_deref(),
        Some("Ship the server part")
    );
    // No switcher: every machine is in the one list.
    assert!(!present(cx, "machine-switcher".into()));
}

#[gpui_kit::test]
fn a_project_on_two_machines_is_one_group(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let here = session(&pc, "app", "Fix the pc part");
    let there = session(&server, "app", "Ship the server part");
    let site = session(&server, "site", "Draw the site");
    let site_row = elsewhere_id("server", &site);

    let (shell, cx) = with_row(&pc, &site_row, cx);

    // A project only the server has is a group of its own.
    assert!(shell.read_with(cx, |s, _| s.project("site").is_some()));
    until(&shell, cx, "app holds both machines' sessions", |cx| {
        let app = listed(&shell, "app", cx);
        app.len() == 2 && app.contains(&here) && app.contains(&elsewhere_id("server", &there))
    });
    assert_eq!(listed(&shell, "site", cx), [site_row]);
}

#[gpui_kit::test]
fn a_session_of_another_machine_carries_its_name(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let here = session(&pc, "app", "Fix the pc part");
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (_shell, cx) = with_row(&pc, &row, cx);

    assert_eq!(
        label(cx, session_machine_id(&row)).as_deref(),
        Some("server")
    );
    assert!(present(cx, session_id(&here)));
    assert!(
        !present(cx, session_machine_id(&here)),
        "a session of this machine carries no tag"
    );
}

#[gpui_kit::test]
fn a_message_sent_from_another_device_appears_live(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let there = session(&server, "app", "Ship the server part");
    let here = session(&pc, "app", "Fix the pc part");
    let row = elsewhere_id("server", &there);
    let (shell, cx) = with_row(&pc, &row, cx);
    until(&shell, cx, "pc's session first", |cx| {
        listed(&shell, "app", cx) == [here.clone(), row.clone()]
    });

    // Written to from the server's own client: its row comes first, live.
    send(&server, &there, "Cover the docs too");
    until(&shell, cx, "the server's session first", |cx| {
        listed(&shell, "app", cx).first() == Some(&row)
    });

    // Opened, it is read through its machine, and what another device writes shows live.
    cx.update(|window, cx| window.click(session_id(&row), cx));
    until(&shell, cx, "the server's session open", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .session_page()
                .is_some_and(|page| page.read(cx).id() == there)
        })
    });
    send(&server, &there, "And the changelog");
    until(&shell, cx, "the message shows", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).session_page().is_some_and(|page| {
                page.read(cx)
                    .conversation()
                    .read(cx)
                    .blocks()
                    .iter()
                    .any(|block| {
                        matches!(block, Block::Message { text, .. } if text == "And the changelog")
                    })
            })
        })
    });
}

#[gpui_kit::test]
fn a_session_of_a_machine_that_is_off_is_greyed_and_cannot_be_opened(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);
    eventually("pc keeps server's list", || {
        pc.user
            .call("remote.sessions", json!({}))
            .is_ok_and(|lists| lists.to_string().contains(&there))
    });
    off(&server);
    pc.wait_for("server", "offline");

    // The window opened while it is off still lists it, from this machine's copy.
    let (shell, cx) = with_row(&pc, &row, cx);
    let said = label(cx, session_machine_id(&row)).unwrap_or_default();
    assert!(said.starts_with("server offline since "), "{said}");
    let greyed = shell.read_with(cx, |s, cx| {
        s.session(&row).is_some_and(|row| row.read(cx).offline())
    });
    assert!(greyed, "its row is greyed");

    cx.update(|window, cx| window.click(session_id(&row), cx));
    settle(&shell, cx);
    assert_eq!(shell.read_with(cx, |s, _| s.machine_shown()), None);
    assert!(
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_none()),
        "nothing opened"
    );

    // Back, it can be opened again, its ink back.
    on(&server);
    pc.wait_for("server", "online");
    until(&shell, cx, "the row is no longer greyed", |cx| {
        label(cx, session_machine_id(&row)).as_deref() == Some("server")
    });
}

#[gpui_kit::test]
fn closing_a_session_of_a_machine_that_is_off_is_sent_when_it_comes_back(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);
    let (shell, cx) = with_row(&pc, &row, cx);
    off(&server);
    pc.wait_for("server", "offline");
    until(&shell, cx, "the row says server is off", |cx| {
        label(cx, session_machine_id(&row)).is_some_and(|said| said.contains("offline"))
    });

    cx.update(|window, cx| window.hover(session_id(&row), cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_menu_id(&row), cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_end_id(&row), cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("sidebar-end-confirm", cx));

    // It leaves the list at once, while its machine is still off.
    until(&shell, cx, "the row left", |cx| {
        !present(cx, session_id(&row))
    });
    on(&server);
    pc.wait_for("server", "online");
    eventually("the session ended on server", || {
        state(&server, &there) == "ended"
    });
    settle(&shell, cx);
    assert!(!present(cx, session_id(&row)), "it does not come back");
}

/// What the window drew and asked for over ten seconds of its clock: the frames drawn, the
/// animation frames asked.
fn frames_at_rest(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (u64, usize) {
    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        asked += step(shell, Duration::from_millis(16), cx);
        gone += Duration::from_millis(16);
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    (drawn, asked)
}

/// Runs the window until it asked no animation frame for a second of its clock and two of real
/// time (`window_at_rest.rs`).
fn quiet(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    const STEP: Duration = Duration::from_millis(16);
    let began = Instant::now();
    let mut last_asked = began;
    let mut still = Duration::ZERO;
    while still < Duration::from_secs(1) || last_asked.elapsed() < Duration::from_secs(2) {
        assert!(
            began.elapsed() < WAIT,
            "the window never stopped asking for animation frames"
        );
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
fn the_window_draws_no_frame_at_rest_on_the_merged_list(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let here = session(&pc, "app", "Fix the pc part");
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);
    let (shell, cx) = with_row(&pc, &row, cx);
    until(&shell, cx, "both rows", |cx| present(cx, session_id(&here)));
    quiet(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}
