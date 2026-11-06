//! The scenarios of #403 and #526, the links an agent hands the user: the window offers them in a
//! toast with an Open button on any page but the session's own, and keeps one card of the links
//! shown in a row in the conversation, a line each with the same button and a thumbnail for a
//! picture; a click opens one with the system's default handler, which these scenarios fake.
//! Nothing opens by itself. On GPUI's test platform in front of a real daemon whose agents are the fake ACP agent,
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
    let link = |url: &str, title: &str| json!({"mcp": {"tool": "web", "arguments": {"action": "show_link", "url": url, "title": title}}});
    let file = |path: &str| json!({"mcp": {"tool": "web", "arguments": {"action": "show_link", "path": path}}});
    user.fake_script(json!({"turns": [
        {"when": "lead the work", "do": [{"say": "On it."}]},
        {"when": "SHOW THREE", "do": [
            link("https://example.org/1", "One"),
            link("https://example.org/2", "Two"),
            link("https://example.org/3", "Three")]},
        {"when": "LAUNCH A WORKER", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Worker", "brief": "WORKER SHOWS ONE"}}}]},
        {"when": "WORKER SHOWS ONE", "do": [link("https://example.org/w", "Worker's")]},
        {"when": "STAY QUIET", "do": []},
        {"when": "SHOW SPLIT", "do": [
            link("https://example.org/1", "One"),
            {"say": "That was the first."},
            link("https://example.org/2", "Two")]},
        {"when": "SHOW PICTURES", "do": [
            file("shot.png"), file("fake.png"), file("big.png"), file("gone.png"), file("huge.png"),
            link("https://example.org/shot.png", "Remote")]}
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
        .filter(|label| label.starts_with("The agent shares"))
        .collect()
}

/// The fake agent shows the links its turn `message` names.
fn tell(user: &User, agent: &str, message: &str) {
    user.call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

/// A picture, 1 pixel by 1.
const PNG: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A picture of 11000 by 11000 pixels, as far as its header says: a few bytes that would take
/// half a gigabyte decoded, and nothing a decoder could read.
const HUGE_PNG: [u8; 58] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x2a, 0xf8, 0x00, 0x00, 0x2a, 0xf8, 0x08, 0x06, 0x00, 0x00, 0x00, 0xce, 0x2f, 0x9a,
    0x8a, 0x00, 0x00, 0x00, 0x01, 0x49, 0x44, 0x41, 0x54, 0x00, 0x28, 0x38, 0x7d, 0xe8, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The row of the card of links, once there is one.
fn card_at(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<usize> {
    blocks(page, cx)
        .iter()
        .position(|block| matches!(block, Block::Links { .. }))
}

/// How many cards of links the conversation holds.
fn cards(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> usize {
    blocks(page, cx)
        .iter()
        .filter(|block| matches!(block, Block::Links { .. }))
        .count()
}

/// The links of the card in row `at`.
fn lines(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) -> usize {
    match &blocks(page, cx)[at] {
        Block::Links { links } => links.len(),
        _ => 0,
    }
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
        card_at(&page, cx).is_some()
    });
    settle(cx);
    let at = card_at(&page, cx).unwrap();

    assert_eq!(
        label(format!("row-{at}-link-0"), cx).as_deref(),
        Some("Link shared by the agent: Ticket 42, example.org")
    );
    // The conversation is on screen: the card is where the link is, and no toast says it too.
    assert!(!has("toast-1", cx), "{:?}", toast_said(cx));
    assert!(opened.lock().unwrap().is_empty());

    cx.update(|window, cx| window.click(format!("row-{at}-link-0-open"), cx));
    settle(cx);

    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://example.org/t/42")]
    );
    // A card is a conversation's own: it is still there after a click.
    assert!(has(format!("row-{at}-links"), cx));
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
        card_at(&page, cx).is_some()
    });
    settle(cx);
    let at = card_at(&page, cx).unwrap();
    assert_eq!(
        label(format!("row-{at}-link-0"), cx).as_deref(),
        Some("Link shared by the agent: report.html, report.html")
    );

    cx.update(|window, cx| window.click(format!("row-{at}-link-0-open"), cx));
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
    until(&shell, cx, "the card is in sight", |cx| {
        card_at(&page, cx).is_some()
    });
    settle(cx);
    let at = card_at(&page, cx).unwrap();

    // Between the call and the click, the file became a program.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.update(|window, cx| window.click(format!("row-{at}-link-0-open"), cx));
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

#[gpui_kit::test]
fn links_shown_in_a_row_are_one_card(cx: &mut TestAppContext) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    let opened = faked_opener(&shell, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);

    tell(&user, &agent, "SHOW THREE");
    until(&shell, cx, "the three links are one card", |cx| {
        card_at(&page, cx).is_some_and(|at| lines(&page, at, cx) == 3)
    });
    settle(cx);
    let at = card_at(&page, cx).unwrap();

    assert_eq!(cards(&page, cx), 1);
    assert_eq!(
        label(format!("row-{at}-links-count"), cx).as_deref(),
        Some("3 links")
    );
    for (n, (name, whence)) in [
        ("One", "example.org"),
        ("Two", "example.org"),
        ("Three", "example.org"),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            label(format!("row-{at}-link-{n}"), cx).as_deref(),
            Some(format!("Link shared by the agent: {name}, {whence}").as_str())
        );
    }
    // The steps of the three calls are not drawn apart: the card is what the user sees of them.
    assert!(
        every_label(cx).iter().all(|label| !label.contains("step")),
        "{:?}",
        every_label(cx)
    );
    // Each line has its own Open.
    cx.update(|window, cx| window.click(format!("row-{at}-link-1-open"), cx));
    settle(cx);
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://example.org/2")]
    );
}

