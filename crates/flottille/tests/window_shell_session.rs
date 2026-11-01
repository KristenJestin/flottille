//! The scenarios of #165 (the shell and the session page redone with the rules of the window) and
//! of #159 (sessions started by agents, and their messages), on GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent, with a throwaway user. No window opens on a
//! screen.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::runs::KEPT_ENDED;
use flottille_window::session::SessionPage;
use flottille_window::tree::{ended_id, place_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const SCRIPTS: [(&str, &str); 2] = [
    // A server: prints a line a second.
    (
        "server.sh",
        "#!/bin/sh\necho listening on 8080\ni=0\nwhile true; do sleep 1; i=$((i+1)); echo tick $i; done\n",
    ),
    // Ends at once with a line of its own.
    ("blip.sh", "#!/bin/sh\necho blip\n"),
];

const CATALOGUE: &str = "\n[commands.server]\nline = \"./server.sh\"\nserve = true\n\
    [commands.blip]\nline = \"./blip.sh\"\n";

/// The lead's context, as its first turn reports it: 41%.
const USED: u64 = 82_000;
const SIZE: u64 = 200_000;

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "lead the work", "do": [
            {"usage": {"used": USED, "size": SIZE}}, {"say": "On it."}]},
        {"when": "run server", "do": [
            {"mcp": {"tool": "run", "arguments": {"task": "server"}}}]},
        {"when": "start the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "root": true,
                "workspace": "w"}}}]},
        {"when": "write to the session", "do": [{"mcp": {"tool": "session_send",
            "arguments": {"session": "{prompt:agent}", "text": "The tests pass now."}}}]}
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

fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path(&format!("remotes/{name}.git"));
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
    folder
}

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Starts a session in `main` of `project` and waits until its first turn ended.
fn start(user: &User, project: &str, message: &str) -> String {
    let answer = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || state_of(user, &id) == "idle");
    id
}

