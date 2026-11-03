//! The motion grammar, on GPUI's test platform: "a fold animates and then stops", "with reduced
//! motion, values jump to their target".

mod common;

use std::time::Duration;

use common::{Desktop, advance, count_redraws, init, pending_frames};
use flottille_ui::motion::PANEL;
use flottille_ui::theme::{palette, set_palette};
use flottille_ui::{AgentState, Fold, Palette, Pill, Status};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Pixels, Render, Styled,
    TestAppContext, VisualTestContext, Window, div, px,
};

struct Content;

impl Render for Content {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w_full().h(px(120.))
    }
}

struct Page {
    fold: Entity<Fold>,
    pill: Entity<Pill>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.fold.clone())
            .child(self.pill.clone())
    }
}

fn page<'a>(
    status: &Entity<Status>,
    cx: &'a mut TestAppContext,
) -> (Entity<Page>, &'a mut VisualTestContext) {
    let status = status.clone();
    cx.add_window_view(move |_, cx| {
        let content = cx.new(|_| Content);
        Page {
            fold: cx.new(|_| Fold::new("fold", content, true)),
            pill: cx.new(|cx| Pill::new(status, cx)),
        }
    })
}

fn fold_height(cx: &mut VisualTestContext) -> Pixels {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find("fold").bounds().size.height
    })
}

#[gpui_kit::test]
fn a_fold_animates_and_then_stops(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let status = cx.new(|_| Status::new(AgentState::Completed));
    let (page, cx) = page(&status, cx);
    let fold = page.read_with(cx, |p, _| p.fold.clone());
    assert_eq!(fold_height(cx), px(120.));
    let redraws = count_redraws(&fold, cx);

    cx.update(|_, cx| fold.update(cx, |f, cx| f.set_open(false, cx)));
    advance(cx, PANEL / 3);
    let folding = fold_height(cx);
    assert!(
        px(0.) < folding && folding < px(120.),
        "part way through, the fold is part way: {folding:?}"
    );

    advance(cx, PANEL);
    assert_eq!(fold_height(cx), px(0.));
    let at_rest = redraws.get();
    assert!(at_rest > 3, "the fold drew its frames: {at_rest}");
    advance(cx, Duration::from_secs(3));
    assert_eq!(
        redraws.get(),
        at_rest,
        "the fold is redrawn after it stopped"
    );
    assert_eq!(pending_frames(cx), 0);

    // And it unfolds again to the content's height.
    cx.update(|_, cx| fold.update(cx, |f, cx| f.set_open(true, cx)));
    advance(cx, PANEL * 2);
    assert_eq!(fold_height(cx), px(120.));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn with_reduced_motion_values_jump_to_their_target(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    let status = cx.new(|_| Status::new(AgentState::Running));
    let (page, cx) = page(&status, cx);
    let (fold, pill) = page.read_with(cx, |p, _| (p.fold.clone(), p.pill.clone()));
    assert_eq!(fold_height(cx), px(120.));

    // A fold: closed at once, no frame asked for.
    cx.update(|_, cx| fold.update(cx, |f, cx| f.set_open(false, cx)));
    cx.run_until_parked();
    assert_eq!(fold_height(cx), px(0.));
    assert_eq!(pending_frames(cx), 0);

    // A state change: the new pill is there at full size at once.
    let pill_redraws = count_redraws(&pill, cx);
    cx.update(|_, cx| status.update(cx, |s, cx| s.set(AgentState::Completed, cx)));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    let full = cx.update(|w, _| {
        w.find(flottille_ui::state::pill_id(AgentState::Completed))
            .bounds()
    });
    assert!(
        full.size.height >= px(17.),
        "the pill is at full size: {full:?}"
    );
    assert_eq!(pending_frames(cx), 0);
    let after_change = pill_redraws.get();
    advance(cx, Duration::from_secs(1));
    assert_eq!(pill_redraws.get(), after_change);

    // A theme change: the palette is the new one at once, no cross-fade.
    let mut other = Palette::base();
    other.acc = flottille_ui::color::hex(0xf0883e);
    cx.update(|_, cx| set_palette(other, cx));
    assert_eq!(cx.update(|_, cx| palette(cx)), other);
}
