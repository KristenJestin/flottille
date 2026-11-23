//! The scenarios of #1182: every page of the window works on any of the user's machines
//! (`docs/remote.md`, "Requests from another machine's clients", "Roles", "One list of every
//! machine"). Settings, a project's pages and Add project address the machine chosen, every request
//! signed with the device key and carried by this machine's daemon, as a session's are. On GPUI's
//! test platform in front of real daemons, each with its own temporary HOME and XDG folders, fake
//! agents and no keyring, and a hub served in the test on 127.0.0.1. Throwaway repositories. No
//! window opens on a screen.

use crate::common;
use crate::remote::{Machine, TestHub, pair, pc_and_server_on};
use flottille_link::Role;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::shell::Selection;
use flottille_window::tree::project_id;
use gpui_design_system::omarchy::Environment;
use gpui_design_system::seen::{colour, seen};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, Hsla, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// How long a link, a switch or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);

/// A user whose agents are the fake agent, Claude enabled and signed in (its login file only
/// there, never read).
fn runner() -> User {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_OFFER_MS", "300")
        .with_env("ANTHROPIC_API_KEY", "")
        .with_env("CLAUDE_CODE_OAUTH_TOKEN", "")
        .with_env("ANTHROPIC_AUTH_TOKEN", "");
    let claude = user.path("home/.claude");
    let user = user.with_env("CLAUDE_CONFIG_DIR", claude.to_str().unwrap());
    user.write_providers("[claude]\nenabled = true\n");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join(".credentials.json"), "{}").unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A throwaway repository at `home/<at>` of `user`, one commit on `main`.
fn repository(user: &User, at: &str) -> std::path::PathBuf {
    let folder = user.path(&format!("home/{at}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    folder
}

/// The project `name` of `user`'s machine, its repository at `home/code/<name>`.
fn project(user: &User, name: &str) {
    let folder = repository(user, &format!("code/{name}"));
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A hub, `pc` its first member and `server` a runner, each with the project `app`.
fn pc_and_server() -> (TestHub, Machine, Machine) {
    let (hub, pc, server) = pc_and_server_on(Machine::of(runner()), runner());
    project(&pc.user, "app");
    project(&server.user, "app");
    (hub, pc, server)
}

fn window<'a>(
    machine: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: machine.user.path("os-release"),
        state_home: machine.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(machine.user.connector());
    let device = Some(Device::in_folder(machine.folder()));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    until(&shell, cx, "the server is read online", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .and_then(|read| read.machine("server"))
                .is_some_and(|server| server.online())
        })
    });
    settle(&shell, cx);
    (shell, cx)
}

/// One step of the window as the platform would run it.
fn step(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(shell, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..40 {
        step(shell, cx);
    }
}

fn click_without_settling(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) {
    let _ = shell;
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    cx.update(|window, cx| window.render_frame(cx));
}

fn click(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(shell, cx);
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The colour the window draws the machine `name` in (`None`: this one), as the member list signs
/// it.
fn shade_of(shell: &Entity<Shell>, name: Option<&str>, cx: &mut VisualTestContext) -> Hsla {
    shell.read_with(cx, |s, cx| {
        let read = s
            .machines()
            .read(cx)
            .machines()
            .expect("the machines are read");
        let choice = flottille_window::machines::choices(read)
            .into_iter()
            .find(|choice| choice.name.as_deref() == name)
            .expect("a machine of the list");
        flottille_window::machines::shade(&choice.colour)
    })
}

/// How wide the element `id` is drawn, `None` when it is not drawn at all.
fn width_of(cx: &mut VisualTestContext, id: &str) -> Option<f32> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map(|found| f32::from(found.bounds().size.width))
    })
}

/// How tall the element `id` is drawn, `None` when it is not drawn at all.
fn height_of(cx: &mut VisualTestContext, id: &str) -> Option<f32> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map(|found| f32::from(found.bounds().size.height))
    })
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

