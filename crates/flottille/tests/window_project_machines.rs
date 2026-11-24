//! The scenarios of #1181 (first part): the projects' list says where each project lives
//! (`docs/design.md` §10, `docs/remote.md`, "One list of every machine"). On GPUI's test platform
//! in front of real daemons, each with its own temporary HOME and XDG folders, fake agents and no
//! keyring, and a hub served in the test on 127.0.0.1: the window signs with the device key file
//! of its machine's key folder. A machine "off" is its daemon stopped. Throwaway repositories. No
//! window opens on a screen.

use crate::common;
use crate::remote::{Machine, TestHub, pc_and_server_on};

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::machines::News;
use flottille_window::model::elsewhere_id;
use flottille_window::project::{machine_block_id, machine_session_mark_id, session_row_id};
use flottille_window::shell::Selection;
use flottille_window::tree::{
    by_machine_id, group_header_id, machine_id, machine_mark_id, project_apart_id, project_id,
    project_mark_id, project_offline_id, projects_menu_id, session_id,
};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// How long a link, a session or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);

fn fake_user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The project `name` on `machine`, over a throwaway repository whose remote is `remote`: none
/// when it is empty.
fn add(machine: &Machine, name: &str, remote: &str) {
    let user = &machine.user;
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    if !remote.is_empty() {
        user.git(&folder, &["remote", "add", "origin", remote]);
    }
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn machines() -> (TestHub, Machine, Machine) {
    pc_and_server_on(Machine::of(fake_user()), fake_user())
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `pc` has been told the projects `names` of `server`, which the sidebar groups by.
fn told(pc: &Machine, names: &[&str]) {
    eventually("pc is told server's projects", || {
        let lists = pc.user.call("remote.sessions", json!({})).unwrap();
        let held: Vec<String> = lists["machines"][0]["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| p["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        names.iter().all(|name| held.iter().any(|h| h == name))
    });
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
    open(machine, cx)
}

/// Another window on `machine`'s daemon.
fn open<'a>(
    machine: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
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

/// A session the user starts in `project` on `machine`, its first turn ended: its id.
fn session(machine: &Machine, project: &str, message: &str) -> String {
    let started = machine
        .user
        .call("agent.run", json!({"project": project, "message": message}))
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

/// The window on `pc`, once the row of the project `row` is in its tree.
fn with_row<'a>(
    pc: &Machine,
    row: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(pc, cx);
    let row = row.to_owned();
    until(&shell, cx, "the project's row", |cx| {
        present(cx, project_id(&row))
    });
    settle(&shell, cx);
    (shell, cx)
}

/// The machines that have the row `name`, in the order the row marks them.
fn homes(shell: &Entity<Shell>, name: &str, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |shell, cx| {
        shell
            .project(name)
            .map(|node| {
                node.read(cx)
                    .homes()
                    .iter()
                    .map(|home| home.machine.clone())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn bounds(
    cx: &mut VisualTestContext,
    id: impl Into<gpui_kit::ElementId>,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let id = id.into();
    cx.update(|window, _| window.find(id).bounds())
}

const APP: &str = "https://git.example/acme/app.git";
const SITE: &str = "git@git.example:acme/site.git";

#[gpui_kit::test]
fn each_project_row_shows_its_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "site", SITE);
    told(&pc, &["site"]);

    let (shell, cx) = with_row(&pc, "site", cx);

    // This machine's project carries its mark too, so that a mixed list reads at a glance.
    assert_eq!(homes(&shell, "app", cx), ["pc"]);
    assert_eq!(homes(&shell, "site", cx), ["server"]);
    assert_eq!(
        label(cx, project_mark_id("app", "pc")).as_deref(),
        Some("pc · this machine")
    );
    assert_eq!(
        label(cx, project_mark_id("site", "server")).as_deref(),
        Some("server · online")
    );
    assert!(!present(cx, project_mark_id("site", "pc")));
}

#[gpui_kit::test]
fn one_project_on_two_machines_is_one_row_with_both_icons(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    // Not the same name, nor the same written address: the same repository.
    add(&pc, "app", APP);
    add(&server, "ledgerly", "git@GIT.example:acme/app");
    told(&pc, &["ledgerly"]);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "both machines on the row", |cx| {
        homes(&shell, "app", cx) == ["pc", "server"]
    });

    assert!(present(cx, project_mark_id("app", "pc")));
    assert!(present(cx, project_mark_id("app", "server")));
    // The marks come after the name, at the end of the row, as the approved prototype draws them.
    let row = bounds(cx, project_id("app"));
    let (pc, server) = (
        bounds(cx, project_mark_id("app", "pc")),
        bounds(cx, project_mark_id("app", "server")),
    );
    assert!(
        pc.origin.x > row.origin.x + row.size.width / 2.0,
        "the marks are in the right half of the row: {pc:?} in {row:?}"
    );
    assert!(pc.origin.x < server.origin.x, "side by side, in order");
    // One row, not two: a row of the same project folds away over a moment, as any row that
    // leaves a list does, so the second one is waited for to be gone, not asked at once.
    until(&shell, cx, "one row, not two", |cx| {
        !present(cx, project_id("ledgerly"))
    });
    assert!(!present(cx, project_id("app@server")));
}

#[gpui_kit::test]
fn a_shared_project_row_holds_the_sessions_of_both_machines(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    told(&pc, &["app"]);
    let here = session(&pc, "app", "Fix the pc part");
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "both sessions in the one row", |cx| {
        present(cx, session_id(&row)) && present(cx, session_id(&here))
    });

    let rows: Vec<String> = shell.read_with(cx, |s, cx| {
        s.project("app")
            .unwrap()
            .read(cx)
            .sessions(cx)
            .iter()
            .map(|row| row.read(cx).id().to_owned())
            .collect()
    });
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.contains(&here) && rows.contains(&row));
}

#[gpui_kit::test]
fn two_projects_of_one_name_but_other_repositories_are_told_apart(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "app", "https://git.example/other/app.git");
    told(&pc, &["app"]);

    let (shell, cx) = with_row(&pc, "app@server", cx);

    // Two rows, each with its own mark; the namesake says its machine in small after its name.
    assert_eq!(homes(&shell, "app", cx), ["pc"]);
    assert_eq!(homes(&shell, "app@server", cx), ["server"]);
    assert_eq!(
        label(cx, project_apart_id("app@server")).as_deref(),
        Some("server")
    );
    assert!(!present(cx, project_apart_id("app")));
    assert!(present(cx, project_mark_id("app@server", "server")));
}

#[gpui_kit::test]
fn a_project_of_an_offline_machine_is_greyed_with_since_when(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "site", SITE);
    told(&pc, &["site"]);
    let stop = server.user.run(&["daemon", "stop"]);
    assert!(stop.status.success(), "{}", server.log());
    pc.wait_for("server", "offline");

    // A window opened while it is off still lists its project, from this machine's copy.
    let (shell, cx) = with_row(&pc, "site", cx);
    until(&shell, cx, "its row says since when", |cx| {
        label(cx, project_offline_id("site")).is_some_and(|said| said.starts_with("offline since "))
    });
    assert!(shell.read_with(cx, |s, cx| s.project("site").unwrap().read(cx).offline()));
    // This machine's own project is not greyed.
    assert!(!shell.read_with(cx, |s, cx| s.project("app").unwrap().read(cx).offline()));
    assert!(!present(cx, project_offline_id("app")));
    let said = label(cx, project_mark_id("site", "server")).unwrap_or_default();
    assert!(said.starts_with("server · offline since "), "{said}");

    // Back online, the row takes its ink back.
    server.user.status();
    pc.wait_for("server", "online");
    until(&shell, cx, "the row is no longer greyed", |cx| {
        !shell.read_with(cx, |s, cx| s.project("site").unwrap().read(cx).offline())
    });
}

