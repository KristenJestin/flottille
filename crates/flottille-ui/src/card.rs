//! `Card`: an optional header (title, mono meta on the right), a body and a footer row of
//! actions. Place cards, sub-agents, agent steps, reports, diffs and approvals are all cards.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, FontWeight, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Window, div, px,
};

use crate::theme::{palette, radius};
use crate::typography::Type;

#[derive(IntoElement, Default)]
pub struct Card {
    title: Option<AnyElement>,
    meta: Option<SharedString>,
    body: Vec<AnyElement>,
    footer: Vec<AnyElement>,
    attention: bool,
}

impl Card {
    pub fn new() -> Self {
        Self::default()
    }

    /// The header's title: text, or an element (an icon and a path).
    pub fn title(mut self, title: impl IntoElement) -> Self {
        self.title = Some(title.into_any_element());
        self
    }

    /// Mono, faint, on the right of the header.
    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.body.push(child.into_any_element());
        self
    }

    /// An action in the footer row, right-aligned.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.footer.push(action.into_any_element());
        self
    }

    /// Amber-bordered: a request that reached the user.
    pub fn attention(mut self) -> Self {
        self.attention = true;
        self
    }
}

impl RenderOnce for Card {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let has_header = self.title.is_some() || self.meta.is_some();
        div()
            .flex()
            .flex_col()
            .bg(p.card)
            .border_1()
            .border_color(if self.attention { p.wait_line } else { p.line })
            .rounded(radius::CARD)
            .text_color(p.text)
            .ui_text()
            .when(has_header, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(14.))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(p.line)
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .children(self.title)
                        .when_some(self.meta, |d, meta| {
                            d.child(
                                div()
                                    .ml_auto()
                                    .mono(11.)
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(p.faint)
                                    .child(meta),
                            )
                        }),
                )
            })
            .child(div().flex().flex_col().children(self.body))
            .when(!self.footer.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .justify_end()
                        .gap(px(6.))
                        .px(px(14.))
                        .pb(px(12.))
                        .pt(px(4.))
                        .children(self.footer),
                )
            })
    }
}
