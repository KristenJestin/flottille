//! The exceptions a session holds, in the window (#1220), on GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent: the card of an exception asked for says what
//! allowing it gives, the rail lists the exception the user accepted, and Withdraw takes it back
//! through the daemon, its row folding away. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use flottille_window::Shell;
use flottille_window::exceptions::ExceptionRow;
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

/// The project `app` over a throwaway repository pushed to a throwaway bare remote, and its ready
/// workspace `w`, on the branch `app/w`.
fn app(user: &User) {
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
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
    user.call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
}

/// A session the user starts in the workspace `w`, its first turn ended: its id and MCP session.
fn root(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "hello"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    (id, Mcp::initialize(&url, &authorization))
}

/// The forced push of its own branch, asked for as an exception: its request.
fn ask(mcp: &Mcp) -> u64 {
    let held = mcp.call(
        "run",
        json!({"line": "git push --force origin HEAD:refs/heads/app/w",
            "exception": "a check refuses an old commit the squash drops"}),
    );
    held.text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", held.text))
}

/// A session of `app` that holds the exception `force-push`: its id.
fn excepted(user: &User) -> String {
    app(user);
    let (agent, mcp) = root(user);
    let number = ask(&mcp);
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let store = Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap();
    eventually("the exception", || {
        store.exception(&agent, "force-push").unwrap().is_some()
    });
    agent
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

fn rows(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<ExceptionRow> {
    page.read_with(cx, |page, cx| page.exceptions(cx).read(cx).rows().to_vec())
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
fn the_exception_is_listed_on_its_session_and_can_be_withdrawn(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_allowing_judge();
    let agent = excepted(&user);
    let (shell, cx) = window(&user, cx);

    let shown = open(&shell, cx, &agent);
    until(&shell, cx, "the rail lists the exception", |cx| {
        !rows(&shown, cx).is_empty()
    });
    settle(cx);
    assert_eq!(label(cx, "rail-exceptions-title").unwrap(), "Exceptions");
    assert_eq!(
        label(cx, &format!("exception-said-{agent}-force-push")).unwrap(),
        "Force-push its own branch"
    );
    assert_eq!(
        label(cx, &format!("exception-why-{agent}-force-push")).unwrap(),
        "a check refuses an old commit the squash drops"
    );
    // What it covers: the branch the user allowed, no other.
    assert_eq!(
        label(cx, &format!("exception-covers-{agent}-force-push")).unwrap(),
        "for the branch app/w"
    );
    assert!(
        label(cx, &format!("exception-since-{agent}-force-push"))
            .unwrap()
            .starts_with("allowed ")
    );
    assert_eq!(
        label(cx, &format!("exception-withdraw-{agent}-force-push")).unwrap(),
        "Withdraw"
    );

    cx.update(|window, cx| window.click(format!("exception-withdraw-{agent}-force-push"), cx));
    until(&shell, cx, "the exception is gone", |cx| {
        rows(&shown, cx).is_empty()
    });
    // It folds away before it is gone.
    cx.update(|window, cx| window.simulate_next_frame(cx));
    assert!(
        present(cx, &format!("exception-said-{agent}-force-push")),
        "the row left at once"
    );
    settle(cx);

    assert!(!present(cx, &format!("exception-said-{agent}-force-push")));
    assert!(!present(cx, "rail-exceptions-title"));
    let listed = user
        .call("exception.list", json!({"session": agent}))
        .unwrap();
    assert_eq!(listed["exceptions"], json!([]), "{listed}");
}

#[gpui_kit::test]
fn the_exception_card_says_what_allowing_it_gives(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_allowing_judge();
    app(&user);
    let (agent, mcp) = root(&user);
    let number = ask(&mcp);
    let (shell, cx) = window(&user, cx);
    open(&shell, cx, &agent);
    until(&shell, cx, "the card is shown", |cx| {
        present(cx, &format!("allow-{number}"))
    });
    settle(cx);

    cx.update(|window, cx| window.click(format!("approval-details-{number}"), cx));
    settle(cx);

    assert_eq!(
        label(cx, &format!("approval-base-{number}")).as_deref(),
        Some(flottille_window::session::EXCEPTION_SAYS)
    );
}

#[gpui_kit::test]
fn a_refused_withdrawal_is_said_in_a_toast(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_allowing_judge();
    let agent = excepted(&user);
    let (shell, cx) = window(&user, cx);
    let shown = open(&shell, cx, &agent);

    shown.update(cx, |page, cx| {
        page.exception_answered(Some("there is no such exception".into()), cx)
    });
    settle(cx);

    until(&shell, cx, "the refusal is said in a toast", |cx| {
        shell.read_with(cx, |s, cx| {
            s.toasts().read(cx).toasts().any(|toast| {
                toast.title() == "The exception was not withdrawn"
                    && toast.second_line().unwrap_or_default() == "there is no such exception"
            })
        })
    });
}

/// A sub-agent `mcp`'s session starts, titled `worker`: its id and MCP session.
fn child(user: &User, mcp: &Mcp) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let launched = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "worker", "brief": "Push the branch."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(user, opened);
    let child = Mcp::initialize(&url, &authorization);
    let listed = user.call("agent.list", json!({})).unwrap()["agents"].clone();
    let id = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["title"] == "worker")
        .and_then(|agent| agent["id"].as_str())
        .unwrap_or_else(|| panic!("no worker in {listed}"))
        .to_owned();
    (id, child)
}

#[gpui_kit::test]
fn the_exception_of_a_sub_agent_is_listed_on_the_session_with_its_holder(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_allowing_judge();
    app(&user);
    let (agent, mcp) = root(&user);
    let (worker, child_mcp) = child(&user, &mcp);
    // The sub-agent asks; its parent forwards to the user, who allows it.
    let held = child_mcp.call(
        "run",
        json!({"line": "git push --force origin HEAD:refs/heads/app/w",
            "exception": "a check refuses an old commit the squash drops"}),
    );
    let number: u64 = held
        .text
        .strip_prefix("Waiting for approval, request #")
        .and_then(|rest| rest.split_once(|c: char| !c.is_ascii_digit()))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", held.text));
    let forwarded = mcp.call(
        "approval_answer",
        json!({"request_id": number, "decision": "forward", "why": "the user decides"}),
    );
    assert!(!forwarded.error, "{}", forwarded.text);
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let store = Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap();
    eventually("the exception", || {
        store.exception(&worker, "force-push").unwrap().is_some()
    });

    let (shell, cx) = window(&user, cx);
    let shown = open(&shell, cx, &agent);
    until(
        &shell,
        cx,
        "the rail lists the sub-agent's exception",
        |cx| rows(&shown, cx).iter().any(|row| row.session == worker),
    );
    settle(cx);
    assert_eq!(
        label(cx, &format!("exception-holder-{worker}-force-push")).unwrap(),
        "held by worker"
    );

    cx.update(|window, cx| window.click(format!("exception-withdraw-{worker}-force-push"), cx));
    until(&shell, cx, "the exception is gone", |cx| {
        rows(&shown, cx).is_empty()
    });
    settle(cx);
    assert!(!present(cx, &format!("exception-said-{worker}-force-push")));
    assert!(store.exception(&worker, "force-push").unwrap().is_none());
}
