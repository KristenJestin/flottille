//! The rows of a setup's steps and the text of a message with its commands, which draw what an
//! agent does with the parts of `flottille-ui`.

use std::ops::Range;
use std::rc::Rc;

use flottille_ui::motion::PANEL;
use flottille_ui::{Type, hoverable, palette, tint, tipped_when_cut};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, HighlightStyle, InteractiveElement,
    IntoElement, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    StyledText, TestSupportExt, Window, div, px,
};

type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// How a step of a setup reads: the one running in the text ink, the ones done muted, the ones
/// to do faint, a failed one in the failure colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepTone {
    Done,
    Now,
    Todo,
    Failed,
}

/// One step of a setup: its mark, what it does (`Run` and `pnpm install`, mono) and its time.
/// 30 px high at least. A step that can be opened fades its hover.
#[derive(IntoElement)]
pub struct StepRow {
    id: ElementId,
    mark: AnyElement,
    verb: SharedString,
    object: Option<SharedString>,
    time: Option<SharedString>,
    time_id: ElementId,
    tone: StepTone,
    on_click: Option<OnClick>,
}

impl StepRow {
    /// The row `id`, its time `time_id`.
    pub fn new(
        id: impl Into<ElementId>,
        mark: impl IntoElement,
        verb: impl Into<SharedString>,
        tone: StepTone,
        time_id: impl Into<ElementId>,
    ) -> Self {
        Self {
            id: id.into(),
            mark: mark.into_any_element(),
            verb: verb.into(),
            object: None,
            time: None,
            time_id: time_id.into(),
            tone,
            on_click: None,
        }
    }

    pub fn object(mut self, object: impl Into<SharedString>) -> Self {
        self.object = Some(object.into());
        self
    }

    pub fn time(mut self, time: impl Into<SharedString>) -> Self {
        self.time = Some(time.into());
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for StepRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let ink = match self.tone {
            StepTone::Done => p.mute,
            StepTone::Now => p.text,
            StepTone::Todo => p.faint,
            StepTone::Failed => p.fail,
        };
        // Its ink moves when the step's state changes: to do, running, done, failed.
        let ink = tint(self.id.clone(), ink, PANEL, window, cx);
        let said = match &self.object {
            Some(object) => format!("{} {object}", self.verb),
            None => self.verb.to_string(),
        };
        let time = self.time.map(|time| {
            div()
                .id(self.time_id)
                .aria_label(time.clone())
                .flex_none()
                .mono(11.)
                .text_color(p.faint)
                .child(time)
                .test_support()
        });
        let clickable = self.on_click.is_some();
        let id = self.id.clone();
        let mut row = div()
            .id(self.id)
            .aria_label(SharedString::from(said))
            .min_h(px(30.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(10.))
            .ui_text()
            .text_size(px(12.5))
            .text_color(ink)
            .child(
                div()
                    .size(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.mark),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(5.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.verb)
                    .when_some(self.object, |d, object| {
                        d.child(tipped_when_cut(
                            div()
                                .id(ElementId::Name(format!("{id}-object").into()))
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .mono(12.)
                                .child(object.clone()),
                            object,
                            window,
                            cx,
                        ))
                    }),
            )
            .children(time);
        if let Some(on_click) = self.on_click {
            row = row
                .cursor_pointer()
                .on_click(move |event, window, cx| on_click(event, window, cx));
        }
        hoverable(row, move |d, t| {
            let t = if clickable { t } else { 0.0 };
            d.bg(p.hover.opacity(t))
        })
    }
}

/// A message as it was typed, each command it used (`commands`, byte ranges of `/name`) drawn as
/// an accent chip in its line: the accent's ink, medium, on the accent's faint fill. A chip is a
/// run of the text, so that the line wraps as the words do.
pub fn commanded_text(text: &str, commands: &[Range<usize>], cx: &App) -> StyledText {
    let p = palette(cx);
    let chip = HighlightStyle {
        color: Some(p.acc),
        background_color: Some(p.acc.opacity(0.14)),
        font_weight: Some(FontWeight::MEDIUM),
        ..Default::default()
    };
    StyledText::new(text.to_owned())
        .with_highlights(commands.iter().map(|range| (range.clone(), chip)))
}
