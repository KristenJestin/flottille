//! The rule "everything that moves is animated", on GPUI's test platform: a hover fades, a row
//! that arrives unfolds, one that goes folds away before it is gone, one that moves slides, a
//! block that comes and goes folds, and each one stops asking for frames once it has arrived.

mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use common::subpage::Settings;
use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::{ENTER, HOVER, MAX_STAGGER, PANEL, RISE, STAGGER};
use flottille_ui::{
    Button, Item, Listing, Segmented, Tabs, changing, enter, presence, presence_of, selectable,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement, Pixels, Render, SharedString,
    Styled, TestAppContext, TestSupportExt as _, VisualTestContext, Window, div, point, px,
};

// ── Hover ──────────────────────────────────────────────────────────────────────────────────────

/// What a hoverable element was asked to draw.
#[derive(Default, Clone)]
struct Seen {
    hover: Rc<Cell<f32>>,
    chosen: Rc<Cell<f32>>,
}

struct Hovered {
    seen: Seen,
    chosen: bool,
}

impl Render for Hovered {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let seen = self.seen.clone();
        div().size_full().child(selectable(
            div().id("target").w(px(120.)).h(px(30.)),
            self.chosen,
            move |d, hover, chosen| {
                seen.hover.set(hover);
                seen.chosen.set(chosen);
                d
            },
        ))
    }
}

fn hovered(
    chosen: bool,
    reduce: bool,
    cx: &mut TestAppContext,
) -> (gpui_kit::Entity<Hovered>, Seen, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(reduce));
    let seen = Seen::default();
    let shared = seen.clone();
    let (view, cx) = cx.add_window_view(move |_, _| Hovered {
        seen: shared,
        chosen,
    });
    cx.update(|window, cx| window.render_frame(cx));
    (view, seen, cx)
}

#[gpui_kit::test]
fn a_hover_on_an_element_of_a_screen_fades_in_and_out_instead_of_switching(
    cx: &mut TestAppContext,
) {
    let (_, seen, cx) = hovered(false, false, cx);
    assert_eq!(seen.hover.get(), 0.0);

    cx.update(|window, cx| window.hover("target", cx));
    advance(cx, HOVER / 3);
    let part = seen.hover.get();
    assert!(0.0 < part && part < 1.0, "part way after a third: {part}");
    advance(cx, HOVER * 2);
    assert_eq!(seen.hover.get(), 1.0);
    assert_eq!(pending_frames(cx), 0, "nothing is drawn once it arrived");

    cx.simulate_mouse_move(point(px(600.), px(400.)), None, Default::default());
    advance(cx, HOVER / 3);
    let back = seen.hover.get();
    assert!(0.0 < back && back < 1.0, "on its way out: {back}");
    advance(cx, HOVER * 2);
    assert_eq!(seen.hover.get(), 0.0);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn with_reduced_motion_a_hover_jumps(cx: &mut TestAppContext) {
    let (_, seen, cx) = hovered(false, true, cx);
    cx.update(|window, cx| window.hover("target", cx));
    advance(cx, Duration::from_millis(16));
    assert_eq!(seen.hover.get(), 1.0);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn an_element_first_drawn_chosen_is_chosen_and_one_chosen_later_fades_to_it(
    cx: &mut TestAppContext,
) {
    let (view, seen, cx) = hovered(false, false, cx);
    assert_eq!(seen.chosen.get(), 0.0);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.chosen = true;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL / 3);
    let part = seen.chosen.get();
    assert!(0.0 < part && part < 1.0, "part way: {part}");
    advance(cx, PANEL * 2);
    assert_eq!(seen.chosen.get(), 1.0);

    let (_, seen, cx) = hovered(true, false, cx);
    assert_eq!(seen.chosen.get(), 1.0, "chosen from its first frame");
    advance(cx, PANEL);
    assert_eq!(pending_frames(cx), 0, "and nothing moves");
}

struct Choices {
    on: bool,
}

impl Render for Choices {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Button::choice("choice", "Copy", self.on))
    }
}

