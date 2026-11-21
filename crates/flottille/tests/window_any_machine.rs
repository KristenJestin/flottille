//! The scenarios of #1182: every page of the window works on any of the user's machines
//! (`docs/remote.md`, "Requests from another machine's clients", "Roles", "One list of every
//! machine"). Settings, a project's pages and Add project address the machine chosen, every request
//! signed with the device key and carried by this machine's daemon, as a session's are. On GPUI's
//! test platform in front of real daemons, each with its own temporary HOME and XDG folders, fake
//! agents and no keyring, and a hub served in the test on 127.0.0.1. Throwaway repositories. No
//! window opens on a screen.

use crate::common;
use crate::remote::{Machine, TestHub, pair, pc_and_server_on};

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::shell::Selection;
use flottille_window::tree::project_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
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
        label(cx, "settings-machine-field-label").as_deref(),
        Some("Settings of")
    );
    click(&shell, "settings-machine", cx);
    assert!(
        present(cx, "settings-machine-here"),
        "this machine is offered"
    );
    assert!(
        label(cx, "settings-machine-server").is_some_and(|said| said.starts_with("server")),
        "{:?}",
        label(cx, "settings-machine-server")
    );
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
        label(cx, "settings-machine-server-line")
            .is_some_and(|said| said.contains("offline since")),
        "{:?}",
        label(cx, "settings-machine-server-line")
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
