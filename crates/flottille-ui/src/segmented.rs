//! `Segmented`: a small control of two or more choices, one of them chosen (the composer's
//! Ask | Auto). The chosen one is a highlight that slides to the one clicked over [`PANEL`], the
//! inks fade along it, and each segment fades its hover.

use std::sync::Arc;

use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::color::mix;
use crate::interact::hoverable;
use crate::motion::{Motion, PANEL};
use crate::theme::palette;

type OnChoose = Box<dyn Fn(usize, &mut Window, &mut App) + 'static>;

/// How wide a segment is: the same for all, so that the highlight slides by whole segments.
const WIDTH: f32 = 42.;
const HEIGHT: f32 = 24.;
const INSET: f32 = 2.;

#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    label: SharedString,
    options: Vec<(SharedString, SharedString)>,
    chosen: usize,
    disabled: bool,
    on_choose: Option<OnChoose>,
}

impl Segmented {
    /// `label` says what it chooses (its accessible name). Each option is `(key, words)`: its
    /// segment's id is `{id}-{key}`.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        options: impl IntoIterator<Item = (&'static str, &'static str)>,
        chosen: usize,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            options: options
                .into_iter()
                .map(|(key, words)| (key.into(), words.into()))
                .collect(),
            chosen,
            disabled: false,
            on_choose: None,
        }
    }

    /// Faded, and a click does nothing.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the index of the segment clicked, when it is not the chosen one.
    pub fn on_choose(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_choose = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let slide = window.use_keyed_state(
            ElementId::NamedChild(Arc::new(self.id.clone()), "slide".into()),
            cx,
            |_, _| Motion::settled(self.chosen as f32),
        );
        let now = cx.background_executor().now();
        let target = self.chosen as f32;
        if slide.read(cx).target() != target {
            let reduce = cx.reduce_motion();
            slide.update(cx, |m, _| m.animate(target, PANEL, now, reduce));
        }
        let motion = *slide.read(cx);
        if motion.is_running(now) {
            window.request_animation_frame();
        }
        let at = motion.value(now);
        let on_choose: Option<Arc<OnChoose>> = self.on_choose.map(Arc::new);
        let chosen = self.chosen;
        let disabled = self.disabled;
        let name = |key: &SharedString| match &self.id {
            ElementId::Name(name) => ElementId::Name(format!("{name}-{key}").into()),
            other => ElementId::NamedChild(Arc::new(other.clone()), key.clone()),
        };
        let segments = self
            .options
            .iter()
            .enumerate()
            .map(|(index, (key, words))| {
                // How near the highlight is to this segment: 1 on it, 0 a segment away or more.
                let near = (1.0 - (at - index as f32).abs()).clamp(0.0, 1.0);
                let on_choose = on_choose.clone();
                let mut segment = div()
                    .id(name(key))
                    .aria_label(words.clone())
                    .aria_selected(index == chosen)
                    .relative()
                    .flex_none()
                    .w(px(WIDTH))
                    .h(px(HEIGHT - 2. * INSET))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .text_size(px(12.5))
                    .child(words.clone());
                if !disabled && index != chosen {
                    segment = segment.cursor_pointer();
                    if let Some(on_choose) = on_choose {
                        segment = segment.on_click(move |_: &ClickEvent, window, cx| {
                            on_choose(index, window, cx)
                        });
                    }
                }
                hoverable(segment, move |d, hover| {
                    d.text_color(mix(mix(p.faint, p.mute, hover), p.text, near))
                })
            });
        // Border-box: the 1 px border takes a pixel on each side.
        let width = WIDTH * self.options.len() as f32 + 2. * INSET + 2.;
        let mut control = div()
            .id(self.id.clone())
            .aria_label(self.label.clone())
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .px(px(INSET))
            .w(px(width))
            .h(px(HEIGHT))
            .rounded(px(7.))
            .bg(p.field)
            .border_1()
            .border_color(p.line)
            .child(
                div()
                    .absolute()
                    .top(px(1.))
                    .left(px(INSET + WIDTH * at))
                    .w(px(WIDTH))
                    .h(px(HEIGHT - 2. * INSET))
                    .rounded(px(5.))
                    .bg(p.card2),
            )
            .children(segments);
        if disabled {
            control = control.opacity(0.35);
        }
        control.test_support()
    }
}
