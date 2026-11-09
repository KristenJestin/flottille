//! A "⋯" menu (#590), on GPUI's test platform: it is as wide as its longest label and no label
//! wraps, the labels of its rows start at the same x whether a row has an icon or not, and no two
//! rows share an icon.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Desktop, advance, init};
use flottille_ui::motion::ENTER;
use flottille_ui::{Icon, Menu};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement, Pixels, Render,
    Styled, TestAppContext, VisualTestContext, Window, div, px, size,
};

type Draw = Box<dyn Fn(&mut Window, &mut Context<Host>) -> AnyElement>;

struct Host {
    draw: Draw,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Away from the window's edges: the list opens leftwards from its button.
        div()
            .size_full()
            .relative()
            .flex()
            .justify_center()
            .pt(px(40.))
            .child((self.draw)(window, cx))
    }
}

/// A window that draws a menu made by `make`, opened and settled.
fn opened(
    cx: &mut TestAppContext,
    make: impl Fn(&mut Context<Menu>) -> Menu + 'static,
) -> &mut VisualTestContext {
    init(cx, Desktop::new(false).env());
    let kept: Rc<RefCell<Option<Entity<Menu>>>> = Rc::default();
    let (_, cx) = cx.add_window_view(move |_, _| Host {
        draw: Box::new(move |_, cx| {
            let menu = kept
                .borrow_mut()
                .get_or_insert_with(|| cx.new(|cx| make(cx)))
                .clone();
            div().child(menu).into_any_element()
        }),
    });
    cx.simulate_resize(size(px(1000.), px(600.)));
    cx.update(|window, cx| window.render_frame(cx));
    cx.update(|window, cx| window.click("more", cx));
    advance(cx, ENTER * 2);
    cx
}

fn bounds(id: &str, cx: &mut VisualTestContext) -> gpui_kit::Bounds<Pixels> {
    let id = id.to_owned();
    cx.update(|window, _| window.find(id).bounds())
}

#[gpui_kit::test]
fn a_menu_label_stays_on_one_line(cx: &mut TestAppContext) {
    let cx = opened(cx, |cx| {
        Menu::new("more", "More", cx)
            .entry("reload", "Reload tools", Some(Icon::Restart), |_, _| {})
            .entry(
                "compact",
                "Compact conversation",
                Some(Icon::Edit),
                |_, _| {},
            )
            .entry(
                "long",
                "Detach from the lead that began the documentation of the whole public API",
                Some(Icon::Plus),
                |_, _| {},
            )
    });
    let (compact, long) = (bounds("compact", cx), bounds("long", cx));
    // One line is a row's minimum height (30 px); a second would add about 20.
    assert!(compact.size.height < px(36.), "{compact:?} is one line");
    assert!(long.size.height < px(36.), "{long:?} is one line");
    // The menu is as wide as its longest label, not a fixed width the label is cut to.
    let panel = bounds("more-menu", cx);
    assert!(panel.size.width > px(300.), "{panel:?} fits its label");
    let label = bounds("long-label", cx);
    assert!(
        label.right() <= long.right() && label.size.width > px(250.),
        "the label is whole: {label:?} in {long:?}"
    );
}

#[gpui_kit::test]
fn a_label_too_long_for_the_window_ellipsizes_instead_of_wrapping(cx: &mut TestAppContext) {
    let cx = opened(cx, |cx| {
        Menu::new("more", "More", cx).entry("huge", "x".repeat(400), Some(Icon::Plus), |_, _| {})
    });
    let (huge, panel) = (bounds("huge", cx), bounds("more-menu", cx));
    assert!(huge.size.height < px(36.), "{huge:?} is one line");
    assert!(panel.size.width < px(600.), "{panel:?} stays in the window");
}

#[gpui_kit::test]
fn every_row_of_a_menu_starts_its_label_at_the_same_x(cx: &mut TestAppContext) {
    let cx = opened(cx, |cx| {
        Menu::new("more", "More", cx)
            .entry(
                "open",
                "Open in editor",
                Some(Icon::OpenInEditor),
                |_, _| {},
            )
            .entry("bare", "Rename…", None, |_, _| {})
            .entry("end", "End session…", Some(Icon::Stop), |_, _| {})
            .danger("remove", "Remove…", |_, _| {})
    });
    let starts: Vec<_> = ["open", "bare", "end", "remove"]
        .into_iter()
        .map(|row| bounds(&format!("{row}-label"), cx).left())
        .collect();
    assert!(
        starts.windows(2).all(|pair| pair[0] == pair[1]),
        "the labels start at {starts:?}"
    );
}

#[gpui_kit::test]
#[should_panic(expected = "same icon")]
fn a_menu_refuses_two_rows_with_the_same_icon(cx: &mut TestAppContext) {
    opened(cx, |cx| {
        Menu::new("more", "More", cx)
            .entry("reload", "Reload tools", Some(Icon::Restart), |_, _| {})
            .entry("compact", "Compact", Some(Icon::Restart), |_, _| {})
    });
}

#[gpui_kit::test]
fn a_menu_entry_can_carry_a_second_line_that_is_cut_never_wrapped(cx: &mut TestAppContext) {
    let cx = opened(cx, |cx| {
        Menu::new("more", "More", cx)
            .entry("detach", "Detach from its lead", Some(Icon::Detach), |_, _| {})
            .detail("Bring every route of the public API under the new guard, then document each of them in the reference")
            .entry("rename", "Rename…", Some(Icon::Edit), |_, _| {})
    });
    let (label, detail) = (bounds("detach-label", cx), bounds("detach-detail", cx));
    let (row, panel) = (bounds("detach", cx), bounds("more-menu", cx));
    assert_eq!(label.left(), detail.left(), "under the label");
    assert!(detail.top() >= label.bottom(), "{detail:?} under {label:?}");
    assert!(detail.size.height < px(24.), "{detail:?} is one line");
    assert!(row.size.height < px(56.), "{row:?} has two lines at most");
    assert!(panel.size.width < px(600.), "{panel:?} stays in the window");
    assert!(
        bounds("rename", cx).size.height < px(36.),
        "a row with no detail stays one line"
    );
}
