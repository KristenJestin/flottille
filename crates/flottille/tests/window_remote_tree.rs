//! The scenarios of #819: sessions on other machines shown under their pilot (`docs/remote.md`,
//! "Requests from another machine's clients", "A tree across machines"). On GPUI's test platform
//! in front of real daemons, each with its own temporary HOME and XDG folders, fake agents and no
//! keyring, and a hub served in the test on 127.0.0.1: the window signs with the device key file
//! of its machine's key folder. Throwaway repositories. No window opens on a screen.

use crate::common;
use crate::remote::{Machine, TestHub, pc_and_server_on};

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::protocol::Event;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::model::elsewhere_id;
use flottille_window::project::{SessionLine, session_end_id, session_menu_id, session_row_id};
use flottille_window::session_link::SessionUpdate;
use flottille_window::shell::Selection;
use flottille_window::tree::{session_id, session_machine_id, session_mark_id};
use gpui_design_system::omarchy::Environment;
use gpui_design_system::tie_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// How long a link, a launch or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);

/// A user whose agents are the fake agent, Claude enabled and signed in (its login file only
/// there, never read), offers looked at often.
fn runner() -> User {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_OFFER_MS", "300")
        .with_env("ANTHROPIC_API_KEY", "")
        .with_env("CLAUDE_CODE_OAUTH_TOKEN", "")
        .with_env("ANTHROPIC_AUTH_TOKEN", "");
    let claude = user.path("home/.claude");
    let user = user.with_env("CLAUDE_CONFIG_DIR", claude.to_str().unwrap());
    user.write_providers("[claude]\nenabled = true\n");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join(".credentials.json"), "{}").unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The project `app` of `machine`, its one repository at `work/app`.
