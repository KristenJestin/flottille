//! The same fault as a long chip (#731), everywhere a text someone wrote is shown:
//! it stays inside the place it sits in, on GPUI's test platform.

mod common;

use common::{Desktop, init};
use flottille_ui::{ListRow, Section};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    TestAppContext, TestSupportExt, VisualTestContext, Window, div, px, size,
};

const PATH: &str =
    "/home/someone/documents/a-folder-with-a-long-name/another/and-another/deep/file.md";
const TITLE: &str = "A section named by a person who wrote a whole sentence into it for no reason";

struct Frame;

impl Render for Frame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("frame")
            .w(px(300.))
            .flex()
            .flex_col()
            .test_support()
            .child(ListRow::new("row", "Skill").tag(PATH))
            .child(Section::new("section", TITLE).tag(PATH).count("12"))
    }
}

fn open(cx: &mut TestAppContext) -> &mut VisualTestContext {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Frame);
    cx.simulate_resize(size(px(800.), px(400.)));
    cx.update(|window, cx| window.render_frame(cx));
    cx
}

fn bounds(cx: &mut VisualTestContext, id: &'static str) -> Bounds<Pixels> {
    cx.update(|window, _| window.find(id).bounds())
}

#[gpui_kit::test]
fn a_long_tag_stays_inside_its_row(cx: &mut TestAppContext) {
    let cx = open(cx);
    let (frame, tag) = (bounds(cx, "frame"), bounds(cx, "row-tag-0"));
    assert!(tag.right() <= frame.right(), "{tag:?} in {frame:?}");
    assert!(
        bounds(cx, "row-title").size.width > px(30.),
        "the title keeps its room"
    );
}

#[gpui_kit::test]
fn a_long_section_title_and_tag_stay_inside_the_page(cx: &mut TestAppContext) {
    let cx = open(cx);
    let frame = bounds(cx, "frame");
    for id in ["section-title", "section-count", "section-tag"] {
        let part = bounds(cx, id);
        assert!(part.right() <= frame.right(), "{id}: {part:?} in {frame:?}");
    }
}
