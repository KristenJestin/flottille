//! The scenarios of the commands in the window (#126), on GPUI's test platform in front of a real
//! daemon whose agents are the fake ACP agent: the rail's Commands, the tree's icon, the output
//! panel, and the card a command's result is in the conversation. No window opens on a screen.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::commands::CommandRow;
use flottille_window::conversation::{Block, Ending};
use flottille_window::session::SessionPage;
use flottille_window::tree::{project_commands_id, session_commands_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const SCRIPTS: [(&str, &str); 4] = [
    // A server: prints a line a second.
    (
        "server.sh",
        "#!/bin/sh\necho listening on 8080\ni=0\nwhile true; do sleep 1; i=$((i+1)); echo tick $i; done\n",
    ),
    // Ends by itself, with a code.
    ("fails.sh", "#!/bin/sh\necho one\nsleep 2\nexit 3\n"),
    // Colours, and a failure.
    (
        "colours.sh",
        "#!/bin/sh\nprintf '\\033[32mok\\033[0m first\\n\\033[1;31mred\\033[0m failed\\n'\nexit 1\n",
    ),
    ("quiet.sh", "#!/bin/sh\ntrue\n"),
];

const CATALOGUE: &str = "\n[commands.server]\nline = \"./server.sh\"\nserve = true\n\
    [commands.fails]\nline = \"./fails.sh\"\n\
    [commands.colours]\nline = \"./colours.sh\"\n";

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "show colours", "do": [
            {"mcp": {"tool": "commands_run", "arguments": {"command": "colours"}}}]}
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

/// The project `app`, its ready workspace `w` with the scripts, and a lead the user started
/// there, its first turn ended: the lead's id, its MCP session and the workspace's folder.
fn lead(user: &User) -> (String, Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(CATALOGUE);
    std::fs::write(&file, text).unwrap();
    user.call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let workspace = PathBuf::from(
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["folder"]
            .as_str()
            .unwrap(),
    );
    for (name, script) in SCRIPTS {
        let path = workspace.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "lead the work"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("the lead's first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    (id, Mcp::initialize(&url, &authorization), workspace)
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

/// Starts a catalogue command and leaves the call after a second: its run's id.
fn start(mcp: &Mcp, command: &str) -> String {
    let text = ok(mcp.call("commands_run", json!({"command": command, "timeout": 1})));
    text.strip_prefix("run ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("no run in {text}"))
        .to_owned()
}

fn window<'a>(
    user: &User,
    width: f32,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(width), px(800.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the links sent, then draws what changed.
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
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn open_session<'a>(
    user: &User,
    lead: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, 1280., cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(
        &shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            session_page(&shell, cx).read_with(cx, |page, cx| {
                !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
            })
        },
    );
    (shell, cx)
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn commands(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<CommandRow> {
    page.read_with(cx, |page, cx| page.commands(cx).read(cx).rows().to_vec())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

#[gpui_kit::test]
fn the_rail_lists_a_running_command_with_its_line_who_started_it_and_its_last_line(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "server");
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    until(&shell, cx, "the rail lists the command", |cx| {
        !commands(&page, cx).is_empty()
    });
    settle(cx);

    let rows = commands(&page, cx);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].run, run);
    assert!(rows[0].running());
    assert_eq!(
        label(cx, &format!("command-line-{run}")).unwrap(),
        "./server.sh"
    );
    assert_eq!(
        label(cx, &format!("command-last-{run}")).unwrap(),
        "listening on 8080"
    );
    let who = label(cx, &format!("command-who-{run}")).unwrap();
    assert!(who.starts_with("task server · "), "{who}");
    assert!(
        cx.update(|window, _| window.try_find(format!("command-stop-{run}")).is_some()),
        "a running command has its Stop"
    );
    // The last line moves on by itself: the program writes, nobody tells the window.
    until(&shell, cx, "the last line moves on", |cx| {
        label(cx, &format!("command-last-{run}")).is_some_and(|line| line.starts_with("tick"))
    });
    ok(mcp.call("commands_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn stop_stops_the_command_and_it_stays_listed_greyed_with_its_end_until_dismissed(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "server");
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the command", |cx| {
        !commands(&page, cx).is_empty()
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("command-stop-{run}"), cx));
    until(&shell, cx, "the command is stopped", |cx| {
        commands(&page, cx)
            .first()
            .is_some_and(|row| !row.running())
    });
    settle(cx);

    let rows = commands(&page, cx);
    assert_eq!(rows[0].state, "stopped");
    assert_eq!(
        label(cx, &format!("command-mark-{run}")).unwrap(),
        "stopped"
    );
    assert!(
        cx.update(|window, _| window.try_find(format!("command-stop-{run}")).is_none()),
        "an ended command has no Stop"
    );

    cx.update(|window, cx| window.click(format!("command-dismiss-{run}"), cx));
    until(&shell, cx, "the command is dismissed", |cx| {
        commands(&page, cx).is_empty()
    });
}

#[gpui_kit::test]
fn a_command_that_ended_by_itself_shows_its_exit_code(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "fails");
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    until(&shell, cx, "the command ended", |cx| {
        commands(&page, cx)
            .first()
            .is_some_and(|row| !row.running())
    });
    settle(cx);

    assert_eq!(commands(&page, cx)[0].exit_code, Some(3));
    assert_eq!(
        label(cx, &format!("command-status-{run}")).unwrap(),
        "exit 3"
    );
}

#[gpui_kit::test]
fn the_tree_shows_an_icon_on_the_project_and_the_session_while_a_command_runs_and_loses_it(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let has = |cx: &mut VisualTestContext, id: gpui_kit::ElementId| {
        cx.update(|window, _| window.try_find(id).is_some())
    };
    settle(cx);
    assert!(!has(cx, project_commands_id("app")));
    assert!(!has(cx, session_commands_id(&lead)));

    let run = start(&mcp, "server");
    until(&shell, cx, "the icons appear", |cx| {
        has(cx, project_commands_id("app")) && has(cx, session_commands_id(&lead))
    });
    assert_eq!(
        label(cx, &format!("commands-session-{lead}")).unwrap(),
        "A command is running"
    );

    ok(mcp.call("commands_status", json!({"run": run, "action": "stop"})));
    until(&shell, cx, "the icons go", |cx| {
        !has(cx, project_commands_id("app")) && !has(cx, session_commands_id(&lead))
    });
}

#[gpui_kit::test]
fn clicking_a_command_opens_its_output_and_follows_it_live(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "server");
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the command", |cx| {
        !commands(&page, cx).is_empty()
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("command-{run}"), cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    until(&shell, cx, "the panel shows the output", |cx| {
        panel.read_with(cx, |panel, _| {
            panel.shown().is_some_and(|shown| {
                shown
                    .lines
                    .first()
                    .is_some_and(|l| l == "listening on 8080")
            })
        })
    });
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.run().map(str::to_owned)),
        Some(run.clone())
    );
    until(&shell, cx, "a line is added while it is open", |cx| {
        panel.read_with(cx, |panel, _| {
            panel
                .shown()
                .is_some_and(|shown| shown.lines.iter().any(|l| l.starts_with("tick")))
        })
    });
    settle(cx);
    assert_eq!(
        label(cx, "command-panel-line-0").unwrap(),
        "listening on 8080"
    );

    cx.simulate_keystrokes("escape");
    until(&shell, cx, "the panel closes", |cx| {
        panel.read_with(cx, |panel, _| panel.run().is_none())
    });
    ok(mcp.call("commands_status", json!({"run": run, "action": "stop"})));
}

