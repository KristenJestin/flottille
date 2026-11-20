//! The services page of the window (#480): each service of the machine with its state, started,
//! stopped, added, edited and removed from it, and a task's needs chosen from them in a project's
//! Settings. On GPUI's test platform in front of a real daemon; the services are the fake
//! development server, linked into the test's own temporary folder and run in temporary folders
//! of a temporary home. No window opens on a screen, no real service runs.

use crate::common;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::project_settings::{ProjectSettingsPage, Reveal};
use flottille_window::services::ServicesPage;
use gpui_design_system::omarchy::Environment;
use gpui_kit::base::input::InputState;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
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

/// Opens the menu of a picker of chips, as a click in its field does: the caret is in it and the
/// text changes, which opens the menu on every item.
fn open_picker(search: &Entity<InputState>, cx: &mut VisualTestContext) {
    let search = search.clone();
    // The caret leaves the field and comes back: that is what opens the menu.
    cx.update(|window, cx| window.blur(cx));
    settle(cx);
    cx.update(|window, cx| {
        let focus = search.focus_handle(cx);
        window.focus(&focus, cx);
    });
    settle(cx);
}

/// A click outside the menu of a picker closes it, as it does for a person, and does no more.
fn close_picker(prefix: &str, cx: &mut VisualTestContext) {
    click(&format!("{prefix}-modal-title"), cx);
    settle(cx);
}

/// Adds `name` to the needs of the form `prefix` (`service` or `task`) with its picker.
fn add_need(prefix: &str, search: &Entity<InputState>, name: &str, cx: &mut VisualTestContext) {
    open_picker(search, cx);
    click(&format!("{prefix}-needs-item-{name}"), cx);
    settle(cx);
    close_picker(prefix, cx);
}

/// Takes `name` out of the needs of the form `prefix` with the remove button of its chip.
fn drop_need(prefix: &str, name: &str, cx: &mut VisualTestContext) {
    close_picker(prefix, cx);
    click(&format!("{prefix}-needs-chip-{name}-remove"), cx);
    settle(cx);
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
    let search = form.read_with(cx, |f, _| f.needs_field().unwrap().clone());
    open_picker(&search, cx);
    assert!(has("service-needs-item-mongo", cx));
    assert!(!has("service-needs-item-auth", cx), "never itself");
    add_need("service", &search, "mongo", cx);
    click("service-ready-switch", cx);
    settle(cx);
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
    open_picker(&search, cx);
    assert!(!has("service-needs-item-auth", cx), "never itself");
    assert!(
        has("service-needs-chip-mongo", cx),
        "the need it has is a chip"
    );
    close_picker("service", cx);
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
    until(&shell, cx, "the form says the services are read", |cx| {
        has("task-needs-search", cx) && !has("task-no-needs", cx)
    });
    let search = section.read_with(cx, |s, _| s.needs_field().unwrap().clone());
    open_picker(&search, cx);
    assert!(has("task-needs-item-mongo", cx) && has("task-needs-item-redis", cx));
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
    add_need("task", &search, "redis", cx);
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
    drop_need("task", "redis", cx);
    add_need("task", &search, "mongo", cx);
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

/// `count` services that listen, `svc-00` on.
fn services_named(user: &User, count: usize) -> String {
    (0..count)
        .map(|n| listening(user, &format!("svc-{n:02}"), 0, &[]))
        .collect()
}

/// The height of the service dialog's body: what the dialog holds, whatever it shows of it.
fn form_height(cx: &mut VisualTestContext) -> gpui_kit::Pixels {
    cx.update(|window, _| window.find("service-form").bounds().size.height)
}

#[gpui_kit::test]
fn a_dialog_with_many_services_stays_short(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, &services_named(&user, 12));
    let (shell, cx) = window(&user, cx);
    let page = open_services(&shell, cx);

    click("add-service", cx);
    settle(cx);
    let with_twelve = form_height(cx);
    click("service-modal-cancel", cx);
    settle(cx);

    // Twenty-eight more on the machine: the dialog is not a line taller for them.
    for n in 12..40 {
        user.call(
            "service.add",
            json!({"name": format!("svc-{n:02}"),
                "line": format!("{} --listen 0", server_program(&user).display())}),
        )
        .unwrap();
    }
    until(&shell, cx, "the forty are listed", |cx| {
        page.read_with(cx, |p, _| p.services().is_some_and(|list| list.len() == 40))
    });
    click("add-service", cx);
    settle(cx);
    assert_eq!(form_height(cx), with_twelve, "the body is as tall with 40");

    // All forty are offered, in a menu that scrolls by itself.
    let form = page.read_with(cx, |p, _| p.form().clone());
    let search = form.read_with(cx, |f, _| f.needs_field().unwrap().clone());
    open_picker(&search, cx);
    assert!(has("service-needs-item-svc-00", cx));
    let (menu, window_height) = cx.update(|window, _| {
        (
            window.find("service-needs-menu").bounds().size.height,
            window.viewport_size().height,
        )
    });
    assert!(menu < window_height / 2., "the menu stays short: {menu:?}");
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn at rest");
}

