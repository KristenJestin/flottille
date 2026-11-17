//! Each repository's commit checks in the project's Settings (#796): the Instructions tab lists
//! each repository's checks, proposes the lines its hooks hold (those with shell syntax left out
//! and said), adds one to its field with Add, its row folding away, and saves; a line that cannot
//! be used is said under the fields. On GPUI's test platform in front of a real daemon, with
//! throwaway repositories and temporary XDG folders. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::commit_checks::CommitChecks;
use flottille_window::link::Connect;
use flottille_window::project_settings::{ProjectSettingsPage, SECTIONS};
use flottille_window::shell::Selection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A project `desk` of two repositories, `api` whose husky hooks hold two lines and one with
/// shell syntax, and `web` with none.
fn desk(user: &User) {
    let folder = user.path("work/desk");
    for (name, hooks) in [
        (
            "api",
            &[
                (
                    "pre-commit",
                    "# lint\npre-lint --staged\npre-fail && echo done\n",
                ),
                ("commit-msg", "msg-lint --edit \"$1\"\n"),
            ][..],
        ),
        ("web", &[][..]),
    ] {
        let repository = folder.join(name);
        user.git_init(&repository);
        std::fs::write(repository.join("README.md"), "desk\n").unwrap();
        for (hook, text) in hooks {
            let husky = repository.join(".husky");
            std::fs::create_dir_all(&husky).unwrap();
            std::fs::write(husky.join(hook), text).unwrap();
        }
        user.git(&repository, &["add", "."]);
        user.git(&repository, &["commit", "--quiet", "-m", "start"]);
    }
    user.call("project.add", json!({"folder": folder, "name": "desk"}))
        .unwrap();
}

