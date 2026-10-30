//! `Card`: an optional header (title, mono meta on the right), a body, blocks and a footer row of
//! actions. Place cards, sub-agents, agent steps, reports, diffs and approvals are all cards.
//!
//! A [`Block`] is a part of a card that may have nothing to show (a workspace's links, its setup
//! steps): the line between two parts is drawn only between two that exist, never twice and
//! never under nothing.

use gpui_kit::prelude::FluentBuilder as _;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Styled, TestSupportExt, Window, div, px,
};

use crate::theme::{palette, radius};
use crate::typography::Type;

/// A part of a [`Card`], drawn with a line above it when something comes before it, and not at
/// all when it has no child.
#[derive(Default)]
pub struct Block {
    children: Vec<AnyElement>,
}

impl Block {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_content(&self) -> bool {
        !self.children.is_empty()
    }
}

impl ParentElement for Block {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

#[derive(IntoElement, Default)]
pub struct Card {
    id: Option<ElementId>,
    title: Option<AnyElement>,
    meta: Option<SharedString>,
    body: Vec<AnyElement>,
    blocks: Vec<Block>,
    footer: Vec<AnyElement>,
    attention: bool,
    alarm: bool,
}

impl Card {
    pub fn new() -> Self {
        Self::default()
    }

    /// Names the card: its lines between parts are `<id>-separator-<n>`, from 1.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// A part after the body, under a line when something comes before it; nothing at all when
    /// it is empty.
    pub fn block(mut self, block: Block) -> Self {
        self.blocks.push(block);
        self
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

    /// Red-bordered, on a red wash: a request that always comes back to the user, whatever the
    /// mode, because it may lose work or reach what must stay private.
    pub fn alarm(mut self) -> Self {
        self.alarm = true;
        self
    }
}

impl RenderOnce for Card {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let has_header = self.title.is_some() || self.meta.is_some();
        let card = self.id.clone();
        let mut drawn = 0;
        let mut separator = move || {
            let line = div().flex_none().h(px(1.)).bg(p.line);
            drawn += 1;
            let id = match &card {
                Some(ElementId::Name(name)) => {
                    ElementId::Name(format!("{name}-separator-{drawn}").into())
                }
                Some(other) => ElementId::NamedChild(
                    Arc::new(other.clone()),
                    format!("separator-{drawn}").into(),
                ),
                None => return line.into_any_element(),
            };
            line.id(id).test_support().into_any_element()
        };
        let mut parts: Vec<AnyElement> = Vec::new();
        if has_header {
            parts.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(10.))
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
                    })
                    .into_any_element(),
            );
        }
        if !self.body.is_empty() {
            if !parts.is_empty() {
                parts.push(separator());
            }
            parts.push(
                div()
                    .flex()
                    .flex_col()
                    .children(self.body)
                    .into_any_element(),
            );
        }
        for block in self.blocks.into_iter().filter(Block::has_content) {
            if !parts.is_empty() {
                parts.push(separator());
            }
            parts.push(
                div()
                    .flex()
                    .flex_col()
                    .children(block.children)
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .bg(if self.alarm { p.fail_bg } else { p.card })
            .border_1()
            .border_color(match (self.alarm, self.attention) {
                (true, _) => p.fail,
                (false, true) => p.wait_line,
                (false, false) => p.line,
            })
            .rounded(radius::CARD)
            .text_color(p.text)
            .ui_text()
            .children(parts)
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