fn register(machine: &Machine) {
    let text = format!(
        "folder = \"{}\"\nworkspaces_root = \"{}\"\nbranch_prefix = \"\"\n\
         [[repositories]]\npath = \".\"\nbase = [\"main\"]\n",
        machine.user.path("work/app").display(),
        machine.user.path("work/app-work").display()
    );
    machine.user.write_project_file("app", &text);
    machine.user.call("project.list", json!({})).unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A hub, `pc` its first member with the project `app` on `feat/export`, and `server` a runner
/// paired with it, `app` registered there, its judge allowing; the two linked, `server` offering.
fn pc_and_server() -> (TestHub, Machine, Machine) {
    let server = runner().with_judge(json!([
        {"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}
    ]));
    let (hub, pc, server) = pc_and_server_on(Machine::of(runner()), server);
    let repo = pc.user.path("work/app");
    pc.user.git_init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    pc.user.git(&repo, &["add", "."]);
    pc.user.git(&repo, &["commit", "--quiet", "-m", "first"]);
    pc.user
        .git(&repo, &["checkout", "--quiet", "-b", "feat/export"]);
    register(&pc);
    register(&server);
    eventually("server offers app", || {
        let machines = pc.remote();
        machines["machines"].as_array().unwrap().iter().any(|m| {
            m["name"] == "server"
                && m["offers"]["projects"]
                    .as_array()
                    .is_some_and(|projects| projects.iter().any(|p| p["name"] == "app"))
        })
    });
    (hub, pc, server)
}

/// A session the user starts in `app` on `pc`, its first turn ended, in `mode`: its id and its
/// MCP session.
fn root(pc: &Machine, mode: &str) -> (String, Mcp) {
    let opened = pc.user.received_requests("session/new").len();
    let started = pc
        .user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(&pc.user, opened);
    eventually("the root's first turn", || {
        pc.user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    pc.user
        .call("agent.mode", json!({"agent": id, "mode": mode}))
        .unwrap();
    (id, Mcp::initialize(&url, &authorization))
}

/// A session led by `pc`'s root started on `server`: its id there.
fn led_on_server(mcp: &Mcp) -> String {
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export of the report."}),
    );
    assert!(!answer.error, "{}", answer.text);
    let at = answer
        .text
        .find("(agent ")
        .unwrap_or_else(|| panic!("no agent in {}", answer.text));
    answer.text[at + 7..at + 19].to_owned()
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

/// One step of the window as the platform would run it.
fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
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

/// The frame a session's row unfolds in, while the tree has the row.
fn frame_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let row = shell.read_with(cx, |s, _| s.session(id).map(|row| row.entity_id()))?;
    Some(format!("tree-row-{}", row.as_u64()))
}

/// The heights of `frame`: none once it is gone, two while a row that moves between two projects
/// of the tree (the one its pilot has and the one its machine lists) is in both, the one it
/// leaves folding away.
fn heights(cx: &mut VisualTestContext, frame: &str) -> Vec<f32> {
    cx.update(|window, _| {
        window
            .find_all(frame.to_owned())
            .iter()
            .map(|found| f32::from(found.bounds().size.height))
            .collect()
    })
}

/// The window on `pc`, the session led on `server` under its pilot's row.
fn the_led_row<'a>(
    pc: &Machine,
    agent: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext, String) {
    let (shell, cx) = window(pc, cx);
    let row = elsewhere_id("server", agent);
    until(&shell, cx, "the row of the session on server", |cx| {
        present(cx, session_id(&row))
    });
    settle(&shell, cx);
    (shell, cx, row)
}

#[gpui_kit::test]
fn a_session_on_another_machine_shows_under_its_pilot_with_the_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (pilot, mcp) = root(&pc, "auto");
    let agent = led_on_server(&mcp);
    eventually("the session runs on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .is_ok()
    });

    let (shell, cx, row) = the_led_row(&pc, &agent, cx);

    // Right under its pilot's row, tied to it, with its title.
    let listed: Vec<String> = shell.read_with(cx, |s, cx| {
        s.project("app")
            .map(|node| {
                node.read(cx)
                    .sessions(cx)
                    .iter()
                    .map(|row| row.read(cx).id().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    });
    let at = listed
        .iter()
        .position(|id| *id == pilot)
        .expect("the pilot");
    assert_eq!(listed.get(at + 1), Some(&row), "{listed:?}");
    assert!(
        label(cx, tie_id(session_id(&row))).is_some(),
        "tied to its pilot"
    );
    assert_eq!(
        label(cx, session_id(&row)).as_deref(),
        Some("Finish the export")
    );
    // Its machine's name and its state, as that machine lists it or as last known.
    let said = label(cx, session_machine_id(&row)).unwrap_or_default();
    assert!(said.starts_with("server · "), "{said}");
    let state = said.trim_start_matches("server · ");
    assert!(
        ["running", "idle", "waiting", "reported"].contains(&state),
        "{said}"
    );
}

/// A hub, `pc` its first member with the project `app` on `feat/export`, `more` the more lines of
/// its file, and `server` a runner paired with it that has no project: the launch makes it there.
fn pc_and_bare_server(more: &str) -> (TestHub, Machine, Machine) {
    let server = runner().with_judge(json!([
        {"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}
    ]));
    let (hub, pc, server) = pc_and_server_on(Machine::of(runner()), server);
    let repo = pc.user.path("work/app");
    pc.user.git_init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    pc.user.git(&repo, &["add", "."]);
    pc.user.git(&repo, &["commit", "--quiet", "-m", "first"]);
    pc.user
        .git(&repo, &["checkout", "--quiet", "-b", "feat/export"]);
    // No branch convention of its own: one would make the launch the user's to decide there.
    let text = format!(
        "folder = \"{}\"\nworkspaces_root = \"{}\"\n{more}\n\
         [[repositories]]\npath = \".\"\nbase = [\"main\"]\n",
        pc.user.path("work/app").display(),
        pc.user.path("work/app-work").display()
    );
    pc.user.write_project_file("app", &text);
    pc.user.call("project.list", json!({})).unwrap();
    eventually("server offers Claude", || {
        pc.remote()["machines"].as_array().unwrap().iter().any(|m| {
            m["name"] == "server"
                && m["offers"]["agents"]
                    .as_array()
                    .is_some_and(|agents| agents.iter().any(|a| a == "claude"))
        })
    });
    (hub, pc, server)
}

#[gpui_kit::test]
fn the_making_of_a_project_shows_on_the_launch_s_row(cx: &mut TestAppContext) {
    // The project is made there, and its session says nothing back for a while.
    let (_hub, pc, server) = pc_and_bare_server("");
    server.user.fake_script(json!({"turns": [
        {"when": "Finish the export", "do": [{"wait": 60000}, {"say": "done"}]}
    ]}));
    let (_pilot, mcp) = root(&pc, "auto");
    let agent = led_on_server(&mcp);
    eventually("the session made on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .is_ok()
    });

    let (shell, cx, row) = the_led_row(&pc, &agent, cx);

    let deadline = Instant::now() + WAIT;
    loop {
        step(&shell, Duration::from_millis(16), cx);
        let said = label(cx, session_machine_id(&row));
        if said.as_deref() == Some("server · preparing app") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the row says {said:?}\n{}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[gpui_kit::test]
fn opening_it_shows_that_machine_s_page_of_it(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (_pilot, mcp) = root(&pc, "auto");
    let agent = led_on_server(&mcp);
    eventually("the session runs on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .is_ok()
    });
    let (shell, cx, row) = the_led_row(&pc, &agent, cx);

    cx.update(|window, cx| window.click(session_id(&row), cx));

    until(&shell, cx, "the page of that session, server's", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().as_deref() == Some("server")
                && s.main()
                    .read(cx)
                    .session_page()
                    .is_some_and(|page| page.read(cx).id() == agent)
        })
    });
    // The page arrives: its header and its composer enter, one after the other.
    for section in ["0", "2"] {
        let id = gpui_kit::ElementId::Name(format!("session-section-{agent}-{section}").into());
        assert!(present(cx, id), "section {section} enters");
    }
    // Its row stays under its pilot: the list is this machine's, nothing switched (#1271).
    settle(&shell, cx);
    assert!(present(cx, session_id(&row)), "its row left the list");
    assert!(server.log().contains("asked by pc"), "{}", server.log());
}

#[gpui_kit::test]
fn its_state_follows_offline_and_back(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (_pilot, mcp) = root(&pc, "auto");
    let agent = led_on_server(&mcp);
    eventually("the session runs on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .is_ok()
    });
    let (shell, cx, row) = the_led_row(&pc, &agent, cx);
    let before = label(cx, session_machine_id(&row)).unwrap_or_default();
    assert!(!before.contains("offline"), "{before}");

    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the row says server is offline", |cx| {
        label(cx, session_machine_id(&row))
            .is_some_and(|said| said.starts_with("server · offline since "))
    });

    server.user.status();
    pc.wait_for("server", "online");
    until(&shell, cx, "the row says its state again", |cx| {
        label(cx, session_machine_id(&row))
            .is_some_and(|said| said.starts_with("server · ") && !said.contains("offline"))
    });
}

#[gpui_kit::test]
fn rows_of_other_machines_unfold_and_fold_away(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    // In Full, its end is let through as a session's it leads on this machine would be.
    let (_pilot, mcp) = root(&pc, "full");
    let (shell, cx) = window(&pc, cx);
    settle(&shell, cx);

    let agent = led_on_server(&mcp);
    let row = elsewhere_id("server", &agent);
    until(&shell, cx, "the row comes", |cx| {
        frame_of(&shell, &row, cx).is_some_and(|frame| !heights(cx, &frame).is_empty())
    });
    let frame = frame_of(&shell, &row, cx).unwrap();
    let coming = heights(cx, &frame).into_iter().fold(f32::MAX, f32::min);
    settle(&shell, cx);
    let full = heights(cx, &frame).into_iter().fold(0., f32::max);
    assert!(full > 0., "{full}");
    assert!(
        coming < full,
        "the row unfolds from nothing: {coming} of {full}"
    );

    let stopped = mcp.call("agent_stop", json!({"agent_id": agent}));
    assert!(!stopped.error, "{}", stopped.text);
    eventually("the session ended on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .unwrap()["agent"]["state"]
            == "ended"
    });
    until(&shell, cx, "the row starts folding away", |cx| {
        heights(cx, &frame).into_iter().all(|height| height < full)
    });
    let going = heights(cx, &frame);
    assert!(
        going.iter().any(|height| *height > 0.),
        "the row folds away, it does not vanish: {going:?}"
    );
    settle(&shell, cx);
    assert!(!present(cx, session_id(&row)), "gone once folded");
}

