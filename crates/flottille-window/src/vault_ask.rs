//! A request for a vault entry (`docs/vault.md`, "Asking the user for an entry"): an agent needs
//! an entry the vault lacks and asked the user to add it. It waits on the user, so its card is in
//! the rail's "Waiting for you" (`waiting.rs`), the one place that says the user is needed; the
//! conversation only records it, in one quiet line (`line`) that says it was asked and, later, how
//! it ended.
//!
//! The card says what was prefilled (by the agent, or by the feature that names the entry), the
//! host the secret will go to on its own line (the daemon's reading of the URL, a login's, never
//! the window's), the project and the agent's reason. It holds no field: "New entry" opens
//! Settings, Vault's own form of a new entry, prefilled, where the user changes what they want and
//! types the secret; Dismiss closes the request. It folds away once the daemon says the request
//! was answered.
//!
//! An agent's proposed edit of an entry that exists is the same request and the same card
//! (`docs/vault.md`, "Proposing an edit of an entry"): each field changed on a line of its own,
//! before → after, then Dismiss, Edit (the entry's own form, prefilled with the proposal) and
//! Accept (the daemon applies it as the user's Save would, the secret kept).

use gpui_design_system::{Button, Palette, Type as _, presence, space};
use gpui_kit::{
    AnyElement, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, div, px,
};

use crate::conversation::{VaultAsk, VaultChange};
use crate::shell::Shell;
use crate::vault::{EVERY_PROJECT, VaultEntry};

/// The entry the form opens with: what the request prefilled, the user's to change.
fn prefilled(ask: &VaultAsk) -> VaultEntry {
    VaultEntry {
        name: ask.name.clone(),
        description: ask.description.clone(),
        url: ask.url.clone(),
        username: ask.username.clone(),
        project: (!ask.every).then(|| ask.project.clone()),
        bound: ask.bound.clone(),
        // The form is told by `connection`, which `add_asked` hands it.
        every_project_only: false,
        shared_from: None,
    }
}

/// The conversation's record of a request: one quiet line, with no button, that fades in. While
/// the request waits it says so, the card being in the rail; once answered, how it ended.
pub fn line(at: usize, ask: &VaultAsk, p: Palette) -> AnyElement {
    let said = SharedString::from(ask.said());
    let id = |part: &str| ElementId::Name(format!("row-{at}-vault-ask-{part}").into());
    // Cut at the card's edge when the entry's name is long: the whole line is its label.
    let line = div()
        .id(id("line"))
        .flex()
        .mono(12.)
        .text_color(p.faint)
        .aria_label(said.clone())
        .child(
            div()
                .id(id("line-text"))
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(said)
                .test_support(),
        )
        .test_support();
    div()
        .id(id("record"))
        .child(presence(id("line-shown"), true, line))
        .into_any_element()
}

/// How a line of the card is named: `vault-ask-<number>-<part>`.
pub fn part(number: u64, part: &str) -> ElementId {
    ElementId::Name(format!("vault-ask-{number}-{part}").into())
}

/// A labelled line of the card, its visible words said as its label. The label stands beside
/// its value, or above it when the value is a sentence the card's narrow width would wrap into
/// a tall column, which pushes the buttons out of the rail's sight.
fn labelled(
    number: u64,
    name: &str,
    label: &str,
    value: &str,
    stacked: bool,
    p: Palette,
) -> AnyElement {
    let said = format!("{label}: {value}");
    let line = div()
        .id(part(number, name))
        .flex()
        .text_size(px(12.))
        .aria_label(SharedString::from(said));
    match stacked {
        true => line.flex_col().gap(space::XS),
        false => line.gap(space::SM),
    }
    .child(
        div()
            .flex_none()
            .text_color(p.mute)
            .child(SharedString::from(label.to_owned())),
    )
    .child(div().min_w_0().child(SharedString::from(value.to_owned())))
    .test_support()
    .into_any_element()
}

/// Whose entry it will be, as the form saves it: an entry a connection's server is given is every
/// project's, and the card says why; a login's keeps the project asked.
fn project_said(ask: &VaultAsk) -> String {
    match (&ask.connection, ask.every) {
        (Some(connection), _) => format!("For the connection {connection} (every project)"),
        (None, true) => EVERY_PROJECT.to_owned(),
        (None, false) => ask.project.clone(),
    }
}

