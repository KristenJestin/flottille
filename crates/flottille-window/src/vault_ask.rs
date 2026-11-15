//! The card of a request for a vault entry (`docs/vault.md`, "Asking the user for an entry"): an
//! agent needs an entry the vault lacks and asked the user to add it. The card says what was
//! prefilled (by the agent, or by the feature that names the entry), the host the secret will go
//! to on its own line (the daemon's reading of the URL, a login's, never the window's), and the
//! agent's reason. It holds no field: "New entry" opens
//! Settings, Vault's own form of a new entry, prefilled, where the user changes what they want and
//! types the secret; Dismiss closes the request. It folds away once the daemon says the request
//! was answered.

use gpui_design_system::{Button, Card, palette, presence, space};
use gpui_kit::{
    AnyElement, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, div, px,
};

use crate::conversation::VaultAsk;
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
    }
}

/// The card of the request: what was prefilled, where the secret goes, the agent's reason, and
/// the way in to the vault's form; it folds away once the request is answered.
pub fn card(
    at: usize,
    me: &str,
    when: SharedString,
    ask: &VaultAsk,
    shell: WeakEntity<Shell>,
    cx: &gpui_kit::App,
) -> AnyElement {
    let p = palette(cx);
    let id = |part: &str| ElementId::Name(format!("row-{at}-{part}").into());
    let line = |part: &str, label: &str, value: &str| {
        let said = format!("{label}: {value}");
        div()
            .id(id(part))
            .flex()
            .gap(space::SM)
            .text_size(px(13.))
            .aria_label(SharedString::from(said))
            .child(
                div()
                    .text_color(p.mute)
                    .child(SharedString::from(label.to_owned())),
            )
            .child(div().min_w_0().child(SharedString::from(value.to_owned())))
            .test_support()
    };
    let used = ask
        .bound
        .as_deref()
        .map(|bound| flottille_core::vault::use_said(bound).unwrap_or_else(|| bound.to_owned()));
    let project = if ask.every {
        EVERY_PROJECT
    } else {
        ask.project.as_str()
    };
    let lines = div()
        .px(space::LG)
        .pt(space::MD)
        .flex()
        .flex_col()
        .gap(space::XS)
        .child(line("vault-ask-name", "Entry", &ask.name))
        .children(
            ask.host
                .as_deref()
                .map(|host| line("vault-ask-host", "Goes to", host)),
        )
        .children(
            ask.description
                .as_deref()
                .map(|what| line("vault-ask-what", "What it is for", what)),
        )
        .children(used.map(|used| line("vault-ask-for", "For", &used)))
        .child(line("vault-ask-project", "Project", project))
        .child(line("vault-ask-why", "Why", &ask.why));
    let number = ask.seq;
    let entry = prefilled(ask);
    let (why, host) = (ask.why.clone(), ask.host.clone());
    let dismiss_to = shell.clone();
    let dismissing = shell
        .upgrade()
        .is_some_and(|shell| shell.read(cx).is_dismissing(number));
    let buttons = div()
        .px(space::LG)
        .py(space::MD)
        .flex()
        .justify_end()
        .gap(space::SM)
        .child(
            Button::quiet(id("vault-ask-dismiss"), "Dismiss")
                .disabled(dismissing)
                .on_click(move |_, _, cx| {
                    dismiss_to
                        .update(cx, |shell, cx| shell.dismiss_vault_ask(number, cx))
                        .ok();
                }),
        )
        .child(
            Button::primary(id("vault-ask-add"), "New entry").on_click(move |_, window, cx| {
                let (entry, why, host) = (entry.clone(), why.clone(), host.clone());
                shell
                    .update(cx, |shell, cx| {
                        shell.add_asked_vault_entry(entry, why, host, window, cx)
                    })
                    .ok();
            }),
        );
    let card = Card::new()
        .title(SharedString::from(format!("{me} asks for a vault entry")))
        .meta(when)
        .child(lines)
        .child(buttons);
    div()
        .id(id("vault-ask"))
        .child(presence(id("vault-ask-open"), ask.open, card))
        .test_support()
        .into_any_element()
}
