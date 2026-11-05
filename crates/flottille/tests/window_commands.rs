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
use flottille_ui::Icon;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::actions::{RunAct, RunAnswer};
use flottille_window::conversation::{Block, Ending};
use flottille_window::runs::CommandRow;
use flottille_window::session::SessionPage;
use flottille_window::tree::{project_commands_id, session_commands_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const SCRIPTS: [(&str, &str); 8] = [
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
    // A server that prints its first line, then ticks only once the test has created `release`:
    // what its last line is stays the test's to say, however slow the machine is.
    (
        "listener.sh",
        "#!/bin/sh\necho listening on 8080\nwhile [ ! -e release ]; do sleep 0.1; done\ni=0\nwhile true; do sleep 1; i=$((i+1)); echo tick $i; done\n",
    ),
    // A server that says it is ready once the test has created `go`.
    (
        "booting.sh",
        "#!/bin/sh\necho booting\nwhile [ ! -e go ]; do sleep 0.1; done\necho 'ready in 12 ms'\nexec sleep 300\n",
    ),
];

const CATALOGUE: &str = "\n[commands.server]\nline = \"./server.sh\"\nserve = true\n\
    [commands.listener]\nline = \"./listener.sh\"\nserve = true\n\
    [commands.fails]\nline = \"./fails.sh\"\n\
    [commands.colours]\nline = \"./colours.sh\"\n\
    [commands.blip]\nline = \"./blip.sh\"\n\
    [commands.many]\nline = \"./many.sh\"\n\
    [tasks.web]\nline = \"./booting.sh\"\nserve = true\nready = { line = \"ready in\" }\n\
    url = \"https://{workspace}.app.localhost/login\"\n";

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
            {"mcp": {"tool": "run", "arguments": {"task": "server"}}}]},
        {"when": "launch the builder", "do": [
            {"mcp": {"tool": "agent_launch",
                "arguments": {"title": "builder", "brief": "build quietly"}}}]},
        {"when": "build quietly", "do": [{"say": "Building."}]}
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
    lead_saying(user, "lead the work")
}

/// `lead`, whose first message is `message`.
fn lead_saying(user: &User, message: &str) -> (String, Mcp, PathBuf) {
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
            json!({"project": "app", "workspace": "w", "message": message}),
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
    open_session_in(&shell, lead, cx);
    (shell, cx)
}

/// Opens the lead's session in a window already there.
fn open_session_in(shell: &Entity<Shell>, lead: &str, cx: &mut VisualTestContext) {
    let shell = shell.clone();
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
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn commands(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<CommandRow> {
    page.read_with(cx, |page, cx| page.commands(cx).read(cx).rows().to_vec())
}

/// Unfolds the `N ended` row of the rail's Runs: the runs that ended are behind it.
fn unfold_ended(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("rail-runs-ended", cx));
    settle(cx);
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
    let (lead, mcp, workspace) = lead(&user);
    let run = start(&mcp, "listener");
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    until(&shell, cx, "the rail lists the command", |cx| {
        !commands(&page, cx).is_empty()
    });
    until(&shell, cx, "the rail shows its first line", |cx| {
        label(cx, &format!("command-last-{run}")).is_some_and(|line| line == "listening on 8080")
    });
    settle(cx);

    let rows = commands(&page, cx);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].run, run);
    assert!(rows[0].running());
    assert_eq!(
        label(cx, &format!("command-line-{run}")).unwrap(),
        "./listener.sh"
    );
    assert_eq!(
        label(cx, &format!("command-last-{run}")).unwrap(),
        "listening on 8080"
    );
    let who = label(cx, &format!("command-who-{run}")).unwrap();
    assert_eq!(who, "task listener", "the root does not name itself");
    assert!(
        cx.update(|window, _| window.try_find(format!("command-stop-{run}")).is_some()),
        "a running command has its Stop"
    );
    assert!(
        cx.update(|window, _| window
            .within(format!("command-stop-{run}"))
            .try_find(Icon::Stop.glyph_id())
            .is_some()),
        "the run row's Stop draws the filled square"
    );
    // The last line moves on by itself: the program writes, nobody tells the window.
    std::fs::write(workspace.join("release"), "").unwrap();
    until(&shell, cx, "the last line moves on", |cx| {
        label(cx, &format!("command-last-{run}")).is_some_and(|line| line.starts_with("tick"))
    });
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn a_command_row_of_the_root_agent_does_not_say_the_conversation(cx: &mut TestAppContext) {
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

    let who = label(cx, &format!("command-who-{run}")).unwrap_or_default();
    assert!(!who.contains("the conversation"), "{who}");
}

