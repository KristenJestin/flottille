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
use flottille_window::conversation::{Block, Ending};
use flottille_window::runs::CommandRow;
use flottille_window::session::SessionPage;
use flottille_window::tree::{project_commands_id, session_commands_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const SCRIPTS: [(&str, &str); 6] = [
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
    // Ends at once with a line of its own.
    ("blip.sh", "#!/bin/sh\necho blip\n"),
    // Sixty coloured lines: more than the output panel's column holds.
    (
        "many.sh",
        "#!/bin/sh\ni=0\nwhile [ $i -lt 60 ]; do printf '\\033[3%dmline %d\\033[0m \\033[1;3%dmbold\\033[0m\\n' $((i % 7 + 1)) $i $(((i + 1) % 7 + 1)); i=$((i + 1)); done\n",
    ),
];

const CATALOGUE: &str = "\n[commands.server]\nline = \"./server.sh\"\nserve = true\n\
    [commands.fails]\nline = \"./fails.sh\"\n\
    [commands.colours]\nline = \"./colours.sh\"\n\
    [commands.blip]\nline = \"./blip.sh\"\n\
    [commands.many]\nline = \"./many.sh\"\n";

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "show colours", "do": [
            {"mcp": {"tool": "run", "arguments": {"task": "colours"}}}]},
        {"when": "run fails", "do": [
            {"mcp": {"tool": "run", "arguments": {"task": "fails", "timeout": 1}}}]},
        {"when": "run slow", "do": [
            {"mcp": {"tool": "run", "arguments": {"task": "fails", "timeout": 1}}},
            {"say": "The slow check is running."}]},
        {"when": "run server", "do": [
            {"mcp": {"tool": "run", "arguments": {"task": "server"}}}]}
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

