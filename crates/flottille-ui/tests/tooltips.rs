//! A button's tooltip, on GPUI's test platform: anchored to its button, never under the pointer.

mod common;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::{HOVER, TOOLTIP};
use flottille_ui::{Button, Icon};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    TestAppContext, VisualTestContext, Window, div, point, px, size,
};

/// Where the button sits in a 600 × 500 window.
#[derive(Clone, Copy)]
enum Place {
    Top,
    Bottom,
    Right,
}

struct Page(Place);

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let button = Button::icon("copy", Icon::Copy, "Copy the message to the clipboard");
        let page = div().size_full().flex().flex_col();
        match self.0 {
            Place::Top => page.items_center().child(div().h(px(40.)).child(button)),
            Place::Bottom => page
                .items_center()
                .justify_end()
                .child(div().h(px(28.)).child(button)),
            Place::Right => page.items_end().child(div().h(px(40.)).child(button)),
        }
    }
}

fn page(place: Place, cx: &mut TestAppContext) -> (Entity<Page>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (page, cx) = cx.add_window_view(move |_, _| Page(place));
    cx.simulate_resize(size(px(600.), px(500.)));
    (page, cx)
}

/// The pointer rests on the button until its tooltip has been shown.
fn hover_and_wait(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.hover("copy", cx));
    advance(cx, TOOLTIP + HOVER * 2);
}

struct Seen {
    tip: gpui_kit::Bounds<Pixels>,
    button: gpui_kit::Bounds<Pixels>,
    viewport: gpui_kit::Size<Pixels>,
}

fn seen(cx: &mut VisualTestContext) -> Seen {
    cx.update(|window, cx| {
        window.render_frame(cx);
        Seen {
            tip: window.find("copy-tooltip").bounds(),
            button: window.find("copy").bounds(),
            viewport: window.viewport_size(),
        }
    })
}

#[gpui_kit::test]
fn the_tooltip_of_a_button_lies_below_the_button_and_does_not_overlap_it(cx: &mut TestAppContext) {
    let (_, cx) = page(Place::Top, cx);
    hover_and_wait(cx);
    let Seen { tip, button, .. } = seen(cx);

    assert!(tip.top() >= button.bottom(), "{tip:?} below {button:?}");
    assert!(!tip.intersects(&button));
    let centres = (tip.center().x - button.center().x).abs();
    assert!(centres <= px(1.), "centred on the button: {centres:?} off");
}

#[gpui_kit::test]
fn a_button_at_the_bottom_edge_has_its_tooltip_above_it(cx: &mut TestAppContext) {
    let (_, cx) = page(Place::Bottom, cx);
    hover_and_wait(cx);
    let Seen {
        tip,
        button,
        viewport,
    } = seen(cx);

    assert!(tip.bottom() <= button.top(), "{tip:?} above {button:?}");
    assert!(!tip.intersects(&button));
    assert!(tip.top() >= px(0.) && tip.bottom() <= viewport.height);
}

#[gpui_kit::test]
fn a_button_at_the_right_edge_has_its_tooltip_inside_the_window(cx: &mut TestAppContext) {
    let (_, cx) = page(Place::Right, cx);
    hover_and_wait(cx);
    let Seen {
        tip,
        button,
        viewport,
    } = seen(cx);

    assert!(tip.left() >= px(0.) && tip.right() <= viewport.width);
    assert!(!tip.intersects(&button));
    assert!(tip.top() >= button.bottom(), "still below it");
}

#[gpui_kit::test]
fn the_tooltip_waits_for_the_delay_then_fades_in_and_out(cx: &mut TestAppContext) {
    let (_, cx) = page(Place::Top, cx);
    cx.update(|window, cx| window.hover("copy", cx));
    advance(cx, TOOLTIP / 2);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("copy-tooltip").is_none(), "not yet");
    });

    advance(cx, TOOLTIP / 2 + HOVER * 2);
    seen(cx);
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn once it is shown");

    cx.simulate_mouse_move(point(px(590.), px(490.)), None, Default::default());
    advance(cx, HOVER / 3);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("copy-tooltip").is_some(),
            "still drawn while it fades out"
        );
    });
    advance(cx, HOVER * 3);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("copy-tooltip").is_none(), "gone");
    });
    assert_eq!(pending_frames(cx), 0, "and nothing is redrawn");
}

#[gpui_kit::test]
fn the_tooltip_of_a_button_whose_menu_is_open_stays_hidden(cx: &mut TestAppContext) {
    struct Hidden;
    impl Render for Hidden {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(Button::icon("copy", Icon::Copy, "Copy").tooltip_hidden(true))
        }
    }
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Hidden);
    cx.simulate_resize(size(px(600.), px(500.)));
    hover_and_wait(cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("copy-tooltip").is_none());
    });
}

/// GPUI's own `.tooltip(|..| ..)` opens under the pointer: a tooltip is `Button::tooltip`.
#[test]
fn no_source_uses_the_tooltip_that_follows_the_pointer() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut seen = Vec::new();
    for krate in ["flottille-ui", "flottille-window"] {
        let mut folders = vec![crates.join(krate).join("src")];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    if text.contains(".tooltip(|") || text.contains(".tooltip(move |") {
                        seen.push(path);
                    }
                }
            }
        }
    }
    assert!(seen.is_empty(), "GPUI's tooltip is used in {seen:?}");
}

/// A mark that is no button, with the same anchored tooltip.
struct Mark;

impl Render for Mark {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mark = flottille_ui::tipped(
            div().id("mark").size(px(16.)),
            "New since you looked: a new message",
            16.,
            window,
            cx,
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .child(div().h(px(40.)).child(mark))
    }
}

#[gpui_kit::test]
fn any_element_can_carry_the_anchored_tooltip_and_the_tooltip_says_its_words(
    cx: &mut TestAppContext,
) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Mark);
    cx.simulate_resize(size(px(600.), px(500.)));
    cx.update(|window, cx| window.hover("mark", cx));
    advance(cx, TOOLTIP / 2);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("mark-tooltip").is_none(), "not yet");
    });
    advance(cx, TOOLTIP / 2 + HOVER * 2);
    cx.update(|window, cx| {
        window.render_frame(cx);
        let (tip, mark) = (window.find("mark-tooltip"), window.find("mark"));
        assert!(
            tip.bounds().top() >= mark.bounds().bottom(),
            "below the mark"
        );
        assert_eq!(
            tip.label(),
            Some("New since you looked: a new message"),
            "it says its words"
        );
    });
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn once it is shown");
}