#[gpui_kit::test]
fn stop_stops_the_command_and_it_stays_listed_greyed_with_its_end(cx: &mut TestAppContext) {
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
    unfold_ended(cx);

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
    unfold_ended(cx);

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

/// Unfolds the group that holds the row `at`, when it is folded: its rows are drawn.
fn unfold(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) {
    let groups = page.read_with(cx, |page, cx| page.conversation().read(cx).groups());
    let Some(group) = groups.into_iter().find(|group| group.contains(&at)) else {
        return;
    };
    let line = page
        .read_with(cx, |page, cx| page.conversation().read(cx).group_id(&group))
        .expect("a group's line");
    if cx.update(|window, _| window.find(line.clone()).expanded()) == Some(false) {
        cx.update(|window, cx| window.click(line, cx));
        settle(cx);
    }
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
    unfold(&page, at, cx);
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
    unfold(&page, at, cx);
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
    unfold(&page, at, cx);
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
    unfold(&page, at, cx);
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
    unfold(&page, at, cx);
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
    until(&shell, cx, "the run ended and the rail lists it", |cx| {
        commands(&page, cx)
            .first()
            .is_some_and(|row| !row.running())
    });
    settle(cx);
    unfold_ended(cx);

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

// The command seen large, and restarted (#348).

/// Opens the session and the panel of a running server: the run, and what drives the window.
fn panel_of_server<'a>(
    user: &User,
    lead: &str,
    mcp: &Mcp,
    cx: &'a mut TestAppContext,
) -> (
    String,
    Entity<Shell>,
    Entity<SessionPage>,
    &'a mut VisualTestContext,
) {
    let run = start(mcp, "server");
    let (shell, cx) = open_session(user, lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the command", |cx| {
        !commands(&page, cx).is_empty()
    });
    settle(cx);
    cx.update(|window, cx| window.click(format!("command-{run}"), cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    until(&shell, cx, "the panel shows the output", |cx| {
        panel.read_with(cx, |panel, _| {
            panel.shown().is_some_and(|shown| !shown.lines.is_empty())
        })
    });
    settle(cx);
    (run, shell, page, cx)
}

fn modal_is_open(cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| window.try_find("run-modal").is_some())
}

#[gpui_kit::test]
fn expand_shows_the_command_in_a_modal_over_the_window(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    assert!(!modal_is_open(cx), "the panel alone at first");

    cx.update(|window, cx| window.click("run-drawer-expand", cx));
    settle(cx);

    assert!(modal_is_open(cx));
    let (modal, viewport) =
        cx.update(|window, _| (window.find("run-modal").bounds(), window.viewport_size()));
    assert!(modal.size.width > viewport.width * 0.9, "{modal:?}");
    assert!(modal.size.height > viewport.height * 0.9, "{modal:?}");
    cx.update(|window, _| {
        assert!(
            window.try_find("run-modal-scrim").is_some(),
            "the rest is darkened"
        );
    });
    assert_eq!(label(cx, "run-modal-title").unwrap(), "./server.sh");
    assert_eq!(
        label(cx, "command-modal-line-0").unwrap(),
        "listening on 8080"
    );
    assert!(
        label(cx, "run-modal-fact-by").is_some(),
        "the facts are in it"
    );
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.run().map(str::to_owned)),
        Some(run.clone()),
        "the panel is still there, under it"
    );
    drop(shell);
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn a_click_outside_the_command_modal_closes_it(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, _, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    cx.update(|window, cx| window.click("run-drawer-expand", cx));
    settle(cx);
    assert!(modal_is_open(cx));

    cx.update(|window, cx| window.click_at("run-modal-scrim", gpui_kit::point(px(3.), px(3.)), cx));
    settle(cx);

    assert!(!modal_is_open(cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.run().map(str::to_owned)),
        Some(run.clone()),
        "the panel is as it was"
    );
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn escape_closes_the_command_modal(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, _, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    cx.update(|window, cx| window.click("run-drawer-expand", cx));
    settle(cx);
    assert!(modal_is_open(cx));

    cx.simulate_keystrokes("escape");
    settle(cx);

    assert!(!modal_is_open(cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.run().map(str::to_owned)),
        Some(run.clone()),
        "Escape closed the modal, not the panel under it"
    );
    // The next Escape is the panel's.
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.run().map(str::to_owned)),
        None
    );
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn the_command_modal_follows_new_output_and_stops_the_command(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    cx.update(|window, cx| window.click("run-drawer-expand", cx));
    settle(cx);

    until(&shell, cx, "a line arrives while the modal is open", |cx| {
        label(cx, "command-modal-line-1").is_some_and(|line| line.starts_with("tick"))
    });
    settle(cx);
    assert_eq!(label(cx, "run-modal-stop").as_deref(), Some("Stop"));

    cx.update(|window, cx| window.click("run-modal-stop", cx));
    until(&shell, cx, "the command is stopped", |cx| {
        commands(&page, cx)
            .iter()
            .any(|row| row.run == run && row.state == "stopped")
    });
    settle(cx);
    assert!(modal_is_open(cx), "stopping leaves the modal where it is");
    assert!(
        cx.update(|window, _| window.try_find("run-modal-stop").is_none()),
        "and there is nothing left to stop"
    );
}

