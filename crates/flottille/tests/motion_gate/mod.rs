//! The motion gate (#528): a scenario is played on the window's clock, frame by frame, and every
//! observed element (an id with `test_support`) is sampled at each frame: its bounds and whether
//! it is drawn (its own opacity above 0 and a part of it inside its clip). The checker then lists
//! what a person looking at the window would notice: a block that pops in or out, a row cut away
//! before it folded, an element that jumps in one frame (alone, or because a sibling popped), and
//! a window that never settles. An allow list says, entry by entry, which finding is intended and
//! why; an entry that matches nothing is reported, so that the list does not rot.
//!
//! What it cannot see: an opacity between 0 and 1 (GPUI says only whether an element is drawn),
//! a colour, and an element with no `test_support`, which is seen only through the observed
//! elements it moves.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use gpui_kit::VisualTestContext;
use gpui_kit::test::TestQueryExt as _;

/// Between the ids of a path: an id itself may hold a `/` (a source location).
const SEP: &str = " > ";
/// One frame of the window's clock.
pub const STEP: Duration = Duration::from_millis(16);
/// How long the window is played once a scenario ends, to find what never settles.
pub const SETTLE: Duration = Duration::from_secs(1);
/// A move of more than this in one frame, with none before or after, is a jump.
const JUMP_PX: f32 = 1.0;
/// Less than this is no move at all (the tail of an easing).
const STILL_PX: f32 = 0.01;
/// How many frames after it arrives (or before it goes) an element must be seen moving for its
/// arrival (or exit) to be animated.
const NEAR: usize = 3;
/// How far apart an element and the fold that brings it may arrive (or go): the content of a
/// fold is clipped to nothing until the fold reaches it (`PANEL` is 280 ms, 18 frames).
const FOLD_FRAMES: usize = 20;

/// What an observed element was drawn as, in one frame.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Sample {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    visible: bool,
}

impl Sample {
    /// Drawn with a size: what a person sees.
    fn shown(&self) -> bool {
        self.visible && self.w > 0.5 && self.h > 0.5
    }

    /// Drawn at its size but not seen: at an opacity of 0, which a fade starts or ends at.
    fn faded(&self) -> bool {
        !self.visible && self.w > 0.5 && self.h > 0.5
    }

    fn area(&self) -> f32 {
        self.w.max(0.) * self.h.max(0.)
    }

    /// The largest change of an edge or a size from `other`.
    fn delta(&self, other: &Sample) -> f32 {
        (self.x - other.x)
            .abs()
            .max((self.y - other.y).abs())
            .max((self.w - other.w).abs())
            .max((self.h - other.h).abs())
    }
}

/// Every observed element of one frame, by its path of ids (`page > section > row`).
type Frame = HashMap<String, Sample>;

fn sample(cx: &mut VisualTestContext) -> Frame {
    let frame: Frame = cx.update(|window, _| {
        window
            .elements()
            .into_iter()
            .map(|element| {
                let path = element
                    .path()
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(SEP);
                let b = element.bounds();
                let sample = Sample {
                    x: f32::from(b.origin.x),
                    y: f32::from(b.origin.y),
                    w: f32::from(b.size.width),
                    h: f32::from(b.size.height),
                    visible: element.visible(),
                };
                (path, sample)
            })
            .collect()
    });
    // GPUI says whether an element's own opacity is above 0; a parent's applies to its
    // children too (an `enter` waiting for its turn holds a button drawn at 0).
    let hidden: Vec<String> = frame
        .keys()
        .filter(|key| ancestors(key).any(|above| frame.get(above).is_some_and(|s| !s.visible)))
        .cloned()
        .collect();
    let mut frame = frame;
    for key in hidden {
        if let Some(s) = frame.get_mut(&key) {
            s.visible = false;
        }
    }
    frame
}

/// Plays a scenario frame by frame and keeps every frame.
pub struct Recorder {
    scenario: &'static str,
    frames: Vec<Frame>,
    /// The time spent reading the elements, all frames together.
    sampling: Duration,
    /// What the window must do before each frame besides its clock (a shell polls its link).
    before: Option<Box<dyn Fn(&mut VisualTestContext)>>,
}

