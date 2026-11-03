//! The text cursor over a selectable text: one region, so that the pointer crossing the gaps
//! between its paragraphs never leaves it (and the I-beam never toggles with the arrow).

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Desktop, init};
use flottille_ui::{space, text_region};
use gpui_kit::test::TestEventExt as _;
use gpui_kit::{
    Context, IntoElement, ParentElement, Render, StatefulInteractiveElement as _, Styled,
    TestAppContext, Window, div, point, px, size,
};

/// Two paragraphs a gap apart, in a text region that says when the pointer enters and leaves it.
struct Message {
    hovered: Rc<RefCell<Vec<bool>>>,
}

impl Render for Message {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let hovered = self.hovered.clone();
        div().p(px(40.)).child(
            text_region("message")
                .on_hover(move |on, _, _| hovered.borrow_mut().push(*on))
                .flex()
                .flex_col()
                .gap(space::XL)
                .w(px(300.))
                .child(div().h(px(40.)).child("first paragraph"))
                .child(div().h(px(40.)).child("second paragraph")),
        )
    }
}

#[gpui_kit::test]
fn moving_the_pointer_across_a_selectable_text_keeps_one_cursor_style(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let hovered = Rc::new(RefCell::new(Vec::new()));
    let seen = hovered.clone();
    let (_, cx) = cx.add_window_view(move |_, _| Message { hovered: seen });
    cx.simulate_resize(size(px(600.), px(400.)));

    // Down the text, a pixel at a time: the first paragraph, the gap, the second paragraph.
    cx.update(|window, cx| {
        for y in 41..(40 + 40 + 24 + 40) {
            window.pointer_move(point(px(100.), px(y as f32)), None, cx);
        }
    });
    assert_eq!(
        *hovered.borrow(),
        [true],
        "entered once, never left on the way through the gap"
    );

    // Off the text, to the side, it is left, and that is the arrow's.
    cx.update(|window, cx| window.pointer_move(point(px(500.), px(100.)), None, cx));
    assert_eq!(*hovered.borrow(), [true, false]);
}
