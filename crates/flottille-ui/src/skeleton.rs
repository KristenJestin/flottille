//! What a section shows while it is still being read: a [`Skeleton`] block of the size of the
//! line it stands for, pulsing slowly as the thinking label does, and [`awaiting`], which
//! crossfades it into the real content in place while the height eases from one to the other.
//!
//! GPUI cannot draw a gradient, so a skeleton pulses its brightness instead of shimmering.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, ElementId, Entity, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, RenderOnce, Styled, Task, TestSupportExt, Window,
    canvas, div, px,
};

use crate::motion::{Motion, PANEL};
use crate::reveal::PULSE;
use crate::theme::palette;

/// Ten draws a second: a pulse of nearly two seconds is smooth with no more, and a forge that
/// hangs keeps the skeleton drawn for as long as it hangs.
const PULSE_FRAME: Duration = Duration::from_millis(100);
/// How faint a skeleton gets at the bottom of its pulse, and how bright at the top.
const FAINTEST: f32 = 0.35;
const BRIGHTEST: f32 = 0.85;

/// The opacity of a skeleton `elapsed` into the clock all skeletons share, so that those drawn
/// together pulse together.
pub fn pulse_opacity(elapsed: Duration) -> f32 {
    let phase = (elapsed.as_secs_f32() / PULSE.as_secs_f32()).fract();
    let wave = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
    FAINTEST + (BRIGHTEST - FAINTEST) * wave
}

/// A block standing for a line that is still being read: draw one of the size of the line.
#[derive(IntoElement)]
pub struct Skeleton {
    id: ElementId,
    width: Option<Pixels>,
    height: Pixels,
}

impl Skeleton {
    /// As wide as the room it is in, 12 px high.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            width: None,
            height: px(12.),
        }
    }

    pub fn w(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    pub fn h(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }
}

/// What the block redraws: its own small view, so that a pulse never draws the screen around it.
struct Pulse {
    ticker: Option<Task<()>>,
    height: Pixels,
}

impl Pulse {
    fn new(height: Pixels, cx: &mut Context<Self>) -> Self {
        let ticker = (!cx.reduce_motion()).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(PULSE_FRAME).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
        });
        Self { ticker, height }
    }
}

thread_local! {
    static DRAWN: Cell<usize> = const { Cell::new(0) };
}

/// How many times a skeleton's block was drawn on this thread: a pulse is a redraw of its own
/// small view, which a test can only see by counting.
pub fn blocks_drawn() -> usize {
    DRAWN.with(Cell::get)
}

/// The instant all skeletons count their pulse from.
fn epoch(now: Instant) -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(|| now)
}

impl Render for Pulse {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        DRAWN.with(|drawn| drawn.set(drawn.get() + 1));
        let opacity = match self.ticker {
            Some(_) => {
                let now = cx.background_executor().now();
                pulse_opacity(now.saturating_duration_since(epoch(now)))
            }
            None => (FAINTEST + BRIGHTEST) / 2.0,
        };
        div()
            .w_full()
            .h(self.height)
            .rounded(px(4.))
            .bg(p.line2.opacity(opacity))
    }
}

/// Holds the pulse's view: the state is watched by the screen that draws the skeleton, the view
/// inside it is not.
struct Held(Entity<Pulse>);

impl RenderOnce for Skeleton {
    fn render(self, window: &mut Window, cx: &mut gpui_kit::App) -> impl IntoElement {
        let key = ElementId::NamedChild(Arc::new(self.id.clone()), "skeleton".into());
        let height = self.height;
        let held = window.use_keyed_state(key, cx, move |_, cx| {
            Held(cx.new(|cx| Pulse::new(height, cx)))
        });
        let pulse = held.read(cx).0.clone();
        pulse.update(cx, |pulse, _| pulse.height = height);
        let block = div().id(self.id).flex_none().h(height);
        match self.width {
            Some(width) => block.w(width),
            None => block.w_full(),
        }
        .child(pulse)
        .test_support()
    }
}

