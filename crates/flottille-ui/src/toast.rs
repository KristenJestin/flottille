//! Toasts: the result of an action the user took (saved, added, removed, started, failed), at the
//! bottom right of the window. Each one is 360 px wide: an icon, one line and an optional second.
//! It rises and fades in, and folds away with its gap. A success leaves after four seconds, held
//! while the pointer is on it; a failure stays until it is dismissed. Several stack, the newest on
//! top.
//!
//! The shell owns the [`ToastStack`] and draws it last; any view says a result with [`toast`].
//! A result is never a line appended under a form (a scan of the window's sources refuses it); a
//! form's own validation stays under its field ([`Field`](crate::Field)).

use std::time::Duration;

use gpui_kit::{
    App, BoxShadow, Context, ElementId, FontWeight, Global, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Task, TestSupportExt,
    WeakEntity, Window, anchored, deferred, div, hsla, point, px,
};

use crate::color::mix;
use crate::icons::{Icon, icon};
use crate::interact::hoverable;
use crate::listing::{Item, Listing};
use crate::theme::palette;
use crate::typography::Type;

/// How long a success stays.
pub const STAYS: Duration = Duration::from_secs(4);
/// How long a success stays once the pointer has left it.
pub const STAYS_AFTER_HOVER: Duration = Duration::from_secs(2);

const WIDTH: f32 = 360.;
const EDGE: f32 = 20.;
const GAP: f32 = 8.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Failure,
}

/// One result: its kind, its line and an optional second line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    kind: ToastKind,
    title: SharedString,
    detail: Option<SharedString>,
}

impl Toast {
    /// Something done: "Task test saved".
    pub fn success(title: impl Into<SharedString>) -> Self {
        Self {
            kind: ToastKind::Success,
            title: title.into(),
            detail: None,
        }
    }

    /// Something that could not be done: "The workspace was not removed".
    pub fn failure(title: impl Into<SharedString>) -> Self {
        Self {
            kind: ToastKind::Failure,
            ..Self::success(title)
        }
    }

    /// The second line: what follows from it, or why it failed.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The stack the shell owns, reached by [`toast`].
struct Stack(WeakEntity<ToastStack>);

impl Global for Stack {}

/// Says `toast` in the window's stack. Without a stack (a view tested alone) it is dropped.
pub fn toast(toast: Toast, cx: &mut App) {
    let Some(stack) = cx.try_global::<Stack>().map(|s| s.0.clone()) else {
        return;
    };
    stack.update(cx, |stack, cx| stack.push(toast, cx)).ok();
}

struct Live {
    number: usize,
    toast: Toast,
    hovered: bool,
    leaving: Option<Task<()>>,
}

/// The toasts of a window, at its bottom right, above everything else.
pub struct ToastStack {
    live: Vec<Live>,
    shown: Listing<(usize, Toast)>,
    next: usize,
}

impl ToastStack {
    /// The window's stack: [`toast`] says results in it from now on. Its toasts are
    /// `toast-<n>`, numbered from 1, each with its button `toast-<n>-dismiss`.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        cx.set_global(Stack(this));
        Self {
            live: Vec::new(),
            shown: Listing::default(),
            next: 0,
        }
    }

    pub fn push(&mut self, toast: Toast, cx: &mut Context<Self>) {
        self.next += 1;
        let number = self.next;
        let leaving =
            (toast.kind == ToastKind::Success).then(|| self.leave_after(number, STAYS, cx));
        self.live.push(Live {
            number,
            toast,
            hovered: false,
            leaving,
        });
        cx.notify();
    }

    /// The toasts on screen, oldest first.
    pub fn toasts(&self) -> impl Iterator<Item = &Toast> {
        self.live.iter().map(|live| &live.toast)
    }

    pub fn dismiss(&mut self, number: usize, cx: &mut Context<Self>) {
        self.live.retain(|live| live.number != number);
        cx.notify();
    }

