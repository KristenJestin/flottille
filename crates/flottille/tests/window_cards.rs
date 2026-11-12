//! The guard of #772: no text of the user or of an agent leaves its card. Each kind of row of the
//! conversation is drawn in a window 360 px wide with a long unbroken word and a long spaced
//! sentence, and `guard::assert_stays_inside` refuses a part of it that goes past the card.
//! On GPUI's test platform in front of a real daemon whose agent is the fake ACP agent, with a
//! throwaway user; the rows are events made up for the page. No window opens on a screen.

mod common;
mod guard;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_core::protocol::Event;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::session_link::SessionUpdate;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use guard::{SPACED, UNBROKEN};
use serde_json::{Value, json};

/// Events made up for the page start here, above the ones the daemon gave.
const FIRST: u64 = 10_000;

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": []}));
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

/// A session of the project `app`, idle after its first turn.
fn session(user: &User) -> String {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("its first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

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

/// The page of a session, in a window 360 px wide.
fn page_of<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<SessionPage>, &'a mut VisualTestContext) {
    let id = session(user);
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(900.)));
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the session's page", |cx| {
        shell
            .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
            .is_some_and(|page| {
                page.read_with(cx, |page, cx| {
                    !page.conversation().read(cx).rows().is_empty()
                })
            })
    });
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .unwrap();
    guard::narrow(cx);
    settle(cx);
    (page, cx)
}

/// The events, numbered after the ones already given, as the page's link would give them.
fn show(page: &Entity<SessionPage>, events: Vec<(&str, Value)>, cx: &mut VisualTestContext) {
    let events: Vec<Event> = events
        .into_iter()
        .zip(FIRST..)
        .map(|((name, data), seq)| Event {
            seq,
            at: seq,
            name: name.into(),
            data,
        })
        .collect();
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
    settle(cx);
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

#[gpui_kit::test]
fn a_long_message_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![
            (
                "agent.prompted",
                json!({"text": format!("[flottille:user]\n{UNBROKEN}")}),
            ),
            (
                "agent.prompted",
                json!({"text": format!("[flottille:user]\n{SPACED}")}),
            ),
        ],
        cx,
    );
    let at = row(
        &page,
        |b| matches!(b, Block::Message { text, .. } if text == UNBROKEN),
        cx,
    );
    guard::assert_stays_inside(cx, format!("row-{at}-bubble"));
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

/// The row of the first block `of` picks, drawn: its group is unfolded.
fn row(
    page: &Entity<SessionPage>,
    of: impl Fn(&Block) -> bool,
    cx: &mut VisualTestContext,
) -> usize {
    let at = blocks(page, cx)
        .iter()
        .position(of)
        .expect("the row is in the conversation");
    unfold(page, at, cx);
    at
}

/// Opens what the row `at` folds, by its toggle, when it has one.
fn open(at: usize, toggle: &str, cx: &mut VisualTestContext) {
    let id = format!("row-{at}-{toggle}");
    if cx.update(|window, _| window.try_find(id.clone()).is_some()) {
        cx.update(|window, cx| window.click(id, cx));
        settle(cx);
    }
}

#[gpui_kit::test]
fn a_long_reply_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![
            (
                "agent.message",
                json!({"text": UNBROKEN, "message_id": "a"}),
            ),
            ("agent.message", json!({"text": SPACED, "message_id": "b"})),
        ],
        cx,
    );
    let at = row(
        &page,
        |b| matches!(b, Block::Text { text } if text == UNBROKEN),
        cx,
    );
    guard::assert_stays_inside(cx, format!("row-{at}-text"));
}

#[gpui_kit::test]
fn a_long_thought_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![
            (
                "agent.thought",
                json!({"text": format!("{UNBROKEN} {SPACED}")}),
            ),
            ("agent.message", json!({"text": "Done.", "message_id": "a"})),
        ],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::Thought { .. }), cx);
    open(at, "thought-toggle", cx);
    guard::assert_stays_inside(cx, format!("row-{at}-thought"));
}

#[gpui_kit::test]
fn a_long_report_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![(
            "agent.reported",
            json!({"text": format!("{UNBROKEN}\n\n{SPACED}")}),
        )],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::Report { .. }), cx);
    open(at, "report-toggle", cx);
    guard::assert_stays_inside(cx, format!("row-{at}-report"));
}

