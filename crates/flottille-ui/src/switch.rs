//! `Switch`: on or off, toggled by a click; the settings page's providers.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::color::mix;
use crate::interact::{feel, wire};
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
        let (track, knob) = if self.on {
            (mix(p.acc, p.on_acc, 0.08 * now.hover), p.on_acc)
        } else {
            (mix(p.card2, p.hover, now.hover), p.faint)
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
            .when(!self.on, |d| d.border_1().border_color(p.line2))
            .child(
                div()
                    .absolute()
                    .top(px(if self.on { 3. } else { 2. }))
                    .when(self.on, |d| d.right(px(3.)))
                    .when(!self.on, |d| d.left(px(2.)))
                    .size(px(12.))
                    .rounded_full()
                    .bg(knob),
            );
        if self.disabled {
            switch = switch.opacity(0.35);
        } else if let Some(on_click) = self.on_click {
            switch = switch.cursor_pointer().on_click(on_click);
        }
        wire(switch, state, false).test_support()
    }
}