#[gpui_kit::test]
fn a_choice_button_moves_its_fill_over_frames_and_then_stops(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Choices { on: false });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL);
    assert_eq!(pending_frames(cx), 0);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.on = true;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(pending_frames(cx) > 0, "the fill is moving");
    advance(cx, PANEL * 2);
    assert_eq!(pending_frames(cx), 0, "it stopped once it was there");
}

struct Modes {
    auto: bool,
    chosen: Rc<Cell<Option<usize>>>,
}

impl Render for Modes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let chosen = self.chosen.clone();
        div().size_full().child(
            Segmented::new(
                "mode",
                "Mode",
                [("ask", "Ask"), ("auto", "Auto")],
                usize::from(self.auto),
            )
            .on_choose(move |index, _, _| chosen.set(Some(index))),
        )
    }
}

#[gpui_kit::test]
fn a_segmented_control_reports_the_segment_clicked_and_its_chosen_one_is_selected(
    cx: &mut TestAppContext,
) {
    init(cx, Desktop::new(false).env());
    let chosen = Rc::new(Cell::new(None));
    let seen = chosen.clone();
    let (view, cx) = cx.add_window_view(move |_, _| Modes {
        auto: false,
        chosen: seen,
    });
    cx.update(|window, cx| window.render_frame(cx));
    cx.update(|window, _| {
        assert_eq!(window.find("mode-ask").selected(), Some(true));
        assert_eq!(window.find("mode-auto").selected(), Some(false));
    });
    // The chosen segment is not a click; the other one is.
    cx.update(|window, cx| window.click("mode-ask", cx));
    assert_eq!(chosen.get(), None);
    cx.update(|window, cx| window.click("mode-auto", cx));
    assert_eq!(chosen.get(), Some(1));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.auto = true;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    cx.update(|window, _| assert_eq!(window.find("mode-auto").selected(), Some(true)));
}

#[gpui_kit::test]
fn a_segmented_control_slides_its_highlight_over_frames_and_then_stops(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Modes {
        auto: false,
        chosen: Rc::default(),
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL);
    assert_eq!(pending_frames(cx), 0, "at rest nothing is drawn");
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.auto = true;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(pending_frames(cx) > 0, "the highlight is moving");
    advance(cx, PANEL * 2);
    assert_eq!(pending_frames(cx), 0, "it stopped once it was there");
}

struct Reach {
    taken: bool,
}

impl Render for Reach {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let segmented = Segmented::new(
            "theme",
            "Theme",
            [("omarchy", "Omarchy"), ("plain", "Plain")],
            1,
        );
        div().size_full().child(if self.taken {
            segmented.unavailable(0)
        } else {
            segmented
        })
    }
}

fn reach(cx: &mut TestAppContext) -> (gpui_kit::Entity<Reach>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Reach { taken: false });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, HOVER * 2);
    (view, cx)
}

fn take(view: &gpui_kit::Entity<Reach>, taken: bool, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.taken = taken;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
}

#[gpui_kit::test]
fn a_segment_that_becomes_unavailable_fades_over_hover_and_fades_back_instead_of_switching(
    cx: &mut TestAppContext,
) {
    let (view, cx) = reach(cx);
    assert_eq!(pending_frames(cx), 0, "at rest nothing is drawn");
    take(&view, true, cx);
    assert!(pending_frames(cx) > 0, "the segment is fading out");
    advance(cx, HOVER * 2);
    assert_eq!(pending_frames(cx), 0, "it stopped once it was faded");
    take(&view, false, cx);
    assert!(pending_frames(cx) > 0, "the segment is fading back in");
    advance(cx, HOVER * 2);
    assert_eq!(pending_frames(cx), 0, "it stopped once it was back");
}

