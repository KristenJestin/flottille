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
use flottille_window::project::{
    machine_block_id, machine_session_ago_id, machine_session_mark_id, session_row_id,
};
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

// A shared project is matched by its repositories' remotes, whatever its name on each machine
// (#1410): the page, the start choice and the settings switch use the sidebar's match.

#[gpui_kit::test]
fn a_project_cloned_under_another_name_is_the_same_shared_project(cx: &mut TestAppContext) {
    let (_hub, pc, server) = ready_machines();
    add(&pc, "app", APP);
    add(&server, "web-app", APP);
    told(&pc, &["web-app"]);
    let here = session(&pc, "app", "Fix the pc part");
    let there = session(&server, "web-app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = page_window(&pc, &["server"], cx);
    assert_eq!(homes(&shell, "app", cx), ["pc", "server"], "one row");
    // The page lists the server's session in the server's block.
    show(&shell, Selection::Project("app".into()), cx);
    until(&shell, cx, "both sessions on app's page", |cx| {
        present(cx, session_row_id(&here))
            && present(cx, session_row_id(&row))
            && present(cx, machine_block_id("server"))
    });
    assert_eq!(shown(&shell, cx), None, "this machine's page of it");

    // The start choice offers the server, and takes it to the server's page of its `web-app`.
    until(&shell, cx, "app's composer offers the server", |cx| {
        present(cx, "machine-picker")
    });
    click(&shell, "machine-picker", cx);
    click(&shell, "machine-choice-server", cx);
    until(&shell, cx, "the server's page of web-app", |cx| {
        shown(&shell, cx).as_deref() == Some("server")
            && shell.read_with(cx, |s, _| {
                s.selected() == Some(&Selection::Project("web-app".into()))
            })
            && present(cx, machine_block_id("pc"))
            && present(cx, session_row_id(&here))
    });
    // And back from there to this machine's `app`.
    click(&shell, "machine-picker", cx);
    click(&shell, "machine-choice-here", cx);
    until(&shell, cx, "this machine's page of app", |cx| {
        shown(&shell, cx).is_none()
            && shell.read_with(cx, |s, _| {
                s.selected() == Some(&Selection::Project("app".into()))
            })
    });

    // The settings switch offers the server too, and shows its settings of `web-app`.
    show(&shell, Selection::ProjectSettings("app".into()), cx);
    until(&shell, cx, "app's switch of machines", |cx| {
        present(cx, "project-settings-machine")
    });
    click(&shell, "project-settings-machine", cx);
    until(&shell, cx, "the server offered", |cx| {
        present(cx, "project-settings-machine-server")
    });
    click(&shell, "project-settings-machine-server", cx);
    until(&shell, cx, "the server's settings of web-app", |cx| {
        shown(&shell, cx).as_deref() == Some("server")
            && shell.read_with(cx, |s, _| {
                s.selected() == Some(&Selection::ProjectSettings("web-app".into()))
            })
    });
}

#[gpui_kit::test]
fn two_projects_of_the_same_name_with_other_remotes_are_not_one(cx: &mut TestAppContext) {
    let (_hub, pc, server) = ready_machines();
    add(&pc, "app", APP);
    add(&server, "app", "https://git.example/other/app.git");
    told(&pc, &["app"]);
    let there = session(&server, "app", "Ship the other app");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = page_window(&pc, &["server"], cx);
    until(&shell, cx, "the server's namesake row", |cx| {
        present(cx, project_id("app@server"))
    });
    assert_eq!(homes(&shell, "app", cx), ["pc"]);
    // This machine's page of `app` holds nothing of the server's namesake, and its start choice
    // does not offer the server.
    show(&shell, Selection::Project("app".into()), cx);
    until(&shell, cx, "app's composer", |cx| {
        present(cx, "place-picker")
    });
    settle(&shell, cx);
    assert_eq!(shown(&shell, cx), None);
    assert!(!present(cx, machine_block_id("server")));
    assert!(!present(cx, session_row_id(&row)));
    assert!(!present(cx, "machine-picker"), "the server is not offered");
    // Nor does its settings switch.
    show(&shell, Selection::ProjectSettings("app".into()), cx);
    until(&shell, cx, "app's settings", |cx| {
        present(cx, "project-settings-header")
    });
    settle(&shell, cx);
    assert!(!present(cx, "project-settings-machine"));
}

// A row only another machine has opens that machine's project, by the name the sidebar grouped
// it by, never one of this machine's that the row does not hold (#1410): the same repository
// cloned twice there, and a project with no remote on both machines.

#[gpui_kit::test]
fn a_row_of_another_machine_alone_opens_that_machine_s_project(cx: &mut TestAppContext) {
    let (_hub, pc, server) = ready_machines();
    add(&pc, "app", APP);
    add(&pc, "notes", "");
    add(&server, "app", APP);
    add(&server, "app2", APP);
    add(&server, "notes", "");
    told(&pc, &["app", "app2", "notes"]);

    let (shell, cx) = page_window(&pc, &["server"], cx);
    until(&shell, cx, "the server's rows", |cx| {
        present(cx, project_id("app2")) && present(cx, project_id("notes@server"))
    });
    assert_eq!(homes(&shell, "app", cx), ["pc", "server"]);
    assert_eq!(homes(&shell, "app2", cx), ["server"], "a clone of its own");
    assert_eq!(homes(&shell, "notes", cx), ["pc"]);
    assert_eq!(homes(&shell, "notes@server", cx), ["server"]);

    // The same repository cloned twice on the server: `app2` is the server's, not this `app`.
    show(&shell, Selection::Project("app2".into()), cx);
    until(&shell, cx, "the server's page of app2", |cx| {
        shown(&shell, cx).as_deref() == Some("server")
            && shell.read_with(cx, |s, _| {
                s.selected() == Some(&Selection::Project("app2".into()))
            })
            && present(cx, "place-picker")
    });
    settle(&shell, cx);
    assert!(!present(cx, "machine-picker"), "only the server has it");

    // No remote on either machine: `notes@server` is the server's `notes`, not this one's.
    show(&shell, Selection::Project("notes@server".into()), cx);
    until(&shell, cx, "the server's page of notes", |cx| {
        shown(&shell, cx).as_deref() == Some("server")
            && shell.read_with(cx, |s, _| {
                s.selected() == Some(&Selection::Project("notes".into()))
            })
            && present(cx, "place-picker")
    });
    settle(&shell, cx);
    assert!(
        !present(cx, "machine-picker"),
        "the switch follows the sidebar's rows"
    );
    // Nor does its settings switch offer this machine's `notes`.
    show(&shell, Selection::ProjectSettings("notes".into()), cx);
    until(&shell, cx, "the server's settings of notes", |cx| {
        present(cx, "project-settings-header")
    });
    settle(&shell, cx);
    assert_eq!(shown(&shell, cx).as_deref(), Some("server"));
    assert!(!present(cx, "project-settings-machine"));
}

// The window's machine switch and a start take the row's machines only (#1439): a project of the
// same name that the row does not mark is never opened.

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

#[gpui_kit::test]
fn switching_the_window_to_a_machine_the_row_does_not_mark_opens_no_project_of_its_name(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = ready_machines();
    // No remote on either machine: the server's `notes` is a row of its own, not this one's.
    add(&pc, "notes", "");
    add(&server, "notes", "");
    told(&pc, &["notes"]);

    let (shell, cx) = page_window(&pc, &["server"], cx);
    until(&shell, cx, "the server's row", |cx| {
        present(cx, project_id("notes@server"))
    });
    assert_eq!(homes(&shell, "notes@server", cx), ["server"]);
    show(&shell, Selection::Project("notes@server".into()), cx);
    until(&shell, cx, "the server's page of notes", |cx| {
        shown(&shell, cx).as_deref() == Some("server")
            && shell.read_with(cx, |s, _| {
                s.selected() == Some(&Selection::Project("notes".into()))
            })
    });

    // Switching to this machine: the row does not mark it, so its own `notes` is not opened.
    shell.update_in(cx, |s, window, cx| s.choose_machine(None, window, cx));
    settle(&shell, cx);
    assert_eq!(shown(&shell, cx), None, "this machine is in front");
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Settings),
        "its Settings, not its `notes`"
    );
}

