//! The scenarios of #403, a link an agent hands the user: the window offers it in a toast with an
//! Open button, on any page, and keeps a card of it in the conversation with the same button; a
//! click opens it with the system's default handler, which these scenarios fake. Nothing opens by
//! itself. On GPUI's test platform in front of a real daemon whose agents are the fake ACP agent,
//! with a throwaway user. No window opens on a screen, and no link opens anywhere.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::tree::session_id;
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const BRIEF: &str = "lead the work on the login page";

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "lead the work", "do": [{"say": "On it."}]}
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

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The project `app` over a throwaway repository.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// A session the user started in the project, its first turn ended: its id and its MCP session.
fn lead(user: &User) -> (String, Mcp) {
    project(user);
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": BRIEF}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("the first turn", || state_of(user, &id) == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(800.)));
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

/// Runs the window until it shows the session `agent` from the daemon. The window subscribes to
/// the daemon's events before it reads what it shows, and an event emitted before it subscribed
/// never reaches it: an offline window already has its frame, so its elements say nothing of it.
fn following(shell: &Entity<Shell>, agent: &str, cx: &mut VisualTestContext) {
    until(shell, cx, "the window follows the daemon", |cx| {
        has(session_id(agent), cx)
    });
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

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Every label the window says, as text.
fn every_label(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| {
        window
            .elements()
            .into_iter()
            .filter_map(|element| element.label().map(str::to_owned))
            .collect()
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn open_session(shell: &Entity<Shell>, agent: &str, cx: &mut VisualTestContext) {
    until(shell, cx, "the session is in the tree", |cx| {
        has(session_id(agent), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
    until(
        shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            session_page(shell, cx).read_with(cx, |page, cx| {
                page.id() == agent
                    && !page.conversation().read(cx).rows().is_empty()
                    && !page.tree().is_empty()
            })
        },
    );
    settle(cx);
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

/// What the window opened: the links and files it was asked to open, in order.
type Opened = Arc<Mutex<Vec<PathBuf>>>;

fn faked_opener(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Opened {
    let opened: Opened = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &std::path::Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    opened
}

fn show(mcp: &Mcp, arguments: Value) {
    let answer = mcp.call("web", arguments);
    assert!(!answer.error, "{}", answer.text);
}

fn toast_said(cx: &mut VisualTestContext) -> Vec<String> {
    every_label(cx)
        .into_iter()
        .filter(|label| label.starts_with("The agent shares a link"))
        .collect()
}

#[gpui_kit::test]
fn show_link_puts_a_toast_with_open_in_the_window(cx: &mut TestAppContext) {
    let user = user();
    let (agent, mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    let opened = faked_opener(&shell, cx);
    following(&shell, &agent, cx);

    show(
        &mcp,
        json!({"action": "show_link", "url": "https://example.org/t/42", "title": "Ticket 42"}),
    );
    until(&shell, cx, "the toast is in sight", |cx| {
        has("toast-1-action", cx)
    });
    settle(cx);

    assert_eq!(
        toast_said(cx),
        ["The agent shares a link: Ticket 42".to_owned()]
    );
    assert_eq!(label("toast-1-action", cx).as_deref(), Some("Open"));
    assert!(
        opened.lock().unwrap().is_empty(),
        "nothing opens on its own"
    );

    cx.update(|window, cx| window.click("toast-1-action", cx));
    settle(cx);

    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://example.org/t/42")]
    );
    assert!(!has("toast-1", cx), "the toast left once it was opened");
}

#[gpui_kit::test]
fn a_toast_for_a_link_without_a_title_says_its_host(cx: &mut TestAppContext) {
    let user = user();
    let (agent, mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    following(&shell, &agent, cx);

    show(
        &mcp,
        json!({"action": "show_link", "url": "http://localhost:3000/"}),
    );
    until(&shell, cx, "the toast is in sight", |cx| has("toast-1", cx));

    assert_eq!(
        toast_said(cx),
        ["The agent shares a link: localhost".to_owned()]
    );
}

#[gpui_kit::test]
fn show_link_leaves_a_card_in_the_conversation(cx: &mut TestAppContext) {
    let user = user();
    let (agent, mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    let opened = faked_opener(&shell, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);

    show(
        &mcp,
        json!({"action": "show_link", "url": "https://example.org/t/42", "title": "Ticket 42"}),
    );
    until(&shell, cx, "the card is in the conversation", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|block| matches!(block, Block::Link { .. }))
    });
    settle(cx);
    let at = blocks(&page, cx)
        .iter()
        .position(|block| matches!(block, Block::Link { .. }))
        .unwrap();

    assert_eq!(
        label(format!("row-{at}-link"), cx).as_deref(),
        Some("Link shared by the agent: Ticket 42, example.org")
    );
    // The toast was missed: the card still opens it.
    cx.update(|window, cx| window.click("toast-1-dismiss", cx));
    settle(cx);
    assert!(!has("toast-1", cx));
    assert!(opened.lock().unwrap().is_empty());

    cx.update(|window, cx| window.click(format!("row-{at}-link-open"), cx));
    settle(cx);

    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://example.org/t/42")]
    );
    // A card is a conversation's own: it is still there after a click.
    assert!(has(format!("row-{at}-link"), cx));
}

#[gpui_kit::test]
fn show_link_of_a_file_opens_the_file_from_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (agent, mcp) = lead(&user);
    std::fs::write(user.path("work/app/report.html"), "<p>report</p>").unwrap();
    let (shell, cx) = window(&user, cx);
    let opened = faked_opener(&shell, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);

    show(&mcp, json!({"action": "show_link", "path": "report.html"}));
    until(&shell, cx, "the card is in the conversation", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|block| matches!(block, Block::Link { .. }))
    });
    settle(cx);
    let at = blocks(&page, cx)
        .iter()
        .position(|block| matches!(block, Block::Link { .. }))
        .unwrap();
    assert_eq!(
        label(format!("row-{at}-link"), cx).as_deref(),
        Some("Link shared by the agent: report.html, report.html")
    );

    cx.update(|window, cx| window.click(format!("row-{at}-link-open"), cx));
    settle(cx);

    let real = std::fs::canonicalize(user.path("work/app/report.html")).unwrap();
    assert_eq!(*opened.lock().unwrap(), [real]);
}

#[gpui_kit::test]
fn a_file_made_executable_after_it_was_shown_does_not_open(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let user = user();
    let (agent, mcp) = lead(&user);
    let file = user.path("work/app/report.html");
    std::fs::write(&file, "<p>report</p>").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (shell, cx) = window(&user, cx);
    let opened = faked_opener(&shell, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);

    show(&mcp, json!({"action": "show_link", "path": "report.html"}));
    until(&shell, cx, "the toast and the card are in sight", |cx| {
        has("toast-1-action", cx)
            && blocks(&page, cx)
                .iter()
                .any(|block| matches!(block, Block::Link { .. }))
    });
    settle(cx);
    let at = blocks(&page, cx)
        .iter()
        .position(|block| matches!(block, Block::Link { .. }))
        .unwrap();

    // Between the call and the click, the file became a program.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.update(|window, cx| window.click("toast-1-action", cx));
    settle(cx);
    cx.update(|window, cx| window.click(format!("row-{at}-link-open"), cx));
    settle(cx);

    assert!(
        opened.lock().unwrap().is_empty(),
        "{:?}",
        opened.lock().unwrap()
    );
    let failures: Vec<String> = every_label(cx)
        .into_iter()
        .filter(|label| label.starts_with("Not opened"))
        .collect();
    assert!(!failures.is_empty(), "{:?}", every_label(cx));
    assert!(
        failures.iter().all(|label| label.contains("executable")),
        "{failures:?}"
    );
}
