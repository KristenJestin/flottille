//! The compaction in the window (#309), on GPUI's test platform in front of a real daemon whose
//! agents and compactor are fake ACP agents: the Compaction tab of Settings, the notice of a
//! compactor Flottille picked, the session's menu entry that compacts, and the conversation's line
//! that unfolds to the summary. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::session::SessionPage;
use flottille_window::settings::SettingsPage;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const SUMMARY: &str = "## Goal\nAdd the login page.\n## Next steps\nWire the form.";

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A user whose agents are the fake agent, `FULL` reporting a context past the threshold once,
/// and whose compactor answers `SUMMARY`.
fn user(settings: &str) -> User {
    let user = User::with_fake_agent();
    let long = "the form posts to the API and shows its errors. ".repeat(60);
    user.fake_script(json!({"turns": [
        {"when": "FULL", "times": 1, "do": [{"say": long}, {"usage": {"used": 150_000, "size": 200_000}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user.with_compactor(json!([{"say": SUMMARY}]), settings)
}

/// A project `app` and a root agent working in its folder: its id.
fn start(user: &User, message: &str) -> String {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    answer["agent"]["id"].as_str().unwrap().to_owned()
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
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
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

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some_and(|f| f.visible()))
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn open_compaction_tab(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SettingsPage> {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    cx.update(|window, cx| window.click("settings-tabs-compaction", cx));
    until(shell, cx, "the compaction is read", |cx| {
        page.read_with(cx, |p, _| p.compaction().is_some())
    });
    settle(cx);
    page
}

fn shown(user: &User) -> Value {
    user.call("compaction.show", json!({})).unwrap()
}

#[gpui_kit::test]
fn the_compaction_tab_shows_the_compactor_its_settings_and_its_guidance(cx: &mut TestAppContext) {
    let user = user("");
    let (shell, cx) = window(&user, cx);
    let page = open_compaction_tab(&shell, cx);

    for id in [
        "compaction-title",
        "compactor-model",
        "compactor-effort",
        "compaction-automatic",
        "compaction-threshold",
        "compaction-maximum",
        "compaction-kept",
        "compactor-cost",
        "compaction-guidance-title",
        "compaction-guidance-text",
        "compaction-guidance-reset",
        "compaction-guidance-save",
    ] {
        assert!(visible(cx, id), "{id}");
    }
    assert!(visible(cx, "compaction-guidance-reach"));

    // The switch asks the daemon, which keeps it.
    cx.update(|window, cx| window.click("compaction-automatic", cx));
    until(&shell, cx, "the daemon keeps it", |_| {
        shown(&user)["automatic"] == false
    });
    until(&shell, cx, "the page follows", |cx| {
        page.read_with(cx, |p, _| p.compaction().is_some_and(|c| !c.automatic))
    });

    // None folds the settings away, with their motion: still drawn a frame after.
    page.update(cx, |page, cx| {
        page.set_compaction(json!({"compactor": "none"}), cx)
    });
    until(&shell, cx, "no compactor", |cx| {
        page.read_with(cx, |p, _| {
            p.compaction().is_some_and(|c| c.compactor.is_none())
        })
    });
    one_frame(cx);
    assert!(present(cx, "compaction-threshold"), "folding away");
    settle(cx);
    assert!(!present(cx, "compaction-threshold"));
    assert_eq!(shown(&user)["compactor"], Value::Null);
}

#[gpui_kit::test]
fn a_compactor_flottille_picked_is_said_once_in_a_notice_that_opens_settings(
    cx: &mut TestAppContext,
) {
    // Claude installed (the fake `claude`) and signed in: the daemon picks its smallest model,
    // as the judge and as the compactor. The judge's notice comes first.
    let user = User::with_fake_claude();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the judge's notice is shown", |cx| {
        shell.read_with(cx, |s, _| s.judge_notice().is_some())
    });
    settle(cx);
    assert!(!present(cx, "compaction-notice"), "one notice at a time");
    cx.update(|window, cx| window.click("judge-notice-dismiss", cx));
    until(&shell, cx, "the compactor's notice is shown", |cx| {
        shell.read_with(cx, |s, _| s.compaction_notice().is_some())
    });
    settle(cx);
    let said = label(cx, "compaction-notice-text").unwrap();
    assert!(
        said.contains("Flottille chose Claude haiku as your compactor"),
        "{said}"
    );

    cx.update(|window, cx| window.click("compaction-notice-open", cx));
    until(&shell, cx, "the settings show the compaction", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).settings_page().is_some_and(|page| {
                page.read(cx).tab()
                    == flottille_window::settings::SECTIONS
                        .iter()
                        .position(|(key, _)| *key == "compaction")
                        .unwrap()
            })
        })
    });
    until(&shell, cx, "the daemon dropped the notice", |_| {
        shown(&user)["compactor"] == json!({"provider": "claude", "model": "haiku"})
    });
    settle(cx);
    assert!(!present(cx, "compaction-notice"));
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn open_session(
    shell: &Entity<Shell>,
    id: &str,
    cx: &mut VisualTestContext,
) -> Entity<SessionPage> {
    until(shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(id)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(id), cx));
    until(shell, cx, "the session's page shows", |cx| {
        session_page(shell, cx).read_with(cx, |page, cx| {
            !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
        })
    });
    session_page(shell, cx)
}

/// The row of the compaction's line, as the page numbers it.
fn compacted_row(
    page: &Entity<SessionPage>,
    cx: &mut VisualTestContext,
) -> Option<(usize, String)> {
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .enumerate()
            .find_map(|(at, b)| match b {
                Block::Notice { kind, text } if kind == "compacted" => Some((at, text.clone())),
                _ => None,
            })
    })
}

#[gpui_kit::test]
fn a_compaction_is_one_line_that_unfolds_to_the_summary(cx: &mut TestAppContext) {
    let user = user("");
    let id = start(&user, "add the login page FULL");
    eventually("the compaction", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.compacted")
    });
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);
    until(&shell, cx, "the line is in the conversation", |cx| {
        compacted_row(&page, cx).is_some()
    });
    settle(cx);
    let (at, _) = compacted_row(&page, cx).unwrap();

    let line = label(cx, &format!("row-{at}-notice-summary")).unwrap();
    assert!(
        line.starts_with("Conversation compacted · 150k → "),
        "{line}"
    );
    assert!(!present(cx, &format!("row-{at}-notice-text")), "folded");

    cx.update(|window, cx| window.click(format!("row-{at}-notice-toggle"), cx));
    one_frame(cx);
    assert!(present(cx, &format!("row-{at}-notice-text")), "unfolding");
    settle(cx);
    let text = label(cx, &format!("row-{at}-notice-text")).unwrap();
    assert!(text.contains("Wire the form."), "{text}");

    cx.update(|window, cx| window.click(format!("row-{at}-notice-toggle"), cx));
    settle(cx);
    assert!(
        !present(cx, &format!("row-{at}-notice-text")),
        "and folds again"
    );
}

#[gpui_kit::test]
fn the_session_menu_compacts_the_conversation(cx: &mut TestAppContext) {
    let user = user("automatic = false");
    let id = start(&user, "add the login page FULL");
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);
    settle(cx);

    cx.update(|window, cx| window.click("rail-agent-menu", cx));
    settle(cx);
    assert!(visible(cx, "rail-agent-compact"));
    cx.update(|window, cx| window.click("rail-agent-compact", cx));
    until(&shell, cx, "the compaction's line", |cx| {
        compacted_row(&page, cx).is_some()
    });
}