#[gpui_kit::test]
fn started_by_is_cut_to_two_lines_in_the_panel(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead_saying(
        &user,
        "Make the whole dev server of the API start in watch mode, read every route line it \
         prints, tell me which controllers are missing a guard, keep going until all of them \
         are covered and the docs list each one, then summarise what changed for the team",
    );
    let (run, _, _, cx) = panel_of_server(&user, &lead, &mcp, cx);

    let by = label(cx, "run-fact-by").unwrap();
    assert!(
        by.chars().count() > 120,
        "a long first message, in full: {by}"
    );
    let (line, cut) = cx.update(|window, _| {
        (
            f32::from(window.find("run-fact-where").bounds().size.height),
            f32::from(window.find("run-fact-by").bounds().size.height),
        )
    });
    assert!(
        cut <= line * 2. + 1.,
        "{cut}px is more than two lines of {line}px"
    );
    assert!(cut > line + 1., "and it did take its second line: {cut}px");
    // The modal shows it whole.
    cx.update(|window, cx| window.click("run-drawer-expand", cx));
    settle(cx);
    let (whole, one_line) = cx.update(|window, _| {
        (
            f32::from(window.find("run-modal-fact-by").bounds().size.height),
            f32::from(window.find("run-modal-fact-where").bounds().size.height),
        )
    });
    assert_eq!(label(cx, "run-modal-fact-by").unwrap(), by);
    assert!(
        whole <= one_line * 2. + 1.,
        "the facts are on one or two lines: {whole}px"
    );
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn the_panel_shows_the_new_run_after_restart(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    assert_eq!(label(cx, "run-drawer-restart").as_deref(), Some("Restart"));

    cx.update(|window, cx| window.click("run-drawer-restart", cx));

    until(&shell, cx, "the panel follows the new run", |cx| {
        panel.read_with(cx, |panel, _| {
            panel.run().is_some_and(|shown| shown != run)
                && panel
                    .shown()
                    .is_some_and(|s| s.row.run != run && !s.lines.is_empty())
        })
    });
    let new = panel.read_with(cx, |panel, _| panel.run().unwrap().to_owned());
    until(
        &shell,
        cx,
        "the old one stopped and the new one runs",
        |cx| {
            let rows = commands(&page, cx);
            rows.iter().any(|r| r.run == run && r.state == "stopped")
                && rows.iter().any(|r| r.run == new && r.state == "running")
        },
    );
    ok(mcp.call("run_status", json!({"run": new, "action": "stop"})));
}

#[gpui_kit::test]
fn the_modal_shows_the_new_run_after_restart(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    cx.update(|window, cx| window.click("run-drawer-expand", cx));
    settle(cx);

    cx.update(|window, cx| window.click("run-modal-restart", cx));

    until(&shell, cx, "the modal follows the new run", |cx| {
        panel.read_with(cx, |panel, _| {
            panel.run().is_some_and(|shown| shown != run)
                && panel
                    .shown()
                    .is_some_and(|s| s.row.run != run && !s.lines.is_empty())
        })
    });
    settle(cx);
    assert!(modal_is_open(cx), "the modal stays open on the new run");
    assert_eq!(
        label(cx, "command-modal-line-0").unwrap(),
        "listening on 8080"
    );
    let new = panel.read_with(cx, |panel, _| panel.run().unwrap().to_owned());
    ok(mcp.call("run_status", json!({"run": new, "action": "stop"})));
}

#[gpui_kit::test]
fn restarting_from_the_modal_keeps_it_open_without_a_gap(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    cx.update(|window, cx| window.click("run-drawer-expand", cx));
    settle(cx);

    cx.update(|window, cx| window.click("run-modal-restart", cx));

    // Every frame between the click and the new run: the modal is drawn, with the command's line
    // and its output, and the drawer never goes back to "Reading…".
    until(&shell, cx, "the modal shows the new run", |cx| {
        assert!(modal_is_open(cx), "the modal vanished");
        assert!(label(cx, "run-modal-title").is_some(), "the modal is empty");
        assert!(
            label(cx, "run-drawer-line").is_some(),
            "the drawer went back to Reading…"
        );
        panel.read_with(cx, |panel, _| {
            panel
                .shown()
                .is_some_and(|s| s.row.run != run && !s.lines.is_empty())
        })
    });
    settle(cx);
    assert!(modal_is_open(cx));
    let new = panel.read_with(cx, |panel, _| panel.run().unwrap().to_owned());
    ok(mcp.call("run_status", json!({"run": new, "action": "stop"})));
}

#[gpui_kit::test]
fn a_double_restart_starts_one_run(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());

    cx.update(|window, cx| window.click("run-drawer-restart", cx));
    cx.update(|window, cx| window.click("run-drawer-restart", cx));

    until(&shell, cx, "the panel follows the new run", |cx| {
        panel.read_with(cx, |panel, _| {
            panel.shown().is_some_and(|s| s.row.run != run)
        })
    });
    // A second run would show in the list a moment after.
    let deadline = Instant::now() + Duration::from_millis(2500);
    while Instant::now() < deadline {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(50));
    }
    let servers = commands(&page, cx)
        .iter()
        .filter(|row| row.line == "./server.sh")
        .count();
    assert_eq!(servers, 2, "the first run and one new run");
    let new = panel.read_with(cx, |panel, _| panel.run().unwrap().to_owned());
    ok(mcp.call("run_status", json!({"run": new, "action": "stop"})));
}