#[gpui_kit::test]
fn a_row_whose_machines_cannot_run_starts_nowhere_else(cx: &mut TestAppContext) {
    let (hub, pc, server) = ready_machines();
    let laptop = Machine::of(ready_user());
    crate::remote::pair(&hub, &laptop, "laptop", "runner");
    pc.wait_for("laptop", "online");
    // This machine lacks `app`; the server has it, and the laptop has another project of that
    // name (other remote): its own row, which the server's row does not mark.
    add(&server, "app", APP);
    add(&laptop, "app", "https://git.example/other/app.git");
    told_by(&pc, "server", &["app"]);
    told_by(&pc, "laptop", &["app"]);
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");

    let (shell, cx) = page_window(&pc, &["laptop"], cx);
    until(&shell, cx, "both rows", |cx| {
        present(cx, project_id("app")) && present(cx, project_id("app@laptop"))
    });
    assert_eq!(homes(&shell, "app", cx), ["server"]);
    assert_eq!(homes(&shell, "app@laptop", cx), ["laptop"]);

    // The row's only machine is off: nothing starts, and no other machine's `app` is shown.
    let before = shell.read_with(cx, |s, _| s.selected().cloned());
    show(&shell, Selection::Project("app".into()), cx);
    settle(&shell, cx);
    assert_eq!(
        shown(&shell, cx),
        None,
        "the laptop's namesake is not shown"
    );
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        before,
        "no page of an `app` was opened"
    );
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|title| title == "app cannot be started"),
        "{:?}",
        toasts(&shell, cx)
    );
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

