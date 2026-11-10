//! `Subpage`: a list of rows that opens the page of one of them in place of itself, inside the
//! tab that holds it (the Features tabs). The page comes in from the right, fading in, while the
//! list leaves to the left; going back is the reverse. Above the page a path line,
//! `Features / Review`, whose first word goes back, as Escape does.
//!
//! The component holds no choice: the screen says which entry is open (`open`) and is told when
//! the user opens one or goes back (`on_open`). A screen that keeps its choice keeps the page
//! open while another tab is shown, and the page is simply there when the tab comes back.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyView, App, ElementId, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::color::mix;
use crate::icons::{Icon, icon};
use crate::interact::hoverable;
use crate::motion::{Motion, PANEL};
use crate::rows::{ListCard, ListRow};
use crate::theme::palette;
use crate::typography::Type;

type OnOpen = Rc<dyn Fn(Option<SharedString>, &mut Window, &mut App) + 'static>;

/// How far a page travels while it comes in and the list goes out, in pixels: they move a little
/// and fade, as the detail drawer does, rather than cross the whole width.
const SLIDE: f32 = 32.;

struct Entry {
    key: SharedString,
    icon: Icon,
    title: SharedString,
    tag: SharedString,
    line: SharedString,
}

/// What a subpage remembers between frames: how far the page is in (0 the list, 1 the page), the
/// entry it last showed (whose row keeps its mark while the list comes back, and whose name the
/// path line keeps while it leaves), the page itself while it slides out, and whether Escape has been given its focus.
struct Slide {
    motion: Motion,
    last: Option<SharedString>,
    page: Option<AnyView>,
    focused: bool,
}

#[derive(IntoElement)]
pub struct Subpage {
    id: ElementId,
    label: SharedString,
    entries: Vec<Entry>,
    open: Option<SharedString>,
    on_open: Option<OnOpen>,
    page: Option<AnyView>,
}

impl Subpage {
    /// `label` names the list and starts the path line (`Features`). The list is `<id>-list`, an
    /// entry `<id>-<key>` (its tag `<id>-<key>-tag-0`), the path line `<id>-path` and the word
    /// that goes back `<id>-back`, the page's frame `<id>-page`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            entries: Vec::new(),
            open: None,
            on_open: None,
            page: None,
        }
    }

    /// A row: its icon, its name, its state said once as `tag` (`On`, `Off`, `2 on`) and the one
    /// `line` that says what it is for, never its state.
    pub fn entry(
        mut self,
        key: impl Into<SharedString>,
        icon: Icon,
        title: impl Into<SharedString>,
        tag: impl Into<SharedString>,
        line: impl Into<SharedString>,
    ) -> Self {
        self.entries.push(Entry {
            key: key.into(),
            icon,
            title: title.into(),
            tag: tag.into(),
            line: line.into(),
        });
        self
    }

    /// The key of the entry whose page is shown, if any.
    pub fn open(mut self, key: Option<&str>) -> Self {
        self.open = key.map(|key| SharedString::from(key.to_string()));
        self
    }

    /// Called with the key of the entry clicked, or `None` when the path line or Escape goes back.
    pub fn on_open(
        mut self,
        f: impl Fn(Option<SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Rc::new(f));
        self
    }

    /// The page of the entry that is open, a view: the subpage keeps it until the page has slid
    /// out, so the screen gives it only while an entry is open and nothing while it leaves.
    pub fn page(mut self, page: impl Into<AnyView>) -> Self {
        self.page = Some(page.into());
        self
    }
}

