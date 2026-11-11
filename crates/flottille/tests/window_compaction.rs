//! The compaction in the window (#309), on GPUI's test platform in front of a real daemon whose
//! agents and compactor are fake ACP agents: the compaction's page of Settings, the notice of a
//! compactor Flottille picked, the session's menu entry that compacts, and the conversation's line
//! that unfolds to the summary. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::session::SessionPage;
use flottille_window::settings::SettingsPage;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
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
    user_with(json!([{"say": SUMMARY}]), settings)
}

/// `user`, the compactor playing `steps`.
fn user_with(steps: Value, settings: &str) -> User {
    let user = User::with_fake_agent();
    let long = "the form posts to the API and shows its errors. ".repeat(60);
    user.fake_script(json!({"turns": [
        {"when": "FULL", "times": 1, "do": [{"say": long}, {"usage": {"used": 150_000, "size": 200_000}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user.with_compactor(steps, settings)
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
    cx.update(|cx| flottille_window::init_with(env, cx));
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

fn height(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| f32::from(window.find(id).bounds().size.height))
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
    cx.update(|window, cx| window.click("settings-tabs-features", cx));
    until(shell, cx, "the compaction is read", |cx| {
        page.read_with(cx, |p, _| p.compaction().is_some()) && present(cx, "features-compaction")
    });
    settle(cx);
    cx.update(|window, cx| window.click("features-compaction", cx));
    settle(cx);
    page
}

fn shown(user: &User) -> Value {
    user.call("compaction.show", json!({})).unwrap()
}

#[gpui_kit::test]
fn the_compaction_page_shows_the_compactor_its_settings_and_its_guidance(cx: &mut TestAppContext) {
    let user = user("");
    let (shell, cx) = window(&user, cx);
    let page = open_compaction_tab(&shell, cx);

    for id in [
        "compaction-switch",
        "compaction-usage",
        "compaction-summary-model",
        "compaction-automatic",
        "compaction-threshold",
        "compaction-summary-cost",
        "compaction-guidance-title",
        "compaction-guidance-text",
        "compaction-guidance-reset",
        "compaction-more",
    ] {
        assert!(visible(cx, id), "{id}");
    }
    assert!(visible(cx, "compaction-guidance-reach"));
    assert!(!present(cx, "compaction-guidance-save"), "no Save at rest");
    // The rarely set ones are folded under More settings.
    assert!(!visible(cx, "compaction-maximum"));
    cx.update(|window, cx| window.click("compaction-more", cx));
    settle(cx);
    for id in [
        "compaction-maximum",
        "compaction-kept",
        "compaction-before-expiry",
        "compaction-after-expiry",
        "compaction-minimum",
        "compaction-lifetimes",
    ] {
        assert!(visible(cx, id), "{id}");
    }
    let lifetimes = label(cx, "compaction-lifetimes").unwrap();
    assert!(
        lifetimes.contains("Claude: an hour") && lifetimes.contains("Codex: unknown"),
        "{lifetimes}"
    );

    // The switch asks the daemon, which keeps it.
    cx.update(|window, cx| window.click("compaction-automatic", cx));
    until(&shell, cx, "the daemon keeps it", |_| {
        shown(&user)["automatic"] == false
    });
    until(&shell, cx, "the page follows", |cx| {
        page.read_with(cx, |p, _| p.compaction().is_some_and(|c| !c.automatic))
    });

    // The switch turned off folds the settings away, with their motion: still drawn a frame
    // after.
    cx.update(|window, cx| window.click("compaction-switch", cx));
    until(&shell, cx, "no compactor", |cx| {
        page.read_with(cx, |p, _| {
            p.compaction().is_some_and(|c| c.compactor.is_none())
        })
    });
    one_frame(cx);
    assert!(present(cx, "compaction-threshold"), "fading out");
    settle(cx);
    assert!(!present(cx, "compaction-threshold"));
    assert_eq!(shown(&user)["compactor"], Value::Null);
}

#[gpui_kit::test]
fn the_summary_choices_are_named_in_the_users_words(cx: &mut TestAppContext) {
    let user = user("");
    let (shell, cx) = window(&user, cx);
    open_compaction_tab(&shell, cx);

    // One select: the session's own agent first, with its one line, then the models; never
    // "None", which is the switch's.
    assert_eq!(
        label(cx, "compaction-summary-field-label").as_deref(),
        Some("Summary written by")
    );
    cx.update(|window, cx| window.click("compaction-summary-model", cx));
    settle(cx);
    assert_eq!(
        label(cx, "compaction-summary-model-agent").as_deref(),
        Some("The session's own agent")
    );
    assert_eq!(
        label(cx, "compaction-summary-model-agent-line").as_deref(),
        Some("The agent summarises its own conversation, in its session.")
    );
    cx.update(|window, _| {
        assert!(window.try_find_by_label("None").is_none());
        assert!(
            window
                .try_find_by_label("None (each program compacts on its own)")
                .is_none()
        );
    });
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
        said.contains("Flottille chose Claude haiku as your compaction model"),
        "{said}"
    );

    cx.update(|window, cx| window.click("compaction-notice-open", cx));
    until(&shell, cx, "the settings show the compaction", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).settings_page().is_some_and(|page| {
                let page = page.read(cx);
                page.tab()
                    == flottille_window::settings::SECTIONS
                        .iter()
                        .position(|(key, _)| *key == "features")
                        .unwrap()
                    && page.open_feature() == Some("compaction")
            })
        })
    });
    one_frame(cx);
    assert!(present(cx, "compaction-notice"), "fading out");
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

/// `user` whose daemon's clock the test moves, an agent whose first turn left 150k tokens of
/// context (past the minimum size) at rest, and its session open in the window.
fn expiring<'a>(
    settings: &str,
    cx: &'a mut TestAppContext,
) -> (
    User,
    String,
    Entity<Shell>,
    Entity<SessionPage>,
    &'a mut VisualTestContext,
) {
    let user =
        user(&format!("automatic = false\n{settings}")).with_env("FLOTTILLE_TEST_CLOCK", "1");
    let id = start(&user, "add the login page FULL");
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);
    settle(cx);
    (user, id, shell, page, cx)
}

fn offer(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<bool> {
    page.read_with(cx, |page, cx| {
        page.reply().map(|r| r.read(cx).compact_offer())
    })
    .flatten()
}

fn compactions(user: &User, id: &str) -> usize {
    user.call("agent.show", json!({"agent": id})).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "agent.compacted")
        .count()
}

/// The toolbar's controls that stay: where each starts, and how far Send ends from the
/// composer's right edge.
fn fixed(cx: &mut VisualTestContext) -> ([gpui_kit::Pixels; 3], gpui_kit::Pixels) {
    cx.update(|window, _| {
        let left =
            ["reply-model", "reply-mode", "attach"].map(|id| window.find(id).bounds().origin.x);
        let edge = window.find("reply").bounds().right();
        (left, edge - window.find("reply-send").bounds().right())
    })
}

/// How wide the Compact control is drawn, and how opaque: the widest alpha of what is painted
/// inside it (its ticked fill).
fn compact_drawn(cx: &mut VisualTestContext) -> (f32, f32) {
    cx.update(|window, _| {
        let Some(found) = window.try_find("reply-compact".to_owned()) else {
            return (0.0, 0.0);
        };
        let bounds = found.bounds().scale(window.scale_factor());
        let alpha = window
            .painted_quads()
            .iter()
            .filter(|quad| bounds.contains(&quad.bounds.center()))
            .filter_map(|quad| quad.background.as_solid())
            .map(|bg| bg.a)
            .fold(0.0, f32::max);
        (f32::from(found.bounds().size.width), alpha)
    })
}

#[gpui_kit::test]
fn compact_shows_left_of_send_only_once_there_is_something_to_compact(cx: &mut TestAppContext) {
    let (user, _id, shell, page, cx) = expiring("", cx);
    assert_eq!(offer(&page, cx), None);
    // Nothing to compact: no control, so no disabled one and no tooltip that says why not.
    assert!(
        !present(cx, "reply-compact"),
        "not drawn while the cache is warm"
    );

    // Claude's cache lives an hour here: not yet expired at 59 minutes.
    user.call("test.advance", json!({"ms": 59 * 60_000}))
        .unwrap();
    settle(cx);
    assert!(!present(cx, "reply-compact"), "nor at 59 minutes");

    user.call("test.advance", json!({"ms": 2 * 60_000}))
        .unwrap();
    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    settle(cx);
    assert_eq!(offer(&page, cx), Some(true), "ticked from the setting");
    assert_eq!(label(cx, "reply-compact").as_deref(), Some("Compact"));
    let hint = page
        .read_with(cx, |page, cx| {
            page.reply().map(|r| r.read(cx).compact_hint())
        })
        .unwrap();
    assert!(hint.starts_with("Its prompt cache expired"), "{hint}");

    // At the right end of the toolbar, on its line, just left of Send.
    let (chip, mode, attach, send) = cx.update(|window, _| {
        (
            window.find("reply-compact").bounds(),
            window.find("reply-mode").bounds(),
            window.find("attach").bounds(),
            window.find("reply-send").bounds(),
        )
    });
    assert!(chip.left() >= attach.right(), "after Attach and the mode");
    assert!(chip.left() >= mode.right());
    assert!(chip.right() <= send.left(), "left of Send");
    assert!(
        send.left() - chip.right() <= px(12.),
        "just left of Send, not {:?} away",
        send.left() - chip.right()
    );
    assert!(
        (chip.center().y - send.center().y).abs() <= px(1.),
        "on the toolbar's line"
    );
    assert!(
        !present(cx, "reply-compact-line"),
        "no block above the toolbar"
    );
}

#[gpui_kit::test]
fn compact_unfolds_and_folds_away_animated(cx: &mut TestAppContext) {
    let (user, _id, shell, page, cx) = expiring("", cx);
    assert_eq!(compact_drawn(cx), (0.0, 0.0));
    user.call("test.advance", json!({"ms": 61 * 60_000}))
        .unwrap();
    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    let mut unfolding = Vec::new();
    for _ in 0..30 {
        one_frame(cx);
        unfolding.push(compact_drawn(cx));
    }
    let (width, opacity) = *unfolding.last().unwrap();
    assert!(width > 0.0 && opacity > 0.0, "it ends drawn: {unfolding:?}");
    assert!(
        unfolding
            .iter()
            .any(|&(w, o)| w > 0.0 && w < width && o > 0.0 && o < opacity),
        "halfway through, its width and opacity are between their ends: {unfolding:?}"
    );

    // Unticking is no way out; a turn that warms the cache again is.
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("go on");
    settle(cx);
    let reply = page.read_with(cx, |page, _| page.reply().cloned()).unwrap();
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "the chip leaves", |cx| {
        offer(&page, cx).is_none()
    });
    let mut folding = Vec::new();
    for _ in 0..30 {
        one_frame(cx);
        folding.push(compact_drawn(cx));
    }
    assert!(
        folding
            .iter()
            .any(|&(w, o)| w > 0.0 && w < width && o > 0.0 && o < opacity),
        "it folds away over frames, not at once: {folding:?}"
    );
    assert_eq!(*folding.last().unwrap(), (0.0, 0.0), "and is gone");
}

