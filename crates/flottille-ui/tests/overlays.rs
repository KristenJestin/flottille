//! What floats over the page, on GPUI's test platform: a dropdown's list and a modal dialog.

mod common;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::{ENTER, HOVER, MODAL};
use flottille_ui::{Align, Dropdown, MenuItem, Modal};
use gpui_kit::base::input::{Input, InputState};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, TestAppContext,
    TestSupportExt as _, VisualTestContext, Window, div, px, size,
};

/// A dropdown at the top or the bottom of the window, and a block drawn after it where its list
/// opens.
struct Page {
    bottom: bool,
    open: bool,
    picked: Option<&'static str>,
    under: usize,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let dropdown = Dropdown::new("picker", "effort")
            .open(self.open)
            .align(Align::End)
            .width(px(200.))
            .on_toggle({
                let this = this.clone();
                move |_, cx| {
                    this.update(cx, |p, cx| {
                        p.open = !p.open;
                        cx.notify();
                    })
                    .ok();
                }
            })
            .items(["low", "medium", "high"].map(|effort| {
                let this = this.clone();
                MenuItem::new(format!("choice-{effort}"), effort).on_click(move |_, cx| {
                    this.update(cx, |p, cx| {
                        p.picked = Some(effort);
                        p.open = false;
                        cx.notify();
                    })
                    .ok();
                })
            }));
        let row = div().h(px(28.)).flex().justify_end().child(dropdown);
        let under = div()
            .id("under")
            .h(px(300.))
            .w_full()
            .bg(gpui_kit::red())
            .on_click(cx.listener(|this, _, _, cx| {
                this.under += 1;
                cx.notify();
            }))
            .test_support();
        // At the top, the block comes after the picker, where its list opens; at the bottom, the
        // picker is the last thing in the window.
        let page = div().size_full().flex().flex_col();
        if self.bottom {
            page.justify_end()
                .child(under)
                .child(row)
                .child(div().h(px(4.)))
        } else {
            page.child(row).child(under)
        }
    }
}

fn page(bottom: bool, cx: &mut TestAppContext) -> (Entity<Page>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (page, cx) = cx.add_window_view(move |_, _| Page {
        bottom,
        open: false,
        picked: None,
        under: 0,
    });
    cx.simulate_resize(size(px(600.), px(500.)));
    (page, cx)
}

#[gpui_kit::test]
fn an_open_dropdown_is_above_the_elements_under_it_and_takes_their_clicks(cx: &mut TestAppContext) {
    let (page, cx) = page(false, cx);
    cx.update(|window, cx| window.click("picker", cx));
    assert!(page.read_with(cx, |p, _| p.open));
    let (menu, under) = cx.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find("picker-menu").bounds(),
            window.find("under").bounds(),
        )
    });
    assert!(menu.intersects(&under), "the list opens over the block");
    cx.update(|window, cx| window.click("choice-high", cx));
    page.read_with(cx, |p, _| {
        assert_eq!(p.picked, Some("high"), "the entry over the block is picked");
        assert_eq!(p.under, 0, "the block under it never saw the click");
        assert!(!p.open);
    });
}

#[gpui_kit::test]
fn a_dropdown_with_no_room_below_opens_upward_inside_the_window(cx: &mut TestAppContext) {
    let (_, cx) = page(true, cx);
    cx.update(|window, cx| window.click("picker", cx));
    advance(cx, ENTER * 2);
    let (menu, button, viewport) = cx.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find("picker-menu").bounds(),
            window.find("picker").bounds(),
            window.viewport_size(),
        )
    });
    assert!(menu.bottom() <= button.top(), "{menu:?} above {button:?}");
    assert!(menu.top() >= px(0.) && menu.left() >= px(0.));
    assert!(menu.right() <= viewport.width && menu.bottom() <= viewport.height);
}

#[gpui_kit::test]
fn a_click_outside_an_open_dropdown_closes_it_and_goes_no_further(cx: &mut TestAppContext) {
    let (page, cx) = page(false, cx);
    cx.update(|window, cx| window.click("picker", cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        let under = window.find("under").bounds();
        let corner = gpui_kit::point(under.size.width - px(10.), under.size.height - px(10.));
        window.click_at("under", corner, cx)
    });
    page.read_with(cx, |p, _| {
        assert!(!p.open);
        assert_eq!(p.under, 0);
        assert_eq!(p.picked, None);
    });
}

