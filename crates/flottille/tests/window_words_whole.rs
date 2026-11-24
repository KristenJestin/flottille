//! #1285: a word never wraps inside itself. The sidebar, the project page, a session with its
//! rail and every tab of Settings are laid out in a narrow window; no element named by one word
//! (a state, a repository, a button) may be drawn as a tall sliver, which is what a word cut
//! letter by letter looks like ("O" over "ff"). On GPUI's test platform in front of a real
//! daemon in temporary folders. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::tree::{project_id, session_id};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// Narrower than any window a user keeps, wide enough for the sidebar.
const NARROW: f32 = 360.;

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
        cx.executor().advance_clock(Duration::from_millis(16));
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
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
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

/// The elements drawn as a tall sliver: a word wrapped letter by letter ("O" over "ff") is
/// narrower than a few letters, taller than it is wide, and two lines high at least (a title
/// crushed to its ellipsis is one line high, and is another defect).
fn slivers(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| {
        window
            .elements()
            .iter()
            .filter(|element| element.visible())
            .filter_map(|element| {
                let bounds = element.bounds();
                let (wide, high) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                (wide > 0. && wide < 40. && high > 36. && high > 1.5 * wide)
                    .then(|| format!("{wide} x {high} px {:?}", element))
            })
            .collect()
    })
}

/// Draws the screen in a wide window and in a narrow one: what each cuts. The window is tall,
/// so that nothing is below its edge.
fn cut(cx: &mut VisualTestContext, screen: &str, all: &mut Vec<String>) {
    for width in [1280., NARROW] {
        cx.simulate_resize(size(px(width), px(2400.)));
        settle(cx);
        all.extend(
            slivers(cx)
                .into_iter()
                .map(|one| format!("{screen} at {width} px: {one}")),
        );
    }
    cx.simulate_resize(size(px(1280.), px(2400.)));
    settle(cx);
}

#[gpui_kit::test]
fn no_word_of_the_window_wraps_letter_by_letter_in_a_narrow_window(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    let dir = folder.join("api");
    user.git_init(&dir);
    std::fs::write(dir.join("README.md"), "api\n").unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &dir,
        &[
            "checkout",
            "--quiet",
            "-b",
            "fix/a-long-branch-name-of-a-fix",
        ],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let lead = answer["agent"]["id"].as_str().unwrap().to_owned();

    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the sidebar lists the session", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    settle(cx);
    let mut all = Vec::new();
    cut(cx, "the sidebar", &mut all);

    cx.update(|window, cx| window.click(project_id("app"), cx));
    settle(cx);
    cut(cx, "the project page", &mut all);

    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the rail names the repository", |cx| {
        present(cx, "rail-repository-api-value")
    });
    settle(cx);
    cut(cx, "a session and its rail", &mut all);

    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-general")
    });
    for tab in [
        "general",
        "agents",
        "approvals",
        "features",
        "machines",
        "repositories",
        "specs",
        "tasks",
        "vault",
        "workspaces",
    ] {
        let id = format!("settings-tabs-{tab}");
        if present(cx, &id) {
            cx.update(|window, cx| window.click(id, cx));
            // The checks of General are read once the tab is shown.
            until(&shell, cx, "the tab is drawn", |cx| {
                tab != "general"
                    || (present(cx, "check-flottille-provider-codex-fix")
                        && label(cx, "check-flottille-sandbox-state")
                            .is_some_and(|state| state != "Reading…"))
            });
            settle(cx);
            cut(cx, &format!("Settings, {tab}"), &mut all);
        }
    }
    assert!(
        all.is_empty(),
        "a word is cut letter by letter at {NARROW} px:\n{}",
        all.join("\n")
    );
}