#[gpui_kit::test]
fn compact_leaves_once_there_is_nothing_to_compact_again(cx: &mut TestAppContext) {
    let (user, id, shell, page, cx) = expiring("", cx);
    user.call("test.advance", json!({"ms": 61 * 60_000}))
        .unwrap();
    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    settle(cx);
    assert!(visible(cx, "reply-compact"));

    // A turn warms the cache again.
    cx.update(|window, cx| window.click("reply-compact", cx));
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("go on");
    settle(cx);
    let reply = page.read_with(cx, |page, _| page.reply().cloned()).unwrap();
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));
    until(&shell, cx, "the chip leaves", |cx| {
        offer(&page, cx).is_none()
    });
    settle(cx);
    assert!(!present(cx, "reply-compact"), "not drawn again");
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["name"] == "agent.turn_ended")
            .count()
            == 2
    });
}

#[gpui_kit::test]
fn compact_coming_and_going_moves_no_other_control_and_keeps_the_composer_s_height(
    cx: &mut TestAppContext,
) {
    let (user, _id, shell, page, cx) = expiring("after_expiry = false", cx);
    let rest = height(cx, "reply");
    let before = fixed(cx);

    user.call("test.advance", json!({"ms": 61 * 60_000}))
        .unwrap();
    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    assert_eq!(
        offer(&page, cx),
        Some(false),
        "unticked, as the setting is off"
    );
    for _ in 0..30 {
        one_frame(cx);
        assert_eq!(height(cx, "reply"), rest, "the composer keeps its height");
        let (left, _) = fixed(cx);
        assert_eq!(left, before.0, "the controls before it stay where they are");
        let send = cx.update(|window, _| window.find("reply-send").bounds().right());
        let edge = cx.update(|window, _| window.find("reply").bounds().right());
        assert_eq!(edge - send, before.1, "and Send too");
    }
    assert_eq!(fixed(cx), before, "nor once it is there");
    assert_eq!(height(cx, "reply"), rest);
}