impl Recorder {
    /// Starts on the window as it is now: its first frame is where the scenario starts from.
    pub fn start(scenario: &'static str, cx: &mut VisualTestContext) -> Self {
        Self {
            scenario,
            frames: vec![sample(cx)],
            sampling: Duration::ZERO,
            before: None,
        }
    }

    /// Runs `before` ahead of each frame (a shell that polls what its link sent).
    pub fn before_each_frame(mut self, before: impl Fn(&mut VisualTestContext) + 'static) -> Self {
        self.before = Some(Box::new(before));
        self
    }

    /// One frame of the window's clock, sampled.
    pub fn frame(&mut self, cx: &mut VisualTestContext) {
        if let Some(before) = &self.before {
            before(cx);
        }
        cx.executor().advance_clock(STEP);
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let began = std::time::Instant::now();
        self.frames.push(sample(cx));
        self.sampling += began.elapsed();
    }

    /// Plays `by` of the window's clock, a frame at a time.
    pub fn play(&mut self, by: Duration, cx: &mut VisualTestContext) {
        let mut gone = Duration::ZERO;
        while gone < by {
            self.frame(cx);
            gone += STEP;
        }
    }

    /// Does what the user does (a click, a change of the view's state), then draws its frame.
    pub fn act(&mut self, cx: &mut VisualTestContext, act: impl FnOnce(&mut VisualTestContext)) {
        act(cx);
        self.frame(cx);
    }

    /// Ends the scenario: plays [`SETTLE`] more, and says whether the window still asks for
    /// frames.
    pub fn finish(mut self, cx: &mut VisualTestContext) -> Recording {
        self.play(SETTLE, cx);
        let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
        Recording {
            scenario: self.scenario,
            frames: self.frames,
            asked_at_end: asked,
            sampling: self.sampling,
        }
    }
}

/// A scenario played: its frames, and the animation frames still asked at its end.
pub struct Recording {
    scenario: &'static str,
    frames: Vec<Frame>,
    asked_at_end: usize,
    sampling: Duration,
}

impl Recording {
    pub fn frames(&self) -> usize {
        self.frames.len()
    }

    /// The time spent reading the elements, all frames together.
    pub fn sampling(&self) -> Duration {
        self.sampling
    }

    /// The most elements one frame held.
    pub fn most_elements(&self) -> usize {
        self.frames.iter().map(HashMap::len).max().unwrap_or(0)
    }
}

/// What a person would notice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Shown at its size in one frame, with nothing moving it in.
    PopIn,
    /// Still in the window but no longer drawn from one frame to the next, with nothing taking
    /// it out.
    PopOut,
    /// Removed from the window while still drawn, before it folded away (or in the middle of
    /// its fold).
    CutExit,
    /// Moved or resized in one frame, with no move before or after.
    Jump,
    /// A jump in the frame where another element popped in, popped out or was cut away.
    SiblingJump,
    /// Still moving, or still asking for frames, once the scenario ended and the window was
    /// played [`SETTLE`] more.
    NeverSettles,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::PopIn => "pop-in",
            Kind::PopOut => "pop-out",
            Kind::CutExit => "cut-exit",
            Kind::Jump => "jump",
            Kind::SiblingJump => "sibling-jump",
            Kind::NeverSettles => "never-settles",
        })
    }
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub kind: Kind,
    /// The element's path of ids.
    pub element: String,
    pub frame: usize,
    pub detail: String,
}

impl Finding {
    /// Whether this finding is about `element` or something inside it: `element` is ids of its
    /// path that follow each other (`project-page`, `rail > rail-agents`), an id ending in `*`
    /// standing for every id it starts (`conversation-empty-*`, whose end is a session's).
    pub fn is_about(&self, element: &str) -> bool {
        let path: Vec<&str> = self.element.split(SEP).collect();
        let wanted: Vec<&str> = element.split(SEP).collect();
        let fits = |id: &str, want: &str| match want.strip_suffix('*') {
            Some(start) => id.starts_with(start),
            None => id == want,
        };
        path.windows(wanted.len())
            .any(|ids| ids.iter().zip(&wanted).all(|(id, want)| fits(id, want)))
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = self.frame as u128 * STEP.as_millis();
        write!(
            f,
            "{} of `{}` at frame {} ({ms} ms): {}",
            self.kind, self.element, self.frame, self.detail
        )
    }
}

