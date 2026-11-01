//! The scenarios of a wake in the window (#254), on GPUI's test platform in front of a real daemon
//! whose agents are the fake ACP agent: the rail lists the wake with its time and a Cancel, the
//! session reads as waiting on it, and the delivery says the timer woke the agent. No window opens
//! on a screen; the daemon's clock is moved by the test.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::session::{SessionPage, clock};
use flottille_window::subscriptions::SubscriptionRow;
use flottille_window::tree::session_id;
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

/// A project `app`, and an agent the user started in it whose first turn ended, which asked to be
/// woken in 20 minutes: its id and MCP session.
fn waking(user: &User) -> (String, Mcp) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    let mcp = Mcp::initialize(&url, &authorization);
    let answer = mcp.call("wake", json!({"minutes": 20, "why": "the deploy of api"}));
    assert!(!answer.error, "{}", answer.text);
    (id, mcp)
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
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(&shell, cx, "the session's page shows", |cx| {
        session_page(&shell, cx).read_with(cx, |page, cx| {
            !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
        })
    });
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

fn one_frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        one_frame(cx);
    }
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn rows(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<SubscriptionRow> {
    page.read_with(cx, |page, cx| {
        page.subscriptions(cx).read(cx).rows().to_vec()
    })
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

/// When the daemon says the wake is due.
fn due(user: &User, lead: &str) -> u64 {
    user.call("subscription.list", json!({"tree": lead}))
        .unwrap()["subscriptions"][0]["expires_at"]
        .as_u64()
        .unwrap()
}

#[gpui_kit::test]
fn the_rail_shows_the_wake_with_its_time_and_cancel(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (lead, _mcp) = waking(&user);
    let at = clock(due(&user, &lead));
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the wake", |cx| {
        !rows(&page, cx).is_empty()
    });
    settle(cx);

    let id = rows(&page, cx)[0].id.clone();
    assert_eq!(
        label(cx, &format!("subscription-pull-{id}")).as_deref(),
        Some("Wake: the deploy of api")
    );
    assert_eq!(
        label(cx, &format!("subscription-times-{id}")),
        Some(format!("at {at}"))
    );
    assert!(
        !present(cx, &format!("subscription-events-{id}")),
        "a wake has no events to name"
    );
    assert_eq!(
        label(cx, &format!("subscription-cancel-{id}")).as_deref(),
        Some("Cancel")
    );
}

#[gpui_kit::test]
fn cancel_removes_the_wake_and_its_row_folds_away(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (lead, _mcp) = waking(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);
    until(&shell, cx, "the rail lists the wake", |cx| {
        !rows(&page, cx).is_empty()
    });
    settle(cx);
    let id = rows(&page, cx)[0].id.clone();

    cx.update(|window, cx| window.click(format!("subscription-cancel-{id}"), cx));
    until(&shell, cx, "the wake is gone", |cx| {
        rows(&page, cx).is_empty()
    });
    one_frame(cx);
    assert!(
        present(cx, &format!("subscription-pull-{id}")),
        "the row is still drawn while it folds away"
    );
    settle(cx);
    assert!(!present(cx, &format!("subscription-pull-{id}")));
    let listed = user
        .call("subscription.list", json!({"tree": lead}))
        .unwrap();
    assert_eq!(listed["subscriptions"], json!([]), "{listed}");

    user.call("test.advance", json!({"ms": 21 * 60_000}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let prompts = user.received_requests("session/prompt");
    assert!(
        prompts
            .iter()
            .all(|p| !p.to_string().contains("[flottille:subscription]")),
        "{prompts:?}"
    );
}

#[gpui_kit::test]
fn the_session_reads_as_waiting_on_the_wake_in_the_sidebar_and_the_rail(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (lead, _mcp) = waking(&user);
    let at = clock(due(&user, &lead));
    let (shell, cx) = open_session(&user, &lead, cx);

    let said = format!("Waiting: wake at {at}");
    until(&shell, cx, "the sidebar says it waits on the wake", |cx| {
        shell.read_with(cx, |shell, cx| {
            shell.session(&lead).is_some_and(|row| {
                let row = row.read(cx);
                row.mark() == Some(AgentState::Waiting) && row.said() == said
            })
        })
    });
    let line = format!("waiting: wake at {at}");
    until(&shell, cx, "the rail says it waits on the wake", |cx| {
        label(cx, &format!("rail-waiting-{lead}")).as_deref() == Some(line.as_str())
    });
}

#[gpui_kit::test]
fn the_delivery_of_a_wake_says_the_timer_woke_the_agent(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (lead, _mcp) = waking(&user);
    let (shell, cx) = open_session(&user, &lead, cx);
    let page = session_page(&shell, cx);

    user.call("test.advance", json!({"ms": 21 * 60_000}))
        .unwrap();
    let woken = |cx: &mut VisualTestContext| {
        page.read_with(cx, |page, cx| {
            page.conversation()
                .read(cx)
                .blocks()
                .iter()
                .find_map(|b| match b {
                    Block::Notice { kind, text } if kind == "woken by" => Some(text.clone()),
                    _ => None,
                })
        })
    };
    until(&shell, cx, "the step is in the conversation", |cx| {
        woken(cx).is_some()
    });
    until(&shell, cx, "the wake left the rail", |cx| {
        rows(&page, cx).is_empty()
    });
    let text = woken(cx).unwrap();
    assert_eq!(text.lines().next(), Some("the timer: the deploy of api"));
}