#[gpui_kit::test]
fn a_link_after_the_agent_s_text_starts_a_new_card(cx: &mut TestAppContext) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);

    tell(&user, &agent, "SHOW SPLIT");
    until(&shell, cx, "the two cards are in the conversation", |cx| {
        cards(&page, cx) == 2
    });
    settle(cx);

    let all = blocks(&page, cx);
    let first = all
        .iter()
        .position(|block| matches!(block, Block::Links { .. }))
        .unwrap();
    assert!(
        matches!(&all[first + 1..], [Block::Text { text }, Block::Steps { .. }, Block::Links { links }]
            if text.contains("That was the first") && links.len() == 1)
    );
    // A card of one link keeps today's card: no count above it.
    assert!(!has(format!("row-{first}-links-count"), cx));
    assert!(has(format!("row-{first}-link-0"), cx));
}

#[gpui_kit::test]
fn a_group_of_links_gives_one_toast(cx: &mut TestAppContext) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    let opened = faked_opener(&shell, cx);
    following(&shell, &agent, cx);

    tell(&user, &agent, "SHOW THREE");
    until(&shell, cx, "the toast counts the three links", |cx| {
        toast_said(cx) == ["The agent shares 3 links".to_owned()]
    });
    settle(cx);

    // The links that came after the first updated its toast; none added a second.
    assert!(has("toast-1", cx));
    assert!(!has("toast-2", cx), "{:?}", every_label(cx));
    assert_eq!(toast_said(cx), ["The agent shares 3 links".to_owned()]);
    // Three links cannot be opened by one button: it goes to the conversation.
    assert_eq!(label("toast-1-action", cx).as_deref(), Some("Show"));
    cx.update(|window, cx| window.click("toast-1-action", cx));
    settle(cx);
    assert!(
        opened.lock().unwrap().is_empty(),
        "nothing opens on its own"
    );
    until(&shell, cx, "the conversation is shown", |cx| {
        session_page(&shell, cx).read_with(cx, |page, _| page.id() == agent)
    });
}

