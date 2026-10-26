//! What floats over the page, on GPUI's test platform: a dropdown's list and a modal dialog.

mod common;

use common::{Desktop, init};
use flottille_ui::{Align, Dropdown, MenuItem, Modal};
use gpui_kit::base::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement, Render, StatefulInteractiveElement as _, Styled, TestAppContext,
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
            .when(self.open, |d| {
                d.child(
                    Modal::new("dialog", "New workspace")
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
            })
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

fn open(dialog: &Entity<Dialog>, cx: &mut VisualTestContext) {
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