/// Group by machine switched on or off from the Projects header's menu.
fn switch_grouping(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(projects_menu_id(), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(by_machine_id(), cx));
    settle(shell, cx);
}

#[gpui_kit::test]
fn group_by_machine_shows_a_section_per_machine_and_back(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    add(&server, "site", SITE);
    told(&pc, &["app", "site"]);
    let here = session(&pc, "app", "Fix the pc part");
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "the shared row", |cx| {
        homes(&shell, "app", cx) == ["pc", "server"] && present(cx, session_id(&row))
    });
    // Offered once there is another machine; off until the user turns it on.
    assert!(shell.read_with(cx, |s, cx| s.offers_grouping(cx)));
    assert!(!shell.read_with(cx, |s, _| s.is_by_machine()));

    switch_grouping(&shell, cx);
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc")) && present(cx, machine_id("server"))
    });
    assert!(shell.read_with(cx, |s, _| s.is_by_machine()));
    assert!(present(cx, machine_mark_id("server")));
    // A shared project is under each machine that has it, with that machine's sessions only.
    let under = |machine: &str, cx: &mut VisualTestContext| -> Vec<String> {
        shell.read_with(cx, |s, cx| {
            s.project_on("app", machine)
                .map(|node| {
                    node.read(cx)
                        .sessions(cx)
                        .iter()
                        .map(|row| row.read(cx).id().to_owned())
                        .collect()
                })
                .unwrap_or_default()
        })
    };
    assert_eq!(under("pc", cx), std::slice::from_ref(&here));
    assert_eq!(under("server", cx), std::slice::from_ref(&row));
    assert!(shell.read_with(cx, |s, _| s.project_on("site", "server").is_some()));
    assert!(shell.read_with(cx, |s, _| s.project_on("site", "pc").is_none()));
    assert!(!present(cx, project_id("app")), "the one list is gone");

    // Off again, the rows come back to the one list as they were.
    switch_grouping(&shell, cx);
    // Both sections fold away before a row is looked for: while one is still folding, its row and
    // the one list's carry the same id.
    until(&shell, cx, "the one list", |cx| {
        !present(cx, machine_id("pc"))
            && !present(cx, machine_id("server"))
            && present(cx, project_id("app"))
    });
    assert!(!shell.read_with(cx, |s, _| s.is_by_machine()));
    assert_eq!(homes(&shell, "app", cx), ["pc", "server"]);
    assert!(present(cx, session_id(&here)) && present(cx, session_id(&row)));
}