#[gpui_kit::test]
fn the_toolbar_never_overflows_the_composer_at_1280_px_with_compact_available(
    cx: &mut TestAppContext,
) {
    let (user, _id, shell, page, cx) = expiring("", cx);
    user.call("test.advance", json!({"ms": 61 * 60_000}))
        .unwrap();
    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    settle(cx);
    let (composer, send, chip) = cx.update(|window, _| {
        (
            window.find("reply").bounds(),
            window.find("reply-send").bounds(),
            window.find("reply-compact").bounds(),
        )
    });
    assert_eq!(
        composer.size.width,
        px(646.),
        "the composer of a 1280 px window"
    );
    assert!(
        send.right() <= composer.right(),
        "Send ends {:?} past the composer's edge",
        send.right() - composer.right()
    );
    assert!(chip.right() <= send.left(), "Compact stays before Send");
}

#[gpui_kit::test]
fn unticking_the_chip_sends_without_compacting(cx: &mut TestAppContext) {
    let (user, id, shell, page, cx) = expiring("", cx);
    user.call("test.advance", json!({"ms": 61 * 60_000}))
        .unwrap();
    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    assert_eq!(offer(&page, cx), Some(true), "ticked from the setting");
    settle(cx);
    let before = fixed(cx);

    cx.update(|window, cx| window.click("reply-compact", cx));
    settle(cx);
    assert_eq!(offer(&page, cx), Some(false));
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("go on");
    settle(cx);
    let reply = page.read_with(cx, |page, _| page.reply().cloned()).unwrap();
    reply.update_in(cx, |reply, window, cx| reply.send(window, cx));

    // The turn starts in the same session, and the control leaves.
    until(&shell, cx, "the chip leaves", |cx| {
        offer(&page, cx).is_none()
    });
    settle(cx);
    assert!(!present(cx, "reply-compact"), "it is not drawn again");
    assert_eq!(fixed(cx), before, "and moves none of the others");
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["name"] == "agent.turn_ended")
            .count()
            == 2
    });
    assert_eq!(compactions(&user, &id), 0);
}

