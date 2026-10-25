//! Things appearing and folding: entrances that fade in while rising, staggered among siblings;
//! folds that animate height and opacity; the "thinking" label that pulses its brightness.
//!
//! GPUI cannot blur an element nor draw a gradient inside text, so an entrance has no blur and the
//! thinking label pulses instead of shimmering.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AnyElement, AnyView, App, Bounds, Context, ElementId, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Task, TestSupportExt, Window, canvas, div, px,
};

use crate::color::mix;
use crate::motion::{ENTER, Motion, PANEL, RISE, STAGGER};
use crate::theme::palette;

/// A level of the tree or a card's body that folds and unfolds over [`PANEL`].
pub struct Fold {
    id: ElementId,
    open: bool,
    motion: Motion,
    height: Rc<Cell<Pixels>>,
    content: AnyView,
}

impl Fold {
    pub fn new(id: impl Into<ElementId>, content: impl Into<AnyView>, open: bool) -> Self {
        Self {
            id: id.into(),
            open,
            motion: Motion::settled(if open { 1.0 } else { 0.0 }),
            height: Rc::new(Cell::new(px(0.))),
            content: content.into(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        let now = cx.background_executor().now();
        self.motion
            .animate(if open { 1.0 } else { 0.0 }, PANEL, now, cx.reduce_motion());
        cx.notify();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.set_open(!self.open, cx);
    }
}

impl Render for Fold {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let running = self.motion.is_running(now);
        if running {
            window.request_animation_frame();
        } else {
            self.motion.settle(now);
        }
        let t = self.motion.value(now);
        let height = self.height.clone();
        // Measures the content's own height, laid out free of the fold's clip.
        let measure = canvas(
            move |bounds: Bounds<Pixels>, _, _| height.set(bounds.size.height),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let outer = div()
            .id(self.id.clone())
            .aria_expanded(self.open)
            .w_full()
            .relative();
        if !running && self.open {
            return outer
                .child(div().relative().child(self.content.clone()).child(measure))
                .test_support();
        }
        outer
            .h(self.height.get() * t)
            .overflow_hidden()
            .opacity(t)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .child(self.content.clone())
                    .child(measure),
            )
            .test_support()
    }
}

/// Wraps `child` in an entrance: it fades in while rising [`RISE`] px over [`ENTER`], after
/// `index` × [`STAGGER`] when siblings arrive together. It plays once, the first time `id` is
/// drawn.
pub fn enter(id: impl Into<ElementId>, index: usize, child: impl IntoElement) -> Enter {
    Enter {
        id: id.into(),
        index,
        child: child.into_any_element(),
    }
}

#[derive(IntoElement)]
pub struct Enter {
    id: ElementId,
    index: usize,
    child: AnyElement,
}

impl RenderOnce for Enter {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let delay = STAGGER * self.index as u32;
        let key = ElementId::NamedChild(Arc::new(self.id.clone()), "enter".into());
        let state = window.use_keyed_state(key, cx, |_, cx| {
            let mut m = Motion::settled(0.0);
            let now = cx.background_executor().now();
            m.play(0.0, 1.0, delay, ENTER, now, cx.reduce_motion());
            m
        });
        let now = cx.background_executor().now();
        let m = *state.read(cx);
        if m.is_running(now) {
            window.request_animation_frame();
        }
        let t = m.value(now);
        div()
            .id(self.id)
            .relative()
            .top(px(RISE * (1.0 - t)))
            .opacity(t)
            .child(self.child)
            .test_support()
    }
}

/// The "Thinking" label: while active its colour pulses between faint and text, once every
/// [`PULSE`]; at rest it is still.
pub struct PulsingLabel {
    text: SharedString,
    active: bool,
    since: std::time::Instant,
    ticker: Option<Task<()>>,
}

/// One brightness pulse of the thinking label.
pub const PULSE: Duration = Duration::from_millis(1800);
const PULSE_FRAME: Duration = Duration::from_millis(66);

impl PulsingLabel {
    pub fn new(text: impl Into<SharedString>, active: bool, cx: &mut Context<Self>) -> Self {
        let mut label = Self {
            text: text.into(),
            active: false,
            since: cx.background_executor().now(),
            ticker: None,
        };
        label.set_active(active, cx);
        label
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.text = text.into();
        cx.notify();
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        self.since = cx.background_executor().now();
        self.ticker = (active && !cx.reduce_motion()).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(PULSE_FRAME).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
        });
        cx.notify();
    }
}

impl Render for PulsingLabel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let brightness = if self.ticker.is_some() {
            let elapsed = cx
                .background_executor()
                .now()
                .saturating_duration_since(self.since);
            let phase = (elapsed.as_secs_f32() / PULSE.as_secs_f32()).fract();
            0.5 - 0.5 * (phase * std::f32::consts::TAU).cos()
        } else {
            0.0
        };
        div()
            .text_color(mix(p.faint, p.text, brightness))
            .child(self.text.clone())
    }
}