#[gpui_kit::test]
fn a_refused_restart_is_said_once_by_a_toast(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    // The task is gone from the project: the daemon cannot start it again.
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        text.replace(
            "[commands.server]\nline = \"./server.sh\"\nserve = true\n",
            "",
        ),
    )
    .unwrap();

    cx.update(|window, cx| window.click("run-drawer-restart", cx));

    until(&shell, cx, "a toast says it was not restarted", |cx| {
        toasts(&shell, cx) == ["The command was not restarted"]
    });
    settle(cx);
    assert_eq!(
        page.read_with(cx, |page, cx| page
            .commands(cx)
            .read(cx)
            .error()
            .map(str::to_owned)),
        None,
        "not also a failed state in the list"
    );
    assert!(
        !panel.read_with(cx, |panel, _| panel.restarting()),
        "Restart and Stop are enabled again"
    );
    // The command was left running, and Restart is back.
    let rows = commands(&page, cx);
    assert!(rows.iter().any(|r| r.run == run && r.state == "running"));
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

/// The titles of the toasts shown.
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
fn the_refusal_of_another_run_keeps_a_restart_pending(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, _shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    panel.update(cx, |panel, cx| panel.restart_asked(&run, cx));
    let refused = |act, of: &str| RunAnswer {
        act,
        run: of.to_owned(),
        refused: Some("no".into()),
        restarted: None,
        notice: None,
    };

    page.update(cx, |page, cx| {
        page.command_answered(refused(RunAct::Stop, "another"), cx)
    });
    page.update(cx, |page, cx| {
        page.command_answered(refused(RunAct::Restart, "another"), cx)
    });
    assert!(
        panel.read_with(cx, |panel, _| panel.restarting()),
        "the restart of this run still waits"
    );

    page.update(cx, |page, cx| {
        page.command_answered(refused(RunAct::Restart, &run), cx)
    });
    assert!(!panel.read_with(cx, |panel, _| panel.restarting()));
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn restarting_an_ended_agent_s_command_says_it_is_now_the_user_s(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, page, cx) = panel_of_server(&user, &lead, &mcp, cx);
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    user.call("agent.stop", json!({"agent": lead})).unwrap();

    cx.update(|window, cx| window.click("run-drawer-restart", cx));

    until(&shell, cx, "a toast says the command is the user's", |cx| {
        toasts(&shell, cx) == ["The command is now yours"]
    });
    until(&shell, cx, "the panel follows the new run", |cx| {
        panel.read_with(cx, |panel, _| panel.run().is_some_and(|new| new != run))
    });
    let new = panel.read_with(cx, |panel, _| panel.run().unwrap().to_owned());
    user.call("run.stop", json!({"run": new})).unwrap();
}