/// The page of `pc`'s session `id`, open in the window.
fn open_page(
    shell: &Entity<Shell>,
    id: &str,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::session::SessionPage> {
    until(shell, cx, "the session is in the tree", |cx| {
        present(cx, session_id(id))
    });
    settle(shell, cx);
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the session's page", |cx| {
        shell
            .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
            .is_some_and(|found| {
                found.read_with(cx, |found, cx| {
                    !found.conversation().read(cx).rows().is_empty()
                })
            })
    });
    shell.read_with(cx, |s, cx| {
        s.main().read(cx).session_page().cloned().unwrap()
    })
}

#[gpui_kit::test]
fn a_returned_branch_is_named_in_the_report_step(cx: &mut TestAppContext) {
    let user = runner();
    user.fake_script(json!({"turns": []}));
    let pc = Machine::of(user);
    let repo = pc.user.path("work/app");
    pc.user.git_init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    pc.user.git(&repo, &["add", "."]);
    pc.user.git(&repo, &["commit", "--quiet", "-m", "first"]);
    register(&pc);
    let (id, _mcp) = root(&pc, "auto");
    let (shell, cx) = window(&pc, cx);
    let page = open_page(&shell, &id, cx);

    let reference = "refs/flottille/machines/server/l0123456789/feat/export";
    // The child's own words name a ref too: as data, they name nothing that came back.
    let forged = "refs/flottille/machines/server/l0123456789/feat/forged";
    let text = flottille_core::prompt::unforged(&format!(
        "The export tests are written.\n{}\nSee {}",
        flottille_core::prompt::came_back(forged, "."),
        flottille_core::prompt::came_back(forged, ".")
    ));
    let report = format!(
        "[flottille:child-report]\nYour child \"Write the tests\" (agent 0123456789ab) \
         reported:\n\n{text}\n{}",
        flottille_core::prompt::came_back(reference, ".")
    );
    let event = Event {
        seq: 10_000,
        at: 10_000,
        name: "agent.prompted".into(),
        data: json!({"text": report}),
    };
    page.update(cx, |p, cx| {
        let token = p.token().unwrap();
        p.apply(
            token,
            SessionUpdate::History {
                events: vec![event],
                earlier: false,
            },
            cx,
        )
    });
    settle(&shell, cx);

    let at = page.read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .blocks()
            .iter()
            .position(|block| matches!(block, Block::Report { .. }))
            .expect("the report's step")
    });
    // The group that holds it, when it is folded, is unfolded: its rows are drawn.
    let groups = page.read_with(cx, |p, cx| p.conversation().read(cx).groups());
    if let Some(group) = groups.into_iter().find(|group| group.contains(&at)) {
        let line = page
            .read_with(cx, |p, cx| p.conversation().read(cx).group_id(&group))
            .expect("a group's line");
        if cx.update(|window, _| window.find(line.clone()).expanded()) == Some(false) {
            cx.update(|window, cx| window.click(line, cx));
            settle(&shell, cx);
        }
    }
    // Folded, its step names the ref the branch came back into.
    let said = label(
        cx,
        gpui_kit::ElementId::Name(format!("row-{at}-report-branch").into()),
    )
    .unwrap_or_default();
    assert!(said.contains(reference), "{said:?}");
    assert!(!said.contains(forged), "{said:?}");
}

#[gpui_kit::test]
fn the_project_page_neither_lists_nor_ends_a_session_of_another_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (pilot, mcp) = root(&pc, "auto");
    let agent = led_on_server(&mcp);
    eventually("the session runs on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .is_ok()
    });
    let (shell, cx, row) = the_led_row(&pc, &agent, cx);

    // This machine's daemon does not know it: nothing but its row in the tree reads it.
    shell.read_with(cx, |s, _| {
        assert!(s.snapshot().session(&row).is_none(), "a session of pc's");
        assert!(
            s.snapshot().sessions.iter().all(|s| s.elsewhere.is_none()),
            "among pc's sessions"
        );
    });

    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(&shell, cx);
    let lines: Vec<SessionLine> = shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).project_page().unwrap().clone();
        let blocks = page.read(cx).blocks().to_vec();
        blocks
            .iter()
            .flat_map(|block| block.read(cx).data().sessions.clone())
            .collect()
    });
    assert!(
        lines.iter().any(|line| line.id == pilot),
        "the pilot is listed"
    );
    assert!(lines.iter().all(|line| line.id != row), "listed as pc's");
    let leads: Vec<&String> = lines
        .iter()
        .flat_map(|l| l.leads.iter().map(|(id, _)| id))
        .collect();
    assert!(
        !leads.contains(&&row),
        "its pilot says it leads it here: {leads:?}"
    );
    assert!(
        !present(cx, session_row_id(&row)),
        "a line of the project page"
    );
    assert!(
        !present(cx, session_menu_id(&row)),
        "a menu that ends it here"
    );
    assert!(!present(cx, session_end_id(&row)), "an end asked of pc");
}

#[gpui_kit::test]
fn the_question_cards_of_another_machine_name_it(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    server.user.fake_script(json!({"turns": [
        {"when": "ask me", "do": [
            {"mcp": {"tool": "ask_user", "arguments": {"question": "Which region?"}}},
            {"say": "Asked."}]}
    ]}));
    let repo = server.user.path("work/app");
    server.user.git_init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    server.user.git(&repo, &["add", "."]);
    server
        .user
        .git(&repo, &["commit", "--quiet", "-m", "first"]);
    let started = server
        .user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let shown = || {
        server
            .user
            .call("agent.show", json!({"agent": id}))
            .unwrap()["agent"]
            .clone()
    };
    eventually("its first turn on server", || shown()["state"] == "idle");
    server
        .user
        .call("agent.mode", json!({"agent": id, "mode": "auto"}))
        .unwrap();
    server
        .user
        .call("agent.send", json!({"agent": id, "message": "ask me"}))
        .unwrap();
    eventually("it asks the user on server", || {
        shown()["waiting_on_answer"] == true
    });
    let (shell, cx) = window(&pc, cx);
    settle(&shell, cx);

    // Its page is the server's, from its row of the one list.
    let row = flottille_window::model::elsewhere_id("server", &id);
    let page = open_page(&shell, &row, cx);
    until(&shell, cx, "the question's card", |cx| {
        page.read_with(cx, |p, cx| p.ask_dock().read(cx).shown().is_some())
    });
    settle(&shell, cx);
    let meta = page.read_with(cx, |p, cx| {
        let card = p.ask_dock().read(cx).shown().unwrap().clone();
        card.read(cx).meta(cx).to_string()
    });
    assert!(meta.ends_with(" · on server"), "{meta:?}");
}

