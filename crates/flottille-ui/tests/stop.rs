//! The stop glyph on GPUI's test platform: a small filled square drawn centred in its button.

mod common;

use common::{Desktop, init};
use flottille_ui::{Button, Icon};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div, px, size,
};

struct Page;

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .gap(px(20.))
            .child(Button::icon("run-stop", Icon::Stop, "Stop"))
            .child(Button::round("reply-stop", Icon::Stop, "Stop"))
            .child(Button::icon("run-ended", Icon::Stopped, "Ended"))
            .child(Button::secondary("drawer-stop", "Stop").leading(Icon::Stop))
    }
}

fn page(cx: &mut TestAppContext) -> &mut gpui_kit::VisualTestContext {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Page);
    cx.simulate_resize(size(px(600.), px(200.)));
    cx.update(|window, cx| window.render_frame(cx));
    cx
}

#[gpui_kit::test]
fn the_stop_buttons_draw_the_stop_glyph_centred(cx: &mut TestAppContext) {
    let cx = page(cx);
    cx.update(|window, _| {
        for button in ["run-stop", "reply-stop"] {
            let outer = window.find(button).bounds();
            let inner = window.within(button).find(Icon::Stop.glyph_id()).bounds();
            assert!(
                outer.contains(&inner.center()),
                "{button}: inside its button"
            );
            let off = (inner.center() - outer.center()).map(|d| d.abs());
            assert!(
                off.x <= px(1.) && off.y <= px(1.),
                "{button}: centred, {off:?} off"
            );
        }
    });
}

#[gpui_kit::test]
fn the_stop_square_is_sized_to_its_button(cx: &mut TestAppContext) {
    let cx = page(cx);
    cx.update(|window, _| {
        // The asset draws a square of 45 % of the glyph box.
        let mut square = |button: &'static str| {
            let drawn = window.within(button).find(Icon::Stop.glyph_id()).bounds();
            drawn.size.width * 0.45
        };
        let icon = square("run-stop");
        let round = square("reply-stop");
        assert!(icon >= px(8.) && icon <= px(12.), "run row: {icon:?}");
        assert!(round >= px(8.) && round <= px(12.), "composer: {round:?}");
    });
}

#[gpui_kit::test]
fn the_ended_mark_is_not_the_stop_glyph(cx: &mut TestAppContext) {
    let cx = page(cx);
    cx.update(|window, _| {
        assert_ne!(Icon::Stop.glyph_id(), Icon::Stopped.glyph_id());
        assert!(
            window
                .within("run-ended")
                .try_find(Icon::Stopped.glyph_id())
                .is_some()
        );
        assert!(
            window
                .within("run-ended")
                .try_find(Icon::Stop.glyph_id())
                .is_none()
        );
    });
}