/// A field changed, as its line says it: its label and before → after, an empty side said so.
fn change_said(change: &VaultChange) -> (&'static str, String) {
    let label = match change.field.as_str() {
        "url" => "URL",
        "username" => "Username",
        "description" => "What it is for",
        "project" => "Project",
        _ => "For",
    };
    let side = |side: &Option<String>| match (change.field.as_str(), side.as_deref()) {
        ("project", None) => EVERY_PROJECT.to_owned(),
        (_, None) => "(empty)".to_owned(),
        ("for", Some(bound)) => {
            flottille_core::vault::use_said(bound).unwrap_or_else(|| bound.to_owned())
        }
        (_, Some(value)) => value.to_owned(),
    };
    (
        label,
        format!("{} → {}", side(&change.before), side(&change.after)),
    )
}

/// The lines the compact card shows: the entry, where its secret goes, the project, the reason.
/// A proposed edit shows each field changed, before → after, instead of the project.
pub fn lines(number: u64, ask: &VaultAsk, p: Palette) -> Vec<AnyElement> {
    let mut lines = vec![labelled(number, "name", "Entry", &ask.name, false, p)];
    for change in &ask.edit {
        let (label, said) = change_said(change);
        // An address or a sentence is too long to stand beside its label.
        let stacked = matches!(change.field.as_str(), "url" | "description");
        let name = format!("change-{}", change.field);
        lines.push(labelled(number, &name, label, &said, stacked, p));
    }
    if let Some(host) = ask.host.as_deref() {
        lines.push(labelled(number, "host", "Goes to", host, false, p));
    }
    if ask.proposes() {
        lines.push(labelled(number, "why", "Why", &ask.why, false, p));
        return lines;
    }
    lines.push(labelled(
        number,
        "project",
        "Project",
        &project_said(ask),
        false,
        p,
    ));
    lines.push(labelled(number, "why", "Why", &ask.why, false, p));
    lines
}

/// What "Details" unfolds: what the entry is for, the use it is bound to, who asked.
pub fn details(number: u64, ask: &VaultAsk, source: String, p: Palette) -> Vec<AnyElement> {
    let used = ask
        .bound
        .as_deref()
        .map(|bound| flottille_core::vault::use_said(bound).unwrap_or_else(|| bound.to_owned()));
    let mut lines = Vec::new();
    if let Some(what) = ask.description.as_deref() {
        lines.push(labelled(number, "what", "What it is for", what, true, p));
    }
    if let Some(used) = used {
        lines.push(labelled(number, "for", "For", &used, false, p));
    }
    lines.push(
        div()
            .id(part(number, "source"))
            .mono(11.)
            .text_color(p.faint)
            .aria_label(SharedString::from(source.clone()))
            .child(SharedString::from(source))
            .test_support()
            .into_any_element(),
    );
    lines
}

/// Dismiss and New entry, or for a proposed edit Dismiss, Edit and Accept: the card's buttons,
/// in the place and kind the other cards have theirs. New entry and Edit open the vault's form
/// over the page; Dismiss and Accept are sent once, every button disabled while one runs.
pub fn buttons(
    number: u64,
    ask: &VaultAsk,
    shell: WeakEntity<Shell>,
    cx: &gpui_kit::App,
) -> Vec<Button> {
    let entry = prefilled(ask);
    let (why, host, connection) = (ask.why.clone(), ask.host.clone(), ask.connection.clone());
    let dismiss_to = shell.clone();
    let answering = shell
        .upgrade()
        .is_some_and(|shell| shell.read(cx).is_answering(number));
    let dismiss = Button::quiet(part(number, "dismiss"), "Dismiss")
        .disabled(answering)
        .on_click(move |_, _, cx| {
            dismiss_to
                .update(cx, |shell, cx| shell.dismiss_vault_ask(number, cx))
                .ok();
        });
    if ask.proposes() {
        let (accept_to, name) = (shell.clone(), ask.name.clone());
        let edit = Button::secondary(part(number, "edit"), "Edit")
            .disabled(answering)
            .on_click(move |_, window, cx| {
                let (entry, why, host) = (entry.clone(), why.clone(), host.clone());
                shell
                    .update(cx, |shell, cx| {
                        shell.edit_proposed_vault_entry(entry, why, host, window, cx)
                    })
                    .ok();
            });
        let accept = Button::primary(part(number, "accept"), "Accept")
            .disabled(answering)
            .on_click(move |_, _, cx| {
                let name = name.clone();
                accept_to
                    .update(cx, |shell, cx| shell.accept_vault_ask(number, name, cx))
                    .ok();
            });
        return vec![dismiss, edit, accept];
    }
    let add = Button::primary(part(number, "add"), "New entry").on_click(move |_, window, cx| {
        let (entry, why, host) = (entry.clone(), why.clone(), host.clone());
        let connection = connection.clone();
        shell
            .update(cx, |shell, cx| {
                shell.add_asked_vault_entry(entry, why, host, connection, window, cx)
            })
            .ok();
    });
    vec![dismiss, add]
}
