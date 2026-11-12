//! The messages the user wrote to the root while it worked: the daemon gives each to the root
//! between its turns, and until then they stay under the conversation, drawn as waiting (quieter
//! than a message sent, with the sentence that says when it goes). Each one folds away once the
//! root read it, when the conversation shows it as a message like the others.
//!
//! A waiting message can still be changed (never one the root has read): Cancel takes it back,
//! Edit takes it back into the composer, Send now stops the root's turn and hands it this one next.
//! The daemon decides each, in one transaction; the window only asks.
//!
//! A message sent from the composer while the root works shows here at once, being sent, and
//! becomes the waiting one in its place once the daemon put it in the queue.

use crate::steps::commanded_text;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_design_system::{
    Button, Icon, Item, Listing, Placement, changing, icon, palette, presence, space, tipped,
};
use gpui_kit::{
    AnyElement, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, StyledImage as _, TestSupportExt as _,
    WeakEntity, Window, div, px,
};

use crate::actions::Command;
use crate::session::{NOT_SENT_LINE, Pending, RETRY, Reply, SENDING, command_chips};
use crate::session_link::Queued;
use crate::shell::Shell;

/// What a waiting message says of itself, in one short line; the whole sentence is its tooltip.
/// A message waits only while the root's turn runs, so it is always "after this step".
pub const WAITS: &str = "Queued · after this step";
pub const WAITS_FULL: &str = "Sent when the agent finishes its step";
/// The same, while the conversation is compacted: the root has no step to finish.
pub const WAITS_COMPACTION: &str = "Queued · after the compaction";
pub const WAITS_COMPACTION_FULL: &str = "Sent when the compaction of the conversation is over";

pub struct QueuedMessages {
    shell: WeakEntity<Shell>,
    /// The session's root: the only agent a message waits for.
    session: String,
    messages: Vec<Queued>,
    /// The root's conversation is compacted: what waits, waits for that.
    compacting: bool,
    /// The messages sent from the composer while the root worked, not in the queue yet.
    pending: Vec<Pending>,
    /// The waiting messages that were one of those, by delivery: the key they are drawn under.
    aliases: HashMap<String, String>,
    list: Listing<Entry>,
    /// The messages a request was sent for and not answered yet: their buttons wait.
    asked: HashSet<String>,
    /// The pictures of the waiting messages, read back from the daemon by name.
    pictures: HashMap<String, Arc<gpui_kit::Image>>,
}

impl QueuedMessages {
    pub fn new(shell: WeakEntity<Shell>, session: String) -> Self {
        Self {
            shell,
            session,
            messages: Vec::new(),
            compacting: false,
            pending: Vec::new(),
            aliases: HashMap::new(),
            list: Listing::default(),
            asked: HashSet::new(),
            pictures: HashMap::new(),
        }
    }

    /// The root the messages wait for.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// A picture of a waiting message, read back: its thumbnail shows.
    pub fn picture_read(
        &mut self,
        name: &str,
        picture: Arc<gpui_kit::Image>,
        cx: &mut Context<Self>,
    ) {
        self.pictures.insert(name.to_owned(), picture);
        cx.notify();
    }

    /// How many pictures of the waiting messages are read.
    pub fn pictures_read(&self) -> usize {
        self.messages
            .iter()
            .flat_map(|m| &m.pictures)
            .filter(|name| self.pictures.contains_key(*name))
            .count()
    }

    pub fn messages(&self) -> &[Queued] {
        &self.messages
    }

    pub fn set_compacting(&mut self, compacting: bool, cx: &mut Context<Self>) {
        if self.compacting != compacting {
            self.compacting = compacting;
            cx.notify();
        }
    }

    pub fn set(&mut self, messages: Vec<Queued>, cx: &mut Context<Self>) {
        if self.messages != messages {
            self.aliases
                .retain(|delivery, _| messages.iter().any(|m| m.delivery == *delivery));
            self.messages = messages;
            cx.notify();
        }
    }

    /// The messages being sent that are to wait here.
    pub fn set_pending(&mut self, pending: Vec<Pending>, cx: &mut Context<Self>) {
        let said = |list: &[Pending]| -> Vec<(u64, Option<String>)> {
            list.iter().map(|p| (p.key, p.refused.clone())).collect()
        };
        if said(&self.pending) != said(&pending) {
            self.pending = pending;
            cx.notify();
        }
    }