/// What the drawn time ago of the row of `session` of `machine` says.
fn said_ago(machine: &str, session: &str, cx: &mut VisualTestContext) -> Option<String> {
    label(cx, machine_session_ago_id(&elsewhere_id(machine, session)))
}

/// The frames the window draws over `by` of its clock, which the clock of its page reads as well.
fn frames_over(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) -> u64 {
    cx.update(|window, _| window.reset_layout_stats());
    step(shell, by, cx);
    cx.update(|window, _| window.layout_stats().frames)
}

#[gpui_kit::test]
fn another_machine_s_session_rows_say_how_long_ago_as_time_goes_by(cx: &mut TestAppContext) {
    let (_hub, pc, server) = ready_machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    told(&pc, &["app"]);
    let there = session(&server, "app", "Ship the server part");

    let (shell, cx) = page_window(&pc, &["server"], cx);
    show(&shell, Selection::Project("app".into()), cx);
    until(&shell, cx, "the server's session on the page", |cx| {
        said_ago("server", &there, cx).is_some()
    });
    quiet(&shell, cx);
    assert_eq!(said_ago("server", &there, cx).as_deref(), Some("now"));

    // The clock of the page runs and the text is the same: nothing is drawn.
    flottille_window::shell::move_clock_on_this_thread(Duration::from_secs(10));
    assert_eq!(frames_over(&shell, Duration::from_secs(30), cx), 0);
    assert_eq!(said_ago("server", &there, cx).as_deref(), Some("now"));

    // Two minutes on, the row says so, drawn once.
    flottille_window::shell::move_clock_on_this_thread(Duration::from_secs(120));
    assert!(frames_over(&shell, Duration::from_secs(30), cx) > 0);
    assert_eq!(said_ago("server", &there, cx).as_deref(), Some("2m"));

    // And again nothing, while the text is the same.
    quiet(&shell, cx);
    assert_eq!(frames_over(&shell, Duration::from_secs(30), cx), 0);
    assert_eq!(said_ago("server", &there, cx).as_deref(), Some("2m"));
}

// The scenarios of #1249: the sidebar's look across machines, after its prototype (variant A, a
// presence badge on each machine's mark).

/// What the kit last drew `id` in, on this thread.
fn colour_of(id: &str) -> Option<gpui_kit::Hsla> {
    gpui_design_system::seen::colour(gpui_kit::ElementId::Name(id.to_owned().into()))
}

/// Whether two colours are the same, but for the rounding of a tint at rest.
fn same_colour(a: Option<gpui_kit::Hsla>, b: Option<gpui_kit::Hsla>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            (a.h - b.h).abs() < 1e-3
                && (a.s - b.s).abs() < 1e-3
                && (a.l - b.l).abs() < 1e-3
                && (a.a - b.a).abs() < 1e-3
        }
        _ => false,
    }
}

/// The id of the part `part` of the element `id` (`machine-mark-server` → `machine-mark-server-online`).
fn part_of(id: gpui_kit::ElementId, part: &str) -> String {
    format!("{id}-{part}")
}

/// The colour the window draws `machine` in, as the member list signs it.
fn shade_of(shell: &Entity<Shell>, machine: &str, cx: &mut VisualTestContext) -> gpui_kit::Hsla {
    shell.read_with(cx, |s, cx| {
        let read = s
            .machines()
            .read(cx)
            .machines()
            .expect("the machines are read");
        let member = read
            .members
            .iter()
            .find(|member| member.name == machine)
            .expect("a member of the list");
        flottille_window::machines::shade(&member.look().1)
    })
}