// A server that says when it is ready and where it answers (#389).

#[gpui_kit::test]
fn a_starting_server_shows_its_spinner_then_a_live_dot_once_ready(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, workspace) = lead(&user);
    let run = start(&mcp, "web");
    let (shell, cx) = open_session(&user, &lead, cx);
    let who = format!("command-who-{run}");
    until(&shell, cx, "the row says it is starting", |cx| {
        label(cx, &who).is_some_and(|who| who == "task web · starting")
    });
    settle(cx);

    assert_eq!(
        label(cx, &format!("command-mark-{run}")).unwrap(),
        "starting"
    );
    assert!(
        cx.update(|window, _| window.try_find(format!("command-dot-{run}")).is_none()),
        "no dot while it starts"
    );
    std::fs::write(workspace.join("go"), "").unwrap();
    until(&shell, cx, "the row says it is ready", |cx| {
        label(cx, &who).is_some_and(|who| who == "task web · ready")
    });
    settle(cx);

    assert_eq!(label(cx, &format!("command-mark-{run}")).unwrap(), "ready");
    assert!(
        cx.update(|window, _| window.try_find(format!("command-dot-{run}")).is_some()),
        "the live dot"
    );
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn a_server_s_address_replaces_its_last_line_without_its_scheme(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "web");
    let (shell, cx) = open_session(&user, &lead, cx);
    let link = format!("command-link-{run}");
    until(&shell, cx, "the row shows the address", |cx| {
        label(cx, &link).is_some()
    });
    settle(cx);

    assert_eq!(label(cx, &link).unwrap(), "w.app.localhost/login");
    assert!(
        label(cx, &format!("command-last-{run}")).is_none(),
        "the address takes the last line's place"
    );
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn clicking_a_server_s_address_opens_it_and_not_its_output(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "web");
    let (shell, cx) = window(&user, 1280., cx);
    let opened: Arc<std::sync::Mutex<Vec<PathBuf>>> = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &std::path::Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    open_session_in(&shell, &lead, cx);
    let page = session_page(&shell, cx);
    let link = format!("command-link-{run}");
    until(&shell, cx, "the row shows the address", |cx| {
        label(cx, &link).is_some()
    });
    settle(cx);

    cx.update(|window, cx| window.click(link.clone(), cx));
    settle(cx);

    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://w.app.localhost/login")]
    );
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.run().map(str::to_owned)),
        None
    );
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn the_drawer_of_a_ready_server_says_ready_and_links_its_address(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, workspace) = lead(&user);
    let run = start(&mcp, "web");
    std::fs::write(workspace.join("go"), "").unwrap();
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the row says it is ready", |cx| {
        label(cx, &format!("command-who-{run}")).is_some_and(|who| who == "task web · ready")
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("command-{run}"), cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    until(&shell, cx, "the drawer is open", |cx| {
        panel.read_with(cx, |panel, _| panel.shown().is_some())
    });
    settle(cx);

    assert!(
        label(cx, "run-drawer-sub")
            .unwrap()
            .starts_with("Task web · ready · "),
        "{:?}",
        label(cx, "run-drawer-sub")
    );
    assert_eq!(label(cx, "run-drawer-mark").unwrap(), "ready");
    assert_eq!(
        label(cx, "run-drawer-link").unwrap(),
        "w.app.localhost/login"
    );
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[gpui_kit::test]
fn a_server_the_user_ran_on_the_project_page_shows_its_dot_and_its_address(
    cx: &mut TestAppContext,
) {
    let user = user();
    lead(&user);
    let folder = user.path("work/app");
    let script = folder.join("booting.sh");
    std::fs::write(&script, SCRIPTS[7].1).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(folder.join("go"), "").unwrap();
    let run = user
        .call("task.run", json!({"project": "app", "name": "web"}))
        .unwrap()["run"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, cx) = window(&user, 1280., cx);
    // The runs come last on the page: tall enough to hold them.
    cx.simulate_resize(size(px(1280.), px(2400.)));
    let opened: Arc<std::sync::Mutex<Vec<PathBuf>>> = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &std::path::Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(
            flottille_window::shell::Selection::Project("app".into()),
            window,
            cx,
        )
    });
    let link = format!("project-run-link-{run}");
    until(&shell, cx, "the project page shows the run ready", |cx| {
        label(cx, &format!("project-run-mark-{run}")).as_deref() == Some("ready")
    });
    settle(cx);

    assert_eq!(label(cx, &link).unwrap(), "main.app.localhost/login");
    assert!(
        cx.update(|window, _| window.try_find(format!("project-run-dot-{run}")).is_some()),
        "the live dot"
    );
    cx.update(|window, cx| window.click(link.clone(), cx));
    settle(cx);
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://main.app.localhost/login")]
    );
    user.call("run.stop", json!({"run": run})).unwrap();
}

