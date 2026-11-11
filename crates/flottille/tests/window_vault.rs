//! The vault in the window's Settings (#382), on GPUI's test platform in front of a real daemon
//! whose keyring is the daemon's memory (`User::new`): an entry added, replaced and removed, its
//! secret typed in a masked field, cleared once sent and never shown again. No window opens on a
//! screen, and the user's own keyring is never reached.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::vault::VaultSection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The secret typed in the dialog, which the window never shows again.
const SECRET: &str = "w1ndow-v4ult-secret";

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

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

/// Opens Settings on its Vault tab, once the daemon listed the entries.
fn open_vault(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<VaultSection> {
    click("settings", cx);
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(cx);
    click("settings-tabs-vault", cx);
    let section = shell
        .read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .settings_page()
                .map(|page| page.read(cx).vault().clone())
        })
        .unwrap();
    until(shell, cx, "the entries are read", |cx| {
        section.read_with(cx, |s, _| s.entries().is_some())
    });
    settle(cx);
    section
}

fn type_in(
    section: &Entity<VaultSection>,
    field: fn(&VaultSection) -> Option<&Entity<gpui_kit::base::input::InputState>>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    let field = section.read_with(cx, |s, _| field(s).unwrap().clone());
    let text = text.to_owned();
    cx.update(|window, cx| field.update(cx, |f, cx| f.set_value(text, window, cx)));
}

fn listed(user: &User) -> Value {
    user.call("vault.list", json!({})).unwrap()["entries"].clone()
}

#[gpui_kit::test]
fn the_vault_page_adds_replaces_and_removes_an_entry_and_never_shows_its_secret(
    cx: &mut TestAppContext,
) {
    let user = User::new();
    let (shell, cx) = window(&user, cx);
    let section = open_vault(&shell, cx);
    assert!(shown("no-vault", cx), "an empty vault says what to add");

    // Added: the dialog's secret field is masked, and empty once the secret is sent.
    click("add-vault-entry", cx);
    settle(cx);
    assert!(section.read_with(cx, |s, _| s.is_open()));
    assert!(
        section.read_with(cx, |s, cx| s
            .secret_field()
            .unwrap()
            .read(cx)
            .presentation()
            .is_masked()),
        "the secret is typed in a masked field"
    );
    type_in(&section, VaultSection::name_field, "jira", cx);
    type_in(&section, VaultSection::secret_field, SECRET, cx);
    type_in(
        &section,
        VaultSection::description_field,
        "Logs time on Jira",
        cx,
    );
    click("save-vault-entry", cx);
    assert_eq!(
        section.read_with(cx, |s, cx| s
            .secret_field()
            .unwrap()
            .read(cx)
            .value()
            .to_string()),
        "",
        "the field lets go of the secret as soon as it is sent"
    );
    until(&shell, cx, "the entry is listed", |cx| {
        section.read_with(cx, |s, _| s.entries().is_some_and(|e| e.len() == 1))
    });
    assert_eq!(listed(&user)[0]["description"], "Logs time on Jira");
    assert!(toasts(&shell, cx).contains(&"Entry jira added".to_owned()));
    // The row unfolds into the list, and the empty line folds away.
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let partway = cx.update(|window, _| window.find("vault-item-jira").bounds().size.height);
    settle(cx);
    let whole = cx.update(|window, _| window.find("vault-item-jira").bounds().size.height);
    assert!(
        partway < whole,
        "the row unfolds: {partway:?} then {whole:?}"
    );
    assert!(!shown("no-vault", cx));
    assert!(
        !section.read_with(cx, |s, _| s.is_open()),
        "the dialog closed"
    );

    // Replaced from its row: the metadata is there, the secret is not.
    click("vault-jira", cx);
    settle(cx);
    assert!(section.read_with(cx, |s, _| s.is_open()));
    assert_eq!(
        section.read_with(cx, |s, cx| s
            .secret_field()
            .unwrap()
            .read(cx)
            .value()
            .to_string()),
        "",
        "a secret is never shown again"
    );
    assert_eq!(
        section.read_with(cx, |s, cx| s
            .description_field()
            .unwrap()
            .read(cx)
            .value()
            .to_string()),
        "Logs time on Jira"
    );
    // Without a secret the dialog says what is missing and stays open.
    click("save-vault-entry", cx);
    assert!(section.read_with(cx, |s, _| s.form_error().is_some()));
    type_in(
        &section,
        VaultSection::secret_field,
        &format!("{SECRET}-2"),
        cx,
    );
    type_in(&section, VaultSection::description_field, "Logs time", cx);
    click("save-vault-entry", cx);
    until(&shell, cx, "the entry is replaced", |cx| {
        toasts(&shell, cx).contains(&"Entry jira replaced".to_owned())
    });
    assert_eq!(listed(&user)[0]["description"], "Logs time");
    settle(cx);

    // Removed through its "⋯" and a confirmation.
    click("vault-more-jira", cx);
    settle(cx);
    click("remove-vault-jira", cx);
    settle(cx);
    assert!(section.read_with(cx, |s, _| s.is_confirming()));
    assert_eq!(listed(&user).as_array().unwrap().len(), 1, "asked first");
    click("vault-removal-action", cx);
    until(&shell, cx, "the entry is gone", |cx| {
        section.read_with(cx, |s, _| s.entries().is_some_and(<[_]>::is_empty))
    });
    assert_eq!(listed(&user), json!([]));
    assert!(toasts(&shell, cx).contains(&"Entry jira removed".to_owned()));
    // The row folds away before it is gone.
    assert!(shown("vault-item-jira", cx), "still folding");
    settle(cx);
    assert!(!shown("vault-item-jira", cx));
    assert!(shown("no-vault", cx));
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing is redrawn at rest"
    );
}

fn label_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

#[gpui_kit::test]
fn the_vault_creates_with_new_entry_and_saves_with_add(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = window(&user, cx);
    open_vault(&shell, cx);

    assert_eq!(
        label_of("add-vault-entry", cx).as_deref(),
        Some("New entry")
    );
    assert_eq!(
        label_of("vault-sentence", cx).as_deref(),
        Some(
            "Secrets agents use without seeing them. Your keyring keeps them; agents see only \
             their names and what they are for."
        )
    );
    assert_eq!(
        label_of("vault-usage", cx).as_deref(),
        Some(
            "An agent names an entry; Flottille puts the secret where it goes. A project's \
             tracker and a connection pick theirs here."
        )
    );

    click("add-vault-entry", cx);
    settle(cx);
    assert_eq!(
        label_of("vault-name-field-hint", cx).as_deref(),
        Some("How agents ask for it.")
    );
    assert_eq!(
        label_of("vault-project-field-label", cx).as_deref(),
        Some("Seen by agents of")
    );
    assert_eq!(
        label_of("save-vault-entry", cx).as_deref(),
        Some("Add"),
        "the dialog's title says it is new: its button only says Add"
    );
}