/// Where the dropdown's list is, after the frame its state asked for.
fn list_top(cx: &mut VisualTestContext) -> gpui_kit::Pixels {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find("picker-menu").bounds().origin.y
    })
}

#[gpui_kit::test]
fn a_dropdown_rises_into_place_when_opened_and_stops_asking_for_frames(cx: &mut TestAppContext) {
    let (page, cx) = page(false, cx);
    cx.update(|window, cx| window.click("picker", cx));
    assert!(page.read_with(cx, |p, _| p.open));
    let first = list_top(cx);
    advance(cx, ENTER / 3);
    let part_way = list_top(cx);
    advance(cx, ENTER * 2);
    let rested = list_top(cx);

    assert!(first > part_way, "it rises: {first:?} then {part_way:?}");
    assert!(
        part_way > rested,
        "and keeps rising: {part_way:?} then {rested:?}"
    );
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn once it rests");
}

#[gpui_kit::test]
fn a_dropdown_stays_drawn_until_its_close_ends_then_goes(cx: &mut TestAppContext) {
    let (page, cx) = page(false, cx);
    cx.update(|window, cx| window.click("picker", cx));
    advance(cx, ENTER * 2);
    let rested = list_top(cx);

    // Closed by its button, or by a click outside: both start the same close.
    cx.update(|window, cx| window.click("picker", cx));
    assert!(!page.read_with(cx, |p, _| p.open));
    advance(cx, HOVER / 3);
    cx.update(|window, _| {
        assert!(
            window.try_find("picker-menu").is_some(),
            "still on screen, leaving"
        );
        assert!(
            window.try_find("picker-backdrop").is_none(),
            "nothing over the window takes a click while it leaves"
        );
    });
    assert!(list_top(cx) > rested, "it sinks as it fades");

    advance(cx, HOVER * 3);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("picker-menu").is_none(),
            "gone once the close ended"
        );
    });
    assert_eq!(pending_frames(cx), 0);

    // And it opens again from the start.
    cx.update(|window, cx| window.click("picker", cx));
    assert!(list_top(cx) > rested);
}

#[gpui_kit::test]
fn picking_an_entry_closes_the_dropdown_with_the_same_animation(cx: &mut TestAppContext) {
    let (page, cx) = page(false, cx);
    cx.update(|window, cx| window.click("picker", cx));
    advance(cx, ENTER * 2);
    cx.update(|window, cx| window.click("choice-low", cx));
    page.read_with(cx, |p, _| assert_eq!(p.picked, Some("low")));
    advance(cx, HOVER / 3);
    cx.update(|window, _| assert!(window.try_find("picker-menu").is_some()));
    advance(cx, HOVER * 3);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("picker-menu").is_none());
    });
}

#[gpui_kit::test]
fn with_reduced_motion_a_dropdown_comes_and_goes_at_once(cx: &mut TestAppContext) {
    let (_, cx) = page(false, cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.update(|window, cx| window.click("picker", cx));
    let first = list_top(cx);
    assert_eq!(pending_frames(cx), 0);
    advance(cx, ENTER * 2);
    assert_eq!(first, list_top(cx), "it was in place at the first frame");

    cx.update(|window, cx| window.click("picker", cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("picker-menu").is_none());
    });
}

/// A page with a button under the modal, and the modal holding one field.
struct Dialog {
    open: bool,
    closed: usize,
    field: Entity<InputState>,
    under: usize,
}

impl Render for Dialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                div()
                    .id("under")
                    .size_full()
                    .on_click(cx.listener(|this, _, _, _| this.under += 1))
                    .test_support(),
            )
            .child(
                Modal::new("dialog", "New workspace")
                    .open(self.open)
                    .child(
                        div()
                            .id("dialog-field")
                            .h(px(30.))
                            .child(Input::new(&self.field)),
                    )
                    .action(div().id("dialog-action").child("Create"))
                    .on_close({
                        let this = cx.entity().downgrade();
                        move |_, cx| {
                            this.update(cx, |d, cx| {
                                d.open = false;
                                d.closed += 1;
                                cx.notify();
                            })
                            .ok();
                        }
                    }),
            )
    }
}