/// A finding that is intended: in `scenario`, the `kind` of `element` (its path, or its last
/// ids), because `why`.
#[derive(Clone, Copy, Debug)]
pub struct Allow {
    pub scenario: &'static str,
    pub kind: Kind,
    pub element: &'static str,
    pub why: &'static str,
}

/// What the checker found in a recording.
#[derive(Default, Debug)]
pub struct Verdict {
    /// Findings no entry allows.
    pub findings: Vec<Finding>,
    /// Findings an entry allows, with its reason.
    pub allowed: Vec<(Finding, &'static str)>,
    /// Entries of this scenario that matched nothing.
    pub unused: Vec<Allow>,
}

impl Verdict {
    pub fn has(&self, kind: Kind, element: &str) -> bool {
        self.findings
            .iter()
            .any(|f| f.kind == kind && f.is_about(element))
    }
}

/// Lists what a person would notice in `recording`, less what `allow` says is intended.
pub fn check(recording: &Recording, allow: &[Allow]) -> Verdict {
    let mut found = Gate::new(&recording.frames).findings();
    found.extend(never_settles(recording));
    let mut verdict = Verdict::default();
    let mut used = HashSet::new();
    for finding in found {
        let entry = allow.iter().enumerate().find(|(_, entry)| {
            entry.scenario == recording.scenario
                && entry.kind == finding.kind
                && finding.is_about(entry.element)
        });
        match entry {
            Some((at, entry)) => {
                used.insert(at);
                verdict.allowed.push((finding, entry.why));
            }
            None => verdict.findings.push(finding),
        }
    }
    verdict.unused = allow
        .iter()
        .enumerate()
        .filter(|(at, entry)| entry.scenario == recording.scenario && !used.contains(at))
        .map(|(_, entry)| *entry)
        .collect();
    verdict
}

/// The frames of a recording, read element by element.
struct Gate<'a> {
    frames: &'a [Frame],
    /// The largest area each element was drawn at.
    largest: HashMap<&'a str, f32>,
}

impl<'a> Gate<'a> {
    fn new(frames: &'a [Frame]) -> Self {
        let mut largest: HashMap<&str, f32> = HashMap::new();
        for frame in frames {
            for (key, s) in frame {
                let area = largest.entry(key.as_str()).or_default();
                *area = area.max(s.area());
            }
        }
        Self { frames, largest }
    }

    fn at(&self, key: &str, frame: usize) -> Option<&Sample> {
        self.frames.get(frame).and_then(|f| f.get(key))
    }

    fn shown(&self, key: &str, frame: usize) -> bool {
        self.at(key, frame).is_some_and(Sample::shown)
    }

    /// Whether `key` moved from `frame - 1` to `frame`.
    fn moved(&self, key: &str, frame: usize) -> bool {
        frame >= 1
            && matches!(
                (self.at(key, frame - 1), self.at(key, frame)),
                (Some(a), Some(b)) if a.delta(b) > STILL_PX
            )
    }

    fn moves_within(&self, key: &str, from: usize, to: usize) -> bool {
        (from..=to).any(|frame| self.moved(key, frame))
    }

    /// Shown at `frame` and not the frame before: it arrived (an element drawn at its size at an
    /// opacity of 0 the frame before fades in, which the gate cannot follow further).
    fn arrives(&self, key: &str, frame: usize) -> bool {
        frame >= 1
            && self.shown(key, frame)
            && !self
                .at(key, frame - 1)
                .is_some_and(|s| s.shown() || s.faded())
    }

    /// Shown the frame before `frame` and not at it (an element still at its size at an opacity
    /// of 0 faded out).
    fn leaves(&self, key: &str, frame: usize) -> bool {
        frame >= 1
            && self.shown(key, frame - 1)
            && !self.at(key, frame).is_some_and(|s| s.shown() || s.faded())
    }

