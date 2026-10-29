//! Buttons (primary, secondary, icon), the badge an icon button can carry, and the tooltip.

use gpui_kit::{
    AnyView, App, AppContext as _, ClickEvent, Context, ElementId, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, TestSupportExt, Window, div, px,
};

use crate::color::mix;
use crate::icons::{Icon, icon};
use crate::interact::{choose, feel, wire};
use crate::motion::{HOVER, Motion};
use crate::theme::{palette, radius};
use crate::typography::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// The primary: "New session", send, "Allow once".
    Primary,
    /// Transparent with a stronger border.
    Secondary,
    /// No border, a hover background: "Deny", "Allow for this tree".
    Quiet,
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
        let (fill, ink, hover_ink, border, height) = match self.kind {
            ButtonKind::Primary | ButtonKind::Round => (Some(p.acc), p.on_acc, p.on_acc, None, 30.),
            ButtonKind::Secondary => (None, p.mute, p.text, Some(p.line2), 28.),
            ButtonKind::Quiet => (None, p.mute, p.text, None, 30.),
            ButtonKind::Icon => (None, p.mute, p.text, None, 28.),
        };
        let hover_bg = match self.kind {
            ButtonKind::Primary | ButtonKind::Round => mix(p.acc, p.on_acc, 0.08),
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
            .text_size(px(if self.kind == ButtonKind::Secondary {
                12.5
            } else {
                13.
            }))
            .font_weight(if self.kind == ButtonKind::Secondary {
                FontWeight::NORMAL
            } else {
                FontWeight::MEDIUM
            })
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
            _ => button.px(px(if self.kind == ButtonKind::Secondary {
                10.
            } else {
                12.
            })),
        };
        if self.full_width {
            button = button.w_full();
        }
        if let Some(glyph) = self.icon {
            let size = match self.kind {
                ButtonKind::Icon => 15.,
                ButtonKind::Round => 14.,
                _ => 13.,
            };
            button = button.child(icon(glyph).size(px(size)).text_color(ink_now).relative());
        }
        if let Some(label) = label {
            button = button.child(div().relative().child(label));
        }
        if let Some(badge) = self.badge {
            button = button.child(div().absolute().top(px(2.)).right(px(1.)).child(badge));
        }
        if let Some(text) = tooltip.filter(|_| !self.tooltip_hidden) {
            button = button.tooltip(move |_, cx| Tooltip::view(text.clone(), cx));
        }
        if self.disabled {
            button = button.opacity(0.35);
        } else if let Some(on_click) = self.on_click {
            button = button.on_click(on_click);
        }
        wire(button, state, true).test_support()
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
        div()
            .opacity(self.shown.value(now))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(p.card2)
            .border_1()
            .border_color(p.line2)
            .text_color(p.text)
            .ui_text()
            .text_size(px(12.))
            .child(self.text.clone())
    }
}
