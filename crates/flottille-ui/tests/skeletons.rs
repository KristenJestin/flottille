//! The skeleton: a block of the size of what it stands for, pulsing while it waits, and the
//! crossfade in place to the real content, with the height change animated.

mod common;

use std::time::Duration;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::PANEL;
use flottille_ui::skeleton::blocks_drawn;
use flottille_ui::{Skeleton, awaiting};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement, Pixels, Render, Styled,
    TestAppContext, TestSupportExt as _, VisualTestContext, Window, div, px,
};

struct Bare;

impl Render for Bare {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Skeleton::new("block").w(px(120.)).h(px(14.)))
    }
}

fn height(id: &str, cx: &mut VisualTestContext) -> Option<Pixels> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .map(|f| f.bounds().size.height)
    })
}

fn top(id: &str, cx: &mut VisualTestContext) -> Pixels {
    cx.update(|window, _| window.find(id.to_owned()).bounds().origin.y)
}

#[gpui_kit::test]
fn a_skeleton_is_a_block_of_the_size_of_the_line_it_stands_for(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Bare);
    cx.update(|window, cx| window.render_frame(cx));
    let bounds = cx.update(|window, _| window.find("block".to_owned()).bounds());
    assert_eq!(bounds.size.width, px(120.));
    assert_eq!(bounds.size.height, px(14.));
}

#[gpui_kit::test]
fn a_skeleton_keeps_pulsing_while_it_is_drawn(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Bare);
    cx.update(|window, cx| window.render_frame(cx));
    let first = blocks_drawn();
    advance(cx, Duration::from_millis(400));
    let drawn = blocks_drawn() - first;
    assert!(
        (3..=8).contains(&drawn),
        "a waiting block is drawn again, slowly, for as long as it waits: {drawn} times in 400 ms"
    );
}

#[gpui_kit::test]
fn a_skeleton_is_drawn_at_the_pace_of_a_slow_pulse_and_no_faster(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Bare);
    cx.update(|window, cx| window.render_frame(cx));
    let first = blocks_drawn();
    advance(cx, Duration::from_millis(1000));
    let drawn = blocks_drawn() - first;
    assert!(
        (8..=11).contains(&drawn),
        "a pulse of nearly two seconds needs ten draws a second, not more: {drawn}"
    );
}

#[gpui_kit::test]
fn with_reduced_motion_a_skeleton_is_still(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_, cx) = cx.add_window_view(|_, _| Bare);
    cx.update(|window, cx| window.render_frame(cx));
    let first = blocks_drawn();
    advance(cx, Duration::from_millis(400));
    assert_eq!(blocks_drawn(), first, "it stays as it was drawn");
    assert_eq!(pending_frames(cx), 0);
}

struct Waiting {
    pending: bool,
    content: f32,
}

impl Render for Waiting {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().id("before").h(px(20.)).test_support())
            .child(awaiting(
                "part",
                self.pending,
                Skeleton::new("skeleton").h(px(14.)),
                div().id("content").h(px(self.content)).test_support(),
            ))
            .child(div().id("after").h(px(20.)).test_support())
    }
}

fn waiting(
    pending: bool,
    content: f32,
    cx: &mut TestAppContext,
) -> (gpui_kit::Entity<Waiting>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(move |_, _| Waiting { pending, content });
    cx.update(|window, cx| window.render_frame(cx));
    (view, cx)
}

fn ready(view: &gpui_kit::Entity<Waiting>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.pending = false;
            cx.notify()
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
}

#[gpui_kit::test]
fn the_pulse_stops_once_nothing_waits(cx: &mut TestAppContext) {
    let (view, cx) = waiting(true, 40., cx);
    advance(cx, Duration::from_millis(300));
    ready(&view, cx);
    advance(cx, PANEL * 2);
    let drawn = blocks_drawn();
    advance(cx, Duration::from_millis(1000));
    assert_eq!(
        blocks_drawn(),
        drawn,
        "no block is drawn again once no skeleton is"
    );
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_pending_part_shows_its_skeleton_and_not_its_content(cx: &mut TestAppContext) {
    let (_, cx) = waiting(true, 40., cx);
    assert_eq!(height("skeleton", cx), Some(px(14.)));
    assert!(
        height("content", cx).is_none(),
        "nothing of the content yet"
    );
    assert_eq!(top("after", cx), px(20. + 14.));
}

#[gpui_kit::test]
fn the_skeleton_fades_out_and_the_content_in_at_the_same_place_while_the_height_eases(
    cx: &mut TestAppContext,
) {
    let (view, cx) = waiting(true, 40., cx);
    ready(&view, cx);
    advance(cx, PANEL / 3);
    assert!(
        height("skeleton", cx).is_some() && height("content", cx).is_some(),
        "both are drawn while one fades into the other"
    );
    assert_eq!(
        top("skeleton", cx),
        top("content", cx),
        "the content comes where the skeleton was"
    );
    let part = top("after", cx);
    assert!(
        px(34.) < part && part < px(60.),
        "what follows moves part way to the content's height: {part:?}"
    );
    advance(cx, PANEL * 2);
    assert!(height("skeleton", cx).is_none(), "the skeleton is gone");
    assert_eq!(height("content", cx), Some(px(40.)));
    assert_eq!(top("after", cx), px(60.));
    assert_eq!(pending_frames(cx), 0, "nothing is drawn once it arrived");
}

#[gpui_kit::test]
fn a_part_that_turns_out_to_have_nothing_folds_its_skeleton_away(cx: &mut TestAppContext) {
    let (view, cx) = waiting(true, 0., cx);
    ready(&view, cx);
    advance(cx, PANEL / 3);
    let part = top("after", cx);
    assert!(px(20.) < part && part < px(34.), "folding: {part:?}");
    advance(cx, PANEL * 2);
    assert_eq!(
        top("after", cx),
        px(20.),
        "no space left for what never came"
    );
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_part_known_from_its_first_frame_is_just_there(cx: &mut TestAppContext) {
    let (_, cx) = waiting(false, 40., cx);
    assert!(height("skeleton", cx).is_none());
    assert_eq!(height("content", cx), Some(px(40.)));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn with_reduced_motion_the_content_replaces_its_skeleton_at_once(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|_, _| Waiting {
        pending: true,
        content: 40.,
    });
    cx.update(|window, cx| window.render_frame(cx));
    ready(&view, cx);
    advance(cx, Duration::from_millis(32));
    assert!(height("skeleton", cx).is_none());
    assert_eq!(top("after", cx), px(60.));
}