/// A part of a screen that waits for a slow read: while `pending` it shows `skeleton`; when
/// `pending` turns false the skeleton fades out and `content` fades in at the same place, and
/// the height eases from the one's to the other's over [`PANEL`] (to nothing when `content` has
/// no height: what was waited for never came). Present with `pending` false at its first frame,
/// it is simply there. Build it on every frame; it is its own state.
pub fn awaiting(
    id: impl Into<ElementId>,
    pending: bool,
    skeleton: impl IntoElement,
    content: impl IntoElement,
) -> Awaiting {
    Awaiting {
        id: id.into(),
        pending,
        skeleton: skeleton.into_any_element(),
        content: content.into_any_element(),
    }
}

#[derive(IntoElement)]
pub struct Awaiting {
    id: ElementId,
    pending: bool,
    skeleton: AnyElement,
    content: AnyElement,
}

/// What an [`Awaiting`] remembers: how far the content has replaced the skeleton, and the height
/// each of them had when it was last drawn (cells, because the measure runs while the frame is
/// drawn).
struct Waited {
    motion: Motion,
    skeleton: Rc<Cell<f32>>,
    content: Rc<Cell<Option<f32>>>,
}

/// Measures the height of the layer it is in, laid out free of any clip.
fn measure<T: Copy + 'static>(cell: &Rc<Cell<T>>, of: fn(f32) -> T) -> gpui_kit::Canvas<()> {
    let cell = cell.clone();
    canvas(
        move |bounds: Bounds<Pixels>, _, _| cell.set(of(f32::from(bounds.size.height))),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

impl RenderOnce for Awaiting {
    fn render(self, window: &mut Window, cx: &mut gpui_kit::App) -> impl IntoElement {
        let key = ElementId::NamedChild(Arc::new(self.id.clone()), "awaiting".into());
        let ready = !self.pending;
        let state = window.use_keyed_state(key, cx, move |_, _| Waited {
            motion: Motion::settled(if ready { 1.0 } else { 0.0 }),
            skeleton: Rc::new(Cell::new(0.0)),
            content: Rc::new(Cell::new(None)),
        });
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let (t, running, skeleton_h, content_h) = state.update(cx, |waited, _| {
            let target = if ready { 1.0 } else { 0.0 };
            if waited.motion.target() != target {
                waited.motion.animate(target, PANEL, now, reduce);
            }
            let running = waited.motion.is_running(now);
            if !running {
                waited.motion.settle(now);
            }
            (
                waited.motion.value(now),
                running,
                waited.skeleton.clone(),
                waited.content.clone(),
            )
        });
        if running {
            window.request_animation_frame();
        }
        let outer = div().id(self.id).w_full().relative();
        if !running {
            return match ready {
                true => outer
                    .child(self.content)
                    .child(measure(&content_h, Some))
                    .test_support(),
                false => outer
                    .child(self.skeleton)
                    .child(measure(&skeleton_h, |h| h))
                    .test_support(),
            };
        }
        let from = skeleton_h.get();
        let to = content_h.get().unwrap_or(from);
        outer
            .h(px(from + (to - from) * t))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .opacity(1.0 - t)
                    .child(self.skeleton)
                    .child(measure(&skeleton_h, |h| h)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .opacity(t)
                    .child(self.content)
                    .child(measure(&content_h, Some)),
            )
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pulse_goes_from_faint_to_bright_and_back_once_every_pulse() {
        let at = |ms: u64| pulse_opacity(Duration::from_millis(ms));
        assert!((at(0) - FAINTEST).abs() < 1e-4);
        assert!((at(900) - BRIGHTEST).abs() < 1e-4);
        assert!((at(1800) - FAINTEST).abs() < 1e-4);
        assert!(FAINTEST < at(450) && at(450) < at(900));
    }
}