impl RenderOnce for Subpage {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let id_text = match &self.id {
            ElementId::Name(name) => name.to_string(),
            other => format!("{other:?}"),
        };
        let part = |tail: &str| ElementId::Name(format!("{id_text}-{tail}").into());
        let key = ElementId::NamedChild(Arc::new(self.id.clone()), "slide".into());
        let open = self.open.is_some();
        let state = window.use_keyed_state(key, cx, |_, _| Slide {
            motion: Motion::settled(if open { 1.0 } else { 0.0 }),
            last: self.open.clone(),
            page: None,
            focused: false,
        });
        let focus = window
            .use_keyed_state(
                ElementId::NamedChild(Arc::new(self.id.clone()), "focus".into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();

        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let target = if open { 1.0 } else { 0.0 };
        let (motion, last, focused, page) = state.update(cx, |slide, _| {
            if self.open.is_some() {
                slide.last = self.open.clone();
                slide.page = self.page.clone();
            }
            if slide.motion.target() != target {
                slide.motion.animate(target, PANEL, now, reduce);
            }
            (
                slide.motion,
                slide.last.clone(),
                slide.focused,
                slide.page.clone(),
            )
        });
        let running = motion.is_running(now);
        if running {
            window.request_animation_frame();
        } else {
            state.update(cx, |slide, _| {
                slide.motion.settle(now);
                if !open {
                    slide.page = None;
                }
            });
        }
        // Escape reaches the focused element: the page takes the focus as it opens, unless
        // something in it (a field) already holds it.
        if open != focused {
            state.update(cx, |slide, _| slide.focused = open);
            if open {
                let focus = focus.clone();
                window.defer(cx, move |window, cx| {
                    if !focus.contains_focused(window, cx) {
                        window.focus(&focus, cx);
                    }
                });
            }
        }
        let at = motion.value(now);

        let on_open = self.on_open;
        let list_shown = at < 1.0;
        let page_shown = at > 0.0 || open;
        let title_of = |key: &SharedString| {
            self.entries
                .iter()
                .find(|entry| &entry.key == key)
                .map_or_else(|| key.clone(), |entry| entry.title.clone())
        };

        let list = list_shown.then(|| {
            let rows = self.entries.iter().map(|entry| {
                let row = ListRow::new(part(&entry.key), entry.title.clone())
                    .leading(icon(entry.icon).text_color(p.mute))
                    .tag(entry.tag.clone())
                    .line(entry.line.clone())
                    // The row that was left keeps its mark while the list comes back, then lets go.
                    .marked(at > 0.0 && last.as_ref() == Some(&entry.key));
                match on_open.clone() {
                    Some(on_open) => {
                        let key = entry.key.clone();
                        row.on_click(move |_, window, cx| on_open(Some(key.clone()), window, cx))
                    }
                    None => row,
                }
            });
            let card = ListCard::new(part("list")).rows(rows);
            div()
                .w_full()
                .relative()
                .when(open, |d| d.absolute().top_0().left_0())
                .left(px(-SLIDE * at))
                .opacity(1.0 - at)
                .child(card)
        });

        let page = page_shown.then(|| {
            let mut back = div()
                .id(part("back"))
                .aria_label(self.label.clone())
                .cursor_pointer()
                .child(self.label.clone());
            if let Some(on_open) = on_open.clone().filter(|_| open) {
                back = back.on_click(move |_, window, cx| on_open(None, window, cx));
            }
            let back = hoverable(back, move |d, hover| {
                d.text_color(mix(p.faint, p.text, hover))
            });
            let path = div()
                .id(part("path"))
                .aria_label(match &last {
                    Some(key) => format!("{} / {}", self.label, title_of(key)),
                    None => self.label.to_string(),
                })
                .flex()
                .items_baseline()
                .gap(px(6.))
                .mono(12.)
                .text_color(p.faint)
                .whitespace_nowrap()
                .child(back)
                .child("/")
                .child(
                    div()
                        .text_color(p.text)
                        .children(last.as_ref().map(title_of)),
                )
                .test_support();
            let escape = on_open.clone();
            div()
                .id(part("page"))
                .track_focus(&focus)
                .w_full()
                .flex()
                .flex_col()
                .gap(px(16.))
                .relative()
                .when(!open, |d| d.absolute().top_0().left_0())
                .left(px(SLIDE * (1.0 - at)))
                .opacity(at)
                // How far the page is in, for a test and for assistive technology.
                .aria_value(format!("{at}"))
                .when(open, |d| {
                    d.on_key_down(move |event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape"
                            && let Some(on_open) = &escape
                        {
                            cx.stop_propagation();
                            on_open(None, window, cx);
                        }
                    })
                })
                .child(path)
                .children(page)
                .test_support()
        });

        div()
            .id(self.id.clone())
            .w_full()
            .relative()
            // Part way, the one that leaves and the one that arrives are both drawn: neither may
            // spill out of the tab.
            .when(running, |d| d.overflow_hidden())
            .children(page)
            .children(list)
            .test_support()
    }
}