const HANDOFF: &str = "## Task\nAdd the login page.\n## About to do\nWire the form.";

#[gpui_kit::test]
fn the_conversation_shows_one_divider_with_the_sizes_and_the_summary_folded(
    cx: &mut TestAppContext,
) {
    // The agent writes its own handoff (#405): no compactor.
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "flottille:handoff", "do": [{"say": HANDOFF}]},
        {"when": "FULL", "times": 1, "do": [{"say": "Done."}, {"usage": {"used": 192_000, "size": 200_000}}]},
        {"when": "your_handoff", "do": [{"say": "Picking up."}, {"usage": {"used": 9_000, "size": 200_000}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "summary = \"agent\"\nthreshold = 90\n",
    )
    .unwrap();
    let id = start(&user, "add the login page FULL");
    eventually("the handoff", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.compacted")
    });
    let (shell, cx) = window(&user, cx);

    // The compaction's page says who writes the summary, and keeps its settings without a compactor.
    let page = open_compaction_tab(&shell, cx);
    assert!(visible(cx, "compaction-summary-model"));
    assert!(visible(cx, "compaction-threshold"));
    page.read_with(cx, |p, _| {
        assert!(p.compaction().is_some_and(|c| c.by_agent));
    });

    let session = open_session(&shell, &id, cx);
    until(&shell, cx, "the line is in the conversation", |cx| {
        compacted_row(&session, cx).is_some()
    });
    settle(cx);
    let lines = session.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .filter(|b| matches!(b, Block::Notice { kind, .. } if kind == "compacted"))
            .count()
    });
    assert_eq!(lines, 1, "one divider");
    let (at, _) = compacted_row(&session, cx).unwrap();
    let line = label(cx, &format!("row-{at}-notice-summary")).unwrap();
    assert!(
        line.starts_with("Conversation compacted · 192k → "),
        "{line}"
    );
    assert!(!present(cx, &format!("row-{at}-notice-text")), "folded");
    cx.update(|window, cx| window.click(format!("row-{at}-notice-toggle"), cx));
    one_frame(cx);
    assert!(present(cx, &format!("row-{at}-notice-text")), "unfolding");
    settle(cx);
    let text = label(cx, &format!("row-{at}-notice-text")).unwrap();
    assert!(text.contains("Wire the form."), "{text}");
}