/// The bounds of an element, if it is drawn.
fn bounds_of(cx: &mut VisualTestContext, id: &str) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).map(|found| found.bounds()))
}

#[gpui_kit::test]
fn a_server_that_ends_leaves_the_rail_s_running_runs_for_the_fold_and_shows_its_last_line(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let run = start(&mcp, "web");
    let (shell, cx) = open_session(&user, &lead, cx);
    let link = format!("command-link-{run}");
    let last = format!("command-last-{run}");
    until(&shell, cx, "the row shows the address", |cx| {
        label(cx, &link).is_some()
    });
    settle(cx);

    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
    until(&shell, cx, "the run is gone from the running ones", |cx| {
        has(cx, "rail-runs-ended")
    });
    settle(cx);
    assert!(
        !has(cx, &format!("command-{run}")),
        "it is behind `N ended`, not among the running"
    );

    unfold_ended(cx);
    assert!(label(cx, &last).is_some(), "its row shows its last line");
    assert!(label(cx, &link).is_none(), "and no address: it ended");
}

/// Opens the drawer of a running `web`: the run, and what drives the window.
fn drawer_of_web<'a>(
    user: &User,
    lead: &str,
    mcp: &Mcp,
    cx: &'a mut TestAppContext,
) -> (String, Entity<Shell>, &'a mut VisualTestContext) {
    let run = start(mcp, "web");
    let (shell, cx) = open_session(user, lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the row shows the address", |cx| {
        label(cx, &format!("command-link-{run}")).is_some()
    });
    settle(cx);
    cx.update(|window, cx| window.click(format!("command-{run}"), cx));
    let panel = page.read_with(cx, |page, _| page.command_panel().clone());
    until(&shell, cx, "the drawer is open", |cx| {
        panel.read_with(cx, |panel, _| panel.shown().is_some())
    });
    settle(cx);
    (run, shell, cx)
}