/// Settings, open on the tab `tab`.
fn settings(shell: &Entity<Shell>, tab: &str, cx: &mut VisualTestContext) {
    click(shell, "settings", cx);
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    click(shell, &format!("settings-tabs-{tab}"), cx);
}

/// The chooser at the top of Settings, on `machine` (`here`: this one): the window then shows it.
fn settings_of(shell: &Entity<Shell>, machine: &str, cx: &mut VisualTestContext) {
    click(shell, "settings-machine", cx);
    click(shell, &format!("settings-machine-{machine}"), cx);
    let shown = (machine != "here").then(|| machine.to_owned());
    until(shell, cx, "the machine chosen is shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()) == shown
    });
    settle(shell, cx);
}

fn developer_on(user: &User) -> bool {
    user.call("developer.show", json!({})).unwrap()["log"] == true
}

fn specs_language(user: &User) -> Value {
    user.call("extension.list", json!({})).unwrap()["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "specs")
        .map(|specs| specs["projects"][0]["settings"]["language"].clone())
        .unwrap_or(Value::Null)
}

fn listed_projects(user: &User) -> Vec<String> {
    user.call("project.list", json!({})).unwrap()["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|project| project["name"].as_str().map(str::to_owned))
        .collect()
}

fn vault_names(user: &User) -> Vec<String> {
    user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
        .collect()
}

#[gpui_kit::test]
fn settings_show_and_change_another_machine_s(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "agents", cx);

    // The chooser says whose settings these are, this machine first, then the runners.
    assert_eq!(
        label(cx, "settings-machine-field").as_deref(),
        Some("Settings of")
    );
    click(&shell, "settings-machine", cx);
    assert!(
        present(cx, "settings-machine-here"),
        "this machine is offered"
    );
    assert_eq!(
        label(cx, "settings-machine-server-label").as_deref(),
        Some("server")
    );
    // Its role and its state are its line, and each machine is marked by its icon and colour:
    // this one's on the chooser, the server's in its row.
    assert!(
        label(cx, "settings-machine-server-detail")
            .is_some_and(|said| said.contains("runner") && said.contains("online")),
        "{:?}",
        label(cx, "settings-machine-server-detail")
    );
    // The kit draws the mark: the icon leads the chooser and each row, in the machine's colour,
    // and the machine chosen has the check.
    assert!(
        !present(cx, "settings-machine-mark"),
        "no mark drawn beside the chooser"
    );
    assert_eq!(
        colour("settings-machine-leading"),
        Some(shade_of(&shell, None, cx)),
        "the chooser's icon"
    );
    assert_eq!(
        colour("settings-machine-here-leading"),
        Some(shade_of(&shell, None, cx))
    );
    assert_eq!(
        colour("settings-machine-server-leading"),
        Some(shade_of(&shell, Some("server"), cx))
    );
    assert_eq!(seen("settings-machine-here-check"), Some(1.0));
    assert_eq!(seen("settings-machine-server-check"), Some(0.0));
    click(&shell, "settings-machine-server", cx);
    until(&shell, cx, "the server is shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    // Said once, at the top: the machine's name in the path line.
    assert_eq!(
        label(cx, "settings-where").as_deref(),
        Some("server / Settings / Agents")
    );

    // A change of its General tab is the server's, not this machine's.
    click(&shell, "settings-tabs-general", cx);
    assert_eq!(
        label(cx, "settings-where").as_deref(),
        Some("server / Settings / General")
    );
    assert!(!developer_on(&server.user));
    until(&shell, cx, "the server's developer mode is read", |cx| {
        present(cx, "developer-switch")
    });
    click(&shell, "developer-switch", cx);
    eventually("the server's developer mode is on", || {
        developer_on(&server.user)
    });
    assert!(!developer_on(&pc.user), "this machine's is unchanged");

    // Back on this machine: its own, and no path line.
    settings_of(&shell, "here", cx);
    assert!(
        !present(cx, "settings-where"),
        "this machine's page names none"
    );
}

/// #1248: gh copies its one-time code to the clipboard of the machine it runs on, so the code the
/// dialog shows for another machine's sign-in is the only one the user sees: it is the recent
/// wording's, read from its parentheses.
#[gpui_kit::test]
fn the_code_of_a_recent_gh_is_shown_for_another_machine_s_sign_in(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server_on(
        Machine::of(runner()),
        runner().with_sign_in_programs(&["gh"]),
    );
    server.user.sign_in_script(
        "gh",
        json!({"signed_in": false,
               "sign_in": {"address": "https://github.com/login/device", "code": "ABCD-1234"}}),
    );
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "agents", cx);
    settings_of(&shell, "server", cx);
    until(&shell, cx, "the server's gh row is read", |cx| {
        label(cx, "sign-in-state-gh").as_deref() == Some("Not signed in")
    });

    settle(&shell, cx);
    click(&shell, "sign-in-gh", cx);

    until(&shell, cx, "the dialog shows the server's code", |cx| {
        label(cx, "sign-in-code").as_deref() == Some("ABCD-1234")
    });
    assert_eq!(
        label(cx, "sign-in-address").as_deref(),
        Some("https://github.com/login/device")
    );
    assert!(
        !server.user.sign_in_record("gh").is_empty(),
        "it is the server's gh that ran"
    );
}

#[gpui_kit::test]
fn the_window_s_own_settings_stay_this_machine_s_and_say_so(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "general", cx);
    assert!(!present(cx, "settings-own"), "said only of another machine");
    settings_of(&shell, "server", cx);
    click(&shell, "settings-tabs-general", cx);

    until(
        &shell,
        cx,
        "the line says what stays this machine's",
        |cx| {
            label(cx, "settings-own")
                .is_some_and(|said| said.starts_with("This window") && said.contains("pc"))
        },
    );
    let theirs = server.user.call("appearance.show", json!({})).unwrap();
    // Off Omarchy, Flottille's own colours are shown already: the shade is what changes.
    click(&shell, "appearance-shade-light", cx);
    until(&shell, cx, "this machine keeps the appearance", |_| {
        pc.user.call("appearance.show", json!({})).unwrap()["shade"] == "light"
    });
    assert_eq!(
        server.user.call("appearance.show", json!({})).unwrap(),
        theirs,
        "the server's is not touched"
    );
}