/// Starts a task and leaves the call after a second: its run's id.
fn start(mcp: &Mcp, command: &str) -> String {
    let text = ok(mcp.call("run", json!({"task": command, "timeout": 1})));
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
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
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

    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
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
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
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
fn a_run_in_the_conversation_is_one_compact_line_and_its_output_is_in_the_panel_with_its_colours_never_shown_raw(
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
    until(&shell, cx, "the line is in the conversation", |cx| {
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
    // The line says the command, how it ended and for how long; no output is drawn in the chat.
    assert_eq!(
        label(cx, &format!("command-line-{at}")).unwrap(),
        "./colours.sh"
    );
    assert_eq!(label(cx, &format!("command-exit-{at}")).unwrap(), "exit 1");
    cx.update(|window, _| {
        assert!(window.try_find(format!("command-seconds-{at}")).is_some());
        assert!(
            window.try_find(format!("command-line-{at}-0")).is_none(),
            "the output is not in the conversation"
        );
        let line = window.find(format!("command-card-{at}")).bounds();
        assert!(line.size.height < px(50.), "one compact line: {line:?}");
    });

    // A click opens the same panel the rail opens, and the panel draws the words, not the bytes.
    cx.update(|window, cx| window.click(format!("command-card-{at}"), cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    until(&shell, cx, "the panel shows the output", |cx| {
        panel.read_with(cx, |panel, _| {
            panel.run() == Some(card.run.as_str()) && panel.shown().is_some()
        })
    });
    settle(cx);
    assert_eq!(
        label(cx, "command-panel-line-0").unwrap(),
        "ok first",
        "the colours are the palette's"
    );
    assert_eq!(label(cx, "command-panel-line-1").unwrap(), "red failed");
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
    let held = mcp.call("run", json!({"line": script.display().to_string()}));
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
    assert!(
        cx.update(|window, _| window.try_find(format!("command-allowed-{at}")).is_some()),
        "the line says who allowed it"
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

fn blocks_of(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

fn ending_of_card(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<Ending> {
    match card_of(page, cx) {
        Some((_, Block::Command(card))) => Some(card.ending),
        _ => None,
    }
}

#[gpui_kit::test]
fn a_running_card_shows_its_end(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    user.call("agent.send", json!({"agent": lead, "message": "run fails"}))
        .unwrap();
    until(&shell, cx, "the card says the command runs", |cx| {
        ending_of_card(&page, cx) == Some(Ending::Running)
    });
    settle(cx);
    let (at, _) = card_of(&page, cx).unwrap();
    assert_eq!(label(cx, &format!("command-exit-{at}")).unwrap(), "running");

    // The command ends by itself, two seconds in, after its call answered.
    until(&shell, cx, "the card shows how it ended", |cx| {
        ending_of_card(&page, cx) == Some(Ending::Exited(3))
    });
    settle(cx);

    let Some((at, Block::Command(card))) = card_of(&page, cx) else {
        unreachable!()
    };
    assert_eq!(label(cx, &format!("command-exit-{at}")).unwrap(), "exit 3");
    assert!(card.seconds.is_some_and(|s| s >= 1.5), "{:?}", card.seconds);
    assert_eq!(card.lines, ["one"], "the final tail of its output");
}

#[gpui_kit::test]
fn a_command_ended_delivery_is_never_shown_as_text_when_its_card_is_on_screen(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, _mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    user.call("agent.send", json!({"agent": lead, "message": "run fails"}))
        .unwrap();
    until(&shell, cx, "the card shows how it ended", |cx| {
        ending_of_card(&page, cx) == Some(Ending::Exited(3))
    });
    settle(cx);

    let blocks = blocks_of(&page, cx);
    assert!(
        blocks.iter().all(|block| !matches!(
            block,
            Block::Notice { kind, text }
                if kind.contains("command") || text.contains("run_status")
                    || text.contains("Your command")
        )),
        "no text, no line: the card says it. {blocks:?}"
    );
    assert!(
        cx.update(|window, _| window.try_find("notice".to_owned()).is_none()),
        "nothing drawn as a notice"
    );
}

#[gpui_kit::test]
fn a_command_ended_whose_card_is_not_on_screen_is_one_quiet_line(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    // Started by a call the conversation shows no card for.
    let _run = start(&mcp, "fails");
    until(&shell, cx, "the line is in the conversation", |cx| {
        blocks_of(&page, cx)
            .iter()
            .any(|block| matches!(block, Block::Notice { kind, .. } if kind == "command"))
    });
    settle(cx);

    let lines: Vec<String> = blocks_of(&page, cx)
        .iter()
        .filter_map(|block| match block {
            Block::Notice { kind, text } if kind == "command" => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("`./fails.sh` ended \u{b7} exit 3 \u{b7} 2."),
        "{}",
        lines[0]
    );
    assert!(
        blocks_of(&page, cx).iter().all(|block| !matches!(
            block,
            Block::Notice { text, .. } if text.contains("run_status")
        )),
        "not the delivery's own words"
    );
}

#[gpui_kit::test]
fn a_card_whose_command_the_agent_stopped_itself_shows_it_stopped(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "run server"}),
    )
    .unwrap();
    until(&shell, cx, "the card says the server runs", |cx| {
        ending_of_card(&page, cx) == Some(Ending::Running)
    });
    let Some((_, Block::Command(card))) = card_of(&page, cx) else {
        unreachable!()
    };

    // No delivery tells the agent its own stop: the list of commands does.
    ok(mcp.call("run_status", json!({"run": card.run, "action": "stop"})));
    until(&shell, cx, "the card shows the stop", |cx| {
        ending_of_card(&page, cx) == Some(Ending::Stopped)
    });
    settle(cx);

    let (at, _) = card_of(&page, cx).unwrap();
    assert_eq!(label(cx, &format!("command-exit-{at}")).unwrap(), "stopped");
}

#[gpui_kit::test]
fn the_old_approval_line_is_dropped_when_the_card_follows(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    // The call is held: the request has its row, until its result comes as a card.
    let script = user.path("shared/colours.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, SCRIPTS[2].1).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let held = mcp.call("run", json!({"line": script.display().to_string()}));
    let number: u64 = held
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", held.text));
    until(&shell, cx, "the request has its row", |cx| {
        blocks_of(&page, cx).contains(&Block::Approval { number })
    });

    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    until(&shell, cx, "the card is in the conversation", |cx| {
        card_of(&page, cx).is_some()
    });
    settle(cx);

    let blocks = blocks_of(&page, cx);
    assert!(blocks.contains(&Block::Answered { number }), "{blocks:?}");
    assert!(!blocks.contains(&Block::Approval { number }), "{blocks:?}");
    assert!(
        cx.update(|window, _| window
            .try_find(format!("approval-record-{number}"))
            .is_none()),
        "no line 'Allowed' above the card"
    );
    let (at, _) = card_of(&page, cx).unwrap();
    assert!(
        cx.update(|window, _| window.try_find(format!("command-allowed-{at}")).is_some()),
        "the card says who allowed it"
    );
}

#[gpui_kit::test]
fn two_agent_messages_with_a_hidden_delivery_between_stay_two_blocks(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    // The agent says the check runs; the command ends by itself and its delivery, hidden by
    // the card, wakes the agent, which says "ok".
    user.call("agent.send", json!({"agent": lead, "message": "run slow"}))
        .unwrap();
    until(&shell, cx, "the agent answered the delivery", |cx| {
        ending_of_card(&page, cx) == Some(Ending::Exited(3)) && texts_of(&page, cx).len() == 3
    });
    settle(cx);

    // The first "ok" answers the session's first message.
    assert_eq!(
        texts_of(&page, cx),
        ["ok", "The slow check is running.", "ok"],
        "{:?}",
        blocks_of(&page, cx)
    );
}

fn texts_of(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<String> {
    blocks_of(page, cx)
        .into_iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(text),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn a_running_line_follows_the_rail_s_run_its_time_and_the_end_of_its_output_in_its_model(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    user.call(
        "agent.send",
        json!({"agent": lead, "message": "run server"}),
    )
    .unwrap();
    until(&shell, cx, "the card says the server runs", |cx| {
        ending_of_card(&page, cx) == Some(Ending::Running)
    });

    // The card follows the rail's live row: its time moves on, and its lines are the server's.
    until(&shell, cx, "the card's time and lines move on", |cx| {
        matches!(card_of(&page, cx), Some((_, Block::Command(card)))
            if card.seconds.is_some_and(|s| s >= 3.0)
                && card.lines.last().is_some_and(|l| l.starts_with("tick")))
    });
    settle(cx);

    let Some((at, Block::Command(card))) = card_of(&page, cx) else {
        unreachable!()
    };
    assert_eq!(card.ending, Ending::Running);
    assert_eq!(label(cx, &format!("command-exit-{at}")).unwrap(), "running");
    cx.update(|window, _| {
        assert!(window.try_find(format!("command-seconds-{at}")).is_some());
    });
    ok(mcp.call("run_status", json!({"run": card.run, "action": "stop"})));
}

fn ids_of(rows: &[&CommandRow]) -> Vec<String> {
    rows.iter().map(|row| row.run.clone()).collect()
}

fn present(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

/// The runs in sight and the ones behind "N ended", as the section sorts them.
fn arranged(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> (Vec<String>, Vec<String>) {
    page.read_with(cx, |page, cx| {
        let list = page.commands(cx);
        let list = list.read(cx);
        let arranged = list.arranged();
        (ids_of(&arranged.kept), ids_of(&arranged.folded))
    })
}

#[gpui_kit::test]
fn the_rail_section_is_named_runs_and_only_the_last_three_ended_runs_are_in_sight_the_rest_behind_n_ended(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let runs: Vec<String> = (0..5).map(|_| start(&mcp, "blip")).collect();
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the five runs", |cx| {
        commands(&page, cx).len() == 5
    });
    settle(cx);

    assert_eq!(
        label(cx, "rail-runs-title").as_deref(),
        Some("Runs"),
        "not 'Commands', which is the project's /name instructions"
    );
    let (kept, folded) = arranged(&page, cx);
    assert_eq!((kept.len(), folded.len()), (3, 2), "{kept:?} {folded:?}");
    for run in &runs {
        assert_eq!(
            present(cx, format!("command-{run}")),
            kept.contains(run),
            "{run}: in sight only if one of the last three"
        );
    }
    assert_eq!(label(cx, "rail-runs-ended").as_deref(), Some("2 ended"));
    cx.update(|window, _| assert_eq!(window.find("rail-runs-ended").expanded(), Some(false)));

    // The folded row opens the older ones, and folds them again.
    cx.update(|window, cx| window.click("rail-runs-ended", cx));
    settle(cx);
    for run in &runs {
        assert!(present(cx, format!("command-{run}")), "{run} after opening");
    }
    cx.update(|window, _| assert_eq!(window.find("rail-runs-ended").expanded(), Some(true)));

    // Dismiss stays on each row, the ones behind the fold too.
    cx.update(|window, cx| window.click(format!("command-dismiss-{}", folded[0]), cx));
    until(&shell, cx, "the run is dismissed", |cx| {
        commands(&page, cx).len() == 4
    });
    settle(cx);
    assert_eq!(label(cx, "rail-runs-ended").as_deref(), Some("1 ended"));

    cx.update(|window, cx| window.click("rail-runs-ended", cx));
    settle(cx);
    assert!(!present(cx, format!("command-{}", folded[1])));
    assert!(present(cx, format!("command-{}", kept[0])));
}

#[gpui_kit::test]
fn an_ended_run_older_than_the_session_s_last_turn_folds_away_on_its_own(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "blip");
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the run", |cx| {
        commands(&page, cx).len() == 1
    });
    settle(cx);
    assert!(present(cx, format!("command-{run}")), "ended in this turn");
    assert!(!present(cx, "rail-runs-ended"));

    // A new turn begins: what ended before it goes behind "N ended".
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "hello again"}),
    )
    .unwrap();
    until(&shell, cx, "the run folds behind its row", |cx| {
        present(cx, "rail-runs-ended")
    });
    settle(cx);
    assert_eq!(label(cx, "rail-runs-ended").as_deref(), Some("1 ended"));
    assert!(!present(cx, format!("command-{run}")));
    assert_eq!(commands(&page, cx).len(), 1, "kept in the history");
}

#[gpui_kit::test]
fn a_run_in_the_conversation_opens_the_panel_the_rail_opens(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "run server"}),
    )
    .unwrap();
    until(&shell, cx, "the line says the server runs", |cx| {
        ending_of_card(&page, cx) == Some(Ending::Running)
    });
    settle(cx);
    let (at, Block::Command(card)) = card_of(&page, cx).unwrap() else {
        unreachable!()
    };
    cx.update(|window, cx| window.click(format!("command-card-{at}"), cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    until(&shell, cx, "the panel follows the run", |cx| {
        panel.read_with(cx, |panel, _| {
            panel.run() == Some(card.run.as_str())
                && panel.shown().is_some_and(|shown| !shown.lines.is_empty())
        })
    });
    ok(mcp.call("run_status", json!({"run": card.run, "action": "stop"})));
}

#[gpui_kit::test]
fn the_output_panel_draws_every_line_of_a_long_coloured_output_on_a_line_of_its_own(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "many");
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the run", |cx| {
        !commands(&page, cx).is_empty()
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("command-{run}"), cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    until(&shell, cx, "the panel shows the sixty lines", |cx| {
        panel.read_with(cx, |panel, _| {
            panel.shown().is_some_and(|shown| shown.lines.len() == 60)
        })
    });
    // The panel has opened and its output has come: a line is drawn once the panel is at rest.
    settle(cx);

    let lines: Vec<(f32, f32)> = (0..60)
        .map(|i| {
            cx.update(|window, _| {
                let bounds = window.find(format!("command-panel-line-{i}")).bounds();
                (f32::from(bounds.origin.y), f32::from(bounds.size.height))
            })
        })
        .collect();
    for (i, pair) in lines.windows(2).enumerate() {
        let ((top, height), (next, _)) = (pair[0], pair[1]);
        assert!(height >= 12., "line {i} is squeezed to {height}px");
        assert!(
            next >= top + height - 0.5,
            "line {} starts at {next}, over line {i} which ends at {}",
            i + 1,
            top + height
        );
    }
    assert_eq!(label(cx, "command-panel-line-0").unwrap(), "line 0 bold");
}
