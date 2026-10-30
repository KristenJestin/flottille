//! `RadioChoices`: two to four kinds of a thing in a form, each with a line that says what it
//! means (a skill is "A file of the project" or "A text Flottille keeps"). Sized to their text.
//! The chosen one's ring takes the accent and its dot scales in over [`PANEL`](crate::motion::PANEL);
//! each choice fades its hover.

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

/// The most kinds it offers: more is a [`Select`](crate::Select)'s.
pub const MAX_RADIO_CHOICES: usize = 4;

type OnChoose = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

struct Choice {
    key: SharedString,
    title: SharedString,
    line: SharedString,
    leading: Option<Icon>,
}

#[derive(IntoElement)]
pub struct RadioChoices {
    id: ElementId,
    label: SharedString,
    choices: Vec<Choice>,
    chosen: usize,
    on_choose: Option<OnChoose>,
}

impl RadioChoices {
    /// `label` says what is chosen (its accessible name; the form's [`Field`](crate::Field)
    /// draws it). Each choice is `<id>-<key>`, its dot `<id>-<key>-dot`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            choices: Vec::new(),
            chosen: 0,
            on_choose: None,
        }
    }

    /// A kind: its title and the line that says what it means. More than
    /// [`MAX_RADIO_CHOICES`] is a defect: it panics.
    pub fn choice(
        mut self,
        key: impl Into<SharedString>,
        title: impl Into<SharedString>,
        line: impl Into<SharedString>,
    ) -> Self {
        assert!(
            self.choices.len() < MAX_RADIO_CHOICES,
            "radio choices offer at most four kinds; more is a Select"
        );
        self.choices.push(Choice {
            key: key.into(),
            title: title.into(),
            line: line.into(),
            leading: None,
        });
        self
    }

    /// An icon before the last choice's title.
    pub fn leading(mut self, glyph: Icon) -> Self {
        if let Some(choice) = self.choices.last_mut() {
            choice.leading = Some(glyph);
        }
        self
    }

    pub fn chosen(mut self, index: usize) -> Self {
        self.chosen = index;
        self
    }

    /// Called with the index of the choice clicked, when it is not the chosen one.
    pub fn on_choose(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_choose = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for RadioChoices {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let name = |key: &SharedString, part: Option<&str>| {
            let text = match part {
                Some(part) => format!("{key}-{part}"),
                None => key.to_string(),
            };
            match &self.id {
                ElementId::Name(id) => ElementId::Name(format!("{id}-{text}").into()),
                other => ElementId::NamedChild(Arc::new(other.clone()), text.into()),
            }
        };
        let choices = self.choices.iter().enumerate().map(|(index, choice)| {
            let chosen = index == self.chosen;
            let dot_id = name(&choice.key, Some("dot"));
            let mut row = div()
                .id(name(&choice.key, None))
                .aria_label(choice.title.clone())
                .aria_selected(chosen)
                .flex()
                .items_start()
                .gap(px(10.))
                .pt(px(6.))
                .pb(px(6.))
                .pl(px(6.))
                .pr(px(10.))
                .ml(px(-6.))
                .rounded(px(8.));
            if !chosen && let Some(on_choose) = self.on_choose.clone() {
                row = row
                    .cursor_pointer()
                    .on_click(move |_, window, cx| on_choose(index, window, cx));
            }
            let title = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(13.))
                .line_height(px(20.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.text)
                .children(
                    choice
                        .leading
                        .map(|glyph| icon(glyph).size(px(12.)).text_color(p.faint)),
                )
                .child(choice.title.clone());
            let line = div()
                .id(name(&choice.key, Some("line")))
                .aria_label(choice.line.clone())
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(p.faint)
                .child(choice.line.clone())
                .test_support();
            let words = div().flex().flex_col().child(title).child(line);
            selectable(row, chosen, move |d, hover, on| {
                let ring = div()
                    .mt(px(2.))
                    .size(px(16.))
                    .flex_none()
                    .rounded_full()
                    .border_1()
                    .border_color(mix(p.line2, p.acc, on))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id(dot_id)
                            .size(px(8. * on))
                            .rounded_full()
                            .bg(p.acc)
                            .test_support(),
                    );
                d.bg(p.hover.opacity(hover)).child(ring).child(words)
            })
        });
        div()
            .id(self.id.clone())
            .aria_label(self.label)
            .flex()
            .flex_col()
            .items_start()
            .gap(px(2.))
            .children(choices)
            .test_support()
    }
}
