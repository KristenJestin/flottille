//! The messages the user wrote to the root while it worked: the daemon gives each to the root
//! between its turns, and until then they stay under the conversation, drawn as waiting (quieter
//! than a message sent, with the sentence that says when it goes). Each one folds away once the
//! root read it, when the conversation shows it as a message like the others.

use flottille_ui::{Icon, Item, Listing, Placement, Type, icon, palette, space};
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, TestSupportExt as _, Window, div, px,
};

use crate::session_link::Queued;

/// What a waiting message says of itself.
pub const WAITS: &str = "Sent when the agent finishes its step";

#[derive(Default)]
pub struct QueuedMessages {
    messages: Vec<Queued>,
    list: Listing<Queued>,
}

impl QueuedMessages {
    pub fn messages(&self) -> &[Queued] {
        &self.messages
    }

    pub fn set(&mut self, messages: Vec<Queued>, cx: &mut Context<Self>) {
        if self.messages != messages {
            self.messages = messages;
            cx.notify();
        }
    }
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
                // The gap under it is its own: it folds away with it.
                let content = div().pb(space::SM).flex().justify_end().child(bubble);
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
