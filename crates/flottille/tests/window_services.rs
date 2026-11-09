//! The services page of the window (#480): each service of the machine with its state, started,
//! stopped, added, edited and removed from it, and a task's needs chosen from them in a project's
//! Settings. On GPUI's test platform in front of a real daemon; the services are the fake
//! development server, linked into the test's own temporary folder and run in temporary folders
//! of a temporary home. No window opens on a screen, no real service runs.

mod common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::project_settings::{ProjectSettingsPage, Reveal};
use flottille_window::services::ServicesPage;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A user with the fake server in a folder of the test's own, and no project.
fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    std::fs::create_dir_all(user.path("bin")).unwrap();
    std::os::unix::fs::symlink(common::fake_server(), server_program(&user)).unwrap();
    user
}

fn server_program(user: &User) -> PathBuf {
    user.path("bin/fake-server")
}

/// A folder of the home a service runs in.
fn folder(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("home/tools/{name}"));
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn services_file(user: &User, text: &str) {
    std::fs::write(user.path("config/flottille/services.toml"), text).unwrap();
}

fn services_text(user: &User) -> String {
    std::fs::read_to_string(user.path("config/flottille/services.toml")).unwrap_or_default()
}

/// A service that waits `wait` ms, then listens and is ready when it says so.
fn listening(user: &User, name: &str, wait: u64, needs: &[&str]) -> String {
    format!(
        "[services.{name}]\nfolder = \"{}\"\nline = \"{} --wait {wait} --listen 0\"\nneeds = {}\nready = {{ line = \"listening on\" }}\n",
        folder(user, name).display(),
        server_program(user).display(),
        json!(needs)
    )
}

/// A service that says why and ends at once, in failure.
fn failing(user: &User, name: &str) -> String {
    format!(
        "[services.{name}]\nfolder = \"{}\"\nline = \"{} --print port-in-use --exit 3\"\n",
        folder(user, name).display(),
        server_program(user).display(),
    )
}

/// The project `app` over a throwaway repository, its task `api` needing `needs`.
fn project(user: &User, needs: &[&str]) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    if !needs.is_empty() {
        user.call(
            "task.add",
            json!({"project": "app", "name": "api", "line": "git status --short", "needs": needs}),
        )
        .unwrap();
    }
}

/// A program the test started itself: killed when the test ends, even when it fails.
struct Started(Child);

impl Drop for Started {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The fake server started by hand, outside Flottille: it and its address.
fn by_hand(user: &User) -> (Started, String) {
    let mut child = Command::new(server_program(user))
        .args(["--listen", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut said)
        .unwrap();
    let address = said
        .trim()
        .strip_prefix("listening on ")
        .unwrap()
        .to_owned();
    (Started(child), address)
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

fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

fn label_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn has(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).is_some_and(|found| found.visible()))
}

fn click(id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
}

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<ServicesPage>> {
    shell.read_with(cx, |s, cx| s.main().read(cx).services_page().cloned())
}

/// Opens the services page from the top bar, and waits for the daemon's list.
fn open_services(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ServicesPage> {
    click("services", cx);
    until(shell, cx, "the services are listed", |cx| {
        page_of(shell, cx).is_some_and(|page| page.read_with(cx, |p, _| p.services().is_some()))
    });
    settle(cx);
    page_of(shell, cx).unwrap()
}

/// The state a service's row says.
fn state_of(name: &str, cx: &mut VisualTestContext) -> Option<String> {
    label_of(&format!("service-mark-{name}"), cx)
}

#[gpui_kit::test]
fn the_services_page_lists_each_service_with_its_state(cx: &mut TestAppContext) {
    let user = user();
    let (outside, address) = by_hand(&user);
    services_file(
        &user,
        &format!(
            "{}{}{}[services.web]\nline = \"{} --listen 0\"\nready = {{ url = \"{address}\" }}\nurl = \"{address}\"\n",
            listening(&user, "mongo", 0, &[]),
            listening(&user, "auth", 0, &["mongo"]),
            failing(&user, "relay"),
            server_program(&user).display(),
        ),
    );
    project(&user, &["auth"]);
    user.call("service.start", json!({"name": "mongo"}))
        .unwrap();
    user.call("service.start", json!({"name": "relay"}))
        .unwrap();
    user.call("service.start", json!({"name": "web"})).unwrap();
    let (shell, cx) = window(&user, cx);

    let page = open_services(&shell, cx);
    until(&shell, cx, "each state is known", |cx| {
        state_of("mongo", cx).as_deref() == Some("ready")
            && state_of("relay", cx).as_deref() == Some("failed")
    });
    settle(cx);

    let names: Vec<String> = page.read_with(cx, |p, _| {
        p.services()
            .unwrap()
            .iter()
            .map(|s| s.name.clone())
            .collect()
    });
    assert_eq!(names, ["auth", "mongo", "relay", "web"]);
    assert_eq!(state_of("auth", cx).as_deref(), Some("stopped"));
    assert_eq!(
        state_of("web", cx).as_deref(),
        Some("running outside Flottille")
    );
    // Ready: the solid dot, where the spinner was.
    assert!(has("service-dot-mongo", cx));
    // Failed, with why.
    let why = label_of("service-why-relay", cx).unwrap();
    assert!(why.ends_with("port-in-use"), "{why}");
    // Its folder, its address as a link, what it needs and which projects' tasks need it.
    let folder = label_of("service-folder-auth", cx).unwrap();
    assert!(folder.ends_with("tools/auth"), "{folder}");
    assert_eq!(
        label_of("service-needs-auth", cx).as_deref(),
        Some("Needs mongo")
    );
    assert_eq!(
        label_of("service-needed-auth", cx).as_deref(),
        Some("Needed by app (api)")
    );
    assert_eq!(
        label_of("service-needed-mongo", cx).as_deref(),
        Some("Needed by app (api)")
    );
    let shown = address.trim_start_matches("http://").to_owned();
    assert_eq!(label_of("service-link-web", cx), Some(shown));
    // Start on a service that does not run, Stop on one that does.
    assert!(has("service-start-auth", cx) && !has("service-stop-auth", cx));
    assert!(has("service-stop-mongo", cx) && !has("service-start-mongo", cx));
    assert_eq!(pending_frames(cx), 0);
    drop(outside);
}

#[gpui_kit::test]
fn starting_a_service_from_its_row_shows_it_starting_then_ready(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, &listening(&user, "mongo", 1500, &[]));
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);
    assert_eq!(state_of("mongo", cx).as_deref(), Some("stopped"));