fn card_of(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<(usize, Block)> {
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .enumerate()
            .find(|(_, block)| matches!(block, Block::Command(_)))
            .map(|(at, block)| (at, block.clone()))
    })
}

#[gpui_kit::test]
fn a_command_s_result_is_a_card_with_its_colours_turned_into_the_palette_s_never_shown_raw(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, _mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    user.call(
        "agent.send",
        json!({"agent": lead, "message": "show colours"}),
    )
    .unwrap();
    until(&shell, cx, "the card is in the conversation", |cx| {
        card_of(&page, cx).is_some()
    });
    settle(cx);

    let (at, Block::Command(card)) = card_of(&page, cx).unwrap() else {
        unreachable!()
    };
    assert_eq!(card.line, "./colours.sh");
    assert_eq!(card.ending, Ending::Exited(1));
    assert_eq!(card.allowed_by, None);
    assert!(
        card.lines[0].contains('\u{1b}'),
        "the model keeps the bytes"
    );
    // What is drawn is the words, and the exit code in its own element.
    assert_eq!(
        label(cx, &format!("command-line-{at}-0")).unwrap(),
        "ok first"
    );
    assert_eq!(
        label(cx, &format!("command-line-{at}-1")).unwrap(),
        "red failed"
    );
    assert_eq!(label(cx, &format!("command-exit-{at}")).unwrap(), "exit 1");
    assert_eq!(
        label(cx, &format!("command-line-{at}")).unwrap(),
        "./colours.sh"
    );
}

#[gpui_kit::test]
fn a_command_that_came_back_through_an_approval_is_the_same_card_with_who_allowed_it_and_never_the_raw_text(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    // A script outside the workspace: the gate holds the call for the user.
    let script = user.path("shared/colours.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, SCRIPTS[2].1).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let held = mcp.call(
        "commands_run",
        json!({"line": script.display().to_string()}),
    );
    let number: u64 = held
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", held.text));
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    until(&shell, cx, "the card is in the conversation", |cx| {
        card_of(&page, cx).is_some()
    });
    settle(cx);

    let (at, Block::Command(card)) = card_of(&page, cx).unwrap() else {
        unreachable!()
    };
    assert_eq!(card.ending, Ending::Exited(1));
    assert_eq!(card.allowed_by.as_deref(), Some("you"));
    assert_eq!(
        label(cx, &format!("command-line-{at}-1")).unwrap(),
        "red failed"
    );
    assert!(
        cx.update(|window, _| window.try_find(format!("command-allowed-{at}")).is_some()),
        "the card says who allowed it"
    );
    let blocks = page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    });
    assert!(
        blocks.iter().all(|block| !matches!(
            block,
            Block::Notice { text, .. } if text.contains("Request #") || text.contains("Its result")
        )),
        "{blocks:?}"
    );
}
