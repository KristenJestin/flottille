//! The window at rest (#444): once a page has settled, with every agent idle, no run and nothing
//! moving, the window draws no frame and asks for none, on GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::shell::Selection;
use flottille_window::state::PULSE_SPAN;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_design_system::toggle_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A user whose fake agent answers "ok", with the project `app` over a throwaway repository and
/// a session in it whose first turn ended: the session's id.
fn idle_session(user: &User) -> String {
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.start", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
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
    let connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

/// One step of the window as the platform would run it: what the link sent, a frame if one was
/// asked, what it started. How many animation frames were asked.
fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) -> usize {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    asked
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        step(shell, Duration::from_millis(16), cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Lets the page's entrance, every motion and every read it started end: the window runs until it
/// has asked for no animation frame over a second of its own clock and over two seconds of real
/// time, so that what the daemon answers late has come, and starts again from zero when a frame
/// is asked. How much of its clock a real second holds is the machine's (a loaded one draws
/// fewer frames in it), so the wait is for the quiet, not for a number of steps.
fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    const STEP: Duration = Duration::from_millis(16);
    let began = Instant::now();
    let mut last_asked = began;
    let mut quiet = Duration::ZERO;
    while quiet < Duration::from_secs(1) || last_asked.elapsed() < Duration::from_secs(2) {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "the window never stopped asking for animation frames"
        );
        if step(shell, STEP, cx) == 0 {
            quiet += STEP;
        } else {
            quiet = Duration::ZERO;
            last_asked = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Lets the pulse of a wait for the user end (`PULSE_SPAN`): it moves, by design, while the wait
/// is new and draws no animation frame, so `settle` does not wait for it; the window's clock is
/// moved past it, and the page settles on the still dot.
fn past_the_pulse(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    step(shell, PULSE_SPAN, cx);
    settle(shell, cx);
}

/// What the window drew and asked for over ten seconds of its clock (and a few of the daemon's):
/// the frames drawn, the animation frames asked.
fn frames_at_rest(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (u64, usize) {
    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        asked += step(shell, Duration::from_millis(16), cx);
        gone += Duration::from_millis(16);
        std::thread::sleep(Duration::from_millis(5));
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    (drawn, asked)
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_session_page(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let id = idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(
        &shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            shell.read_with(cx, |s, cx| {
                s.main()
                    .read(cx)
                    .session_page()
                    .is_some_and(|page| !page.read(cx).conversation().read(cx).rows().is_empty())
            })
        },
    );
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_project_page(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_each_tab_of_the_project_page(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    user.call(
        "workspace.create",
        json!({"project": "app", "name": "login"}),
    )
    .unwrap();
    eventually("the workspace is ready", || {
        user.call("workspace.show", json!({"project": "app", "name": "login"}))
            .unwrap()["state"]
            == "ready"
    });
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(&shell, cx, "Work shows the workspace", |cx| {
        cx.update(|window, _| window.try_find("place-login").is_some())
    });
    settle(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "Work: (frames drawn, frames asked)"
    );

    for tab in ["pulls", "tickets", "work"] {
        let id = format!("project-tabs-{tab}");
        cx.update(|window, cx| window.click(id, cx));
        settle(&shell, cx);
        assert_eq!(
            frames_at_rest(&shell, cx),
            (0, 0),
            "{tab}: (frames drawn, frames asked)"
        );
    }
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_prepared_workspace_s_page_with_its_time_to_ready(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    // A step that takes a little time, so that the workspace has a total to say.
    let script = user.path("tools/slow.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "sleep 0.3\n").unwrap();
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&format!("\n[[setup]]\nrun = \"sh {}\"\n", script.display()));
    std::fs::write(file, text).unwrap();
    user.call(
        "workspace.create",
        json!({"project": "app", "name": "login"}),
    )
    .unwrap();
    eventually("the workspace is ready", || {
        user.call("workspace.show", json!({"project": "app", "name": "login"}))
            .unwrap()["state"]
            == "ready"
    });
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    let said = |id: &str, cx: &mut VisualTestContext| {
        let id = id.to_owned();
        cx.update(|window, _| {
            window
                .try_find(id)
                .and_then(|found| found.label().map(str::to_owned))
        })
    };
    until(&shell, cx, "the place says how long it took", |cx| {
        said("place-total-login", cx).is_some()
    });
    settle(&shell, cx);

    assert!(
        said("place-total-login", cx).is_some_and(|total| total.starts_with("ready in ")),
        "{:?}",
        said("place-total-login", cx)
    );
    // It comes in through an entrance (with its separator), and its steps' times are one folded,
    // labelled line away.
    assert!(
        said("place-total-enter-login", cx).is_some()
            || cx.update(|window, _| window.try_find("place-total-enter-login").is_some()),
        "the total enters"
    );
    assert_eq!(
        said("place-times-login", cx).as_deref(),
        Some("How long it took")
    );
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_add_a_project_with_its_folder_detected(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let folder = user.path("work/other");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "other\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| s.add_project(window, cx));
    let form = shell.read_with(cx, |s, _| s.add_project_form().clone());
    cx.update(|window, cx| form.update(cx, |f, cx| f.set_folder(folder.clone(), window, cx)));
    until(&shell, cx, "the folder is detected", |cx| {
        form.read_with(cx, |f, _| f.detected().is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_project_page_with_a_picture_attached(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    // Its agent takes images, so that the composer attaches the picture.
    user.fake_script(json!({"images": true}));
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    let composer = shell
        .read_with(cx, |s, _| s.composer_of("app").cloned())
        .expect("the page has a composer");
    let mut picture = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 80, 40]))
        .write_to(&mut picture, image::ImageFormat::Png)
        .unwrap();
    composer.update_in(cx, |composer, _, cx| {
        composer.attach(
            gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, picture.into_inner()),
            cx,
        )
    });
    until(&shell, cx, "the thumbnail is there", |cx| {
        cx.update(|window, _| window.try_find("attachment-0").is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_project_s_rules_and_message_checks(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    cx.update(|window, cx| window.click("project-settings-tabs-instructions", cx));
    until(&shell, cx, "the message checks are shown", |cx| {
        cx.update(|window, _| window.try_find("message-checks-text-app").is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_project_s_commit_checks_and_their_proposals(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let husky = user.path("work/app/.husky");
    std::fs::create_dir_all(&husky).unwrap();
    std::fs::write(husky.join("pre-commit"), "lint-staged\nlint && test\n").unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    cx.update(|window, cx| window.click("project-settings-tabs-instructions", cx));
    until(&shell, cx, "the proposals are shown", |cx| {
        cx.update(|window, _| window.try_find("commit-checks-proposals-app-.").is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_task_s_copy_dialog(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    user.call(
        "task.add",
        json!({"project": "app", "name": "check", "line": "cargo test"}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    cx.update(|window, cx| window.click("project-settings-tabs-tasks", cx));
    until(&shell, cx, "the task is listed", |cx| {
        cx.update(|window, _| window.try_find("task-more-check").is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("task-more-check", cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("copy-task-check", cx));
    until(
        &shell,
        cx,
        "the dialog says there is no other project",
        |cx| cx.update(|window, _| window.try_find("task-copy-none-line").is_some()),
    );
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_project_s_branch_format(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    cx.update(|window, cx| window.click("project-settings-tabs-workspaces", cx));
    until(&shell, cx, "the branch format is shown", |cx| {
        cx.update(|window, _| window.try_find("branch-format-input-app").is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_project_s_repositories_and_tracker(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    until(&shell, cx, "the tracker is shown", |cx| {
        cx.update(|window, _| window.try_find("tracker-row").is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_once_the_repositories_were_read_again_on_focus(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    let page = |cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
    };
    until(&shell, cx, "the tracker is shown", |cx| {
        cx.update(|window, _| window.try_find("tracker-row").is_some())
    });
    settle(&shell, cx);
    let checks = page(cx)
        .unwrap()
        .read_with(cx, |page, _| page.programs().clone());
    let before = checks.read_with(cx, |checks, _| checks.last_request());

    // The window goes away and comes back: both reads are made, and nothing keeps moving.
    cx.deactivate_window();
    cx.update(|window, _| window.activate_window());
    until(&shell, cx, "the programs are read again", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.last_request() > before && !checks.is_checking()
        })
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_project_s_features(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    cx.update(|window, cx| window.click("project-settings-tabs-features", cx));
    until(&shell, cx, "the features are listed", |cx| {
        cx.update(|window, _| window.try_find("project-features-list").is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_project_feature_s_page_with_a_value_set_here(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    user.call("extension.set", json!({"name": "specs", "enabled": true}))
        .unwrap();
    user.call(
        "extension.set",
        json!({"name": "specs", "project": "app",
            "settings": {"language": "fr", "mode": "ticket"}}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    cx.update(|window, cx| window.click("project-settings-tabs-features", cx));
    until(&shell, cx, "Specs is listed", |cx| {
        cx.update(|window, _| window.try_find("project-features-specs").is_some())
    });
    cx.update(|window, cx| window.click("project-features-specs", cx));
    until(&shell, cx, "the values say they are set here", |cx| {
        cx.update(|window, _| {
            window
                .try_find("project-specs-setting-language-set")
                .is_some()
                && window.try_find("project-specs-setting-mode-set").is_some()
        })
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_settings(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_each_tab_of_settings_and_a_feature_s_page(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    user.call("provider.set_judge", json!({"provider": "claude"}))
        .unwrap();
    user.call("extension.set", json!({"name": "review", "enabled": true}))
        .unwrap();
    user.call("extension.set", json!({"name": "specs", "enabled": true}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the providers are shown", |cx| {
        cx.update(|window, _| window.try_find("providers-title").is_some())
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "agents");

    // The other tabs, each shown by one of its parts.
    for (tab, shown) in [
        ("approvals", "judge-model"),
        ("features", "features-review"),
        ("vault", "vault-title"),
        ("general", "appearance-title"),
        // The machines are the last section of General.
        ("general", "machines-off"),
    ] {
        cx.update(|window, cx| window.click(format!("settings-tabs-{tab}"), cx));
        until(&shell, cx, "the tab is shown", |cx| {
            cx.update(|window, _| window.try_find(shown).is_some())
        });
        settle(&shell, cx);
        assert_eq!(frames_at_rest(&shell, cx), (0, 0), "{tab}");
    }

    // A feature's page, drawn from its declaration, and the compaction's.
    cx.update(|window, cx| window.click("settings-tabs-features", cx));
    settle(&shell, cx);
    for (row, shown) in [
        ("review", "review-checks"),
        ("specs", "specs-setting-mode-internal"),
        ("compaction", "compaction-switch"),
    ] {
        cx.update(|window, cx| window.click(format!("features-{row}"), cx));
        until(&shell, cx, "the page is shown", |cx| {
            cx.update(|window, _| window.try_find(shown).is_some())
        });
        settle(&shell, cx);
        assert_eq!(frames_at_rest(&shell, cx), (0, 0), "{row}");
        cx.update(|window, cx| window.click("features-back", cx));
        settle(&shell, cx);
    }
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_review_s_page_with_a_check_s_instructions_open(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    user.call("extension.set", json!({"name": "review", "enabled": true}))
        .unwrap();
    user.call(
        "review.set",
        json!({"part": "bugs", "text": "Look at the retries first."}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1280.), px(3200.)));
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        cx.update(|window, _| window.try_find("settings-tabs-features").is_some())
    });
    cx.update(|window, cx| window.click("settings-tabs-features", cx));
    until(&shell, cx, "Review is a row", |cx| {
        cx.update(|window, _| window.try_find("features-review").is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("features-review", cx));
    until(&shell, cx, "the check's instructions are read", |cx| {
        cx.update(|window, _| {
            window
                .try_find("review-check-bugs-instructions-summary")
                .and_then(|found| found.label().map(str::to_owned))
                .is_some_and(|said| said.starts_with("Look at"))
        })
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "folded");

    cx.update(|window, cx| window.click("review-check-bugs-instructions", cx));
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "unfolded");
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_vault(cx: &mut TestAppContext) {
    let user = User::new();
    user.call(
        "vault.set",
        json!({"name": "jira", "secret": "at-rest-s3cret", "description": "Logs time"}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("settings-tabs-vault", cx));
    until(&shell, cx, "the entry is listed", |cx| {
        cx.update(|window, _| window.try_find("vault-jira").is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_vault_form_opened_from_an_ask(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = window(&user, cx);
    // As the card of an ask opens it: for a connection's server, whose entry is every project's,
    // the line under "Seen by agents of" says why.
    let entry = flottille_window::vault::VaultEntry {
        name: "notes-key".to_owned(),
        bound: Some("connection:notes".to_owned()),
        ..Default::default()
    };
    shell.update_in(cx, |shell, window, cx| {
        shell.add_asked_vault_entry(
            entry,
            "to read the notes".to_owned(),
            None,
            Some("notes".to_owned()),
            window,
            cx,
        )
    });
    until(&shell, cx, "the form says why", |cx| {
        cx.update(|window, _| window.try_find("vault-connection-note").is_some())
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "opened");

    // A refused Add: its error, at rest too.
    cx.update(|window, cx| window.click("save-vault-entry", cx));
    until(&shell, cx, "the refusal is said", |cx| {
        cx.update(|window, _| window.try_find("vault-error").is_some())
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "refused");
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_session_with_a_leave(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let x = idle_session(&user);
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.start", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let writer = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = common::mcp::server_of(&user, opened);
    let idle = |id: &str| {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    };
    eventually("the writer's first turn ends", || idle(&writer));
    let mcp = common::mcp::Mcp::initialize(&url, &authorization);
    let held = mcp.call("session_send", json!({"session": x, "text": "Hello."}));
    let number: u64 = held
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", held.text));
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the leave", || {
        user.call("leave.list", json!({"session": x})).unwrap()["leaves"] != json!([])
    });
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&x)).is_some())
    });
    eventually("both sessions idle", || idle(&x) && idle(&writer));
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&x), cx));
    until(&shell, cx, "the rail lists the leave", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .session_page()
                .is_some_and(|page| !page.read(cx).leaves(cx).read(cx).rows().is_empty())
        })
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_services_page_is_still_at_rest(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    // The fake server, from the test's own folder: one ready, one that failed, one stopped.
    std::fs::create_dir_all(user.path("bin")).unwrap();
    let server = user.path("bin/fake-server");
    std::os::unix::fs::symlink(common::fake_server(), &server).unwrap();
    let folder = user.path("home/tools");
    std::fs::create_dir_all(&folder).unwrap();
    let service = |name: &str, args: &str, ready: &str| {
        format!(
            "[services.{name}]\nfolder = \"{}\"\nline = \"{} {args}\"\n{ready}",
            folder.display(),
            server.display()
        )
    };
    std::fs::write(
        user.path("config/flottille/services.toml"),
        [
            service(
                "mongo",
                "--listen 0",
                "ready = { line = \"listening on\" }\n",
            ),
            service("relay", "--print port-in-use --exit 3", ""),
            service("redis", "--listen 0", ""),
        ]
        .concat(),
    )
    .unwrap();
    user.call("service.start", json!({"name": "mongo"}))
        .unwrap();
    user.call("service.start", json!({"name": "relay"}))
        .unwrap();
    eventually("mongo is ready and relay failed", || {
        let listed = user.call("service.list", json!({})).unwrap();
        let state = |name: &str| {
            listed["services"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == name)
                .unwrap()["state"]
                .clone()
        };
        state("mongo") == "ready" && state("relay") == "failed"
    });
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    cx.update(|window, cx| window.click("services", cx));
    until(&shell, cx, "the services are listed", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .services_page()
                .is_some_and(|page| page.read(cx).services().is_some_and(|l| l.len() == 3))
        })
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn leaving_the_services_page_with_an_output_open_leaves_the_window_at_rest(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    std::fs::create_dir_all(user.path("bin")).unwrap();
    let server = user.path("bin/fake-server");
    std::os::unix::fs::symlink(common::fake_server(), &server).unwrap();
    std::fs::write(
        user.path("config/flottille/services.toml"),
        format!(
            "[services.mongo]\nline = \"{} --listen 0\"\nready = {{ line = \"listening on\" }}\n",
            server.display()
        ),
    )
    .unwrap();
    user.call("service.start", json!({"name": "mongo"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    cx.update(|window, cx| window.click("services", cx));
    until(&shell, cx, "mongo is listed with its run", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).services_page().is_some_and(|page| {
                page.read(cx)
                    .services()
                    .is_some_and(|l| l.iter().any(|s| s.run.is_some()))
            })
        })
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("service-output-mongo", cx));
    let page = shell.read_with(cx, |s, cx| {
        s.main().read(cx).services_page().cloned().unwrap()
    });
    until(&shell, cx, "its output is followed", |cx| {
        page.read_with(cx, |p, _| p.follows_output())
    });

    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    settle(&shell, cx);

    assert!(
        !page.read_with(cx, |p, _| p.follows_output()),
        "no more run.output is asked"
    );
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_session_with_a_folded_sub_agents_card(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "launch a worker", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"kind": "child", "title": "write the docs", "brief": "REPORT and write the docs"}}}]},
        {"when": "REPORT", "do": [{"mcp": {"tool": "report", "arguments": {"text": "done"}}}]}
    ]}));
    let id = idle_session(&user);
    user.call(
        "agent.send",
        json!({"agent": id, "message": "launch a worker"}),
    )
    .unwrap();
    eventually("the worker reported and the lead is idle again", || {
        let agents = user.call("agent.list", json!({})).unwrap()["agents"].clone();
        let agents = agents.as_array().unwrap();
        agents.len() == 2
            && agents
                .iter()
                .all(|a| a["state"] == "idle" || a["state"] == "ended")
    });
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the sub-agents card is there", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).session_page().is_some_and(|page| {
                page.read(cx)
                    .conversation()
                    .read(cx)
                    .rows()
                    .iter()
                    .any(|row| !row.read(cx).sub_agents().is_empty())
            })
        })
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_session_with_a_card_of_links_and_a_thumbnail(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "show the results", "do": [
            {"mcp": {"tool": "web", "arguments": {"action": "show_link", "path": "shot.png"}}},
            {"mcp": {"tool": "web", "arguments": {"action": "show_link", "path": "other.png"}}},
            {"mcp": {"tool": "web", "arguments": {"action": "show_link",
                "url": "https://example.org/t/42", "title": "Ticket 42"}}}]}
    ]}));
    let id = idle_session(&user);
    // A picture of 1 pixel by 1.
    let picture = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64,
        0x60, 0xf8, 0x5f, 0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    std::fs::write(user.path("work/app/shot.png"), picture).unwrap();
    std::fs::write(user.path("work/app/other.png"), picture).unwrap();
    user.call(
        "agent.send",
        json!({"agent": id, "message": "show the results"}),
    )
    .unwrap();
    eventually("the lead showed its links and is idle again", || {
        let shown = user.call("agent.show", json!({"agent": id})).unwrap();
        shown["agent"]["state"] == "idle"
            && shown["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["name"] == "agent.link_shown")
                .count()
                == 3
    });
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(
        &shell,
        cx,
        "the card of links and its pictures are there",
        |cx| {
            cx.update(|window, _| {
                (0..12).any(|at| window.try_find(format!("row-{at}-link-0-thumb")).is_some())
            })
        },
    );
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

/// A session that asked three questions (one choice, words, several choices) and waits for the
/// answers: the user, the session's id and the window.
fn asked_three(cx: &mut TestAppContext) -> (User, String, Entity<Shell>, &mut VisualTestContext) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "ship it", "do": [
            {"mcp": {"tool": "ask", "arguments": {"questions": [
                {"question": "Push `feat/42-export` and open the pull request?",
                 "options": [{"label": "Yes"},
                             {"label": "Not yet", "description": "Keep it local"}]},
                {"question": "Title of the pull request?"},
                {"question": "Who reviews it?", "multiple": true,
                 "options": [{"label": "Alice"}, {"label": "Bob"}, {"label": "Carol"}]}
            ]}}},
            {"say": "Asked."}]},
        {"when": "Export orders", "do": [{"say": "Shipping."}]}
    ]}));
    let id = idle_session(&user);
    user.call("agent.send", json!({"agent": id, "message": "ship it"}))
        .unwrap();
    eventually("the lead asked and is idle again", || {
        let shown = user.call("agent.show", json!({"agent": id})).unwrap();
        shown["agent"]["state"] == "idle"
            && shown["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["name"] == "agent.asked")
    });
    let (shell, cx) = window(&user, cx);
    // Tall enough for the whole card: the conversation draws the rows in sight.
    cx.simulate_resize(size(px(1280.), px(1800.)));
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    (user, id, shell, cx)
}

/// The number of the ask that waits in the session `id`, as the daemon's tree says it.
fn asked_seq(user: &User, id: &str) -> u64 {
    let trees = user.call("agent.trees", json!({})).unwrap();
    trees["trees"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|tree| tree["agents"].as_array().unwrap().clone())
        .find(|entry| entry["agent"]["id"] == id)
        .unwrap()["questions"][0]["seq"]
        .as_u64()
        .unwrap()
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_question_card_open_with_three_questions(
    cx: &mut TestAppContext,
) {
    let (user, id, shell, cx) = asked_three(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    // Docked above the composer, one question at a time (#777).
    let seq = asked_seq(&user, &id);
    until(&shell, cx, "the card of the questions is there", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("ask-{seq}-question-0-option-1"))
                .is_some()
                && window
                    .try_find(format!("ask-{seq}-question-dots"))
                    .is_some()
        })
    });
    // The row of a session that needs the user pulses, by design, while the wait is new: the card
    // is measured once the pulse is over.
    past_the_pulse(&shell, cx);
    let (frames, asked) = frames_at_rest(&shell, cx);
    assert_eq!(asked, 0, "no animation frame asked");
    assert_eq!(frames, 0, "the open card draws no frame at rest");
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_question_card_folded_to_its_header(
    cx: &mut TestAppContext,
) {
    let (user, id, shell, cx) = asked_three(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    let seq = asked_seq(&user, &id);
    until(&shell, cx, "the card of the questions is there", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("ask-{seq}-question-0-option-1"))
                .is_some()
        })
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(format!("ask-{seq}-question-fold"), cx));
    until(&shell, cx, "the card folds to its header", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("ask-{seq}-question-0-option-1"))
                .is_none()
                && window
                    .try_find(format!("ask-{seq}-question-fold"))
                    .is_some()
        })
    });
    settle(&shell, cx);
    let (with_card, asked) = frames_at_rest(&shell, cx);
    assert_eq!(asked, 0, "no animation frame asked");

    // The row of a session that needs the user pulses, by design, on every page: the folded
    // card adds nothing to what the pulse alone draws.
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(&shell, cx);
    let (pulse, _) = frames_at_rest(&shell, cx);
    assert!(
        with_card <= pulse + 5,
        "the folded card adds no frame to the pulse of the row: {with_card} against {pulse}"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_question_card_folded_to_its_answers(
    cx: &mut TestAppContext,
) {
    let (user, id, shell, cx) = asked_three(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    // Docked above the composer, one question at a time (#777).
    let seq = asked_seq(&user, &id);
    until(&shell, cx, "the card of the questions is there", |cx| {
        cx.update(|window, _| {
            window
                .try_find(format!("ask-{seq}-question-0-option-1"))
                .is_some()
                && window
                    .try_find(format!("ask-{seq}-question-dots"))
                    .is_some()
        })
    });
    user.call(
        "agent.answer",
        json!({"agent": id, "question": seq, "answers": [
            {"choices": ["Yes"], "text": "Squash it first."},
            {"text": "Export orders as CSV"},
            {"choices": ["Alice", "Carol"]}
        ]}),
    )
    .unwrap();
    until(&shell, cx, "the card folds to its answers", |cx| {
        cx.update(|window, _| {
            (0..12).any(|at| {
                window
                    .try_find(format!("row-{at}-answered-2-answer"))
                    .is_some()
            })
        })
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

/// The lead `idle_session` started launches a session it leads, titled `title`, allowed by the
/// user when it is held: the session's id.
fn led_by_lead(user: &User, mcp: &common::mcp::Mcp, title: &str) -> String {
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "led", "title": title, "brief": format!("{title} now.")}),
    );
    assert!(!answer.error, "{}", answer.text);
    if let Some(number) = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse::<u64>().unwrap())
    {
        user.call("approval.approve", json!({"number": number}))
            .unwrap();
    }
    let found = || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["title"] == title)
            .map(|a| a["id"].as_str().unwrap().to_owned())
    };
    eventually("the led session", || found().is_some());
    let id = found().unwrap();
    eventually("its first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_with_a_pilot_and_two_led_sessions_one_folded(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    let lead = idle_session(&user);
    let (url, authorization) = common::mcp::server_of(&user, 0);
    let mcp = common::mcp::Mcp::initialize(&url, &authorization);
    let first = led_by_lead(&user, &mcp, "Docs");
    let second = led_by_lead(&user, &mcp, "Tests");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both led sessions under their pilot", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&first)).is_some()
                && window.try_find(session_id(&second)).is_some()
        })
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "unfolded: (frames drawn, frames asked)"
    );

    // The pilot folds its led sessions away.
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    until(&shell, cx, "the led rows folded away", |cx| {
        cx.update(|window, _| window.try_find(session_id(&first)).is_none())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "folded: (frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_spec_page_with_amendments(cx: &mut TestAppContext) {
    let user = User::with_fake_agent()
        .with_fetcher()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
        // The development's workspace fetches from `origin`: it reaches no server.
        .with_env("GIT_SSH_COMMAND", "false");
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@example.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call("extension.set", json!({"name": "specs", "enabled": true}))
        .unwrap();
    let spec = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Order filters"}),
        )
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    user.call(
        "spec.write",
        json!({"spec": spec, "section": "behaviour",
            "content": [{"text": "The orders list filters by status."}]}),
    )
    .unwrap();
    user.call("spec.ready", json!({"spec": spec})).unwrap();
    let opened = user.received_requests("session/new").len();
    user.call("spec.develop", json!({"spec": spec})).unwrap();
    let (url, authorization) = common::mcp::server_of(&user, opened);
    let developer = common::mcp::Mcp::initialize(&url, &authorization);
    for content in [
        json!({"id": "C1", "text": "The orders list filters by status, with chips."}),
        json!({"text": "Filters are remembered."}),
        json!({"text": "Filters are shared by link."}),
    ] {
        let answer = developer.call(
            "spec",
            json!({"action": "amend", "spec": spec, "section": "behaviour",
                "content": content, "why": "The user chose it."}),
        );
        assert!(!answer.error, "{}", answer.text);
    }
    let numbers: Vec<u64> =
        user.call("spec.amendments", json!({"spec": spec})).unwrap()["amendments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["amendment"].as_u64().unwrap())
            .collect();
    for (n, answer) in [(numbers[0], "accept"), (numbers[2], "refuse")] {
        user.call(
            "spec.amendment",
            json!({"spec": spec, "amendment": n, "answer": answer}),
        )
        .unwrap();
    }
    let (shell, cx) = window(&user, cx);
    // Tall enough that the page's toggles are on screen to be clicked.
    cx.simulate_resize(size(px(1280.), px(1600.)));
    until(&shell, cx, "the spec is listed", |cx| {
        shell.update_in(cx, |s, window, cx| {
            s.open(
                Selection::Spec {
                    project: "app".into(),
                    spec,
                },
                window,
                cx,
            )
        });
        cx.update(|window, _| window.try_find("spec-behaviour-C1-was-toggle").is_some())
    });
    settle(&shell, cx);
    // The old text and the refused ones unfolded: what unfolds settles too.
    for toggle in [
        "spec-behaviour-C1-was-toggle",
        "spec-amendments-refused-toggle",
    ] {
        cx.update(|window, cx| window.click(toggle.to_owned(), cx));
    }
    settle(&shell, cx);
    assert!(cx.update(|window, _| window.try_find("spec-behaviour-C1-was-text").is_some()));
    assert!(cx.update(|window, _| {
        window
            .try_find(format!("spec-amendment-{}", numbers[1]))
            .is_some()
    }));

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_reply_with_code_blocks(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "show the blocks", "do": [{"say":
            "Run this:\n\n```sh\ncargo test\n```\n\nThen, as it is:\n\n```\nfirst line\n```"}]}
    ]}));
    let id = idle_session(&user);
    user.call(
        "agent.send",
        json!({"agent": id, "message": "show the blocks"}),
    )
    .unwrap();
    eventually("the lead answered and is idle again", || {
        let shown = user.call("agent.show", json!({"agent": id})).unwrap();
        shown["agent"]["state"] == "idle" && shown.to_string().contains("first line")
    });
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(
        &shell,
        cx,
        "the code blocks' Copy buttons are there",
        |cx| {
            cx.update(|window, _| {
                (0..80).any(|at| window.try_find(format!("copy-code-{at}")).is_some())
            })
        },
    );
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_connections_page_and_its_review(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let fake =
        std::path::Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    let script = user.path("notes-script.json");
    // Added while its server cannot start (its script is not written yet), it waits for its
    // first review, which shows its actions.
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "A notes server.",
            "command": [fake.display().to_string(), "--script", script.display().to_string()]}),
    )
    .unwrap();
    std::fs::write(
        &script,
        json!({"server": {"name": "notes", "version": "1.0.0"}, "instructions": "Be brief.",
            "tools": [{"name": "search", "description": "Finds notes.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}}],
            "answers": {"search": {"text": "found"}}})
        .to_string(),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        cx.update(|window, _| window.try_find("settings-tabs-features").is_some())
    });
    cx.update(|window, cx| window.click("settings-tabs-features", cx));
    until(&shell, cx, "Connections is a row", |cx| {
        cx.update(|window, _| window.try_find("features-connections").is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("features-connections", cx));
    until(&shell, cx, "the server is listed", |cx| {
        cx.update(|window, _| window.try_find("connection-notes").is_some())
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "the list");

    cx.update(|window, cx| window.click("connection-more-notes", cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("review-connection-notes", cx));
    until(&shell, cx, "the review is read", |cx| {
        cx.update(|window, _| window.try_find("connection-review-row-3-search").is_some())
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "the review");
}

/// #1015: the form of a URL connection with a plain header and a secret of each place, open.
#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_connection_form_with_secrets(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    for name in ["notes-plain", "notes-key", "notes-query"] {
        user.call(
            "vault.set",
            json!({"name": name, "secret": "s3cret-at-rest"}),
        )
        .unwrap();
    }
    user.call(
        "connection.add",
        json!({"name": "remote", "about": "A notes server elsewhere.",
        "url": "https://notes.example.com/mcp",
        "headers": {"X-Plain": {"vault": "notes-plain"}},
        "secrets": [
            {"in": "header", "name": "Authorization", "prefix": "Bearer ", "entry": "notes-key"},
            {"in": "query", "name": "workspace", "entry": "notes-query"},
        ]}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        cx.update(|window, _| window.try_find("settings-tabs-features").is_some())
    });
    cx.update(|window, cx| window.click("settings-tabs-features", cx));
    until(&shell, cx, "Connections is a row", |cx| {
        cx.update(|window, _| window.try_find("features-connections").is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("features-connections", cx));
    until(&shell, cx, "the server is listed", |cx| {
        cx.update(|window, _| window.try_find("connection-remote").is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("connection-more-remote", cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("edit-connection-remote", cx));
    until(&shell, cx, "its secrets are in the form", |cx| {
        cx.update(|window, _| window.try_find("connection-secret-2-name").is_some())
    });
    settle(&shell, cx);
    // The caret blinks in a field that has the focus: the form at rest has none.
    cx.update(|window, cx| window.blur(cx));
    settle(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

/// #773 and #846: the composer holding a picture's chip and a command's, and a message that shows
/// its chips in the conversation, at rest.
#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_with_chips_in_the_composer_and_a_message(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true}));
    let id = idle_session(&user);
    user.call(
        "command.add",
        json!({"project": "app", "name": "ship", "description": "Ship it",
            "instructions": "Push the branch."}),
    )
    .unwrap();
    let mut picture = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 80, 40]))
        .write_to(&mut picture, image::ImageFormat::Png)
        .unwrap();
    let picture = picture.into_inner();
    let data = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&picture)
    };
    user.call(
        "agent.send",
        json!({"agent": id, "message": "see [Image 1: a.png] then /ship",
            "images": [{"mime": "image/png", "data": data}]}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the message shows with its picture", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).session_page().is_some_and(|page| {
                page.read(cx)
                    .conversation()
                    .read(cx)
                    .blocks()
                    .iter()
                    .any(|b| {
                        matches!(b, flottille_window::conversation::Block::Message { images, .. }
                        if images.len() == 1)
                    })
            })
        })
    });
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&id).cloned())
        .expect("the session has a composer");
    until(&shell, cx, "the composer knows the commands", |cx| {
        reply.read_with(cx, |r, _| r.knows_commands())
    });
    reply.update_in(cx, |reply, window, cx| {
        reply.restore("/ship ", Vec::new(), window, cx)
    });
    reply.update_in(cx, |reply, _, cx| {
        reply.attach(
            gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, picture.clone()),
            cx,
        )
    });
    until(&shell, cx, "both chips are in the composer", |cx| {
        reply.read_with(cx, |r, cx| {
            r.tokens()
                .read(cx)
                .value(cx)
                .iter()
                .filter(|p| matches!(p, gpui_design_system::Piece::Token(_)))
                .count()
                == 2
        })
    });
    // The caret blinks in a field that has the focus: the page at rest has none.
    cx.update(|window, cx| window.blur(cx));
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

/// #1000: a message's pictures opened in the viewer, zoomed and back, at rest each time.
#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_picture_viewer_open_zoomed_and_back(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true}));
    let id = idle_session(&user);
    let pictures: Vec<_> = [(900, 600), (300, 200)]
        .into_iter()
        .map(|(w, h)| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::RgbImage::from_pixel(w, h, image::Rgb([200, 80, 40]))
                .write_to(&mut out, image::ImageFormat::Png)
                .unwrap();
            use base64::Engine as _;
            let data = base64::engine::general_purpose::STANDARD.encode(out.into_inner());
            json!({"mime": "image/png", "data": data})
        })
        .collect();
    user.call(
        "agent.send",
        json!({"agent": id, "message": "look at these", "images": pictures}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    let message = |shell: &Entity<Shell>, cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).session_page().and_then(|page| {
                page.read(cx)
                    .conversation()
                    .read(cx)
                    .blocks()
                    .iter()
                    .position(|b| {
                        matches!(b, flottille_window::conversation::Block::Message { images, .. }
                        if images.len() == 2)
                    })
            })
        })
    };
    until(&shell, cx, "the message shows with its pictures", |cx| {
        message(&shell, cx).is_some()
    });
    settle(&shell, cx);
    let at = message(&shell, cx).unwrap();
    cx.update(|window, cx| window.blur(cx));
    cx.update(|window, cx| window.click(format!("row-{at}-image-0"), cx));
    settle(&shell, cx);
    assert!(cx.update(|window, _| window.try_find("picture-modal").is_some()));
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "open: (frames drawn, frames asked)"
    );

    cx.simulate_keystrokes("+ +");
    settle(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "zoomed: (frames drawn, frames asked)"
    );

    cx.simulate_keystrokes("right");
    settle(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "on the next picture: (frames drawn, frames asked)"
    );

    cx.simulate_keystrokes("0");
    settle(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "back to fit: (frames drawn, frames asked)"
    );

    cx.simulate_keystrokes("escape");
    settle(&shell, cx);
    assert!(cx.update(|window, _| window.try_find("picture-modal").is_none()));
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "closed: (frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_project_page_s_progress_tab(cx: &mut TestAppContext) {
    let user = User::with_fake_agent().with_fake_gh();
    idle_session(&user);
    let folder = user.path("work/app");
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    let issue = |number: u64, state: &str| {
        json!({"number": number, "title": format!("Ticket {number}"), "state": state,
            "body": "", "url": format!("https://github.com/acme/app/issues/{number}"),
            "author": {"login": "someone"}, "assignees": [], "labels": [],
            "milestone": {"number": 1, "title": "v0.1"},
            "updatedAt": "2020-01-01T00:00:00Z", "comments": []})
    };
    user.gh_script(json!({
        "milestones": {"acme/app": [{"number": 1, "title": "v0.1", "state": "open",
            "due_on": null, "open_issues": 1, "closed_issues": 1,
            "html_url": "https://github.com/acme/app/milestone/1"}]},
        "issues": {"acme/app": {"1": issue(1, "OPEN"), "2": issue(2, "CLOSED")}},
    }));
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(&shell, cx, "the Progress tab shows", |cx| {
        cx.update(|window, _| window.try_find("project-tabs-progress").is_some())
    });
    cx.update(|window, cx| window.click("project-tabs-progress", cx));
    until(&shell, cx, "the milestones show", |cx| {
        cx.update(|window, _| window.try_find("progress-milestone-github-1").is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("progress-milestone-github-1", cx));
    until(&shell, cx, "its tickets show", |cx| {
        cx.update(|window, _| window.try_find("progress-github-1-0-1").is_some())
    });
    cx.update(|window, cx| window.blur(cx));
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_session_page_with_a_pasted_text(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let log: String = (1..=120)
        .map(|n| format!("2026-11-15 INFO request {n} served\n"))
        .collect();
    // The first message carries a pasted text: the conversation shows its chip.
    let id = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "see [Pasted text 1: pasted-text.txt]",
                "texts": [{"name": "pasted-text.txt", "text": log}]}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    let message = |cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).session_page().and_then(|page| {
                page.read(cx)
                    .conversation()
                    .read(cx)
                    .blocks()
                    .iter()
                    .position(|b| {
                        matches!(b, flottille_window::conversation::Block::Message { text, .. }
                        if text.contains("read it at"))
                    })
            })
        })
    };
    until(&shell, cx, "the message is shown", |cx| {
        message(cx).is_some()
    });
    let at = message(cx).unwrap();
    let chip =
        gpui_kit::ElementId::NamedInteger(format!("row-{at}-message-tokens-token").into(), 0);
    until(&shell, cx, "the message shows its chip", |cx| {
        cx.update(|window, _| window.try_find(chip.clone()).is_some())
    });
    settle(&shell, cx);

    // The viewer opened, then closed.
    cx.update(|window, cx| window.click(chip.clone(), cx));
    until(&shell, cx, "the text is opened", |cx| {
        cx.update(|window, _| window.try_find("pasted-text-modal").is_some())
    });
    settle(&shell, cx);
    cx.simulate_keystrokes("escape");
    until(&shell, cx, "the viewer is closed", |cx| {
        cx.update(|window, _| window.try_find("pasted-text-modal").is_none())
    });
    // A large paste in the reply field: a chip there too. Attached as a paste attaches it,
    // without the field's focus, whose caret blinks.
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(&id).cloned())
        .expect("the session has a reply field");
    reply.update_in(cx, |reply, _, cx| reply.attach_text(log, cx));
    until(&shell, cx, "the reply holds the chip", |cx| {
        reply.read_with(cx, |r, cx| {
            r.tokens()
                .read(cx)
                .value(cx)
                .iter()
                .any(|piece| matches!(piece, gpui_design_system::Piece::Token(_)))
        })
    });
    // The toast that says it once goes away on its own: the rest is after it.
    let deadline = Instant::now() + Duration::from_secs(30);
    while shell.read_with(cx, |s, cx| s.toasts().read(cx).toasts().count() > 0) {
        assert!(Instant::now() < deadline, "the toast never went");
        step(&shell, Duration::from_millis(100), cx);
    }
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_services_page_is_at_rest_with_the_stop_dialog_open_then_closed(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    std::fs::create_dir_all(user.path("bin")).unwrap();
    let server = user.path("bin/fake-server");
    std::os::unix::fs::symlink(common::fake_server(), &server).unwrap();
    std::fs::write(
        user.path("config/flottille/services.toml"),
        format!(
            "[services.db]\nline = \"{} --listen 0\"\nready = {{ line = \"listening on\" }}\n",
            server.display()
        ),
    )
    .unwrap();
    // A task that runs and needs it: Stop asks first.
    user.call(
        "task.add",
        json!({"project": "app", "name": "api", "line": format!("{} --listen 0", server.display()),
            "serve": true, "needs": ["db"]}),
    )
    .unwrap();
    user.call("task.run", json!({"project": "app", "name": "api"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    cx.update(|window, cx| window.click("services", cx));
    until(
        &shell,
        cx,
        "db is listed, needed by the running api",
        |cx| {
            cx.update(|window, _| {
                window
                    .try_find("service-needed-db")
                    .and_then(|found| found.label().map(str::to_owned))
                    .is_some_and(|label| label.contains("api running"))
            })
        },
    );
    settle(&shell, cx);

    cx.update(|window, cx| window.click("service-stop-db", cx));
    settle(&shell, cx);
    assert!(cx.update(|window, _| window.try_find("service-stopping").is_some()));
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "open: (frames drawn, frames asked)"
    );
    cx.update(|window, cx| window.click("service-stopping-cancel", cx));
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "closed: (frames drawn, frames asked)"
    );
}

/// A project `app` with the connections extension on, a vault entry of its own, and a lead
/// session whose turn ended (a session at work moves): the user, the lead's id and its MCP.
fn lead_for_connections() -> (User, String, common::mcp::Mcp) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": "hippocampe-k3y-at-rest", "project": "app"}),
    )
    .unwrap();
    let opened = user.received_requests("session/new").len();
    let lead = user
        .call("agent.run", json!({"project": "app", "message": "start"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (url, authorization) = common::mcp::server_of(&user, opened);
    let mcp = common::mcp::Mcp::initialize(&url, &authorization);
    // Its turn ended, as in every scenario at rest: a session at work moves.
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": lead})).unwrap()["agent"]["state"] == "idle"
    });
    (user, lead, mcp)
}

/// The number of the request the daemon holds for the user, from the tool's answer.
fn held_number(answer: &common::mcp::Answer) -> u64 {
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// Opens the session `lead` and waits for its request `number`'s card, then for the page to rest.
fn open_with_request_card(
    shell: &Entity<Shell>,
    lead: &str,
    number: u64,
    cx: &mut VisualTestContext,
) {
    until(shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(shell, cx, "the request's card", |cx| {
        cx.update(|window, _| window.try_find(format!("open-request-{number}")).is_some())
    });
    settle(shell, cx);
    past_the_pulse(shell, cx);
}

/// #1036: a session page alone, its conversation shown, at rest.
#[gpui_kit::test]
fn a_session_page_at_rest_draws_no_frame(cx: &mut TestAppContext) {
    let (user, lead, _mcp) = lead_for_connections();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the conversation", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .session_page()
                .is_some_and(|page| !page.read(cx).conversation().read(cx).rows().is_empty())
        })
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "(drawn, asked)");
}

/// #1036: a plain request (no entry to widen), pending, no dialog open: the card is shown and
/// nothing moves.
#[gpui_kit::test]
fn a_session_with_a_pending_connection_request_draws_no_frame_at_rest(cx: &mut TestAppContext) {
    let (user, lead, mcp) = lead_for_connections();
    let answer = mcp.call(
        "connections",
        json!({"action": "add", "name": "notes", "about": "The user's notes.",
            "url": "http://127.0.0.1:7468/mcp"}),
    );
    let number = held_number(&answer);
    let (shell, cx) = window(&user, cx);
    open_with_request_card(&shell, &lead, number, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "(drawn, asked)");
}

/// #1036: the reply box's caret blinks while its field has the focus and stops when it loses it:
/// the focus leaves for the page, and the window rests. The window is active, as a window in use
/// is: an inactive one is sent no focus event, and so no blur.
#[gpui_kit::test]
fn the_caret_stops_when_its_field_loses_the_focus(cx: &mut TestAppContext) {
    let (user, lead, _mcp) = lead_for_connections();
    let (shell, cx) = window(&user, cx);
    cx.update(|window, _| window.activate_window());
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the reply box", |cx| {
        cx.update(|window, _| window.try_find("reply-field").is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("reply-field", cx));
    let (drawn, _) = frames_at_rest(&shell, cx);
    assert!(drawn > 0, "the focused caret blinks");
    let focus = shell.read_with(cx, |shell, _| shell.focus_handle());
    cx.update(|window, cx| window.focus(&focus, cx));
    settle(&shell, cx);
    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "the field lost the focus: (drawn, asked)"
    );
}

/// #1019: an agent's request for a connection whose entry is one project's and has no address,
/// its dialog open on the lines that offer to share and to bind it, at rest.
#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_connection_request_with_entries_to_widen(
    cx: &mut TestAppContext,
) {
    let (user, lead, mcp) = lead_for_connections();
    let answer = mcp.call(
        "connections",
        json!({"action": "add", "name": "hippocampe", "about": "The user's memory.",
            "url": "http://127.0.0.1:7468/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "hippocampe-key"}]}),
    );
    let number = held_number(&answer);

    let (shell, cx) = window(&user, cx);
    open_with_request_card(&shell, &lead, number, cx);
    cx.update(|window, cx| window.click(format!("open-request-{number}"), cx));
    until(&shell, cx, "the lines to share and bind", |cx| {
        cx.update(|window, _| {
            window
                .try_find("connection-request-share-hippocampe-key-action")
                .is_some()
                && window
                    .try_find("connection-request-bind-hippocampe-key-action")
                    .is_some()
        })
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "the dialog");
}

/// #1031: a session whose agent's accepted edit left a connection waiting for the user's review:
/// the card of that review, open, then folded away once it is reviewed.
#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_session_with_the_card_of_a_connection_waiting_for_its_review(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    let opened = user.received_requests("session/new").len();
    let id = idle_session(&user);
    let (url, authorization) = common::mcp::server_of(&user, opened);
    let mcp = common::mcp::Mcp::initialize(&url, &authorization);
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    let fake =
        std::path::Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    let script = user.path("notes-script.json");
    let listing = |more: bool| {
        let mut tools = vec![json!({"name": "search", "description": "Finds notes.",
            "inputSchema": {"type": "object"}})];
        if more {
            tools.push(json!({"name": "link", "description": "Links two notes.",
                "inputSchema": {"type": "object"}}));
        }
        json!({"server": {"name": "notes", "version": "1.0.0"}, "instructions": "Be brief.",
            "tools": tools, "answers": {"search": {"text": "found"}}})
        .to_string()
    };
    std::fs::write(&script, listing(false)).unwrap();
    let command = format!("{} --script {}", fake.display(), script.display());
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "A notes server.",
            "command": flottille_core::command::split_line(&command).unwrap()}),
    )
    .unwrap();
    // The server now announces one more action: the agent's edit is held, and accepted.
    std::fs::write(&script, listing(true)).unwrap();
    let held = mcp.call(
        "connections",
        json!({"action": "edit", "name": "notes", "command": format!("{command} --verbose")}),
    );
    let number: u64 = held
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split('.').next())
        .unwrap_or_else(|| panic!("not held: {}", held.text))
        .parse()
        .unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();

    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1280.), px(1800.)));
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&id), cx));
    let card = |shell: &Entity<Shell>, cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).session_page().and_then(|page| {
                page.read(cx)
                    .conversation()
                    .read(cx)
                    .blocks()
                    .iter()
                    .position(|block| {
                        matches!(
                            block,
                            flottille_window::conversation::Block::ConnectionReview(_)
                        )
                    })
            })
        })
    };
    until(&shell, cx, "the card of the review", |cx| {
        card(&shell, cx).is_some_and(|at| {
            cx.update(|window, _| window.try_find(format!("row-{at}-review-open")).is_some())
        })
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "the card, open");

    // Reviewed: the card folds away, and the window rests again.
    user.call("connection.review", json!({"name": "notes"}))
        .unwrap();
    user.call("connection.accept", json!({"name": "notes"}))
        .unwrap();
    until(&shell, cx, "the card folds away", |cx| {
        card(&shell, cx).is_some_and(|at| {
            cx.update(|window, _| {
                window
                    .try_find(format!("row-{at}-review-open"))
                    .is_none_or(|found| !found.visible())
            })
        })
    });
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "the card, folded away");
}