/// Where the element `id` is drawn, `None` when it is not drawn at all.
fn bounds_of(cx: &mut VisualTestContext, id: &str) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).map(|found| found.bounds()))
}

/// The hub's host gives the server the role `role`; this machine's window reads it.
fn server_role(hub: &TestHub, shell: &Entity<Shell>, role: Role, cx: &mut VisualTestContext) {
    hub.with_hub(|hub| hub.change_role("server", role).unwrap());
    let runs = role == Role::Runner;
    until(shell, cx, "the server's role is read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .and_then(|read| read.machine("server"))
                .is_some_and(|server| server.runs() == runs)
        })
    });
}

#[gpui_kit::test]
fn two_machines_put_the_settings_picker_on_the_title_s_line(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "agents", cx);

    let row = bounds_of(cx, "settings-header-line").expect("the header's row");
    let title = bounds_of(cx, "settings-header-title").expect("the title");
    let picker = bounds_of(cx, "settings-machine").expect("the picker");
    // On the title's line: within the header's row, level with the title, after it.
    assert!(
        picker.top() >= row.top() && picker.bottom() <= row.bottom(),
        "{picker:?} in {row:?}"
    );
    assert!(
        picker.top() >= title.top() - gpui_kit::px(1.)
            && picker.bottom() <= title.bottom() + gpui_kit::px(1.),
        "{picker:?} level with {title:?}"
    );
    assert!(picker.left() > title.right(), "{picker:?} after {title:?}");
    // The title is its label: "Settings of" is said to a reader, never written.
    assert_eq!(
        label(cx, "settings-machine-field").as_deref(),
        Some("Settings of")
    );
    assert!(
        !present(cx, "settings-machine-field-label"),
        "no words \"Settings of\" drawn"
    );
    let tabs = bounds_of(cx, "settings-tabs").expect("the tabs");
    assert!(tabs.top() >= row.bottom(), "the tabs under the header");
}

