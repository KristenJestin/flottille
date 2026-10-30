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
use crate::motion::{ENTER, MAX_STAGGER, Motion, PANEL, RISE, STAGGER};
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
        fold_frame(
            self.id.clone(),
            self.open,
            running,
            t,
            &self.height,
            self.content.clone(),
        )
    }
}

/// `content` folded to `t` of its height (1 open, 0 folded): clipped and faded while `running`,
/// untouched once open at rest. `height` remembers the content's own height, measured on each
/// frame it is drawn, which a fold needs to start from when it closes.
pub fn fold_frame(
    id: impl Into<ElementId>,
    open: bool,
    running: bool,
    t: f32,
    height: &Rc<Cell<Pixels>>,
    content: impl IntoElement,
) -> impl IntoElement {
    let measured = height.clone();
    // Measures the content's own height, laid out free of the fold's clip.
    let measure = canvas(
        move |bounds: Bounds<Pixels>, _, _| measured.set(bounds.size.height),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full();
    let outer = div().id(id.into()).aria_expanded(open).w_full().relative();
    if !running && open {
        return outer
            .child(div().relative().child(content).child(measure))
            .test_support();
    }
    outer
        .h(height.get() * t)
        .overflow_hidden()
        .opacity(t)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .child(content)
                .child(measure),
        )
        .test_support()
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
        let delay = STAGGER * self.index.min(MAX_STAGGER) as u32;
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

/// What a block that comes and goes remembers between frames: how far it is shown and its own
/// height, which a fold needs to start from.
pub(crate) struct Shown {
    pub motion: Motion,
    pub height: Rc<Cell<Pixels>>,
    /// The last line a [`PresenceOf`] had, drawn again while it folds away.
    pub last: SharedString,
    /// A row's slide to its new place and the move it answers: see [`Item`](crate::Item).
    pub slide: Motion,
    pub moved: u64,
}

impl Shown {
    pub fn new(motion: Motion) -> Self {
        Self {
            motion,
            height: Rc::new(Cell::new(px(0.))),
            last: SharedString::default(),
            slide: Motion::settled(0.0),
            moved: 0,
        }
    }
}

/// Where a [`Shown`] is: how far (0 to 1), whether it still runs, and its own height.
pub(crate) struct Driven {
    pub t: f32,
    pub running: bool,
    pub height: Rc<Cell<Pixels>>,
}

/// Moves a [`Shown`] toward `open` and says where it is now; asks for the next frame while it
/// runs.
pub(crate) fn drive(
    state: &gpui_kit::Entity<Shown>,
    open: bool,
    window: &mut Window,
    cx: &mut App,
) -> Driven {
    let now = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    let target = if open { 1.0 } else { 0.0 };
    let (motion, height) = state.update(cx, |shown, _| {
        if shown.motion.target() != target {
            shown.motion.animate(target, PANEL, now, reduce);
        }
        (shown.motion, shown.height.clone())
    });
    let running = motion.is_running(now);
    if running {
        window.request_animation_frame();
    } else {
        state.update(cx, |shown, _| shown.motion.settle(now));
    }
    Driven {
        t: motion.value(now),
        running,
        height,
    }
}

/// `content` folded to `t`, with the `gap` its parent puts between siblings folding with it, so
/// that nothing jumps when the block ends. Nothing at all while it is closed and at rest. With
/// `rising`, a block that arrives is at its full size from its first frame and rises into place
/// while it fades in, as the design has a row enter; it still folds away when it leaves.
pub(crate) fn shown_frame(
    id: ElementId,
    open: bool,
    rising: bool,
    at: &Driven,
    gap: f32,
    offset: f32,
    content: AnyElement,
) -> AnyElement {
    let Driven { t, running, height } = at;
    let (t, running) = (*t, *running);
    if !open && !running {
        return div().hidden().into_any_element();
    }
    if open && rising {
        return div()
            .w_full()
            .relative()
            .top(px(offset + RISE * (1.0 - t)))
            .opacity(t)
            .child(
                div()
                    .id(id)
                    .aria_expanded(open)
                    .w_full()
                    .child(content)
                    .test_support(),
            )
            .into_any_element();
    }
    div()
        .w_full()
        .relative()
        .top(px(offset))
        .mt(px(-gap * (1.0 - t)))
        .child(fold_frame(id, open, running, t, height, content))
        .into_any_element()
}

/// A block that comes and goes while its view stays: it unfolds and fades in when `present`
/// turns true and folds away when it turns false (over [`PANEL`]). Build it on every frame, also
/// when it is absent, and not inside a `when`: it is its own state. Present at its first frame,
/// it is simply there (the page's entrance covers it).
pub fn presence(id: impl Into<ElementId>, present: bool, content: impl IntoElement) -> Presence {
    Presence {
        id: id.into(),
        present,
        gap: 0.0,
        content: content.into_any_element(),
    }
}

#[derive(IntoElement)]
pub struct Presence {
    id: ElementId,
    present: bool,
    gap: f32,
    content: AnyElement,
}

impl Presence {
    /// The gap the parent flex puts between its children, which comes and goes with the block.
    pub fn gap(mut self, gap: Pixels) -> Self {
        self.gap = f32::from(gap);
        self
    }
}

impl RenderOnce for Presence {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let key = ElementId::NamedChild(Arc::new(self.id.clone()), "presence".into());
        let open = self.present;
        let state = window.use_keyed_state(key, cx, move |_, _| {
            Shown::new(Motion::settled(if open { 1.0 } else { 0.0 }))
        });
        let at = drive(&state, open, window, cx);
        shown_frame(self.id, open, false, &at, self.gap, 0.0, self.content)
    }
}

/// Like [`presence`] for a block made of a line that goes away with it (a message, an error):
/// while it folds away it still shows the last line. `build` draws a line.
pub fn presence_of(
    id: impl Into<ElementId>,
    line: Option<SharedString>,
    build: impl FnOnce(SharedString) -> AnyElement + 'static,
) -> PresenceOf {
    PresenceOf {
        id: id.into(),
        line,
        gap: 0.0,
        build: Box::new(build),
    }
}

#[derive(IntoElement)]
pub struct PresenceOf {
    id: ElementId,
    line: Option<SharedString>,
    gap: f32,
    build: Box<dyn FnOnce(SharedString) -> AnyElement>,
}

impl PresenceOf {
    /// See [`Presence::gap`].
    pub fn gap(mut self, gap: Pixels) -> Self {
        self.gap = f32::from(gap);
        self
    }
}

impl RenderOnce for PresenceOf {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let key = ElementId::NamedChild(Arc::new(self.id.clone()), "presence".into());
        let open = self.line.is_some();
        let state = window.use_keyed_state(key, cx, move |_, _| {
            Shown::new(Motion::settled(if open { 1.0 } else { 0.0 }))
        });
        let line = state.update(cx, |shown, _| {
            if let Some(line) = self.line {
                shown.last = line;
            }
            shown.last.clone()
        });
        let at = drive(&state, open, window, cx);
        let content = if open || at.running {
            (self.build)(line)
        } else {
            div().into_any_element()
        };
        shown_frame(self.id, open, false, &at, self.gap, 0.0, content)
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

    pub fn text(&self) -> &str {
        &self.text
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
