//! The motion grammar: ease-out cubic, interruptible, reduced motion honoured.
//!
//! A [`Motion`] is a plain value a view owns. It knows nothing of GPUI: the view samples it with
//! the executor's clock while rendering and asks for the next frame only while it runs, so
//! nothing is redrawn once every motion has reached its target.

use std::time::{Duration, Instant};

/// A block, row, card or agent appears.
pub const ENTER: Duration = Duration::from_millis(240);
/// A level folds, a drawer slides, a state icon or pill swaps, a highlight moves.
pub const PANEL: Duration = Duration::from_millis(280);
/// A whole surface changes: the palette cross-fading to a new theme.
pub const COVER: Duration = Duration::from_millis(420);
/// A hover background.
pub const HOVER: Duration = Duration::from_millis(140);
/// A button pressed.
pub const PRESS: Duration = Duration::from_millis(90);

/// The gap between siblings that arrive together.
pub const STAGGER: Duration = Duration::from_millis(70);
/// How far an entrance rises while it fades in, in pixels.
pub const RISE: f32 = 6.0;
/// The size a new state icon or pill starts from.
pub const SWAP_FROM: f32 = 0.4;
/// The size of a button while it is pressed.
pub const PRESSED: f32 = 0.96;

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// One animated number, retargetable at any time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    from: f32,
    to: f32,
    /// When the current run started; `None` once settled.
    start: Option<Instant>,
    delay: Duration,
    duration: Duration,
}

impl Motion {
    /// A value at rest.
    pub fn settled(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: None,
            delay: Duration::ZERO,
            duration: Duration::ZERO,
        }
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    /// Heads for `target` from wherever the value is at `now`. With reduced motion, or a zero
    /// duration, it jumps.
    pub fn animate(&mut self, target: f32, duration: Duration, now: Instant, reduce: bool) {
        self.animate_after(target, Duration::ZERO, duration, now, reduce);
    }

    /// Like [`Motion::animate`], holding the current value for `delay` first (a stagger).
    pub fn animate_after(
        &mut self,
        target: f32,
        delay: Duration,
        duration: Duration,
        now: Instant,
        reduce: bool,
    ) {
        let current = self.value(now);
        if reduce || duration.is_zero() {
            *self = Self::settled(target);
            return;
        }
        if current == target && !self.is_running(now) {
            *self = Self::settled(target);
            return;
        }
        *self = Self {
            from: current,
            to: target,
            start: Some(now),
            delay,
            duration,
        };
    }

    /// Restarts from `from` toward `to` (an entrance, a swap).
    pub fn play(
        &mut self,
        from: f32,
        to: f32,
        delay: Duration,
        duration: Duration,
        now: Instant,
        reduce: bool,
    ) {
        *self = Self::settled(from);
        self.animate_after(to, delay, duration, now, reduce);
    }

    pub fn value(&self, now: Instant) -> f32 {
        let Some(start) = self.start else {
            return self.to;
        };
        let elapsed = now.saturating_duration_since(start);
        if elapsed < self.delay {
            return self.from;
        }
        let t = (elapsed - self.delay).as_secs_f32() / self.duration.as_secs_f32();
        if t >= 1.0 {
            return self.to;
        }
        self.from + (self.to - self.from) * ease_out_cubic(t)
    }

    /// Whether frames are still needed to draw this value.
    pub fn is_running(&self, now: Instant) -> bool {
        self.start
            .is_some_and(|start| now.saturating_duration_since(start) < self.delay + self.duration)
    }

    /// Forgets a finished run, so the next sample needs no clock.
    pub fn settle(&mut self, now: Instant) {
        if self.start.is_some() && !self.is_running(now) {
            *self = Self::settled(self.to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn eases_out_toward_its_target() {
        let now = t0();
        let mut m = Motion::settled(0.0);
        m.animate(1.0, PANEL, now, false);
        assert_eq!(m.value(now), 0.0);
        let half = m.value(now + PANEL / 2);
        assert!(
            half > 0.5 && half < 1.0,
            "ease-out is past half way at half time: {half}"
        );
        assert!(m.is_running(now + PANEL / 2));
        assert_eq!(m.value(now + PANEL), 1.0);
        assert!(!m.is_running(now + PANEL));
    }

    #[test]
    fn a_new_target_starts_from_the_current_value() {
        let now = t0();
        let mut m = Motion::settled(0.0);
        m.animate(1.0, PANEL, now, false);
        let mid = now + PANEL / 3;
        let at = m.value(mid);
        m.animate(0.0, PANEL, mid, false);
        assert_eq!(m.value(mid), at);
        assert!(m.value(mid + PANEL / 2) < at);
        assert_eq!(m.value(mid + PANEL), 0.0);
    }

    #[test]
    fn reduced_motion_jumps_to_the_target() {
        let now = t0();
        let mut m = Motion::settled(0.0);
        m.animate(1.0, PANEL, now, true);
        assert_eq!(m.value(now), 1.0);
        assert!(!m.is_running(now));
        m.play(0.0, 1.0, STAGGER, ENTER, now, true);
        assert_eq!(m.value(now), 1.0);
        assert!(!m.is_running(now));
    }

    #[test]
    fn a_delay_holds_the_start_value() {
        let now = t0();
        let mut m = Motion::settled(0.0);
        m.play(0.0, 1.0, STAGGER, ENTER, now, false);
        assert_eq!(m.value(now + STAGGER / 2), 0.0);
        assert!(m.is_running(now + STAGGER / 2));
        assert_eq!(m.value(now + STAGGER + ENTER), 1.0);
        assert!(!m.is_running(now + STAGGER + ENTER));
    }

    #[test]
    fn the_five_durations_follow_the_reference() {
        assert_eq!(
            [ENTER, PANEL, COVER, HOVER, PRESS].map(|d| d.as_millis()),
            [240, 280, 420, 140, 90]
        );
    }
}
