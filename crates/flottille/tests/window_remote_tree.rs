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
use flottille_window::shell::{Selection, bell_group_title_id};
use flottille_window::tree::{session_id, session_machine_id};
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

/// The height of `frame`, `None` once it is gone.
fn height(cx: &mut VisualTestContext, frame: &str) -> Option<f32> {
    cx.update(|window, _| {
        window
            .try_find(frame.to_owned())
            .map(|found| f32::from(found.bounds().size.height))
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
    // Its machine's name and its state as last known.
    let said = label(cx, session_machine_id(&row)).unwrap_or_default();
    assert!(said.starts_with("server · "), "{said}");
    let state = said.trim_start_matches("server · ");
    assert!(
        ["running", "waiting", "reported"].contains(&state),
        "{said}"
    );
}

#[gpui_kit::test]
fn opening_it_switches_to_that_machine(cx: &mut TestAppContext) {
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

    until(
        &shell,
        cx,
        "the window shows server on that session",
        |cx| {
            shell.read_with(cx, |s, cx| {
                s.machine_shown().as_deref() == Some("server")
                    && s.main()
                        .read(cx)
                        .session_page()
                        .is_some_and(|page| page.read(cx).id() == agent)
            })
        },
    );
    // The page arrived by the switch: its header and its composer enter, one after the other.
    for section in ["0", "2"] {
        let id = gpui_kit::ElementId::Name(format!("session-section-{agent}-{section}").into());
        assert!(present(cx, id), "section {section} enters");
    }
    // Its own row now: the session as server lists it.
    until(&shell, cx, "its row on server", |cx| {
        present(cx, session_id(&agent))
    });
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
        frame_of(&shell, &row, cx).is_some_and(|frame| height(cx, &frame).is_some())
    });
    let frame = frame_of(&shell, &row, cx).unwrap();
    let coming = height(cx, &frame).unwrap();
    settle(&shell, cx);
    let full = height(cx, &frame).unwrap();
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
        height(cx, &frame).is_none_or(|height| height < full)
    });
    let going = height(cx, &frame);
    assert!(
        going.is_some_and(|height| height > 0.),
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
fn the_inbox_and_question_cards_name_the_other_machine(cx: &mut TestAppContext) {
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

    shell.update(cx, |s, cx| s.choose_machine(Some("server".to_owned()), cx));
    until(&shell, cx, "server's question in the inbox", |cx| {
        shell.read_with(cx, |s, _| {
            s.machine_shown().as_deref() == Some("server")
                && s.snapshot().bell.iter().any(|item| {
                    item.session.as_deref() == Some(id.as_str())
                        && item.key.starts_with("question-")
                })
        })
    });
    cx.update(|window, cx| window.click("bell", cx));
    settle(&shell, cx);
    // The panel groups a session's rows under its title, which says the machine.
    let said = label(cx, bell_group_title_id(&id)).unwrap_or_default();
    assert!(said.ends_with(" · on server"), "{said:?}");
    cx.update(|window, cx| window.click("bell", cx));
    settle(&shell, cx);

    let page = open_page(&shell, &id, cx);
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
