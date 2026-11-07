//! The card of a request for a vault entry (`docs/vault.md`, "Asking the user for an entry"): an
//! agent met an entry a feature of Flottille names and that is missing, and asked the user for it.
//! Flottille prefilled the entry's name, what it is for and its project; the user types the
//! secret in a masked field (and the username when the entry takes one), then Set or Dismiss. The
//! secret goes to the daemon (`vault.ask_set`) and leaves the field at once; the card folds away
//! once the daemon says the request was answered.

use flottille_ui::{Button, Card, Field, Toast, palette, presence, space, toast};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::actions::{Command, VaultAnswer};
use crate::commands::{boxed, line_field, perform_later};
use crate::conversation::VaultAsk;
use crate::shell::Shell;

/// What the card says of the secret, under its field.
pub const SECRET_HINT: &str =
    "Kept by your keyring; the agent never sees it, only that you set it.";

/// The fields and the buttons of a request's card, in their own view: the caret blinks without
/// drawing the row.
pub struct VaultAskBox {
    at: usize,
    ask: VaultAsk,
    session: String,
    shell: WeakEntity<Shell>,
    secret: Entity<InputState>,
    username: Entity<InputState>,
    sending: bool,
    _subscriptions: Vec<Subscription>,
}

impl VaultAskBox {
    pub fn new(
        at: usize,
        ask: VaultAsk,
        session: String,
        shell: WeakEntity<Shell>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let secret = cx.new(|cx| {
            let mut field = InputState::new(window, cx).masked(true);
            flottille_ui::follow_theme(&mut field, cx);
            field
        });
        let username = line_field("me@example.com", window, cx);
        let mut subscriptions = Vec::new();
        for field in [&secret, &username] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |this: &mut Self, _, event, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.set(window, cx),
                    _ => {}
                },
            ));
        }
        VaultAskBox {
            at,
            ask,
            session,
            shell,
            secret,
            username,
            sending: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn secret(&self) -> &Entity<InputState> {
        &self.secret
    }

    pub fn username(&self) -> &Entity<InputState> {
        &self.username
    }

    fn ready(&self, cx: &gpui_kit::App) -> bool {
        !self.sending
            && !self.secret.read(cx).value().is_empty()
            && (!self.ask.takes_username || !self.username.read(cx).value().trim().is_empty())
    }

    /// Sends the secret typed, which leaves the field at once.
    pub fn set(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        // The secret as typed: spaces may be part of it.
        let secret = self.secret.read(cx).value().to_string();
        let username = self.username.read(cx).value().trim().to_owned();
        self.secret
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.answer(VaultAnswer::Set { secret, username }, cx);
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.sending {
            self.answer(VaultAnswer::Dismiss, cx);
        }
    }

    fn answer(&mut self, answer: VaultAnswer, cx: &mut Context<Self>) {
        self.sending = true;
        let command = Command::VaultAnswer {
            session: self.session.clone(),
            ask: self.ask.seq,
            answer,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// What the daemon answered: a refusal is said in a toast and the card stays to try again;
    /// taken, the card folds away when the request's answer comes.
    pub fn answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.sending = false;
        if let Some(why) = refused {
            toast(
                Toast::failure("The vault entry was not set").detail(why),
                cx,
            );
        }
        cx.notify();
    }
}

impl Render for VaultAskBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = |part: &str| ElementId::Name(format!("row-{}-{part}", self.at).into());
        let ready = self.ready(cx);
        let username = Field::new(
            id("vault-ask-username-field"),
            "Username",
            boxed(id("vault-ask-username"), Input::new(&self.username), cx),
        )
        .hint("The account that goes with the secret.");
        let secret = Field::new(
            id("vault-ask-secret-field"),
            "Secret",
            boxed(id("vault-ask-secret"), Input::new(&self.secret), cx),
        )
        .hint(SECRET_HINT);
        let buttons = div()
            .flex()
            .justify_end()
            .gap(space::SM)
            .child(
                Button::quiet(id("vault-ask-dismiss"), "Dismiss")
                    .disabled(self.sending)
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
            )
            .child(
                Button::primary(id("vault-ask-set"), "Set")
                    .disabled(!ready)
                    .on_click(cx.listener(|this, _, window, cx| this.set(window, cx))),
            );
        div()
            .flex()
            .flex_col()
            .gap(space::MD)
            .when_some(self.ask.takes_username.then_some(username), |d, u| {
                d.child(u)
            })
            .child(secret)
            .child(buttons)
    }
}

/// The card of the request: what Flottille prefilled, the agent's reason, and the fields; it
/// folds away once the request is answered.
pub fn card(
    at: usize,
    me: &str,
    when: SharedString,
    ask: &VaultAsk,
    fields: Entity<VaultAskBox>,
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
    let card = Card::new()
        .title(SharedString::from(format!("{me} asks for a vault entry")))
        .meta(when)
        .child(
            div()
                .px(space::LG)
                .pt(space::MD)
                .flex()
                .flex_col()
                .gap(space::XS)
                .child(line("vault-ask-name", "Entry", &ask.name))
                .child(line("vault-ask-what", "For", &ask.description))
                .child(line("vault-ask-project", "Project", &ask.project))
                .child(line("vault-ask-why", "Why", &ask.why)),
        )
        .child(div().px(space::LG).py(space::MD).child(fields));
    div()
        .id(id("vault-ask"))
        .child(presence(id("vault-ask-open"), ask.open, card))
        .test_support()
        .into_any_element()
}