#[gpui_kit::test]
fn a_dialog_footer_stays_in_view(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, &services_named(&user, 40));
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);
    cx.simulate_resize(size(px(1280.), px(480.)));
    click("add-service", cx);
    settle(cx);

    let bounds = |id: &str, cx: &mut VisualTestContext| {
        let id = id.to_owned();
        cx.update(|window, _| window.find(id).bounds())
    };
    let viewport = cx.update(|window, _| window.viewport_size());
    let (panel, title, save, cancel) = (
        bounds("service-modal", cx),
        bounds("service-modal-title", cx),
        bounds("save-service", cx),
        bounds("service-modal-cancel", cx),
    );
    assert!(
        panel.top() >= px(0.) && panel.bottom() <= viewport.height,
        "the dialog fits the window: {panel:?} in {viewport:?}"
    );
    assert!(
        title.top() >= panel.top()
            && save.bottom() <= panel.bottom()
            && cancel.bottom() <= panel.bottom(),
        "title and buttons are in it: {title:?} {save:?} {cancel:?}"
    );
    let last = bounds("service-flag-ask_before_running", cx);
    assert!(
        last.top() > panel.bottom(),
        "the last switch is further down"
    );

    cx.update(|window, cx| {
        window.scroll(
            "service-name-field-label",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-4000.))),
            cx,
        )
    });
    settle(cx);
    assert_eq!(bounds("service-modal-title", cx), title, "the title stays");
    assert_eq!(bounds("save-service", cx), save, "Save stays");
    let last = bounds("service-flag-ask_before_running", cx);
    assert!(
        last.bottom() <= save.top() && last.top() >= title.bottom(),
        "the last switch is now between the title and the buttons: {last:?}"
    );

    // A refusal is above the buttons, in sight, wherever the body was scrolled to.
    click("save-service", cx);
    let form = page_of(&shell, cx)
        .unwrap()
        .read_with(cx, |p, _| p.form().clone());
    until(&shell, cx, "the daemon refuses an empty form", |cx| {
        form.read_with(cx, |f, _| f.form_error().is_some())
    });
    settle(cx);
    let error = bounds("service-error", cx);
    assert!(
        error.top() >= title.bottom() && error.bottom() <= save.top(),
        "the refusal is between the title and the buttons: {error:?}"
    );
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn at rest");
}

