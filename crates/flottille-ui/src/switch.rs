//! `Switch`: one thing on or off, its label drawn beside it and an optional hint under the label.
//! A click on the track or on the words toggles it.

use std::sync::Arc;

use gpui_kit::{
    App, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::color::mix;
use crate::interact::{fade, feel, wire};
use crate::motion::{HOVER, Motion, PANEL};
use crate::theme::palette;

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    on: bool,
    label: SharedString,
    hint: Option<SharedString>,
    disabled: bool,
    on_click: Option<OnClick>,
}

impl Switch {
    /// `label` says what it switches, drawn beside it: the switch `id`, its track `<id>-track`,
    /// its knob `<id>-knob`, its label `<id>-label` and its hint `<id>-hint`.
    pub fn new(id: impl Into<ElementId>, on: bool, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            on,
            label: label.into(),
            hint: None,
            disabled: false,
            on_click: None,
        }
    }

    /// One line under the label: what being on does.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
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
        let off_track = p.card2;
        let on_track = mix(p.acc, p.on_acc, 0.08 * now.hover);
        let track = mix(off_track, on_track, at);
        let knob = mix(p.faint, p.on_acc, at);
        // The border is always there so the knob's place does not depend on the state; it
        // melts into the track as the switch turns on.
        let border = mix(mix(p.line2, p.focus_line, now.hover), on_track, at);
        let part = |name: &'static str| match &self.id {
            ElementId::Name(text) => ElementId::Name(format!("{text}-{name}").into()),
            other => ElementId::NamedChild(Arc::new(other.clone()), name.into()),
        };
        let track = div()
            .id(part("track"))
            .relative()
            .flex_none()
            .mt(px(1.))
            .w(px(30.))
            .h(px(18.))
            .rounded_full()
            .bg(track)
            .border_1()
            .border_color(border)
            .child(
                div()
                    .id(part("knob"))
                    .absolute()
                    .top(px(2.))
                    .left(px(2. + 12. * at))
                    .size(px(12.))
                    .rounded_full()
                    .bg(knob)
                    .test_support(),
            )
            .test_support();
        let words = div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                div()
                    .id(part("label"))
                    .aria_label(self.label.clone())
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child(self.label.clone())
                    .test_support(),
            )
            .children(self.hint.map(|hint| {
                div()
                    .id(part("hint"))
                    .aria_label(hint.clone())
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(p.faint)
                    .child(hint)
                    .test_support()
            }));
        let mut switch = div()
            .id(self.id.clone())
            .aria_label(self.label.clone())
            .aria_selected(self.on)
            .flex()
            .items_start()
            .gap(px(10.))
            .py(px(6.))
            .child(track)
            .child(words);
        // Enabled or not is a state: it fades over HOVER instead of switching.
        let enabled = fade(part("enabled"), !self.disabled, HOVER, window, cx);
        if enabled < 1.0 {
            switch = switch.opacity(0.35 + 0.65 * enabled);
        }
        if !self.disabled
            && let Some(on_click) = self.on_click
        {
            switch = switch.cursor_pointer().on_click(on_click);
        }
        wire(switch, state, false).test_support()
    }
}