#[gpui_kit::test]
fn the_arrangement_survives_a_restart_of_the_window(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "site", SITE);
    told(&pc, &["site"]);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "the choice is offered", |cx| {
        shell.read_with(cx, |s, cx| s.offers_grouping(cx))
    });
    switch_grouping(&shell, cx);
    assert!(shell.read_with(cx, |s, _| s.is_by_machine()));
    // The daemon has it: the state a window reads.
    eventually("the daemon keeps it", || {
        pc.user
            .call("window.get", json!({}))
            .is_ok_and(|kept| kept["state"]["sidebar"]["by_machine"] == true)
    });

    // Another window, on the same daemon, shows the same arrangement.
    let (again, cx) = open(&pc, cx);
    until(&again, cx, "the sections of the new window", |cx| {
        present(cx, machine_id("pc")) && present(cx, machine_id("server"))
    });
    assert!(again.read_with(cx, |s, _| s.is_by_machine()));
}

/// The projects of the user's list as drawn, top to bottom.
fn drawn_order(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    let mut rows: Vec<(f32, String)> = shell
        .read_with(cx, |s, _| s.listing().keys())
        .into_iter()
        .map(|name| {
            let at = cx.update(|window, _| {
                window
                    .find(flottille_window::tree::project_slot_id(&name))
                    .bounds()
                    .origin
                    .y
            });
            (f32::from(at), name)
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, name)| name).collect()
}

#[gpui_kit::test]
fn group_by_machine_off_brings_the_users_groups_back_as_they_were(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    for name in ["app", "docs", "extra", "notes", "zeta"] {
        add(&pc, name, "");
    }
    add(&server, "site", SITE);
    told(&pc, &["site"]);
    let (shell, cx) = with_row(&pc, "zeta", cx);
    until(&shell, cx, "the choice is offered", |cx| {
        shell.read_with(cx, |s, cx| s.offers_grouping(cx))
    });
    // The user's own arrangement, unlike the daemon's order: two groups, and the projects
    // outside them in another order than the daemon lists them.
    shell.update_in(cx, |shell, window, cx| {
        shell.add_group("Work", cx);
        shell.add_group("Play", cx);
        let (work, play) = {
            let groups = shell.listing().groups;
            (groups[0].id.clone(), groups[1].id.clone())
        };
        shell.project_moved("notes", Some(&work), 0, window, cx);
        shell.project_moved("docs", Some(&work), 1, window, cx);
        shell.project_moved("app", Some(&play), 0, window, cx);
        shell.project_moved("zeta", None, 0, window, cx);
    });
    settle(&shell, cx);
    let before = shell.read_with(cx, |s, _| s.listing());
    assert_eq!(before.groups[0].projects, ["notes", "docs"]);
    assert_eq!(before.groups[1].projects, ["app"]);
    // The other machine's project is in the list too, at the end.
    assert_eq!(before.loose, ["zeta", "extra", "site"]);
    let drawn = drawn_order(&shell, cx);
    assert_eq!(drawn, ["notes", "docs", "app", "zeta", "extra", "site"]);
    let id = before.groups[0].id.clone();
    assert!(present(cx, group_header_id(&id)));

    // By machine the user's groups give way to the sections, and the rows are the sections'.
    switch_grouping(&shell, cx);
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc")) && !present(cx, group_header_id(&id))
    });
    assert!(!present(cx, project_id("docs")), "the one list is gone");

    // Switched off, the groups and the order are back exactly as they were.
    switch_grouping(&shell, cx);
    until(&shell, cx, "the user's list", |cx| {
        !present(cx, machine_id("pc")) && present(cx, group_header_id(&id))
    });
    settle(&shell, cx);
    assert_eq!(shell.read_with(cx, |s, _| s.listing()), before);
    assert_eq!(drawn_order(&shell, cx), drawn);
}

