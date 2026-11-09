//! A spent quota in the window (`docs/design.md` §5.9), on GPUI's test platform in front of a real
//! daemon whose agent is the fake ACP agent answering with Claude's words: the session's state said
//! once, `Out of quota, resets at 13:00`, in the sidebar and the rail; the composer's chip
//! `Resume at 13:00` for this session alone; and the setting of Settings, Sessions, off by
//! default. Each unfolds and folds away, and the page rests. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::session::{SessionPage, clock};
use flottille_window::settings::SettingsPage;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The step in which the provider reports a spent limit, as the bridge does at the end of a turn.
fn limit(message: &str) -> Value {
    json!({"update": {"sessionUpdate": "session_info_update", "_meta": {"jetbrains": {"air":
        {"sessionFailure": {"severity": "error", "category": "limit", "message": message}}}}}})
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "hello", "do": [
            limit("You've hit your weekly limit · resets 1pm (Europe/Paris)")]},
        {"when": "unreadable", "do": [limit("You've hit your limit · resets whenever")]},
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

/// A project `app` and a root agent whose first turn spent its quota: its id and its reset.
fn spent(user: &User) -> (String, u64) {
    let id = start(user, "hello");
    let reset = quota(user, &id)["reset_at"].as_u64().unwrap();
    (id, reset)
}

/// The same, whose first message is `message`: the id, once the wait is stored.
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
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the wait", || quota(user, &id).is_object());
    id
}

fn quota(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["quota"].clone()
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
    cx.simulate_resize(size(px(1280.), px(1600.)));
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
    for _ in 0..60 {
        one_frame(cx);
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some_and(|f| f.visible()))
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

fn mark_said(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> String {
    shell.read_with(cx, |s, _| {
        s.snapshot().session(id).unwrap().mark_said.clone()
    })
}

fn offer(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<bool> {
    page.read_with(cx, |page, cx| {
        page.reply().map(|r| r.read(cx).quota_offer())
    })
    .flatten()
}

#[gpui_kit::test]
fn a_session_out_of_quota_says_it_once_in_the_sidebar_and_the_rail(cx: &mut TestAppContext) {
    let user = user();
    let (id, reset) = spent(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);
    let words = format!("Out of quota, resets at {}", clock(reset));

    until(&shell, cx, "the sidebar says it", |cx| {
        mark_said(&shell, &id, cx) == words
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.snapshot().session(&id).unwrap().mark),
        Some(flottille_ui::AgentState::Waiting),
        "waiting, not idle"
    );
    until(&shell, cx, "the rail says it", |cx| {
        label(cx, &format!("rail-waiting-{id}")).as_deref() == Some(words.as_str())
    });
    settle(cx);
    // Said once on the page: the rail's line, nowhere else.
    let said = page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .filter(|block| format!("{block:?}").contains("Out of quota"))
            .count()
    });
    assert_eq!(said, 0, "the conversation does not say it again");
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);

    // A message of the user's ends the wait: the line folds away.
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    until(&shell, cx, "the sidebar no longer says it", |cx| {
        !mark_said(&shell, &id, cx).starts_with("Out of quota")
    });
    // It folds away with its words, then is gone.
    until(&shell, cx, "the rail's line folds away", |cx| {
        one_frame(cx);
        !present(cx, &format!("rail-waiting-{id}"))
    });
}

#[gpui_kit::test]
fn the_composer_chip_shows_in_the_toolbar_only_while_the_reset_is_known(cx: &mut TestAppContext) {
    let user = user();
    let (id, reset) = spent(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);

    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    assert_eq!(offer(&page, cx), Some(false), "off, as the setting is");
    // It fades in, in its own room.
    one_frame(cx);
    assert!(present(cx, "reply-quota"), "fading in");
    settle(cx);
    assert!(visible(cx, "reply-quota"));
    assert_eq!(
        label(cx, "reply-quota"),
        Some(format!("Resume at {}", clock(reset)))
    );

    // In the toolbar, on the line of the mode and before the send button, and no block of
    // its own under the field.
    let (chip, mode, attach, send) = cx.update(|window, _| {
        (
            window.find("reply-quota").bounds(),
            window.find("reply-mode").bounds(),
            window.find("attach").bounds(),
            window.find("reply-send").bounds(),
        )
    });
    assert!(chip.left() >= attach.right(), "after the fixed controls");
    assert!(
        attach.left() >= mode.right(),
        "Attach stays beside the mode"
    );
    assert!(chip.right() <= send.left(), "before the send button");
    assert!(
        (chip.center().y - mode.center().y).abs() <= px(1.),
        "on the toolbar's line"
    );
    assert!(!present(cx, "reply-quota-line"), "no block under the field");
}

