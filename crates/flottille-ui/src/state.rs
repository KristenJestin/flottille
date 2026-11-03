//! The state vocabulary, drawn the same way wherever it appears.
//!
//! An agent's or a session's state is held once, in a [`Status`] entity, and every component that
//! shows it ([`StateIcon`], [`Pill`]) reads that entity: one change redraws all of them, and
//! nothing else. Each is its own small view, so the spinner turning or the waiting ring pulsing
//! redraws only that mark. The shape carries the state as well as the colour.

use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext, Bounds, Context, Entity, Hsla, InteractiveElement, IntoElement, ParentElement,
    PathBuilder, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Task, TestSupportExt, Window, canvas, div, point, px, relative,
};

use crate::motion::{Motion, PANEL, SWAP_FROM};
use crate::theme::{Palette, palette, radius};
use crate::typography::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentState {
    Running,
    /// Waits for the user. Only a root agent, its session and what holds them can be here.
    NeedsYou,
    /// A sub-agent waiting on its parent.
    Waiting,
    Completed,
    Idle,
    Denied,
    Stopped,
    Failed,
}

/// The shape a state is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A ring open on one side, turning.
    Spinner,
    /// A dot whose ring pulses.
    Pulse,
    Hollow,
    /// A check in a round.
    Check,
    /// A cross in a round.
    Cross,
}

impl AgentState {
    pub const ALL: [Self; 8] = [
        Self::Running,
        Self::NeedsYou,
        Self::Waiting,
        Self::Completed,
        Self::Idle,
        Self::Denied,
        Self::Stopped,
        Self::Failed,
    ];

    /// The pill's text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::NeedsYou => "NEEDS YOU",
            Self::Waiting => "WAITING",
            Self::Completed => "COMPLETED",
            Self::Idle => "IDLE",
            Self::Denied => "DENIED",
            Self::Stopped => "STOPPED",
            Self::Failed => "FAILED",
        }
    }

    pub fn mark(self) -> Mark {
        match self {
            Self::Running => Mark::Spinner,
            Self::NeedsYou => Mark::Pulse,
            Self::Waiting | Self::Idle | Self::Denied | Self::Stopped => Mark::Hollow,
            Self::Completed => Mark::Check,
            Self::Failed => Mark::Cross,
        }
    }

    /// Whether its mark moves: only running and waiting for the user move at rest.
    pub fn moves(self) -> bool {
        matches!(self, Self::Running | Self::NeedsYou)
    }

    /// The ink and background of its pill.
    pub fn colors(self, p: &Palette) -> (Hsla, Hsla) {
        match self {
            Self::Running => (p.run, p.run_bg),
            Self::NeedsYou => (p.wait, p.wait_bg),
            Self::Completed => (p.ok, p.ok_bg),
            Self::Failed => (p.fail, p.fail_bg),
            Self::Waiting | Self::Idle | Self::Denied | Self::Stopped => (p.faint, p.card2),
        }
    }

    /// The mark a folded level shows for what it holds: waiting for the user beats running, and
    /// nothing else is shown.
    pub fn strongest(states: impl IntoIterator<Item = Self>) -> Option<Self> {
        let mut found = None;
        for state in states {
            match state {
                Self::NeedsYou => return Some(Self::NeedsYou),
                Self::Running => found = Some(Self::Running),
                _ => {}
            }
        }
        found
    }
}

/// The one place a state is held.
pub struct Status {
    state: AgentState,
}

impl Status {
    pub fn new(state: AgentState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> AgentState {
        self.state
    }

    pub fn set(&mut self, state: AgentState, cx: &mut Context<Self>) {
        if self.state != state {
            self.state = state;
            cx.notify();
        }
    }
}

/// One turn of the spinner.
pub const SPIN_TURN: Duration = Duration::from_millis(800);
/// One pulse of the waiting ring.
pub const PULSE_PERIOD: Duration = Duration::from_millis(1800);
/// The spinner turns at a low frame rate: the cost of an animation is the frames it asks for.
const SPIN_FRAME: Duration = Duration::from_millis(40);
const PULSE_FRAME: Duration = Duration::from_millis(66);

/// What a state view shares: the state it shows, the swap in when it changes, the ticker while
/// its mark moves.
struct Shown {
    status: Entity<Status>,
    state: AgentState,
    swap: Motion,
    since: Instant,
    ticker: Option<Task<()>>,
    _observe: Subscription,
}

impl Shown {
    fn new<V: 'static>(status: Entity<Status>, ticks: bool, cx: &mut Context<V>) -> Self {
        let state = status.read(cx).state();
        let _observe = cx.observe(&status, |_, _, cx| cx.notify());
        let mut shown = Self {
            status,
            state,
            swap: Motion::settled(1.0),
            since: cx.background_executor().now(),
            ticker: None,
            _observe,
        };
        if ticks {
            shown.tick(cx);
        }
        shown
    }