#[gpui_kit::test]
fn new_group_is_not_offered_while_the_projects_are_grouped_by_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "site", SITE);
    told(&pc, &["site"]);
    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "the choice is offered", |cx| {
        shell.read_with(cx, |s, cx| s.offers_grouping(cx))
    });
    let reason = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            window
                .try_find("projects-new-group-detail")
                .and_then(|found| found.label().map(str::to_owned))
        })
    };

    switch_grouping(&shell, cx);
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc"))
    });
    cx.update(|window, cx| window.click(projects_menu_id(), cx));
    settle(&shell, cx);
    assert_eq!(
        reason(cx).as_deref(),
        Some("Shown by machine"),
        "the entry says why"
    );
    cx.update(|window, cx| window.click(flottille_window::tree::new_group_id(), cx));
    settle(&shell, cx);
    assert!(
        !shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()),
        "it opens nothing"
    );

    // Back to the one list: it is offered again.
    cx.update(|window, cx| window.click(by_machine_id(), cx));
    until(&shell, cx, "the user's list", |cx| {
        !present(cx, machine_id("pc"))
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(projects_menu_id(), cx));
    settle(&shell, cx);
    assert_eq!(reason(cx), None);
    cx.update(|window, cx| window.click(flottille_window::tree::new_group_id(), cx));
    settle(&shell, cx);
    assert!(shell.read_with(cx, |s, cx| s.group_form().read(cx).is_open()));
}

/// The frames the window draws, and the animation frames it asks, over ten seconds of its clock.
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
fn the_window_draws_no_frame_at_rest_with_marks_and_with_sections(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    add(&server, "site", SITE);
    told(&pc, &["app", "site"]);
    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "the shared row", |cx| {
        homes(&shell, "app", cx) == ["pc", "server"]
    });
    quiet(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "marks: (drawn, asked)");

    switch_grouping(&shell, cx);
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc"))
    });
    quiet(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "sections: (drawn, asked)"
    );
}

// The shared project's page (#1266): its sessions of every machine, where a session starts, and
// its settings' switch between the machines that have it.

