//! The card of a connection waiting for the user's review after an agent's request they accepted
//! (#1031, `docs/extensions.md`, "Connections"): the edit is written, but its server now announces
//! actions nobody read, and none is given to an agent before the user reviews them. The card is in
//! the session of the agent that asked, says which actions are to be read, and **Review** opens
//! the connection's review over the page. It folds away once the connection is reviewed, removed
//! or turned off (`connection.review_done`).

use gpui_design_system::{Button, Card, palette, presence, space};
use gpui_kit::{
    AnyElement, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, WeakEntity, div, px,
};

use crate::conversation::ConnectionReview;
use crate::shell::Shell;

/// The card: what waits, the actions to read, and the way into the review; it unfolds when the
/// connection starts to wait and folds away once the review is done.
pub fn card(
    at: usize,
    me: &str,
    when: SharedString,
    review: &ConnectionReview,
    shell: WeakEntity<Shell>,
    cx: &gpui_kit::App,
) -> AnyElement {
    let p = palette(cx);
    let id = |part: &str| ElementId::Name(format!("row-{at}-{part}").into());
    let said = format!("{} waits for your review of its new actions", review.name);
    let line = div()
        .id(id("review-said"))
        .px(space::LG)
        .pt(space::MD)
        .text_size(px(13.))
        .aria_label(SharedString::from(said.clone()))
        .child(SharedString::from(said))
        .test_support();
    let actions = (!review.actions.is_empty()).then(|| {
        let which = format!("To read: {}", review.actions.join(", "));
        div()
            .id(id("review-actions"))
            .px(space::LG)
            .pt(space::XS)
            .text_size(px(13.))
            .text_color(p.mute)
            .aria_label(SharedString::from(which.clone()))
            .child(SharedString::from(which))
            .test_support()
    });
    let name = review.name.clone();
    let buttons = div()
        .px(space::LG)
        .py(space::MD)
        .flex()
        .justify_end()
        .child(
            Button::primary(id("review-open"), "Review").on_click(move |_, _, cx| {
                let name = name.clone();
                shell
                    .update(cx, |shell, cx| shell.review_connection(&name, cx))
                    .ok();
            }),
        );
    let mut card = Card::new()
        .title(SharedString::from(format!("{me} edited a connection")))
        .meta(when)
        .child(line);
    if let Some(actions) = actions {
        card = card.child(actions);
    }
    let card = card.child(buttons);
    div()
        .id(id("review-card"))
        .child(presence(id("review-card-open"), review.open, card))
        .test_support()
        .into_any_element()
}
