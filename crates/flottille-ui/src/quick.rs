//! A quick switcher's look: [`QuickPanel`], a search field over a list of results, drawn over
//! the page; and [`QuickRow`], one result.
//!
//! Like [`Modal`](crate::Modal) it is drawn all the time and told whether it is
//! [`open`](QuickPanel::open): the scrim fades and the panel fades and rises when it opens, the
//! other way when it closes, and once that has ended nothing is drawn. Escape, a click outside
//! and the mouse's back button close it. It is at the top of the window, where the eye already
//! is, not in the middle: its height changes with the number of results.
//!
//! The screen owns what is typed, the results (a [`Listing`](crate::Listing) of
//! [`Item`](crate::Item)s, so that they come, go and move) and which one is marked; this draws.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, ClickEvent, ElementId, FontWeight, HighlightStyle,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, RenderOnce,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledText, TestSupportExt,
    Window, anchored, deferred, div, point, px,
};

use crate::color::mix;
use crate::icons::{Icon, icon};
use crate::interact::{choose, dismiss_on_navigate, feel, open_overlay, wire_with};
use crate::layer;
use crate::motion::{MODAL, Motion, RISE};
use crate::space;
use crate::theme::{palette, radius};
use crate::typography::Type;

/// What a panel remembers between two frames: whether it was open, and its entrance.
struct Shown {
    open: bool,
    motion: Motion,
}

type Handler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type KeyHandler = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App) + 'static>;

/// The panel's width.
pub const QUICK_WIDTH: f32 = 600.;
/// The most the results take before they scroll.
pub const QUICK_RESULTS: f32 = 400.;

#[derive(IntoElement)]
pub struct QuickPanel {
    id: ElementId,
    field: Option<AnyElement>,
    results: Vec<AnyElement>,
    scroll: Option<ScrollHandle>,
    empty: Option<AnyElement>,
    open: bool,
    on_close: Option<Handler>,
    on_key: Option<KeyHandler>,
}

impl QuickPanel {
    /// The panel `id`, its scrim `<id>-scrim`, its results `<id>-results`.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            field: None,
            results: Vec::new(),
            scroll: None,
            empty: None,
            open: true,
            on_close: None,
            on_key: None,
        }
    }

    /// The text field, with its visible label: the screen builds it.
    pub fn field(mut self, field: impl IntoElement) -> Self {
        self.field = Some(field.into_any_element());
        self
    }

    /// Whether it is open, `true` unless said.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// The results, in order: group headings and [`QuickRow`]s, each inside an
    /// [`Item`](crate::Item).
    pub fn results(mut self, results: impl IntoIterator<Item = AnyElement>) -> Self {
        self.results.extend(results);
        self
    }

    /// Keeps the marked result in sight as the arrows move it.
    pub fn scroll(mut self, scroll: &ScrollHandle) -> Self {
        self.scroll = Some(scroll.clone());
        self
    }

    /// What stands in the results' place when there are none: an
    /// [`EmptyLine`](crate::EmptyLine), folded by the screen as the results come.
    pub fn empty(mut self, empty: impl IntoElement) -> Self {
        self.empty = Some(empty.into_any_element());
        self
    }

    /// Escape, a click outside and the mouse's back button call it.
    pub fn on_close(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }

    /// A key pressed in the panel, the field's own included, other than Escape: the arrows that
    /// move the mark.
    pub fn on_key(mut self, f: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_key = Some(Rc::new(f));
        self
    }
}

fn part(id: &ElementId, name: &'static str) -> ElementId {
    match id {
        ElementId::Name(text) => ElementId::Name(format!("{text}-{name}").into()),
        other => ElementId::NamedChild(Arc::new(other.clone()), name.into()),
    }
}