    /// Catches up with the model, and asks for a frame only while the swap runs.
    fn sync<V: 'static>(&mut self, ticks: bool, window: &mut Window, cx: &mut Context<V>) -> f32 {
        let now = cx.background_executor().now();
        let state = self.status.read(cx).state();
        if state != self.state {
            self.state = state;
            self.since = now;
            self.swap.play(
                SWAP_FROM,
                1.0,
                Duration::ZERO,
                PANEL,
                now,
                cx.reduce_motion(),
            );
            if ticks {
                self.tick(cx);
            }
        }
        if self.swap.is_running(now) {
            window.request_animation_frame();
        } else {
            self.swap.settle(now);
        }
        self.swap.value(now)
    }

    /// Starts the ticker if the mark moves (and motion is allowed), stops it otherwise.
    fn tick<V: 'static>(&mut self, cx: &mut Context<V>) {
        let frame = match self.state {
            AgentState::Running => SPIN_FRAME,
            AgentState::NeedsYou => PULSE_FRAME,
            _ => {
                self.ticker = None;
                return;
            }
        };
        if cx.reduce_motion() {
            self.ticker = None;
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(frame).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    /// How far into its cycle a moving mark is, 0 to 1.
    fn phase(&self, period: Duration, cx: &App) -> f32 {
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(self.since);
        (elapsed.as_secs_f32() / period.as_secs_f32()).fract()
    }
}

/// A state's icon, in a 16 px box.
pub struct StateIcon {
    shown: Shown,
}

impl StateIcon {
    pub fn new(status: Entity<Status>, cx: &mut Context<Self>) -> Self {
        Self {
            shown: Shown::new(status, true, cx),
        }
    }

    pub fn status(&self) -> &Entity<Status> {
        &self.shown.status
    }
}

/// The element id of a state's icon, the same everywhere (`state-icon-running`, …).
pub fn icon_id(state: AgentState) -> SharedString {
    format!("state-icon-{}", slug(state)).into()
}

/// The element id of a state's pill (`pill-needs-you`, …).
pub fn pill_id(state: AgentState) -> SharedString {
    format!("pill-{}", slug(state)).into()
}

fn slug(state: AgentState) -> String {
    state.label().to_lowercase().replace(' ', "-")
}

impl Render for StateIcon {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.shown.sync(true, window, cx);
        let state = self.shown.state;
        let p = palette(cx);
        let size = |base: f32| px(base * scale);
        let opacity = ((scale - SWAP_FROM) / (1.0 - SWAP_FROM)).clamp(0.0, 1.0);
        let still = cx.reduce_motion();
        let mark = match state.mark() {
            Mark::Spinner => {
                let turn = if still {
                    0.0
                } else {
                    self.shown.phase(SPIN_TURN, cx)
                };
                spinner(p.run, turn).size(size(12.)).into_any_element()
            }
            Mark::Pulse => {
                let t = if still {
                    0.0
                } else {
                    self.shown.phase(PULSE_PERIOD, cx)
                };
                let ease = 1.0 - (1.0 - t).powi(2);
                let halo = 8.0 + 14.0 * ease;
                div()
                    .size(size(8.))
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .absolute()
                            .size(px(halo * scale))
                            .rounded_full()
                            .bg(p.wait.opacity(0.55 * (1.0 - ease))),
                    )
                    .child(div().absolute().size_full().rounded_full().bg(p.wait))
                    .into_any_element()
            }
            Mark::Hollow => div()
                .size(size(8.))
                .rounded_full()
                .border(px(1.4))
                .border_color(p.faint)
                .into_any_element(),
            Mark::Check => round(p.ok_bg)
                .size(size(15.))
                .child(check(p.ok).size(relative(0.6)))
                .into_any_element(),
            Mark::Cross => round(p.fail_bg)
                .size(size(15.))
                .child(cross(p.fail).size(relative(0.5)))
                .into_any_element(),
        };
        div()
            .id(icon_id(state))
            .test_support()
            .aria_label(state.label().to_lowercase())
            .size(px(16.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .opacity(opacity)
            .child(mark)
    }
}

fn round(bg: Hsla) -> gpui_kit::Div {
    div()
        .rounded_full()
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
}

/// A ring open on one quarter, `turn` (0 to 1) of the way round.
fn spinner(color: Hsla, turn: f32) -> gpui_kit::Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let stroke = 1.6;
            let r = f32::from(bounds.size.width) / 2.0 - stroke / 2.0;
            let c = bounds.center();
            let at = |a: f32| point(c.x + px(r * a.cos()), c.y + px(r * a.sin()));
            let start = turn * TAU;
            let mut path = PathBuilder::stroke(px(stroke));
            path.move_to(at(start));
            path.arc_to(
                point(px(r), px(r)),
                px(0.),
                true,
                true,
                at(start + TAU * 0.75),
            );
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
}

