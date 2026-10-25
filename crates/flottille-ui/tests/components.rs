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