fn window<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = gpui_design_system::omarchy::Environment {
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

/// One frame, 16 ms on: a motion is under way.
fn frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// Opens the project's settings on its Instructions tab: the page and its commit checks, read.
fn open(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> (Entity<ProjectSettingsPage>, Entity<CommitChecks>) {
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
    let instructions = SECTIONS
        .iter()
        .position(|(key, _)| *key == "instructions")
        .unwrap();
    page.update(cx, |page, cx| page.choose(instructions, cx));
    let checks = page.read_with(cx, |page, _| page.commit_checks().clone());
    until(shell, cx, "the commit checks are read", |cx| {
        checks.read_with(cx, |c, _| c.repositories().is_some())
    });
    settle(cx);
    (page, checks)
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

/// The id of `part` (`proposal`, `add`, `left-out`) of the row of api's proposal whose line is
/// `line`.
fn row(
    checks: &Entity<CommitChecks>,
    line: &str,
    part: &str,
    cx: &mut VisualTestContext,
) -> String {
    let id = checks.read_with(cx, |c, _| {
        c.repositories().unwrap()[0]
            .proposals()
            .iter()
            .find(|proposal| proposal.line == line)
            .map(|proposal| proposal.id())
            .unwrap_or_else(|| panic!("{line} is not proposed"))
    });
    format!("commit-checks-{part}-{id}-desk-api")
}

fn typed(checks: &Entity<CommitChecks>, hook: &str, cx: &mut VisualTestContext) -> String {
    let field = checks.read_with(cx, |c, _| c.field("api", hook)).unwrap();
    cx.update(|_, cx| field.read(cx).value().to_string())
}

#[gpui_kit::test]
fn the_project_s_commit_checks_are_listed_proposed_and_saved(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    desk(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, checks) = open(&shell, cx);
    let ids = |line: &str, cx: &mut VisualTestContext| {
        ["proposal", "add", "left-out"].map(|part| row(&checks, line, part, cx))
    };
    let [pre_row, pre_add, _] = ids("pre-lint --staged", cx);
    let [msg_row, msg_add, _] = ids("msg-lint --edit {message_file}", cx);
    let [_, left_add, left_said] = ids("pre-fail && echo done", cx);

    // Each repository, named, with its two labelled fields; nothing to save at rest.
    cx.update(|window, _| {
        assert!(window.find("commit-checks-desk-sentence").visible());
        for path in ["api", "web"] {
            assert_eq!(
                window
                    .find(ElementId::from(format!("commit-checks-name-desk-{path}")))
                    .label(),
                Some(path)
            );
            assert_eq!(
                window
                    .find(ElementId::from(format!(
                        "commit-checks-pre-commit-field-desk-{path}-label"
                    )))
                    .label(),
                Some("Before the commit (pre-commit)")
            );
            assert_eq!(
                window
                    .find(ElementId::from(format!(
                        "commit-checks-commit-msg-field-desk-{path}-label"
                    )))
                    .label(),
                Some("On its message (commit-msg)")
            );
            assert!(
                window
                    .try_find(ElementId::from(format!("commit-checks-save-desk-{path}")))
                    .is_none(),
                "no Save at rest"
            );
        }
        // What api's hooks propose, from their file; the line with shell syntax said, no Add.
        assert_eq!(
            window.find(ElementId::from(pre_row.clone())).label(),
            Some("pre-lint --staged")
        );
        assert_eq!(
            window.find(ElementId::from(msg_row.clone())).label(),
            Some("msg-lint --edit {message_file}")
        );
        assert!(window.find(ElementId::from(pre_add.clone())).visible());
        let left = window.find(ElementId::from(left_said.clone()));
        let said = left.label().unwrap();
        assert!(
            said.starts_with("Left out of .husky/pre-commit: "),
            "{said}"
        );
        assert!(said.contains("is shell syntax"), "{said}");
        assert!(window.try_find(ElementId::from(left_add.clone())).is_none());
        // web's hooks propose nothing: no block.
        assert!(
            window
                .try_find("commit-checks-proposals-desk-web")
                .is_none()
        );
    });

    // Add puts the line in its field; its row folds away, drawn while it folds, and Save unfolds.
    cx.update(|window, cx| window.click(ElementId::from(pre_add.clone()), cx));
    frame(cx);
    assert_eq!(typed(&checks, "pre-commit", cx), "pre-lint --staged");
    cx.update(|window, _| {
        assert!(
            window.try_find(ElementId::from(pre_row.clone())).is_some(),
            "the row went at once"
        );
    });
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find(ElementId::from(pre_row.clone())).is_none());
        assert!(window.find("commit-checks-save-desk-api").visible());
    });
    cx.update(|window, cx| window.click(ElementId::from(msg_add.clone()), cx));
    settle(cx);
    assert_eq!(
        typed(&checks, "commit-msg", cx),
        "msg-lint --edit {message_file}"
    );

    cx.update(|window, cx| window.click("commit-checks-save-desk-api", cx));
    until(&shell, cx, "the checks are saved", |cx| {
        checks.read_with(cx, |c, _| {
            c.repositories().unwrap()[0].saved(0) == ["pre-lint --staged".to_owned()]
        })
    });
    assert!(
        file(&user).contains(
            "commit_checks = { pre_commit = [\"pre-lint --staged\"], commit_msg = [\"msg-lint \
             --edit {message_file}\"] }"
        ),
        "{}",
        file(&user)
    );
    until(&shell, cx, "the toast says it is saved", |cx| {
        toasts(&shell, cx).contains(&"Commit checks of api saved".to_owned())
    });
    assert!(toasts(&shell, cx).contains(&"Commit checks of api saved".to_owned()));
    settle(cx);
    cx.update(|window, _| {
        assert!(window.try_find("commit-checks-save-desk-api").is_none());
        // Saved, they are no longer proposed; the line left out stays said.
        assert!(window.try_find(ElementId::from(msg_row.clone())).is_none());
        assert!(window.find(ElementId::from(left_said.clone())).visible());
    });

    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

/// What the hooks propose is a block of its own, which holds the space above it (#884): it sits
/// where the gap of the repository's column put it, under the Save slot.
#[gpui_kit::test]
fn what_the_hooks_propose_sits_where_the_gap_put_it(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    desk(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    open(&shell, cx);

    let (repository, proposals) = cx.update(|window, _| {
        (
            window.find("commit-checks-repository-desk-api").bounds(),
            window.find("commit-checks-proposals-desk-api").bounds(),
        )
    });
    let under = f32::from(proposals.top() - repository.top());
    // Measured on `origin/dev`.
    assert_eq!(under, 278.0);
}

#[gpui_kit::test]
fn a_refused_pre_commit_line_is_said_under_the_pre_commit_field(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    desk(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, checks) = open(&shell, cx);

    // A line with shell syntax is refused under its own field, and nothing is written.
    let field = checks
        .read_with(cx, |c, _| c.field("web", "pre-commit"))
        .unwrap();
    cx.update(|window, cx| {
        field.update(cx, |field, cx| {
            field.set_value("lint | tee out", window, cx)
        })
    });
    settle(cx);
    cx.update(|window, cx| window.click("commit-checks-save-desk-web", cx));
    until(&shell, cx, "the refusal is said", |cx| {
        checks.read_with(cx, |c, _| c.repositories().unwrap()[1].problem().is_some())
    });
    settle(cx);
    cx.update(|window, _| {
        let error = window.find("commit-checks-pre-commit-field-desk-web-error");
        assert!(error.visible());
        let said = error.label().unwrap();
        assert!(said.contains("the pre-commit check"), "{said}");
        assert!(said.contains("nothing was saved"), "{said}");
        assert!(
            window
                .try_find("commit-checks-commit-msg-field-desk-web-error")
                .is_none_or(|error| !error.visible())
        );
    });
    assert!(!file(&user).contains("lint | tee"), "{}", file(&user));
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}
