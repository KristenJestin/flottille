//! `Select`: one option among a list that may grow, more than three, or with long labels (a
//! tracker, a forge, a setup step's kind or place). A 34 px field showing the chosen option; a
//! click opens its list, which fades in while rising, each option with its title and one line,
//! the chosen one with a check that fades along it. Picking one closes the list; so do Escape and
//! a click outside it.
//!
//! The fields a chosen option needs are the form's: drawn under the select with
//! [`presence`](crate::presence), they unfold when the option is chosen and fold away when another
//! is.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Bounds, ElementId, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    TestSupportExt, Window, canvas, div, px,
};

use crate::color::mix;
use crate::dropdown::{Align, Handler, list_overlay};
use crate::icons::{Icon, icon};
use crate::interact::{hoverable, selectable};
use crate::motion::{ENTER, HOVER, Motion};
use crate::theme::palette;

/// The height of a select's field: the list opens below it.
const HEIGHT: f32 = 34.;

type OnChoose = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

/// One option: what it is called and, when its name does not say it, one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    key: SharedString,
    title: SharedString,
    line: Option<SharedString>,
}

impl SelectOption {
    /// The option `<select id>-<key>`.
    pub fn new(key: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            line: None,
        }
    }

    pub fn line(mut self, line: impl Into<SharedString>) -> Self {
        self.line = Some(line.into());
        self
    }
}

/// What a select remembers between frames: whether its list is open, its fade, its width.
struct Open {
    open: bool,
    fade: Motion,
    width: Rc<Cell<Pixels>>,
    focus: FocusHandle,
}

#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    options: Vec<SelectOption>,
    chosen: usize,
    fit: bool,
    disabled: bool,
    on_choose: Option<OnChoose>,
}

impl Select {
    /// The field `id`, its list `<id>-menu`, each option `<id>-<key>`; `chosen` is an index of
    /// `options`.
    pub fn new(
        id: impl Into<ElementId>,
        options: impl IntoIterator<Item = SelectOption>,
        chosen: usize,
    ) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().collect(),
            chosen,
            fit: false,
            disabled: false,
            on_choose: None,
        }
    }

    /// As wide as its words (at least 220 px) instead of its column's width.
    pub fn fit(mut self) -> Self {
        self.fit = true;
        self
    }

    /// Faded, and a click does nothing.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the index of the option picked, when it is not the chosen one.
    pub fn on_choose(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_choose = Some(Rc::new(f));
        self
    }
}

fn part(id: &ElementId, name: &str) -> ElementId {
    match id {
        ElementId::Name(text) => ElementId::Name(format!("{text}-{name}").into()),
        other => ElementId::NamedChild(Arc::new(other.clone()), name.to_owned().into()),
    }
}

impl RenderOnce for Select {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let state = window.use_keyed_state(part(&self.id, "open"), cx, |_, cx| Open {
            open: false,
            fade: Motion::settled(0.0),
            width: Rc::new(Cell::new(px(220.))),
            focus: cx.focus_handle(),
        });
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let (open, motion, width, focus) = state.update(cx, |s, _| {
            let (target, duration) = if s.open { (1.0, ENTER) } else { (0.0, HOVER) };
            if s.fade.target() != target {
                s.fade.animate(target, duration, now, reduce);
            }
            s.fade.settle(now);
            (s.open, s.fade, s.width.clone(), s.focus.clone())
        });
        if motion.is_running(now) {
            window.request_animation_frame();
        }
        let t = motion.value(now);
        let set_open = {
            let state = state.clone();
            move |open: bool, window: &mut Window, cx: &mut App| {
                state.update(cx, |s, cx| {
                    s.open = open;
                    cx.notify();
                    if open {
                        window.focus(&s.focus, cx);
                    }
                });
            }
        };
        let chosen = self.options.get(self.chosen).cloned();
        let measured = width.clone();
        let measure = canvas(
            move |bounds: Bounds<Pixels>, _, _| measured.set(bounds.size.width),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let mut field = div()
            .id(self.id.clone())
            .aria_label(chosen.as_ref().map(|o| o.title.clone()).unwrap_or_default())
            .aria_expanded(open)
            .relative()
            .h(px(HEIGHT))
            .px(px(10.))
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(p.text)
            .child(measure)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .children(chosen.as_ref().map(|o| o.title.clone())),
            )
            .child(icon(Icon::Choose).size(px(12.)).text_color(p.faint));
        field = if self.fit {
            field.min_w(px(220.))
        } else {
            field.w_full()
        };
        if self.disabled {
            field = field.opacity(0.35);
        } else {
            let toggle = set_open.clone();
            field = field
                .cursor_pointer()
                .on_click(move |_, window, cx| toggle(!open, window, cx));
        }
        let field = hoverable(field, move |d, hover| {
            let on = if open { 1.0 } else { hover };
            d.border_color(mix(p.field_line, p.focus_line, on))
        });
        let close = set_open.clone();
        let root = div()
            .id(part(&self.id, "root"))
            .track_focus(&focus)
            .on_key_down(move |event, window, cx| {
                if open && event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    close(false, window, cx);
                }
            })
            .relative()
            .flex()
            .flex_col()
            .map(|d| if self.fit { d.self_start() } else { d.w_full() })
            .child(field);
        if t == 0.0 && !open {
            return root;
        }
        let on_choose = self.on_choose.clone();
        let items = self
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let is_chosen = index == self.chosen;
                let (pick, on_choose) = (set_open.clone(), on_choose.clone());
                let mut item = div()
                    .id(part(&self.id, &option.key))
                    .aria_label(option.title.clone())
                    .aria_selected(is_chosen)
                    .relative()
                    .flex_none()
                    .min_h(px(30.))
                    .py(px(6.))
                    .pl(px(30.))
                    .pr(px(10.))
                    .rounded(px(6.))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        pick(false, window, cx);
                        if !is_chosen && let Some(on_choose) = &on_choose {
                            on_choose(index, window, cx);
                        }
                    })
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child(option.title.clone()),
                    );
                if let Some(line) = &option.line {
                    item = item.child(
                        div()
                            .id(part(&self.id, &format!("{}-line", option.key)))
                            .aria_label(line.clone())
                            .text_size(px(11.5))
                            .line_height(px(16.))
                            .text_color(p.faint)
                            .child(line.clone())
                            .test_support(),
                    );
                }
                selectable(item, is_chosen, move |d, hover, on| {
                    d.bg(p.hover.opacity(hover)).child(
                        div()
                            .absolute()
                            .left(px(10.))
                            .top(px(8.))
                            .opacity(on)
                            .child(icon(Icon::Check).size(px(12.)).text_color(p.text)),
                    )
                })
                .into_any_element()
            })
            .collect();
        let dismiss: Option<Handler> = open.then(|| {
            let close = set_open.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| close(false, window, cx)) as Handler
        });
        root.child(list_overlay(
            &self.id,
            HEIGHT,
            Align::Start,
            width.get(),
            items,
            dismiss,
            t,
            window,
            cx,
        ))
    }
}