/// Runs the window until it asked for no animation frame over a second of its clock and two of
/// real time (`window_at_rest`'s wait): what the daemons answer late has come.
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
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(STEP);
        cx.run_until_parked();
        let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if asked == 0 {
            still += STEP;
        } else {
            still = Duration::ZERO;
            last_asked = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn a_row_of_another_machine_draws_no_frame_at_rest(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    // A running row's spinner turns by design: a child that reported is at rest.
    server.user.fake_script(json!({"turns": [
        {"when": "Write the tests of the export.", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The export tests are written."}}}]}
    ]}));
    let (_pilot, mcp) = root(&pc, "auto");
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests of the export."}),
    );
    assert!(!answer.error, "{}", answer.text);
    let at = answer
        .text
        .find("(agent ")
        .unwrap_or_else(|| panic!("no agent in {}", answer.text));
    let agent = answer.text[at + 7..at + 19].to_owned();
    let (shell, cx, row) = the_led_row(&pc, &agent, cx);
    until(&shell, cx, "the row says it reported", |cx| {
        label(cx, session_machine_id(&row)).as_deref() == Some("server · reported")
    });
    // The pulse of a new wait moves by design; past it, the dot is still.
    cx.executor()
        .advance_clock(flottille_window::state::PULSE_SPAN);
    quiet(&shell, cx);

    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        asked += cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += Duration::from_millis(16);
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    assert_eq!((drawn, asked), (0, 0), "(frames drawn, frames asked)");
}

#[gpui_kit::test]
fn the_session_header_names_its_machine_by_its_icon(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (pilot, _mcp) = root(&pc, "auto");
    let (shell, cx) = window(&pc, cx);
    open_page(&shell, &pilot, cx);

    // `[icon] pc / main`: the machine, then the place, in the header's path.
    until(&shell, cx, "the header names the machine", |cx| {
        label(cx, "session-machine".into()).as_deref() == Some("pc")
    });
    let mark = label(cx, "session-machine-mark".into()).unwrap_or_default();
    assert!(
        mark.contains(" in "),
        "its icon and colour, in words: {mark:?}"
    );
    assert!(present(cx, "session-place".into()), "the place stays");
}

#[gpui_kit::test]
fn a_session_led_on_another_machine_is_in_the_rail_with_its_machine_icon(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (pilot, mcp) = root(&pc, "auto");
    let agent = led_on_server(&mcp);
    eventually("the session runs on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .is_ok()
    });
    let (shell, cx, _row) = the_led_row(&pc, &agent, cx);
    open_page(&shell, &pilot, cx);

    let id = format!("rail-remote-{agent}");
    until(&shell, cx, "its row in the rail's Agents", |cx| {
        present(cx, id.clone().into())
    });
    let said = label(cx, id.clone().into()).unwrap_or_default();
    assert!(said.starts_with("Finish the export"), "{said}");
    assert!(said.contains("server"), "{said}");
    let mark = label(cx, format!("{id}-mark").into()).unwrap_or_default();
    assert!(mark.contains(" in "), "{mark:?}");
}

#[gpui_kit::test]
fn a_child_on_another_machine_is_in_the_sub_agents_list_with_its_machine_icon(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    let (pilot, mcp) = root(&pc, "auto");
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "child", "machine": "server", "title": "Render the year",
            "brief": "Render a year of invoices."}),
    );
    assert!(!answer.error, "{}", answer.text);
    let at = answer.text.find("(agent ").expect("an agent");
    let agent = answer.text[at + 7..at + 19].to_owned();
    eventually("the child runs on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .is_ok()
    });
    let (shell, cx) = window(&pc, cx);
    let page = open_page(&shell, &pilot, cx);

    // The conversation names the launch: its sub-agents card lists the child, on its machine.
    let call = |seq: u64, data: serde_json::Value, name: &str| Event {
        seq,
        at: seq,
        name: name.into(),
        data,
    };
    let events = vec![
        call(
            20_000,
            json!({"id": "l", "title": "mcp__flottille__agent_launch", "status": "in_progress",
                "raw_input": {"title": "Render the year", "kind": "child", "machine": "server"}}),
            "agent.tool_call",
        ),
        call(
            20_001,
            json!({"id": "l", "update": true, "status": "completed"}),
            "agent.tool_call",
        ),
        call(
            20_002,
            json!({"text": "Rendering it on the server."}),
            "agent.message",
        ),
    ];
    page.update(cx, |p, cx| {
        let token = p.token().unwrap();
        p.apply(
            token,
            SessionUpdate::History {
                events,
                earlier: false,
            },
            cx,
        )
    });
    settle(&shell, cx);
    let at = page.read_with(cx, |p, cx| {
        p.conversation()
            .read(cx)
            .blocks()
            .iter()
            .position(|block| matches!(block, Block::SubAgents))
            .expect("the sub-agents card")
    });
    cx.update(|window, cx| window.click(format!("row-{at}-sub-agents-toggle"), cx));
    settle(&shell, cx);

    let row = format!("sub-agent-{}", elsewhere_id("server", &agent));
    assert!(present(cx, row.clone().into()), "{row}");
    let mark = label(cx, format!("{row}-mark").into()).unwrap_or_default();
    assert!(mark.contains(" in "), "{mark:?}");

    // At rest the row is whole. When its launch is no longer listed it folds away over several
    // frames, then is gone: the frames are drawn with no read of the machines, which would list
    // the launch again.
    let item = format!("{row}-item");
    let height = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            window
                .try_find(item.clone())
                .map(|found| f32::from(found.bounds().size.height))
        })
    };
    let whole = height(cx).expect("the row's frame");
    assert!(whole > 20., "{whole}");
    cx.update(|_, cx| {
        cx.set_global(flottille_window::model::RemoteAgents(Vec::new()));
        cx.refresh_windows();
    });
    let mut drawn = Vec::new();
    for _ in 0..60 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        match height(cx) {
            Some(height) => drawn.push(height),
            None => break,
        }
    }
    assert!(
        drawn.iter().any(|h| *h > 1. && *h < whole - 1.),
        "it folds before it is gone: {drawn:?}"
    );
    assert!(!present(cx, row.clone().into()), "gone once the fold ended");
}