#[gpui_kit::test]
fn no_toast_for_a_link_shown_in_the_session_on_screen(cx: &mut TestAppContext) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);

    tell(&user, &agent, "SHOW THREE");
    until(&shell, cx, "the card has its three links", |cx| {
        card_at(&page, cx).is_some_and(|at| lines(&page, at, cx) == 3)
    });
    settle(cx);

    assert!(!has("toast-1", cx), "{:?}", toast_said(cx));
}

#[gpui_kit::test]
fn a_shown_picture_is_a_thumbnail_that_opens_it(cx: &mut TestAppContext) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    std::fs::write(user.path("work/app/shot.png"), PNG).unwrap();
    std::fs::write(user.path("work/app/fake.png"), "<p>not a picture</p>").unwrap();
    std::fs::write(user.path("work/app/gone.png"), PNG).unwrap();
    std::fs::write(user.path("work/app/huge.png"), HUGE_PNG).unwrap();
    std::fs::File::create(user.path("work/app/big.png"))
        .unwrap()
        .set_len(21 * 1024 * 1024)
        .unwrap();
    let (shell, cx) = window(&user, cx);
    let opened = faked_opener(&shell, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);

    tell(&user, &agent, "SHOW PICTURES");
    until(&shell, cx, "the card has its six links", |cx| {
        card_at(&page, cx).is_some_and(|at| lines(&page, at, cx) == 6)
    });
    let at = card_at(&page, cx).unwrap();
    until(&shell, cx, "the picture is read", |cx| {
        has(format!("row-{at}-link-0-thumb"), cx)
    });
    settle(cx);

    // An address gets no thumbnail: the window fetches nothing from the network.
    assert!(!has(format!("row-{at}-link-5-thumb"), cx));
    assert!(has(format!("row-{at}-link-5-open"), cx));

    // The click on the thumbnail goes where Open goes.
    cx.update(|window, cx| window.click(format!("row-{at}-link-0-thumb"), cx));
    settle(cx);
    let real = std::fs::canonicalize(user.path("work/app/shot.png")).unwrap();
    assert_eq!(
        opened.lock().unwrap().as_slice(),
        std::slice::from_ref(&real)
    );

    // ... through the same checks: a picture made a program is not opened by it.
    {
        use std::os::unix::fs::PermissionsExt;
        let file = user.path("work/app/shot.png");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    cx.update(|window, cx| window.click(format!("row-{at}-link-0-thumb"), cx));
    settle(cx);
    assert_eq!(*opened.lock().unwrap(), [real]);
    assert!(
        every_label(cx)
            .iter()
            .any(|label| label.starts_with("Not opened") && label.contains("executable")),
        "{:?}",
        every_label(cx)
    );
}

#[gpui_kit::test]
fn a_picture_that_cannot_be_read_keeps_its_plain_line(cx: &mut TestAppContext) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    std::fs::write(user.path("work/app/shot.png"), PNG).unwrap();
    std::fs::write(user.path("work/app/fake.png"), "<p>not a picture</p>").unwrap();
    std::fs::write(user.path("work/app/gone.png"), PNG).unwrap();
    std::fs::write(user.path("work/app/huge.png"), HUGE_PNG).unwrap();
    std::fs::File::create(user.path("work/app/big.png"))
        .unwrap()
        .set_len(21 * 1024 * 1024)
        .unwrap();
    // Shown before the window reads them: the conversation is built from the history, once the
    // file the agent showed was removed.
    tell(&user, &agent, "SHOW PICTURES");
    eventually("the turn", || {
        user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["name"] == "agent.link_shown")
            .count()
            == 6
    });
    std::fs::remove_file(user.path("work/app/gone.png")).unwrap();
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the card is in the conversation", |cx| {
        card_at(&page, cx).is_some()
    });
    let at = card_at(&page, cx).unwrap();
    until(&shell, cx, "the picture is read", |cx| {
        has(format!("row-{at}-link-0-thumb"), cx)
    });
    settle(cx);

    // The one that is a picture has its thumbnail; the others keep their line and their Open.
    for n in 1..6 {
        assert!(!has(format!("row-{at}-link-{n}-thumb"), cx), "link {n}");
        assert!(has(format!("row-{at}-link-{n}-open"), cx), "link {n}");
    }
    assert_eq!(
        label(format!("row-{at}-link-1"), cx).as_deref(),
        Some("Link shared by the agent: fake.png, fake.png")
    );
}

