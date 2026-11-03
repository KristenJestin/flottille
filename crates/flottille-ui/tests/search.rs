//! A search over the page, as the window's quick switcher builds it from `Modal::search` and
//! `ListRow`s, on GPUI's test platform: the dialog rises and fades in near the top and is gone
//! once it has left, Escape and a click outside close it, while it is open it holds the keys, a
//! row's marked fill moves over `HOVER`, the pointer marks the row it enters, what matched is
//! drawn in a row whose label stays its whole title, and nothing is drawn at rest.

mod common;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::{HOVER, MODAL};
use flottille_ui::{ListRow, Modal, forget_overlays, keys_held, pressable};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, ParentElement, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};

struct Search {
    open: bool,
    marked: usize,
    closed: usize,
    clicked: Vec<usize>,
    keys: Vec<String>,
    focus: FocusHandle,
}

impl Render for Search {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // As the window's root does: the overlays drawn open say so again in each frame.
        forget_overlays(cx);
        let this = cx.entity().downgrade();
        let rows = (0..3).map(|at| {
            let (click, mark) = (this.clone(), this.clone());
            ListRow::new(format!("row-{at}"), format!("Result {at}"))
                .pieces(vec![("Res".into(), true), (format!("ult {at}"), false)])
                .line("harbor · main")
                .marked(self.marked == at)
                .on_click(move |_, _, cx| {
                    click
                        .update(cx, |this, cx| {
                            this.clicked.push(at);
                            cx.notify();
                        })
                        .ok();
                })
                .on_enter(move |cx| {
                    mark.update(cx, |this, cx| {
                        this.marked = at;
                        cx.notify();
                    })
                    .ok();
                })
        });
        let (close, key) = (this.clone(), this.clone());
        div().size_full().child(
            Modal::new("search", "Go to")
                .search()
                .open(self.open)
                .on_close(move |_, cx| {
                    close
                        .update(cx, |this, cx| {
                            this.open = false;
                            this.closed += 1;
                            cx.notify();
                        })
                        .ok();
                })
                .on_key(move |event, _, cx| {
                    let pressed = event.keystroke.key.clone();
                    key.update(cx, |this, _| this.keys.push(pressed)).ok();
                })
                .above(
                    div()
                        .id("search-field")
                        .track_focus(&self.focus)
                        .child("field"),
                )
                .children(rows),
        )
    }
}

fn search(cx: &mut TestAppContext) -> (Entity<Search>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (search, cx) = cx.add_window_view(|_, cx| Search {
        open: false,
        marked: 0,
        closed: 0,
        clicked: Vec::new(),
        keys: Vec::new(),
        focus: cx.focus_handle(),
    });
    cx.simulate_resize(gpui_kit::size(px(1000.), px(700.)));
    (search, cx)
}

fn open(search: &Entity<Search>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        search.update(cx, |s, cx| {
            s.open = true;
            cx.notify();
            window.focus(&s.focus, cx);
        });
        window.render_frame(cx);
    });
}

fn top(cx: &mut VisualTestContext) -> gpui_kit::Pixels {
    cx.update(|window, _| window.find("search").bounds().top())
}

#[gpui_kit::test]
fn a_search_rises_in_near_the_top_without_a_title_or_a_cancel_and_rests(cx: &mut TestAppContext) {
    let (search, cx) = search(cx);
    cx.update(|window, _| assert!(window.try_find("search").is_none(), "closed: not drawn"));
    open(&search, cx);
    let first = top(cx);
    advance(cx, MODAL / 2);
    let part_way = top(cx);
    advance(cx, MODAL * 2);
    let rested = top(cx);
    assert!(
        first > part_way && part_way > rested,
        "{first:?} {part_way:?} {rested:?}"
    );
    assert!(rested < px(100.), "near the top, not centred: {rested:?}");
    cx.update(|window, _| {
        assert_eq!(
            window.find("search").label(),
            Some("Go to"),
            "its title is its label"
        );
        assert!(window.try_find("search-title").is_none(), "no title drawn");
        assert!(window.try_find("search-cancel").is_none(), "no Cancel");
    });
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn at rest");
}