#[gpui_kit::test]
fn the_service_dialog_says_what_is_required_and_what_is_optional_once(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, &services_named(&user, 3));
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);
    click("add-service", cx);
    settle(cx);

    // The first section is the required one, and says so; a line after it says the rest is not.
    assert_eq!(
        label_of("service-section-run-tag", cx).as_deref(),
        Some("required")
    );
    assert_eq!(
        label_of("service-optional", cx).as_deref(),
        Some("Optional: everything below can stay empty")
    );
    let top = |id: &str, cx: &mut VisualTestContext| {
        let id = id.to_owned();
        cx.update(|window, _| window.find(id).bounds())
    };
    let line = top("service-optional", cx);
    for required in ["service-name-field-label", "service-line-field-label"] {
        assert!(
            top(required, cx).bottom() <= line.top(),
            "{required} is above the line"
        );
    }
    for optional in [
        "service-folder-field-label",
        "service-needs",
        "service-section-ready-title",
        "service-section-address-title",
        "service-section-agents-title",
    ] {
        assert!(
            top(optional, cx).top() >= line.bottom(),
            "{optional} is below the line"
        );
    }
    // Each part has a heading, and the others' sections say what they are for.
    for heading in ["run", "where", "ready", "address", "agents"] {
        assert!(
            has(&format!("service-section-{heading}-title"), cx),
            "{heading}"
        );
    }
    // No label carries the word "optional" any more.
    for field in ["folder", "name", "line"] {
        let said = label_of(&format!("service-{field}-field-label"), cx).unwrap();
        assert!(!said.contains("optional"), "{said}");
    }
}

#[gpui_kit::test]
fn the_ready_and_address_fields_of_a_service_unfold_behind_their_switches(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, &listening(&user, "mongo", 0, &[]));
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);
    click("add-service", cx);
    settle(cx);

    // Nothing said: two switches, and no kind, no field.
    assert!(has("service-ready-switch", cx) && has("service-url-switch", cx));
    for hidden in [
        "service-ready-line",
        "service-url-address",
        "service-ready-line-block",
    ] {
        assert!(!has(hidden, cx), "{hidden}");
    }
    // Turned on, the kinds unfold and the first is chosen, its field under it.
    click("service-ready-switch", cx);
    settle(cx);
    assert!(has("service-ready-line", cx) && has("service-ready-line-block", cx));
    click("service-ready-url", cx);
    settle(cx);
    assert!(has("service-ready-url-block", cx) && !has("service-ready-line-block", cx));
    click("service-url-switch", cx);
    settle(cx);
    assert!(has("service-url-address-block", cx));
    // Turned off, they fold away: nothing said.
    click("service-ready-switch", cx);
    settle(cx);
    click("service-url-switch", cx);
    settle(cx);
    assert!(!has("service-ready-url-block", cx) && !has("service-url-address-block", cx));
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn at rest");
}