#[gpui_kit::test]
fn a_segment_that_is_unavailable_has_no_hover(cx: &mut TestAppContext) {
    let (view, cx) = reach(cx);
    take(&view, true, cx);
    advance(cx, HOVER * 2);
    cx.update(|window, cx| window.hover("theme-omarchy", cx));
    assert_eq!(pending_frames(cx), 0, "the pointer on it moves nothing");
}

#[test]
fn a_segmented_control_fills_its_alarm_choice_in_the_alarm_colour_and_fades_to_it_along_the_slide()
{
    use flottille_ui::segmented::highlight;
    use flottille_ui::theme::Palette;
    let p = Palette::base();
    let close = |(fill, ink): (gpui_kit::Hsla, gpui_kit::Hsla),
                 (f, i): (gpui_kit::Hsla, gpui_kit::Hsla)| {
        let near = |a: gpui_kit::Hsla, b: gpui_kit::Hsla| {
            let (a, b) = (a.to_rgb(), b.to_rgb());
            [(a.r, b.r), (a.g, b.g), (a.b, b.b), (a.a, b.a)]
                .iter()
                .all(|(x, y)| (x - y).abs() < 0.002)
        };
        near(fill, f) && near(ink, i)
    };
    // On the alarm choice: the fail fill and its ink; elsewhere, the accent.
    assert!(close(highlight(&p, Some(2), 2.0), (p.fail, p.on_fail)));
    assert!(close(highlight(&p, Some(2), 1.0), (p.acc, p.on_acc)));
    assert!(close(highlight(&p, None, 2.0), (p.acc, p.on_acc)));
    // Halfway there, it is between the two: it fades, it does not switch.
    let half = highlight(&p, Some(2), 1.5);
    assert!(!close(half, (p.acc, p.on_acc)));
    assert!(!close(half, (p.fail, p.on_fail)));
}

struct Pages {
    chosen: usize,
    picked: Rc<Cell<Option<usize>>>,
}

impl Render for Pages {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let picked = self.picked.clone();
        div().size_full().child(
            Tabs::new("pages", "Pages", self.chosen)
                .tab("one", "One")
                .counted("two", "Two", "3")
                .tab("three", "Three")
                .on_choose(move |index, _, _| picked.set(Some(index))),
        )
    }
}

fn pages(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<Pages>,
    Rc<Cell<Option<usize>>>,
    &mut VisualTestContext,
) {
    init(cx, Desktop::new(false).env());
    let picked = Rc::new(Cell::new(None));
    let seen = picked.clone();
    let (view, cx) = cx.add_window_view(move |_, _| Pages {
        chosen: 0,
        picked: seen,
    });
    cx.update(|window, cx| window.render_frame(cx));
    (view, picked, cx)
}

#[gpui_kit::test]
fn tabs_report_the_tab_clicked_and_their_chosen_one_is_selected(cx: &mut TestAppContext) {
    let (view, picked, cx) = pages(cx);
    cx.update(|window, _| {
        assert_eq!(window.find("pages-one").selected(), Some(true));
        assert_eq!(window.find("pages-two").selected(), Some(false));
    });
    // The chosen tab is not a click; another one is.
    cx.update(|window, cx| window.click("pages-one", cx));
    assert_eq!(picked.get(), None);
    cx.update(|window, cx| window.click("pages-three", cx));
    assert_eq!(picked.get(), Some(2));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.chosen = 2;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    cx.update(|window, _| {
        assert_eq!(window.find("pages-three").selected(), Some(true));
        assert_eq!(window.find("pages-one").selected(), Some(false));
    });
}

#[gpui_kit::test]
fn tabs_say_their_count_next_to_their_name_when_they_have_one(cx: &mut TestAppContext) {
    let (_, _, cx) = pages(cx);
    cx.update(|window, _| {
        assert_eq!(window.find("pages-two-count").label(), Some("3"));
        assert!(window.try_find("pages-one-count").is_none());
    });
}