/// The project `app`, its ready workspace `w` with the scripts and a `.env`, and a lead the user
/// started there, its first turn ended: the lead's id and its MCP session.
fn lead(user: &User) -> (String, Mcp) {
    project(user, "app");
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
    std::fs::write(workspace.join(".env"), "TOKEN=made-up-made-up\n").unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "lead the work"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("the lead's first turn", || state_of(user, &id) == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

/// Starts a task as the lead and leaves the call after a second: its run's id.
fn run(mcp: &Mcp, task: &str) -> String {
    let text = ok(mcp.call("run", json!({"task": task, "timeout": 1})));
    text.strip_prefix("run ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("no run in {text}"))
        .to_owned()
}

/// The number of the request a held call answered with.
fn held_number(answer: Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    let rest = answer
        .text
        .split_once("request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
        .1
        .to_owned();
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
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
    let connect: Connect = Arc::new(user.connector());
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
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn rect(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    let id = id.into();
    cx.update(|window, _| window.find(id).bounds())
}

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The labels said inside the element `within`.
fn said_within(within: &str, cx: &mut VisualTestContext) -> Vec<String> {
    let within = within.to_owned();
    cx.update(|window, _| {
        window
            .within(within)
            .elements()
            .into_iter()
            // A state mark names its state for the accessibility tree: it is a mark, not words.
            .filter(|element| !format!("{:?}", element.path()).contains("state-icon-"))
            .filter_map(|element| element.label().map(str::to_owned))
            .collect()
    })
}

/// The element paths inside `within`, as text.
fn paths_within(within: &str, cx: &mut VisualTestContext) -> Vec<String> {
    let within = within.to_owned();
    cx.update(|window, _| {
        window
            .within(within)
            .elements()
            .into_iter()
            .map(|element| format!("{:?}", element.path()))
            .collect()
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn open_session<'a>(
    user: &User,
    agent: &str,
    width: f32,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, width, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(agent), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
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
    settle(cx);
    (shell, cx)
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

/// Holds a read of `.env` as the lead, and waits for its card in the rail: its number.
fn hold(shell: &Entity<Shell>, mcp: &Mcp, cx: &mut VisualTestContext) -> u64 {
    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    until(shell, cx, "the card is in the rail", |cx| {
        has(format!("allow-{number}"), cx)
    });
    settle(cx);
    number
}

// ── The sidebar ──────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn ended_is_pinned_at_the_bottom_of_the_sidebar_whatever_the_tree_s_length(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let old = start(&user, "app", "an old question");
    let live = start(&user, "app", "a live question");
    user.call("agent.stop", json!({"agent": old})).unwrap();
    let (shell, cx) = window(&user, 1440., cx);
    until(&shell, cx, "the tree and Ended", |cx| {
        has(session_id(&live), cx) && has(ended_id(), cx)
    });
    settle(cx);

    let sidebar = rect("sidebar", cx);
    let ended = rect(ended_id(), cx);
    let last = rect(session_id(&live), cx);
    assert!(
        sidebar.bottom() - ended.bottom() < px(16.),
        "Ended sits at the bottom of the sidebar: {ended:?} in {sidebar:?}"
    );
    assert!(
        ended.origin.y - last.bottom() > px(200.),
        "not right under the last row of a short tree: {last:?} {ended:?}"
    );
}

#[gpui_kit::test]
fn ended_unfolds_upward_over_the_tree_and_folds_back(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let old = start(&user, "app", "an old question");
    start(&user, "app", "a live question");
    user.call("agent.stop", json!({"agent": old})).unwrap();
    let (shell, cx) = window(&user, 1440., cx);
    until(&shell, cx, "Ended", |cx| has(ended_id(), cx));
    settle(cx);
    let sidebar = rect("sidebar", cx);
    let folded = rect(ended_id(), cx);

    cx.update(|window, cx| window.click(ended_id(), cx));
    settle(cx);
    let open = rect(ended_id(), cx);
    let row = rect(session_id(&old), cx);
    assert!(
        open.origin.y < folded.origin.y - px(20.),
        "its row rose to make room: {folded:?} → {open:?}"
    );
    assert!(
        row.origin.y > open.origin.y && sidebar.bottom() - row.bottom() < px(24.),
        "its sessions unfold under it, down to the bottom: {row:?}"
    );

    cx.update(|window, cx| window.click(ended_id(), cx));
    settle(cx);
    assert_eq!(
        rect(ended_id(), cx).origin.y,
        folded.origin.y,
        "folded back"
    );
}

#[gpui_kit::test]
fn the_tree_rows_keep_their_state_marks_only_and_no_text_badge(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    hold(&shell, &mcp, cx);
    let badges: Vec<String> = paths_within("tree", cx)
        .into_iter()
        .filter(|path| path.contains("pill-"))
        .collect();
    assert!(badges.is_empty(), "no pill in the tree: {badges:?}");
    let said: Vec<String> = said_within("tree", cx)
        .into_iter()
        .filter(|label| label.to_lowercase().contains("needs you"))
        .collect();
    assert!(said.is_empty(), "no text says the state: {said:?}");
}

// ── The session's header, its end and its conversation ──────────────────────────────────────

#[gpui_kit::test]
fn the_session_header_says_its_title_and_place_and_no_state(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    hold(&shell, &mcp, cx);
    let pills: Vec<String> = paths_within("session-header", cx)
        .into_iter()
        .filter(|path| path.contains("pill-"))
        .collect();
    assert!(pills.is_empty(), "no state pill in the header: {pills:?}");
    let said = said_within("session-header", cx).join(" | ").to_lowercase();
    assert!(!said.contains("needs you"), "{said}");
    assert_eq!(label("session-title", cx).as_deref(), Some("lead the work"));
    assert_eq!(label("session-place", cx).as_deref(), Some("w"));
}

#[gpui_kit::test]
fn an_ended_session_says_so_in_the_bar_that_replaces_the_composer_and_not_in_its_header(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "a question");
    let (shell, cx) = open_session(&user, &id, 1440., cx);
    user.call("agent.stop", json!({"agent": id})).unwrap();
    until(&shell, cx, "the bar says it ended", |cx| {
        has("session-ended", cx)
    });
    settle(cx);
    let said = said_within("session-header", cx).join(" | ").to_lowercase();
    assert!(
        !said.contains("stopped") && !said.contains("failed") && !said.contains("ended"),
        "the header says no state: {said}"
    );
    let pills: Vec<String> = paths_within("session-header", cx)
        .into_iter()
        .filter(|path| path.contains("pill-"))
        .collect();
    assert!(pills.is_empty(), "{pills:?}");
    let bar = label("session-ended", cx)
        .unwrap_or_default()
        .to_lowercase();
    assert!(bar.contains("ended"), "the bar says it: {bar}");
}

#[gpui_kit::test]
fn a_request_leaves_a_neutral_line_where_it_happened_that_shows_its_card_in_the_rail(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    let number = hold(&shell, &mcp, cx);

    let line = label(format!("approval-line-{number}"), cx).unwrap();
    assert_eq!(line, format!("Request #{number} · Read .env"));
    let rail = rect("session-rail", cx);
    assert!(
        rect(format!("approval-line-{number}"), cx).origin.x < rail.origin.x,
        "the line is in the conversation"
    );
    let waiting: Vec<String> = said_within("session-page", cx)
        .into_iter()
        .filter(|label| label.contains("Waiting for you"))
        .collect();
    assert_eq!(
        waiting,
        vec!["Waiting for you".to_owned()],
        "no amber line in the conversation says it"
    );

    cx.update(|window, cx| window.click(format!("approval-line-{number}"), cx));
    let waiting = session_page(&shell, cx).read_with(cx, |p, cx| p.waiting(cx));
    assert_eq!(
        waiting.read_with(cx, |w, _| w.pointed()),
        Some(number),
        "its card is lit in the rail"
    );
}

#[gpui_kit::test]
fn needs_you_is_said_once_on_the_session_page_by_the_waiting_section_s_title(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    hold(&shell, &mcp, cx);
    let said: Vec<String> = said_within("session-page", cx)
        .into_iter()
        .filter(|label| {
            let label = label.to_lowercase();
            label.contains("needs you") || label.contains("waiting for you")
        })
        .collect();
    assert_eq!(said, vec!["Waiting for you".to_owned()]);
}

// ── The rail ─────────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn the_rail_reads_agents_workspace_runs_then_waiting_for_you_at_the_very_bottom(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let server = run(&mcp, "server");
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    until(&shell, cx, "the rail lists the run", |cx| {
        has(format!("command-{server}"), cx)
    });
    hold(&shell, &mcp, cx);

    let rail = rect("session-rail", cx);
    let agents = rect("rail-agents", cx);
    let workspace = rect("rail-workspace", cx);
    let runs = rect("rail-runs", cx);
    let waiting = rect("rail-waiting", cx);
    assert!(
        agents.origin.y < workspace.origin.y
            && workspace.origin.y < runs.origin.y
            && runs.origin.y < waiting.origin.y,
        "Agents, Workspace, Runs, Waiting: {agents:?} {workspace:?} {runs:?} {waiting:?}"
    );
    assert!(
        rail.bottom() - waiting.bottom() < px(24.),
        "Waiting for you is pinned at the bottom: {waiting:?} in {rail:?}"
    );
    assert!(
        waiting.origin.y - runs.bottom() > px(100.),
        "pinned, not following the sections above: {runs:?} {waiting:?}"
    );
    ok(mcp.call("run_status", json!({"run": server, "action": "stop"})));
}

#[gpui_kit::test]
fn a_waiting_card_says_the_call_on_one_line_and_its_class_and_reason_on_the_next(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    let number = hold(&shell, &mcp, cx);
    assert_eq!(
        label(format!("approval-call-{number}"), cx).as_deref(),
        Some(r#"fs_read {"path":".env"}"#)
    );
    let call = rect(format!("approval-call-{number}"), cx);
    let class = rect(format!("approval-class-{number}"), cx);
    let reason = rect(format!("approval-reason-{number}"), cx);
    assert!(call.bottom() <= class.top(), "the call, then its class");
    assert!(
        (class.center().y - reason.center().y).abs() < px(2.),
        "the class and the reason on one line"
    );
    assert!(
        !has(format!("approval-base-{number}"), cx),
        "the rest is in Details"
    );
}

#[gpui_kit::test]
fn waiting_for_you_folds_away_when_nothing_waits(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    assert!(!has("rail-waiting", cx), "nothing waits: no section");
    let number = hold(&shell, &mcp, cx);
    assert!(has("rail-waiting", cx));

    cx.update(|window, cx| window.click(format!("deny-{number}"), cx));
    until(&shell, cx, "the card leaves", |cx| {
        settle(cx);
        !has(format!("allow-{number}"), cx)
    });
    settle(cx);
    assert!(!has("rail-waiting", cx), "the section folded away");
}

#[gpui_kit::test]
fn each_agent_row_carries_its_context_gauge_and_the_composer_shows_none(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    until(&shell, cx, "the gauge on the lead's row", |cx| {
        has(format!("rail-context-{lead}"), cx)
    });
    let gauge = label(format!("rail-context-{lead}"), cx).unwrap();
    assert_eq!(gauge, "41% of the context used");
    let row = rect(format!("rail-agent-{lead}"), cx);
    let at = rect(format!("rail-context-{lead}"), cx);
    assert!(
        at.origin.x > row.origin.x + row.size.width / 2.
            && at.origin.y >= row.origin.y
            && at.bottom() <= row.bottom(),
        "on the right of the agent's own row: {at:?} in {row:?}"
    );
    let composer = said_within("reply-model", cx).join(" | ");
    assert!(
        !composer.contains("context") && !composer.contains('%'),
        "the composer's bar says no context: {composer}"
    );
}

#[gpui_kit::test]
fn the_workspace_section_has_the_labelled_lines_of_the_card_and_its_menu(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    until(&shell, cx, "the repository's line", |cx| {
        has("rail-line-repository-app", cx)
    });
    assert_eq!(
        label("rail-line-repository-app", cx).as_deref(),
        Some("Repository app")
    );
    assert!(has("workspace-menu", cx), "its actions in a ⋯");
    let facts = said_within("rail-facts", cx).join(" | ");
    assert!(
        !facts.contains("Branch"),
        "the branch is said once: {facts}"
    );
}

#[gpui_kit::test]
fn runs_list_the_running_first_then_the_last_five_finished_with_how_they_ended(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let blips: Vec<String> = (0..KEPT_ENDED + 1).map(|_| run(&mcp, "blip")).collect();
    let server = run(&mcp, "server");
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the runs", |cx| {
        has(format!("command-{server}"), cx) && has(format!("command-{}", blips[KEPT_ENDED]), cx)
    });
    settle(cx);

    assert_eq!(KEPT_ENDED, 5);
    let arranged = page.read_with(cx, |page, cx| {
        let list = page.commands(cx).read(cx);
        let arranged = list.arranged();
        (
            arranged
                .running
                .iter()
                .map(|r| r.run.clone())
                .collect::<Vec<_>>(),
            arranged
                .kept
                .iter()
                .map(|r| r.run.clone())
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(arranged.0, vec![server.clone()], "running first");
    let mut latest: Vec<String> = blips[1..].to_vec();
    latest.reverse();
    assert_eq!(
        arranged.1, latest,
        "then the last five finished, latest first"
    );
    assert!(
        !has(format!("command-{}", blips[0]), cx),
        "the sixth one is gone"
    );
    let server_at = rect(format!("command-{server}"), cx);
    let first_ended = rect(format!("command-{}", blips[KEPT_ENDED]), cx);
    assert!(server_at.origin.y < first_ended.origin.y);
    assert_eq!(
        label(format!("command-status-{}", blips[KEPT_ENDED]), cx).as_deref(),
        Some("exit 0"),
        "how it ended"
    );
    // Who started it: the task; the root agent, whose conversation this is, is not named (#213).
    let who = label(format!("command-who-{server}"), cx).unwrap();
    assert_eq!(who, "task server");
    ok(mcp.call("run_status", json!({"run": server, "action": "stop"})));
}

/// Whether the run drawer is open on the page.
fn drawer_open(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> bool {
    page.read_with(cx, |page, cx| page.run_drawer().read(cx).is_open())
}

#[gpui_kit::test]
fn a_run_opens_its_drawer_from_the_rail_and_escape_closes_it(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let server = run(&mcp, "server");
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the run", |cx| {
        has(format!("command-{server}"), cx)
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("command-{server}"), cx));
    until(&shell, cx, "the drawer shows the output", |cx| {
        has("command-panel-line-0", cx)
    });
    settle(cx);
    assert!(drawer_open(&page, cx));
    assert!(has("run-drawer-panel", cx));
    assert_eq!(
        label("run-fact-command", cx).as_deref(),
        Some("./server.sh")
    );
    assert_eq!(label("run-fact-task", cx).as_deref(), Some("server"));
    assert_eq!(label("run-fact-where", cx).as_deref(), Some("w"));
    let by = label("run-fact-by", cx).unwrap();
    assert!(
        by.contains("the conversation") && by.contains("lead the work"),
        "who started it, and its session: {by}"
    );
    assert!(has("run-fact-since", cx));
    assert_eq!(label("run-fact-machine", cx).as_deref(), Some("this PC"));
    assert!(has("run-drawer-stop", cx), "Stop while it runs");
    assert!(has("run-drawer-copy", cx));
    assert_eq!(
        label("command-panel-line-0", cx).as_deref(),
        Some("listening on 8080")
    );

    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(!drawer_open(&page, cx), "Escape closes it");
    assert!(!has("run-drawer-panel", cx));
    ok(mcp.call("run_status", json!({"run": server, "action": "stop"})));
}

#[gpui_kit::test]
fn the_conversation_s_run_line_opens_the_same_drawer(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    let page = session_page(&shell, cx);
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "run server"}),
    )
    .unwrap();
    until(&shell, cx, "the conversation's run line", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Command(_)))
    });
    settle(cx);
    let (at, run) = blocks(&page, cx)
        .iter()
        .enumerate()
        .find_map(|(at, b)| match b {
            Block::Command(card) => Some((at, card.run.clone())),
            _ => None,
        })
        .unwrap();
    cx.update(|window, cx| window.click(format!("command-card-{at}"), cx));
    until(&shell, cx, "the drawer shows the run", |cx| {
        has("run-drawer-panel", cx) && has("command-panel-line-0", cx)
    });
    assert!(drawer_open(&page, cx));
    let shown = page.read_with(cx, |page, cx| {
        page.command_panel().read(cx).run().map(str::to_owned)
    });
    assert_eq!(shown.as_deref(), Some(run.as_str()));
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

// ── Sessions started by agents, and their messages (#159) ───────────────────────────────────

/// Has the lead start the session `Docs` in `w`, allowed by the user: its id.
fn docs_session(user: &User, lead: &str) -> String {
    let pending = || {
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .clone()
    };
    user.call(
        "agent.send",
        json!({"agent": lead, "message": "start the docs session"}),
    )
    .unwrap();
    eventually("the launch is held", || !pending().is_empty());
    let number = pending()[0]["number"].as_u64().unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let launched = || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["launched_by"] == lead)
            .and_then(|agent| agent["id"].as_str().map(str::to_owned))
    };
    eventually("the session exists", || launched().is_some());
    let id = launched().unwrap();
    eventually("its first turn ends", || state_of(user, &id) == "idle");
    id
}

