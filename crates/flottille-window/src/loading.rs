//! What the sidebar and the page show until the daemon has answered: skeletons of the kit, so that
//! nothing looks finished before it is, and nothing is held back once it is ready.
//!
//! The page's skeleton is a veil laid over it that fades out when the daemon answered, whatever
//! the page under it is: what is ready is drawn at once, under the veil, and enters with its own
//! motion. The sidebar's is a block that stands in while it has no row at all and folds away, as a
//! `presence` does, as soon as it has one (or the daemon answered).
//!
//! A conversation's is its own view over the conversation: until the first page of its history
//! has arrived it says so, never that nothing was said.

use std::time::Duration;

use gpui_design_system::motion::PANEL;
use gpui_design_system::typography::BODY;
use gpui_design_system::{Button, Palette, Skeleton, fade, palette, presence_of, space};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, TestSupportExt as _,
    WeakEntity, Window, div, px, relative,
};

use crate::session::SessionPage;

/// The sidebar's rows, as the projects' will be: a line each.
pub fn sidebar() -> impl IntoElement {
    div()
        .id("sidebar-skeleton")
        .flex()
        .flex_col()
        .gap(space::MD)
        .px(space::MD)
        .pt(space::MD)
        .pb(space::LG)
        .children(
            [150., 118., 138., 96.]
                .into_iter()
                .enumerate()
                .map(|(row, width)| {
                    Skeleton::new(ElementId::Name(format!("sidebar-skeleton-{row}").into()))
                        .w(px(width))
                        .h(px(14.))
                }),
        )
        .test_support()
}

/// The page's skeleton laid over it while `pending`, fading out over [`PANEL`] once the daemon
/// answered; `None` when it has gone. Build it on every frame.
pub fn veil(
    pending: bool,
    p: &Palette,
    window: &mut Window,
    cx: &mut App,
) -> Option<impl IntoElement> {
    let shown = fade("main-skeleton-veil", pending, PANEL, window, cx);
    if !pending && shown <= 0.0 {
        return None;
    }
    let block =
        |id: &'static str, width: f32, height: f32| Skeleton::new(id).w(px(width)).h(px(height));
    Some(
        div()
            .id("main-skeleton")
            .absolute()
            .inset_0()
            .bg(p.bg)
            .opacity(shown)
            .px(space::XL)
            .pt(space::XL)
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(block("main-skeleton-title", 220., 24.))
            .child(block("main-skeleton-line", 420., 14.))
            .child(
                div()
                    .pt(space::MD)
                    .child(Skeleton::new("main-skeleton-block").h(px(96.))),
            )
            .test_support(),
    )
}

/// How long a conversation may take to be read before its skeleton shows: a fast open shows none.
pub const CONVERSATION_AFTER: Duration = Duration::from_millis(200);

/// Where the first read of a conversation stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Opening {
    Waiting,
    Read,
    /// It could not be read, with why.
    Unread(String),
}

/// What a conversation shows until its first page has arrived: past [`CONVERSATION_AFTER`] a
/// skeleton of a few messages, at the bottom where a conversation opens, which fades out over the
/// rows once they are read; or why it could not be read, with Try again. A view of its own, so
/// that its pulse and its fades never draw the conversation's list.
pub struct ConversationLoading {
    id: String,
    opening: Opening,
    /// The delay passed while it was being read.
    long: bool,
    delay: Option<Task<()>>,
    page: WeakEntity<SessionPage>,
}

impl ConversationLoading {
    pub fn new(id: &str, page: WeakEntity<SessionPage>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            id: id.to_owned(),
            opening: Opening::Waiting,
            long: false,
            delay: None,
            page,
        };
        this.wait(cx);
        this
    }

    fn wait(&mut self, cx: &mut Context<Self>) {
        self.long = false;
        self.delay = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CONVERSATION_AFTER).await;
            this.update(cx, |this, cx| {
                this.long = true;
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn opening(&self) -> &Opening {
        &self.opening
    }

    pub fn set(&mut self, opening: Opening, cx: &mut Context<Self>) {
        if self.opening == opening {
            return;
        }
        if opening == Opening::Waiting {
            self.wait(cx);
        }
        self.opening = opening;
        cx.notify();
    }
}

/// The lines of the skeleton's messages, as shares of the column: a reply, a message the user
/// sent (on the right, as its bubble), another reply.
const MESSAGES: [(bool, &[f32]); 3] = [
    (false, &[1.0, 0.92, 0.6]),
    (true, &[0.34]),
    (false, &[0.96, 0.74]),
];

impl Render for ConversationLoading {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let id = self.id.clone();
        let waiting = self.opening == Opening::Waiting && self.long;
        let shown = fade(
            ElementId::Name(format!("conversation-skeleton-fade-{id}").into()),
            waiting,
            PANEL,
            window,
            cx,
        );
        let unread = match &self.opening {
            Opening::Unread(why) => Some(SharedString::from(format!(
                "The conversation could not be read: {why}"
            ))),
            _ => None,
        };
        let narrow = window.viewport_size().width < px(crate::shell::NARROW);
        let column = move || {
            div()
                .max_w(px(780.))
                .w_full()
                .mx_auto()
                .px(if narrow { space::LG } else { space::XXL })
        };
        // Each line of a message is a bar of the conversation's text size, in a line of its
        // height: the messages that replace them take the same room.
        let line = |key: String, share: f32, right: bool| {
            div()
                .w_full()
                .h(px(BODY * 1.5))
                .flex()
                .items_center()
                .when(right, |d| d.justify_end())
                .child(
                    div()
                        .w(relative(share))
                        .child(Skeleton::new(ElementId::Name(key.into())).h(px(BODY))),
                )
        };
        let skeleton =
            (waiting || shown > 0.0).then(|| {
                div()
                    .id(ElementId::Name(
                        format!("conversation-skeleton-{id}").into(),
                    ))
                    .absolute()
                    .inset_0()
                    .bg(p.bg)
                    .opacity(shown)
                    .flex()
                    .flex_col()
                    .justify_end()
                    .aria_label("Reading the conversation…")
                    .child(
                        column()
                            .pb(px(20.))
                            .flex()
                            .flex_col()
                            .gap(px(18.))
                            .children(MESSAGES.iter().enumerate().map(|(n, (right, lines))| {
                                div().w_full().flex().flex_col().children(
                                    lines.iter().enumerate().map(|(at, share)| {
                                        line(
                                            format!("conversation-skeleton-{id}-{n}-{at}"),
                                            *share,
                                            *right,
                                        )
                                    }),
                                )
                            })),
                    )
                    .test_support()
            });
        let page = self.page.clone();
        let again_id = format!("conversation-try-again-{id}");
        let unread_id = format!("conversation-unread-{id}");
        div()
            .id(ElementId::Name(format!("conversation-loading-{id}").into()))
            .absolute()
            .inset_0()
            .children(skeleton)
            .child(presence_of(
                ElementId::Name(format!("conversation-unread-block-{id}").into()),
                unread,
                move |why| {
                    column()
                        .id(ElementId::Name(unread_id.into()))
                        .pt(if narrow { space::XL } else { space::XXL })
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(space::SM)
                        .aria_label(why.clone())
                        .child(div().text_color(p.mute).child(why))
                        .child(
                            Button::secondary(ElementId::Name(again_id.into()), "Try again")
                                .on_click(move |_, _, cx| {
                                    page.update(cx, |page, cx| page.read_again(cx)).ok();
                                }),
                        )
                        .test_support()
                        .into_any_element()
                },
            ))
            .into_any_element()
    }
}