    click("service-start-mongo", cx);
    until(&shell, cx, "the row says it starts", |cx| {
        state_of("mongo", cx).as_deref() == Some("starting")
    });
    // Start fades into Stop.
    settle(cx);
    assert!(has("service-stop-mongo", cx) && !has("service-start-mongo", cx));
    until(&shell, cx, "the row says it is ready", |cx| {
        state_of("mongo", cx).as_deref() == Some("ready")
    });
    settle(cx);
    assert!(has("service-dot-mongo", cx));

    // Its output, in the same panel as a run's.
    click("service-output-mongo", cx);
    let page = page_of(&shell, cx).unwrap();
    until(&shell, cx, "its output is shown", |cx| {
        page.read_with(cx, |p, cx| {
            p.panel().read(cx).text().contains("listening on http://")
        })
    });
    settle(cx);
    page.update(cx, |p, cx| p.close_output(cx));
    settle(cx);

    // Stop asks nothing: it is the user who clicks.
    click("service-stop-mongo", cx);
    until(&shell, cx, "the row says it stopped", |cx| {
        state_of("mongo", cx).as_deref() == Some("stopped")
    });
    assert!(!has("service-removal", cx));
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_service_running_outside_flottille_says_so(cx: &mut TestAppContext) {
    let user = user();
    let (outside, address) = by_hand(&user);
    services_file(
        &user,
        &format!(
            "[services.web]\nfolder = \"{}\"\nline = \"{} --listen 0\"\nready = {{ url = \"{address}\" }}\n",
            folder(&user, "web").display(),
            server_program(&user).display(),
        ),
    );
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);

    click("service-start-web", cx);
    until(&shell, cx, "the row says it runs outside", |cx| {
        state_of("web", cx).as_deref() == Some("running outside Flottille")
    });
    settle(cx);
    drop(outside);

