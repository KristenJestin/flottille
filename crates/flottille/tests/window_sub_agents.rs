//! The scenarios of #512, the sub-agents card of a conversation: it starts folded to one line that
//! counts its sub-agents by state, a click on its title opens and folds the list, the choice is
//! kept while a sub-agent changes or arrives, and the folded line names a sub-agent whose request
//! waits for the user. On GPUI's test platform in front of a real daemon whose agents are the
//! fake ACP agent, with a throwaway user. No window opens on a screen.

mod common;
mod guard;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::Mcp;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// Workers that hang mid-turn (running) or report at once (completed).
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "REPORT", "do": [{"mcp": {"tool": "report", "arguments": {"text": "done"}}}]}
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

/// The project `app` over a throwaway repository with a remote and a `.env`, its ready
/// workspace `w`, and a lead the user started there, its first turn ended: the lead's id and its
/// MCP session.
fn lead(user: &User) -> (String, Mcp) {
    let folder: PathBuf = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "TOKEN=made-up-made-up\n").unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
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
    text.push_str("\n[[recipe]]\ncopy = [\".env\"]\n");
    std::fs::write(&file, text).unwrap();
    user.call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace is ready", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "lead the work"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let mcp = mcp_session(user, |prompt| prompt.contains("You are the root"));
    eventually("the lead's first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    (id, mcp)
}

/// The MCP session of the last agent session opened whose instructions `which` accepts.
fn mcp_session(user: &User, which: impl Fn(&str) -> bool) -> Mcp {
    let found = || {
        user.received_requests("session/new")
            .into_iter()
            .rev()
            .find(|opened| {
                which(
                    opened["params"]["_meta"]["claudeCode"]["options"]["systemPrompt"]["prompt"][2]
                        .as_str()
                        .unwrap_or_default(),
                )
            })
    };
    eventually("the session", || found().is_some());
    let server = found().unwrap()["params"]["mcpServers"][0].clone();
    Mcp::initialize(
        server["url"].as_str().unwrap(),
        server["headers"][0]["value"].as_str().unwrap(),
    )
}

/// A worker of the lead's in `w` that `what` decides the turn of (`HANG`: it stays running,
/// `REPORT`: it reports and is completed): its id.
fn launch(lead: &Mcp, what: &str, title: &str) -> String {
    let answer = lead.call(
        "agent_launch",
        json!({"kind": "child", "title": title, "brief": format!("{what} and {title}"), "workspace": "w"}),
    );
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap()
        .to_owned()
}

/// A worker kept mid-turn: its id and its MCP session.
fn running(user: &User, lead: &Mcp, title: &str) -> (String, Mcp) {
    let id = launch(lead, "HANG", title);
    let task = format!("Your task: {title}.");
    (id, mcp_session(user, |prompt| prompt.contains(&task)))
}

/// The number of the request a held call's answer names.
fn held_number(answer: common::mcp::Answer) -> u64 {
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

fn open_session<'a>(
    user: &User,
    lead: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(lead), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(&shell, cx, "the session's page", |cx| {
        session_page(&shell, cx).read_with(cx, |page, cx| {
            !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
        })
    });
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the link sent, then draws what changed.
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

/// Lets every animation end.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn at_rest(cx: &mut VisualTestContext) {
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

/// The row of the sub-agents card, once the conversation has it.
fn card_row(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<usize> {
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .position(|b| *b == Block::SubAgents)
    })
}

/// Opens the lead's page and waits for its sub-agents card to have `count` sub-agents.
fn card_of<'a>(
    user: &User,
    lead: &str,
    count: usize,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<SessionPage>,
    usize,
    &'a mut VisualTestContext,
) {
    let (shell, cx) = open_session(user, lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the sub-agents card lists them", |cx| {
        card_row(&page, cx).is_some()
            && page.read_with(cx, |page, cx| {
                page.conversation()
                    .read(cx)
                    .rows()
                    .iter()
                    .map(|row| row.read(cx).sub_agents().len())
                    .sum::<usize>()
                    == count
            })
    });
    settle(cx);
    let at = card_row(&page, cx).unwrap();
    (shell, page, at, cx)
}

fn toggle(at: usize) -> String {
    format!("row-{at}-sub-agents-toggle")
}

fn listed(child: &str, cx: &mut VisualTestContext) -> bool {
    has(format!("sub-agent-{child}"), cx)
}

#[gpui_kit::test]
fn the_sub_agents_card_starts_folded_with_its_counts(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let first = launch(&mcp, "REPORT", "write the docs");
    let second = launch(&mcp, "REPORT", "write the tests");
    let (third, _worker) = running(&user, &mcp, "build the form");
    eventually("the reports are in", || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["state"] == "ended")
            .count()
            == 2
    });
    let (shell, _page, at, cx) = card_of(&user, &lead, 3, cx);
    until(&shell, cx, "the counts are read", |cx| {
        label(toggle(at), cx).as_deref() == Some("3 sub-agents · 2 completed · 1 running")
    });

    assert_eq!(
        cx.update(|window, _| window.find(toggle(at)).expanded()),
        Some(false),
        "folded"
    );
    for child in [&first, &second, &third] {
        assert!(!listed(child, cx), "{child} is not listed while folded");
    }
    assert!(has(format!("row-{at}-sub-agents"), cx), "the card is drawn");
    assert!(
        has(format!("row-{at}-sub-agents-card-meta"), cx),
        "beside its meta, launched by the lead"
    );
}