#[gpui_kit::test]
fn one_machine_leaves_no_settings_picker_and_the_title_in_place(cx: &mut TestAppContext) {
    let (hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "agents", cx);
    let title = bounds_of(cx, "settings-header-title").expect("the title");
    assert!(
        present(cx, "settings-machine-fold"),
        "the picker, in the header"
    );

    // The server only controls now: this machine is the only one that runs agents.
    server_role(&hub, &shell, Role::Controller, cx);
    until(&shell, cx, "the picker is gone", |cx| {
        !present(cx, "settings-machine-fold")
    });
    settle(&shell, cx);
    assert!(!present(cx, "settings-machine"));
    assert!(!present(cx, "settings-machine-field"));
    assert_eq!(bounds_of(cx, "settings-header-title"), Some(title));
}

#[gpui_kit::test]
fn the_tabs_and_content_do_not_move_when_the_settings_picker_comes_or_goes(
    cx: &mut TestAppContext,
) {
    let (hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "agents", cx);
    let tabs = bounds_of(cx, "settings-tabs").expect("the tabs");
    let page = bounds_of(cx, "settings-page").expect("the content");
    let whole = width_of(cx, "settings-machine-fold").expect("the picker's fold");

    // It goes, then comes back: frame by frame, it narrows or widens, and nothing else moves.
    for (role, gone) in [(Role::Controller, true), (Role::Runner, false)] {
        hub.with_hub(|hub| hub.change_role("server", role).unwrap());
        let widths = std::cell::RefCell::new(Vec::new());
        until(&shell, cx, "the picker has folded or unfolded", |cx| {
            assert_eq!(bounds_of(cx, "settings-tabs"), Some(tabs), "the tabs moved");
            assert_eq!(
                bounds_of(cx, "settings-page"),
                Some(page),
                "the content moved"
            );
            let width = width_of(cx, "settings-machine-fold");
            widths.borrow_mut().extend(width);
            match gone {
                true => width.is_none(),
                false => width.is_some_and(|width| (width - whole).abs() < 0.5),
            }
        });
        let widths = widths.into_inner();
        assert!(
            widths
                .iter()
                .any(|width| *width > 1. && *width < whole - 1.),
            "it folds, it does not jump: {widths:?}"
        );
    }
    settle(&shell, cx);
    assert_eq!(bounds_of(cx, "settings-tabs"), Some(tabs));
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

#[gpui_kit::test]
fn a_project_is_added_on_another_machine_with_its_folder_picked_there(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let folder = repository(&server.user, "code/reports");
    let (shell, cx) = window(&pc, cx);
    shell.update_in(cx, |s, window, cx| s.add_project(window, cx));
    settle(&shell, cx);

    // On which machine first: this one, then the runners, each with its role and state.
    assert_eq!(
        label(cx, "add-project-machine-here").as_deref(),
        Some("pc"),
        "this machine first"
    );
    // Each is marked by its icon in its colour, drawn by the kit.
    assert_eq!(
        colour("add-project-machine-here-leading"),
        Some(shade_of(&shell, None, cx))
    );
    assert_eq!(
        colour("add-project-machine-server-leading"),
        Some(shade_of(&shell, Some("server"), cx))
    );
    click(&shell, "add-project-machine-server", cx);
    until(&shell, cx, "the server is shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    click(&shell, "add-project-step-next", cx);
    // The folder is picked in the server's folders, opened one after the other, never typed.
    until(&shell, cx, "the server's home is listed", |cx| {
        present(cx, "add-project-folder-code")
    });
    assert!(
        !present(cx, "add-project-browse"),
        "no dialog of this machine's"
    );
    assert!(!present(cx, "add-project-folder"), "no path typed");
    assert_eq!(label(cx, "add-project-path").as_deref(), Some("server: ~"));
    click(&shell, "add-project-folder-code", cx);
    until(&shell, cx, "its code folder is listed", |cx| {
        present(cx, "add-project-folder-reports")
    });
    assert_eq!(
        label(cx, "add-project-path").as_deref(),
        Some("server: ~/code")
    );
    click(&shell, "add-project-folder-reports", cx);
    until(&shell, cx, "the folder is detected there", |cx| {
        shell.read_with(cx, |s, cx| {
            s.add_project_form()
                .read(cx)
                .detected()
                .is_some_and(|found| found.name == "reports")
        })
    });
    click(&shell, "add-project-next", cx);
    eventually("the project is the server's", || {
        listed_projects(&server.user).contains(&"reports".to_owned())
    });
    assert!(!listed_projects(&pc.user).contains(&"reports".to_owned()));
    let added = server
        .user
        .call("project.show", json!({"name": "reports"}))
        .unwrap();
    assert_eq!(added["folder"], folder.to_str().unwrap());
}

#[gpui_kit::test]
fn a_project_of_another_machine_opens_its_whole_page(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    project(&server.user, "reports");
    server
        .user
        .call(
            "agent.run",
            json!({"project": "reports", "message": "hello"}),
        )
        .unwrap();
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the server's project is a group", |cx| {
        present(cx, &id_text(project_id("reports")))
    });

    click(&shell, &id_text(project_id("reports")), cx);
    until(&shell, cx, "the server shows its page", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().as_deref() == Some("server")
                && s.main()
                    .read(cx)
                    .project_page()
                    .is_some_and(|page| page.read(cx).name() == "reports")
        })
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, "project-where").as_deref(),
        Some("server / reports")
    );
    // Its whole page: its tabs and its work.
    for id in [
        "project-tabs",
        "project-work",
        "project-settings",
        "new-workspace",
    ] {
        assert!(present(cx, id), "{id}");
    }
    let _ = pc;
}