    /// The message being sent `key` is the waiting `delivery` now: it is drawn in its place.
    pub fn alias(&mut self, delivery: String, key: u64, cx: &mut Context<Self>) {
        self.aliases.insert(delivery, sending_key(key));
        cx.notify();
    }

    fn reply(&self, cx: &gpui_kit::App) -> Option<Entity<Reply>> {
        let shell = self.shell.upgrade()?;
        shell.read(cx).reply_of(&self.session).cloned()
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

/// The key a message being sent is drawn under, and the waiting one it becomes.
fn sending_key(key: u64) -> String {
    format!("pending-{key}")
}

/// A bubble under the conversation: a message waiting for the root, or one being sent.
#[derive(Clone)]
enum Entry {
    Waiting(Queued),
    Sending(Pending),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let entries = self
            .messages
            .iter()
            .map(|m| {
                let key = self.aliases.get(&m.delivery).cloned();
                (
                    key.unwrap_or_else(|| m.delivery.clone()),
                    Entry::Waiting(m.clone()),
                )
            })
            .chain(
                self.pending
                    .iter()
                    .map(|p| (sending_key(p.key), Entry::Sending(p.clone()))),
            );
        self.list.sync(entries, now, cx.reduce_motion());
        let p = palette(cx);
        let listed: Vec<(String, Entry, Placement)> = self
            .list
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let items: Vec<AnyElement> = listed
            .into_iter()
            .map(|(key, entry, placement)| {
                let (text, commands) = match &entry {
                    Entry::Waiting(m) => (m.text.clone(), m.commands.clone()),
                    Entry::Sending(p) => (p.text.clone(), Vec::new()),
                };
                let text = SharedString::from(text);
                let pictures: Vec<Option<Arc<gpui_kit::Image>>> = match &entry {
                    Entry::Waiting(m) => m
                        .pictures
                        .iter()
                        .map(|name| self.pictures.get(name).cloned())
                        .collect(),
                    Entry::Sending(p) => p.images.iter().cloned().map(Some).collect(),
                };
                let thumbs = (!pictures.is_empty()).then(|| {
                    div()
                        .id(ElementId::Name(format!("queued-{key}-images").into()))
                        .flex()
                        .flex_wrap()
                        .justify_end()
                        .gap(space::SM)
                        .children(pictures.into_iter().enumerate().map(|(n, picture)| {
                            let thumb = div()
                                .id(ElementId::Name(format!("queued-{key}-image-{n}").into()))
                                .flex_none()
                                .size(px(40.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(p.line2)
                                .bg(p.card)
                                .overflow_hidden();
                            match picture {
                                Some(picture) => thumb.child(
                                    gpui_kit::img(picture)
                                        .size_full()
                                        .rounded(px(6.))
                                        .object_fit(gpui_kit::ObjectFit::Cover),
                                ),
                                None => thumb,
                            }
                            .test_support()
                        }))
                });
                // What the user typed, each command a chip in it.
                let shown = match commands.is_empty() {
                    true => text.clone().into_any_element(),
                    false => commanded_text(&text, &command_chips(&text, &commands), cx)
                        .into_any_element(),
                };
                // One line says where it is: waiting, being sent, or refused and why.
                let waits_id = ElementId::Name(format!("queued-{key}-waits").into());
                let line = |glyph: Icon, said: SharedString, ink| {
                    div()
                        .id(waits_id.clone())
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(space::XS)
                        .text_size(px(11.5))
                        .text_color(ink)
                        .aria_label(said.clone())
                        .child(icon(glyph).size(px(11.)).flex_none().text_color(ink))
                        .child(div().min_w_0().child(said))
                };
                let (said, line) = match &entry {
                    Entry::Waiting(_) => {
                        let (waits, full) = match self.compacting {
                            true => (WAITS_COMPACTION, WAITS_COMPACTION_FULL),
                            false => (WAITS, WAITS_FULL),
                        };
                        let tip = tipped(
                            line(Icon::Clock, waits.into(), p.faint),
                            full,
                            16.,
                            window,
                            cx,
                        )
                        .into_any_element();
                        // What it waits for changes under it: the sentence fades to the next.
                        (
                            SharedString::from(waits),
                            changing(
                                ElementId::Name(format!("queued-{key}-waits-for").into()),
                                waits,
                                move |_| tip,
                            )
                            .into_any_element(),
                        )
                    }
                    Entry::Sending(Pending {
                        refused: Some(why), ..
                    }) => {
                        let said = SharedString::from(format!("{NOT_SENT_LINE} · {why}"));
                        (
                            said.clone(),
                            line(Icon::Warning, said, p.fail)
                                .test_support()
                                .into_any_element(),
                        )
                    }
                    Entry::Sending(_) => (
                        SharedString::from(SENDING),
                        line(Icon::Clock, SENDING.into(), p.faint)
                            .test_support()
                            .into_any_element(),
                    ),
                };
                let line = changing(
                    ElementId::Name(format!("queued-{key}-line").into()),
                    said,
                    move |_| line,
                );
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
                    .children(thumbs)
                    .child(
                        div()
                            .id(ElementId::Name(format!("queued-{key}-text").into()))
                            .max_h(px(60.))
                            .overflow_hidden()
                            .text_size(px(13.5))
                            .text_color(p.mute)
                            .child(shown)
                            .test_support(),
                    )
                    .child(line)
                    .test_support();
                let buttons: Vec<AnyElement> = match &entry {
                    Entry::Waiting(message) => {
                        let delivery = message.delivery.clone();
                        let waiting = self.asked.contains(&delivery);
                        let button = |which: &str, label: &str, glyph: Icon| {
                            Button::quiet(queued_button(&key, which), label.to_owned())
                                .leading(glyph)
                                .disabled(waiting)
                        };
                        let on =
                            |act: fn(&mut QueuedMessages, &str, &mut Context<QueuedMessages>)| {
                                let delivery = delivery.clone();
                                cx.listener(move |this, _, _, cx| act(this, &delivery, cx))
                            };
                        vec![
                            button("send-now", SEND_NOW, Icon::Send)
                                .on_click(on(|this, delivery, cx| this.send_now(delivery, cx)))
                                .into_any_element(),
                            button("edit", EDIT, Icon::Edit)
                                .on_click(on(|this, delivery, cx| this.unqueue(delivery, true, cx)))
                                .into_any_element(),
                            button("cancel", CANCEL, Icon::Close)
                                .on_click(on(|this, delivery, cx| {
                                    this.unqueue(delivery, false, cx)
                                }))
                                .into_any_element(),
                        ]
                    }
                    Entry::Sending(pending) if pending.refused.is_some() => {
                        let sent = pending.key;
                        vec![
                            Button::quiet(queued_button(&key, "retry"), RETRY)
                                .leading(Icon::Restart)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(reply) = this.reply(cx) {
                                        reply.update(cx, |reply, cx| reply.retry(sent, cx));
                                    }
                                }))
                                .into_any_element(),
                            Button::quiet(queued_button(&key, "edit"), EDIT)
                                .leading(Icon::Edit)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(reply) = this.reply(cx) {
                                        reply.update(cx, |reply, cx| {
                                            reply.edit_pending(sent, window, cx)
                                        });
                                    }
                                }))
                                .into_any_element(),
                        ]
                    }
                    Entry::Sending(_) => Vec::new(),
                };
                let has_buttons = !buttons.is_empty();
                let buttons = div()
                    .id(ElementId::Name(format!("queued-{key}-buttons").into()))
                    .flex()
                    .justify_end()
                    .gap(space::XS)
                    .children(buttons)
                    .test_support();
                // The gap under it is its own: it folds away with it.
                let content = div()
                    .pb(space::SM)
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(space::XS)
                    // A flex item, which shrinks to the room there is: the words under it wrap.
                    .child(div().w_full().flex().justify_end().child(bubble.min_w_0()))
                    .child(
                        presence(
                            ElementId::Name(format!("queued-{key}-buttons-shown").into()),
                            has_buttons,
                            buttons,
                        )
                        .gap(space::XS),
                    );
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
pub fn refused_toast(why: &str, not_done: &str) -> gpui_design_system::Toast {
    if why == ALREADY_READ {
        return gpui_design_system::Toast::failure("Already read by the agent")
            .detail("It is sent: it shows in the conversation.");
    }
    gpui_design_system::Toast::failure(not_done.to_owned()).detail(why.to_owned())
}
