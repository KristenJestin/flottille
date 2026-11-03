//! `Menu`: a "⋯" button and the small list of entries it opens, which fades in while rising and
//! fades out on closing.
//!
//! The list is the dropdowns' (drawn after everything else, inside the window). Escape closes it,
//! so does a click anywhere outside it, and so does picking an entry, which runs after the menu
//! has started to close.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    Context, ElementId, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, Styled, TestSupportExt, Window, div, px,
};

use crate::button::Button;
use crate::dropdown::{Align, Handler, MenuItem, TRIGGER, list_overlay};
use crate::icons::Icon;
use crate::motion::{ENTER, HOVER, Motion};

struct Entry {
    id: ElementId,
    label: SharedString,
    leading: Option<Icon>,
    danger: bool,
    pick: Handler,
}

pub struct Menu {
    id: ElementId,
    tooltip: SharedString,
    align: Align,
    width: Pixels,
    entries: Vec<Entry>,
    open: bool,
    fade: Motion,
    focus: FocusHandle,
}

impl Menu {
    /// A closed menu: its button is `id` (`tooltip` says what it is), its list `<id>-menu`.
    pub fn new(
        id: impl Into<ElementId>,
        tooltip: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            tooltip: tooltip.into(),
            align: Align::End,
            width: px(180.),
            entries: Vec::new(),
            open: false,
            fade: Motion::settled(0.0),
            focus: cx.focus_handle(),
        }
    }

    /// An entry `id`, picked by a click.
    pub fn entry(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        leading: Option<Icon>,
        pick: impl Fn(&mut Window, &mut gpui_kit::App) + 'static,
    ) -> Self {
        self.entries.push(Entry {
            id: id.into(),
            label: label.into(),
            leading,
            danger: false,
            pick: Rc::new(pick),
        });
        self
    }

    /// A destructive entry, "Remove…": in the failure colour, after the others and a line,
    /// whatever the order it was given in. Its `pick` opens the [`Confirm`](crate::Confirm).
    pub fn danger(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        pick: impl Fn(&mut Window, &mut gpui_kit::App) + 'static,
    ) -> Self {
        self.entries.push(Entry {
            id: id.into(),
            label: label.into(),
            leading: None,
            danger: true,
            pick: Rc::new(pick),
        });
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.open = true;
        let now = cx.background_executor().now();
        self.fade.animate(1.0, ENTER, now, cx.reduce_motion());
        // Escape goes to the focused element: the menu takes the focus while it is open.
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        let now = cx.background_executor().now();
        self.fade.animate(0.0, HOVER, now, cx.reduce_motion());
        cx.notify();
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.open {
            true => self.close(cx),
            false => self.open(window, cx),
        }
    }
}

impl Focusable for Menu {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        if self.fade.is_running(now) {
            window.request_animation_frame();
        } else {
            self.fade.settle(now);
        }
        let t = self.fade.value(now);
        let trigger = Button::icon(self.id.clone(), Icon::More, self.tooltip.clone())
            .tooltip_hidden(self.open)
            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx)));
        let root = div()
            .id(ElementId::NamedChild(
                Arc::new(self.id.clone()),
                "root".into(),
            ))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                if this.open && event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close(cx);
                }
            }))
            .relative()
            .flex_none()
            .child(trigger);
        if t == 0.0 && !self.open {
            return root.test_support();
        }
        let menu = cx.entity().downgrade();
        let p = crate::theme::palette(cx);
        let item = |entry: &Entry| {
            let (menu, pick) = (menu.clone(), entry.pick.clone());
            let item =
                MenuItem::new(entry.id.clone(), entry.label.clone()).on_click(move |window, cx| {
                    menu.update(cx, |menu, cx| menu.close(cx)).ok();
                    pick(window, cx);
                });
            let item = match entry.leading {
                Some(glyph) => item.leading(glyph),
                None => item,
            };
            if entry.danger { item.danger() } else { item }.into_any_element()
        };
        let mut items: Vec<_> = self
            .entries
            .iter()
            .filter(|e| !e.danger)
            .map(item)
            .collect();
        let dangers: Vec<_> = self.entries.iter().filter(|e| e.danger).map(item).collect();
        if !items.is_empty() && !dangers.is_empty() {
            items.push(
                div()
                    .h(px(1.))
                    .mx(px(2.))
                    .my(px(4.))
                    .bg(p.line)
                    .into_any_element(),
            );
        }
        items.extend(dangers);
        // While it closes, nothing is over the window to take a click.
        let dismiss: Option<Handler> = self.open.then(|| {
            let menu = menu.clone();
            Rc::new(move |_: &mut Window, cx: &mut gpui_kit::App| {
                menu.update(cx, |menu, cx| menu.close(cx)).ok();
            }) as Handler
        });
        root.child(list_overlay(
            &self.id, TRIGGER, self.align, self.width, items, dismiss, t, window, cx,
        ))
        .test_support()
    }
}
