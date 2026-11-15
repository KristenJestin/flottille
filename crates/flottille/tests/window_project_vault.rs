//! A project's settings have a Vault tab (#948), on GPUI's test platform in front of a real
//! daemon whose keyring is the daemon's memory (`User::new`): the entries the project sees, its
//! own first, a new entry made in Settings' own form set to the project, and the list following
//! the vault live. No window opens on a screen, and the user's own keyring is never reached.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::project_settings::ProjectSettingsPage;
use flottille_window::shell::Selection;
use flottille_window::vault::VaultSection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// The secret of an entry, which no part of the window shows.
const SECRET: &str = "pr0ject-v4ult-secret";

fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
    folder
}

fn entry(user: &User, name: &str, project: Option<&str>) {
    user.call(
        "vault.set",
        json!({"name": name, "secret": SECRET, "description": format!("{name} token"),
            "project": project}),
    )
    .unwrap();
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1600.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the links sent, then draws what changed.
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
        // The test clock moves a frame at a time, as a real one would: a row unfolds in view.
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Lets every motion end.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn click(id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
}

fn shown(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).is_some_and(|found| found.visible()))
}

fn label_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The top of a row, to say which is above which.
fn top_of(id: &str, cx: &mut VisualTestContext) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| f32::from(window.find(id).bounds().origin.y))
}

/// Opens the project's settings on its Vault tab, once the daemon listed the entries.
fn open_vault_tab(
    shell: &Entity<Shell>,
    project: &str,
    cx: &mut VisualTestContext,
) -> Entity<ProjectSettingsPage> {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project(project.into()), window, cx)
    });
    until(shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == project)
        })
    });
    settle(cx);
    click("project-settings", cx);
    until(shell, cx, "the settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).name() == project && page.read(cx).is_loaded())
        })
    });
    settle(cx);
    click("project-settings-tabs-vault", cx);
    settle(cx);
    shell.read_with(cx, |s, cx| {
        s.main().read(cx).project_settings_page().cloned().unwrap()
    })
}

fn vault_section(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<VaultSection> {
    shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .settings_page()
            .map(|page| page.read(cx).vault().clone())
            .expect("Settings is shown")
    })
}

#[gpui_kit::test]
fn a_project_s_vault_tab_lists_its_entries_then_every_project_s(cx: &mut TestAppContext) {
    let user = User::new();
    project(&user, "atlas");
    entry(&user, "zeta", Some("atlas"));
    entry(&user, "alpha", None);
    entry(&user, "beta", Some("atlas"));
    let (shell, cx) = window(&user, cx);
    open_vault_tab(&shell, "atlas", cx);
    until(&shell, cx, "the three entries are listed", |cx| {
        shown("project-vault-alpha", cx)
            && shown("project-vault-beta", cx)
            && shown("project-vault-zeta", cx)
    });
    settle(cx);
    let (beta, zeta, alpha) = (
        top_of("project-vault-beta", cx),
        top_of("project-vault-zeta", cx),
        top_of("project-vault-alpha", cx),
    );
    assert!(
        beta < zeta && zeta < alpha,
        "its own first, by name, then every project's: {beta} {zeta} {alpha}"
    );
    assert_eq!(
        label_of("project-vault-beta-tag-0", cx).as_deref(),
        Some("atlas")
    );
    assert_eq!(
        label_of("project-vault-alpha-tag-0", cx).as_deref(),
        Some("Every project"),
        "an every-project entry is marked as such"
    );
    assert!(!shown("no-project-vault", cx));
    assert_eq!(
        label_of("project-vault-add", cx).as_deref(),
        Some("New entry")
    );
}

#[gpui_kit::test]
fn an_entry_of_another_project_is_not_listed(cx: &mut TestAppContext) {
    let user = User::new();
    project(&user, "atlas");
    project(&user, "harbor");
    entry(&user, "harbor-token", Some("harbor"));
    entry(&user, "shared", None);
    let (shell, cx) = window(&user, cx);
    open_vault_tab(&shell, "atlas", cx);
    until(&shell, cx, "the shared entry is listed", |cx| {
        shown("project-vault-shared", cx)
    });
    settle(cx);
    assert!(!shown("project-vault-harbor-token", cx));
    assert!(!shown("no-project-vault", cx));
}

