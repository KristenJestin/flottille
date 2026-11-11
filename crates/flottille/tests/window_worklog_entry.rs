//! The vault entry a Jira project logs time with (#638): the Tracker dialog of the project's
//! Settings shows `Log time with`, a select of the entries the project sees, saved as
//! `worklog_token`, and a link that adds one in Settings, Vault and comes back; a `log_time`
//! refused for want of an entry says so in the conversation, with a button to the Tracker. On
//! GPUI's test platform in front of a real daemon, with the fake ACP agent, the in-memory keyring,
//! throwaway repositories and temporary XDG folders. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::{Block, TIME_LOGGING_KIND};
use flottille_window::link::Connect;
use flottille_window::project_settings::{Dialog, ProjectSettingsPage, Reveal};
use flottille_window::session::SessionPage;
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const SECRET: &str = "w0rklog-t0ken-never-shown";
const ACCOUNT: &str = "dev@example.test";

fn user() -> User {
    let user = User::with_fake_agent().with_fake_acli();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A throwaway repository, added as the project `name`.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &[
            "remote",
            "add",
            "origin",
            &format!("git@github.com:acme/{name}.git"),
        ],
    );
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

/// The project `app`, its tracker Jira (with the entry `token` when one is named).
fn jira(user: &User, token: Option<&str>) {
    project(user, "app");
    let mut tracker = json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]});
    if let Some(token) = token {
        tracker["worklog_token"] = token.into();
    }
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": tracker}),
    )
    .unwrap();
}

fn entry(user: &User, name: &str, username: Option<&str>, project: Option<&str>) {
    let mut set = json!({"name": name, "secret": SECRET});
    if let Some(username) = username {
        set["username"] = username.into();
    }
    if let Some(project) = project {
        set["project"] = project.into();
    }
    user.call("vault.set", set).unwrap();
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
    let connect: Connect = Arc::new(user.connector());
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

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some_and(|found| found.visible()))
}

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// `app`'s Settings, Tracker, its dialog open, the vault read.
fn tracker_dialog(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<ProjectSettingsPage> {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    let page = |cx: &mut VisualTestContext| -> Option<Entity<ProjectSettingsPage>> {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
    };
    until(shell, cx, "the settings are read", |cx| {
        page(cx).is_some_and(|page| page.read_with(cx, |page, cx| page.has_read(cx)))
    });
    let page = page(cx).unwrap();
    page.update(cx, |page, cx| page.reveal(Reveal::Tracker, cx));
    settle(cx);
    cx.update(|window, cx| window.click("tracker-row", cx));
    until(shell, cx, "the vault is read", |cx| {
        page.read_with(cx, |page, _| page.worklog_options().len() > 1)
    });
    settle(cx);
    assert_eq!(
        page.read_with(cx, |page, _| page.dialog().cloned()),
        Some(Dialog::Tracker)
    );
    page
}

#[gpui_kit::test]
fn the_jira_tracker_shows_the_worklog_entry_and_saves_it(cx: &mut TestAppContext) {
    let user = user();
    jira(&user, None);
    entry(&user, "jira-time", Some(ACCOUNT), Some("app"));
    let (shell, cx) = window(&user, cx);
    let page = tracker_dialog(&shell, cx);

    // A visible label, and under it what the entry is: no secret is anywhere.
    assert_eq!(
        label("tracker-worklog-field-label", cx).as_deref(),
        Some("Log time with")
    );
    assert_eq!(
        label("tracker-worklog-field-hint", cx).as_deref(),
        Some("A Jira API token in the vault, its username your Jira e-mail.")
    );
    assert_eq!(
        label("tracker-worklog-add", cx).as_deref(),
        Some("New entry in the vault")
    );
    assert!(
        page.read_with(cx, |page, _| page.worklog_choice())
            .is_none(),
        "none is chosen: the file names no entry"
    );
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );

    cx.update(|window, cx| window.click("tracker-worklog", cx));
    settle(cx);
    cx.update(|window, cx| window.click("tracker-worklog-jira-time", cx));
    settle(cx);
    assert_eq!(
        page.read_with(cx, |page, _| page.worklog_choice())
            .as_deref(),
        Some("jira-time")
    );
    cx.update(|window, cx| window.click("save-settings-modal", cx));
    until(&shell, cx, "the tracker is saved", |_| {
        std::fs::read_to_string(user.project_file("app"))
            .is_ok_and(|file| file.contains("worklog_token = \"jira-time\""))
    });
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!file.contains(SECRET), "{file}");

    // None, chosen again, takes it off. The saved file is read again and the dialog's choices come
    // from the daemon: a fixed settle polls neither, so each is waited for.
    until(&shell, cx, "the saved settings are read again", |cx| {
        page.read_with(cx, |page, cx| page.has_read(cx) && page.dialog().is_none())
    });
    settle(cx);
    cx.update(|window, cx| window.click("tracker-row", cx));
    until(&shell, cx, "the dialog shows the entry again", |cx| {
        has("tracker-worklog", cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click("tracker-worklog", cx));
    settle(cx);
    cx.update(|window, cx| window.click("tracker-worklog-none", cx));
    settle(cx);
    cx.update(|window, cx| window.click("save-settings-modal", cx));
    until(&shell, cx, "the entry is taken off", |_| {
        std::fs::read_to_string(user.project_file("app"))
            .is_ok_and(|file| !file.contains("worklog_token"))
    });
}

#[gpui_kit::test]
fn the_link_adds_an_entry_in_the_vault_and_comes_back(cx: &mut TestAppContext) {
    let user = user();
    jira(&user, None);
    entry(&user, "jira-time", Some(ACCOUNT), Some("app"));
    let (shell, cx) = window(&user, cx);
    let _ = tracker_dialog(&shell, cx);

    cx.update(|window, cx| window.click("tracker-worklog-add", cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Settings)
    );
    let vault = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .settings_page()
            .expect("Settings is shown")
            .read(cx)
            .vault()
            .clone()
    });
    assert!(
        vault.read_with(cx, |vault, _| vault.is_open()),
        "a new entry is being made"
    );
    assert!(
        vault.read_with(cx, |vault, _| vault.name_field().is_some()),
        "its form"
    );

    // Closed, it goes back to the project's Tracker.
    vault.update_in(cx, |vault, window, cx| vault.close(window, cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::ProjectSettings("app".into()))
    );
    let page = shell
        .read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
        .unwrap();
    assert_eq!(
        page.read_with(cx, |page, _| page.revealed().cloned()),
        Some(Reveal::Tracker)
    );
}