/// A session led by `pc`'s root, in Ask, started on `server` and held there for its user: its id
/// there, once its request waits.
fn held_on_server(mcp: &Mcp, server: &Machine) -> String {
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "You build the export of the report.\nStart with the totals."}),
    );
    assert!(!answer.error, "{}", answer.text);
    let deadline = Instant::now() + WAIT;
    loop {
        let requests = server.user.call("approval.list", json!({})).unwrap();
        if let Some(agent) = requests["requests"][0]["agent"].as_str() {
            return agent.to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "the launch was never held on server"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The state `machine` lists its session `agent` in, for the other machines.
fn listed_state(machine: &Machine, agent: &str) -> Option<String> {
    let lists = machine.user.call("remote.sessions", json!({})).ok()?;
    lists["this"]
        .as_array()?
        .iter()
        .find(|session| session["id"] == agent)
        .and_then(|session| session["state"].as_str())
        .map(str::to_owned)
}

#[gpui_kit::test]
fn a_launch_held_on_another_machine_shows_it_waits_for_the_user(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (_pilot, mcp) = root(&pc, "ask");
    let agent = held_on_server(&mcp, &server);

    // Its machine lists it as needing the user, as its own window shows it.
    eventually("server lists the held session as needing the user", || {
        listed_state(&server, &agent).as_deref() == Some("needs you")
    });
    let (shell, cx, row) = the_led_row(&pc, &agent, cx);
    // From the moment it is held, and for as long as it is, never running: the lists the
    // machines send each other meanwhile say the same.
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        step(&shell, Duration::from_millis(16), cx);
        let said = label(cx, session_mark_id(&row)).unwrap_or_default();
        assert_eq!(said, "Needs you on server");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        label(cx, session_machine_id(&row)).as_deref(),
        Some("server · needs you")
    );
    let mark = shell.read_with(cx, |s, _| {
        s.snapshot()
            .elsewhere(&row)
            .and_then(|session| session.mark)
    });
    assert_eq!(mark, Some(flottille_window::state::AgentState::NeedsYou));
}

#[gpui_kit::test]
fn a_launch_held_on_another_machine_is_titled_with_its_title(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (_pilot, mcp) = root(&pc, "ask");
    let agent = held_on_server(&mcp, &server);

    // On the window of the machine it is held on: its title, never its brief's first line.
    let (shell, cx) = window(&server, cx);
    until(&shell, cx, "the held session is in the tree", |cx| {
        present(cx, session_id(&agent))
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, session_id(&agent)).as_deref(),
        Some("Finish the export")
    );
    // Its machine lists it so for the others.
    let lists = server.user.call("remote.sessions", json!({})).unwrap();
    let listed = lists["this"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == agent.as_str())
        .cloned()
        .unwrap_or_default();
    assert_eq!(listed["title"], "Finish the export", "{lists}");
}

#[gpui_kit::test]
fn the_page_of_a_held_launch_says_it_waits_for_the_user_s_decision(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (_pilot, mcp) = root(&pc, "ask");
    let agent = held_on_server(&mcp, &server);

    let (shell, cx) = window(&server, cx);
    let page = open_page(&shell, &agent, cx);
    settle(&shell, cx);
    let lines: Vec<String> = page.read_with(cx, |page, cx| {
        let conversation = page.conversation().read(cx);
        conversation
            .groups()
            .iter()
            .filter_map(|group| conversation.group_id(group))
            .collect()
    });
    let said: Vec<Option<String>> = lines
        .iter()
        .map(|line| label(cx, line.clone().into()))
        .collect();
    // No step was run: the line over its request says what it waits for, not `0 steps · 0.0s`.
    assert_eq!(
        said,
        [Some("Waits for your decision".to_owned())],
        "{lines:?}"
    );
}

#[gpui_kit::test]
fn a_running_root_of_another_machine_with_a_question_shows_needs_you(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    // Its own repository on server, for a session started there.
    let repo = server.user.path("work/app");
    server.user.git_init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    server.user.git(&repo, &["add", "."]);
    server
        .user
        .git(&repo, &["commit", "--quiet", "-m", "first"]);
    // It runs, then asks the user and goes on working: an ask does not hold the turn.
    server.user.fake_script(json!({"turns": [
        {"when": "Export the totals", "do": [
            {"hold": "release"},
            {"mcp": {"tool": "ask", "arguments": {"question": "CSV or JSON first?"}}},
            {"wait": 60000},
            {"say": "done"}
        ]}
    ]}));
    let started = server
        .user
        .call(
            "agent.run",
            json!({"project": "app", "message": "Export the totals of the year"}),
        )
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();

    // Its row in the server's group of the tree, running, the window open before it asks.
    let (shell, cx) = window(&pc, cx);
    let row = elsewhere_id("server", &id);
    until(&shell, cx, "the running root of server", |cx| {
        label(cx, session_mark_id(&row)).as_deref() == Some("Running")
    });

    std::fs::write(server.user.path("fake/release"), "").unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        let agent = server
            .user
            .call("agent.show", json!({"agent": id}))
            .unwrap()["agent"]
            .clone();
        if agent["waiting_on_answer"] == true && agent["state"] == "working" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "it never asked while its turn ran: {agent}\n{}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // Needs you wins over running, as on its machine.
    let deadline = Instant::now() + WAIT;
    loop {
        step(&shell, Duration::from_millis(16), cx);
        let said = label(cx, session_mark_id(&row));
        if said.as_deref() == Some("Needs you") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the row says {said:?}, server lists it {:?}",
            listed_state(&server, &id)
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let mark = shell.read_with(cx, |s, _| {
        s.snapshot()
            .elsewhere(&row)
            .and_then(|session| session.mark)
    });
    assert_eq!(mark, Some(flottille_window::state::AgentState::NeedsYou));

    let agent = server
        .user
        .call("agent.show", json!({"agent": id}))
        .unwrap()["agent"]
        .clone();
    assert_eq!(agent["state"], "working", "its turn still runs");
}

#[gpui_kit::test]
fn a_led_session_of_another_machine_that_asks_the_user_needs_you_under_its_pilot(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    // It asks once its pilot's machine is off: the question goes to the user there.
    server.user.fake_script(json!({"turns": [
        {"when": "Finish the export of the report.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "ask", "arguments": {"question": "CSV or JSON first?"}}}
        ]}
    ]}));
    let (_pilot, mcp) = root(&pc, "auto");
    let agent = led_on_server(&mcp);
    eventually("the session runs on server", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .is_ok_and(|shown| shown["agent"]["state"] == "working")
    });
    let stopped = pc.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", pc.log());
    eventually("server sees pc offline", || {
        server.remote()["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "pc" && m["state"] != "online")
    });
    std::fs::write(server.user.path("fake/release"), "").unwrap();
    eventually("the session waits on the user of server", || {
        server
            .user
            .call("agent.tree", json!({"agent": agent}))
            .unwrap()["agents"][0]["waiting_on"]
            == "user"
    });
    // pc back, its link to server up again.
    pc.user.status();
    eventually("pc sees server online", || {
        pc.remote()["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "server" && m["state"] == "online")
    });

    // Under its pilot, its row says what server says of it: it needs the user there.
    let (shell, cx, row) = the_led_row(&pc, &agent, cx);
    let deadline = Instant::now() + WAIT;
    loop {
        step(&shell, Duration::from_millis(16), cx);
        let said = label(cx, session_mark_id(&row));
        if said.as_deref() == Some("Needs you on server") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the row says {said:?}, server lists it {:?}",
            listed_state(&server, &agent)
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        label(cx, session_machine_id(&row)).as_deref(),
        Some("server · needs you")
    );
}

#[gpui_kit::test]
fn a_child_launched_from_another_machine_is_not_a_root_in_the_window(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (pilot, mcp) = root(&pc, "auto");
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests of the export."}),
    );
    assert!(!answer.error, "{}", answer.text);
    let at = answer
        .text
        .find("(agent ")
        .unwrap_or_else(|| panic!("no agent in {}", answer.text));
    let child = answer.text[at + 7..at + 19].to_owned();
    eventually("the child runs on server", || {
        server
            .user
            .call("agent.show", json!({"agent": child}))
            .is_ok()
    });
    let (shell, cx) = window(&server, cx);

    open_page(&shell, &child, cx);

    // Its parent writes to it: a bar naming that agent stands where the composer would, with
    // nothing to go back to.
    until(&shell, cx, "the bar in place of the composer", |cx| {
        present(cx, "read-only".into())
    });
    let said = label(cx, "read-only".into()).unwrap_or_default();
    assert!(
        said.contains(&format!("a child of agent {pilot} of pc")),
        "{said}"
    );
    assert!(!present(cx, "composer-field".into()), "no composer");
    assert!(!present(cx, "back-to-root".into()));
}

/// The address of the one repository of `app`, on both machines: one project row on either window.
const APP: &str = "https://git.example/acme/app.git";

/// [`pc_and_server`], `app` over the same repository on both machines, each machine told the
/// other's: the sidebar of either window holds one row of `app`, with both machines' sessions.
fn pc_and_server_on_one_repository() -> (TestHub, Machine, Machine) {
    let (hub, pc, server) = pc_and_server();
    pc.user
        .git(&pc.user.path("work/app"), &["remote", "add", "origin", APP]);
    let repo = server.user.path("work/app");
    server.user.git_init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    server.user.git(&repo, &["add", "."]);
    server
        .user
        .git(&repo, &["commit", "--quiet", "-m", "first"]);
    server.user.git(&repo, &["remote", "add", "origin", APP]);
    register(&pc);
    register(&server);
    let told = |machine: &Machine| {
        machine.user.call("remote.sessions", json!({})).unwrap()["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|m| m["projects"].as_array().cloned().unwrap_or_default())
            .any(|p| p["name"] == "app" && p["remotes"] == json!(["git.example/acme/app"]))
    };
    eventually("each machine is told the other's app", || {
        told(&pc) && told(&server)
    });
    (hub, pc, server)
}

/// Another window, on `machine`'s daemon, in the application of the first one.
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

/// The ids of the rows of the project `app`, in the order the tree draws them.
fn rows_of_app(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.project("app")
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

/// Whether `row` is drawn right under `pilot` in `app`, tied to it.
fn right_under(shell: &Entity<Shell>, pilot: &str, row: &str, cx: &mut VisualTestContext) -> bool {
    let rows = rows_of_app(shell, cx);
    rows.iter()
        .position(|id| id == pilot)
        .is_some_and(|at| rows.get(at + 1).map(String::as_str) == Some(row))
        && label(cx, tie_id(session_id(row))).is_some()
}

#[gpui_kit::test]
fn a_led_session_of_another_machine_is_under_its_pilot(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server_on_one_repository();
    // In Full, its end is let through as a session's it leads on this machine would be.
    let (pilot, mcp) = root(&pc, "full");

    // On the machine it runs on: under the row of its pilot, which that machine's list holds.
    let agent = {
        let (shell, cx) = window(&server, cx);
        let pilot_row = elsewhere_id("pc", &pilot);
        until(&shell, cx, "the pilot's row on server", |cx| {
            present(cx, session_id(&pilot_row))
        });
        settle(&shell, cx);

        let agent = led_on_server(&mcp);
        until(&shell, cx, "its row comes", |cx| {
            frame_of(&shell, &agent, cx).is_some_and(|frame| !heights(cx, &frame).is_empty())
        });
        let frame = frame_of(&shell, &agent, cx).unwrap();
        let coming = heights(cx, &frame).into_iter().fold(f32::MAX, f32::min);
        until(&shell, cx, "the session under its pilot on server", |cx| {
            right_under(&shell, &pilot_row, &agent, cx)
        });
        settle(&shell, cx);
        let full = heights(cx, &frame).into_iter().fold(0., f32::max);
        assert!(
            coming < full,
            "the row unfolds from nothing: {coming} of {full}"
        );
        assert!(
            right_under(&shell, &pilot_row, &agent, cx),
            "{:?}",
            rows_of_app(&shell, cx)
        );
        // Its machine's mark, as it sits among another machine's rows.
        let said = label(cx, session_machine_id(&agent)).unwrap_or_default();
        assert!(said.starts_with("server"), "{said}");
        assert!(present(
            cx,
            gpui_kit::ElementId::Name(format!("session-machine-icon-{agent}").into())
        ));

        // It ends: its row folds away from under its pilot.
        let stopped = mcp.call("agent_stop", json!({"agent_id": agent}));
        assert!(!stopped.error, "{}", stopped.text);
        until(&shell, cx, "the row starts folding away", |cx| {
            heights(cx, &frame).into_iter().all(|height| height < full)
        });
        let going = heights(cx, &frame);
        assert!(
            going.iter().any(|height| *height > 0.),
            "the row folds away, it does not vanish: {going:?}"
        );
        settle(&shell, cx);
        assert!(!rows_of_app(&shell, cx).contains(&agent));
        agent
    };

    // On the pilot's machine, a led session of server that lives: under its pilot, with server's
    // mark.
    let other = led_on_server(&mcp);
    assert_ne!(other, agent);
    let (shell, cx) = open(&pc, cx);
    let row = elsewhere_id("server", &other);
    until(&shell, cx, "the session under its pilot on pc", |cx| {
        right_under(&shell, &pilot, &row, cx)
    });
    let said = label(cx, session_machine_id(&row)).unwrap_or_default();
    assert!(said.starts_with("server · "), "{said}");
    assert!(present(
        cx,
        gpui_kit::ElementId::Name(format!("session-machine-icon-{row}").into())
    ));
}

/// The notification `machine` keeps of a request held for its session `agent`: its number there,
/// once it is kept.
fn request_noted(machine: &Machine, agent: &str) -> u64 {
    let deadline = Instant::now() + WAIT;
    loop {
        let listed = machine
            .user
            .call("notifications.list", json!({"limit": 100}))
            .unwrap();
        let found = listed["notifications"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["kind"] == "request" && row["session"] == agent)
            .and_then(|row| row["id"].as_u64());
        if let Some(id) = found {
            return id;
        }
        assert!(
            Instant::now() < deadline,
            "the request was never noted: {listed}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether `machine` says its notification `id` was read.
fn noted_read(machine: &Machine, id: u64) -> bool {
    let listed = machine
        .user
        .call("notifications.list", json!({"limit": 100}))
        .unwrap();
    listed["notifications"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|row| row["id"] == id && !row["read_at"].is_null())
}

#[gpui_kit::test]
fn the_bell_holds_a_request_of_another_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (_pilot, mcp) = root(&pc, "ask");
    let agent = held_on_server(&mcp, &server);
    let id = request_noted(&server, &agent);

    // On pc's window, the bell holds what waits on server, said of server.
    let (shell, cx, row) = the_led_row(&pc, &agent, cx);
    shell.update(cx, |shell, cx| shell.toggle_bell(cx));
    let key = format!("n-{id}@server");
    until(&shell, cx, "the bell holds server's request", |cx| {
        shell.read_with(cx, |s, cx| s.bell_list().read(cx).order().contains(&key))
    });
    // Its row unfolds into the open panel, from nothing.
    let item = format!("bell-item-{key}");
    let coming = heights(cx, &item).into_iter().fold(f32::MAX, f32::min);
    settle(&shell, cx);
    let full = heights(cx, &item).into_iter().fold(0., f32::max);
    assert!(coming < full, "the row unfolds: {coming} of {full}");
    let titles = shell.read_with(cx, |s, cx| s.bell_sessions(cx));
    assert!(
        titles
            .iter()
            .any(|title| title == "Finish the export · on server"),
        "{titles:?}"
    );
    let counts = label(cx, gpui_kit::ElementId::Name("bell-counts".into())).unwrap_or_default();
    assert!(counts.contains("need you"), "{counts}");

    // Chosen, it opens server's page of its session, and it is read there.
    shell.update_in(cx, |shell, window, cx| shell.choose(&key, window, cx));
    until(&shell, cx, "server's page of the session", |cx| {
        shell.read_with(cx, |s, _| {
            s.machine_shown().as_deref() == Some("server")
                && s.selected() == Some(&Selection::Session(row.clone()))
        })
    });
    eventually("server says its notification was read", || {
        noted_read(&server, id)
    });
}

/// A session the user starts on `server` in `app`, its first turn ended, then ended: its id.
fn ended_on_server(server: &Machine) -> String {
    let repo = server.user.path("work/app");
    server.user.git_init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    server.user.git(&repo, &["add", "."]);
    server
        .user
        .git(&repo, &["commit", "--quiet", "-m", "first"]);
    let started = server
        .user
        .call(
            "agent.run",
            json!({"project": "app", "message": "Fix the export of the report"}),
        )
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let state = || {
        server
            .user
            .call("agent.show", json!({"agent": id}))
            .unwrap()["agent"]["state"]
            .clone()
    };
    eventually("its first turn on server", || state() == "idle");
    server
        .user
        .call("agent.stop", json!({"agent": id}))
        .unwrap();
    eventually("it ended on server", || state() == "ended");
    id
}

/// The ids of the rows of the "Ended" group.
fn ended_rows(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.ended()
            .map(|group| {
                group
                    .read(cx)
                    .sessions(cx)
                    .iter()
                    .map(|row| row.read(cx).id().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Opens the "Ended" group, or closes it.
fn toggle_ended(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let group = shell.read_with(cx, |s, _| s.ended().cloned()).unwrap();
    group.update(cx, |group, cx| group.toggle(cx));
}

#[gpui_kit::test]
fn an_ended_session_of_another_machine_is_in_ended(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let agent = ended_on_server(&server);
    let row = elsewhere_id("server", &agent);

    // Opened on pc, the "Ended" group asks server for its ended sessions: there with its title
    // and its machine.
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the Ended group", |cx| {
        shell.read_with(cx, |s, _| s.ended().is_some())
    });
    if !shell.read_with(cx, |s, cx| s.ended().unwrap().read(cx).is_open()) {
        toggle_ended(&shell, cx);
    }
    until(&shell, cx, "server's ended session is in Ended", |cx| {
        ended_rows(&shell, cx).contains(&row) && present(cx, session_id(&row))
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, session_id(&row)).as_deref(),
        Some("Fix the export of the report")
    );
    let scope = shell.read_with(cx, |s, cx| {
        s.session(&row)
            .and_then(|found| found.read(cx).scope().map(str::to_owned))
    });
    assert_eq!(scope.as_deref(), Some("app on server"));

    // Off, it says so once when the group is opened again, and its sessions are not listed.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the window sees server off", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .and_then(|read| read.machine("server"))
                .is_some_and(|server| !server.online())
        })
    });
    toggle_ended(&shell, cx);
    settle(&shell, cx);
    toggle_ended(&shell, cx);
    let unlisted = gpui_kit::ElementId::Name("ended-unlisted".into());
    until(&shell, cx, "the group says server is off", |cx| {
        label(cx, unlisted.clone()).is_some_and(|said| {
            said.starts_with("server is offline since ")
                && said.ends_with(": its ended sessions are not listed")
        })
    });
    settle(&shell, cx);
    assert!(!ended_rows(&shell, cx).contains(&row));
}

/// Stops `server`'s daemon and waits until `pc` and its window see it off.
fn server_off(pc: &Machine, server: &Machine, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(shell, cx, "the window sees server off", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .and_then(|read| read.machine("server"))
                .is_some_and(|server| !server.online())
        })
    });
}