#[gpui_kit::test]
fn the_composer_chip_fades_out_when_the_wait_ends(cx: &mut TestAppContext) {
    let user = user();
    let (id, reset) = spent(&user);
    let (shell, cx) = window(&user, cx);
    // Wide enough for the whole toolbar with the chip: in a 646 px composer the pickers, Attach,
    // the chip and Send need 20 px more than the row has.
    cx.simulate_resize(size(px(1600.), px(1600.)));
    let page = open_session(&shell, &id, cx);
    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    settle(cx);
    assert!(visible(cx, "reply-quota"));
    let fixed = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            // Send is measured from the composer's right edge: the page may have gained a
            // scrollbar by then, the chip is not what moves it.
            let edge = window.find("reply").bounds().right();
            let left =
                ["reply-model", "reply-mode", "attach"].map(|id| window.find(id).bounds().origin.x);
            (left, edge - window.find("reply-send").bounds().right())
        })
    };
    let with_chip = fixed(cx);

    // The wait ends with a message of the user's: the chip fades out and is gone.
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    until(&shell, cx, "the chip leaves", |cx| {
        offer(&page, cx).is_none()
    });
    one_frame(cx);
    assert!(present(cx, "reply-quota"), "fading out");
    assert_eq!(
        label(cx, "reply-quota"),
        Some(format!("Resume at {}", clock(reset))),
        "it keeps its words while it leaves"
    );
    settle(cx);
    assert!(!present(cx, "reply-quota"));
    assert_eq!(
        fixed(cx),
        with_chip,
        "its coming and going moves no other control"
    );
}

#[gpui_kit::test]
fn the_composer_chip_is_hidden_while_the_reset_is_unknown(cx: &mut TestAppContext) {
    let user = user();
    let id = start(&user, "unreadable");
    assert!(quota(&user, &id)["reset_at"].is_null());
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);
    until(&shell, cx, "the rail says it", |cx| {
        label(cx, &format!("rail-waiting-{id}")).as_deref() == Some("Out of quota")
    });
    settle(cx);
    assert_eq!(offer(&page, cx), None);
    assert!(!present(cx, "reply-quota"));
}

#[gpui_kit::test]
fn toggling_the_composer_chip_resumes_for_this_session_alone(cx: &mut TestAppContext) {
    let user = user();
    let (id, _) = spent(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);

    until(&shell, cx, "the chip is offered", |cx| {
        offer(&page, cx).is_some()
    });
    assert_eq!(offer(&page, cx), Some(false), "off, as the setting is");
    settle(cx);

    cx.update(|window, cx| window.click("reply-quota", cx));
    assert_eq!(offer(&page, cx), Some(true), "on at once");
    until(&shell, cx, "the daemon takes it", |_| {
        quota(&user, &id)["resume"] == true
    });
    assert_eq!(
        user.call("sessions.show", json!({})).unwrap()["resume"],
        false,
        "the setting stays off"
    );
    settle(cx);
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

fn open_sessions_tab(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SettingsPage> {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click("settings-tabs-sessions", cx));
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    until(shell, cx, "the page knows the setting", |cx| {
        page.read_with(cx, |p, _| p.resume_said().is_some())
    });
    settle(cx);
    page
}

#[gpui_kit::test]
fn the_app_setting_is_off_by_default_and_turns_the_composer_s_switch_on(cx: &mut TestAppContext) {
    let user = user();
    let (id, _) = spent(&user);
    let (shell, cx) = window(&user, cx);
    let settings = open_sessions_tab(&shell, cx);

    assert_eq!(settings.read_with(cx, |p, _| p.resume_said()), Some(false));
    assert_eq!(
        label(cx, "sessions-resume-switch").as_deref(),
        Some("Resume sessions when the quota resets")
    );
    cx.update(|window, cx| window.click("sessions-resume-switch", cx));
    until(&shell, cx, "the daemon says on", |cx| {
        settings.read_with(cx, |p, _| p.resume_said() == Some(true))
    });
    assert_eq!(
        user.call("sessions.show", json!({})).unwrap()["resume"],
        true
    );
    settle(cx);
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);

    // The session follows the setting.
    let page = open_session(&shell, &id, cx);
    until(&shell, cx, "the switch is offered, on", |cx| {
        offer(&page, cx) == Some(true)
    });
}
