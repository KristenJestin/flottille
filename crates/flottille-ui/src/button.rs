//! Buttons, the badge an icon button can carry, and the tooltip.
//!
//! Four kinds of text button and one size: every text button is 30 px high, 13 px medium, with
//! 12 px on each side; a kind changes the fill, the border and the ink, never the size. An icon
//! button is a 28 px square with a tooltip. There is no way to ask for another size.

use std::sync::Arc;

use gpui_kit::{
    Anchor, AnyView, App, AppContext as _, ClickEvent, Context, Div, ElementId, Entity, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, RenderOnce, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Task, TestSupportExt, Window, anchored, deferred,
    div, point, px,
};

use crate::color::mix;
use crate::icons::{Icon, icon};
use crate::interact::{choose, fade, feel, wire_with};
use crate::layer;
use crate::motion::{HOVER, Motion, TOOLTIP};
use crate::space::SM;
use crate::theme::{Palette, palette, radius};
use crate::typography::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// Filled with the accent: the one main action of a view or a dialog (Save, Create, Send,
    /// Allow).
    Primary,
    /// Transparent with a stronger border: a page's or a section's actions.
    Secondary,
    /// No border, a hover background: Cancel, Deny, lesser actions.
    Quiet,
    /// Filled with the failure colour: only the action of a confirmation of something
    /// destructive ([`Confirm`](crate::Confirm)).
    Danger,
    /// A 28 px square holding one icon.
    Icon,
    /// A 30 px round primary holding one icon: the composer's send.
    Round,
}

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    kind: ButtonKind,
    label: Option<SharedString>,
    icon: Option<Icon>,
    tooltip: Option<SharedString>,
    tooltip_hidden: bool,
    badge: Option<Badge>,
    full_width: bool,
    disabled: bool,
    chosen: Option<bool>,
    on_click: Option<OnClick>,
}

impl Button {
    fn new(id: impl Into<ElementId>, kind: ButtonKind) -> Self {
        Self {
            id: id.into(),
            kind,
            label: None,
            icon: None,
            tooltip: None,
            tooltip_hidden: false,
            badge: None,
            full_width: false,
            disabled: false,
            chosen: None,
            on_click: None,
        }
    }

    pub fn primary(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self::new(id, ButtonKind::Primary).label(label)
    }

    pub fn secondary(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self::new(id, ButtonKind::Secondary).label(label)
    }

    pub fn quiet(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self::new(id, ButtonKind::Quiet).label(label)
    }

    /// The action of a confirmation of something destructive, and nothing else: a page or a row
    /// offers it as a "Remove…" item of its "⋯" menu, which opens the confirmation.
    pub fn danger(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self::new(id, ButtonKind::Danger).label(label)
    }

    /// An icon button; `tooltip` says what it does (it is also its accessible name).
    pub fn icon(id: impl Into<ElementId>, glyph: Icon, tooltip: impl Into<SharedString>) -> Self {
        let mut button = Self::new(id, ButtonKind::Icon);
        button.icon = Some(glyph);
        button.tooltip = Some(tooltip.into());
        button
    }

    /// A round primary holding one icon; `tooltip` says what it does.
    pub fn round(id: impl Into<ElementId>, glyph: Icon, tooltip: impl Into<SharedString>) -> Self {
        let mut button = Self::new(id, ButtonKind::Round);
        button.icon = Some(glyph);
        button.tooltip = Some(tooltip.into());
        button
    }

    /// A quiet button that is one of several choices: it fills with the accent when it is the
    /// chosen one, and the fill moves over [`PANEL`](crate::motion::PANEL) when the choice does.
    pub fn choice(id: impl Into<ElementId>, label: impl Into<SharedString>, chosen: bool) -> Self {
        let mut button = Self::new(id, ButtonKind::Quiet).label(label);
        button.chosen = Some(chosen);
        button
    }