/// A user one of whose providers is ready, as the window sees it: a machine of theirs can run an
/// agent, and its composer starts.
fn ready_user() -> User {
    let user = User::with_fake_providers();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A hub, `pc` and `server`, each able to run an agent.
fn ready_machines() -> (TestHub, Machine, Machine) {
    pc_and_server_on(Machine::of(ready_user()), ready_user())
}

/// `pc` has been told the projects `names` of `machine`.
fn told_by(pc: &Machine, machine: &str, names: &[&str]) {
    eventually(&format!("pc is told {machine}'s projects"), || {
        let lists = pc.user.call("remote.sessions", json!({})).unwrap();
        let held: Vec<String> = lists["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|listed| listed["name"] == machine)
            .flat_map(|listed| listed["projects"].as_array().cloned().unwrap_or_default())
            .map(|p| p["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        names.iter().all(|name| held.iter().any(|h| h == name))
    });
}

/// The window on `pc`, tall enough for the whole page, its machines read.
fn page_window<'a>(
    pc: &Machine,
    others: &[&str],
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(pc, cx);
    cx.simulate_resize(size(px(1280.), px(2400.)));
    let others: Vec<String> = others.iter().map(|name| (*name).to_owned()).collect();
    until(
        &shell,
        cx,
        "the machines and this one's providers are read",
        |cx| {
            shell.read_with(cx, |s, cx| {
                s.providers().offered().next().is_some()
                    && s.machines().read(cx).machines().is_some_and(|read| {
                        others
                            .iter()
                            .all(|name| read.machine(name).is_some_and(|m| m.online()))
                    })
            })
        },
    );
    settle(&shell, cx);
    (shell, cx)
}

fn show(shell: &Entity<Shell>, selection: Selection, cx: &mut VisualTestContext) {
    shell.update_in(cx, |s, window, cx| s.open(selection, window, cx));
    settle(shell, cx);
}

fn click(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(shell, cx);
}

fn shown(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    shell.read_with(cx, |s, _| s.machine_shown())
}

#[gpui_kit::test]
fn its_page_lists_the_sessions_of_both_machines(cx: &mut TestAppContext) {
    let (_hub, pc, server) = ready_machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    told(&pc, &["app"]);
    let here = session(&pc, "app", "Fix the pc part");
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = page_window(&pc, &["server"], cx);
    show(&shell, Selection::Project("app".into()), cx);
    until(&shell, cx, "both sessions on the page", |cx| {
        present(cx, session_row_id(&here)) && present(cx, session_row_id(&row))
    });
    assert_eq!(shown(&shell, cx), None, "this machine's page of it");
    // The server's session is in the server's block, with the server's icon; this machine's in
    // its place, as before.
    assert!(present(cx, machine_block_id("server")));
    assert!(!present(cx, machine_block_id("pc")));
    let mark = label(cx, machine_session_mark_id(&row)).unwrap_or_default();
    assert!(
        mark.contains(" in "),
        "the server's icon in its colour: {mark}"
    );
    assert_eq!(
        label(cx, machine_block_id("server")).as_deref(),
        Some("server · online")
    );

    // The server's page of it lists this machine's session the same way.
    click(&shell, "machine-picker", cx);
    click(&shell, "machine-choice-server", cx);
    until(
        &shell,
        cx,
        "the server's page with this machine's session",
        |cx| {
            shown(&shell, cx).as_deref() == Some("server")
                && present(cx, machine_block_id("pc"))
                && present(cx, session_row_id(&here))
                && present(cx, session_row_id(&row))
        },
    );
    let mark = label(cx, machine_session_mark_id(&here)).unwrap_or_default();
    assert!(
        mark.contains(" in "),
        "this machine's icon in its colour: {mark}"
    );
    assert!(!present(cx, machine_block_id("server")));
}

#[gpui_kit::test]
fn starting_a_session_runs_on_this_machine_when_it_can_else_asks(cx: &mut TestAppContext) {
    let (hub, pc, server) = ready_machines();
    let laptop = Machine::of(ready_user());
    crate::remote::pair(&hub, &laptop, "laptop", "runner");
    pc.wait_for("laptop", "online");
    // `app` everywhere; `site` on the server only; `web` on the server and the laptop.
    add(&pc, "app", APP);
    add(&server, "app", APP);
    add(&laptop, "app", APP);
    add(&server, "site", SITE);
    add(&server, "web", "https://git.example/acme/web.git");
    add(&laptop, "web", "https://git.example/acme/web.git");
    told_by(&pc, "server", &["app", "site", "web"]);
    told_by(&pc, "laptop", &["app", "web"]);

    let (shell, cx) = page_window(&pc, &["server", "laptop"], cx);
    // Where a session can start is what each machine offers over its link.
    until(
        &shell,
        cx,
        "the window knows what each machine offers",
        |cx| {
            shell.read_with(cx, |s, cx| {
                s.machines().read(cx).machines().is_some_and(|read| {
                    read.has_project(Some("server"), "site")
                        && read.has_project(Some("server"), "web")
                        && read.has_project(Some("laptop"), "web")
                })
            })
        },
    );
    // This machine has it and can run it: the session runs here, nothing asked.
    show(&shell, Selection::Project("app".into()), cx);
    until(&shell, cx, "app's composer", |cx| {
        present(cx, "place-picker")
    });
    let providers = shell.read_with(cx, |s, _| {
        let p = s.providers();
        format!(
            "known {} offered {} why {:?}",
            p.known,
            p.offered().count(),
            p.why_none()
        )
    });
    assert_eq!(shown(&shell, cx), None, "{providers}");
    assert!(!present(cx, "composer-ask"));

    // Not here, on one machine online: there, nothing asked.
    show(&shell, Selection::Project("site".into()), cx);
    until(&shell, cx, "site's page on the server", |cx| {
        shown(&shell, cx).as_deref() == Some("server") && present(cx, "place-picker")
    });
    assert!(!present(cx, "composer-ask"));

    // Not here, on two: the start asks which, and starts nothing before.
    show(&shell, Selection::Project("web".into()), cx);
    until(&shell, cx, "web's composer asks", |cx| {
        present(cx, "composer-ask") && present(cx, "machine-picker")
    });
    let asked = label(cx, "composer-ask").unwrap_or_default();
    assert!(
        asked.contains("server") && asked.contains("laptop"),
        "{asked}"
    );
    let can_start = |shell: &Entity<Shell>, cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| {
            s.composer_of("web")
                .is_some_and(|composer| composer.read(cx).asks_where().is_none())
        })
    };
    assert!(!can_start(&shell, cx));
    click(&shell, "machine-picker", cx);
    click(&shell, "machine-choice-laptop", cx);
    until(&shell, cx, "web on the laptop, asked no more", |cx| {
        shown(&shell, cx).as_deref() == Some("laptop") && !present(cx, "composer-ask")
    });

    // The choice is kept for the project: web opens on the laptop again, asking nothing, and the
    // daemon keeps it with the window's own state.
    show(&shell, Selection::Project("app".into()), cx);
    until(&shell, cx, "app on this machine", |cx| {
        shown(&shell, cx).is_none()
    });
    show(&shell, Selection::Project("web".into()), cx);
    until(&shell, cx, "web on the laptop again", |cx| {
        shown(&shell, cx).as_deref() == Some("laptop") && present(cx, "place-picker")
    });
    assert!(!present(cx, "composer-ask"));
    eventually("the daemon keeps the choice", || {
        pc.user
            .call("window.get", json!({}))
            .is_ok_and(|state| state["state"]["sidebar"]["starts"]["web"] == "laptop")
    });
}

#[gpui_kit::test]
fn its_settings_switch_between_the_machines_that_have_it(cx: &mut TestAppContext) {
    let (_hub, pc, server) = ready_machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    add(&pc, "site", SITE);
    told(&pc, &["app"]);

    let (shell, cx) = page_window(&pc, &["server"], cx);
    // A project on this machine only has no switch.
    show(&shell, Selection::ProjectSettings("site".into()), cx);
    until(&shell, cx, "site's settings", |cx| {
        present(cx, "project-settings-header")
    });
    assert!(!present(cx, "project-settings-machine"));

    show(&shell, Selection::ProjectSettings("app".into()), cx);
    until(&shell, cx, "app's switch of machines", |cx| {
        present(cx, "project-settings-machine")
    });
    assert_eq!(
        label(cx, "project-settings-machine-field").as_deref(),
        Some("Settings of")
    );
    click(&shell, "project-settings-machine", cx);
    // This machine first, then the other one that has it.
    let top = |cx: &mut VisualTestContext, id: &str| bounds(cx, id.to_owned()).origin.y;
    until(&shell, cx, "both machines offered", |cx| {
        present(cx, "project-settings-machine-here")
            && present(cx, "project-settings-machine-server")
    });
    assert!(top(cx, "project-settings-machine-here") < top(cx, "project-settings-machine-server"));
    click(&shell, "project-settings-machine-server", cx);
    until(&shell, cx, "the server's settings of app", |cx| {
        shown(&shell, cx).as_deref() == Some("server")
            && shell.read_with(cx, |s, _| {
                s.selected() == Some(&Selection::ProjectSettings("app".into()))
            })
            && present(cx, "project-settings-machine")
    });
    // And back to this machine's.
    click(&shell, "project-settings-machine", cx);
    click(&shell, "project-settings-machine-here", cx);
    until(&shell, cx, "this machine's settings of app", |cx| {
        shown(&shell, cx).is_none()
            && shell.read_with(cx, |s, _| {
                s.selected() == Some(&Selection::ProjectSettings("app".into()))
            })
    });
}

// The shared project's page at rest (#1411): nothing is drawn once it settled, whatever the other
// machines hold, and a read of them that changes nothing on it draws nothing.

/// A shared project `app` on `pc`, `server` and a paired `laptop`: the server has a session that
/// has a turn that runs while the page opens (held until `release` is written in its agent's
/// folder, which the scenario does once the page shows it) and one that ended, the laptop one that ended and then went off. The window on `pc` shows the page of
/// `app`, settled: it asks for no animation frame.
fn shared_page_settled(
    cx: &mut TestAppContext,
) -> (
    TestHub,
    Machine,
    Machine,
    Entity<Shell>,
    &mut VisualTestContext,
) {
    let (hub, pc, server) = ready_machines();
    let laptop = Machine::of(ready_user());
    crate::remote::pair(&hub, &laptop, "laptop", "runner");
    pc.wait_for("laptop", "online");
    for machine in [&pc, &server, &laptop] {
        add(machine, "app", APP);
    }
    told_by(&pc, "server", &["app"]);
    told_by(&pc, "laptop", &["app"]);
    session(&server, "app", "Ship the server part");
    session(&laptop, "app", "Tidy the laptop part");
    server.user.provider_script(
        "claude",
        json!({"turns": [{"when": "Keep watching", "do": [{"hold": "release"}]}]}),
    );
    let running = server
        .user
        .call(
            "agent.start",
            json!({"project": "app", "message": "Keep watching"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the server's session works", || {
        server
            .user
            .call("agent.show", json!({"agent": running}))
            .is_ok_and(|shown| shown["agent"]["state"] == "working")
    });
    // The laptop's session stays in this machine's copy of its list while it is off.
    eventually("pc is told the laptop's session", || {
        let lists = pc.user.call("remote.sessions", json!({})).unwrap();
        lists["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|listed| {
                listed["name"] == "laptop"
                    && listed["sessions"].as_array().is_some_and(|s| !s.is_empty())
            })
    });
    let stop = laptop.user.run(&["daemon", "stop"]);
    assert!(stop.status.success(), "{}", laptop.log());
    pc.wait_for("laptop", "offline");

    let (shell, cx) = page_window(&pc, &["server"], cx);
    show(&shell, Selection::Project("app".into()), cx);
    until(&shell, cx, "both machines' blocks", |cx| {
        present(cx, machine_block_id("server")) && present(cx, machine_block_id("laptop"))
    });
    // Its turn ends with the page shown: the page takes the snapshot that says so. A session that
    // runs spins its mark, by design, and is no rest (`window_at_rest.rs` waits for every agent
    // to be idle); the page is measured once it ended.
    std::fs::write(server.user.path("fake-claude/release"), "").unwrap();
    eventually("the server's session ends its turn", || {
        server
            .user
            .call("agent.show", json!({"agent": running}))
            .is_ok_and(|shown| shown["agent"]["state"] == "idle")
    });
    // The window's copy of the server's list says it too before the page is measured.
    until(&shell, cx, "the server's list says no session runs", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines().read(cx).machines().is_some_and(|read| {
                read.lists
                    .machines
                    .iter()
                    .filter(|(name, _)| name == "server")
                    .flat_map(|(_, listed)| listed)
                    .all(|session| session.state == "idle")
            })
        })
    });
    quiet(&shell, cx);
    (hub, pc, server, shell, cx)
}

