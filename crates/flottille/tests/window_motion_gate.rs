//! The motion gate (#528) on GPUI's test platform: samples of wrong code it must find (a block
//! shown without `presence`, a row removed without an `Item`, a sibling moved in one frame, a
//! block hidden at once, a motion that never ends), the kit's helpers it must pass, and pages of
//! the window, in front of a real daemon whose agents are the fake ACP agent. No window opens on
//! a screen.

use crate::kit_common;

use std::time::{Duration, Instant};

use crate::motion_gate::{Allow, Kind, Recorder, Recording, assert_moves, check};
use gpui_design_system::motion::PANEL;
use gpui_design_system::{Button, Item, Listing, enter, presence};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render, Styled,
    TestAppContext, TestSupportExt as _, VisualTestContext, Window, div, px,
};
use kit_common::{Desktop, init};

/// Draws the view's first frame and lets whatever it started end.
fn shown<V: Render>(
    build: impl FnOnce() -> V + 'static,
    cx: &mut TestAppContext,
) -> (Entity<V>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(move |_, _| build());
    let mut warm = Recorder::start("warm-up", cx);
    warm.play(PANEL * 3, cx);
    (view, cx)
}

/// Changes the view as its owner would, and plays the frames that follow.
fn record<V: 'static>(
    scenario: &'static str,
    view: &Entity<V>,
    change: impl FnOnce(&mut V),
    cx: &mut VisualTestContext,
) -> Recording {
    let mut recorder = Recorder::start(scenario, cx);
    recorder.act(cx, |cx| {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                change(view);
                cx.notify();
            })
        })
    });
    recorder.play(PANEL * 2, cx);
    recorder.finish(cx)
}

fn row(id: &'static str) -> impl IntoElement {
    div().id(id).h(px(40.)).w_full().test_support()
}

// ── A block that comes ─────────────────────────────────────────────────────────────────────────

/// A row, a block that comes and goes, and a row below it unless `last`.
struct Block {
    shown: bool,
    with_presence: bool,
    last: bool,
}

impl Render for Block {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let block = if self.with_presence {
            presence("block", self.shown, div().h(px(40.)).w_full())
                .gap(px(8.))
                .into_any_element()
        } else if self.shown {
            row("block").into_any_element()
        } else {
            div().into_any_element()
        };
        div()
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(row("above"))
            .child(block)
            .when(!self.last, |d| d.child(row("below")))
    }
}

fn block(with_presence: bool, last: bool) -> impl FnOnce() -> Block {
    move || Block {
        shown: false,
        with_presence,
        last,
    }
}

#[gpui_kit::test]
fn a_block_shown_without_presence_is_found_as_a_pop_in(cx: &mut TestAppContext) {
    let (view, cx) = shown(block(false, true), cx);
    let recording = record("block", &view, |b| b.shown = true, cx);
    let verdict = check(&recording, &[]);
    assert!(verdict.has(Kind::PopIn, "block"), "{verdict:#?}");
}

#[gpui_kit::test]
fn a_block_shown_and_hidden_with_presence_passes(cx: &mut TestAppContext) {
    let (view, cx) = shown(block(true, false), cx);
    assert_moves(&record("show", &view, |b| b.shown = true, cx), &[]);
    assert_moves(&record("hide", &view, |b| b.shown = false, cx), &[]);
}

#[gpui_kit::test]
fn a_row_moved_by_a_sibling_that_popped_in_is_found_as_a_sibling_jump(cx: &mut TestAppContext) {
    let (view, cx) = shown(block(false, false), cx);
    let recording = record("block", &view, |b| b.shown = true, cx);
    let verdict = check(&recording, &[]);
    assert!(verdict.has(Kind::PopIn, "block"), "{verdict:#?}");
    assert!(verdict.has(Kind::SiblingJump, "below"), "{verdict:#?}");
}

#[gpui_kit::test]
fn an_allowed_finding_passes_with_its_reason(cx: &mut TestAppContext) {
    let (view, cx) = shown(block(false, true), cx);
    let recording = record("block", &view, |b| b.shown = true, cx);
    let allow = [Allow {
        scenario: "block",
        kind: Kind::PopIn,
        element: "block",
        why: "a sample: the block is meant to be there at once",
    }];
    let verdict = check(&recording, &allow);
    assert!(verdict.findings.is_empty(), "{verdict:#?}");
    assert!(verdict.unused.is_empty(), "{verdict:#?}");
    assert_eq!(verdict.allowed.len(), 1, "{verdict:#?}");
    assert_eq!(
        verdict.allowed[0].1,
        "a sample: the block is meant to be there at once"
    );
    assert_moves(&recording, &allow);
}

#[gpui_kit::test]
fn an_allow_entry_that_matches_nothing_is_reported_as_unused(cx: &mut TestAppContext) {
    let (view, cx) = shown(block(true, true), cx);
    let recording = record("block", &view, |b| b.shown = true, cx);
    let allow = [
        Allow {
            scenario: "block",
            kind: Kind::PopIn,
            element: "block",
            why: "it once popped in",
        },
        Allow {
            scenario: "another scenario",
            kind: Kind::PopIn,
            element: "block",
            why: "not this scenario's: not reported here",
        },
    ];
    let verdict = check(&recording, &allow);
    assert!(verdict.findings.is_empty(), "{verdict:#?}");
    assert_eq!(verdict.unused.len(), 1, "{verdict:#?}");
    assert_eq!(verdict.unused[0].why, "it once popped in");
}

// ── A block that goes at once ──────────────────────────────────────────────────────────────────