    // What Start would do, said plainly.
    let said = label_of("service-outside-web", cx).unwrap();
    assert!(said.contains("Flottille did not start it"), "{said}");
    assert!(said.contains("Start runs"), "{said}");
    assert!(has("service-start-web", cx) && !has("service-stop-web", cx));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_service_is_added_and_edited_from_the_page(cx: &mut TestAppContext) {
    use flottille_window::tasks::ReadyKind;
    let user = user();
    services_file(&user, &listening(&user, "mongo", 0, &[]));
    let (shell, cx) = window(&user, cx);
    let page = open_services(&shell, cx);

    click("add-service", cx);
    settle(cx);
    let form = page.read_with(cx, |p, _| p.form().clone());
    let (name, line, folder_field, ready) = form.read_with(cx, |f, _| {
        (
            f.name_field().unwrap().clone(),
            f.line_field().unwrap().clone(),
            f.folder_field().unwrap().clone(),
            f.ready_field(ReadyKind::Line).unwrap().clone(),
        )
    });
    let auth = folder(&user, "auth");
    let program = server_program(&user);
    cx.update(|window, cx| {
        name.update(cx, |f, cx| f.set_value("auth", window, cx));
        line.update(cx, |f, cx| {
            f.set_value(format!("{} --listen 0", program.display()), window, cx)
        });
        folder_field.update(cx, |f, cx| {
            f.set_value(auth.display().to_string(), window, cx)
        });
    });
    // The services it may need: the others, never itself.
    assert!(has("service-need-mongo", cx));
    click("service-need-mongo", cx);
    click("service-ready-line", cx);
    settle(cx);
    cx.update(|window, cx| ready.update(cx, |f, cx| f.set_value("listening on", window, cx)));
    click("save-service", cx);
    until(&shell, cx, "the service is listed", |cx| {
        page.read_with(cx, |p, _| {
            p.services()
                .is_some_and(|list| list.iter().any(|s| s.name == "auth"))
        })
    });
    let text = services_text(&user);
    assert!(text.contains("[services.auth]"), "{text}");
    assert!(text.contains("needs = [\"mongo\"]"), "{text}");
    assert!(
        text.contains("ready = { line = \"listening on\" }"),
        "{text}"
    );
    settle(cx);
    assert_eq!(
        label_of("service-needs-auth", cx).as_deref(),
        Some("Needs mongo")
    );

    // A click on the row edits it; the form does not offer the service to itself.
    click("service-auth", cx);
    until(&shell, cx, "the form is open on it", |cx| {
        form.read_with(cx, |f, _| f.is_open())
    });
    settle(cx);
    assert!(!has("service-need-auth", cx));
    click("service-flag-ask_before_running", cx);
    cx.update(|window, cx| {
        line.update(cx, |f, cx| {
            f.set_value(
                format!("{} --wait 10 --listen 0", program.display()),
                window,
                cx,
            )
        })
    });
    click("save-service", cx);
    until(&shell, cx, "the file says so", |_| {
        services_text(&user).contains("--wait 10")
    });
    assert!(
        services_text(&user).contains("ask_before_running = true"),
        "{}",
        services_text(&user)
    );
    until(&shell, cx, "the form closed", |cx| {
        form.read_with(cx, |f, _| !f.is_open())
    });
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn removing_a_service_names_the_tasks_that_need_it(cx: &mut TestAppContext) {
    let user = user();
    services_file(
        &user,
        &format!(
            "{}{}",
            listening(&user, "mongo", 0, &[]),
            listening(&user, "redis", 0, &[])
        ),
    );
    project(&user, &["mongo"]);
    let (shell, cx) = window(&user, cx);
    let page = open_services(&shell, cx);

    click("service-more-mongo", cx);
    settle(cx);
    click("remove-service-mongo", cx);
    settle(cx);
    // The dialog names the tasks, and cannot remove it.
    let why = label_of("service-removal-refused", cx).unwrap();
    assert!(why.contains("app (api)"), "{why}");
    assert!(
        page.read_with(cx, |p, _| p.removal_refused().is_some()),
        "its Remove is disabled"
    );
    click("service-removal-cancel", cx);
    settle(cx);

    // One no task needs is removed once confirmed.
    click("service-more-redis", cx);
    settle(cx);
    click("remove-service-redis", cx);
    settle(cx);
    assert!(label_of("service-removal-refused", cx).is_none());
    assert!(services_text(&user).contains("[services.redis]"));
    click("service-removal-action", cx);
    until(&shell, cx, "it leaves the list", |cx| {
        page.read_with(cx, |p, _| {
            p.services()
                .is_some_and(|list| list.iter().all(|s| s.name != "redis"))
        })
    });
    assert!(
        !services_text(&user).contains("redis"),
        "{}",
        services_text(&user)
    );
    settle(cx);
    assert!(!has("service-redis", cx));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_task_s_needs_are_chosen_from_the_services(cx: &mut TestAppContext) {
    let user = user();
    services_file(
        &user,
        &format!(
            "{}{}",
            listening(&user, "mongo", 0, &[]),
            listening(&user, "redis", 0, &[])
        ),
    );
    project(&user, &[]);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at("app", Reveal::Tasks, window, cx)
    });
    let settings = |cx: &mut VisualTestContext| -> Option<Entity<ProjectSettingsPage>> {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
    };
    until(&shell, cx, "the settings are read", |cx| {
        settings(cx).is_some_and(|page| page.read_with(cx, |page, _| page.is_loaded()))
    });
    settle(cx);
    let section = settings(cx)
        .unwrap()
        .read_with(cx, |p, _| p.tasks().clone());

    click("add-task", cx);
    until(&shell, cx, "the services are offered", |cx| {
        has("task-need-mongo", cx) && has("task-need-redis", cx)
    });
    let (name, line) = section.read_with(cx, |s, _| {
        (
            s.name_field().unwrap().clone(),
            s.line_field().unwrap().clone(),
        )
    });
    cx.update(|window, cx| {
        name.update(cx, |f, cx| f.set_value("api", window, cx));
        line.update(cx, |f, cx| f.set_value("git status", window, cx));
    });
    click("task-need-redis", cx);
    click("save-task", cx);
    until(&shell, cx, "the task is listed", |cx| {
        section.read_with(cx, |s, _| {
            s.tasks()
                .is_some_and(|tasks| tasks.iter().any(|t| t.needs == ["redis"]))
        })
    });
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(file.contains("needs = [\"redis\"]"), "{file}");
    settle(cx);
    // The row says what it needs.
    assert_eq!(
        label_of("task-needs-api", cx).as_deref(),
        Some("Needs redis")
    );

    // Edited, it needs mongo instead.
    click("task-api", cx);
    until(&shell, cx, "the form is open on it", |cx| {
        section.read_with(cx, |s, _| s.is_open())
    });
    settle(cx);
    click("task-need-redis", cx);
    click("task-need-mongo", cx);
    click("save-task", cx);
    until(&shell, cx, "the file says so", |_| {
        std::fs::read_to_string(user.project_file("app"))
            .unwrap()
            .contains("needs = [\"mongo\"]")
    });
    until(&shell, cx, "the row follows", |cx| {
        label_of("task-needs-api", cx).as_deref() == Some("Needs mongo")
    });
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn removing_a_service_another_service_needs_names_it(cx: &mut TestAppContext) {
    let user = user();
    services_file(
        &user,
        &format!(
            "{}{}",
            listening(&user, "mongo", 0, &[]),
            listening(&user, "auth", 0, &["mongo"])
        ),
    );
    let (shell, cx) = window(&user, cx);
    let page = open_services(&shell, cx);

    click("service-more-mongo", cx);
    settle(cx);
    click("remove-service-mongo", cx);
    settle(cx);

    let why = label_of("service-removal-refused", cx).unwrap();
    assert!(why.contains("services auth"), "{why}");
    assert!(
        page.read_with(cx, |p, _| p.removal_refused().is_some()),
        "its Remove is disabled"
    );
    click("service-removal-cancel", cx);
    settle(cx);
    assert!(services_text(&user).contains("[services.mongo]"));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_service_that_says_no_ready_is_running_with_the_solid_dot(cx: &mut TestAppContext) {
    let user = user();
    services_file(
        &user,
        &format!(
            "[services.relay]\nfolder = \"{}\"\nline = \"{} --listen 0\"\n",
            folder(&user, "relay").display(),
            server_program(&user).display(),
        ),
    );
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);

    click("service-start-relay", cx);
    until(&shell, cx, "the row says it runs", |cx| {
        state_of("relay", cx).as_deref() == Some("running")
    });
    settle(cx);

    assert!(has("service-dot-relay", cx), "the solid dot, no spinner");
    assert!(has("service-stop-relay", cx) && !has("service-start-relay", cx));
    assert_eq!(pending_frames(cx), 0, "nothing turns while it runs");
}

#[gpui_kit::test]
fn editing_a_service_keeps_its_folder_as_written(cx: &mut TestAppContext) {
    let user = user();
    std::fs::create_dir_all(user.path("home/tools/mongo")).unwrap();
    let program = server_program(&user);
    services_file(
        &user,
        &format!(
            "[services.cache]\nline = \"{0} --listen 0\"\n\n[services.mongo]\nfolder = \"~/tools/mongo\"\nline = \"{0} --listen 0\"\n",
            program.display()
        ),
    );
    let (shell, cx) = window(&user, cx);
    let page = open_services(&shell, cx);
    let form = page.read_with(cx, |p, _| p.form().clone());

    for name in ["cache", "mongo"] {
        click(&format!("service-{name}"), cx);
        until(&shell, cx, "the form is open on it", |cx| {
            form.read_with(cx, |f, _| f.is_open())
        });
        settle(cx);
        click("service-flag-ask_before_running", cx);
        click("save-service", cx);
        until(&shell, cx, "the form closed", |cx| {
            form.read_with(cx, |f, _| !f.is_open())
        });
        settle(cx);
    }

    let text = services_text(&user);
    assert_eq!(
        text.matches("ask_before_running = true").count(),
        2,
        "{text}"
    );
    assert!(text.contains("folder = \"~/tools/mongo\""), "{text}");
    assert_eq!(
        text.matches("folder").count(),
        1,
        "cache gained no folder: {text}"
    );
}

#[gpui_kit::test]
fn a_task_s_needs_say_why_the_services_cannot_be_read(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, "[services.mongo]\nbogus = 1\n");
    project(&user, &[]);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at("app", Reveal::Tasks, window, cx)
    });
    until(&shell, cx, "the settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    settle(cx);

    click("add-task", cx);
    until(&shell, cx, "the form says why", |cx| {
        label_of("task-no-needs", cx).is_some_and(|said| said.contains("services.toml"))
    });
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
}