#[gpui_kit::test]
fn an_off_machine_s_bell_is_not_asked_again_and_again(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the bell and Ended read the server", |cx| {
        shell.read_with(cx, |s, _| s.bell_reached("server"))
    });

    // Off, server is not dialled again and again while it stays off.
    server_off(&pc, &server, &shell, cx);
    settle(&shell, cx);
    let before = shell.read_with(cx, |s, _| s.bell_dials("server"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        step(&shell, Duration::from_millis(16), cx);
        std::thread::sleep(Duration::from_millis(50));
    }
    let after = shell.read_with(cx, |s, _| s.bell_dials("server"));
    assert!(
        after <= before + 1,
        "server was dialled {} times in 3 seconds off",
        after - before
    );
}

#[gpui_kit::test]
fn an_off_machine_s_notifications_leave_the_bell(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (_pilot, mcp) = root(&pc, "ask");
    let agent = held_on_server(&mcp, &server);
    let id = request_noted(&server, &agent);
    let (shell, cx, _row) = the_led_row(&pc, &agent, cx);
    shell.update(cx, |shell, cx| shell.toggle_bell(cx));
    let key = format!("n-{id}@server");
    until(&shell, cx, "the bell holds server's request", |cx| {
        shell.read_with(cx, |s, cx| s.bell_list().read(cx).order().contains(&key))
    });
    settle(&shell, cx);
    let counts = shell.read_with(cx, |s, cx| s.bell_list().read(cx).counts());

    // Off, its request leaves the bell, folding away, and is no longer counted.
    server_off(&pc, &server, &shell, cx);
    until(&shell, cx, "server's request leaves the bell", |cx| {
        !shell.read_with(cx, |s, cx| s.bell_list().read(cx).order().contains(&key))
    });
    let item = format!("bell-item-{key}");
    let going = heights(cx, &item);
    assert!(
        going.iter().any(|height| *height > 0.),
        "the row folds away, it does not vanish: {going:?}"
    );
    settle(&shell, cx);
    assert!(heights(cx, &item).is_empty(), "gone once folded");
    let now = shell.read_with(cx, |s, cx| s.bell_list().read(cx).counts());
    assert!(now.0 < counts.0, "{counts:?} then {now:?}");
}