/// The text of an element id that is a name.
fn id_text(id: gpui_kit::ElementId) -> String {
    match id {
        gpui_kit::ElementId::Name(name) => name.to_string(),
        other => panic!("not a name: {other:?}"),
    }
}

#[gpui_kit::test]
fn a_project_setting_of_another_machine_is_changed_there(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    for user in [&pc.user, &server.user] {
        user.call("extension.set", json!({"name": "specs", "enabled": true}))
            .unwrap();
    }
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, cx| s.choose_machine(Some("server".to_owned()), cx));
    until(&shell, cx, "the server is shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the server's settings of app are read", |cx| {
        shell
            .read_with(cx, |s, cx| {
                s.main().read(cx).project_settings_page().cloned()
            })
            .is_some_and(|page| {
                page.read_with(cx, |page, cx| page.is_loaded() && page.has_read(cx))
            })
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, "project-settings-where").as_deref(),
        Some("server / app / Settings")
    );
    click(&shell, "project-settings-tabs-features", cx);
    until(&shell, cx, "Specs is a row", |cx| {
        present(cx, "project-features-specs")
    });
    click(&shell, "project-features-specs", cx);
    until(&shell, cx, "its page is in", |cx| {
        present(cx, "project-specs-setting-language-select")
    });
    click(&shell, "project-specs-setting-language-select", cx);
    click(&shell, "project-specs-setting-language-select-fr", cx);
    eventually("the server's app writes in French", || {
        specs_language(&server.user) == "fr"
    });
    assert_ne!(
        specs_language(&pc.user),
        "fr",
        "this machine's app is unchanged"
    );
}