fn dialog(cx: &mut TestAppContext) -> (Entity<Dialog>, &mut VisualTestContext) {
    init(cx, Desktop::new(false).env());
    let (dialog, cx) = cx.add_window_view(|window, cx| Dialog {
        open: false,
        closed: 0,
        field: cx.new(|cx| InputState::new(window, cx)),
        under: 0,
    });
    cx.simulate_resize(size(px(900.), px(700.)));
    (dialog, cx)
}

/// Opens it and lets its entrance end.
fn open(dialog: &Entity<Dialog>, cx: &mut VisualTestContext) {
    start_opening(dialog, cx);
    advance(cx, MODAL * 2);
}

/// Opens it and draws the first frame of its entrance.
fn start_opening(dialog: &Entity<Dialog>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        dialog.update(cx, |d, cx| {
            d.open = true;
            let focus = d.field.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            cx.notify();
        });
        window.render_frame(cx);
    });
}

#[gpui_kit::test]
fn a_modal_is_centred_over_the_page_and_escape_closes_it(cx: &mut TestAppContext) {
    let (dialog, cx) = dialog(cx);
    open(&dialog, cx);
    let (panel, viewport) =
        cx.update(|window, _| (window.find("dialog").bounds(), window.viewport_size()));
    let centre = panel.center();
    assert!((centre.x - viewport.width / 2.).abs() < px(1.));
    assert!((centre.y - viewport.height / 2.).abs() < px(1.));
    cx.update(|window, cx| window.input("a", cx));
    assert_eq!(
        dialog.read_with(cx, |d, cx| d.field.read(cx).value().to_string()),
        "a"
    );
    cx.update(|window, cx| window.press("escape", cx));
    dialog.read_with(cx, |d, _| {
        assert!(!d.open);
        assert_eq!(d.closed, 1);
    });
}

#[gpui_kit::test]
fn a_modal_closes_on_cancel_and_on_a_click_outside_which_goes_no_further(cx: &mut TestAppContext) {
    let (dialog, cx) = dialog(cx);
    open(&dialog, cx);
    cx.update(|window, cx| window.click("dialog-cancel", cx));
    assert_eq!(dialog.read_with(cx, |d, _| d.closed), 1);
    open(&dialog, cx);
    cx.update(|window, cx| window.click_at("dialog-scrim", gpui_kit::point(px(5.), px(5.)), cx));
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.closed, 2);
        assert_eq!(d.under, 0, "the page under the scrim never saw the click");
    });
}

/// How far the panel is from where it rests, which is its place once the entrance has ended.
fn panel_top(cx: &mut VisualTestContext) -> gpui_kit::Pixels {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find("dialog").bounds().origin.y
    })
}

#[gpui_kit::test]
fn a_modal_and_its_scrim_move_in_when_opened_and_stop_asking_for_frames(cx: &mut TestAppContext) {
    let (dialog, cx) = dialog(cx);
    start_opening(&dialog, cx);
    let first = panel_top(cx);
    advance(cx, MODAL / 2);
    let part_way = panel_top(cx);
    advance(cx, MODAL * 2);
    let rested = panel_top(cx);

    assert!(first > part_way, "it rises: {first:?} then {part_way:?}");
    assert!(
        part_way > rested,
        "and keeps rising: {part_way:?} then {rested:?}"
    );
    assert_eq!(pending_frames(cx), 0, "nothing is redrawn once it rests");
    cx.update(|window, _| assert!(window.find("dialog-scrim").visible()));
}

