//! The "⋯" menus of the session (#590), on GPUI's test platform in front of a real daemon whose
//! agents are the fake ACP agent: the agent's menu in the rail has an icon of its own for each row
//! and offers Detach to a led session, and the sidebar's Detach is one short line whatever its
//! lead's title. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::Icon;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::session::SessionPage;
use flottille_window::tree::{session_detach_id, session_id, session_menu_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// What the lead's first message says: long, for the title the sidebar's Detach used to quote.
const LONG: &str = "begin the docs session and bring every route of the public API under the new guard before the release";

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]}
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

fn agents(user: &User) -> Vec<serde_json::Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

/// A lead the user started in the project `atlas` and the session it leads: their ids.
fn lead_and_led(user: &User) -> (String, String) {
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
    let lead = user
        .call("agent.start", json!({"project": "atlas", "message": LONG}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The launch comes to the user; the session starts once they allow it.
    eventually("the launch is held", || {
        !user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    let number = user.call("approval.list", json!({})).unwrap()["requests"][0]["number"]
        .as_u64()
        .unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the session exists", || agents(user).len() == 2);
    let led = agents(user)
        .into_iter()
        .find(|agent| agent["led_by"] == lead.as_str())
        .expect("a session led by the lead")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, led)
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
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn said(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

/// The session `id`'s page open, its rail drawn with its agent's row.
fn open_session<'a>(
    user: &User,
    id: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session's row", |cx| {
        present(cx, &session_id(id).to_string())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(&shell, cx, "the agent's menu in the rail", |cx| {
        let _ = session_page(&shell, cx);
        present(cx, "rail-agent-menu")
    });
    settle(cx);
    (shell, cx)
}

#[gpui_kit::test]
fn the_agents_menu_rows_have_distinct_icons(cx: &mut TestAppContext) {
    let user = user();
    let (_, led) = lead_and_led(&user);
    let (shell, cx) = open_session(&user, &led, cx);

    let rows = session_page(&shell, cx).read_with(cx, |page, cx| {
        page.rail().read(cx).agent_menu().read(cx).entries()
    });
    let icon = |id: &str| {
        rows.iter()
            .find(|(row, _, _)| row.to_string() == id)
            .unwrap_or_else(|| panic!("the row {id} in {rows:?}"))
            .2
    };
    assert_eq!(icon("rail-agent-reload-tools"), Some(Icon::Restart));
    assert_eq!(icon("rail-agent-compact"), Some(Icon::Fold));
    assert_eq!(icon("rail-agent-end"), Some(flottille_window::glyphs::STOP));
    let mut icons: Vec<_> = rows.iter().map(|(_, _, icon)| *icon).collect();
    assert!(icons.iter().all(Option::is_some), "an icon on every row");
    icons.sort_by_key(|icon| format!("{icon:?}"));
    icons.dedup();
    assert_eq!(icons.len(), rows.len(), "no two rows share an icon");
}

#[gpui_kit::test]
fn the_rail_menu_of_a_led_session_offers_detach(cx: &mut TestAppContext) {
    let user = user();
    let (lead, led) = lead_and_led(&user);
    let (shell, cx) = open_session(&user, &led, cx);

    cx.update(|window, cx| window.click("rail-agent-menu", cx));
    settle(cx);
    assert!(present(cx, "rail-agent-detach"), "Detach is in the menu");
    for row in [
        "rail-agent-reload-tools",
        "rail-agent-compact",
        "rail-agent-end",
    ] {
        let height = cx.update(|window, _| window.find(row.to_owned()).bounds().size.height);
        assert!(height < px(36.), "{row} is one line: {height:?}");
    }
    cx.update(|window, cx| window.click("rail-agent-detach", cx));
    let shown = || user.call("agent.show", json!({"agent": led})).unwrap();
    until(&shell, cx, "the daemon detached it", |_| {
        shown()["agent"]["detached_from"] == lead.as_str()
    });
    until(&shell, cx, "the menu forgets Detach", |cx| {
        settle(cx);
        cx.update(|window, cx| window.click("rail-agent-menu", cx));
        settle(cx);
        let offered = present(cx, "rail-agent-detach");
        let open = present(cx, "rail-agent-end");
        cx.update(|window, cx| window.click("rail-agent-menu", cx));
        open && !offered
    });
}

#[gpui_kit::test]
fn a_session_nobody_leads_has_no_detach_in_its_rail_menu(cx: &mut TestAppContext) {
    let user = user();
    let (lead, _) = lead_and_led(&user);
    let (_, cx) = open_session(&user, &lead, cx);

    cx.update(|window, cx| window.click("rail-agent-menu", cx));
    settle(cx);
    assert!(present(cx, "rail-agent-end"), "the menu is open");
    assert!(!present(cx, "rail-agent-detach"));
}

#[gpui_kit::test]
fn detach_in_the_sidebar_menu_is_one_line_whatever_the_leads_title(cx: &mut TestAppContext) {
    let user = user();
    let (_, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led session's row", |cx| {
        present(cx, &session_id(&led).to_string())
    });
    settle(cx);

    cx.update(|window, cx| window.hover(session_id(&led), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_menu_id(&led), cx));
    settle(cx);
    let detach = session_detach_id(&led).to_string();
    let row = cx.update(|window, _| window.find(detach.clone()).bounds());
    assert!(row.size.height < px(56.), "{row:?} is two lines at most");
    assert_eq!(
        said(cx, &format!("{detach}-label")).as_deref(),
        Some("Detach from its lead")
    );
    // The lead's title is the fainter second line, one line high.
    let title = said(cx, &format!("{detach}-detail")).expect("the lead's title");
    assert!(title.contains("begin the docs session"), "{title}");
    let detail = cx.update(|window, _| window.find(format!("{detach}-detail")).bounds());
    assert!(detail.size.height < px(24.), "{detail:?} is one line");
    let label = cx.update(|window, _| window.find(format!("{detach}-label")).bounds());
    assert!(label.size.height < px(24.), "{label:?} is one line");
}
