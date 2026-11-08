//! The scenarios of #585, the mode chosen when a session starts: the composer that starts one
//! offers the same Ask | Auto | Full control as a session's composer, on Auto each time it is shown
//! and after each start, and the session starts in the mode chosen. On GPUI's test platform in
//! front of a real daemon whose agents are the fake ACP agent, with a throwaway user. No window
//! opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::composer::{Choice, Composer};
use flottille_window::link::Connect;
use flottille_window::shell::Selection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A user with the project `atlas` over a throwaway repository, and no judge chosen.
fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
    user
}

fn window<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1000.)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
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

/// Lets every motion end, then draws: the window at rest.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Opens `atlas`'s page, as a click in the tree would, and lets its entrance end.
fn open_atlas(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("atlas".into()), window, cx)
    });
    until(shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(cx);
}

/// `atlas`'s page, open in a window once the providers are read, with its composer.
fn atlas<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, Entity<Composer>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), cx);
    until(
        &shell,
        cx,
        "the project and the providers are known",
        |cx| {
            shell.read_with(cx, |s, _| {
                !s.snapshot().projects.is_empty() && s.providers().known
            })
        },
    );
    open_atlas(&shell, cx);
    let composer = shell
        .read_with(cx, |s, _| s.composer_of("atlas").cloned())
        .expect("atlas's page has a composer");
    (shell, composer, cx)
}

/// The mode the composer's control shows, and whether it is drawn in the alarm colour.
fn shown(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> (Option<String>, bool) {
    composer.read_with(cx, |c, cx| {
        let mode = c.mode().read(cx);
        (mode.mode().map(str::to_owned), mode.alarm())
    })
}

/// The control is drawn on Auto, in the composer's bar beside Start, with the no-judge warning.
fn on_auto_in_the_bar(composer: &Entity<Composer>, cx: &mut VisualTestContext) {
    assert_eq!(shown(composer, cx), (Some("auto".into()), false));
    cx.update(|window, _| {
        assert_eq!(window.find("start-mode-auto").selected(), Some(true));
        assert_eq!(window.find("start-mode-ask").selected(), Some(false));
        assert_eq!(window.find("start-mode-full").selected(), Some(false));
        assert_eq!(
            window.find("start-mode-warning").label(),
            Some(flottille_window::mode::NO_JUDGE)
        );
        let control = window.find("start-mode").bounds();
        let send = window.find("send").bounds();
        assert!(control.right() <= send.left(), "{control:?} {send:?}");
        assert!(
            (control.center().y - send.center().y).abs() < px(4.),
            "in the bar of Start: {control:?} {send:?}"
        );
    });
}

fn type_task(text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_input(text);
}

#[gpui_kit::test]
fn the_composer_that_starts_a_session_offers_ask_auto_full_on_auto(cx: &mut TestAppContext) {
    let user = user();
    user.call(
        "workspace.create",
        json!({"project": "atlas", "name": "login"}),
    )
    .unwrap();
    let (shell, composer, cx) = atlas(&user, cx);

    // On the project's page.
    on_auto_in_the_bar(&composer, cx);

    // "New session", from Settings.
    shell.update_in(cx, |s, window, cx| s.open(Selection::Settings, window, cx));
    settle(cx);
    shell.update_in(cx, |s, window, cx| s.new_session(window, cx));
    settle(cx);
    on_auto_in_the_bar(&composer, cx);

    // "New session in login", from the workspace's card.
    let deadline = Instant::now() + Duration::from_secs(20);
    while user
        .call(
            "workspace.show",
            json!({"project": "atlas", "name": "login"}),
        )
        .unwrap()["state"]
        != "ready"
    {
        assert!(Instant::now() < deadline, "the workspace is never ready");
        std::thread::sleep(Duration::from_millis(50));
    }
    until(&shell, cx, "the window knows the workspace", |cx| {
        shell.read_with(cx, |s, _| {
            s.snapshot()
                .projects
                .iter()
                .any(|p| p.workspaces.iter().any(|w| w == "login"))
        })
    });
    shell.update_in(cx, |s, window, cx| {
        s.compose_in("atlas", Choice::Workspace("login".into()), window, cx)
    });
    settle(cx);
    on_auto_in_the_bar(&composer, cx);

    // Full is drawn in the alarm colour.
    cx.update(|window, cx| window.click("start-mode-full", cx));
    settle(cx);
    assert_eq!(shown(&composer, cx), (Some("full".into()), true));
    cx.update(|window, _| {
        assert_eq!(window.find("start-mode-full").selected(), Some(true));
        assert!(window.try_find("start-mode-warning").is_none());
    });
    // Choosing it starts nothing and asks the daemon nothing.
    let agents = user.call("agent.list", json!({})).unwrap()["agents"].clone();
    assert_eq!(agents.as_array().unwrap().len(), 0, "{agents}");

    // Shown again, the composer is back on Auto: a choice never waits for a later session.
    shell.update_in(cx, |s, window, cx| s.open(Selection::Settings, window, cx));
    settle(cx);
    open_atlas(&shell, cx);
    on_auto_in_the_bar(&composer, cx);
}

#[gpui_kit::test]
fn after_a_start_the_composer_is_back_on_auto(cx: &mut TestAppContext) {
    let user = user();
    let (shell, composer, cx) = atlas(&user, cx);

    cx.update(|window, cx| window.click("start-mode-full", cx));
    type_task("Fix the login", cx);
    cx.update(|window, cx| window.click("send", cx));
    until(&shell, cx, "the session started", |cx| {
        !composer.read_with(cx, |c, _| c.is_sending())
            && user.call("agent.list", json!({})).unwrap()["agents"]
                .as_array()
                .is_some_and(|agents| agents.len() == 1)
    });

    let agents = user.call("agent.list", json!({})).unwrap()["agents"].clone();
    let id = agents[0]["id"].as_str().unwrap();
    assert_eq!(
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["gate_mode"],
        "full",
        "the session started in the mode chosen"
    );
    assert_eq!(shown(&composer, cx), (Some("auto".into()), false));
    open_atlas(&shell, cx);
    on_auto_in_the_bar(&composer, cx);
}