#[gpui_kit::test]
fn a_shared_project_s_page_draws_nothing_at_rest(cx: &mut TestAppContext) {
    let (_hub, _pc, _server, shell, cx) = shared_page_settled(cx);
    assert_eq!(
        label(cx, machine_block_id("laptop"))
            .as_deref()
            .map(|said| said.starts_with("laptop · offline since ")),
        Some(true)
    );
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "(drawn, asked)");
}

#[gpui_kit::test]
fn a_snapshot_of_another_machine_that_changes_nothing_draws_nothing(cx: &mut TestAppContext) {
    let (_hub, _pc, _server, shell, cx) = shared_page_settled(cx);
    // The machines' worker tells what it read again after each thing the window asked, whether it
    // changed or not (`Remote::ask`): the same read, as the daemon says it, comes again.
    let read = shell
        .read_with(cx, |s, cx| s.machines().read(cx).machines().cloned())
        .expect("the machines are read");
    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        if gone.is_zero() || gone == Duration::from_millis(16 * 100) {
            let again = read.clone();
            shell.update_in(cx, |s, window, cx| {
                s.machines().update(cx, |machines, cx| {
                    machines.news(News::Read(Box::new(again)), window, cx)
                })
            });
        }
        asked += step(&shell, Duration::from_millis(16), cx);
        gone += Duration::from_millis(16);
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    assert_eq!((drawn, asked), (0, 0), "(drawn, asked)");
}