impl RenderOnce for QuickPanel {
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
        // Out of the page's flow, open or not: it takes no room where it is drawn.
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
            open_overlay(self.id.clone(), close.clone(), cx);
        }
        let (on_scrim, on_escape) = (close.clone(), close.clone());
        let on_key = self.on_key.clone();
        let scrim = div()
            .id(part(&self.id, "scrim"))
            .absolute()
            .inset_0()
            .bg(p.scrim.opacity(t))
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| on_scrim(window, cx))
            .test_support();
        let scrim = dismiss_on_navigate(scrim, close.clone());
        let results = div()
            .id(part(&self.id, "results"))
            .min_h_0()
            .max_h(px(QUICK_RESULTS))
            .overflow_y_scroll()
            .when_some(self.scroll, |d, scroll| d.track_scroll(&scroll))
            .child(
                div()
                    .p(space::SM)
                    .flex()
                    .flex_col()
                    .children(self.results)
                    .children(self.empty),
            );
        let panel = div()
            .id(self.id.clone())
            .occlude()
            .relative()
            .top(px(RISE * (1.0 - t)))
            .opacity(t)
            .w(px(QUICK_WIDTH))
            .max_w(viewport.width - px(48.))
            .max_h(viewport.height - px(96.))
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
                    on_escape(window, cx);
                } else if let Some(on_key) = &on_key {
                    on_key(event, window, cx);
                }
            })
            .child(
                div()
                    .flex_none()
                    .p(space::MD)
                    .border_b_1()
                    .border_color(p.line)
                    .children(self.field),
            )
            .child(results)
            .child(
                div()
                    .id(part(&self.id, "keys"))
                    .flex_none()
                    .px(space::MD)
                    .py(space::SM)
                    .border_t_1()
                    .border_color(p.line)
                    .mono(11.)
                    .text_color(p.faint)
                    .child("↑↓ move · Enter choose · Esc close"),
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
                    .items_start()
                    .justify_center()
                    .pt(px(72.))
                    .child(scrim)
                    .child(panel),
            ),
        )
        .priority(layer::MODAL);
        div().absolute().child(layer).into_any_element()
    }
}

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type OnMark = Rc<dyn Fn(&mut App) + 'static>;

/// One result: a mark, its title with what matched drawn in the accent, a line of context and
/// what it says at the right (a state, a time). The marked one is filled; the fill moves over
/// [`PANEL`](crate::motion::PANEL) as the mark does, the hover fades over
/// [`HOVER`](crate::motion::HOVER), and the pointer entering a row marks it.
#[derive(IntoElement)]
pub struct QuickRow {
    id: ElementId,
    icon: Option<Icon>,
    title: Vec<(String, bool)>,
    line: Option<SharedString>,
    trailing: Option<AnyElement>,
    marked: bool,
    on_click: Option<OnClick>,
    on_mark: Option<OnMark>,
}

impl QuickRow {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon: None,
            title: vec![(title.into().to_string(), false)],
            line: None,
            trailing: None,
            marked: false,
            on_click: None,
            on_mark: None,
        }
    }

    /// The title in pieces, each with whether it matched what was typed
    /// ([`Match::pieces`](crate::fuzzy::Match::pieces)).
    pub fn pieces(mut self, pieces: Vec<(String, bool)>) -> Self {
        self.title = pieces;
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Where it is, after the title: a session's project and place.
    pub fn line(mut self, line: impl Into<SharedString>) -> Self {
        self.line = Some(line.into());
        self
    }

    /// On the right: a state, how long ago.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    /// Whether it is the one Enter chooses.
    pub fn marked(mut self, marked: bool) -> Self {
        self.marked = marked;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }

    /// The pointer entered the row: the screen marks it.
    pub fn on_mark(mut self, f: impl Fn(&mut App) + 'static) -> Self {
        self.on_mark = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for QuickRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let (state, now) = feel(&self.id, window, cx);
        let background = mix(
            p.hover.opacity(now.hover),
            p.card2,
            choose(&state, self.marked, window, cx),
        );
        let text: String = self.title.iter().map(|(piece, _)| piece.as_str()).collect();
        let mut at = 0;
        let marks: Vec<_> = self
            .title
            .iter()
            .filter_map(|(piece, hit)| {
                let range = at..at + piece.len();
                at = range.end;
                hit.then_some((
                    range,
                    HighlightStyle {
                        color: Some(p.acc),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                ))
            })
            .collect();
        let mut row = div()
            .id(self.id.clone())
            .aria_label(text.clone())
            .w_full()
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .gap(space::MD)
            .px(space::MD)
            .rounded(radius::LIST_ROW)
            .bg(background)
            .text_color(p.text)
            .ui_text()
            .text_size(px(13.))
            .child(
                div()
                    .w(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(
                        self.icon
                            .map(|mark| icon(mark).size(px(14.)).text_color(p.mute)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(space::SM)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_none()
                            .max_w_full()
                            .font_weight(FontWeight::MEDIUM)
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(StyledText::new(text).with_highlights(marks)),
                    )
                    .children(self.line.map(|line| {
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(p.faint)
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(line)
                    })),
            )
            .children(
                self.trailing
                    .map(|trailing| div().flex_none().child(trailing)),
            );
        if let Some(on_click) = self.on_click {
            row = row.cursor_pointer().on_click(on_click);
        }
        let on_mark = self.on_mark;
        wire_with(row, state, false, move |hovered, cx| {
            if let (true, Some(mark)) = (hovered, &on_mark) {
                mark(cx);
            }
        })
        .test_support()
    }
}
