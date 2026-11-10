//! The window at rest (#444): once a page has settled, with every agent idle, no run and nothing
//! moving, the window draws no frame and asks for none, on GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_ui::toggle_id;
use flottille_window::Shell;
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
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

/// Lets the page's entrance, every motion and every read it started end: two seconds of the
/// window, in real time too, so that what the daemon answers late has come.
fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(2) {
        step(shell, Duration::from_millis(16), cx);
        std::thread::sleep(Duration::from_millis(5));
    }
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
    cx.update(|window, cx| window.click("project-settings-tabs-rules", cx));
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
    cx.update(|window, cx| window.click("project-settings-tabs-setup", cx));
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
