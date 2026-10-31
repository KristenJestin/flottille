//! `Tabs`: the row of names that switches a page between its sections, one of them chosen and
//! shown alone. Unlike [`Segmented`] (two or three short words that switch a mode), it takes as
//! many tabs as a page has sections, each with a count when it lists something. The chosen one is
//! marked by a bar under it that slides to the one clicked over [`PANEL`], widening or narrowing
//! to its words, while the inks fade along it; each tab fades its hover.
//!
//! [`Segmented`]: crate::Segmented

use std::sync::Arc;

use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt, TextRun, Window, div, font,
    px,
};

use crate::color::mix;
use crate::interact::hoverable;
use crate::motion::{Motion, PANEL};
use crate::theme::palette;
use crate::typography::{MONO, Type, UI};

type OnChoose = Box<dyn Fn(usize, &mut Window, &mut App) + 'static>;

const HEIGHT: f32 = 34.;
/// The space on each side of a tab's words.
const PAD: f32 = 10.;
const TEXT: f32 = 12.5;
const COUNT: f32 = 11.;
/// Between a tab's name and its count.
const GAP: f32 = 6.;
const BAR: f32 = 2.;

struct Tab {
    key: SharedString,
    label: SharedString,
    count: Option<SharedString>,
}

#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    label: SharedString,
    tabs: Vec<Tab>,
    chosen: usize,
    on_choose: Option<OnChoose>,
}

impl Tabs {
    /// `label` says what the tabs switch (their accessible name); each tab is `<id>-<key>`, its
    /// count `<id>-<key>-count`, the marker `<id>-marker`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, chosen: usize) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            tabs: Vec::new(),
            chosen,
            on_choose: None,
        }
    }

    pub fn tab(mut self, key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        self.tabs.push(Tab {
            key: key.into(),
            label: label.into(),
            count: None,
        });
        self
    }

    /// A tab whose section lists something: its count follows its name.
    pub fn counted(
        mut self,
        key: impl Into<SharedString>,
        label: impl Into<SharedString>,
        count: impl Into<SharedString>,
    ) -> Self {
        self.tabs.push(Tab {
            key: key.into(),
            label: label.into(),
            count: Some(count.into()),
        });
        self
    }

    /// Called with the index of the tab clicked, when it is not the chosen one.
    pub fn on_choose(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_choose = Some(Box::new(f));
        self
    }
}

/// The width of `words` set in `face` at `size`.
fn width_of(words: &SharedString, face: &'static str, size: f32, window: &Window) -> f32 {
    let run = TextRun {
        len: words.len(),
        font: font(face),
        color: gpui_kit::Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(words.clone(), px(size), &[run], None)
            .width,
    )
}

impl RenderOnce for Tabs {
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
        // Each tab is as wide as its words and its count: the marker slides from one place and
        // width to the other's.
        let widths: Vec<f32> = self
            .tabs
            .iter()
            .map(|tab| {
                let count = tab
                    .count
                    .as_ref()
                    .map_or(0., |count| GAP + width_of(count, MONO, COUNT, window));
                (width_of(&tab.label, UI, TEXT, window) + count + 2. * PAD).ceil()
            })
            .collect();
        let lefts: Vec<f32> = widths
            .iter()
            .scan(0., |left, width| {
                let at = *left;
                *left += width;
                Some(at)
            })
            .collect();
        let last = widths.len().saturating_sub(1);
        let (low, high) = (
            (at.floor() as usize).min(last),
            (at.ceil() as usize).min(last),
        );
        let part = at - at.floor();
        let lerp = |a: f32, b: f32| a + (b - a) * part;
        let on_choose: Option<Arc<OnChoose>> = self.on_choose.map(Arc::new);
        let chosen = self.chosen;
        let name = |key: &SharedString, tail: Option<&'static str>| {
            let base = format!("{}-{key}", id_text(&self.id));
            match tail {
                Some(tail) => ElementId::Name(format!("{base}-{tail}").into()),
                None => ElementId::Name(base.into()),
            }
        };
        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            // How near the marker is to this tab: 1 on it, 0 a tab away or more.
            let near = (1.0 - (at - index as f32).abs()).clamp(0.0, 1.0);
            let on_choose = on_choose.clone();
            let mut element = div()
                .id(name(&tab.key, None))
                .aria_label(tab.label.clone())
                .aria_selected(index == chosen)
                .flex_none()
                .w(px(widths[index]))
                .h(px(HEIGHT - BAR))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(GAP))
                .whitespace_nowrap()
                .rounded_t(px(6.))
                .font_family(UI)
                .text_size(px(TEXT))
                .child(tab.label.clone())
                .children(tab.count.clone().map(|count| {
                    div()
                        .id(name(&tab.key, Some("count")))
                        .aria_label(count.clone())
                        .mono(COUNT)
                        .child(count)
                        .test_support()
                }));
            if index != chosen {
                element = element.cursor_pointer();
                if let Some(on_choose) = on_choose {
                    element = element
                        .on_click(move |_: &ClickEvent, window, cx| on_choose(index, window, cx));
                }
            }
            hoverable(element, move |d, hover| {
                d.bg(p.hover.opacity(hover * (1. - near))).text_color(mix(
                    mix(p.faint, p.text, hover),
                    p.text,
                    near,
                ))
            })
        });
        // The rule under the tabs is the wrapper's border; the tabs scroll sideways when the
        // window is too narrow for all of them, the marker with them.
        div().w_full().border_b_1().border_color(p.line).child(
            div()
                .id(self.id.clone())
                .aria_label(self.label.clone())
                .relative()
                .w_full()
                .h(px(HEIGHT))
                .flex()
                .items_start()
                .overflow_x_scroll()
                .children(tabs)
                .child(
                    div()
                        .id(ElementId::Name(
                            format!("{}-marker", id_text(&self.id)).into(),
                        ))
                        .absolute()
                        .bottom_0()
                        .left(px(lerp(lefts[low], lefts[high])))
                        .w(px(lerp(widths[low], widths[high])))
                        .h(px(BAR))
                        .rounded_t(px(2.))
                        .bg(p.acc)
                        .test_support(),
                )
                .test_support(),
        )
    }
}

/// The text of a named id, for the ids of the parts.
fn id_text(id: &ElementId) -> String {
    match id {
        ElementId::Name(name) => name.to_string(),
        other => format!("{other:?}"),
    }
}