#[gpui_kit::test]
fn the_drawer_s_address_folds_away_when_the_server_ends(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, cx) = drawer_of_web(&user, &lead, &mcp, cx);
    assert!(label(cx, "run-drawer-link").is_some());

    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
    until(&shell, cx, "the drawer says it ended", |cx| {
        label(cx, "run-drawer-sub").is_some_and(|sub| sub.contains(" after "))
    });

    assert!(
        label(cx, "run-drawer-link").is_some(),
        "the address is still drawn while it folds away"
    );
    settle(cx);
    assert!(
        label(cx, "run-drawer-link").is_none(),
        "and gone once folded"
    );
}

#[gpui_kit::test]
fn the_modal_s_address_folds_away_when_the_server_ends(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp, _) = lead(&user);
    let (run, shell, cx) = drawer_of_web(&user, &lead, &mcp, cx);
    cx.update(|window, cx| window.click("run-drawer-expand", cx));
    settle(cx);
    assert!(label(cx, "run-modal-link").is_some());

    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
    until(&shell, cx, "the drawer says it ended", |cx| {
        label(cx, "run-drawer-sub").is_some_and(|sub| sub.contains(" after "))
    });

    assert!(
        label(cx, "run-modal-link").is_some(),
        "the address is still drawn while it folds away"
    );
    settle(cx);
    assert!(
        label(cx, "run-modal-link").is_none(),
        "and gone once folded"
    );
}

// The rail's Runs section shows the task runs of the agent shown (#395). The runs are put in the
// list by hand once the page is open, and the window is not polled again: what the section does
// with them is what is looked at, not the daemon's timing.

fn has(cx: &mut VisualTestContext, id: &str) -> bool {
    bounds_of(cx, id).is_some()
}

/// A run of `agent`, of the task `task` (none: a one-off line), still running or ended.
fn run_of(run: &str, agent: &str, task: Option<&str>, running: bool, at: u64) -> CommandRow {
    CommandRow {
        run: run.to_owned(),
        agent: agent.to_owned(),
        name: task.map(str::to_owned),
        workspace: None,
        line: task.map_or_else(|| format!("git diff {run}"), |task| format!("./{task}.sh")),
        state: if running { "running" } else { "exited" }.to_owned(),
        exit_code: (!running).then_some(0),
        signal: None,
        started_at: 1_000 + at,
        ended_at: (!running).then_some(2_000 + at),
        last_line: String::new(),
        tail: Vec::new(),
        step: false,
        ready: None,
        url: None,
    }
}

/// The lead's session open, with a child of it: the lead's id, the child's, and the window.
fn with_a_child<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (String, String, Entity<Shell>, &'a mut VisualTestContext) {
    let (lead, ..) = lead_saying(user, "launch the builder");
    let (shell, cx) = open_session(user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the builder is in the tree", |cx| {
        page.read_with(cx, |page, _| page.tree().len() == 2)
    });
    let child = page.read_with(cx, |page, _| {
        page.tree()
            .iter()
            .find(|agent| agent.parent.is_some())
            .map(|agent| agent.id.clone())
            .unwrap()
    });
    // Its row is drawn, and the tree's first run list is in: nothing polls after this.
    until(&shell, cx, "the builder's row", |cx| {
        has(cx, &format!("rail-row-{child}"))
    });
    (lead, child, shell, cx)
}

/// Gives the rail's list `rows` and lets everything that moves settle.
fn list_runs(shell: &Entity<Shell>, rows: Vec<CommandRow>, cx: &mut VisualTestContext) {
    let page = session_page(shell, cx);
    let list = page.read_with(cx, |page, cx| page.commands(cx));
    list.update(cx, |list, cx| list.set(rows, cx));
    settle(cx);
}

#[gpui_kit::test]
fn the_runs_section_lists_task_runs_only(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _, shell, cx) = with_a_child(&user, cx);
    list_runs(
        &shell,
        vec![
            run_of("r-task", &lead, Some("server"), true, 1),
            run_of("r-line", &lead, None, true, 2),
            run_of("r-old-line", &lead, None, false, 3),
        ],
        cx,
    );

    assert!(has(cx, "command-r-task"), "the task's run is listed");
    assert!(
        !has(cx, "command-r-line"),
        "a one-off line that still runs is not"
    );
    assert!(
        !has(cx, "command-r-old-line") && !has(cx, "rail-runs-ended"),
        "nor is one that ended, and it does not count under `N ended`"
    );
}