#[gpui_kit::test]
fn a_modal_stays_until_its_close_animation_ends_then_goes(cx: &mut TestAppContext) {
    let (dialog, cx) = dialog(cx);
    open(&dialog, cx);
    let rested = panel_top(cx);

    cx.update(|window, cx| window.press("escape", cx));
    assert_eq!(dialog.read_with(cx, |d, _| d.closed), 1);
    advance(cx, MODAL / 2);
    cx.update(|window, _| {
        assert!(
            window.try_find("dialog").is_some(),
            "still on screen, leaving"
        );
        assert!(window.try_find("dialog-scrim").is_some());
    });
    assert!(panel_top(cx) > rested, "it moves out as it fades");

    advance(cx, MODAL * 2);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("dialog").is_none(),
            "gone once the animation ended"
        );
        assert!(window.try_find("dialog-scrim").is_none());
    });
    assert_eq!(pending_frames(cx), 0);

    // And it opens again from the start.
    open(&dialog, cx);
    assert_eq!(panel_top(cx), rested);
}

#[gpui_kit::test]
fn with_reduced_motion_a_modal_comes_and_goes_at_once(cx: &mut TestAppContext) {
    let (dialog, cx) = dialog(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    start_opening(&dialog, cx);
    let first = panel_top(cx);
    assert_eq!(pending_frames(cx), 0);
    advance(cx, MODAL * 2);
    assert_eq!(first, panel_top(cx), "it was in place at the first frame");

    cx.update(|window, cx| window.press("escape", cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("dialog").is_none());
    });
}

/// A modal with forty rows of a body, in a window too small for them.
struct Tall {
    open: bool,
}

impl Render for Tall {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Modal::new("tall", "Repository api")
            .sentence("In the project folder.")
            .open(self.open)
            .children((0..40).map(|row| {
                div()
                    .id(SharedString::from(format!("tall-row-{row}")))
                    .h(px(30.))
                    .child(format!("row {row}"))
                    .test_support()
            }))
            .action(div().id("tall-action").child("Save"))
    }
}

#[gpui_kit::test]
fn a_modal_taller_than_the_window_scrolls_its_body_and_keeps_its_title_and_buttons(
    cx: &mut TestAppContext,
) {
    init(cx, Desktop::new(false).env());
    let (_tall, cx) = cx.add_window_view(|_, _| Tall { open: true });
    cx.simulate_resize(size(px(900.), px(500.)));
    advance(cx, MODAL * 2);

    let (viewport, panel, title, cancel, last) = cx.update(|window, _| {
        (
            window.viewport_size(),
            window.find("tall").bounds(),
            window.find("tall-title").bounds(),
            window.find("tall-cancel").bounds(),
            window.find("tall-row-39").bounds(),
        )
    });
    assert!(
        panel.bottom() <= viewport.height && panel.top() >= px(0.),
        "the dialog fits the window: {panel:?} in {viewport:?}"
    );
    assert!(
        title.top() >= panel.top() && cancel.bottom() <= panel.bottom(),
        "title and buttons are inside it: {title:?} {cancel:?} in {panel:?}"
    );
    assert!(
        last.top() > panel.bottom(),
        "the last row is below, clipped"
    );

    // The wheel over the body moves the rows, not the title or the buttons.
    cx.update(|window, cx| {
        window.scroll(
            "tall-row-3",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-3000.))),
            cx,
        )
    });
    let (title_after, cancel_after, last_after, panel_after) = cx.update(|window, _| {
        (
            window.find("tall-title").bounds(),
            window.find("tall-cancel").bounds(),
            window.find("tall-row-39").bounds(),
            window.find("tall").bounds(),
        )
    });
    assert_eq!(title_after, title, "the title stays");
    assert_eq!(cancel_after, cancel, "the buttons stay");
    assert_eq!(panel_after, panel);
    assert!(
        last_after.bottom() <= cancel_after.top(),
        "the last row came into view above the buttons: {last_after:?}"
    );
}

#[gpui_kit::test]
fn a_short_modal_does_not_scroll_and_keeps_its_height(cx: &mut TestAppContext) {
    let (dialog, cx) = dialog(cx);
    open(&dialog, cx);
    let (panel, title) = cx.update(|window, _| {
        (
            window.find("dialog").bounds(),
            window.find("dialog-title").bounds(),
        )
    });
    assert!(panel.size.height < px(200.), "{panel:?}");
    cx.update(|window, cx| {
        window.scroll(
            "dialog-title",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-300.))),
            cx,
        )
    });
    assert_eq!(
        cx.update(|window, _| window.find("dialog-title").bounds()),
        title
    );
}