#[gpui_kit::test]
fn an_empty_vault_says_so_and_an_entry_added_elsewhere_appears_in_the_tab(cx: &mut TestAppContext) {
    let user = User::new();
    project(&user, "atlas");
    let (shell, cx) = window(&user, cx);
    open_vault_tab(&shell, "atlas", cx);
    until(&shell, cx, "the empty state shows", |cx| {
        shown("no-project-vault", cx)
    });
    // Added anywhere (a client, a tool): the row unfolds into the tab and the empty line goes.
    entry(&user, "jira", Some("atlas"));
    until(&shell, cx, "the entry appears in the tab", |cx| {
        shown("project-vault-jira", cx)
    });
    let partway =
        cx.update(|window, _| window.find("project-vault-item-jira").bounds().size.height);
    settle(cx);
    let whole = cx.update(|window, _| window.find("project-vault-item-jira").bounds().size.height);
    assert!(
        partway < whole,
        "the row unfolds: {partway:?} then {whole:?}"
    );
    assert!(!shown("no-project-vault", cx));
    // Removed anywhere: it folds away.
    user.call("vault.remove", json!({"name": "jira", "confirmed": true}))
        .unwrap();
    until(&shell, cx, "the entry leaves the tab", |cx| {
        !shown("project-vault-jira", cx)
    });
    settle(cx);
    assert!(shown("no-project-vault", cx));
}

#[gpui_kit::test]
fn new_entry_from_a_project_opens_the_vault_form_set_to_that_project(cx: &mut TestAppContext) {
    let user = User::new();
    project(&user, "atlas");
    let (shell, cx) = window(&user, cx);
    open_vault_tab(&shell, "atlas", cx);
    click("project-vault-add", cx);
    settle(cx);
    // Settings' own form, in Settings, its project chosen.
    let section = vault_section(&shell, cx);
    assert!(
        section.read_with(cx, |s, _| s.is_open()),
        "the form is open"
    );
    assert!(shown("vault-modal", cx));
    assert_eq!(
        label_of("vault-project-field-label", cx).as_deref(),
        Some("Seen by agents of")
    );
    assert_eq!(
        section.read_with(cx, |s, _| s.project().map(str::to_owned)),
        Some("atlas".to_owned()),
        "Seen by agents of is this project"
    );
    // Saved: the entry is the project's, and the window goes back to its Vault tab.
    let (name, secret) = section.read_with(cx, |s, _| {
        (
            s.name_field().unwrap().clone(),
            s.secret_field().unwrap().clone(),
        )
    });
    cx.update(|window, cx| {
        name.update(cx, |f, cx| f.set_value("tracker", window, cx));
        secret.update(cx, |f, cx| f.set_value(SECRET, window, cx));
    });
    click("save-vault-entry", cx);
    until(
        &shell,
        cx,
        "the window is back on the project's tab",
        |cx| shown("project-vault-tracker", cx),
    );
    let entries = user.call("vault.list", json!({})).unwrap()["entries"].clone();
    assert_eq!(entries[0]["project"], "atlas");
}

#[gpui_kit::test]
fn editing_an_every_project_entry_from_a_project_says_so(cx: &mut TestAppContext) {
    let user = User::new();
    project(&user, "atlas");
    entry(&user, "shared", None);
    entry(&user, "own", Some("atlas"));
    let (shell, cx) = window(&user, cx);
    open_vault_tab(&shell, "atlas", cx);
    until(&shell, cx, "the entries are listed", |cx| {
        shown("project-vault-shared", cx) && shown("project-vault-own", cx)
    });
    settle(cx);
    // The project's own entry opens in the same form, with nothing to say.
    click("project-vault-own", cx);
    settle(cx);
    let section = vault_section(&shell, cx);
    assert!(section.read_with(cx, |s, _| s.is_open()));
    assert!(!shown("vault-project-field-hint", cx));
    section.update_in(cx, |section, window, cx| section.close(window, cx));
    until(
        &shell,
        cx,
        "the window is back on the project's Vault tab",
        |cx| shown("project-vault-shared", cx),
    );
    settle(cx);
    // An every-project entry says on the form that it is every project's.
    click("project-vault-shared", cx);
    settle(cx);
    assert!(section.read_with(cx, |s, _| s.is_open()));
    let said = label_of("vault-project-field-hint", cx).expect("the form says it");
    assert!(said.contains("every project"), "{said}");
}