#[gpui_kit::test]
fn a_device_that_runs_nothing_is_no_group_of_the_projects_list(cx: &mut TestAppContext) {
    let (hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "site", SITE);
    told(&pc, &["site"]);
    let _phone = crate::remote::Phone::pair(&hub, "mobile");
    pc.wait_for_member("mobile", true);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "the window knows the phone", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .is_some_and(|m| m.machine("mobile").is_some())
        })
    });
    switch_grouping(&shell, cx);
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc")) && present(cx, machine_id("server"))
    });
    settle(&shell, cx);
    assert!(
        !present(cx, machine_id("mobile")),
        "a controller is no group of the projects list"
    );
}

#[gpui_kit::test]
fn a_project_with_nothing_under_it_shows_no_open_chevron(cx: &mut TestAppContext) {
    use gpui_design_system::rows::toggle_id;
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&pc, "notes", "");
    add(&server, "app", APP);
    told(&pc, &["app"]);
    session(&pc, "app", "Fix the pc part");

    let (shell, cx) = with_row(&pc, "notes", cx);
    until(&shell, cx, "the session under app", |cx| {
        present(cx, toggle_id(project_id("app")))
    });
    settle(&shell, cx);
    assert!(
        !present(cx, toggle_id(project_id("notes"))),
        "a project with nothing under it has no chevron"
    );
    // Its title starts where every other project's does.
    let title = |name: &str, cx: &mut VisualTestContext| {
        f32::from(bounds(cx, format!("project-{name}-title")).origin.x)
    };
    assert!((title("notes", cx) - title("app", cx)).abs() < 0.5);

    // By machine too: server's app holds nothing of server.
    switch_grouping(&shell, cx);
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc")) && present(cx, machine_id("server"))
    });
    settle(&shell, cx);
    assert!(present(
        cx,
        toggle_id(flottille_window::tree::project_on_id("app", "pc"))
    ));
    assert!(!present(
        cx,
        toggle_id(flottille_window::tree::project_on_id("app", "server"))
    ));

    // A session comes: its chevron comes with it.
    switch_grouping(&shell, cx);
    session(&pc, "notes", "Write the notes");
    until(&shell, cx, "the chevron of notes", |cx| {
        present(cx, toggle_id(project_id("notes")))
    });
}

#[gpui_kit::test]
fn a_project_in_a_machine_s_group_does_not_repeat_its_icon(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    told(&pc, &["app"]);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "both marks on the row", |cx| {
        present(cx, project_mark_id("app", "pc")) && present(cx, project_mark_id("app", "server"))
    });
    switch_grouping(&shell, cx);
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc")) && present(cx, machine_id("server"))
    });
    settle(&shell, cx);
    assert!(present(cx, machine_mark_id("pc")) && present(cx, machine_mark_id("server")));
    assert!(!present(cx, project_mark_id("app", "pc")));
    assert!(!present(cx, project_mark_id("app", "server")));
}

#[gpui_kit::test]
fn each_machine_s_mark_says_its_state_by_its_badge(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    told(&pc, &["app"]);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "both marks on the row", |cx| {
        present(cx, project_mark_id("app", "server"))
    });
    settle(&shell, cx);
    // On a project's row: this machine a house, server a filled dot, each in its own colour.
    assert!(present(cx, part_of(project_mark_id("app", "pc"), "here")));
    assert!(present(
        cx,
        part_of(project_mark_id("app", "server"), "online")
    ));
    assert_eq!(
        label(cx, project_mark_id("app", "server")).as_deref(),
        Some("server · online")
    );
    let server_ink = shade_of(&shell, "server", cx);
    let drawn = colour_of(&part_of(project_mark_id("app", "server"), "icon"));
    assert!(same_colour(drawn, Some(server_ink)), "{drawn:?}");

    // The group's heading: the same mark, and no words after the name.
    switch_grouping(&shell, cx);
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc")) && present(cx, machine_id("server"))
    });
    settle(&shell, cx);
    assert!(present(cx, part_of(machine_mark_id("pc"), "here")));
    assert!(present(cx, part_of(machine_mark_id("server"), "online")));
    assert_eq!(
        label(cx, machine_mark_id("server")).as_deref(),
        Some("server · online")
    );
    assert_eq!(
        label(cx, machine_mark_id("pc")).as_deref(),
        Some("pc · this machine")
    );
    for machine in ["pc", "server"] {
        assert!(
            !present(cx, flottille_window::tree::machine_state_id(machine)),
            "the heading of {machine} ends with no words"
        );
    }

    // Off: a hollow ring, the words in the tooltip.
    let stopped = server.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", server.log());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the hollow ring", |cx| {
        present(cx, part_of(machine_mark_id("server"), "offline"))
    });
    settle(&shell, cx);
    assert!(!present(cx, part_of(machine_mark_id("server"), "online")));
    let said = label(cx, machine_mark_id("server")).unwrap_or_default();
    assert!(said.starts_with("server · offline since "), "{said}");
}

