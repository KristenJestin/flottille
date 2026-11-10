//! A chip is never wider than the place it sits in (#731), on GPUI's test platform: a chip with a
//! long description wraps its text inside itself and grows in height, a short one is unchanged.

mod common;

use common::{Desktop, init};
use flottille_ui::{Chip, Icon};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    TestAppContext, TestSupportExt, VisualTestContext, Window, div, px, size,
};

const LONG: &str = "· a description as long as a paragraph, the way a writer explains one when it explains what each option does and what follows from it";

/// A card 300 px wide holding the row of chips the way a question card does.
struct Options(fn() -> Vec<Chip>);

impl Render for Options {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("card").w(px(300.)).test_support().child(
            div()
                .id("options")
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .children((self.0)())
                .test_support(),
        )
    }
}

fn open(chips: fn() -> Vec<Chip>, cx: &mut TestAppContext) -> &mut VisualTestContext {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(move |_, _| Options(chips));
    cx.simulate_resize(size(px(800.), px(600.)));
    cx.update(|window, cx| window.render_frame(cx));
    cx
}

fn bounds(cx: &mut VisualTestContext, id: &'static str) -> Bounds<Pixels> {
    cx.update(|window, _| window.find(id).bounds())
}

#[gpui_kit::test]
fn a_long_option_wraps_inside_its_chip_and_stays_in_the_card(cx: &mut TestAppContext) {
    let cx = open(
        || vec![Chip::choice("option", "Keep both", false).faint(LONG)],
        cx,
    );
    let (card, chip) = (bounds(cx, "card"), bounds(cx, "option"));
    assert!(chip.left() >= card.left(), "{chip:?} in {card:?}");
    assert!(chip.right() <= card.right(), "{chip:?} in {card:?}");
    assert!(chip.size.height > px(24.), "the text wraps: {chip:?}");
}

#[gpui_kit::test]
fn a_short_option_s_chip_is_unchanged(cx: &mut TestAppContext) {
    let cx = open(|| vec![Chip::choice("option", "Keep both", false)], cx);
    let chip = bounds(cx, "option");
    assert_eq!(chip.size.height, px(24.), "{chip:?}");
    assert!(chip.size.width < px(120.), "as wide as its text: {chip:?}");
}

#[gpui_kit::test]
fn a_short_description_stays_on_one_line(cx: &mut TestAppContext) {
    let cx = open(
        || vec![Chip::choice("option", "Keep both", false).faint("· safest")],
        cx,
    );
    let chip = bounds(cx, "option");
    assert_eq!(chip.size.height, px(24.), "{chip:?}");
}

#[gpui_kit::test]
fn a_long_word_without_a_space_stays_inside_the_card(cx: &mut TestAppContext) {
    let cx = open(
        || {
            vec![Chip::new(
                "chip",
                Icon::Repository,
                "a-branch-name-that-has-no-space-and-is-longer-than-the-card-that-holds-it-by-far",
            )]
        },
        cx,
    );
    let (card, chip) = (bounds(cx, "card"), bounds(cx, "chip"));
    assert!(chip.right() <= card.right(), "{chip:?} in {card:?}");
}