#[gpui_kit::test]
fn a_session_an_agent_started_is_listed_under_its_workspace(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead(&user);
    let docs = docs_session(&user, &lead);
    let (shell, cx) = window(&user, 1440., cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(&docs), cx)
    });
    settle(cx);

    let listed: Vec<String> = shell.read_with(cx, |s, cx| {
        s.place("app", &Some("w".to_owned()))
            .map(|node| {
                node.read(cx)
                    .sessions(cx)
                    .iter()
                    .map(|row| row.read(cx).id().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    });
    assert!(
        listed.contains(&docs) && listed.contains(&lead),
        "both under w: {listed:?}"
    );
    let place = rect(place_id("app", &Some("w".to_owned())), cx);
    let row = rect(session_id(&docs), cx);
    assert!(row.origin.y > place.origin.y, "under its workspace's row");
}

#[gpui_kit::test]
fn a_session_send_message_is_marked_with_its_sender_in_both_conversations(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead(&user);
    let docs = docs_session(&user, &lead);
    let pending = || {
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .clone()
    };
    user.call(
        "agent.send",
        json!({"agent": lead, "message": format!("write to the session {docs}")}),
    )
    .unwrap();
    eventually("the message is held", || !pending().is_empty());
    let number = pending()[0]["number"].as_u64().unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();

    // The session that received it: a message of the lead's, never the user's.
    let (shell, cx) = open_session(&user, &docs, 1440., cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the message reached the session", |cx| {
        blocks(&page, cx).iter().any(|b| {
            matches!(b, Block::Relayed { sent: false, text, .. } if text == "The tests pass now.")
        })
    });
    assert!(
        !blocks(&page, cx).iter().any(|b| matches!(
            b,
            Block::Message { text, .. } if text.contains("The tests pass now.")
        )),
        "not drawn as the user's"
    );
    let (at, peer) = blocks(&page, cx)
        .iter()
        .enumerate()
        .find_map(|(at, b)| match b {
            Block::Relayed {
                sent: false, peer, ..
            } => Some((at, peer.clone())),
            _ => None,
        })
        .unwrap();
    assert!(peer.contains(&lead), "the daemon names the sender: {peer}");
    settle(cx);
    assert_eq!(
        label(format!("row-{at}-relayed-from"), cx).as_deref(),
        Some("from lead the work")
    );

    // The session that sent it: the same message, to its target.
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the sender shows what it sent", |cx| {
        let page = session_page(&shell, cx);
        page.read_with(cx, |p, _| p.id() == lead)
            && blocks(&page, cx).iter().any(|b| {
                matches!(b, Block::Relayed { sent: true, peer, text, .. }
                    if peer == "Docs" && text == "The tests pass now.")
            })
    });
}

#[gpui_kit::test]
fn the_session_page_with_its_rail_draws_nothing_at_rest(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (shell, cx) = open_session(&user, &lead, 1440., cx);
    let number = hold(&shell, &mcp, cx);
    until(&shell, cx, "the gauge on the lead's row", |cx| {
        has(format!("rail-context-{lead}"), cx)
    });
    cx.update(|window, cx| window.click(format!("approval-details-{number}"), cx));
    settle(cx);
    cx.update(|window, cx| window.click(format!("approval-details-{number}"), cx));
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves once the card folded back"
    );
}
