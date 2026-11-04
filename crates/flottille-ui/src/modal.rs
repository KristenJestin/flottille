//! `Modal`: a dialog over the page for every form that adds or changes something: a title, one
//! sentence under it, the fields, then Cancel and the main action on the right. Escape, Cancel
//! and a click outside it close it. It is 520 px wide, 640 for a wide form, with 24 px on each
//! side. In a window too small for it, the fields scroll between the title and the buttons,
//! which stay in sight.
//!
//! It is drawn after everything else, above the page's dropdowns, over a scrim that takes the
//! clicks meant for the page. The list of a select or a menu opened inside it is drawn above it
//! ([`layer`](crate::layer)). The owner gives the focus to its first field when it opens it.
//!
//! The owner draws it all the time and says whether it is [`open`](Modal::open): the scrim fades
//! and the dialog fades and rises when it opens, and the other way when it closes. Once the close
//! animation has ended nothing is drawn.
//!
//! A [`large`](Modal::large) one is for looking at something at length (a command's output): nearly
//! the whole window, a margin around it; the title and its sentence on the left of the header, the
//! actions and a close button on its right, no Cancel and no footer, and the body takes all the
//! height left, for its owner to scroll.
//!
//! A [`search`](Modal::search) is the same dialog for finding something (the window's quick
//! switcher): near the top of the window, where the eye already is, its height following its
//! results; its field above the results, which scroll; no title drawn and no Cancel.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, RenderOnce, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt, Window, anchored, deferred, div, point, px,
};

use crate::button::Button;
use crate::icons::Icon;
use crate::interact::{dismiss_on_navigate, open_holding_overlay};
use crate::layer;
use crate::motion::{MODAL, Motion, RISE};
use crate::space;
use crate::theme::palette;
use crate::typography::Type;

/// What a modal remembers between two frames: whether it was open, and its entrance.
struct Shown {
    open: bool,
    motion: Motion,
    focus: Option<FocusHandle>,
}

type Handler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type KeyHandler = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Modal {
    id: ElementId,
    title: SharedString,
    sentence: Option<SharedString>,
    wide: bool,
    body: Vec<AnyElement>,
    kept: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    on_close: Option<Handler>,
    open: bool,
    cancel: SharedString,
    takes_focus: bool,
    search: bool,
    large: bool,
    above: Vec<AnyElement>,
    scroll: Option<ScrollHandle>,
    on_key: Option<KeyHandler>,
}