#[gpui_kit::test]
fn opening_ended_twice_reads_each_machine_once(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    ended_on_server(&server);
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the bell and Ended read the server", |cx| {
        shell.read_with(cx, |s, _| s.bell_reached("server"))
    });
    settle(&shell, cx);
    let before = shell.read_with(cx, |s, _| s.ended_reads("server"));

    // Opened twice before server answers, "Ended" reads it once.
    shell.update(cx, |s, cx| {
        s.ended_opened(cx);
        s.ended_opened(cx);
    });
    settle(&shell, cx);
    let after = shell.read_with(cx, |s, _| s.ended_reads("server"));
    assert_eq!(after, before + 1, "reads of server's ended sessions");
}

#[gpui_kit::test]
fn an_ended_row_of_another_machine_opens_that_machine_s_page(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let agent = ended_on_server(&server);
    let row = elsewhere_id("server", &agent);
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the Ended group", |cx| {
        shell.read_with(cx, |s, _| s.ended().is_some())
    });
    if !shell.read_with(cx, |s, cx| s.ended().unwrap().read(cx).is_open()) {
        toggle_ended(&shell, cx);
    }
    until(&shell, cx, "server's ended session is in Ended", |cx| {
        ended_rows(&shell, cx).contains(&row) && present(cx, session_id(&row))
    });
    settle(&shell, cx);

    cx.update(|window, cx| window.click(session_id(&row), cx));
    until(&shell, cx, "server's page of the ended session", |cx| {
        shell.read_with(cx, |s, _| {
            s.machine_shown().as_deref() == Some("server")
                && s.selected() == Some(&Selection::Session(row.clone()))
        })
    });
}

#[gpui_kit::test]
fn mark_all_read_marks_read_on_the_other_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (_pilot, mcp) = root(&pc, "ask");
    let agent = held_on_server(&mcp, &server);
    let id = request_noted(&server, &agent);
    let (shell, cx, _row) = the_led_row(&pc, &agent, cx);
    shell.update(cx, |shell, cx| shell.toggle_bell(cx));
    let key = format!("n-{id}@server");
    until(&shell, cx, "the bell holds server's request", |cx| {
        shell.read_with(cx, |s, cx| s.bell_list().read(cx).order().contains(&key))
    });
    settle(&shell, cx);

    cx.update(|window, cx| window.click("bell-mark-all", cx));
    eventually("server says its notification was read", || {
        noted_read(&server, id)
    });
}