#[gpui_kit::test]
fn a_click_on_the_sub_agents_title_opens_and_folds_the_list(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (child, _worker) = running(&user, &mcp, "build the form");
    let (_shell, _page, at, cx) = card_of(&user, &lead, 1, cx);
    assert!(!listed(&child, cx), "folded to start");

    cx.update(|window, cx| window.click(toggle(at), cx));
    settle(cx);
    assert_eq!(
        cx.update(|window, _| window.find(toggle(at)).expanded()),
        Some(true)
    );
    assert!(listed(&child, cx), "the list is open");

    cx.update(|window, cx| window.click(toggle(at), cx));
    settle(cx);
    assert_eq!(
        cx.update(|window, _| window.find(toggle(at)).expanded()),
        Some(false)
    );
    assert!(!listed(&child, cx), "the list is folded away");
    at_rest(cx);
}

#[gpui_kit::test]
fn the_sub_agents_card_stays_as_the_user_left_it_when_a_sub_agent_changes(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (first, worker) = running(&user, &mcp, "build the form");
    let (shell, page, at, cx) = card_of(&user, &lead, 1, cx);

    // Folded by the user's choice (the start): a sub-agent arrives, another one ends.
    let (second, _other) = running(&user, &mcp, "write the docs");
    until(&shell, cx, "the new sub-agent is counted", |cx| {
        label(toggle(at), cx).as_deref() == Some("2 sub-agents · 2 running")
    });
    held_number(worker.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the wait is counted", |cx| {
        label(toggle(at), cx).as_deref() == Some("2 sub-agents · 1 running · 1 waiting")
    });
    settle(cx);
    assert_eq!(
        cx.update(|window, _| window.find(toggle(at)).expanded()),
        Some(false),
        "still folded"
    );
    assert!(!listed(&first, cx) && !listed(&second, cx));

    // Opened by the user: a sub-agent arrives, another one ends.
    cx.update(|window, cx| window.click(toggle(at), cx));
    settle(cx);
    assert!(listed(&first, cx) && listed(&second, cx));
    let (third, other) = running(&user, &mcp, "write the tests");
    until(&shell, cx, "the third is listed", |cx| listed(&third, cx));
    held_number(other.call("fs_read", json!({"path": ".env"})));
    until(&shell, cx, "the wait is counted", |cx| {
        label(toggle(at), cx).as_deref() == Some("3 sub-agents · 1 running · 2 waiting")
    });
    settle(cx);
    assert_eq!(
        cx.update(|window, _| window.find(toggle(at)).expanded()),
        Some(true),
        "still open"
    );
    assert!(listed(&first, cx) && listed(&second, cx) && listed(&third, cx));
    assert_eq!(card_row(&page, cx), Some(at), "the same card");
    at_rest(cx);
}

#[gpui_kit::test]
fn a_sub_agent_that_needs_the_user_is_said_on_the_folded_line(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (child, worker) = running(&user, &mcp, "read the settings");
    let (other, _other) = running(&user, &mcp, "build the form");
    let (shell, _page, at, cx) = card_of(&user, &lead, 2, cx);
    assert!(!has(format!("row-{at}-sub-agents-needs"), cx));

    // The worker's call is held by the lead, which passes it up: it is the user's now.
    let number = held_number(worker.call("fs_read", json!({"path": ".env"})));
    let answer = mcp.call(
        "approval_answer",
        json!({"request_id": number, "decision": "forward", "why": "it needs the database name"}),
    );
    assert!(!answer.error, "{}", answer.text);
    until(&shell, cx, "the folded line names it", |cx| {
        label(toggle(at), cx).as_deref() == Some("2 sub-agents · 1 running · 1 needs you")
    });
    settle(cx);
    assert!(
        has(format!("row-{at}-sub-agents-needs"), cx),
        "in its own ink"
    );
    assert_eq!(
        cx.update(|window, _| window.find(toggle(at)).expanded()),
        Some(false),
        "reachable without opening the card"
    );
    assert!(!listed(&child, cx) && !listed(&other, cx));

    // Answered: the line is back to its states.
    user.call("approval.deny", json!({"number": number}))
        .unwrap();
    until(&shell, cx, "the line no longer asks", |cx| {
        label(toggle(at), cx).as_deref() == Some("2 sub-agents · 2 running")
    });
    settle(cx);
    assert!(!has(format!("row-{at}-sub-agents-needs"), cx));
    at_rest(cx);
}

#[gpui_kit::test]
fn the_folded_sub_agents_card_is_at_rest(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (_child, _worker) = running(&user, &mcp, "build the form");
    let (_shell, _page, at, cx) = card_of(&user, &lead, 1, cx);
    assert_eq!(
        cx.update(|window, _| window.find(toggle(at)).expanded()),
        Some(false),
        "folded"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn a_long_sub_agent_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = lead(&user);
    let (child, _worker) = running(
        &user,
        &mcp,
        &format!("{} {}", guard::UNBROKEN, guard::SPACED),
    );
    let (shell, _page, at, cx) = card_of(&user, &lead, 1, cx);
    cx.update(|window, cx| window.click(toggle(at), cx));
    settle(cx);
    assert!(listed(&child, cx), "the list is open");
    guard::narrow(cx);
    settle(cx);
    guard::assert_stays_inside(&shell, cx, format!("row-{at}-sub-agents"));
}
