//! `Confirm`: the one way the window asks before it removes, ends or forgets something. A modal
//! dialog whose title is the question with the object's name ("Remove the task test?"), one
//! sentence of what is lost and what stays, then Cancel and the danger button. Escape, Cancel
//! and the scrim cancel. When the action cannot be done, the dialog says why and its button does
//! nothing.
//!
//! There is no other confirmation: never a button that turns into other buttons in place (a scan
//! of the window's sources refuses the element ids that pattern used).

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::button::Button;
use crate::modal::Modal;
use crate::reveal::presence_of;
use crate::theme::palette;

type Handler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Confirm {
    id: ElementId,
    question: SharedString,
    sentence: Option<SharedString>,
    action: SharedString,
    refused: Option<SharedString>,
    open: bool,
    on_confirm: Option<Handler>,
    on_cancel: Option<Handler>,
}

impl Confirm {
    /// The dialog `id`, with its scrim `<id>-scrim`, its Cancel `<id>-cancel`, its danger button
    /// `<id>-action` and the reason it cannot be done `<id>-refused`. `question` names the object.
    pub fn new(id: impl Into<ElementId>, question: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            question: question.into(),
            sentence: None,
            action: "Remove".into(),
            refused: None,
            open: true,
            on_confirm: None,
            on_cancel: None,
        }
    }

    /// What is lost and what stays, in one sentence.
    pub fn sentence(mut self, sentence: impl Into<SharedString>) -> Self {
        self.sentence = Some(sentence.into());
        self
    }

    /// The danger button's label: a verb and its object ("Remove task", "End 3 agents").
    pub fn action(mut self, label: impl Into<SharedString>) -> Self {
        self.action = label.into();
        self
    }

    /// Why the action cannot be done now: said in the dialog, and its button does nothing.
    pub fn refused(mut self, why: Option<SharedString>) -> Self {
        self.refused = why;
        self
    }

    /// Whether it is open, `true` unless said. Draw it on every frame: it fades away when it
    /// closes.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// The danger button calls it.
    pub fn on_confirm(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_confirm = Some(Rc::new(f));
        self
    }

    /// Escape, Cancel and the scrim call it.
    pub fn on_cancel(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(f));
        self
    }
}

fn part(id: &ElementId, name: &'static str) -> ElementId {
    match id {
        ElementId::Name(text) => ElementId::Name(format!("{text}-{name}").into()),
        other => ElementId::NamedChild(Arc::new(other.clone()), name.into()),
    }
}

impl RenderOnce for Confirm {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let refused_id = part(&self.id, "refused");
        let refused = presence_of(refused_id.clone(), self.refused.clone(), move |why| {
            div()
                .id(refused_id)
                .aria_label(why.clone())
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(p.fail)
                .child(why)
                .test_support()
                .into_any_element()
        });
        let cancel = self.on_cancel.clone();
        let confirm = self.on_confirm.clone();
        let action = Button::danger(part(&self.id, "action"), self.action)
            .disabled(self.refused.is_some())
            .on_click(move |_, window, cx| {
                if let Some(confirm) = &confirm {
                    confirm(window, cx);
                }
            });
        let mut modal = Modal::new(self.id, self.question)
            .open(self.open)
            .takes_focus()
            .on_close(move |window, cx| {
                if let Some(cancel) = &cancel {
                    cancel(window, cx);
                }
            })
            .action(action);
        if let Some(sentence) = self.sentence {
            modal = modal.sentence(sentence);
        }
        // The reason is known when the dialog opens: it comes with the dialog's own entrance.
        if self.refused.is_some() {
            modal = modal.child(refused);
        }
        modal
    }
}