/// Draws `points` (in a 12-unit box) as one stroked line, scaled to the bounds.
fn strokes(color: Hsla, lines: &'static [&'static [(f32, f32)]]) -> gpui_kit::Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let unit = f32::from(bounds.size.width) / 12.0;
            let at = |(x, y): (f32, f32)| {
                point(
                    bounds.origin.x + px(x * unit),
                    bounds.origin.y + px(y * unit),
                )
            };
            let mut path = PathBuilder::stroke(px(2.0 * unit.max(0.75)));
            for line in lines {
                path.move_to(at(line[0]));
                for &p in &line[1..] {
                    path.line_to(at(p));
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
}

fn check(color: Hsla) -> gpui_kit::Canvas<()> {
    strokes(color, &[&[(2.5, 6.3), (4.8, 8.5), (9.5, 3.5)]])
}

fn cross(color: Hsla) -> gpui_kit::Canvas<()> {
    strokes(
        color,
        &[&[(2.5, 2.5), (9.5, 9.5)], &[(9.5, 2.5), (2.5, 9.5)]],
    )
}

/// A state's pill: mono 10 px, weight 500, uppercase, 4×6 px padding, radius 5.
pub struct Pill {
    shown: Shown,
    /// The pill says nothing while the state is `Running`: the icon beside it already does.
    quiet_running: bool,
    /// Nor while it is `NeedsYou`, on a page whose "Waiting for you" says it once.
    quiet_needs_you: bool,
}

impl Pill {
    pub fn new(status: Entity<Status>, cx: &mut Context<Self>) -> Self {
        Self {
            shown: Shown::new(status, false, cx),
            quiet_running: false,
            quiet_needs_you: false,
        }
    }

    /// A pill that draws nothing while the state is `Running`.
    pub fn quiet_running(mut self) -> Self {
        self.quiet_running = true;
        self
    }

    /// A pill that draws nothing while the state is `NeedsYou`: a view says it once, elsewhere.
    pub fn quiet_needs_you(mut self) -> Self {
        self.quiet_needs_you = true;
        self
    }
}

impl Render for Pill {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.shown.sync(false, window, cx);
        let state = self.shown.state;
        if (self.quiet_running && state == AgentState::Running)
            || (self.quiet_needs_you && state == AgentState::NeedsYou)
        {
            return div().into_any_element();
        }
        let (ink, bg) = state.colors(&palette(cx));
        let opacity = ((scale - SWAP_FROM) / (1.0 - SWAP_FROM)).clamp(0.0, 1.0);
        div()
            .id(pill_id(state))
            .test_support()
            .aria_label(state.label().to_lowercase())
            .flex_none()
            .px(px(6. * scale))
            .py(px(4. * scale))
            .rounded(radius::PILL)
            .bg(bg)
            .text_color(ink)
            .mono(10. * scale)
            .line_height(px(10. * scale))
            .medium()
            .whitespace_nowrap()
            .opacity(opacity)
            .child(state.label())
            .into_any_element()
    }
}

/// A state's icon and pill reading one model, made in one call.
pub fn views(status: &Entity<Status>, cx: &mut App) -> (Entity<StateIcon>, Entity<Pill>) {
    let icon = cx.new(|cx| StateIcon::new(status.clone(), cx));
    let pill = cx.new(|cx| Pill::new(status.clone(), cx));
    (icon, pill)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_for_the_user_beats_running() {
        use AgentState::*;
        assert_eq!(
            AgentState::strongest([Completed, Running, NeedsYou, Idle]),
            Some(NeedsYou)
        );
        assert_eq!(
            AgentState::strongest([Completed, Running, Waiting]),
            Some(Running)
        );
        assert_eq!(AgentState::strongest([Completed, Waiting, Failed]), None);
    }

    #[test]
    fn the_shape_carries_the_state() {
        use AgentState::*;
        assert_eq!(Running.mark(), Mark::Spinner);
        assert_eq!(NeedsYou.mark(), Mark::Pulse);
        for quiet in [Waiting, Idle, Denied, Stopped] {
            assert_eq!(quiet.mark(), Mark::Hollow);
        }
        assert_eq!(Completed.mark(), Mark::Check);
        assert_eq!(Failed.mark(), Mark::Cross);
        assert!(
            AgentState::ALL
                .iter()
                .filter(|s| s.moves())
                .eq([Running, NeedsYou].iter())
        );
    }

    #[test]
    fn waiting_on_a_parent_is_never_amber() {
        let p = Palette::base();
        assert_ne!(AgentState::Waiting.colors(&p).0, p.wait);
        assert_eq!(AgentState::NeedsYou.colors(&p).0, p.wait);
    }
}