/// #1018: a dialog opens over the session it is asked from: the vault's form, from an agent's
/// request for an entry, at rest with the session behind it.
#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_vault_form_over_a_session(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let id = idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the conversation", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .session_page()
                .is_some_and(|page| !page.read(cx).conversation().read(cx).rows().is_empty())
        })
    });
    let entry = flottille_window::vault::VaultEntry {
        name: "notes-key".to_owned(),
        ..Default::default()
    };
    shell.update_in(cx, |shell, window, cx| {
        shell.add_asked_vault_entry(
            entry,
            "to read the notes".to_owned(),
            None,
            None,
            window,
            cx,
        )
    });
    until(&shell, cx, "the form is drawn", |cx| {
        cx.update(|window, _| window.try_find("vault-modal").is_some_and(|f| f.visible()))
    });
    settle(&shell, cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(id)),
        "the form is over the session"
    );
    cx.update(|window, cx| window.blur(cx));
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "(drawn, asked)");
}

/// #1018: the connection's form, from Edit on an agent's request, over the session, at rest.
#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_connection_form_over_a_session(
    cx: &mut TestAppContext,
) {
    let (user, lead, mcp) = lead_for_connections();
    let answer = mcp.call(
        "connections",
        json!({"action": "add", "name": "notes", "about": "The user's notes.",
            "url": "http://127.0.0.1:7468/mcp"}),
    );
    let number = held_number(&answer);
    let (shell, cx) = window(&user, cx);
    open_with_request_card(&shell, &lead, number, cx);
    cx.update(|window, cx| window.click(format!("open-request-{number}"), cx));
    until(&shell, cx, "the request's dialog", |cx| {
        cx.update(|window, _| {
            window
                .try_find("connection-request-edit")
                .is_some_and(|f| f.visible())
        })
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("connection-request-edit", cx));
    until(&shell, cx, "the form is drawn", |cx| {
        cx.update(|window, _| {
            window
                .try_find("connection-modal")
                .is_some_and(|f| f.visible())
        })
    });
    settle(&shell, cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(lead)),
        "the form is over the session"
    );
    cx.update(|window, cx| window.blur(cx));
    settle(&shell, cx);
    assert_eq!(frames_at_rest(&shell, cx), (0, 0), "(drawn, asked)");
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_session_whose_rail_holds_a_vault_ask(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent().with_fake_acli();
    user.fake_script(json!({"turns": [
        {"when": "hello", "do": [{"say": "ok"}]},
        {"when": "log my time", "do": [
            {"mcp": {"tool": "vault", "arguments":
                {"action": "ask", "name": "hippocampe-key", "why": "to read the notes"}}},
            {"say": "Asked."}]}
    ]}));
    let id = idle_session(&user);
    // The tracker names an entry the vault lacks: the agent may ask for it.
    user.git(
        &user.path("work/app"),
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"],
                "worklog_token": "hippocampe-key"}}),
    )
    .unwrap();
    user.call("agent.send", json!({"agent": id, "message": "log my time"}))
        .unwrap();
    eventually("the ask is open", || {
        !user.call("vault.asks", json!({})).unwrap()["asks"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    let number = user.call("vault.asks", json!({})).unwrap()["asks"][0]["ask"]
        .as_u64()
        .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&id), cx));
    // The card of "Waiting for you", and the line that records it in the conversation.
    until(&shell, cx, "the card is in the rail", |cx| {
        cx.update(|window, _| window.try_find(format!("vault-ask-{number}-add")).is_some())
    });
    past_the_pulse(&shell, cx);
    let (frames, asked) = frames_at_rest(&shell, cx);
    assert_eq!(asked, 0, "no animation frame asked");
    assert_eq!(frames, 0, "the card and the line draw no frame at rest");
}
