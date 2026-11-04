//! A long text never leaves its frame (#391), on GPUI's test platform: a path on one line keeps
//! its file name whole and cuts its folder at the start, a card's title makes room for its meta,
//! and the whole path is in a tooltip.

mod common;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::{HOVER, TOOLTIP};
use flottille_ui::{Card, Icon, icon, path_line};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    TestAppContext, TestSupportExt, VisualTestContext, Window, div, px, size,
};

const DEEP: &str = "apps/front/src/app/pages/(dashboard)/settings/components/mail-recipient-sheet/mail-recipient-sheet.html";
const NAME: &str = "mail-recipient-sheet.html";

/// A card as the conversation's edit card builds it, 300 px wide: an icon, the path, a chevron,
/// and the count in its meta.
struct Edit(&'static str);

impl Render for Edit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = div()
            .id("title")
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_1()
            .min_w_0()
            .child(icon(Icon::File).flex_none())
            .child(path_line("path", self.0, 12.5, window, cx))
            .child(
                div()
                    .id("chevron")
                    .flex_none()
                    .ml_auto()
                    .child(icon(Icon::Chevron))
                    .test_support(),
            )
            .test_support();
        div().id("frame").w(px(300.)).test_support().child(
            Card::new()
                .id("card")
                .title(title)
                .meta("+13 −14")
                .child(div().child("x")),
        )
    }
}

fn open<'a>(path: &'static str, cx: &'a mut TestAppContext) -> &'a mut VisualTestContext {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(move |_, _| Edit(path));
    cx.simulate_resize(size(px(800.), px(400.)));
    cx.update(|window, cx| window.render_frame(cx));
    cx
}

fn bounds(cx: &mut VisualTestContext, id: &'static str) -> Bounds<Pixels> {
    cx.update(|window, _| window.find(id).bounds())
}

#[gpui_kit::test]
fn a_long_path_keeps_the_count_inside_the_card(cx: &mut TestAppContext) {
    let cx = open(DEEP, cx);
    let (frame, chevron, meta) = (
        bounds(cx, "frame"),
        bounds(cx, "chevron"),
        bounds(cx, "card-meta"),
    );
    assert!(chevron.right() <= frame.right(), "{chevron:?} in {frame:?}");
    assert!(meta.right() <= frame.right(), "{meta:?} in {frame:?}");
    assert!(
        chevron.right() <= meta.left(),
        "the chevron comes before the count"
    );
    assert!(
        bounds(cx, "path").right() <= chevron.left(),
        "the path ends before the chevron"
    );
}

#[gpui_kit::test]
fn the_file_name_stays_whole_when_the_folder_is_cut(cx: &mut TestAppContext) {
    let cx = open(NAME, cx);
    let alone = bounds(cx, "path-name").size.width;
    let cx = open(DEEP, cx);
    let (name, folder, path) = (
        bounds(cx, "path-name"),
        bounds(cx, "path-folder"),
        bounds(cx, "path"),
    );
    assert_eq!(name.size.width, alone, "the name is as wide as it is alone");
    assert!(
        folder.size.width < px(DEEP.len() as f32 * 6.),
        "the folder is cut: {folder:?}"
    );
    assert!(
        folder.right() <= name.left() + px(0.5),
        "the folder comes before the name"
    );
    assert!(name.right() <= path.right(), "{name:?} in {path:?}");
}

#[gpui_kit::test]
fn a_file_name_wider_than_the_card_is_cut_at_its_end(cx: &mut TestAppContext) {
    let cx = open(
        "src/a-file-name-that-is-longer-than-the-card-that-holds-it-by-far-and-far.html",
        cx,
    );
    let (frame, name, chevron) = (
        bounds(cx, "frame"),
        bounds(cx, "path-name"),
        bounds(cx, "chevron"),
    );
    assert!(name.right() <= chevron.left(), "{name:?}");
    assert!(chevron.right() <= frame.right());
}

#[gpui_kit::test]
fn a_path_with_no_folder_is_its_name(cx: &mut TestAppContext) {
    let cx = open("README.md", cx);
    cx.update(|window, _| {
        assert!(window.try_find("path-folder").is_none());
        assert_eq!(window.find("path-name").label(), Some("README.md"));
    });
}

#[gpui_kit::test]
fn the_whole_path_is_in_the_tooltip(cx: &mut TestAppContext) {
    let cx = open(DEEP, cx);
    // Narrower than the path set on one line.
    cx.simulate_resize(size(px(500.), px(400.)));
    cx.update(|window, cx| window.hover("path", cx));
    advance(cx, TOOLTIP + HOVER * 2);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("path-tooltip").label(), Some(DEEP));
        assert_eq!(
            window.find("path").label(),
            Some(DEEP),
            "and it is what is read"
        );
        let tip = window.find("path-tooltip").bounds();
        assert!(
            tip.left() >= px(-1.) && tip.right() <= px(500.),
            "the tooltip stays inside the window: {tip:?}"
        );
    });
    assert_eq!(pending_frames(cx), 0);
}

const LONG_TITLE: &str = "Fix the recipients sheet so that a very long title of a session never pushes its actions out of the page";

/// A page header 300 px wide.
struct Header;

impl Render for Header {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("frame").w(px(300.)).test_support().child(
            flottille_ui::PageHeader::new("page", LONG_TITLE).action(
                "page-edit",
                "Edit",
                None,
                |_, _| {},
            ),
        )
    }
}

#[gpui_kit::test]
fn a_long_title_stays_inside_its_page_header(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Header);
    cx.simulate_resize(size(px(800.), px(400.)));
    cx.update(|window, cx| window.render_frame(cx));
    let (frame, title, edit) = (
        bounds(cx, "frame"),
        bounds(cx, "page-title"),
        bounds(cx, "page-edit"),
    );
    assert!(edit.right() <= frame.right(), "{edit:?} in {frame:?}");
    assert!(title.right() <= edit.left(), "{title:?} before {edit:?}");
}

/// A card whose title is only words.
struct Words;

impl Render for Words {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("frame").w(px(300.)).test_support().child(
            Card::new()
                .id("card")
                .title("Reviews and comments from the people who looked at this pull request")
                .meta("12 of 12")
                .child(div().child("x")),
        )
    }
}

#[gpui_kit::test]
fn a_long_card_title_stays_inside_the_card(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Words);
    cx.simulate_resize(size(px(800.), px(400.)));
    cx.update(|window, cx| window.render_frame(cx));
    let (frame, meta) = (bounds(cx, "frame"), bounds(cx, "card-meta"));
    assert!(meta.right() <= frame.right(), "{meta:?} in {frame:?}");
    assert!(
        meta.size.width < px(80.),
        "the count keeps its room: {meta:?}"
    );
    assert!(meta.top() < px(30.), "on the first line: {meta:?}");
}
