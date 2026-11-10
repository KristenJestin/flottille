//! `ChoiceRow`: one option of a list the user picks from, as a row: a mark (round for one choice
//! among several, square for any number of them), the label, and under it a description on as
//! many lines as it needs. The whole row is the target. Its hover fades over
//! [`HOVER`](crate::motion::HOVER); when it is chosen its fill and border fade in and its mark
//! fills, its dot scaling in or its tick growing, over [`PANEL`](crate::motion::PANEL).

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::color::mix;
use crate::icons::{Icon, icon};
use crate::interact::selectable;
use crate::theme::palette;

type OnClick = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Round,
    Square,
}

#[derive(IntoElement)]
pub struct ChoiceRow {
    id: ElementId,
    label: SharedString,
    description: Option<SharedString>,
    shape: Shape,
    chosen: bool,
    faint: bool,
    on_click: Option<OnClick>,
}

impl ChoiceRow {
    /// A row with a round mark. Its parts are `<id>-label`, `<id>-line` (the description),
    /// `<id>-mark` and `<id>-dot` (round) or `<id>-tick` (square).
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            shape: Shape::Round,
            chosen: false,
            faint: false,
            on_click: None,
        }
    }

    /// What the choice means, under its label.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// A square mark with a tick: the row is one of several that may all be chosen.
    pub fn square(mut self) -> Self {
        self.shape = Shape::Square;
        self
    }

    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }

    /// A faint label in the regular weight: the way out of the choices ("Something else…").
    pub fn faint(mut self) -> Self {
        self.faint = true;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for ChoiceRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let part = |part: &str| match &self.id {
            ElementId::Name(id) => ElementId::Name(format!("{id}-{part}").into()),
            other => ElementId::NamedChild(Arc::new(other.clone()), part.to_owned().into()),
        };
        let (mark_id, glyph_id) = (
            part("mark"),
            part(match self.shape {
                Shape::Round => "dot",
                Shape::Square => "tick",
            }),
        );
        let mut row = div()
            .id(self.id.clone())
            .aria_label(self.label.clone())
            .aria_selected(self.chosen)
            .w_full()
            .flex()
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .border_1()
            .cursor_pointer();
        if let Some(on_click) = self.on_click.clone() {
            row = row.on_click(move |_, window, cx| on_click(window, cx));
        }
        let label = div()
            .id(part("label"))
            .text_size(px(14.))
            .line_height(px(20.))
            .when_faint(self.faint, p.mute, p.text)
            .child(self.label.clone())
            .test_support();
        let line = self.description.clone().map(|description| {
            div()
                .id(part("line"))
                .aria_label(description.clone())
                .text_size(px(13.))
                .line_height(px(18.))
                .text_color(p.mute)
                .child(description)
                .test_support()
        });
        let words = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(label)
            .children(line);
        let shape = self.shape;
        selectable(row, self.chosen, move |d, hover, on| {
            let glyph = div().id(glyph_id);
            let glyph = match shape {
                Shape::Round => glyph
                    .size(px(6. * on))
                    .rounded_full()
                    .bg(p.on_acc)
                    .test_support(),
                Shape::Square => glyph
                    .size(px(10. * on))
                    .child(icon(Icon::Check).size(px(10. * on)).text_color(p.on_acc))
                    .test_support(),
            };
            let mark = div()
                .id(mark_id)
                .mt(px(2.))
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .border(px(1.5))
                .border_color(mix(p.line2, p.acc, on))
                .bg(mix(p.field, p.acc, on))
                .when_round(shape == Shape::Round)
                .child(glyph)
                .test_support();
            d.bg(mix(p.hover.opacity(hover), p.bg, on))
                .border_color(p.line2.opacity(on))
                .child(mark)
                .child(words)
        })
    }
}

/// The two small conveniences the row needs on a styled element.
trait RowStyle: Styled + Sized {
    fn when_round(self, round: bool) -> Self {
        if round {
            self.rounded_full()
        } else {
            self.rounded(px(4.))
        }
    }

    fn when_faint(self, faint: bool, faint_ink: gpui_kit::Hsla, ink: gpui_kit::Hsla) -> Self {
        if faint {
            self.text_color(faint_ink)
        } else {
            self.text_color(ink).font_weight(FontWeight::MEDIUM)
        }
    }
}

impl<T: Styled + Sized> RowStyle for T {}