#[gpui_kit::test]
fn adding_and_removing_a_need_with_the_picker(cx: &mut TestAppContext) {
    let user = user();
    services_file(
        &user,
        &format!(
            "{}{}{}",
            listening(&user, "mongo", 0, &[]),
            listening(&user, "redis", 0, &[]),
            listening(&user, "auth", 0, &[]),
        ),
    );
    let (shell, cx) = window(&user, cx);
    let page = open_services(&shell, cx);
    click("add-service", cx);
    settle(cx);
    let form = page.read_with(cx, |p, _| p.form().clone());

    // Nothing needed: a line says so, no count.
    assert!(has("service-needs-none", cx));
    assert!(!has("service-needs-count", cx));

    // The menu offers each service with its line; one added is a chip, a count, and leaves it.
    let search = form.read_with(cx, |f, _| f.needs_field().unwrap().clone());
    open_picker(&search, cx);
    assert!(has("service-needs-item-mongo", cx) && has("service-needs-item-redis", cx));
    click("service-needs-item-mongo", cx);
    settle(cx);
    assert!(has("service-needs-chip-mongo", cx));
    assert_eq!(label_of("service-needs-count", cx).as_deref(), Some("1"));
    assert!(!has("service-needs-none", cx));
    assert!(
        !has("service-needs-item-mongo", cx),
        "chosen, no longer offered"
    );

    // Typing narrows the menu; chips keep the order they were added in.
    cx.update(|window, cx| search.update(cx, |f, cx| f.set_value("red", window, cx)));
    settle(cx);
    assert!(has("service-needs-item-redis", cx) && !has("service-needs-item-auth", cx));
    click("service-needs-item-redis", cx);
    cx.update(|window, cx| search.update(cx, |f, cx| f.set_value("", window, cx)));
    click("service-needs-item-auth", cx);
    settle(cx);
    assert_eq!(
        form.read_with(cx, |f, _| f.needs().to_vec()),
        ["mongo", "redis", "auth"]
    );
    assert_eq!(label_of("service-needs-count", cx).as_deref(), Some("3"));

    // A chip's button takes it out: it folds away and is offered again.
    drop_need("service", "redis", cx);
    assert!(!has("service-needs-chip-redis", cx));
    assert_eq!(
        form.read_with(cx, |f, _| f.needs().to_vec()),
        ["mongo", "auth"]
    );
    assert_eq!(label_of("service-needs-count", cx).as_deref(), Some("2"));
    open_picker(&search, cx);
    assert!(has("service-needs-item-redis", cx));
    close_picker("service", cx);

    // Saved as the file says them.
    let (name, line) = form.read_with(cx, |f, _| {
        (
            f.name_field().unwrap().clone(),
            f.line_field().unwrap().clone(),
        )
    });
    cx.update(|window, cx| {
        name.update(cx, |f, cx| f.set_value("web", window, cx));
        line.update(cx, |f, cx| {
            f.set_value(
                format!("{} --listen 0", server_program(&user).display()),
                window,
                cx,
            )
        });
    });
    click("save-service", cx);
    until(&shell, cx, "the file says so", |_| {
        services_text(&user).contains("[services.web]")
    });
    assert!(
        services_text(&user).contains("needs = [\"mongo\", \"auth\"]"),
        "{}",
        services_text(&user)
    );
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn adding_and_removing_a_need_with_the_picker_of_a_task(cx: &mut TestAppContext) {
    let user = user();
    services_file(
        &user,
        &format!(
            "{}{}",
            listening(&user, "mongo", 0, &[]),
            listening(&user, "redis", 0, &[]),
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
    until(&shell, cx, "the settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    settle(cx);
    let section = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .project_settings_page()
            .unwrap()
            .read(cx)
            .tasks()
            .clone()
    });
    click("add-task", cx);
    until(&shell, cx, "the form says the services are read", |cx| {
        has("task-needs-search", cx) && !has("task-no-needs", cx)
    });

    assert!(has("task-needs-none", cx));
    let search = section.read_with(cx, |s, _| s.needs_field().unwrap().clone());
    add_need("task", &search, "redis", cx);
    add_need("task", &search, "mongo", cx);
    assert!(has("task-needs-chip-redis", cx) && has("task-needs-chip-mongo", cx));
    assert_eq!(label_of("task-needs-count", cx).as_deref(), Some("2"));
    assert_eq!(
        section.read_with(cx, |s, _| s.needs().to_vec()),
        ["redis", "mongo"],
        "in the order added"
    );
    drop_need("task", "redis", cx);
    assert!(!has("task-needs-chip-redis", cx));
    assert_eq!(section.read_with(cx, |s, _| s.needs().to_vec()), ["mongo"]);
    drop_need("task", "mongo", cx);
    assert!(has("task-needs-none", cx), "the line is back");
    assert!(!has("task-needs-count", cx));
    assert_eq!(pending_frames(cx), 0);
}

/// The task `api` keeps running (a server) and needs `needs`; the user runs it.
fn running_api(user: &User, needs: &[&str]) {
    user.call(
        "task.edit",
        json!({"project": "app", "name": "api",
            "line": format!("{} --listen 0", server_program(user).display()),
            "serve": true, "needs": needs}),
    )
    .unwrap();
    user.call("task.run", json!({"project": "app", "name": "api"}))
        .unwrap();
}

#[gpui_kit::test]
fn stopping_a_service_a_running_task_needs_asks_first(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &["db"]);
    running_api(&user, &["db"]);
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);
    until(&shell, cx, "db is ready, needed by the running api", |cx| {
        state_of("db", cx).as_deref() == Some("ready")
            && label_of("service-needed-db", cx).as_deref() == Some("Needed by app (api running)")
    });
    settle(cx);
    // The running mark beside the task.
    assert!(has("service-running-db-app-api", cx));

    click("service-stop-db", cx);
    settle(cx);
    assert!(has("service-stopping", cx));
    // The sentence names the service; what needs it is said once, in the list.
    assert_said_once(cx);
    click("service-stopping-cancel", cx);
    settle(cx);
    assert!(!has("service-stopping", cx));
    assert_eq!(state_of("db", cx).as_deref(), Some("ready"));

    click("service-stop-db", cx);
    settle(cx);
    click("service-stopping-action", cx);
    until(&shell, cx, "the row says it stopped", |cx| {
        state_of("db", cx).as_deref() == Some("stopped")
    });
    settle(cx);
    assert!(!has("service-stopping", cx));
    assert_eq!(pending_frames(cx), 0);
}