/// The compactor is at work: it was given its prompt and holds until `release` is written.
fn compactor_at_work(user: &User) {
    eventually("the compactor at work", || {
        !user.compactor_received().is_empty()
    });
}

fn release(user: &User) {
    std::fs::write(user.path("fake-compactor/release"), "").unwrap();
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

fn step_row(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<usize> {
    blocks(page, cx)
        .iter()
        .position(|b| matches!(b, Block::Compacting { .. }))
}

fn said_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> String {
    shell.read_with(cx, |s, _| {
        s.snapshot().session(id).unwrap().mark_said.clone()
    })
}

#[gpui_kit::test]
fn a_compaction_that_runs_is_a_live_step_and_the_session_says_compacting(cx: &mut TestAppContext) {
    let user = user_with(json!([{"hold": "release"}, {"say": SUMMARY}]), "");
    let id = start(&user, "add the login page FULL");
    compactor_at_work(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);

    until(&shell, cx, "the step that runs", |cx| {
        step_row(&page, cx).is_some()
    });
    settle(cx);
    let at = step_row(&page, cx).unwrap();
    assert_eq!(
        label(cx, &format!("row-{at}-compaction")).as_deref(),
        Some("Compacting the conversation")
    );
    assert!(
        page.read_with(cx, |page, cx| page.conversation().read(cx).rows()[at]
            .read(cx)
            .compaction_head(cx))
            .is_some_and(|head| head.starts_with("Compacting the conversation ")),
        "with its elapsed time"
    );

    // The session's state says it, in the sidebar and the rail, and is no longer idle.
    until(&shell, cx, "the session says it compacts", |cx| {
        said_of(&shell, &id, cx) == "Compacting"
    });
    assert_eq!(
        label(cx, &format!("rail-waiting-{id}")).as_deref(),
        Some("Compacting the conversation")
    );

    // It folds into the mark once done, in the same row.
    release(&user);
    until(&shell, cx, "the mark", |cx| {
        compacted_row(&page, cx).is_some()
    });
    settle(cx);
    assert_eq!(step_row(&page, cx), None, "no step left running");
    assert_eq!(compacted_row(&page, cx).map(|(row, _)| row), Some(at));
    // The shell reads the sessions again on the event: the state follows a moment after the row.
    until(&shell, cx, "the session no longer says it compacts", |cx| {
        said_of(&shell, &id, cx) != "Compacting"
    });
    settle(cx);
    assert!(
        !present(cx, &format!("rail-waiting-{id}")),
        "the line folded away"
    );
}

#[gpui_kit::test]
fn a_message_sent_during_a_compaction_says_it_waits_for_the_compaction(cx: &mut TestAppContext) {
    let user = user_with(json!([{"hold": "release"}, {"say": SUMMARY}]), "");
    let id = start(&user, "add the login page FULL");
    compactor_at_work(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);
    until(&shell, cx, "the step that runs", |cx| {
        step_row(&page, cx).is_some()
    });
    user.call(
        "agent.send",
        json!({"agent": id, "message": "and add the logout page"}),
    )
    .unwrap();

    let waiting = |page: &Entity<SessionPage>, cx: &mut VisualTestContext| -> Option<String> {
        page.read_with(cx, |page, cx| {
            page.queued()
                .read(cx)
                .messages()
                .first()
                .map(|m| m.delivery.clone())
        })
    };
    until(&shell, cx, "the message waits", |cx| {
        waiting(&page, cx).is_some()
    });
    settle(cx);
    let delivery = waiting(&page, cx).unwrap();
    assert_eq!(
        label(cx, &format!("queued-{delivery}-waits")).as_deref(),
        Some("Queued · after the compaction")
    );

    release(&user);
    until(&shell, cx, "the message is sent", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Message { text, .. } if text == "and add the logout page"))
    });
}

