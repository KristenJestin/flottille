//! A provider's models are in the composer before any session opened (#307, `docs/agents.md`,
//! "The models"). On GPUI's test platform in front of a real daemon whose providers are fake ACP
//! agents, with a throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::composer::{Composer, Menu};
use flottille_window::link::Connect;
use flottille_window::shell::Selection;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn window<'a>(
    user: &User,
    connect: Connect,
    width: f32,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(width), px(800.)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
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
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn menu_labels(composer: &Entity<Composer>, menu: Menu, cx: &mut VisualTestContext) -> Vec<String> {
    composer.read_with(cx, |c, _| {
        c.entries(menu).into_iter().map(|e| e.label).collect()
    })
}

#[gpui_kit::test]
fn the_composer_offers_the_models_before_any_session(cx: &mut TestAppContext) {
    let user = User::with_fake_providers().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.provider_script(
        "claude",
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}),
    );
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();

    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("atlas".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == "atlas")
        })
    });
    settle(cx);
    let composer = shell
        .read_with(cx, |s, _| s.composer_of("atlas").cloned())
        .expect("the page has a composer");

    // No session ever opened: the daemon read the models, and the window shows them as they
    // come (the existing live update).
    until(&shell, cx, "the models are offered", |cx| {
        composer.read_with(cx, |c, _| {
            c.entries(Menu::Model)
                .iter()
                .any(|e| e.label == "fake-large")
        })
    });
    settle(cx);

    assert_eq!(
        menu_labels(&composer, Menu::Model, cx),
        ["fake-small", "fake-large"]
    );
    // The provider's real default is named, and already selected.
    let default = composer.read_with(cx, |c, _| c.entries(Menu::Model)[0].clone());
    assert!(default.on);
    assert_eq!(default.note.as_deref(), Some("default"));
    let agents = user.call("agent.list", serde_json::Value::Null).unwrap();
    assert_eq!(agents["agents"], json!([]), "no session was opened");
}