#[gpui_kit::test]
fn another_machine_s_vault_and_connections_are_managed_from_here(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "vault", cx);
    settings_of(&shell, "server", cx);
    click(&shell, "settings-tabs-vault", cx);
    let section = shell
        .read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .settings_page()
                .map(|page| page.read(cx).vault().clone())
        })
        .unwrap();
    until(&shell, cx, "the server's vault is read", |cx| {
        section.read_with(cx, |s, _| s.entries().is_some())
    });
    click(&shell, "add-vault-entry", cx);
    for (field, text) in [
        (
            flottille_window::vault::VaultSection::name_field
                as fn(&_) -> Option<&Entity<gpui_kit::base::input::InputState>>,
            "tracker",
        ),
        (
            flottille_window::vault::VaultSection::secret_field,
            "s3rver-0nly",
        ),
    ] {
        let field = section.read_with(cx, |s, _| field(s).unwrap().clone());
        cx.update(|window, cx| field.update(cx, |f, cx| f.set_value(text, window, cx)));
    }
    click(&shell, "save-vault-entry", cx);
    eventually("the entry is the server's", || {
        vault_names(&server.user).contains(&"tracker".to_owned())
    });
    assert!(!vault_names(&pc.user).contains(&"tracker".to_owned()));

    // Its connections are the server's, read there.
    click(&shell, "settings-tabs-features", cx);
    let connections = shell
        .read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .settings_page()
                .map(|page| page.read(cx).connections().clone())
        })
        .unwrap();
    // This machine's list was read when the window opened: the server's is read once it is shown.
    until(&shell, cx, "the server's connections are read", |cx| {
        server.log().contains("connection.list asked by pc")
            && connections.read_with(cx, |s, _| s.servers().is_some())
    });
}

#[gpui_kit::test]
fn a_machine_that_is_off_shows_greyed_and_sends_nothing(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "general", cx);
    settings_of(&shell, "server", cx);
    click(&shell, "settings-tabs-general", cx);
    until(&shell, cx, "the server's developer mode is read", |cx| {
        present(cx, "developer-switch")
    });

    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the page says the server is off", |cx| {
        label(cx, "page-held").is_some_and(|said| said.starts_with("server offline since "))
    });
    // Greyed, not a daemon that does not answer: no restart is offered for another machine.
    assert!(!present(cx, "not-answering-banner"));
    assert!(present(cx, "page-held-veil"), "the page is greyed");
    let asked = pc.log().matches("asked by").count();
    click(&shell, "developer-switch", cx);
    settle(&shell, cx);
    assert!(
        toasts(&shell, cx)
            .iter()
            .all(|toast| !toast.contains("not")),
        "nothing was sent to be refused: {:?}",
        toasts(&shell, cx)
    );
    assert_eq!(pc.log().matches("asked by").count(), asked);

    // Its project rows in the sidebar are greyed too, and choosing it sends nothing either.
    settings_of(&shell, "here", cx);
    click(&shell, "settings-machine", cx);
    assert!(
        label(cx, "settings-machine-server-detail")
            .is_some_and(|said| said.contains("offline since")),
        "{:?}",
        label(cx, "settings-machine-server-detail")
    );
}

#[gpui_kit::test]
fn a_viewer_s_controls_are_disabled_with_the_reason(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let server = Machine::of(runner());
    let pc = Machine::of(runner());
    pair(&hub, &server, "server", "runner");
    pair(&hub, &pc, "pc", "viewer");
    pc.wait_for("server", "online");
    project(&server.user, "app");
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "general", cx);
    settings_of(&shell, "server", cx);
    click(&shell, "settings-tabs-general", cx);
    until(&shell, cx, "the reason is said", |cx| {
        label(cx, "page-held").as_deref() == Some("This device may only view server")
    });
    assert!(present(cx, "page-held-veil"), "its controls are disabled");
    click(&shell, "developer-switch", cx);
    settle(&shell, cx);
    assert!(!developer_on(&server.user));
    assert!(
        !server.log().contains("developer.set asked by pc"),
        "nothing was sent"
    );
    // Reading still works: the tabs are not held.
    click(&shell, "settings-tabs-agents", cx);
    assert_eq!(
        label(cx, "settings-where").as_deref(),
        Some("server / Settings / Agents")
    );
}

