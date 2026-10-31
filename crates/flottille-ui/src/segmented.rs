//! `Segmented`: two or three short words that switch a mode or a view at once (the composer's
//! Ask | Auto), one of them chosen. Sized to its words, never stretched, never in a form (a scan
//! of the window's dialogs refuses it there: a form's choice of a kind is [`RadioChoices`] or a
//! [`Select`]). The chosen one is a filled highlight that slides to the one clicked over
//! [`PANEL`], the inks fade along it, and each segment fades its hover. A choice that must never be
//! taken by mistake (Full) is an alarm: the highlight fades to the alarm colour as it slides onto
//! it.
//!
//! [`RadioChoices`]: crate::RadioChoices
//! [`Select`]: crate::Select

use std::sync::Arc;

use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt, TextRun, Window, div, font,
    px,
};

use crate::color::mix;
use crate::interact::hoverable;
use crate::motion::{Motion, PANEL};
use crate::theme::{Palette, palette};

type OnChoose = Box<dyn Fn(usize, &mut Window, &mut App) + 'static>;

/// The most choices it takes: more is a select's.
pub const MAX_CHOICES: usize = 3;
const HEIGHT: f32 = 26.;
const INSET: f32 = 2.;
/// The space on each side of a segment's words.
const PAD: f32 = 9.;
const TEXT: f32 = 12.;

#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    label: SharedString,
    options: Vec<(SharedString, SharedString)>,
    chosen: usize,
    disabled: bool,
    alarm: Option<usize>,
    on_choose: Option<OnChoose>,
}

/// The highlight's fill and ink with the slide `at` (the index it is on, fractional while it
/// moves): the accent's, fading to the alarm colour's as it nears the `alarm` choice.
pub fn highlight(p: &Palette, alarm: Option<usize>, at: f32) -> (gpui_kit::Hsla, gpui_kit::Hsla) {
    let on_alarm = alarm.map_or(0.0, |alarm| {
        (1.0 - (at - alarm as f32).abs()).clamp(0.0, 1.0)
    });
    (
        mix(p.acc, p.fail, on_alarm),
        mix(p.on_acc, p.on_fail, on_alarm),
    )
}

impl Segmented {
    /// `label` says what it chooses (its accessible name). Each option is `(key, words)`: its
    /// segment's id is `{id}-{key}`. More than [`MAX_CHOICES`] options is a defect: it panics.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        options: impl IntoIterator<Item = (&'static str, &'static str)>,
        chosen: usize,
    ) -> Self {
        let options: Vec<(SharedString, SharedString)> = options
            .into_iter()
            .map(|(key, words)| (key.into(), words.into()))
            .collect();
        assert!(
            options.len() <= MAX_CHOICES,
            "a segmented control takes at most three choices; more is a Select"
        );
        Self {
            id: id.into(),
            label: label.into(),
            options,
            chosen,
            disabled: false,
            alarm: None,
            on_choose: None,
        }
    }

    /// The choice at `index` is drawn in the alarm colour when it is the chosen one.
    pub fn alarm(mut self, index: usize) -> Self {
        self.alarm = Some(index);
        self
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
        // Each segment is as wide as its words: the highlight slides from one place and width to
        // the other's.
        let face = font(crate::typography::UI);
        let widths: Vec<f32> = self
            .options
            .iter()
            .map(|(_, words)| {
                let run = TextRun {
                    len: words.len(),
                    font: face.clone(),
                    color: p.text,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window
                    .text_system()
                    .shape_line(words.clone(), px(TEXT), &[run], None);
                (f32::from(line.width) + 2. * PAD).ceil()
            })
            .collect();
        let lefts: Vec<f32> = widths
            .iter()
            .scan(INSET, |left, width| {
                let at = *left;
                *left += width;
                Some(at)
            })
            .collect();
        let (low, high) = (at.floor() as usize, at.ceil() as usize);
        let (low, high) = (
            low.min(widths.len().saturating_sub(1)),
            high.min(widths.len().saturating_sub(1)),
        );
        let part = at - at.floor();
        let lerp = |a: f32, b: f32| a + (b - a) * part;
        let highlight_left = lerp(lefts[low], lefts[high]);
        let highlight_width = lerp(widths[low], widths[high]);
        let (fill, ink) = highlight(&p, self.alarm, at);
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
                    .w(px(widths[index]))
                    .h(px(HEIGHT - 2. * INSET - 2.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .whitespace_nowrap()
                    .rounded(px(6.))
                    .text_size(px(TEXT))
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
                    d.text_color(mix(mix(p.mute, p.text, hover), ink, near))
                })
            });
        // Border-box: the 1 px border takes a pixel on each side.
        let width = widths.iter().sum::<f32>() + 2. * INSET + 2.;
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
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.field_line)
            .child(
                div()
                    .absolute()
                    .top(px(INSET))
                    .left(px(highlight_left))
                    .w(px(highlight_width))
                    .h(px(HEIGHT - 2. * INSET - 2.))
                    .rounded(px(6.))
                    .bg(fill),
            )
            .children(segments);
        if disabled {
            control = control.opacity(0.35);
        }
        control.test_support()
    }
}