/// A block that loses its height in one frame while it stays in the window.
struct Flat {
    open: bool,
}

impl Render for Flat {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let height = if self.open { 40. } else { 0. };
        div()
            .w(px(300.))
            .flex()
            .flex_col()
            .child(
                div()
                    .id("block")
                    .w_full()
                    .h(px(height))
                    .overflow_hidden()
                    .test_support(),
            )
            .child(row("below"))
    }
}

#[gpui_kit::test]
fn a_block_hidden_at_once_is_found_as_a_pop_out(cx: &mut TestAppContext) {
    let (view, cx) = shown(|| Flat { open: true }, cx);
    let recording = record("flat", &view, |f| f.open = false, cx);
    let verdict = check(&recording, &[]);
    assert!(verdict.has(Kind::PopOut, "block"), "{verdict:#?}");
    assert!(verdict.has(Kind::SiblingJump, "below"), "{verdict:#?}");
}

// ── Rows of a list ─────────────────────────────────────────────────────────────────────────────

struct Rows {
    items: Vec<&'static str>,
    list: Listing<&'static str>,
    with_items: bool,
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let column = div().w(px(300.)).flex().flex_col().gap(px(8.));
        if !self.with_items {
            return column.children(self.items.iter().map(|key| {
                div()
                    .id(format!("row-{key}"))
                    .h(px(40.))
                    .w_full()
                    .test_support()
            }));
        }
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        self.list
            .sync(self.items.iter().map(|k| (k.to_string(), *k)), now, reduce);
        column.children(self.list.rows().map(|row| {
            Item::new(
                format!("row-{}", row.key),
                row.placement,
                div().h(px(40.)).w_full(),
            )
            .gap(px(8.))
        }))
    }
}

fn rows(with_items: bool) -> impl FnOnce() -> Rows {
    move || Rows {
        items: vec!["a", "b", "c"],
        list: Listing::default(),
        with_items,
    }
}

#[gpui_kit::test]
fn a_row_removed_without_an_item_is_found_as_a_cut_exit(cx: &mut TestAppContext) {
    let (view, cx) = shown(rows(false), cx);
    let recording = record("rows", &view, |r| r.items = vec!["a", "c"], cx);
    let verdict = check(&recording, &[]);
    assert!(verdict.has(Kind::CutExit, "row-b"), "{verdict:#?}");
    assert!(verdict.has(Kind::SiblingJump, "row-c"), "{verdict:#?}");
}

#[gpui_kit::test]
fn rows_that_arrive_go_and_move_as_items_pass(cx: &mut TestAppContext) {
    let (view, cx) = shown(rows(true), cx);
    assert_moves(
        &record("arrive", &view, |r| r.items = vec!["a", "b", "c", "d"], cx),
        &[],
    );
    assert_moves(
        &record("go", &view, |r| r.items = vec!["a", "c", "d"], cx),
        &[],
    );
    assert_moves(
        &record("move", &view, |r| r.items = vec!["d", "a", "c"], cx),
        &[],
    );
}

// ── A sibling that moves in one frame ──────────────────────────────────────────────────────────

/// A row under a space that takes its height in one frame: the space has no id, the row is all
/// the gate sees.
struct Spaced {
    tall: bool,
}

impl Render for Spaced {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let height = if self.tall { 40. } else { 0. };
        div()
            .w(px(300.))
            .flex()
            .flex_col()
            .child(div().h(px(height)))
            .child(row("row"))
    }
}

#[gpui_kit::test]
fn a_sibling_moved_in_one_frame_is_found_as_a_jump(cx: &mut TestAppContext) {
    let (view, cx) = shown(|| Spaced { tall: false }, cx);
    let recording = record("spaced", &view, |s| s.tall = true, cx);
    let verdict = check(&recording, &[]);
    assert!(verdict.has(Kind::Jump, "row"), "{verdict:#?}");
}

// ── A page that enters ─────────────────────────────────────────────────────────────────────────

/// A page whose blocks enter one after another, buttons in them.
struct Page {
    open: bool,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let page = div().w(px(400.)).flex().flex_col().gap(px(12.));
        if !self.open {
            return page;
        }
        page.children((0..4).map(|at| {
            enter(
                format!("section-{at}"),
                at,
                div()
                    .h(px(60.))
                    .child(Button::secondary(format!("action-{at}"), "Open")),
            )
        }))
    }
}

#[gpui_kit::test]
fn a_page_whose_sections_enter_staggered_passes(cx: &mut TestAppContext) {
    let (view, cx) = shown(|| Page { open: false }, cx);
    assert_moves(&record("page", &view, |p| p.open = true, cx), &[]);
}

// ── A motion that never ends ───────────────────────────────────────────────────────────────────

/// An element that drifts for ever, a little each frame.
struct Drift {
    since: Instant,
}

impl Render for Drift {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        let ms = cx
            .background_executor()
            .now()
            .saturating_duration_since(self.since)
            .as_millis();
        div().child(
            div()
                .id("drift")
                .relative()
                .top(px((ms % 2000) as f32 / 200.))
                .h(px(40.))
                .w(px(40.))
                .test_support(),
        )
    }
}

#[gpui_kit::test]
fn a_block_that_moves_for_ever_is_found_as_never_settling(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, cx| Drift {
        since: cx.background_executor().now(),
    });
    let mut recorder = Recorder::start("drift", cx);
    recorder.play(Duration::from_millis(100), cx);
    let verdict = check(&recorder.finish(cx), &[]);
    assert!(verdict.has(Kind::NeverSettles, "drift"), "{verdict:#?}");
}