impl Modal {
    /// A dialog `id`, its scrim `<id>-scrim` and its Cancel button `<id>-cancel`.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            sentence: None,
            wide: false,
            body: Vec::new(),
            kept: Vec::new(),
            actions: Vec::new(),
            on_close: None,
            open: true,
            cancel: "Cancel".into(),
            takes_focus: false,
            search: false,
            large: false,
            above: Vec::new(),
            scroll: None,
            on_key: None,
        }
    }

    /// A search over the page rather than a form: near the top of the window, 640 px wide, its
    /// title said as its label and not drawn, no Cancel (Escape and a click outside close it),
    /// and its body (the results) drawn close together.
    pub fn search(mut self) -> Self {
        self.search = true;
        self
    }

    /// Nearly the whole window, to look at something at length: its actions are in the header
    /// beside a close button, there is no Cancel, and the body takes the height left.
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }

    /// What stays in sight above the body, whatever it has scrolled to: a search's field.
    pub fn above(mut self, element: impl IntoElement) -> Self {
        self.above.push(element.into_any_element());
        self
    }

    /// The body's scroll, which the owner moves: the result the arrows mark stays in sight.
    pub fn scroll(mut self, scroll: &ScrollHandle) -> Self {
        self.scroll = Some(scroll.clone());
        self
    }

    /// A key pressed in it, other than Escape: a search's arrows.
    pub fn on_key(mut self, f: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_key = Some(Rc::new(f));
        self
    }

    /// One sentence under the title: what the thing is, or what the action does.
    pub fn sentence(mut self, sentence: impl Into<SharedString>) -> Self {
        self.sentence = Some(sentence.into());
        self
    }

    /// 640 px instead of 520: a form with wide fields.
    pub fn wide(mut self) -> Self {
        self.wide = true;
        self
    }

    /// Takes the focus when it opens, so that Escape reaches it: a dialog with no field.
    pub fn takes_focus(mut self) -> Self {
        self.takes_focus = true;
        self
    }

    /// What the button that closes it says, `Cancel` unless said: a dialog that changes nothing
    /// says `Close`.
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel = label.into();
        self
    }

    /// Whether it is open, `true` unless said.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// What stays in sight under the fields, above the buttons, whatever the body has scrolled
    /// to: a refusal. It takes no room while it is empty, and folds as the owner animates it.
    pub fn kept(mut self, element: impl IntoElement) -> Self {
        self.kept.push(element.into_any_element());
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
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(part(&self.id, "motion"), cx, |_, _| Shown {
            open: false,
            motion: Motion::settled(0.0),
            focus: None,
        });
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let takes_focus = self.takes_focus;
        let (opening, focus) = state.update(cx, |shown, cx| {
            let opening = !shown.open && self.open;
            if shown.open != self.open {
                shown.open = self.open;
                let target = if self.open { 1.0 } else { 0.0 };
                shown.motion.animate(target, MODAL, now, reduce);
            }
            if takes_focus && shown.focus.is_none() {
                shown.focus = Some(cx.focus_handle());
            }
            (opening, shown.focus.clone())
        });
        if opening && let Some(focus) = focus.clone() {
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        let motion = state.read(cx).motion;
        if motion.is_running(now) {
            window.request_animation_frame();
        } else {
            state.update(cx, |shown, _| shown.motion.settle(now));
        }
        let t = motion.value(now);
        // Out of the page's flow, open or not: a modal takes no room and no gap where it is drawn.
        if !self.open && t == 0.0 {
            return div().absolute().into_any_element();
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
            open_holding_overlay(self.id.clone(), close.clone(), cx);
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
        let search = self.search;
        let large = self.large;
        let title = self.title.clone();
        let (header_actions, footer_actions) = if large {
            (std::mem::take(&mut self.actions), Vec::new())
        } else {
            (Vec::new(), std::mem::take(&mut self.actions))
        };
        let on_close_button = close.clone();
        let width = if large {
            viewport.width - space::XL * 2.
        } else if self.wide || search {
            space::DIALOG_WIDE
        } else {
            space::DIALOG
        };
        let titles = (!search).then(|| {
            div()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .id(part(&self.id, "title"))
                        .aria_label(self.title.clone())
                        .text_size(px(15.))
                        .line_height(px(22.))
                        .semibold()
                        .child(self.title)
                        .test_support(),
                )
                .children(self.sentence.map(|sentence| {
                    div()
                        .id(part(&self.id, "sentence"))
                        .aria_label(sentence.clone())
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(p.mute)
                        .child(sentence)
                        .test_support()
                }))
        });
        let header = titles.map(|titles| {
            let header = div().flex_none().pt(px(20.)).px(px(24.)).flex();
            if large {
                header
                    .items_start()
                    .gap(space::MD)
                    .child(titles.flex_1().min_w_0().flex())
                    .children(header_actions)
                    .child(
                        Button::icon(part(&self.id, "close"), Icon::Close, "Close (Esc)")
                            .on_click(move |_, window, cx| on_close_button(window, cx)),
                    )
            } else {
                header.flex_col().child(titles.flex())
            }
        });
        let above = (!self.above.is_empty()).then(|| {
            div()
                .flex_none()
                .p(space::MD)
                .border_b_1()
                .border_color(p.line)
                .children(self.above)
        });
        // It scrolls inside a column of its own, so that the rows keep their heights while it
        // is shorter than they are; the title above and the buttons below stay where they are.
        let scroll = self.scroll;
        let body = (!self.body.is_empty()).then(|| {
            let scrolled = div()
                .id(part(&self.id, "body"))
                .min_h_0()
                .overflow_y_scroll()
                .when_some(scroll, |d, scroll| d.track_scroll(&scroll));
            if search {
                // The results are the scrolled element's own children, so that the owner scrolls
                // to one by its place; each keeps its height while the list is shorter.
                scrolled.p(space::SM).flex().flex_col().children(
                    self.body
                        .into_iter()
                        .map(|child| div().flex_none().child(child)),
                )
            } else if large {
                // The owner's body takes the height and scrolls what it holds itself.
                div()
                    .id(part(&self.id, "body"))
                    .flex_1()
                    .min_h_0()
                    .px(px(24.))
                    .pt(px(16.))
                    .pb(px(24.))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .children(self.body)
            } else {
                scrolled.child(
                    div()
                        .py(px(20.))
                        .px(px(24.))
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .children(self.body),
                )
            }
        });
        let kept = div()
            .flex_none()
            .when(!search, |d| d.px(px(24.)))
            .flex()
            .flex_col()
            .children(self.kept);
        let footer = (!search && !large).then(|| {
            div()
                .flex_none()
                .pt(px(14.))
                .pb(px(20.))
                .px(px(24.))
                .flex()
                .justify_end()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::quiet(part(&self.id, "cancel"), self.cancel)
                        .on_click(move |_, window, cx| on_cancel(window, cx)),
                )
                .children(footer_actions)
        });
        let on_other_key = self.on_key;
        let mut panel = div()
            .id(self.id.clone())
            .occlude()
            .relative()
            .top(px(RISE * (1.0 - t)))
            .opacity(t)
            .w(width)
            .max_w(viewport.width - px(48.))
            .when(large, |d| d.h(viewport.height - space::XL * 2.))
            .max_h(if search {
                viewport.height - space::XXXL * 2.
            } else {
                viewport.height - px(48.)
            })
            .when(search, |d| d.aria_label(title))
            .rounded(px(14.))
            .bg(p.card)
            .border_1()
            .border_color(p.line2)
            .shadow(vec![BoxShadow {
                color: p.shadow.opacity(0.6),
                offset: point(px(0.), px(24.)),
                blur_radius: px(60.),
                spread_radius: px(0.),
                inset: false,
            }])
            .ui_text()
            .text_color(p.text)
            .flex()
            .flex_col()
            .on_key_down(move |event, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    on_key(window, cx);
                } else if let Some(on_other_key) = &on_other_key {
                    on_other_key(event, window, cx);
                }
            })
            .children(header)
            .children(above)
            .children(body)
            .child(kept)
            .children(footer);
        if let Some(focus) = &focus {
            panel = panel.track_focus(focus);
        }
        let panel = panel.test_support();
        let panel = dismiss_on_navigate(panel, close);
        let layer = deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .w(viewport.width)
                    .h(viewport.height)
                    .relative()
                    .flex()
                    .justify_center()
                    .map(|d| {
                        if search {
                            d.items_start().pt(space::XXXL)
                        } else {
                            d.items_center()
                        }
                    })
                    .child(scrim)
                    .child(panel),
            ),
        )
        .priority(layer::MODAL);
        div().absolute().child(layer).into_any_element()
    }
}