#[gpui_kit::test]
fn no_task_run_folds_the_runs_section_away(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _, shell, cx) = with_a_child(&user, cx);
    list_runs(
        &shell,
        vec![run_of("r-task", &lead, Some("server"), true, 1)],
        cx,
    );
    assert!(has(cx, "rail-runs"), "a task run: the section is drawn");

    list_runs(&shell, vec![run_of("r-line", &lead, None, true, 2)], cx);
    assert!(
        !has(cx, "rail-runs"),
        "a one-off line alone does not keep the section"
    );

    list_runs(&shell, vec![], cx);
    assert!(!has(cx, "rail-runs"));
}

#[gpui_kit::test]
fn the_runs_section_shows_only_the_root_s_runs(cx: &mut TestAppContext) {
    let user = user();
    let (lead, child, shell, cx) = with_a_child(&user, cx);
    list_runs(
        &shell,
        vec![
            run_of("r-root", &lead, Some("server"), true, 1),
            run_of("r-child", &child, Some("listener"), true, 2),
        ],
        cx,
    );

    assert!(has(cx, "command-r-root"));
    assert!(
        !has(cx, "command-r-child"),
        "a child's run is read where the child is"
    );
}

#[gpui_kit::test]
fn opening_a_child_shows_its_runs(cx: &mut TestAppContext) {
    let user = user();
    let (lead, child, shell, cx) = with_a_child(&user, cx);
    list_runs(
        &shell,
        vec![
            run_of("r-root", &lead, Some("server"), true, 1),
            run_of("r-child", &child, Some("listener"), true, 2),
        ],
        cx,
    );

    cx.update(|window, cx| window.click(format!("rail-row-{child}"), cx));
    settle(cx);

    assert!(has(cx, "command-r-child"));
    assert!(
        !has(cx, "command-r-root"),
        "the root's run is no longer listed"
    );
}

#[gpui_kit::test]
fn ended_runs_fold_under_n_ended(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _, shell, cx) = with_a_child(&user, cx);
    list_runs(
        &shell,
        vec![
            run_of("r-up", &lead, Some("server"), true, 1),
            run_of("r-old", &lead, Some("fails"), false, 2),
            run_of("r-new", &lead, Some("blip"), false, 3),
        ],
        cx,
    );

    assert_eq!(label(cx, "rail-runs-ended").as_deref(), Some("2 ended"));
    assert!(has(cx, "command-r-up"), "a running one is in sight");
    assert!(
        !has(cx, "command-r-old") && !has(cx, "command-r-new"),
        "the ended ones are folded"
    );

    cx.update(|window, cx| window.click("rail-runs-ended", cx));
    settle(cx);
    let (old, new) = (
        bounds_of(cx, "command-r-old"),
        bounds_of(cx, "command-r-new"),
    );
    assert!(old.is_some() && new.is_some(), "a click unfolds them");
    assert!(
        new.unwrap().origin.y < old.unwrap().origin.y,
        "the newest first"
    );

    cx.update(|window, cx| window.click("rail-runs-ended", cx));
    settle(cx);
    assert!(!has(cx, "command-r-old"), "and a click folds them again");
}

#[gpui_kit::test]
fn a_run_that_ends_slides_into_the_fold(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _, shell, cx) = with_a_child(&user, cx);
    list_runs(
        &shell,
        vec![
            run_of("r-up", &lead, Some("server"), true, 1),
            run_of("r-end", &lead, Some("fails"), true, 2),
        ],
        cx,
    );
    assert!(has(cx, "command-r-end") && !has(cx, "rail-runs-ended"));

    list_runs(
        &shell,
        vec![
            run_of("r-up", &lead, Some("server"), true, 1),
            run_of("r-end", &lead, Some("fails"), false, 2),
        ],
        cx,
    );
    assert_eq!(label(cx, "rail-runs-ended").as_deref(), Some("1 ended"));
    assert!(
        !has(cx, "command-r-end") && has(cx, "command-r-up"),
        "the run that ended left the running ones for the fold"
    );
}