#[gpui_kit::test]
fn a_long_notice_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![(
            "agent.failed",
            json!({"reason": format!("{UNBROKEN} {SPACED}")}),
        )],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::Notice { .. }), cx);
    open(at, "notice-toggle", cx);
    guard::assert_stays_inside(cx, format!("row-{at}-notice"));
}

#[gpui_kit::test]
fn a_long_relayed_message_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    let wrote = format!(
        "[flottille:session]\n\"{UNBROKEN}\" (agent 0123) wrote to this session:\n\n{UNBROKEN}\n{SPACED}"
    );
    show(
        &page,
        vec![(
            "agent.prompted",
            json!({"text": wrote, "deliveries": ["d1"]}),
        )],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::Relayed { .. }), cx);
    open(at, "relayed-toggle", cx);
    guard::assert_stays_inside(cx, format!("row-{at}-relayed-card"));
}

#[gpui_kit::test]
fn a_long_ask_of_a_led_session_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![(
            "agent.asked",
            json!({"to": "pilot", "question": format!("{UNBROKEN} {SPACED}")}),
        )],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::Asked { .. }), cx);
    guard::assert_stays_inside(cx, format!("row-{at}-asked"));
}

#[gpui_kit::test]
fn a_long_vault_request_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![(
            "vault.asked",
            json!({"name": UNBROKEN, "project": UNBROKEN, "description": SPACED, "why": format!("{UNBROKEN} {SPACED}"),
                "takes_username": true}),
        )],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::VaultAsk(_)), cx);
    guard::assert_stays_inside(cx, format!("row-{at}-vault-ask"));
}

#[gpui_kit::test]
fn a_long_link_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![(
            "agent.link_shown",
            json!({"agent": "x", "target": format!("https://example.test/{UNBROKEN}{UNBROKEN}"),
                "title": format!("{UNBROKEN} {SPACED}"), "host": "example.test"}),
        )],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::Links { .. }), cx);
    guard::assert_stays_inside(cx, format!("row-{at}-links"));
}

#[gpui_kit::test]
fn a_long_step_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    let call = |id: &str, title: &str, input: Value| {
        (
            "agent.tool_call",
            json!({"id": id, "title": title, "status": "completed", "raw_input": input}),
        )
    };
    show(
        &page,
        vec![
            call(
                "c1",
                "run",
                json!({"line": format!("echo {UNBROKEN} {SPACED}")}),
            ),
            call(
                "c2",
                "fs_read",
                json!({"path": format!("{UNBROKEN}/{UNBROKEN}.rs")}),
            ),
            call(
                "c3",
                "session_send",
                json!({"text": format!("{UNBROKEN} {SPACED}")}),
            ),
        ],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::Steps { .. }), cx);
    open(at, "steps-toggle", cx);
    guard::assert_stays_inside(cx, format!("row-{at}-steps"));
}

#[gpui_kit::test]
fn a_long_delivery_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    show(
        &page,
        vec![(
            "agent.prompted",
            json!({"text": format!("[flottille:reminder]\n{UNBROKEN} {SPACED}\n{SPACED}"),
                "deliveries": ["d1"]}),
        )],
        cx,
    );
    let at = row(
        &page,
        |b| matches!(b, Block::Notice { kind, .. } if kind == "reminder"),
        cx,
    );
    open(at, "notice-toggle", cx);
    guard::assert_stays_inside(cx, format!("row-{at}-notice"));
}

#[gpui_kit::test]
fn a_long_command_stays_inside_its_card(cx: &mut TestAppContext) {
    let user = user();
    let (page, cx) = page_of(&user, cx);
    let body = format!(
        "[flottille:approval]\nRequest #7 (run) has now done it. Its result:\n\nrun r7: (`echo {UNBROKEN} {SPACED}` exited with code 1 after 2.5 s)\n(lines 1-1 of 1)\n{SPACED}"
    );
    show(
        &page,
        vec![(
            "agent.prompted",
            json!({"text": body, "deliveries": ["d1"]}),
        )],
        cx,
    );
    let at = row(&page, |b| matches!(b, Block::Command(_)), cx);
    guard::assert_stays_inside(cx, format!("command-card-{at}"));
}
