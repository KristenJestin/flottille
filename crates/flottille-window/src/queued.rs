//! The messages the user wrote to the root while it worked: the daemon gives each to the root
//! between its turns, and until then they stay under the conversation, drawn as waiting (quieter
//! than a message sent, with the sentence that says when it goes). Each one folds away once the
//! root read it, when the conversation shows it as a message like the others.
//!
//! A waiting message can still be changed (never one the root has read): Cancel takes it back,
//! Edit takes it back into the composer, Send now stops the root's turn and hands it this one next.
//! The daemon decides each, in one transaction; the window only asks.

use std::collections::HashSet;

use flottille_ui::{Button, Icon, Item, Listing, Placement, Type, icon, palette, space};
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, Window,
    div, px,
};

use crate::actions::Command;
use crate::session_link::Queued;
use crate::shell::Shell;

/// What a waiting message says of itself.
pub const WAITS: &str = "Sent when the agent finishes its step";

pub struct QueuedMessages {
    shell: WeakEntity<Shell>,
    /// The session's root: the only agent a message waits for.
    session: String,
    messages: Vec<Queued>,
    list: Listing<Queued>,
    /// The messages a request was sent for and not answered yet: their buttons wait.
    asked: HashSet<String>,
}

impl QueuedMessages {
    pub fn new(shell: WeakEntity<Shell>, session: String) -> Self {
        Self {
            shell,
            session,
            messages: Vec::new(),
            list: Listing::default(),
            asked: HashSet::new(),
        }
    }

    pub fn messages(&self) -> &[Queued] {
        &self.messages
    }

    pub fn set(&mut self, messages: Vec<Queued>, cx: &mut Context<Self>) {
        if self.messages != messages {
            self.messages = messages;
            cx.notify();
        }
    }

    /// Asks the daemon to take the message back (`edit`: to put it back in the composer).
    pub fn unqueue(&mut self, delivery: &str, edit: bool, cx: &mut Context<Self>) {
        self.ask(
            delivery,
            |session, delivery| Command::Unqueue {
                session,
                delivery,
                edit,
            },
            cx,
        );
    }

    /// Asks the daemon to stop the root's turn and hand it this message next.
    pub fn send_now(&mut self, delivery: &str, cx: &mut Context<Self>) {
        self.ask(
            delivery,
            |session, delivery| Command::SendNow { session, delivery },
            cx,
        );
    }

    fn ask(
        &mut self,
        delivery: &str,
        command: impl FnOnce(String, String) -> Command,
        cx: &mut Context<Self>,
    ) {
        if !self.asked.insert(delivery.to_owned()) {
            return;
        }
        let command = command(self.session.clone(), delivery.to_owned());
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    /// The daemon answered for a message: its buttons are live again, and when it `left` the queue
    /// (taken back, or read by the root first) its bubble folds away without waiting for the next
    /// read of the tree.
    pub fn answered(&mut self, delivery: &str, left: bool, cx: &mut Context<Self>) {
        self.asked.remove(delivery);
        if left {
            self.messages.retain(|m| m.delivery != delivery);
        }
        cx.notify();
    }
}

/// What a waiting message's buttons are called.
pub const SEND_NOW: &str = "Send now";
pub const EDIT: &str = "Edit";
pub const CANCEL: &str = "Cancel";

/// The id of one of a waiting message's buttons: `send-now`, `edit` or `cancel`.
pub fn queued_button(delivery: &str, which: &str) -> ElementId {
    ElementId::Name(format!("queued-{delivery}-{which}").into())
}

/// The id of a waiting message's bubble.
pub fn queued_id(delivery: &str) -> ElementId {
    ElementId::Name(format!("queued-{delivery}").into())
}

impl Render for QueuedMessages {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        self.list.sync(
            self.messages
                .iter()
                .map(|m| (m.delivery.clone(), m.clone())),
            now,
            cx.reduce_motion(),
        );
        let p = palette(cx);
        let listed: Vec<(String, Queued, Placement)> = self
            .list
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let items: Vec<AnyElement> = listed
            .into_iter()
            .map(|(key, message, placement)| {
                let pictures = match message.images {
                    0 => None,
                    1 => Some("1 picture".to_owned()),
                    n => Some(format!("{n} pictures")),
                };
                let waits = SharedString::from(match pictures {
                    Some(pictures) => format!("{pictures} · {WAITS}"),
                    None => WAITS.to_owned(),
                });
                let text = SharedString::from(message.text.clone());
                let bubble = div()
                    .id(queued_id(&key))
                    .max_w(px(540.))
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .border_1()
                    .border_color(p.line2)
                    .rounded_tl(px(14.))
                    .rounded_tr(px(14.))
                    .rounded_bl(px(14.))
                    .rounded_br(px(4.))
                    .px(space::MD)
                    .py(space::SM)
                    .aria_label(text.clone())
                    .child(
                        div()
                            .max_h(px(60.))
                            .overflow_hidden()
                            .text_size(px(13.5))
                            .text_color(p.mute)
                            .child(text),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("queued-{key}-waits").into()))
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(space::XS)
                            .mono(11.)
                            .text_color(p.faint)
                            .aria_label(waits.clone())
                            .child(icon(Icon::Clock).size(px(11.)).text_color(p.faint))
                            .child(waits)
                            .test_support(),
                    )
                    .test_support();
                let waiting = self.asked.contains(&key);
                let button = |which: &str, label: &str, glyph: Icon| {
                    Button::quiet(queued_button(&key, which), label.to_owned())
                        .leading(glyph)
                        .disabled(waiting)
                };
                let on = |act: fn(&mut QueuedMessages, &str, &mut Context<QueuedMessages>)| {
                    let delivery = key.clone();
                    cx.listener(move |this, _, _, cx| act(this, &delivery, cx))
                };
                let buttons = div()
                    .id(ElementId::Name(format!("queued-{key}-buttons").into()))
                    .flex()
                    .justify_end()
                    .gap(space::XS)
                    .child(
                        button("send-now", SEND_NOW, Icon::Send)
                            .on_click(on(|this, delivery, cx| this.send_now(delivery, cx))),
                    )
                    .child(
                        button("edit", EDIT, Icon::Text)
                            .on_click(on(|this, delivery, cx| this.unqueue(delivery, true, cx))),
                    )
                    .child(
                        button("cancel", CANCEL, Icon::Close)
                            .on_click(on(|this, delivery, cx| this.unqueue(delivery, false, cx))),
                    )
                    .test_support();
                // The gap under it is its own: it folds away with it.
                let content = div()
                    .pb(space::SM)
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(space::XS)
                    .child(bubble)
                    .child(buttons);
                Item::new(
                    ElementId::Name(format!("queued-item-{key}").into()),
                    placement,
                    content,
                )
                .into_any_element()
            })
            .collect();
        div()
            .id("queued")
            .w_full()
            .flex()
            .flex_col()
            .children(items)
            .test_support()
    }
}

/// The sentence the daemon gives for a message the root read before it could be changed.
pub const ALREADY_READ: &str = "already read by the agent";

/// The toast a refused request says. A message the root read first is a title of its own, and
/// the message shows as sent, like the others; any other refusal says what was not done.
pub fn refused_toast(why: &str, not_done: &str) -> flottille_ui::Toast {
    if why == ALREADY_READ {
        return flottille_ui::Toast::failure("Already read by the agent")
            .detail("It is sent: it shows in the conversation.");
    }
    flottille_ui::Toast::failure(not_done.to_owned()).detail(why.to_owned())
}
