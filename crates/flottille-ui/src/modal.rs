//! `Modal`: a dialog over the page for every form that adds or changes something: a title, the
//! fields, Cancel and the main action. Escape, Cancel and a click outside it close it.
//!
//! It is drawn after everything else, above the dropdowns, over a scrim that takes the clicks
//! meant for the page. The owner gives the focus to its first field when it opens it.
//!
//! The owner draws it all the time and says whether it is [`open`](Modal::open): the scrim fades
//! and the dialog fades and rises when it opens, and the other way when it closes. Once the close
//! animation has ended nothing is drawn.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, BoxShadow, ElementId, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, RenderOnce, SharedString, Styled, TestSupportExt, Window, anchored,
    deferred, div, hsla, point, px,
};

use crate::button::Button;
use crate::interact::{dismiss_on_navigate, open_overlay};
use crate::motion::{MODAL, Motion, RISE};
use crate::theme::palette;
use crate::typography::Type;

/// What a modal remembers between two frames: whether it was open, and its entrance.
struct Shown {
    open: bool,
    motion: Motion,
}

type Handler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Modal {
    id: ElementId,
    title: SharedString,
    width: Pixels,
    body: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    on_close: Option<Handler>,
    open: bool,
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
            open: true,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Whether it is open, `true` unless said.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
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
        let state = window.use_keyed_state(part(&self.id, "motion"), cx, |_, _| Shown {
            open: false,
            motion: Motion::settled(0.0),
        });
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        state.update(cx, |shown, _| {
            if shown.open != self.open {
                shown.open = self.open;
                let target = if self.open { 1.0 } else { 0.0 };
                shown.motion.animate(target, MODAL, now, reduce);
            }
        });
        let motion = state.read(cx).motion;
        if motion.is_running(now) {
            window.request_animation_frame();
        } else {
            state.update(cx, |shown, _| shown.motion.settle(now));
        }
        let t = motion.value(now);
        if !self.open && t == 0.0 {
            return div().into_any_element();
        }
        let p = palette(cx);
        let viewport = window.viewport_size();
        let close = self.on_close.clone();
        let close = move |window: &mut Window, cx: &mut App| {
            if let Some(close) = &close {
                close(window, cx);
            }
        };
        if self.open {
            open_overlay(self.id.clone(), close.clone(), cx);
        }
        let (on_scrim, on_key, on_cancel) = (close.clone(), close.clone(), close.clone());
        let scrim = div()
            .id(part(&self.id, "scrim"))
            .absolute()
            .inset_0()
            .bg(p.scrim.opacity(t))
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| on_scrim(window, cx))
            .test_support();
        let scrim = dismiss_on_navigate(scrim, close.clone());
        let panel = div()
            .id(self.id.clone())
            .occlude()
            .relative()
            .top(px(RISE * (1.0 - t)))
            .opacity(t)
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
        let panel = dismiss_on_navigate(panel, close);
        let layer = deferred(
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
        .priority(2);
        layer.into_any_element()
    }
}