#[gpui_kit::test]
fn every_request_of_a_page_goes_to_its_machine_signed(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    repository(&server.user, "code/reports");
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "agents", cx);
    settings_of(&shell, "server", cx);
    for tab in ["agents", "vault", "general"] {
        click(&shell, &format!("settings-tabs-{tab}"), cx);
    }
    // The server's first read has come: what Settings shows is read from it.
    until(&shell, cx, "the server's developer mode is asked", |_| {
        server.log().contains("developer.show asked by pc")
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    settle(&shell, cx);
    shell.update_in(cx, |s, window, cx| s.add_project(window, cx));
    until(&shell, cx, "the server's folders are asked", |_| {
        server.log().contains("project.folders asked by pc")
    });

    let log = server.log();
    for method in [
        "provider.list",
        "developer.show",
        "vault.list",
        "project.settings",
        "project.folders",
    ] {
        assert!(
            log.contains(&format!("{method} asked by pc")),
            "{method} was not carried to the server signed:\n{log}"
        );
    }
    // What is the window's own is asked of this machine only.
    assert!(!log.contains("appearance.show asked by pc"), "{log}");
}

/// The page of the project `name`, open on the machine in front.
fn open_project(shell: &Entity<Shell>, name: &str, cx: &mut VisualTestContext) {
    let project = name.to_owned();
    shell.update_in(cx, |shell, window, cx| {
        shell.open(Selection::Project(project), window, cx)
    });
    let project = name.to_owned();
    until(shell, cx, "the project's page", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == project)
        })
    });
    settle(shell, cx);
}

fn shown(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    shell.read_with(cx, |s, _| s.machine_shown())
}

#[gpui_kit::test]
fn the_composer_picks_the_machine_among_those_that_have_the_project(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    project(&pc.user, "solo");
    let (shell, cx) = window(&pc, cx);

    // A project on one machine has no picker.
    open_project(&shell, "solo", cx);
    assert!(!present(cx, "machine-picker"), "one machine: no picker");

    // On two, the picker says the machine in front and lists the machines that have the project.
    open_project(&shell, "app", cx);
    until(&shell, cx, "the machine picker", |cx| {
        present(cx, "machine-picker")
    });
    assert_eq!(label(cx, "machine-picker").as_deref(), Some("pc"));
    assert!(
        !present(cx, "machine-picker-mark"),
        "the kit draws the mark"
    );
    assert_eq!(
        colour("machine-picker-leading"),
        Some(shade_of(&shell, None, cx))
    );
    click(&shell, "machine-picker", cx);
    assert_eq!(
        seen("machine-choice-here-check"),
        Some(1.0),
        "the chosen row"
    );
    assert_eq!(seen("machine-choice-server-check"), Some(0.0));
    assert_eq!(
        colour("machine-choice-server-leading"),
        Some(shade_of(&shell, Some("server"), cx))
    );
    assert_eq!(
        label(cx, "machine-choice-here-label").as_deref(),
        Some("pc")
    );
    assert_eq!(
        label(cx, "machine-choice-server-label").as_deref(),
        Some("server")
    );

    // Choosing the server shows its page of the project, the place back on `main`.
    click(&shell, "machine-choice-server", cx);
    until(&shell, cx, "the server's page of the project", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().as_deref() == Some("server")
                && s.main()
                    .read(cx)
                    .project_page()
                    .is_some_and(|page| page.read(cx).name() == "app")
        })
    });
    settle(&shell, cx);
    assert_eq!(label(cx, "machine-picker").as_deref(), Some("server"));
    assert_eq!(
        colour("machine-picker-leading"),
        Some(shade_of(&shell, Some("server"), cx)),
        "the button's icon follows the machine chosen"
    );
    assert_eq!(label(cx, "place-picker").as_deref(), Some("main"));
}

#[gpui_kit::test]
fn an_offline_machine_is_disabled_in_the_picker_and_says_since_when(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    open_project(&shell, "app", cx);
    until(&shell, cx, "the machine picker", |cx| {
        present(cx, "machine-picker")
    });
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the picker lists the server as off", |cx| {
        if !present(cx, "machine-choice-server") {
            cx.update(|window, cx| window.click("machine-picker".to_owned(), cx));
        }
        label(cx, "machine-choice-server-detail")
            .is_some_and(|said| said.starts_with("offline since "))
    });
    click(&shell, "machine-choice-server", cx);
    assert_eq!(shown(&shell, cx), None, "an offline machine is not chosen");
}

