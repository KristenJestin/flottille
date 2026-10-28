//! `Switch`: on or off, toggled by a click; the settings page's providers.

use std::sync::Arc;

use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::color::mix;
use crate::interact::{feel, wire};
use crate::motion::{Motion, PANEL};
use crate::theme::palette;

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    on: bool,
    label: SharedString,
    disabled: bool,
    on_click: Option<OnClick>,
}

impl Switch {
    /// `label` says what it switches (its accessible name).
    pub fn new(id: impl Into<ElementId>, on: bool, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            on,
            label: label.into(),
            disabled: false,
            on_click: None,
        }
    }

    /// Faded, and a click does nothing.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let (state, now) = feel(&self.id, window, cx);
        // 0 is off, 1 is on: the knob slides and the colours fade along it.
        let slide = window.use_keyed_state(
            ElementId::NamedChild(Arc::new(self.id.clone()), "slide".into()),
            cx,
            |_, _| Motion::settled(if self.on { 1.0 } else { 0.0 }),
        );
        let t = cx.background_executor().now();
        let target = if self.on { 1.0 } else { 0.0 };
        if slide.read(cx).target() != target {
            let reduce = cx.reduce_motion();
            slide.update(cx, |m, _| m.animate(target, PANEL, t, reduce));
        }
        let motion = *slide.read(cx);
        if motion.is_running(t) {
            window.request_animation_frame();
        }
        let at = motion.value(t);
        let off_track = mix(p.card2, p.hover, now.hover);
        let on_track = mix(p.acc, p.on_acc, 0.08 * now.hover);
        let track = mix(off_track, on_track, at);
        let knob = mix(p.faint, p.on_acc, at);
        // The border is always there so the knob's place does not depend on the state; it
        // melts into the track as the switch turns on.
        let border = mix(p.line2, on_track, at);
        let knob_id = match &self.id {
            ElementId::Name(name) => ElementId::Name(format!("{name}-knob").into()),
            other => ElementId::NamedChild(Arc::new(other.clone()), "knob".into()),
        };
        let mut switch = div()
            .id(self.id.clone())
            .aria_label(self.label.clone())
            .relative()
            .flex_none()
            .w(px(30.))
            .h(px(18.))
            .rounded_full()
            .bg(track)
            .border_1()
            .border_color(border)
            .child(
                div()
                    .id(knob_id)
                    .absolute()
                    .top(px(2.))
                    .left(px(2. + 12. * at))
                    .size(px(12.))
                    .rounded_full()
                    .bg(knob)
                    .test_support(),
            );
        if self.disabled {
            switch = switch.opacity(0.35);
        } else if let Some(on_click) = self.on_click {
            switch = switch.cursor_pointer().on_click(on_click);
        }
        wire(switch, state, false).test_support()
    }
}
