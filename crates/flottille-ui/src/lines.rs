//! The small parts the cards of a page are made of: a [`Chip`] under a title, a card's
//! [`CardHead`], a [`LabelledLine`] (an icon, a label, a value), a block's [`caption`], a
//! [`FlatRow`] (a session in its place), a [`StepRow`] of a setup and a card's [`FooterAction`].
//!
//! Each part that is clicked fades its hover; none draws a separator: the [`Card`](crate::Card)
//! draws them between its blocks.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    TestSupportExt, Window, div, px,
};

use crate::color::mix;
use crate::icons::{Icon, icon};
use crate::interact::{hoverable, tint};
use crate::menu::Menu;
use crate::motion::PANEL;
use crate::theme::palette;
use crate::typography::Type;

type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A read-only fact under a page's title that leads somewhere: `atlas · main`, which opens the
/// repository's row in the settings. 24 px high, mono.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    glyph: Icon,
    text: SharedString,
    faint: Option<SharedString>,
    on_click: Option<OnClick>,
}

impl Chip {
    pub fn new(id: impl Into<ElementId>, glyph: Icon, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            glyph,
            text: text.into(),
            faint: None,
            on_click: None,
        }
    }

    /// A second part in the faint ink: `· main`.
    pub fn faint(mut self, text: impl Into<SharedString>) -> Self {
        self.faint = Some(text.into());
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let label = match &self.faint {
            Some(faint) => format!("{} {faint}", self.text),
            None => self.text.to_string(),
        };
        let mut chip = div()
            .id(self.id)
            .aria_label(SharedString::from(label))
            .flex_none()
            .h(px(24.))
            .px(px(8.))
            .rounded(px(6.))
            .border_1()
            .flex()
            .items_center()
            .gap(px(6.))
            .mono(11.5)
            .child(icon(self.glyph).size(px(12.)).text_color(p.faint))
            .child(self.text)
            .when_some(self.faint, |d, faint| {
                d.child(div().text_color(p.faint).child(faint))
            });
        if let Some(on_click) = self.on_click {
            chip = chip
                .cursor_pointer()
                .on_click(move |event, window, cx| on_click(event, window, cx));
        }
        hoverable(chip, move |d, t| {
            d.bg(p.hover.opacity(t))
                .text_color(mix(p.mute, p.text, t))
                .border_color(mix(p.line, p.line2, t))
        })
    }
}

/// A card's head: its icon and name in bold, a mono word beside it ("from main"), and on the
/// right what it holds and its "⋯". 46 px high.
#[derive(IntoElement)]
pub struct CardHead {
    id: ElementId,
    glyph: Icon,
    title: SharedString,
    meta: Option<SharedString>,
    trailing: Vec<AnyElement>,
}

impl CardHead {
    /// The head `id`, its word beside the title `<id>-meta`.
    pub fn new(id: impl Into<ElementId>, glyph: Icon, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            glyph,
            title: title.into(),
            meta: None,
            trailing: Vec::new(),
        }
    }

    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// On the right, in order: the lines changed, the "⋯".
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }
}

impl RenderOnce for CardHead {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let meta = self.meta.map(|meta| {
            div()
                .id(ElementId::Name(format!("{}-meta", self.id).into()))
                .aria_label(meta.clone())
                .flex_none()
                .mono(11.5)
                .text_color(p.faint)
                .whitespace_nowrap()
                .child(meta)
                .test_support()
        });
        div()
            .id(self.id)
            .aria_label(self.title.clone())
            .h(px(46.))
            .flex_none()
            .pl(px(14.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .ui_text()
            .text_color(p.text)
            .child(icon(self.glyph).size(px(14.)).text_color(p.faint))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title),
            )
            .children(meta)
            .child(div().flex_1())
            .children(self.trailing)
            .test_support()
    }
}

/// One fact of a card on its own line: an icon and a label in a 112 px column, then the value,
/// mono. A line is drawn only when it has something to say.
#[derive(IntoElement)]
pub struct LabelledLine {
    id: ElementId,
    glyph: Icon,
    label: SharedString,
    value: Vec<AnyElement>,
    said: SharedString,
    on_click: Option<OnClick>,
}

impl LabelledLine {
    /// `said` is what the value says in words, for its label: `atlas 1 ahead · 4 uncommitted`.
    pub fn new(
        id: impl Into<ElementId>,
        glyph: Icon,
        label: impl Into<SharedString>,
        said: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            glyph,
            label: label.into(),
            value: Vec::new(),
            said: said.into(),
            on_click: None,
        }
    }

    /// Opens what the value names (a pull request, a ticket).
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl ParentElement for LabelledLine {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.value.extend(elements);
    }
}