#[gpui_kit::test]
fn a_chosen_machine_that_goes_off_says_to_pick_another_or_wait(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    open_project(&shell, "app", cx);
    until(&shell, cx, "the machine picker", |cx| {
        present(cx, "machine-picker")
    });
    click(&shell, "machine-picker", cx);
    click(&shell, "machine-choice-server", cx);
    until(&shell, cx, "the server is shown", |cx| {
        shown(&shell, cx).as_deref() == Some("server")
    });
    settle(&shell, cx);
    assert!(!present(cx, "composer-machine-off"), "online: no notice");

    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the notice under the composer", |cx| {
        label(cx, "composer-machine-off").is_some_and(|said| {
            said.starts_with("server is offline since ") && said.contains("pick another")
        })
    });
    // It can be left: this machine is still chosen from the picker.
    click(&shell, "machine-picker", cx);
    click(&shell, "machine-choice-here", cx);
    until(&shell, cx, "this machine is back in front", |cx| {
        shown(&shell, cx).is_none()
    });
    settle(&shell, cx);
    assert!(!present(cx, "composer-machine-off"));
}

#[gpui_kit::test]
fn the_composer_machine_picker_folds_away_when_the_project_is_on_one_machine_only(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    open_project(&shell, "app", cx);
    until(&shell, cx, "the machine picker", |cx| {
        present(cx, "machine-picker")
    });
    settle(&shell, cx);

    // At rest it is drawn whole, not clipped.
    let whole = width_of(cx, "machine-picker-fold").expect("the picker is drawn");
    let picker = width_of(cx, "machine-picker").expect("its button");
    assert!(whole >= picker && whole > 40., "{whole} {picker}");

    // The server drops the project: the picker narrows and fades while it goes, then is gone.
    server
        .user
        .call("project.remove", json!({"name": "app"}))
        .unwrap();
    let drawn = std::cell::RefCell::new(Vec::new());
    until(&shell, cx, "the picker is gone", |cx| {
        match width_of(cx, "machine-picker-fold") {
            Some(width) => {
                drawn.borrow_mut().push(width);
                false
            }
            None => true,
        }
    });
    let drawn = drawn.into_inner();
    assert!(
        drawn.iter().any(|width| *width > 1. && *width < whole - 1.),
        "it narrows before it is gone: {drawn:?}"
    );
    assert!(
        drawn.windows(2).all(|pair| pair[1] <= pair[0] + 0.5),
        "and never widens again: {drawn:?}"
    );
    assert!(!present(cx, "machine-picker"));
    settle(&shell, cx);
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

#[gpui_kit::test]
fn the_off_machine_notice_folds_away_when_the_machine_is_left(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    open_project(&shell, "app", cx);
    until(&shell, cx, "the machine picker", |cx| {
        present(cx, "machine-picker")
    });
    click(&shell, "machine-picker", cx);
    click(&shell, "machine-choice-server", cx);
    until(&shell, cx, "the server is shown", |cx| {
        shown(&shell, cx).as_deref() == Some("server")
    });
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the notice under the composer", |cx| {
        present(cx, "composer-machine-off")
    });
    settle(&shell, cx);
    let whole = height_of(cx, "composer-machine-off-line").expect("its frame");
    assert!(whole > 10., "{whole}");

    // Leaving the machine folds the notice away, with its words, over several frames.
    click_without_settling(&shell, "machine-picker", cx);
    cx.update(|window, cx| window.click("machine-choice-here".to_owned(), cx));
    let drawn = std::cell::RefCell::new(Vec::new());
    until(&shell, cx, "the notice is gone", |cx| {
        match height_of(cx, "composer-machine-off-line") {
            Some(height) => {
                drawn.borrow_mut().push(height);
                false
            }
            None => true,
        }
    });
    let drawn = drawn.into_inner();
    assert!(
        drawn
            .iter()
            .any(|height| *height > 1. && *height < whole - 1.),
        "it folds before it is gone: {drawn:?}"
    );
    assert!(!present(cx, "composer-machine-off"));
}
