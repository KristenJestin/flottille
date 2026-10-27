//! `Dropdown`: a picker's button and the list it opens, and `MenuItem`, one entry of that list.
//!
//! The list is drawn after everything else (`deferred`), on the opaque card colour with a border
//! and a shadow, so that nothing of the page shows through it or takes its clicks. It stays inside
//! the window (`anchored`): it opens upward when there is no room below. While it is open, a
//! click anywhere outside it closes it and goes no further.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, BoxShadow, ElementId, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    TestSupportExt, Window, anchored, deferred, div, hsla, point, px,
};

use crate::icons::{Icon, icon};
use crate::interact::dismiss_on_navigate;
use crate::motion::RISE;
use crate::theme::palette;
use crate::typography::Type;

/// The height of a picker's button: the list opens this far below its top.
const TRIGGER: f32 = 28.;
/// The space between the button and its list, on either side.
const GAP: f32 = 4.;

/// Which edge of the button the list lines up with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    End,
}

pub(crate) type Handler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    label: SharedString,
    leading: Option<Icon>,
    strong: bool,
    mono: bool,
    open: bool,
    align: Align,
    width: Pixels,
    items: Vec<AnyElement>,
    on_toggle: Option<Handler>,
}

impl Dropdown {
    /// A picker whose button says `label`; its list is `<id>-menu`, over `<id>-backdrop`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            leading: None,
            strong: false,
            mono: false,
            open: false,
            align: Align::Start,
            width: px(260.),
            items: Vec::new(),
            on_toggle: None,
        }
    }

    /// An icon before the label.
    pub fn leading(mut self, glyph: Icon) -> Self {
        self.leading = Some(glyph);
        self
    }

    /// The label in the text colour and medium weight: the picker that matters most.
    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }

    /// The label in the mono face: a model, an effort.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn item(mut self, item: impl IntoElement) -> Self {
        self.items.push(item.into_any_element());
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.items
            .extend(items.into_iter().map(IntoElement::into_any_element));
        self
    }

    /// Called by a click on the button, and by a click outside the open list: the owner opens
    /// or closes it.
    pub fn on_toggle(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Dropdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let toggle = self.on_toggle.clone();
        let button = div()
            .id(self.id.clone())
            .h(px(TRIGGER))
            .px(px(9.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.5))
            .text_color(if self.strong { p.text } else { p.mute })
            .when(self.strong, |d| d.medium())
            .when(self.mono, |d| d.mono(12.))
            .when(self.open, |d| d.bg(p.hover).text_color(p.text))
            .hover(|d| d.bg(p.hover).text_color(p.text))
            .when_some(self.leading, |d, glyph| {
                d.child(icon(glyph).size(px(14.)).text_color(p.faint))
            })
            .child(self.label)
            .child(icon(Icon::Choose).size(px(10.)).text_color(p.faint))
            .when_some(toggle, |d, toggle| {
                d.on_click(move |_, window, cx| toggle(window, cx))
            })
            .test_support();
        let root = div().relative().flex_none().child(button);
        if !self.open {
            return root;
        }
        root.child(list_overlay(
            &self.id,
            self.align,
            self.width,
            self.items,
            self.on_toggle,
            1.0,
            window,
            cx,
        ))
    }
}

/// The list a button opens, drawn after everything else: below the button, or above it when the
/// window has no room below, on the opaque card colour. With a `dismiss`, a backdrop over the
/// whole window takes the click that closes it. `fade` is how far into view it is (1 at rest):
/// an entering list rises into place while it grows opaque.
#[allow(clippy::too_many_arguments)]
pub(crate) fn list_overlay(
    id: &ElementId,
    align: Align,
    width: Pixels,
    items: Vec<AnyElement>,
    dismiss: Option<Handler>,
    fade: f32,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let p = palette(cx);
    let viewport = window.viewport_size();
    let panel_dismiss = dismiss.clone();
    let backdrop = dismiss.map(|dismiss| {
        let on_navigate = dismiss.clone();
        let backdrop = div()
            .id(ElementId::Name(format!("{}-backdrop", id_text(id)).into()))
            .w(viewport.width)
            .h(viewport.height)
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| dismiss(window, cx))
            .test_support();
        anchored()
            .position(point(px(0.), px(0.)))
            .child(dismiss_on_navigate(backdrop, move |window, cx| {
                on_navigate(window, cx)
            }))
    });
    let menu = ElementId::Name(format!("{}-menu", id_text(id)).into());
    let panel = div()
        .id(menu)
        .occlude()
        .when(fade < 1.0, |d| {
            d.relative().top(px(RISE * (1.0 - fade))).opacity(fade)
        })
        .w(width)
        .p(px(6.))
        .rounded(px(10.))
        .bg(p.card)
        .border_1()
        .border_color(p.line2)
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.5),
            offset: point(px(0.), px(8.)),
            blur_radius: px(24.),
            spread_radius: px(-4.),
            inset: false,
        }])
        .ui_text()
        .text_color(p.text)
        .flex()
        .flex_col()
        .children(items)
        .test_support();
    let panel = match panel_dismiss {
        Some(dismiss) => dismiss_on_navigate(panel, move |window, cx| dismiss(window, cx)),
        None => panel,
    };
    // The padding is the gap on either side.
    let list = anchored()
        .anchor(match align {
            Align::Start => Anchor::TopLeft,
            Align::End => Anchor::TopRight,
        })
        .offset(point(px(0.), px(TRIGGER - GAP)))
        .child(div().py(px(GAP)).child(panel));
    div()
        .absolute()
        .top_0()
        .map(|d| match align {
            Align::Start => d.left_0(),
            Align::End => d.right_0(),
        })
        .child(deferred(div().children(backdrop).child(list)).priority(1))
}

fn id_text(id: &ElementId) -> String {
    match id {
        ElementId::Name(name) => name.to_string(),
        other => other.to_string(),
    }
}

/// One entry of a dropdown's list: an icon, its label, a note on the right; picked by a click,
/// or faint and inert without one.
#[derive(IntoElement)]
pub struct MenuItem {
    id: ElementId,
    label: SharedString,
    leading: Option<Icon>,
    note: Option<SharedString>,
    selected: bool,
    on_click: Option<Handler>,
}

impl MenuItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            leading: None,
            note: None,
            selected: false,
            on_click: None,
        }
    }

    pub fn leading(mut self, glyph: Icon) -> Self {
        self.leading = Some(glyph);
        self
    }

    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// The entry picked now.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for MenuItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let active = self.on_click.is_some();
        div()
            .id(self.id)
            .flex_none()
            .min_h(px(28.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.5))
            .text_color(if active { p.text } else { p.faint })
            .when(self.selected, |d| d.bg(p.sel))
            .when(active && !self.selected, |d| d.hover(|d| d.bg(p.hover)))
            .when_some(self.leading, |d, glyph| {
                d.child(icon(glyph).size(px(14.)).text_color(p.faint))
            })
            .child(div().flex_1().min_w_0().child(self.label))
            .when_some(self.note, |d, note| {
                d.child(div().flex_none().mono(11.).text_color(p.faint).child(note))
            })
            .when_some(self.on_click, |d, f| {
                d.cursor_pointer()
                    .on_click(move |_, window, cx| f(window, cx))
            })
            .test_support()
    }
}