    /// Not drawn at `frame`, or not in the window at all.
    fn hidden(&self, key: &str, frame: usize) -> bool {
        !self.at(key, frame).is_some_and(|s| s.shown() || s.faded())
    }

    /// What carries an arrival at `frame` in: the element itself, moving in the frames that
    /// follow; a fold above it that arrived not long before and grows; or what it holds, which
    /// arrives at an opacity of 0 or moves in (a page whose sections enter: the page itself is a
    /// frame the gate cannot tell from a painted block).
    fn carried_in(&self, key: &str, frame: usize) -> bool {
        let after = frame + NEAR;
        self.moves_within(key, frame + 1, after)
            || ancestors(key).any(|above| {
                let from = frame.saturating_sub(FOLD_FRAMES).max(1);
                (from..=frame).any(|came| {
                    self.at(above, came).is_some()
                        && self.hidden(above, came - 1)
                        && self.moves_within(above, came + 1, after)
                })
            })
            || self.frames[frame].iter().any(|(inside, s)| {
                inside.starts_with(key)
                    && inside[key.len()..].starts_with(SEP)
                    && (s.faded() || self.moves_within(inside, frame + 1, after))
            })
    }

    /// What takes an element out at `frame`: itself, moving in the frames before, or a fold
    /// above it that moves and goes not long after. The element that carried it, if any.
    fn carried_out(&self, key: &'a str, frame: usize) -> Option<&'a str> {
        let before = frame.saturating_sub(NEAR).max(1);
        if self.moves_within(key, before, frame - 1) {
            return Some(key);
        }
        let last = (frame + FOLD_FRAMES).min(self.frames.len() - 1);
        ancestors(key).find(|above| {
            (frame..=last)
                .any(|went| self.hidden(above, went) && self.moves_within(above, before, went - 1))
        })
    }

    /// Removed from the window at `frame` while it was still shrinking and drawn at more than a
    /// quarter of its largest size: its fold was cut.
    fn cut(&self, key: &str, frame: usize) -> bool {
        if frame < 2 || self.at(key, frame).is_some() {
            return false;
        }
        let (Some(last), Some(before)) = (self.at(key, frame - 1), self.at(key, frame - 2)) else {
            return false;
        };
        let largest = self.largest.get(key).copied().unwrap_or(0.);
        last.area() < before.area() - STILL_PX && last.area() > largest / 4.
    }

    /// Moved more than [`JUMP_PX`] at `frame`, still before and after.
    fn jumps(&self, key: &str, frame: usize) -> bool {
        frame >= 1
            && self.shown(key, frame - 1)
            && self.shown(key, frame)
            && matches!(
                (self.at(key, frame - 1), self.at(key, frame)),
                (Some(a), Some(b)) if a.delta(b) > JUMP_PX
            )
            && !self.moved(key, frame - 1)
            && !self.moved(key, frame + 1)
    }

    fn findings(&self) -> Vec<Finding> {
        let mut found = Vec::new();
        let mut pops: Vec<(usize, String)> = Vec::new();
        for frame in 1..self.frames.len() {
            let keys: HashSet<&str> = self.frames[frame - 1]
                .keys()
                .chain(self.frames[frame].keys())
                .map(String::as_str)
                .collect();
            let popped_in: HashSet<&str> = keys
                .iter()
                .copied()
                .filter(|key| self.arrives(key, frame) && !self.carried_in(key, frame))
                .collect();
            let popped_out: HashSet<&str> = keys
                .iter()
                .copied()
                .filter(|key| self.leaves(key, frame) && self.carried_out(key, frame).is_none())
                .collect();
            let outermost =
                |key: &str, set: &HashSet<&str>| !ancestors(key).any(|a| set.contains(a));
            for key in &popped_in {
                if outermost(key, &popped_in) {
                    pops.push((frame, key.to_string()));
                    found.push(Finding {
                        kind: Kind::PopIn,
                        element: key.to_string(),
                        frame,
                        detail: "shown at its size in one frame, nothing moved it in".into(),
                    });
                }
            }
            for key in &popped_out {
                if !outermost(key, &popped_out) {
                    continue;
                }
                pops.push((frame, key.to_string()));
                let gone = self.at(key, frame).is_none();
                found.push(Finding {
                    kind: if gone { Kind::CutExit } else { Kind::PopOut },
                    element: key.to_string(),
                    frame,
                    detail: if gone {
                        "removed while drawn at its size, before it folded away".into()
                    } else {
                        "no longer drawn from one frame to the next, nothing took it out".into()
                    },
                });
            }
            for key in &keys {
                if !self.leaves(key, frame) {
                    continue;
                }
                let Some(carrier) = self.carried_out(key, frame) else {
                    continue;
                };
                if self.cut(carrier, frame) && (carrier == *key || !keys.contains(carrier)) {
                    found.push(Finding {
                        kind: Kind::CutExit,
                        element: key.to_string(),
                        frame,
                        detail: format!("removed while `{carrier}` was still folding"),
                    });
                }
            }
        }
        // A jump is a sibling's when an element popped within a frame of it (a removal is seen
        // a frame late: its registration outlives the frame it left).
        for frame in 1..self.frames.len() {
            let jumped: HashSet<&str> = self.frames[frame]
                .keys()
                .map(String::as_str)
                .filter(|key| self.jumps(key, frame))
                .collect();
            for key in &jumped {
                if ancestors(key).any(|a| jumped.contains(a)) {
                    continue;
                }
                let cause = pops
                    .iter()
                    .find(|(at, popped)| at.abs_diff(frame) <= 1 && popped != key);
                found.push(match cause {
                    Some((_, popped)) => Finding {
                        kind: Kind::SiblingJump,
                        element: key.to_string(),
                        frame,
                        detail: format!("moved in one frame as `{popped}` popped"),
                    },
                    None => Finding {
                        kind: Kind::Jump,
                        element: key.to_string(),
                        frame,
                        detail: "moved or resized in one frame, still before and after".into(),
                    },
                });
            }
        }
        found.sort_by_key(|f| f.frame);
        found
    }
}

/// What still moves in the second half of [`SETTLE`], and a window that still asks for frames.
fn never_settles(recording: &Recording) -> Vec<Finding> {
    let gate = Gate::new(&recording.frames);
    let last = recording.frames.len() - 1;
    let half = (SETTLE.as_millis() / STEP.as_millis() / 2) as usize;
    let from = last.saturating_sub(half).max(1);
    let moving: HashSet<&str> = recording.frames[last]
        .keys()
        .map(String::as_str)
        .filter(|key| gate.moves_within(key, from, last))
        .collect();
    let mut found: Vec<Finding> = moving
        .iter()
        .filter(|key| !ancestors(key).any(|a| moving.contains(a)))
        .map(|key| Finding {
            kind: Kind::NeverSettles,
            element: key.to_string(),
            frame: last,
            detail: format!(
                "still moving {} ms after the scenario ended",
                SETTLE.as_millis()
            ),
        })
        .collect();
    if recording.asked_at_end > 0 {
        found.push(Finding {
            kind: Kind::NeverSettles,
            element: "window".into(),
            frame: last,
            detail: format!(
                "still asks for {} animation frames once settled",
                recording.asked_at_end
            ),
        });
    }
    found
}

/// Fails with every finding no entry allows and every entry that matched nothing.
pub fn assert_moves(recording: &Recording, allow: &[Allow]) {
    let verdict = check(recording, allow);
    let mut said = Vec::new();
    for finding in &verdict.findings {
        said.push(format!("  {finding}"));
    }
    for entry in &verdict.unused {
        said.push(format!(
            "  an allow entry matches nothing: {} of `{}` ({})",
            entry.kind, entry.element, entry.why
        ));
    }
    assert!(
        said.is_empty(),
        "the motion gate, scenario `{}` ({} frames, at most {} elements):\n{}",
        recording.scenario,
        recording.frames(),
        recording.most_elements(),
        said.join("\n")
    );
}

/// The ids above `path`, nearest first.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.rmatch_indices(SEP).map(move |(at, _)| &path[..at])
}