#[gpui_kit::test]
fn tabs_slide_their_marker_over_frames_and_then_stop(cx: &mut TestAppContext) {
    let (view, _, cx) = pages(cx);
    advance(cx, PANEL);
    assert_eq!(pending_frames(cx), 0, "at rest nothing is drawn");
    let before = cx.update(|window, _| window.find("pages-marker").bounds());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.chosen = 2;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(pending_frames(cx) > 0, "the marker is moving");
    advance(cx, PANEL / 2);
    let midway = cx.update(|window, _| window.find("pages-marker").bounds());
    let tab = cx.update(|window, _| window.find("pages-three").bounds());
    assert!(
        midway.left() > before.left() && midway.left() < tab.left(),
        "halfway between the two tabs: {:?} {:?} {:?}",
        before.left(),
        midway.left(),
        tab.left()
    );
    advance(cx, PANEL * 2);
    assert_eq!(pending_frames(cx), 0, "it stopped once it was there");
    let after = cx.update(|window, _| window.find("pages-marker").bounds());
    assert_eq!(after.left(), tab.left(), "under the chosen tab");
    assert_eq!(after.size.width, tab.size.width);
}

struct Enabling {
    on: bool,
}

impl Render for Enabling {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Button::quiet("go", "Go").disabled(!self.on))
    }
}

#[gpui_kit::test]
fn a_button_that_becomes_enabled_fades_to_it_and_then_stops(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Enabling { on: false });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL);
    assert_eq!(
        pending_frames(cx),
        0,
        "a button first drawn disabled is just disabled"
    );
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.on = true;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(pending_frames(cx) > 0, "it fades in");
    advance(cx, HOVER * 3);
    assert_eq!(pending_frames(cx), 0, "and stops");
}

// ── Rows of a list ─────────────────────────────────────────────────────────────────────────────

struct Rows {
    list: Listing<&'static str>,
    items: Vec<&'static str>,
    rising: bool,
}

