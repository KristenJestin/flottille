//! A text cut with an ellipsis says itself whole in a tooltip (#428), and only then: a text that
//! fits has none. On GPUI's test platform, where the text system measures every glyph the same.

mod common;

use common::{Desktop, advance, init};
use flottille_ui::motion::{HOVER, TOOLTIP};
use flottille_ui::{ListRow, TreeLevel, TreeRow};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
    TestSupportExt, VisualTestContext, Window, div, px, size,
};

const LONG: &str = "Fix the recipients sheet so that a very long title never leaves its row";
const SHORT: &str = "Fix it";
const SENTENCE: &str =
    "Runs the whole test suite of the workspace, then the linter, then the formatter check";

/// A list row and a rail row, 260 px wide each.
struct Rows(&'static str);

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("frame")
            .w(px(260.))
            .flex()
            .flex_col()
            .test_support()
            .child(ListRow::new("list-row", self.0).line(SENTENCE))
            .child(TreeRow::new("rail-row", TreeLevel::Session, self.0))
    }
}

fn open<'a>(title: &'static str, cx: &'a mut TestAppContext) -> &'a mut VisualTestContext {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(move |_, _| Rows(title));
    cx.simulate_resize(size(px(800.), px(400.)));
    cx.update(|window, cx| window.render_frame(cx));
    cx
}

/// The pointer rests on `id` until its tooltip has been shown.
fn rest_on(id: &'static str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.hover(id, cx));
    advance(cx, TOOLTIP + HOVER * 2);
    cx.update(|window, cx| window.render_frame(cx));
}

#[gpui_kit::test]
fn a_cut_row_shows_its_whole_text_in_a_tooltip(cx: &mut TestAppContext) {
    let cx = open(LONG, cx);
    for (text, whole) in [
        ("list-row-title", LONG),
        ("list-row-line", SENTENCE),
        ("rail-row-title", LONG),
    ] {
        rest_on(text, cx);
        cx.update(|window, _| {
            let tip = window.find(format!("{text}-tooltip"));
            assert_eq!(tip.label(), Some(whole), "{text}");
            assert!(tip.bounds().right() <= px(800.), "inside the window");
        });
    }
}

#[gpui_kit::test]
fn a_text_that_fits_has_no_tooltip(cx: &mut TestAppContext) {
    let cx = open(SHORT, cx);
    for text in ["list-row-title", "rail-row-title"] {
        rest_on(text, cx);
        cx.update(|window, _| {
            assert!(
                window.try_find(format!("{text}-tooltip")).is_none(),
                "{text}"
            );
        });
    }
}