    fn leave_after(&self, number: usize, after: Duration, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(after).await;
            this.update(cx, |this, cx| {
                if this.live.iter().any(|l| l.number == number && !l.hovered) {
                    this.dismiss(number, cx);
                }
            })
            .ok();
        })
    }

    fn hovered(&mut self, number: usize, hovered: bool, cx: &mut Context<Self>) {
        let restart = {
            let Some(live) = self.live.iter_mut().find(|l| l.number == number) else {
                return;
            };
            live.hovered = hovered;
            live.toast.kind == ToastKind::Success
        };
        if !restart {
            return;
        }
        let leaving = (!hovered).then(|| self.leave_after(number, STAYS_AFTER_HOVER, cx));
        if let Some(live) = self.live.iter_mut().find(|l| l.number == number) {
            live.leaving = leaving;
        }
    }

    fn card(&self, number: usize, toast: &Toast, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let failed = toast.kind == ToastKind::Failure;
        let (glyph, ink, wash) = if failed {
            (Icon::Close, p.fail, p.fail_bg)
        } else {
            (Icon::Check, p.ok, p.ok_bg)
        };
        let mark = div()
            .mt(px(2.))
            .size(px(15.))
            .flex_none()
            .rounded_full()
            .bg(wash)
            .flex()
            .items_center()
            .justify_center()
            .child(icon(glyph).size(px(9.)).text_color(ink));
        let text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child(toast.title.clone()),
            )
            .children(toast.detail.clone().map(|detail| {
                div()
                    .text_size(px(12.5))
                    .line_height(px(19.))
                    .text_color(p.mute)
                    .child(detail)
            }));
        let dismiss = div()
            .id(ElementId::Name(format!("toast-{number}-dismiss").into()))
            .aria_label("Dismiss")
            .flex_none()
            .size(px(22.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.dismiss(number, cx)));
        let dismiss = hoverable(dismiss, move |d, t| {
            let ink = mix(p.mute, p.text, t);
            d.bg(p.hover.opacity(t))
                .child(icon(Icon::Close).size(px(12.)).text_color(ink))
        });
        let label = match &toast.detail {
            Some(detail) => format!("{} {detail}", toast.title),
            None => toast.title.to_string(),
        };
        div()
            .id(ElementId::Name(format!("toast-{number}").into()))
            .aria_label(SharedString::from(label))
            .occlude()
            .w(px(WIDTH))
            .flex()
            .items_start()
            .gap(px(10.))
            .pt(px(12.))
            .pb(px(12.))
            .pl(px(14.))
            .pr(px(10.))
            .rounded(px(12.))
            .bg(mix(p.card, p.card2, 0.6))
            .border_1()
            .border_color(if failed { p.fail_line } else { p.line2 })
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.55),
                offset: point(px(0.), px(14.)),
                blur_radius: px(36.),
                spread_radius: px(0.),
                inset: false,
            }])
            .ui_text()
            .text_color(p.text)
            .on_hover(
                cx.listener(move |this, hovered: &bool, _, cx| this.hovered(number, *hovered, cx)),
            )
            .child(mark)
            .child(text)
            .child(dismiss)
            .test_support()
    }
}

impl Render for ToastStack {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        // Newest on top: the column is drawn from the newest down to the oldest.
        self.shown.sync(
            self.live
                .iter()
                .rev()
                .map(|live| (live.number.to_string(), (live.number, live.toast.clone()))),
            now,
            reduce,
        );
        if self.shown.rows().next().is_none() {
            return div().into_any_element();
        }
        let rows: Vec<(usize, Toast, crate::listing::Placement)> = self
            .shown
            .rows()
            .map(|row| (row.value.0, row.value.1.clone(), row.placement))
            .collect();
        let cards: Vec<_> = rows
            .into_iter()
            .map(|(number, toast, placement)| {
                Item::new(
                    ElementId::Name(format!("toast-item-{number}").into()),
                    placement,
                    self.card(number, &toast, cx),
                )
                .rising()
                .gap(px(GAP))
            })
            .collect();
        let viewport = window.viewport_size();
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .w(viewport.width)
                    .h(viewport.height)
                    .flex()
                    .flex_col()
                    .justify_end()
                    .items_end()
                    .pr(px(EDGE))
                    .pb(px(EDGE))
                    .child(
                        div()
                            .w(px(WIDTH))
                            .flex()
                            .flex_col()
                            .gap(px(GAP))
                            .children(cards),
                    ),
            ),
        )
        .priority(3)
        .into_any_element()
    }
}