#[gpui_kit::test]
fn an_entry_added_from_the_tracker_is_bound_to_its_worklog(cx: &mut TestAppContext) {
    let user = user();
    jira(&user, None);
    entry(&user, "jira-time", Some(ACCOUNT), Some("app"));
    let (shell, cx) = window(&user, cx);
    let _ = tracker_dialog(&shell, cx);
    cx.update(|window, cx| window.click("tracker-worklog-add", cx));
    settle(cx);
    let vault = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .settings_page()
            .expect("Settings is shown")
            .read(cx)
            .vault()
            .clone()
    });
    let fields = vault.read_with(cx, |v, _| {
        [
            v.name_field().unwrap().clone(),
            v.secret_field().unwrap().clone(),
        ]
    });
    for (field, text) in fields.into_iter().zip(["app-time", SECRET]) {
        cx.update(|window, cx| field.update(cx, |f, cx| f.set_value(text, window, cx)));
    }
    cx.update(|window, cx| window.click("save-vault-entry", cx));
    until(&shell, cx, "the entry is set", |_| {
        user.call("vault.list", json!({})).unwrap()["entries"][0]["name"] == "app-time"
    });
    let set = user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    // The entry made elsewhere stays any feature's.
    let other = user.call("vault.list", json!({})).unwrap()["entries"][1].clone();
    assert_eq!(other["for"], serde_json::Value::Null, "{other}");
    assert_eq!(set["for"], "worklog:app", "{set}");
}

#[gpui_kit::test]
fn the_worklog_select_lists_only_entries_the_project_sees(cx: &mut TestAppContext) {
    let user = user();
    jira(&user, None);
    project(&user, "other");
    entry(&user, "jira-time", Some(ACCOUNT), Some("app"));
    entry(&user, "shared", Some(ACCOUNT), None);
    entry(&user, "elsewhere", Some(ACCOUNT), Some("other"));
    user.call(
        "vault.set",
        json!({"name": "notes-key", "secret": SECRET, "for": "connection:almanac"}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    let page = tracker_dialog(&shell, cx);

    // None first, then the entries by name, each with its username.
    assert_eq!(
        page.read_with(cx, |page, _| page.worklog_options()),
        [
            ("none".to_owned(), "None".to_owned()),
            ("jira-time".to_owned(), format!("jira-time · {ACCOUNT}")),
            ("shared".to_owned(), format!("shared · {ACCOUNT}")),
        ]
    );
    cx.update(|window, cx| window.click("tracker-worklog", cx));
    settle(cx);
    assert!(has("tracker-worklog-none", cx));
    assert!(has("tracker-worklog-jira-time", cx));
    assert!(has("tracker-worklog-shared", cx));
    assert!(
        !has("tracker-worklog-elsewhere", cx),
        "another project's entry is not offered"
    );
    assert!(
        !has("tracker-worklog-notes-key", cx),
        "an entry bound to another use is not offered"
    );
}

/// A session of `app`, whose Jira tracker names no entry, told to log time: its turn calls
/// `tickets log_time`, which is refused at once.
fn refused_log_time(user: &User) -> String {
    user.fake_script(json!({"turns": [
        {"when": "log my time", "do": [
            {"mcp": {"tool": "tickets", "arguments":
                {"action": "log_time", "ticket": "ABC-1", "duration": "1h"}}},
            {"say": "It was refused."}]}
    ]}));
    jira(user, None);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "log my time"}),
        )
        .unwrap();
    answer["agent"]["id"].as_str().unwrap().to_owned()
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn setup_row(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<usize> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().iter().position(
            |block| matches!(block, Block::Notice { kind, .. } if kind == TIME_LOGGING_KIND),
        )
    })
}

#[gpui_kit::test]
fn a_log_time_refused_for_want_of_a_token_offers_set_up_time_logging(cx: &mut TestAppContext) {
    let user = user();
    let id = refused_log_time(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(&id), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the line of the refusal", |cx| {
        let page = session_page(&shell, cx);
        setup_row(&page, cx).is_some_and(|at| has(format!("row-{at}-time-logging-setup"), cx))
    });
    let page = session_page(&shell, cx);
    let at = setup_row(&page, cx).unwrap();
    // It unfolds into the conversation: it moves before it rests.
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the line enters without moving"
    );
    settle(cx);

    // The line says what happened, the button where to go.
    assert!(
        label(format!("row-{at}-notice"), cx).is_some_and(|said| said.contains("Settings")),
        "the line names the window"
    );
    assert_eq!(
        label(format!("row-{at}-time-logging-setup"), cx).as_deref(),
        Some("Set up time logging")
    );
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
    cx.update(|window, cx| window.click(format!("row-{at}-time-logging-setup"), cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::ProjectSettings("app".into()))
    );
    let settings = shell
        .read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
        .unwrap();
    assert_eq!(
        settings.read_with(cx, |page, _| page.revealed().cloned()),
        Some(Reveal::Tracker)
    );
}
