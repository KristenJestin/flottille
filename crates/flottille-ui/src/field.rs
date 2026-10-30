//! `Field`: a control with its label above it, a hint under it and its own error under that. A
//! text field always has one: the placeholder is only an example of a value, never its label.
//!
//! The error is the form's own validation (a field left empty, a name taken): it unfolds under
//! its field and folds away when it goes. The result of an action is a toast, not an error here.

use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::reveal::presence_of;
use crate::theme::palette;

#[derive(IntoElement)]
pub struct Field {
    id: ElementId,
    label: SharedString,
    optional: bool,
    control: AnyElement,
    hint: Option<SharedString>,
    hint_mono: bool,
    error: Option<SharedString>,
}

impl Field {
    /// `control` labelled `label`: the field `id`, its label `<id>-label`, its hint `<id>-hint`
    /// and its error `<id>-error`.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        control: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            optional: false,
            control: control.into_any_element(),
            hint: None,
            hint_mono: false,
            error: None,
        }
    }

    /// "optional", faint, after the label.
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// One line under the control: what the value is used for.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The hint in the mono face: what the value becomes (`branch harbor/login-fix`).
    pub fn hint_mono(mut self) -> Self {
        self.hint_mono = true;
        self
    }

    /// The form's own complaint about this value, under it until it changes.
    pub fn error(mut self, error: Option<SharedString>) -> Self {
        self.error = error;
        self
    }
}

fn part(id: &ElementId, name: &'static str) -> ElementId {
    match id {
        ElementId::Name(text) => ElementId::Name(format!("{text}-{name}").into()),
        other => ElementId::NamedChild(Arc::new(other.clone()), name.into()),
    }
}

impl RenderOnce for Field {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        use crate::typography::Type as _;
        let p = palette(cx);
        let label = div()
            .id(part(&self.id, "label"))
            .aria_label(self.label.clone())
            .flex()
            .items_baseline()
            .gap(px(4.))
            .text_size(px(12.5))
            .line_height(px(18.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.text)
            .child(self.label)
            .when(self.optional, |d| {
                d.child(
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(p.faint)
                        .child("optional"),
                )
            })
            .test_support();
        let hint = self.hint.map(|hint| {
            let line = div()
                .id(part(&self.id, "hint"))
                .aria_label(hint.clone())
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(p.faint);
            let line = if self.hint_mono { line.mono(12.) } else { line };
            line.child(hint).test_support()
        });
        let error_id = part(&self.id, "error");
        let error = presence_of(part(&self.id, "error-fold"), self.error, move |error| {
            div()
                .id(error_id)
                .aria_label(error.clone())
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(p.fail)
                .child(error)
                .test_support()
                .into_any_element()
        })
        .gap(px(6.));
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(label)
            .child(self.control)
            .children(hint)
            .child(error)
    }
}
