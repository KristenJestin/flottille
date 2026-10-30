//! `Drawer`: a panel sliding from the left or right edge over a dimmed scrim, closed by the scrim
//! or by a choice made inside it (a narrow window's tree and rail), or, made with
//! [`Drawer::detail`], the detail of a run: 520 px from the right edge, over the page without a
//! scrim, closed by Escape or its own close button.

use std::sync::Arc;

use gpui_kit::{
    AnyView, App, BoxShadow, Context, ElementId, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Render, StatefulInteractiveElement, Styled,
    TestSupportExt, Window, div, hsla, point, px,
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
    /// A detail over the page, without a scrim, which Escape closes: its focus.
    detail: Option<FocusHandle>,
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
            detail: None,
        }
    }

    /// A closed detail drawer holding `content`: 520 px from the right edge, over the page with
    /// no scrim, its panel `<id>-panel`. [`Drawer::show`] opens it with the focus, so that Escape
    /// closes it.
    pub fn detail(
        id: impl Into<ElementId>,
        content: impl Into<AnyView>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            detail: Some(cx.focus_handle()),
            ..Self::new(id, Side::Right, px(520.), content)
        }
    }

    /// Opens it and gives it the focus: Escape closes it.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = &self.detail {
            window.focus(focus, cx);
        }
        self.set(true, cx);
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
        if t == 0.0 && !self.open {
            // Closed: nothing drawn, nothing in the way of the pointer.
            return root.invisible().test_support();
        }
        let p = palette(cx);
        if let Some(focus) = self.detail.clone() {
            return self.detail_panel(focus, t, cx).test_support();
        }
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

/// How far a detail drawer travels while it slides in: it comes from a little to the right,
/// fading in.
const DETAIL_SLIDE: f32 = 24.;

impl Drawer {
    fn detail_panel(
        &mut self,
        focus: FocusHandle,
        t: f32,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let close = move |_: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.close(cx)).ok();
        };
        if self.open {
            open_overlay(self.id.clone(), close.clone(), cx);
        }
        let panel_id = match &self.id {
            ElementId::Name(name) => ElementId::Name(format!("{name}-panel").into()),
            other => ElementId::NamedChild(Arc::new(other.clone()), "panel".into()),
        };
        let panel = div()
            .id(panel_id)
            .track_focus(&focus)
            .occlude()
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(-DETAIL_SLIDE * (1.0 - t)))
            .w(self.width)
            .max_w_full()
            .opacity(t)
            .bg(p.card)
            .border_l_1()
            .border_color(p.line2)
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.5),
                offset: point(px(-24.), px(0.)),
                blur_radius: px(60.),
                spread_radius: px(0.),
                inset: false,
            }])
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.open && event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close(cx);
                }
            }))
            .child(self.content.clone())
            .test_support();
        // No scrim: the page stays where it is, and takes its clicks.
        div()
            .id(self.id.clone())
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(self.width)
            .max_w_full()
            .child(dismiss_on_navigate(panel, close))
    }
}
