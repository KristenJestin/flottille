//! A spent quota in the window (`docs/design.md` §5.9), on GPUI's test platform in front of a real
//! daemon whose agent is the fake ACP agent answering with Claude's words: the session's state said
//! once, `Out of quota, resets at 13:00`, in the sidebar and the rail; the composer's switch
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

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "hello", "do": [
            {"say": "You've hit your weekly limit · resets 1pm (Europe/Paris)"}]},
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
    eventually("the wait", || quota(user, &id).is_object());
    let reset = quota(user, &id)["reset_at"].as_u64().unwrap();
    (id, reset)
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

fn height(cx: &mut VisualTestContext, id: &str) -> Option<f32> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        let found = window.try_find(id.to_owned())?;
        Some(f32::from(found.bounds().size.height))
    })
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
fn the_composer_switches_the_resume_for_this_session_alone(cx: &mut TestAppContext) {
    let user = user();
    let (id, reset) = spent(&user);
    let (shell, cx) = window(&user, cx);
    let page = open_session(&shell, &id, cx);

    until(&shell, cx, "the switch is offered", |cx| {
        offer(&page, cx).is_some()
    });
    assert_eq!(offer(&page, cx), Some(false), "off, as the setting is");
    let first = height(cx, "reply-quota").expect("drawn from the first frame");
    for _ in 0..4 {
        one_frame(cx);
    }
    let part_way = height(cx, "reply-quota").unwrap();
    settle(cx);
    let open = height(cx, "reply-quota").unwrap();
    assert!(
        first < part_way && part_way < open,
        "it unfolds: {first} then {part_way} then {open}"
    );
    assert_eq!(
        label(cx, "reply-quota-toggle"),
        Some(format!("Resume at {}", clock(reset)))
    );

    cx.update(|window, cx| window.click("reply-quota-toggle", cx));
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

    // A message of the user's ends the wait: the switch folds away.
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    until(&shell, cx, "the switch leaves", |cx| {
        offer(&page, cx).is_none()
    });
    one_frame(cx);
    assert!(present(cx, "reply-quota"), "folding away");
    settle(cx);
    assert!(!present(cx, "reply-quota"));
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
