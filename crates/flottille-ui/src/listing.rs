//! A list whose rows come, go and move: [`Listing`] remembers the rows a view showed, so that one
//! that went is still drawn while it folds away and one that moved slides to its new place;
//! [`Item`] is one row's frame.
//!
//! The view calls [`Listing::sync`] with what the daemon last said, on each render, and draws
//! [`Listing::rows`], each inside an [`Item`]. Rows are told apart by a key that stays the same
//! while a row is the same row.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{AnyElement, App, ElementId, IntoElement, Pixels, RenderOnce, Window};

use crate::motion::{MAX_STAGGER, Motion, PANEL, STAGGER};
use crate::reveal::{Shown, drive, shown_frame};

/// Where a row is in its coming and going: what [`Item`] needs to animate it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    gone: bool,
    stagger: usize,
    /// How many rows it moved (positive: up) and the move it belongs to.
    shift: Option<(isize, u64)>,
}

impl Placement {
    /// A row that has left and is folding away.
    pub fn is_gone(&self) -> bool {
        self.gone
    }
}

struct Slot<T> {
    key: String,
    value: T,
    gone: Option<Instant>,
    placement: Placement,
}

/// A row to draw.
pub struct Row<'a, T> {
    pub key: &'a str,
    pub value: &'a T,
    pub placement: Placement,
}

pub struct Listing<T> {
    slots: Vec<Slot<T>>,
    started: bool,
    moves: u64,
}

impl<T> Default for Listing<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            started: false,
            moves: 0,
        }
    }
}

impl<T> Listing<T> {
    /// The rows now, in order: the ones that left sit where they were, folding away.
    pub fn rows(&self) -> impl Iterator<Item = Row<'_, T>> {
        self.slots.iter().map(|slot| Row {
            key: &slot.key,
            value: &slot.value,
            placement: slot.placement,
        })
    }

    /// The rows that are not leaving.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.gone.is_none()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Takes the list as it is now. The first call sets the rows the screen opens with, which
    /// enter one after another; later, a new row unfolds at once, a missing one folds away and a
    /// row that changed place slides there. Keys are unique.
    pub fn sync(
        &mut self,
        items: impl IntoIterator<Item = (String, T)>,
        now: Instant,
        reduce: bool,
    ) {
        let first = !self.started;
        self.started = true;
        let mut old = std::mem::take(&mut self.slots);
        old.retain(|slot| {
            slot.gone
                .is_none_or(|at| !reduce && now.saturating_duration_since(at) < PANEL)
        });
        let items: Vec<(String, T)> = items.into_iter().collect();
        // A row moved when its place among the rows that stay changed: a row above it that came
        // or went is the fold's business, not a slide.
        let staying: HashSet<&str> = items.iter().map(|(key, _)| key.as_str()).collect();
        let was: Vec<&str> = old
            .iter()
            .filter(|s| s.gone.is_none() && staying.contains(s.key.as_str()))
            .map(|s| s.key.as_str())
            .collect();
        let now_order: Vec<&str> = items
            .iter()
            .map(|(key, _)| key.as_str())
            .filter(|key| was.contains(key))
            .collect();
        let live_at: HashMap<String, isize> = was
            .iter()
            .enumerate()
            .map(|(at, key)| {
                let to = now_order.iter().position(|k| k == key).unwrap_or(at);
                (key.to_string(), at as isize - to as isize)
            })
            .collect();
        let mut by_key: HashMap<String, Slot<T>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for slot in old {
            order.push(slot.key.clone());
            by_key.insert(slot.key.clone(), slot);
        }

        self.moves += 1;
        let stamp = self.moves;
        let mut placed: Vec<Slot<T>> = Vec::new();
        let mut claimed: HashSet<String> = HashSet::new();
        for (at, (key, value)) in items.into_iter().enumerate() {
            let slot = match by_key.remove(&key) {
                Some(mut slot) => {
                    claimed.insert(key.clone());
                    slot.value = value;
                    slot.placement.shift = live_at
                        .get(&key)
                        .copied()
                        .filter(|by| *by != 0)
                        .map(|by| (by, stamp));
                    if slot.gone.take().is_some() {
                        slot.placement.gone = false;
                        slot.placement.shift = None;
                    }
                    slot
                }
                None => Slot {
                    key,
                    value,
                    gone: None,
                    placement: Placement {
                        gone: false,
                        stagger: if first { at } else { 0 },
                        shift: None,
                    },
                },
            };
            placed.push(slot);
        }

        // What left stays where it was: after the nearest row before it that stayed.
        let mut after: HashMap<Option<String>, Vec<Slot<T>>> = HashMap::new();
        let mut before: Option<String> = None;
        for key in order {
            if claimed.contains(&key) {
                before = Some(key);
                continue;
            }
            let Some(mut slot) = by_key.remove(&key) else {
                continue;
            };
            if slot.gone.is_none() && !reduce {
                slot.gone = Some(now);
                slot.placement.gone = true;
                slot.placement.shift = None;
            } else if slot.gone.is_none() {
                continue;
            }
            after.entry(before.clone()).or_default().push(slot);
        }
        let mut slots = after.remove(&None).unwrap_or_default();
        for slot in placed {
            let key = Some(slot.key.clone());
            slots.push(slot);
            slots.extend(after.remove(&key).unwrap_or_default());
        }
        self.slots = slots;
    }
}

/// One row of a [`Listing`]: it unfolds and fades in when it arrives, folds away when it leaves
/// and slides when it moves, over [`PANEL`].
#[derive(IntoElement)]
pub struct Item {
    id: ElementId,
    placement: Placement,
    gap: f32,
    rising: bool,
    content: AnyElement,
}

impl Item {
    pub fn new(id: impl Into<ElementId>, placement: Placement, content: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            placement,
            gap: 0.0,
            rising: false,
            content: content.into_any_element(),
        }
    }

    /// A row that arrives rises into place and fades in, at its full size from its first frame (a
    /// row of the tree, which is clicked as it comes), instead of unfolding. It still folds away.
    pub fn rising(mut self) -> Self {
        self.rising = true;
        self
    }

    /// The gap the parent flex puts between its rows, which folds with the row.
    pub fn gap(mut self, gap: Pixels) -> Self {
        self.gap = f32::from(gap);
        self
    }
}

impl RenderOnce for Item {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let key = ElementId::NamedChild(Arc::new(self.id.clone()), "item".into());
        let delay = STAGGER * self.placement.stagger.min(MAX_STAGGER) as u32;
        let state = window.use_keyed_state(key, cx, move |_, cx| {
            let mut motion = Motion::settled(0.0);
            let now = cx.background_executor().now();
            motion.play(0.0, 1.0, delay, PANEL, now, cx.reduce_motion());
            Shown::new(motion)
        });
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        if let Some((by, stamp)) = self.placement.shift {
            state.update(cx, |shown, _| {
                if shown.moved != stamp {
                    shown.moved = stamp;
                    shown.slide.play(
                        by as f32,
                        0.0,
                        std::time::Duration::ZERO,
                        PANEL,
                        now,
                        reduce,
                    );
                }
            });
        }
        let at = drive(&state, !self.placement.gone, window, cx);
        let slide = state.read(cx).slide;
        if slide.is_running(now) {
            window.request_animation_frame();
        }
        let offset = (f32::from(at.height.get()) + self.gap) * slide.value(now);
        shown_frame(
            self.id,
            !self.placement.gone,
            self.rising,
            &at,
            self.gap,
            offset,
            self.content,
        )
    }
}
