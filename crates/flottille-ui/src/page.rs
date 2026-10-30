//! What a page is made of: its header ([`PageHeader`]), its sections ([`Section`]) and the line
//! an empty list is ([`EmptyLine`]).
//!
//! A page's column is [`space::COLUMN`](crate::space::COLUMN) wide at most, with
//! [`space::XXL`](crate::space::XXL) around it and between its sections.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::button::Button;
use crate::icons::{Icon, icon};
use crate::menu::Menu;
use crate::theme::palette;
use crate::typography::Type;

/// The most text buttons a page header holds; the rest goes in its "⋯".
pub const MAX_HEADER_ACTIONS: usize = 2;

type OnClick = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

struct Action {
    id: ElementId,
    label: SharedString,
    leading: Option<Icon>,
    on_click: OnClick,
}

fn part(id: &ElementId, name: &'static str) -> ElementId {
    match id {
        ElementId::Name(text) => ElementId::Name(format!("{text}-{name}").into()),
        other => ElementId::NamedChild(Arc::new(other.clone()), name.into()),
    }
}

/// A page's header: a breadcrumb back, the title with its path beside it, and on the title's line
/// at most two secondary buttons and one "⋯" menu for the rest (a destructive action is a
/// "Remove…" of that menu, never a button). Chips or a line can go under the title.
#[derive(IntoElement)]
pub struct PageHeader {
    id: ElementId,
    title: SharedString,
    path: Option<SharedString>,
    crumb: Option<AnyElement>,
    below: Vec<AnyElement>,
    actions: Vec<Action>,
    more: Option<Entity<Menu>>,
}

impl PageHeader {
    /// The header `id`, its title `<id>-title`, its path `<id>-path`.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            path: None,
            crumb: None,
            below: Vec::new(),
            actions: Vec::new(),
            more: None,
        }
    }

    /// Mono, faint, beside the title: the folder the page is about.
    pub fn path(mut self, path: impl Into<SharedString>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Above the title: the way back to the page this one belongs to (`harbor /`).
    pub fn crumb(mut self, crumb: impl IntoElement) -> Self {
        self.crumb = Some(crumb.into_any_element());
        self
    }

    /// Under the title: chips, a line.
    pub fn below(mut self, element: impl IntoElement) -> Self {
        self.below.push(element.into_any_element());
        self
    }

    /// A secondary button on the title's line. A third is a defect: it panics; it belongs in
    /// [`PageHeader::more`].
    pub fn action(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        leading: Option<Icon>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        assert!(
            self.actions.len() < MAX_HEADER_ACTIONS,
            "a page header holds at most two actions; the others go in its \"⋯\" menu"
        );
        self.actions.push(Action {
            id: id.into(),
            label: label.into(),
            leading,
            on_click: Rc::new(on_click),
        });
        self
    }

    /// The "⋯" at the end of the title's line: every other action, "Remove…" last.
    pub fn more(mut self, menu: Entity<Menu>) -> Self {
        self.more = Some(menu);
        self
    }
}

impl RenderOnce for PageHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let actions = self.actions.into_iter().map(|action| {
            let on_click = action.on_click.clone();
            let button = Button::secondary(action.id, action.label)
                .on_click(move |_, window, cx| on_click(window, cx));
            match action.leading {
                Some(glyph) => button.leading(glyph),
                None => button,
            }
        });
        let title = div()
            .id(part(&self.id, "title"))
            .aria_label(self.title.clone())
            .text_size(px(22.))
            .line_height(px(30.))
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .child(self.title)
            .test_support();
        let path = self.path.map(|path| {
            div()
                .id(part(&self.id, "path"))
                .aria_label(path.clone())
                .mono(12.)
                .text_color(p.faint)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .min_w_0()
                .child(path)
                .test_support()
        });
        let line = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(title)
                    .children(path),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(actions)
                    .children(self.more),
            );
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .text_color(p.text)
            .ui_text()
            .when_some(self.crumb, |d, crumb| {
                d.child(
                    div()
                        .mb(px(2.))
                        .text_size(px(13.))
                        .text_color(p.faint)
                        .child(crumb),
                )
            })
            .child(line)
            .when(!self.below.is_empty(), |d| {
                d.child(
                    div()
                        .mt(px(10.))
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .children(self.below),
                )
            })
            .test_support()
    }
}

/// A section of a page: a 30 px title row (its title, a count when it is a list, its buttons on
/// the right), one sentence that says what it is for, then its content 12 px under.
#[derive(IntoElement)]
pub struct Section {
    id: ElementId,
    title: SharedString,
    count: Option<SharedString>,
    sentence: Option<SharedString>,
    actions: Vec<AnyElement>,
    content: Vec<AnyElement>,
}

impl Section {
    /// The section `id`, its title `<id>-title`, its sentence `<id>-sentence`.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            count: None,
            sentence: None,
            actions: Vec::new(),
            content: Vec::new(),
        }
    }

    /// How many it lists (or "1 running"), mono and faint after the title.
    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// What the section is for, in one sentence.
    pub fn sentence(mut self, sentence: impl Into<SharedString>) -> Self {
        self.sentence = Some(sentence.into());
        self
    }

    /// A button on the right of the title row: a secondary to add, a quiet for a lesser action.
    pub fn action(mut self, button: Button) -> Self {
        self.actions.push(button.into_any_element());
        self
    }
}

impl ParentElement for Section {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements);
    }
}

impl RenderOnce for Section {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let head = div()
            .min_h(px(30.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .id(part(&self.id, "title"))
                    .aria_label(self.title.clone())
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title)
                    .test_support(),
            )
            .when_some(self.count, |d, count| {
                d.child(div().mono(11.).text_color(p.faint).child(count))
            })
            .child(div().flex_1())
            .children(self.actions);
        let sentence = self.sentence.map(|sentence| {
            div()
                .id(part(&self.id, "sentence"))
                .aria_label(sentence.clone())
                .mt(px(-6.))
                .max_w(px(640.))
                .text_size(px(13.))
                .text_color(p.mute)
                .child(sentence)
                .test_support()
        });
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_color(p.text)
            .ui_text()
            .child(head)
            .children(sentence)
            .children(self.content)
    }
}

/// What an empty list is: one dashed line saying what to add and why, with the button that adds
/// it when the section's header does not have it.
#[derive(IntoElement)]
pub struct EmptyLine {
    id: ElementId,
    glyph: Icon,
    text: SharedString,
    action: Option<AnyElement>,
}

impl EmptyLine {
    pub fn new(id: impl Into<ElementId>, glyph: Icon, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            glyph,
            text: text.into(),
            action: None,
        }
    }

    pub fn action(mut self, button: Button) -> Self {
        self.action = Some(button.into_any_element());
        self
    }
}

impl RenderOnce for EmptyLine {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .id(self.id)
            .aria_label(self.text.clone())
            .flex()
            .items_center()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(12.))
            .border_1()
            .border_dashed()
            .border_color(p.line2)
            .ui_text()
            .text_size(px(13.))
            .text_color(p.mute)
            .child(icon(self.glyph).text_color(p.faint))
            .child(div().flex_1().min_w_0().child(self.text))
            .children(self.action)
            .test_support()
    }
}