#[gpui_kit::test]
fn escape_and_a_click_outside_close_it_and_it_is_gone_once_it_has_left(cx: &mut TestAppContext) {
    let (search, cx) = search(cx);
    open(&search, cx);
    advance(cx, MODAL * 2);
    cx.update(|window, cx| window.press("down", cx));
    assert_eq!(search.read_with(cx, |s, _| s.keys.clone()), ["down"]);
    cx.update(|window, cx| window.press("escape", cx));
    assert_eq!(search.read_with(cx, |s, _| s.closed), 1);
    assert_eq!(
        search.read_with(cx, |s, _| s.keys.len()),
        1,
        "Escape is its own"
    );
    advance(cx, MODAL / 2);
    cx.update(|window, _| assert!(window.try_find("search").is_some(), "still leaving"));
    advance(cx, MODAL * 2);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("search").is_none());
    });
    assert_eq!(pending_frames(cx), 0);

    open(&search, cx);
    advance(cx, MODAL * 2);
    cx.update(|window, cx| window.click("search-scrim", cx));
    assert_eq!(search.read_with(cx, |s, _| s.closed), 2);
}

#[gpui_kit::test]
fn an_open_dialog_holds_the_keys_and_a_closed_one_does_not(cx: &mut TestAppContext) {
    let (search, cx) = search(cx);
    advance(cx, MODAL);
    assert!(!cx.update(|_, cx| keys_held(cx)));
    open(&search, cx);
    advance(cx, MODAL * 2);
    assert!(cx.update(|_, cx| keys_held(cx)));
    cx.update(|window, cx| window.press("escape", cx));
    advance(cx, MODAL * 2);
    assert!(!cx.update(|_, cx| keys_held(cx)));
}

#[gpui_kit::test]
fn the_pointer_marks_the_row_it_enters_and_the_fill_moves_over_hover(cx: &mut TestAppContext) {
    let (search, cx) = search(cx);
    open(&search, cx);
    advance(cx, MODAL * 2);
    assert_eq!(pending_frames(cx), 0);
    cx.update(|window, cx| window.hover("row-2", cx));
    advance(cx, HOVER / 2);
    assert_eq!(search.read_with(cx, |s, _| s.marked), 2);
    assert!(pending_frames(cx) > 0, "the fill is still moving");
    advance(cx, HOVER);
    assert_eq!(pending_frames(cx), 0, "it has moved within HOVER");
    cx.update(|window, cx| window.click("row-1", cx));
    assert_eq!(search.read_with(cx, |s, _| s.clicked.clone()), [1]);
}

#[gpui_kit::test]
fn a_row_says_its_title_whole_whatever_is_marked_in_it(cx: &mut TestAppContext) {
    let (search, cx) = search(cx);
    open(&search, cx);
    advance(cx, MODAL * 2);
    cx.update(|window, _| {
        assert_eq!(window.find("row-0").label(), Some("Result 0"));
    });
}

struct Pressed;

impl Render for Pressed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(pressable(
            div().id("field").w(px(200.)).h(px(26.)).child("Search"),
            |d, hover, press| d.opacity(1.0 - 0.1 * hover - 0.1 * press),
        ))
    }
}

#[gpui_kit::test]
fn a_pressable_element_moves_when_pressed_and_when_released(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Pressed);
    cx.simulate_resize(gpui_kit::size(px(400.), px(200.)));
    let at = cx.update(|window, _| window.find("field").bounds().center());
    let none = gpui_kit::Modifiers::default();
    cx.simulate_mouse_move(at, None, none);
    advance(cx, HOVER * 2);
    assert_eq!(pending_frames(cx), 0);
    cx.simulate_mouse_down(at, gpui_kit::MouseButton::Left, none);
    assert!(pending_frames(cx) > 0, "the press moves");
    advance(cx, HOVER * 2);
    assert_eq!(pending_frames(cx), 0);
    cx.simulate_mouse_up(at, gpui_kit::MouseButton::Left, none);
    assert!(pending_frames(cx) > 0, "the release moves");
    advance(cx, HOVER * 2);
    assert_eq!(pending_frames(cx), 0);
}
