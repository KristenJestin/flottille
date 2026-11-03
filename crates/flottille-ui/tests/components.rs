//! Components that answer the pointer, on GPUI's test platform.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use common::{Desktop, advance, init, pending_frames};
use flottille_ui::motion::PANEL;
use flottille_ui::{Drawer, Side};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px,
};

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

struct Host {
    drawer: Entity<Drawer>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().relative().child(self.drawer.clone())
    }
}

#[gpui_kit::test]
fn a_drawer_is_closed_by_its_scrim(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (host, cx) = cx.add_window_view(|_, cx| {
        let content = cx.new(|_| Empty);
        Host {
            drawer: cx.new(|_| Drawer::new("drawer", Side::Right, px(290.), content)),
        }
    });
    let drawer = host.read_with(cx, |h, _| h.drawer.clone());
    let closed = Rc::new(Cell::new(0));
    let seen = closed.clone();
    cx.update(|_, cx| {
        cx.subscribe(&drawer, move |_, _: &flottille_ui::drawer::Closed, _| {
            seen.set(seen.get() + 1)
        })
        .detach();
    });

    cx.update(|_, cx| drawer.update(cx, |d, cx| d.open(cx)));
    advance(cx, PANEL * 2);
    cx.update(|window, cx| window.click("drawer-scrim", cx));
    assert!(!drawer.read_with(cx, |d, _| d.is_open()));
    assert_eq!(closed.get(), 1, "closing is told");
    advance(cx, PANEL * 2);
    cx.update(|window, cx| window.render_frame(cx));
    assert!(
        cx.update(|w, _| w.try_find("drawer-scrim")).is_none(),
        "nothing left in the way"
    );
    assert_eq!(pending_frames(cx), 0);
}

struct Toggled {
    on: bool,
}

impl Render for Toggled {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            flottille_ui::Switch::new("sw", self.on, "Offline").on_click(cx.listener(
                |this, _, _, cx| {
                    this.on = !this.on;
                    cx.notify();
                },
            )),
        )
    }
}

/// The knob's distance from the track's left edge, as drawn in the next frame.
fn knob_offset(cx: &mut gpui_kit::VisualTestContext) -> gpui_kit::Pixels {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find("sw-knob").bounds().origin.x - window.find("sw").bounds().origin.x
    })
}

#[gpui_kit::test]
fn a_switch_slides_its_knob_to_its_place_over_frames(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    let (host, cx) = cx.add_window_view(|_, _| Toggled { on: false });
    let off = knob_offset(cx);

    cx.update(|window, cx| window.click("sw", cx));
    assert!(host.read_with(cx, |h, _| h.on));
    cx.update(|window, cx| window.render_frame(cx));
    advance(cx, PANEL / 3);
    let sliding = knob_offset(cx);
    advance(cx, PANEL * 2);
    let on = knob_offset(cx);
    assert!(
        off < sliding && sliding < on,
        "the knob is part way after a third of the time: {off:?} < {sliding:?} < {on:?}"
    );
    assert_eq!(pending_frames(cx), 0, "nothing is drawn once it arrived");

    // And back, interrupted half way: it starts again from where it is.
    cx.update(|window, cx| window.click("sw", cx));
    advance(cx, PANEL / 3);
    let back = knob_offset(cx);
    assert!(
        off < back && back < on,
        "on its way back: {off:?} < {back:?} < {on:?}"
    );
    advance(cx, PANEL * 2);
    assert_eq!(knob_offset(cx), off);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn with_reduced_motion_a_switch_jumps(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_, cx) = cx.add_window_view(|_, _| Toggled { on: false });
    let off = knob_offset(cx);

    cx.update(|window, cx| window.click("sw", cx));
    cx.run_until_parked();
    let on = knob_offset(cx);
    assert!(
        on > off,
        "the knob is at its place at once: {off:?} → {on:?}"
    );
    assert_eq!(pending_frames(cx), 0);
}