#[gpui_kit::test]
fn the_group_is_the_same_after_the_page_is_left_and_shown_again(cx: &mut TestAppContext) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);

    tell(&user, &agent, "SHOW SPLIT");
    until(&shell, cx, "the two cards are in the conversation", |cx| {
        cards(&page, cx) == 2
    });
    settle(cx);
    let cards_of = |blocks: Vec<Block>| -> Vec<Block> {
        blocks
            .into_iter()
            .filter(|block| matches!(block, Block::Links { .. } | Block::Text { .. }))
            .collect()
    };
    let before = cards_of(blocks(&page, cx));

    cx.update(|window, cx| window.click(flottille_window::tree::project_id("app"), cx));
    settle(cx);
    open_session(&shell, &agent, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the conversation is built again", |cx| {
        cards(&page, cx) == 2
    });
    settle(cx);

    assert_eq!(cards_of(blocks(&page, cx)), before);
    let first = card_at(&page, cx).unwrap();
    assert_eq!(
        label(format!("row-{first}-link-0"), cx).as_deref(),
        Some("Link shared by the agent: One, example.org")
    );
}

#[gpui_kit::test]
fn a_link_of_a_sub_agent_toasts_while_only_the_root_s_conversation_is_shown(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    open_session(&shell, &agent, cx);

    tell(&user, &agent, "LAUNCH A WORKER");
    // The worker's conversation is not the one on screen: the toast says it.
    until(&shell, cx, "the toast of the worker's link", |cx| {
        toast_said(cx) == ["The agent shares a link: Worker's".to_owned()]
    });
}

#[gpui_kit::test]
fn a_new_group_of_links_gets_a_toast_of_its_own(cx: &mut TestAppContext) {
    let user = user();
    let (agent, _mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    following(&shell, &agent, cx);

    tell(&user, &agent, "SHOW SPLIT");
    // The agent's words in between end the row: the second link is not the first one's toast
    // overwritten, it has its own.
    until(&shell, cx, "both toasts are on screen", |cx| {
        let said = toast_said(cx);
        said.contains(&"The agent shares a link: One".to_owned())
            && said.contains(&"The agent shares a link: Two".to_owned())
    });
    assert!(has("toast-2", cx));
}

#[gpui_kit::test]
fn the_user_s_own_message_ends_the_row_of_links_too(cx: &mut TestAppContext) {
    let user = user();
    let (agent, mcp) = lead(&user);
    let (shell, cx) = window(&user, cx);
    following(&shell, &agent, cx);

    show(
        &mcp,
        json!({"action": "show_link", "url": "https://example.org/a", "title": "A"}),
    );
    until(&shell, cx, "the first toast", |cx| {
        toast_said(cx) == ["The agent shares a link: A".to_owned()]
    });
    tell(&user, &agent, "STAY QUIET");
    eventually("the message is recorded", || {
        user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["name"] == "agent.prompted"
                    && event["data"]["text"]
                        .as_str()
                        .is_some_and(|t| t.contains("STAY QUIET"))
            })
    });
    // Let the window take the event before the next link comes.
    until(&shell, cx, "the window took it", |cx| {
        settle(cx);
        true
    });
    show(
        &mcp,
        json!({"action": "show_link", "url": "https://example.org/b", "title": "B"}),
    );
    until(&shell, cx, "the second toast", |cx| {
        toast_said(cx).contains(&"The agent shares a link: B".to_owned())
    });
    assert!(
        toast_said(cx).contains(&"The agent shares a link: A".to_owned()),
        "{:?}",
        toast_said(cx)
    );
}