impl Rows {
    fn set(&mut self, items: &[&'static str], cx: &mut Context<Self>) {
        self.items = items.to_vec();
        cx.notify();
    }
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        self.list.sync(
            self.items.iter().map(|name| (name.to_string(), *name)),
            now,
            reduce,
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(self.list.rows().map(|row| {
                let item = Item::new(format!("item-{}", row.key), row.placement, div().h(px(40.)))
                    .gap(px(8.));
                if self.rising { item.rising() } else { item }
            }))
    }
}

fn rows<'a>(
    items: &[&'static str],
    cx: &'a mut TestAppContext,
) -> (gpui_kit::Entity<Rows>, &'a mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let items = items.to_vec();
    let (view, cx) = cx.add_window_view(move |_, _| Rows {
        list: Listing::default(),
        items,
        rising: false,
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL * 3);
    (view, cx)
}

fn change(view: &gpui_kit::Entity<Rows>, items: &[&'static str], cx: &mut VisualTestContext) {
    cx.update(|_, cx| view.update(cx, |rows, cx| rows.set(items, cx)));
    cx.update(|window, cx| window.render_frame(cx));
}

fn height(id: &str, cx: &mut VisualTestContext) -> Option<Pixels> {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).map(|found| found.bounds().size.height))
}

fn top(id: &str, cx: &mut VisualTestContext) -> Pixels {
    let id = id.to_owned();
    cx.update(|window, _| window.find(id).bounds().origin.y)
}

#[gpui_kit::test]
fn a_row_that_arrives_unfolds_to_its_height_and_the_list_stops_asking_for_frames(
    cx: &mut TestAppContext,
) {
    let (view, cx) = rows(&["a", "b"], cx);
    assert_eq!(height("item-a", cx), Some(px(40.)));
    change(&view, &["a", "b", "c"], cx);
    advance(cx, PANEL / 3);
    let part = height("item-c", cx).expect("the new row is drawn");
    assert!(
        px(0.) < part && part < px(40.),
        "part way after a third: {part:?}"
    );
    advance(cx, PANEL * 2);
    assert_eq!(height("item-c", cx), Some(px(40.)));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_rising_row_is_at_its_full_size_at_once_and_rises_into_place(cx: &mut TestAppContext) {
    let (view, cx) = rows(&["a", "b"], cx);
    cx.update(|_, cx| view.update(cx, |rows, _| rows.rising = true));
    change(&view, &["a", "b", "c"], cx);
    let first = top("item-c", cx);
    assert_eq!(
        height("item-c", cx),
        Some(px(40.)),
        "full size from its first frame, so that it can be clicked as it comes"
    );
    advance(cx, PANEL * 2);
    let settled = top("item-c", cx);
    assert_eq!(first - settled, px(RISE), "it rose {RISE} px into place");
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_row_that_goes_is_still_drawn_while_it_folds_away_and_then_is_gone(cx: &mut TestAppContext) {
    let (view, cx) = rows(&["a", "b", "c"], cx);
    let before = top("item-c", cx);
    change(&view, &["a", "c"], cx);
    advance(cx, PANEL / 3);
    let part = height("item-b", cx).expect("the removed row is still drawn");
    assert!(
        px(0.) < part && part < px(48.),
        "folding, part way: {part:?}"
    );
    let during = top("item-c", cx);
    assert!(
        px(40.) < before - during + px(41.) && during < before,
        "the row below moves up with the fold: {during:?} < {before:?}"
    );
    advance(cx, PANEL * 2);
    assert_eq!(height("item-b", cx), None, "gone once folded");
    assert_eq!(
        top("item-c", cx),
        before - px(48.),
        "and its gap went with it, with no jump at the end"
    );
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_row_that_moves_slides_from_where_it_was_to_its_place(cx: &mut TestAppContext) {
    let (view, cx) = rows(&["a", "b"], cx);
    let (a, b) = (top("item-a", cx), top("item-b", cx));
    change(&view, &["b", "a"], cx);
    advance(cx, PANEL / 3);
    let sliding = top("item-a", cx);
    assert!(
        a < sliding && sliding < b,
        "a is on its way down: {a:?} < {sliding:?} < {b:?}"
    );
    let rising = top("item-b", cx);
    assert!(
        a < rising && rising < b,
        "b is on its way up: {a:?} < {rising:?} < {b:?}"
    );
    advance(cx, PANEL * 2);
    assert_eq!((top("item-b", cx), top("item-a", cx)), (a, b));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_rising_row_that_moves_slides_too_as_the_tree_s_rows_do(cx: &mut TestAppContext) {
    // Rising from its first frame, as the tree's rows are: its height was never measured by a
    // fold.
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Rows {
        list: Listing::default(),
        items: vec!["a", "b"],
        rising: true,
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL * 3);
    let (a, b) = (top("item-a", cx), top("item-b", cx));
    change(&view, &["b", "a"], cx);
    advance(cx, PANEL / 3);
    let sliding = top("item-b", cx);
    // A row's own height and gap are how far it is from its place: not the gap alone.
    assert!(
        a + px(8.) < sliding && sliding < b,
        "b is on its way up, one row away: {a:?} < {sliding:?} < {b:?}"
    );
    advance(cx, PANEL * 2);
    assert_eq!((top("item-b", cx), top("item-a", cx)), (a, b));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn the_row_below_one_that_goes_does_not_slide_on_top_of_the_fold(cx: &mut TestAppContext) {
    let (view, cx) = rows(&["a", "b", "c"], cx);
    let before = top("item-c", cx);
    change(&view, &["b", "c"], cx);
    // Just after the removal the row below has not moved yet: the fold carries it, and nothing
    // else pushes it.
    advance(cx, Duration::from_millis(16));
    let at_once = top("item-c", cx);
    assert!(
        f32::from(at_once - before).abs() < 10.,
        "c has barely moved: {before:?} → {at_once:?}"
    );
    advance(cx, PANEL * 2);
}

#[gpui_kit::test]
fn with_reduced_motion_rows_arrive_and_go_at_once(cx: &mut TestAppContext) {
    let (view, cx) = rows(&["a", "b"], cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    change(&view, &["a", "b", "c"], cx);
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    cx.update(|window, cx| window.render_frame(cx));
    assert_eq!(height("item-c", cx), Some(px(40.)));
    change(&view, &["a", "c"], cx);
    cx.update(|window, cx| window.render_frame(cx));
    assert_eq!(height("item-b", cx), None);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_row_that_comes_back_before_it_is_gone_is_the_same_row(cx: &mut TestAppContext) {
    let (view, cx) = rows(&["a", "b"], cx);
    change(&view, &["a"], cx);
    advance(cx, PANEL / 3);
    change(&view, &["a", "b"], cx);
    advance(cx, PANEL * 2);
    assert_eq!(height("item-b", cx), Some(px(40.)));
}

// ── Blocks that come and go ────────────────────────────────────────────────────────────────────

struct Blocks {
    shown: bool,
    line: Option<SharedString>,
}

impl Render for Blocks {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().id("before").h(px(20.)).test_support())
            .child(
                presence(
                    "block",
                    self.shown,
                    div().id("content").h(px(60.)).test_support(),
                )
                .gap(px(10.)),
            )
            .child(
                presence_of("said", self.line.clone(), |line| {
                    div()
                        .id("line")
                        .h(px(20.))
                        .child(line)
                        .test_support()
                        .into_any_element()
                })
                .gap(px(10.)),
            )
            .child(div().id("after").h(px(20.)).test_support())
    }
}

fn blocks<'a>(
    shown: bool,
    line: Option<&str>,
    cx: &'a mut TestAppContext,
) -> (gpui_kit::Entity<Blocks>, &'a mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let line = line.map(|line| SharedString::from(line.to_owned()));
    let (view, cx) = cx.add_window_view(move |_, _| Blocks { shown, line });
    cx.update(|window, cx| window.render_frame(cx));
    (view, cx)
}

#[gpui_kit::test]
fn a_block_that_comes_unfolds_and_one_that_goes_folds_away_with_the_gap_around_it(
    cx: &mut TestAppContext,
) {
    let (view, cx) = blocks(false, None, cx);
    let closed = top("after", cx);
    assert!(
        height("content", cx).is_none(),
        "absent at rest: nothing is drawn"
    );

    cx.update(|_, cx| {
        view.update(cx, |blocks, cx| {
            blocks.shown = true;
            cx.notify()
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL / 3);
    let part = top("after", cx);
    assert!(
        closed < part && part < closed + px(70.),
        "what follows is pushed down part way: {closed:?} < {part:?}"
    );
    advance(cx, PANEL * 2);
    assert_eq!(top("after", cx), closed + px(70.));
    assert_eq!(pending_frames(cx), 0);

    cx.update(|_, cx| {
        view.update(cx, |blocks, cx| {
            blocks.shown = false;
            cx.notify()
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL / 3);
    let folding = top("after", cx);
    assert!(
        closed < folding && folding < closed + px(70.),
        "and pulled back: {folding:?}"
    );
    advance(cx, PANEL * 2);
    assert_eq!(top("after", cx), closed, "no jump of the gap at the end");
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_line_that_goes_away_is_still_drawn_with_its_last_words_while_it_folds(
    cx: &mut TestAppContext,
) {
    let (view, cx) = blocks(false, Some("Refused: no such folder"), cx);
    assert!(height("line", cx).is_some());
    cx.update(|_, cx| {
        view.update(cx, |blocks, cx| {
            blocks.line = None;
            cx.notify()
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL / 3);
    let part = cx.update(|window, _| {
        window
            .try_find("line")
            .map(|l| l.label().map(str::to_owned))
    });
    assert!(part.is_some(), "still drawn while it folds");
    advance(cx, PANEL * 2);
    assert!(height("line", cx).is_none());
    assert_eq!(pending_frames(cx), 0);
}

// ── Entrances ──────────────────────────────────────────────────────────────────────────────────

struct Entrances;

impl Render for Entrances {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(enter("first", 0, div().h(px(10.))))
            .child(enter("far", 40, div().h(px(10.))))
    }
}

#[gpui_kit::test]
fn an_entrance_waits_for_at_most_a_few_siblings_however_long_the_list(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Entrances);
    cx.update(|window, cx| window.render_frame(cx));
    let at_rest = top("far", cx) - px(RISE);
    advance(
        cx,
        STAGGER * MAX_STAGGER as u32 + ENTER + Duration::from_millis(32),
    );
    assert_eq!(
        top("far", cx),
        at_rest + px(RISE) - px(RISE),
        "the 41st sibling is in place after eight staggers, not forty"
    );
    assert_eq!(pending_frames(cx), 0);
}

struct Tinted {
    to: gpui_kit::Hsla,
    seen: Rc<Cell<gpui_kit::Hsla>>,
}

impl Render for Tinted {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.seen
            .set(flottille_ui::tint("tinted", self.to, PANEL, window, cx));
        div().size_full()
    }
}

#[gpui_kit::test]
fn a_colour_that_changes_moves_through_the_colours_between_and_stops(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (black, white) = (
        gpui_kit::rgb(0x000000).into(),
        gpui_kit::rgb(0xffffff).into(),
    );
    let seen = Rc::new(Cell::new(black));
    let shared = seen.clone();
    let (view, cx) = cx.add_window_view(move |_, _| Tinted {
        to: black,
        seen: shared,
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert_eq!(seen.get(), black, "first drawn, it is just there");

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.to = white;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL / 3);
    let part = seen.get().to_rgb().r;
    assert!(0.0 < part && part < 1.0, "between black and white: {part}");
    advance(cx, PANEL * 2);
    assert_eq!(seen.get(), white);
    assert_eq!(pending_frames(cx), 0);
}

// ── A line whose text changes ──────────────────────────────────────────────────────────────────

struct Said {
    text: SharedString,
}

impl Render for Said {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        changing("said-line", self.text.clone(), |text| {
            div()
                .id("said-text")
                .child(text)
                .test_support()
                .into_any_element()
        })
    }
}

#[gpui_kit::test]
fn a_line_whose_text_changes_fades_back_in_over_hover_and_then_stops_asking_for_frames(
    cx: &mut TestAppContext,
) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Said {
        text: "first".into(),
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, HOVER);
    assert_eq!(pending_frames(cx), 0, "a first text is simply there");

    cx.update(|_, cx| {
        view.update(cx, |said, cx| {
            said.text = "second".into();
            cx.notify()
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(pending_frames(cx) > 0, "the change asks for frames");
    advance(cx, HOVER / 2);
    assert!(pending_frames(cx) > 0, "and it is still moving half way");
    advance(cx, HOVER * 2);
    assert_eq!(pending_frames(cx), 0);
    assert!(cx.update(|window, _| window.find("said-text").visible()));
}

// ── A tree row whose title changes ─────────────────────────────────────────────────────────────

struct Titled {
    title: SharedString,
}

impl Render for Titled {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(240.))
            .child(flottille_ui::TreeRow::new("titled-row", self.title.clone()))
    }
}

#[gpui_kit::test]
fn a_tree_row_whose_title_changes_fades_to_the_new_one(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (view, cx) = cx.add_window_view(|_, _| Titled {
        title: "write the release notes".into(),
    });
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, HOVER);
    assert_eq!(pending_frames(cx), 0, "a first title is simply there");

    cx.update(|_, cx| {
        view.update(cx, |titled, cx| {
            titled.title = "Release notes".into();
            cx.notify()
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(pending_frames(cx) > 0, "the new title fades in");
    advance(cx, HOVER * 2);
    assert_eq!(pending_frames(cx), 0);
    assert_eq!(
        cx.update(|window, _| window.find("titled-row").label().map(str::to_owned)),
        Some("Release notes".to_owned())
    );
}

// ── Subpage ────────────────────────────────────────────────────────────────────────────────────

fn left(id: &str, cx: &mut VisualTestContext) -> Pixels {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find(id.to_string()).bounds().left()
    })
}

#[gpui_kit::test]
fn the_page_slides_in_and_the_list_out(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Settings::new());
    let list = left("feat-specs", cx);
    advance(cx, PANEL);
    assert_eq!(pending_frames(cx), 0, "the list at rest draws nothing");

    cx.update(|window, cx| window.click("feat-review", cx));
    cx.update(|window, cx| window.render_frame(cx));
    assert!(pending_frames(cx) > 0, "the page is on its way");
    advance(cx, PANEL / 3);
    let leaving = left("feat-specs", cx);
    let arriving = left("review-page", cx);
    advance(cx, PANEL * 2);
    let page = left("review-page", cx);
    assert!(
        leaving < list,
        "the list leaves to the left: {leaving:?} < {list:?}"
    );
    assert!(
        arriving > page,
        "the page comes in from the right: {arriving:?} > {page:?}"
    );
    cx.update(|window, _| {
        assert!(window.try_find("feat-specs").is_none(), "the list is gone");
    });
    assert_eq!(pending_frames(cx), 0, "nothing is drawn once it arrived");

    // Back is the reverse: the list comes in from the left.
    cx.update(|window, cx| window.click("feat-back", cx));
    advance(cx, PANEL / 3);
    let returning = left("feat-specs", cx);
    advance(cx, PANEL * 2);
    assert_eq!(left("feat-specs", cx), list, "it is where it was");
    assert!(
        returning < list,
        "the list comes back from the left: {returning:?} < {list:?}"
    );
    cx.update(|window, _| assert!(window.try_find("review-page").is_none()));
    // The row that was left keeps its hover while it goes, and lets go of it.
    advance(cx, PANEL * 2);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn reduced_motion_jumps_between_list_and_page(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_, cx) = cx.add_window_view(|_, _| Settings::new());
    let list = left("feat-specs", cx);

    cx.update(|window, cx| window.click("feat-review", cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    cx.update(|window, _| {
        assert!(window.try_find("feat-specs").is_none());
        assert!(window.try_find("review-page").is_some());
    });
    assert_eq!(pending_frames(cx), 0);

    cx.update(|window, cx| window.click("feat-back", cx));
    cx.run_until_parked();
    assert_eq!(left("feat-specs", cx), list);
    cx.update(|window, _| assert!(window.try_find("review-page").is_none()));
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn going_back_the_page_slides_out_and_fades_while_the_list_comes_back(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (_, cx) = cx.add_window_view(|_, _| Settings::new());
    let list = left("feat-specs", cx);
    cx.update(|window, cx| window.click("feat-review", cx));
    advance(cx, PANEL * 2);
    let page = left("feat-page", cx);
    let shown = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .find("feat-page")
                .value()
                .and_then(|v| v.parse::<f32>().ok())
        })
    };
    assert_eq!(shown(cx), Some(1.0), "at rest the page is fully there");

    cx.update(|window, cx| window.click("feat-back", cx));
    advance(cx, PANEL / 2);
    cx.update(|window, _| {
        assert!(
            window.try_find("review-page").is_some(),
            "half way the page is still drawn"
        );
    });
    let leaving = left("feat-page", cx);
    let fading = shown(cx).expect("the page says how far it is");
    assert!(
        leaving > page && leaving < page + px(32.),
        "it slides out to the right: {page:?} < {leaving:?} < {:?}",
        page + px(32.)
    );
    assert!(0.0 < fading && fading < 1.0, "and fades: {fading}");
    let coming = left("feat-specs", cx);
    assert!(coming < list, "while the list comes back from the left");

    advance(cx, PANEL * 2);
    cx.update(|window, _| {
        assert!(
            window.try_find("feat-page").is_none(),
            "at the end it is gone"
        );
        assert!(window.try_find("review-page").is_none());
    });
    assert_eq!(left("feat-specs", cx), list);
    assert_eq!(pending_frames(cx), 0);
}