#[gpui_kit::test]
fn the_machines_marks_keep_their_colour_whichever_machine_is_in_front(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&server, "app", APP);
    told(&pc, &["app"]);
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "the server's session", |cx| {
        present(cx, session_id(&row)) && present(cx, project_mark_id("app", "server"))
    });
    settle(&shell, cx);
    let icon = part_of(project_mark_id("app", "server"), "icon");
    let before = colour_of(&icon);
    assert!(
        same_colour(before, Some(shade_of(&shell, "server", cx))),
        "{before:?}"
    );

    cx.update(|window, cx| window.click(session_id(&row), cx));
    until(&shell, cx, "the server's session is open", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    settle(&shell, cx);
    let after = colour_of(&icon);
    assert!(same_colour(after, before), "{after:?} after {before:?}");
}

/// What the sidebar shows, top to bottom: each project and its sessions, each with its place.
fn sidebar_as_drawn(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    let mut drawn = Vec::new();
    for name in drawn_order(shell, cx) {
        let rows: Vec<String> = shell.read_with(cx, |s, cx| {
            s.project(&name)
                .map(|node| {
                    node.read(cx)
                        .sessions(cx)
                        .iter()
                        .map(|row| row.read(cx).id().to_owned())
                        .collect()
                })
                .unwrap_or_default()
        });
        drawn.push(name);
        for id in rows {
            let place =
                label(cx, flottille_window::tree::session_place_id(&id)).unwrap_or_default();
            drawn.push(format!("  {id} {place}"));
        }
    }
    drawn
}

#[gpui_kit::test]
fn opening_a_session_of_another_machine_leaves_the_sidebar_as_it_is(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "app", APP);
    add(&pc, "notes", "");
    add(&server, "app", APP);
    add(&server, "site", SITE);
    told(&pc, &["app", "site"]);
    session(&pc, "app", "Fix the pc part");
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = with_row(&pc, "app", cx);
    until(&shell, cx, "every row", |cx| {
        present(cx, session_id(&row)) && present(cx, project_id("site"))
    });
    settle(&shell, cx);
    let before = sidebar_as_drawn(&shell, cx);

    cx.update(|window, cx| window.click(session_id(&row), cx));
    until(&shell, cx, "the server's session is open", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    settle(&shell, cx);
    assert_eq!(sidebar_as_drawn(&shell, cx), before);
}

#[gpui_kit::test]
fn a_project_only_this_machine_has_stays_listed_while_another_machine_is_in_front(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = machines();
    add(&pc, "notes", "");
    add(&server, "app", APP);
    told(&pc, &["app"]);
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = with_row(&pc, "notes", cx);
    until(&shell, cx, "the server's session", |cx| {
        present(cx, session_id(&row))
    });
    cx.update(|window, cx| window.click(session_id(&row), cx));
    until(&shell, cx, "the server's session is open", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some())
    });
    settle(&shell, cx);
    assert!(present(cx, project_id("notes")));
    assert_eq!(homes(&shell, "notes", cx), ["pc"]);
}

#[gpui_kit::test]
fn no_skeleton_in_the_sidebar_when_a_machine_comes_in_front(cx: &mut TestAppContext) {
    let (_hub, pc, server) = machines();
    add(&pc, "notes", "");
    add(&server, "app", APP);
    told(&pc, &["app"]);
    let there = session(&server, "app", "Ship the server part");
    let row = elsewhere_id("server", &there);

    let (shell, cx) = with_row(&pc, "notes", cx);
    until(&shell, cx, "the server's session", |cx| {
        present(cx, session_id(&row))
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&row), cx));
    // Every frame until the page is there: the sidebar keeps its rows.
    let deadline = Instant::now() + WAIT;
    loop {
        step(&shell, Duration::from_millis(16), cx);
        assert!(
            !present(cx, "sidebar-skeleton-0"),
            "the sidebar said it loads"
        );
        assert!(present(cx, project_id("notes")));
        if shell.read_with(cx, |s, cx| s.main().read(cx).session_page().is_some()) {
            break;
        }
        assert!(Instant::now() < deadline, "the page never came");
    }
}
