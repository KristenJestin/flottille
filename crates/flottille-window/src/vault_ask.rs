//! The card of a request for a vault entry (`docs/vault.md`, "Asking the user for an entry"): an
//! agent needs an entry the vault lacks and asked the user to add it. The card says what was
//! prefilled (by the agent, or by the feature that names the entry), the host the secret will go
//! to on its own line, and the agent's reason. It holds no field: "Add to the vault" opens
//! Settings, Vault's own form of a new entry, prefilled, where the user changes what they want and
//! types the secret; Dismiss closes the request. It folds away once the daemon says the request
//! was answered.

use gpui_design_system::{Button, Card, palette, presence, space};
use gpui_kit::{
    AnyElement, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, div, px,
};

use crate::actions::Command;
use crate::commands::perform_later;
use crate::conversation::VaultAsk;
use crate::shell::Shell;
use crate::vault::{EVERY_PROJECT, VaultEntry};

/// The host of `url`, where a secret given for it goes: without its scheme, user, port or path.
pub fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = match authority.strip_prefix('[') {
        // An IPv6 address keeps its brackets' content whole.
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

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
            ask.url
                .as_deref()
                .and_then(host)
                .map(|host| line("vault-ask-host", "Goes to", &host)),
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
    let why = ask.why.clone();
    let dismiss_to = shell.clone();
    let buttons = div()
        .px(space::LG)
        .py(space::MD)
        .flex()
        .justify_end()
        .gap(space::SM)
        .child(
            Button::quiet(id("vault-ask-dismiss"), "Dismiss").on_click(move |_, _, cx| {
                perform_later(&dismiss_to, Command::VaultDismiss { ask: number }, cx)
            }),
        )
        .child(
            Button::primary(id("vault-ask-add"), "Add to the vault").on_click(
                move |_, window, cx| {
                    let (entry, why) = (entry.clone(), why.clone());
                    shell
                        .update(cx, |shell, cx| {
                            shell.add_asked_vault_entry(entry, why, window, cx)
                        })
                        .ok();
                },
            ),
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

#[cfg(test)]
mod tests {
    use super::host;

    #[test]
    fn the_host_is_said_without_user_port_or_path() {
        for (url, said) in [
            (
                "https://tester@Staging.example.com:8443/login?next=/",
                Some("staging.example.com"),
            ),
            ("http://[::1]:8080/", Some("::1")),
            ("shop.example.com/login", Some("shop.example.com")),
            ("https:///nothing", None),
        ] {
            assert_eq!(host(url).as_deref(), said, "{url}");
        }
    }
}