#[gpui_kit::test]
fn a_compaction_that_fails_says_so_and_the_queued_message_is_sent_without_it(
    cx: &mut TestAppContext,
) {
    let user = user_with(json!([{"hold": "release"}, {"die": 3}]), "");
    let id = start(&user, "add the login page FULL");
    compactor_at_work(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);
    until(&shell, cx, "the step that runs", |cx| {
        step_row(&page, cx).is_some()
    });
    user.call(
        "agent.send",
        json!({"agent": id, "message": "and add the logout page"}),
    )
    .unwrap();
    release(&user);

    until(&shell, cx, "the error line", |cx| {
        blocks(&page, cx).iter().any(|b| {
            matches!(b, Block::Notice { kind, text } if kind == "compaction failed"
                && text.starts_with("The conversation was not compacted: "))
        })
    });
    settle(cx);
    assert_eq!(step_row(&page, cx), None, "the step stopped running");
    until(&shell, cx, "the session no longer says it compacts", |cx| {
        said_of(&shell, &id, cx) != "Compacting"
    });
    until(&shell, cx, "the message is sent all the same", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Message { text, .. } if text == "and add the logout page"))
    });
    assert!(
        compacted_row(&page, cx).is_none(),
        "it was not compacted: the session went on as it was"
    );
}

/// The messages of the user the conversation shows, in order.
fn user_messages(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<String> {
    blocks(page, cx)
        .into_iter()
        .filter_map(|b| match b {
            Block::Message { from, text, .. } if from == "you" => Some(text),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn after_a_compaction_the_brief_is_not_shown_again_as_a_message_of_the_user(
    cx: &mut TestAppContext,
) {
    let user = user_with(json!([{"hold": "release"}, {"say": SUMMARY}]), "");
    let id = start(&user, "add the login page FULL");
    compactor_at_work(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);
    until(&shell, cx, "the step that runs", |cx| {
        step_row(&page, cx).is_some()
    });
    // The user writes during the compaction, and after it.
    user.call(
        "agent.send",
        json!({"agent": id, "message": "and add the logout page"}),
    )
    .unwrap();
    release(&user);
    until(&shell, cx, "the message is sent", |cx| {
        user_messages(&page, cx).contains(&"and add the logout page".to_owned())
            && compacted_row(&page, cx).is_some()
    });
    user.call(
        "agent.send",
        json!({"agent": id, "message": "and the footer"}),
    )
    .unwrap();
    until(&shell, cx, "the message after", |cx| {
        user_messages(&page, cx).contains(&"and the footer".to_owned())
    });
    settle(cx);

    // Each message of the user once, in the order they wrote them: no brief again.
    assert_eq!(
        user_messages(&page, cx),
        [
            "add the login page FULL",
            "and add the logout page",
            "and the footer"
        ]
    );
    // After the line, the agent's next answer and the user's messages: nothing Flottille sent.
    let shown = blocks(&page, cx);
    assert!(
        !shown.iter().any(|b| matches!(b,
            Block::Notice { kind, .. } if kind == "resume" || kind == "brief")),
        "{shown:?}"
    );
}
