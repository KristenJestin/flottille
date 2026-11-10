//! The project's message checks in its Settings (#465): the field of the Rules section that shows
//! the checks the file declares, saves them, says a line that cannot be used under itself and
//! removes them all. On GPUI's test platform in front of a real daemon, with a throwaway
//! repository and temporary XDG folders. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::message_checks::MessageChecks;
use flottille_window::project_settings::{ProjectSettingsPage, SECTIONS};
use flottille_window::shell::Selection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A project `desk` whose file declares one check.
fn desk(user: &User) {
    let folder = user.path("work/desk");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "desk\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.write_project_file(
        "desk",
        &format!(
            "folder = \"{}\"\nmessage_checks = [\"lint-a {{message}}\"]\n\n\
             [[repositories]]\npath = \".\"\n",
            folder.display()
        ),
    );
}

fn window<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = flottille_ui::omarchy::Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    (shell, cx)
}

/// Runs the window until `check` holds, as real frames would.
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

/// Lets every motion end, then draws.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Opens the project's settings on its Rules tab: the page and its message checks.
fn open_rules(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> (Entity<ProjectSettingsPage>, Entity<MessageChecks>) {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("desk".into()), window, cx)
    });
    let page = |cx: &mut VisualTestContext| -> Option<Entity<ProjectSettingsPage>> {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
    };
    until(shell, cx, "the settings are read", |cx| {
        page(cx).is_some_and(|page| page.read_with(cx, |page, _| page.is_loaded()))
    });
    let page = page(cx).unwrap();
    let rules = SECTIONS
        .iter()
        .position(|(key, _)| *key == "rules")
        .unwrap();
    page.update(cx, |page, cx| page.choose(rules, cx));
    settle(cx);
    let checks = page.read_with(cx, |page, _| page.message_checks().clone());
    (page, checks)
}

fn type_checks(checks: &Entity<MessageChecks>, text: &str, cx: &mut VisualTestContext) {
    let field = checks
        .read_with(cx, |checks, _| checks.field().cloned())
        .unwrap();
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(text, window, cx)));
    cx.run_until_parked();
}

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

fn file(user: &User) -> String {
    std::fs::read_to_string(user.project_file("desk")).unwrap()
}

#[gpui_kit::test]
fn the_settings_field_saves_the_message_checks(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    desk(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, checks) = open_rules(&shell, cx);

    // What the file declares, one line each, with nothing to save yet.
    until(&shell, cx, "the checks are shown", |cx| {
        checks.read_with(cx, |c, _| c.saved().is_some())
    });
    settle(cx);
    assert_eq!(
        checks.read_with(cx, |c, cx| c.typed(cx)),
        Some("lint-a {message}".to_owned())
    );
    assert!(!checks.read_with(cx, |c, cx| c.savable(cx)));
    cx.update(|window, _| {
        assert!(window.find("message-checks-text-desk").visible());
        assert_eq!(
            window.find("message-checks-field-desk-label").label(),
            Some("Checks, one command line each")
        );
        assert!(window.find("message-checks-save-desk").visible());
    });

    // Two lines, a blank one between them left out.
    type_checks(&checks, "lint-a {message}\n\nlint-b --edit {message}\n", cx);
    assert!(checks.read_with(cx, |c, cx| c.savable(cx)));
    cx.update(|window, cx| window.click("message-checks-save-desk", cx));
    until(&shell, cx, "the checks are saved", |cx| {
        checks.read_with(cx, |c, _| c.saved().is_some_and(|saved| saved.len() == 2))
    });
    assert!(
        file(&user)
            .contains("message_checks = [\"lint-a {message}\", \"lint-b --edit {message}\"]"),
        "{}",
        file(&user)
    );
    assert!(toasts(&shell, cx).contains(&"Message checks saved".to_owned()));
    assert_eq!(
        checks.read_with(cx, |c, cx| c.typed(cx)),
        Some("lint-a {message}\nlint-b --edit {message}".to_owned())
    );

    // A line that names no message is said under the field, and nothing is written.
    type_checks(&checks, "lint-c --strict", cx);
    cx.update(|window, cx| window.click("message-checks-save-desk", cx));
    until(&shell, cx, "the refusal is said", |cx| {
        checks.read_with(cx, |c, _| c.problem().is_some())
    });
    settle(cx);
    cx.update(|window, _| {
        let error = window.find("message-checks-field-desk-error");
        assert!(error.visible());
        assert!(
            error.label().unwrap().contains("{message}"),
            "{:?}",
            error.label()
        );
    });
    assert!(file(&user).contains("lint-b"), "{}", file(&user));
    type_checks(&checks, "", cx);

    // None removes them.
    cx.update(|window, cx| window.click("message-checks-save-desk", cx));
    until(&shell, cx, "the checks are removed", |cx| {
        checks.read_with(cx, |c, _| c.saved().is_some_and(Vec::is_empty))
    });
    assert!(!file(&user).contains("message_checks"), "{}", file(&user));
    settle(cx);
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
    let shown = user.call("project.show", json!({"name": "desk"})).unwrap();
    assert!(shown.get("message_checks").is_none(), "{shown}");
}