impl RenderOnce for LabelledLine {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let clickable = self.on_click.is_some();
        let mut line = div()
            .id(self.id)
            .aria_label(SharedString::from(format!("{} {}", self.label, self.said)))
            .min_h(px(26.))
            .flex()
            .items_center()
            .gap(px(8.))
            .ui_text()
            .text_size(px(12.5))
            .child(
                div()
                    .w(px(112.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(p.faint)
                    .child(icon(self.glyph).size(px(13.)).text_color(p.faint))
                    .child(self.label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .mono(12.)
                    .children(self.value),
            );
        if let Some(on_click) = self.on_click {
            line = line
                .cursor_pointer()
                .on_click(move |event, window, cx| on_click(event, window, cx));
        }
        hoverable(line, move |d, t| {
            let t = if clickable { t } else { 0.0 };
            d.text_color(mix(p.mute, p.text, t))
        })
    }
}

/// The frame of a card's labelled lines: 8 px above and below, 14 px on the sides.
/// The gap between the lines of a [`lines_block`]: a line that folds away takes it with it.
pub const LINE_GAP: Pixels = px(2.);

pub fn lines_block(lines: impl IntoIterator<Item = AnyElement>) -> impl IntoElement {
    div()
        .py(px(8.))
        .px(px(14.))
        .flex()
        .flex_col()
        .gap(LINE_GAP)
        .children(lines)
}

/// The caption above a block of a card: `SESSIONS 1`, `SETUP STEP 2 OF 3`. Its label is the
/// caption in words (`Setup · step 2 of 3`).
pub fn caption(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    count: impl Into<SharedString>,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    let (text, count) = (text.into(), count.into());
    div()
        .id(id.into())
        .aria_label(SharedString::from(format!("{text} · {count}")))
        .pt(px(10.))
        .px(px(14.))
        .pb(px(2.))
        .flex()
        .gap(px(6.))
        .mono(10.5)
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.faint)
        .child(text.to_uppercase())
        .child(
            div()
                .text_color(p.faint.opacity(0.75))
                .child(count.to_uppercase()),
        )
        .test_support()
}

/// A row flush with its card: a state mark, a title and a mono word on the right. 40 px high.
#[derive(IntoElement)]
pub struct FlatRow {
    id: ElementId,
    leading: Option<AnyElement>,
    title: SharedString,
    meta: Option<AnyElement>,
    more: Option<Entity<Menu>>,
    on_click: Option<OnClick>,
}

impl FlatRow {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            leading: None,
            title: title.into(),
            meta: None,
            more: None,
            on_click: None,
        }
    }

    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    /// Mono, faint, on the right: a time, the agents.
    pub fn meta(mut self, element: impl IntoElement) -> Self {
        self.meta = Some(element.into_any_element());
        self
    }

    /// Its "⋯" menu, shown while the pointer is on the row or the menu is open: it fades in and
    /// out with the row's hover, in `<id>-actions`. Its clicks are its own, not the row's.
    pub fn more(mut self, menu: Entity<Menu>) -> Self {
        self.more = Some(menu);
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for FlatRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let id = self.id.clone();
        let mut row = div()
            .id(self.id)
            .aria_label(self.title.clone())
            .h(px(40.))
            .flex_none()
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(10.))
            .ui_text()
            .text_color(p.text)
            .child(
                div()
                    .w(px(16.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .children(self.leading),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.title),
            )
            .when_some(self.meta, |d, meta| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .mono(11.)
                        .text_color(p.faint)
                        .child(meta),
                )
            });
        if let Some(on_click) = self.on_click {
            row = row
                .cursor_pointer()
                .on_click(move |event, window, cx| on_click(event, window, cx));
        }
        let menu_open = self
            .more
            .as_ref()
            .is_some_and(|menu| menu.read(cx).is_open());
        let actions = self.more.map(|menu| {
            div()
                .id(ElementId::Name(format!("{id}-actions").into()))
                .flex_none()
                // The menu's clicks are its own, not the row's (which opens the session).
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(menu)
                .test_support()
        });
        hoverable(row, move |d, t| {
            let shown = if menu_open { 1.0 } else { t };
            d.bg(p.hover.opacity(t))
                .children(actions.map(|actions| actions.opacity(shown)))
        })
    }
}

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
                        d.child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .mono(12.)
                                .child(object),
                        )
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

/// The last row of a card: "+ New session in main", faint until hovered. 40 px high.
#[derive(IntoElement)]
pub struct FooterAction {
    id: ElementId,
    label: SharedString,
    enabled: bool,
    on_click: Option<OnClick>,
}

impl FooterAction {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled: true,
            on_click: None,
        }
    }

    /// A place still being prepared takes no session yet: the row stays, dimmed.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for FooterAction {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let enabled = self.enabled;
        let mut row = div()
            .id(self.id)
            .aria_label(self.label.clone())
            .h(px(40.))
            .flex_none()
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(6.))
            .ui_text()
            .text_size(px(13.))
            .when(!enabled, |d| d.opacity(0.5))
            .child(icon(Icon::Plus).size(px(12.)).text_color(p.faint))
            .child(self.label);
        if let Some(on_click) = self.on_click.filter(|_| enabled) {
            row = row
                .cursor_pointer()
                .on_click(move |event, window, cx| on_click(event, window, cx));
        }
        hoverable(row, move |d, t| {
            let t = if enabled { t } else { 0.0 };
            d.bg(p.hover.opacity(t)).text_color(mix(p.faint, p.text, t))
        })
    }
}
