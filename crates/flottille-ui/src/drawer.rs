//! `Drawer`: a panel sliding from the left or right edge over a dimmed scrim, closed by the scrim
//! or by a choice made inside it.

use gpui_kit::{
    AnyView, App, BoxShadow, Context, ElementId, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, TestSupportExt, Window, div,
    hsla, point, px,
};

use crate::interact::{dismiss_on_navigate, open_overlay};
use crate::motion::{Motion, PANEL};
use crate::theme::palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Emitted when the drawer closes, whether by the scrim or by [`Drawer::close`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

pub struct Drawer {
    id: ElementId,
    side: Side,
    width: Pixels,
    open: bool,
    slide: Motion,
    content: AnyView,
}

impl EventEmitter<Closed> for Drawer {}

impl Drawer {
    /// A closed drawer of `width` (280–290 px) holding `content`.
    pub fn new(
        id: impl Into<ElementId>,
        side: Side,
        width: Pixels,
        content: impl Into<AnyView>,
    ) -> Self {
        Self {
            id: id.into(),
            side,
            width,
            open: false,
            slide: Motion::settled(0.0),
            content: content.into(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.set(true, cx);
    }

    /// Closes it: the scrim does this, and so should a choice made in the drawer.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.set(false, cx);
            cx.emit(Closed);
        }
    }

    fn set(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        let now = cx.background_executor().now();
        self.slide
            .animate(if open { 1.0 } else { 0.0 }, PANEL, now, cx.reduce_motion());
        cx.notify();
    }
}

impl Render for Drawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        if self.slide.is_running(now) {
            window.request_animation_frame();
        } else {
            self.slide.settle(now);
        }
        let t = self.slide.value(now);
        let root = div().id(self.id.clone()).absolute().inset_0();
        if t == 0.0 {
            // Closed: nothing drawn, nothing in the way of the pointer.
            return root.invisible().test_support();
        }
        let p = palette(cx);
        let offset = -self.width * (1.0 - t);
        let shadow_x = match self.side {
            Side::Left => px(20.),
            Side::Right => px(-20.),
        };
        let panel = div()
            .id("drawer-panel")
            .occlude()
            .absolute()
            .top_0()
            .bottom_0()
            .w(self.width)
            .bg(p.side)
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.8),
                offset: point(shadow_x, px(0.)),
                blur_radius: px(40.),
                spread_radius: px(-20.),
                inset: false,
            }])
            .child(self.content.clone());
        let panel = match self.side {
            Side::Left => panel.left(offset).border_r_1().border_color(p.line),
            Side::Right => panel.right(offset).border_l_1().border_color(p.line),
        };
        let this = cx.entity().downgrade();
        let close = move |_: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.close(cx)).ok();
        };
        if self.open {
            open_overlay(self.id.clone(), close.clone(), cx);
        }
        let scrim = div()
            .id("drawer-scrim")
            .absolute()
            .inset_0()
            .bg(p.scrim.opacity(t))
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .test_support();
        root.child(dismiss_on_navigate(scrim, close.clone()))
            .child(dismiss_on_navigate(panel, close))
            .test_support()
    }
}
