//! The leaves of a session in the window (#463), on GPUI's test platform in front of a real daemon
//! whose agents are the fake ACP agent: the rail lists the leave the user gave for a session to
//! write to another, on both sessions' pages, and Withdraw takes it back through the daemon, its
//! row folding away. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use flottille_window::Shell;
use flottille_window::leaves::LeaveRow;
use flottille_window::session::SessionPage;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A session the user starts in `app`, its first turn ended: its id and MCP session.
fn root(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    (id, Mcp::initialize(&url, &authorization))
}

/// The project `app` over a throwaway repository.
fn app(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
}

/// A first message from `mcp`'s session to `session`, held for the user's leave: its request.
fn first_send(mcp: &Mcp, session: &str) -> u64 {
    let held = mcp.call(
        "session_send",
        json!({"session": session, "text": "Hello."}),
    );
    held.text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", held.text))
}

/// Two sessions of `app`, the first let by the user write to the second: their ids.
fn pair(user: &User) -> (String, String) {
    app(user);
    let (writer, mcp) = root(user);
    let (session, _) = root(user);
    let number = first_send(&mcp, &session);
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    eventually("the leave", || store.has_leave(&writer, &session).unwrap());
    (writer, session)
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn open(shell: &Entity<Shell>, cx: &mut VisualTestContext, session: &str) -> Entity<SessionPage> {
    until(shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(session)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(session), cx));
    until(shell, cx, "the session's page shows", |cx| {
        page(shell, cx).is_some_and(|page| page.read_with(cx, |page, _| page.id() == session))
    });
    page(shell, cx).unwrap()
}

fn page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<SessionPage>> {
    shell.read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
}

fn rows(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<LeaveRow> {
    page.read_with(cx, |page, cx| page.leaves(cx).read(cx).rows().to_vec())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

#[gpui_kit::test]
fn the_leave_is_listed_on_both_sessions_and_can_be_withdrawn(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_allowing_judge();
    let (writer, session) = pair(&user);
    let key = format!("{writer}-{session}");
    let (shell, cx) = window(&user, cx);

    // The session written to says who may write to it.
    let shown = open(&shell, cx, &session);
    until(&shell, cx, "the rail lists the leave", |cx| {
        !rows(&shown, cx).is_empty()
    });
    settle(cx);
    assert_eq!(
        label(cx, &format!("leave-said-{key}")).unwrap(),
        format!("agent {writer} may write here")
    );
    assert_eq!(label(cx, "rail-leaves-title").unwrap(), "Write access");
    assert!(
        label(cx, &format!("leave-since-{key}"))
            .unwrap()
            .starts_with("allowed ")
    );
    assert_eq!(
        label(cx, &format!("leave-withdraw-{key}")).unwrap(),
        "Withdraw"
    );

    // The writer's own page says where it may write, and Withdraw takes it back.
    let shown = open(&shell, cx, &writer);
    until(&shell, cx, "the writer's rail lists the leave", |cx| {
        !rows(&shown, cx).is_empty()
    });
    settle(cx);
    assert_eq!(
        label(cx, &format!("leave-said-{key}")).unwrap(),
        format!("May write to agent {session}")
    );

    cx.update(|window, cx| window.click(format!("leave-withdraw-{key}"), cx));
    until(&shell, cx, "the leave is gone", |cx| {
        rows(&shown, cx).is_empty()
    });
    // It folds away before it is gone.
    cx.update(|window, cx| window.simulate_next_frame(cx));
    assert!(
        present(cx, &format!("leave-said-{key}")),
        "the row left at once"
    );
    settle(cx);

    assert!(!present(cx, &format!("leave-said-{key}")));
    assert!(!present(cx, "rail-leaves-title"));
    let listed = user
        .call("leave.list", json!({"session": session}))
        .unwrap();
    assert_eq!(listed["leaves"], json!([]), "{listed}");
}

#[gpui_kit::test]
fn the_leave_card_says_what_allowing_it_gives(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_allowing_judge();
    app(&user);
    let (writer, mcp) = root(&user);
    let (session, _) = root(&user);
    let number = first_send(&mcp, &session);
    let (shell, cx) = window(&user, cx);
    open(&shell, cx, &writer);
    until(&shell, cx, "the card is shown", |cx| {
        present(cx, &format!("allow-{number}"))
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("approval-details-{number}"), cx));
    settle(cx);

    assert_eq!(
        label(cx, &format!("approval-base-{number}")).as_deref(),
        Some(flottille_window::session::LEAVE_SAYS)
    );
    assert_eq!(
        flottille_window::session::LEAVE_SAYS,
        "Flottille always asks you this, in every mode: allowing it lets this session write to \
         that one until either ends or you withdraw it."
    );
}

#[gpui_kit::test]
fn a_refused_withdrawal_is_said_in_a_toast(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_allowing_judge();
    let (_, session) = pair(&user);
    let (shell, cx) = window(&user, cx);
    let shown = open(&shell, cx, &session);

    shown.update(cx, |page, cx| {
        page.leave_answered(Some("there is no such leave".into()), cx)
    });
    settle(cx);

    let toasts: Vec<(String, String)> = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| {
                (
                    toast.title().to_owned(),
                    toast.second_line().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    });
    assert_eq!(
        toasts,
        [(
            "The write access was not withdrawn".to_owned(),
            "there is no such leave".to_owned()
        )]
    );
    assert!(!present(cx, "rail-leaves-error"));
}
