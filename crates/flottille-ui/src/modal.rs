//! `Modal`: a dialog over the page for every form that adds or changes something: a title, the
//! fields, Cancel and the main action. Escape, Cancel and a click outside it close it.
//!
//! It is drawn after everything else, above the dropdowns, over a scrim that takes the clicks
//! meant for the page. The owner gives the focus to its first field when it opens it.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, BoxShadow, ElementId, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, RenderOnce, SharedString, Styled, TestSupportExt, Window, anchored,
    deferred, div, hsla, point, px,
};

use crate::button::Button;
use crate::theme::palette;
use crate::typography::Type;

type Handler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Modal {
    id: ElementId,
    title: SharedString,
    width: Pixels,
    body: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    on_close: Option<Handler>,
}

impl Modal {
    /// A dialog `id`, its scrim `<id>-scrim` and its Cancel button `<id>-cancel`.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            width: px(420.),
            body: Vec::new(),
            actions: Vec::new(),
            on_close: None,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// The main action, after Cancel: a primary button, or what stands for it while it runs.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// Escape, Cancel and a click outside call it.
    pub fn on_close(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }
}

impl ParentElement for Modal {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

fn part(id: &ElementId, name: &'static str) -> ElementId {
    match id {
        ElementId::Name(text) => ElementId::Name(format!("{text}-{name}").into()),
        other => ElementId::NamedChild(Arc::new(other.clone()), name.into()),
    }
}

impl RenderOnce for Modal {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let viewport = window.viewport_size();
        let close = self.on_close.clone();
        let close = move |window: &mut Window, cx: &mut App| {
            if let Some(close) = &close {
                close(window, cx);
            }
        };
        let (on_scrim, on_key, on_cancel) = (close.clone(), close.clone(), close);
        let scrim = div()
            .id(part(&self.id, "scrim"))
            .absolute()
            .inset_0()
            .bg(p.scrim)
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| on_scrim(window, cx))
            .test_support();
        let panel = div()
            .id(self.id.clone())
            .occlude()
            .w(self.width)
            .max_w(viewport.width - px(32.))
            .p(px(18.))
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line2)
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.6),
                offset: point(px(0.), px(16.)),
                blur_radius: px(48.),
                spread_radius: px(-8.),
                inset: false,
            }])
            .ui_text()
            .text_color(p.text)
            .flex()
            .flex_col()
            .gap(px(14.))
            .on_key_down(move |event, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    on_key(window, cx);
                }
            })
            .child(div().text_size(px(15.)).semibold().child(self.title))
            .children(self.body)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::quiet(part(&self.id, "cancel"), "Cancel")
                            .on_click(move |_, window, cx| on_cancel(window, cx)),
                    )
                    .children(self.actions),
            )
            .test_support();
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .w(viewport.width)
                    .h(viewport.height)
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(scrim)
                    .child(panel),
            ),
        )
        .priority(2)
    }
}