    /// Faded, and a click does nothing.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// An icon before the label.
    pub fn leading(mut self, glyph: Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Keeps the tooltip from showing (it stays the button's accessible name): a button whose
    /// menu is open would have it drawn over the menu.
    pub fn tooltip_hidden(mut self, hidden: bool) -> Self {
        self.tooltip_hidden = hidden;
        self
    }

    pub fn badge(mut self, badge: Badge) -> Self {
        self.badge = Some(badge);
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let (state, now) = feel(&self.id, window, cx);
        let height = match self.kind {
            ButtonKind::Icon => 28.,
            _ => 30.,
        };
        let (fill, ink, hover_ink) = match self.kind {
            ButtonKind::Primary | ButtonKind::Round => (Some(p.acc), p.on_acc, p.on_acc),
            ButtonKind::Danger => (Some(p.fail), p.on_fail, p.on_fail),
            ButtonKind::Secondary => (None, p.text, p.text),
            ButtonKind::Quiet | ButtonKind::Icon => (None, p.mute, p.text),
        };
        let border =
            (self.kind == ButtonKind::Secondary).then(|| mix(p.line2, p.focus_line, now.hover));
        let hover_bg = match self.kind {
            ButtonKind::Primary | ButtonKind::Round => mix(p.acc, p.on_acc, 0.08),
            ButtonKind::Danger => mix(p.fail, p.text, 0.15),
            _ => p.hover,
        };
        let mut background = match fill {
            Some(fill) => mix(fill, hover_bg, now.hover),
            None => hover_bg.opacity(now.hover),
        };
        let mut ink_now = mix(ink, hover_ink, now.hover);
        if let Some(chosen) = self.chosen {
            let t = choose(&state, chosen, window, cx);
            background = mix(background, p.acc, t);
            ink_now = mix(ink_now, p.on_acc, t);
        }
        // GPUI cannot scale an element: pressing shrinks the background layer instead.
        let inset = px(height * (1.0 - now.scale) / 2.0);
        let label = self.label.clone();
        let tooltip = self.tooltip.clone();
        let accessible = label
            .clone()
            .or_else(|| tooltip.clone())
            .unwrap_or_default();

        let mut button = div()
            .id(self.id.clone())
            .aria_label(accessible)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(px(height))
            .text_color(ink_now)
            .ui_text()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .child(
                div()
                    .absolute()
                    .top(inset)
                    .bottom(inset)
                    .left(inset)
                    .right(inset)
                    .when(self.kind == ButtonKind::Round, |d| d.rounded_full())
                    .when(self.kind != ButtonKind::Round, |d| {
                        d.rounded(radius::BUTTON)
                    })
                    .bg(background)
                    .when_some(border, |d, border| d.border_1().border_color(border)),
            );
        button = match self.kind {
            ButtonKind::Icon => button.w(px(28.)),
            ButtonKind::Round => button.w(px(30.)),
            _ => button.px(px(12.)),
        };
        if self.full_width {
            button = button.w_full();
        }
        if let Some(glyph) = self.icon {
            let size = match self.kind {
                ButtonKind::Icon => 15.,
                ButtonKind::Round => 14.,
                _ => 14.,
            };
            button = button.child(icon(glyph).size(px(size)).text_color(ink_now).relative());
        }
        if let Some(label) = label {
            button = button.child(div().relative().child(label));
        }
        if let Some(badge) = self.badge {
            button = button.child(div().absolute().top(px(1.)).right(px(0.)).child(badge));
        }
        // GPUI places its own tooltip under the pointer, in front of the element it describes:
        // this one is anchored to the element.
        let tip = tip_state(&self.id, window, cx);
        let shown = tip.update(cx, |tip, cx| tip.sample(self.tooltip_hidden, window, cx));
        if let Some(text) = tooltip.filter(|_| shown > 0.0) {
            button = button.child(tooltip_overlay(&self.id, text, height, shown, p));
        }
        let down = tip.clone();
        button = button.on_mouse_down(MouseButton::Left, move |_, _, cx| {
            down.update(cx, |tip, cx| tip.rest(false, cx));
        });
        // Enabled or not is a state: it fades over HOVER instead of switching.
        let enabled = fade(
            ElementId::NamedChild(Arc::new(self.id.clone()), "enabled".into()),
            !self.disabled,
            HOVER,
            window,
            cx,
        );
        if enabled < 1.0 {
            button = button.opacity(0.35 + 0.65 * enabled);
        }
        if !self.disabled
            && let Some(on_click) = self.on_click
        {
            button = button.on_click(on_click);
        }
        let on_hover = tip.clone();
        wire_with(button, state, true, move |hovered, cx| {
            on_hover.update(cx, |tip, cx| tip.rest(hovered, cx));
        })
        .test_support()
    }
}

use gpui_kit::prelude::FluentBuilder as _;

/// A count or a mark on an icon button: the bell's count, the amber "!" on the agents button.
/// Amber, because it only ever counts what waits for the user.
#[derive(IntoElement, Clone)]
pub struct Badge {
    text: SharedString,
}

impl Badge {
    pub fn count(n: usize) -> Self {
        Self {
            text: n.to_string().into(),
        }
    }

    pub fn alert() -> Self {
        Self { text: "!".into() }
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .min_w(px(14.))
            .h(px(14.))
            .px(px(3.))
            .rounded(px(7.))
            .bg(p.wait)
            .text_color(p.on_wait)
            .mono(9.5)
            .line_height(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .flex()
            .justify_center()
            .child(self.text)
    }
}

/// A short label shown on hover: it fades in over [`HOVER`](crate::motion::HOVER).
pub struct Tooltip {
    text: SharedString,
    shown: Motion,
}

impl Tooltip {
    pub fn view(text: SharedString, cx: &mut App) -> AnyView {
        let now = cx.background_executor().now();
        let mut shown = Motion::settled(0.0);
        shown.animate(1.0, HOVER, now, cx.reduce_motion());
        cx.new(|_| Self { text, shown }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let now = cx.background_executor().now();
        if self.shown.is_running(now) {
            window.request_animation_frame();
        }
        bubble(self.text.clone(), p).opacity(self.shown.value(now))
    }
}

fn bubble(text: SharedString, p: Palette) -> gpui_kit::Div {
    div()
        .px(px(8.))
        .py(px(4.))
        .rounded(px(6.))
        .bg(p.card2)
        .border_1()
        .border_color(p.line2)
        .text_color(p.text)
        .ui_text()
        .text_size(px(12.))
        .whitespace_nowrap()
        .child(text)
}

/// Whether the pointer rests on a button, and how far its tooltip is shown (0 to 1).
struct Tip {
    shown: Motion,
    /// The wait before the tooltip comes; dropping it cancels it.
    wait: Option<Task<()>>,
}

fn tip_state(id: &ElementId, window: &mut Window, cx: &mut App) -> Entity<Tip> {
    let key = ElementId::NamedChild(Arc::new(id.clone()), "tooltip".into());
    window.use_keyed_state(key, cx, |_, _: &mut Context<Tip>| Tip {
        shown: Motion::settled(0.0),
        wait: None,
    })
}

impl Tip {
    /// The pointer came or went: it comes after [`TOOLTIP`] of rest, and goes at once.
    fn rest(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.wait = None;
        if hovered {
            self.wait = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(TOOLTIP).await;
                this.update(cx, |tip, cx| tip.fade(1.0, cx)).ok();
            }));
        } else {
            self.fade(0.0, cx);
        }
    }

    fn fade(&mut self, target: f32, cx: &mut Context<Self>) {
        if self.shown.target() != target {
            let now = cx.background_executor().now();
            self.shown.animate(target, HOVER, now, cx.reduce_motion());
            cx.notify();
        }
    }

    /// How far it is shown now. A button that hides its tooltip (its menu is open) loses the
    /// pointer's rest, so the tooltip does not come back when the menu closes.
    fn sample(&mut self, hidden: bool, window: &mut Window, cx: &mut Context<Self>) -> f32 {
        if hidden {
            self.wait = None;
            self.shown = Motion::settled(0.0);
            return 0.0;
        }
        let now = cx.background_executor().now();
        if self.shown.is_running(now) {
            window.request_animation_frame();
        } else {
            self.shown.settle(now);
        }
        self.shown.value(now)
    }
}

/// Gives an element that is no button the tooltip of one: after [`TOOLTIP`] of rest it fades in
/// anchored to it (`<id>-tooltip`), `height` being the element's own. The element needs an id.
pub fn tipped(
    mut element: Stateful<Div>,
    text: impl Into<SharedString>,
    height: f32,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let id = element
        .interactivity()
        .element_id
        .clone()
        .expect("an element with a tooltip has an id");
    let tip = tip_state(&id, window, cx);
    let shown = tip.update(cx, |tip, cx| tip.sample(false, window, cx));
    let mut element = element.relative();
    if shown > 0.0 {
        let p = palette(cx);
        element = element.child(tooltip_overlay(&id, text.into(), height, shown, p));
    }
    let down = tip.clone();
    element
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            down.update(cx, |tip, cx| tip.rest(false, cx));
        })
        .on_hover(move |hovered, _, cx| tip.update(cx, |tip, cx| tip.rest(*hovered, cx)))
        .test_support()
}

/// The tooltip of the button `id`, drawn after everything else: centred below it with a gap, above
/// it when the window has no room below, kept inside the window sideways. It takes no pointer, so
/// the button keeps its hover.
fn tooltip_overlay(
    id: &ElementId,
    text: SharedString,
    height: f32,
    shown: f32,
    p: Palette,
) -> impl IntoElement {
    let name = ElementId::Name(format!("{id}-tooltip").into());
    let tip = anchored()
        .anchor(Anchor::TopCenter)
        .offset(point(px(0.), px(height)))
        .child(
            // The padding is the gap on either side of the button, whichever it opens on.
            div().py(SM).child(
                bubble(text.clone(), p)
                    .id(name)
                    .aria_label(text)
                    .opacity(shown)
                    .test_support(),
            ),
        );
    div()
        .absolute()
        .top_0()
        .left_1_2()
        .child(deferred(tip).priority(layer::TOOLTIP))
}