/// The stop confirmation of `db` says what needs it once: its sentence names the service only,
/// its list the task that runs.
fn assert_said_once(cx: &mut VisualTestContext) {
    assert_eq!(
        label_of("service-stopping-needed", cx).as_deref(),
        Some("What runs and needs db goes on without it:")
    );
    assert_eq!(
        label_of("service-stopping-needed-0", cx).as_deref(),
        Some("app's api")
    );
    assert!(!has("service-stopping-needed-1", cx));
}

#[gpui_kit::test]
fn a_stop_refused_for_a_task_started_since_the_list_asks_first(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &["db"]);
    user.call("service.start", json!({"name": "db"})).unwrap();
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);
    until(&shell, cx, "db is ready, nothing running needs it", |cx| {
        state_of("db", cx).as_deref() == Some("ready")
    });
    settle(cx);
    assert!(!has("service-running-db-app-api", cx));

    // The task starts after the list was read, and the window has not read it again: the
    // daemon refuses the stop, which is asked in the confirmation instead.
    running_api(&user, &["db"]);
    // Waited for on the daemon's side only: the window is not polled meanwhile.
    let deadline = Instant::now() + Duration::from_secs(20);
    while user.call("service.show", json!({"name": "db"})).unwrap()["needed_now"]
        .as_array()
        .is_none_or(Vec::is_empty)
    {
        assert!(Instant::now() < deadline, "the api never needed db");
        std::thread::sleep(Duration::from_millis(50));
    }
    click("service-stop-db", cx);
    // Found as soon as it opens; seen once its motion ended.
    until(&shell, cx, "the confirmation opens", |cx| {
        label_of("service-stopping-needed", cx).is_some()
    });
    settle(cx);
    assert!(has("service-stopping", cx));
    // Drawn from the daemon's data, not from its sentence.
    assert_said_once(cx);
    assert_eq!(state_of("db", cx).as_deref(), Some("ready"));

    click("service-stopping-action", cx);
    until(&shell, cx, "the row says it stopped", |cx| {
        state_of("db", cx).as_deref() == Some("stopped")
    });
    settle(cx);
    assert!(!has("service-stopping", cx));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn stopping_a_service_nothing_running_needs_asks_nothing(cx: &mut TestAppContext) {
    let user = user();
    services_file(&user, &listening(&user, "db", 0, &[]));
    // A task needs it, but does not run.
    project(&user, &["db"]);
    user.call("service.start", json!({"name": "db"})).unwrap();
    let (shell, cx) = window(&user, cx);
    open_services(&shell, cx);
    until(&shell, cx, "db is ready", |cx| {
        state_of("db", cx).as_deref() == Some("ready")
    });
    settle(cx);
    assert!(!has("service-running-db-app-api", cx));

    click("service-stop-db", cx);
    until(&shell, cx, "the row says it stopped", |cx| {
        state_of("db", cx).as_deref() == Some("stopped")
    });
    settle(cx);
    assert!(!has("service-stopping", cx));
    assert_eq!(pending_frames(cx), 0);
}
