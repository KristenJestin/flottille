//! A settings page's shape, shared by the project settings and Settings: a header, an index of
//! the sections that stays at the top of the column once the page scrolls past it, then the
//! sections. A click on an index entry scrolls its section to the top, over `PANEL`; the entry of
//! the section in sight is lit.

use flottille_ui::motion::{Motion, PANEL};
use flottille_ui::{PageIndex, enter, fade, space};
use gpui_kit::{
    AnyElement, App, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    ScrollHandle, StatefulInteractiveElement, Styled, Window, div, point, px,
};

/// The children of the page's column before its sections: the header and the index.
const BEFORE_SECTIONS: usize = 2;

/// Where a page of sections is scrolled, and the scroll to a section on its way.
#[derive(Default)]
pub struct Sections {
    scroll: ScrollHandle,
    /// From where to where, and how far it has gone.
    scrolling: Option<(f32, f32, Motion)>,
}

impl Sections {
    /// Where a child of the column is drawn now: its bounds are kept as laid out, before the
    /// scroll moves them.
    fn top_of(&self, child: usize) -> Option<Pixels> {
        let bounds = self.scroll.bounds_for_item(child)?;
        Some(bounds.top() + self.scroll.offset().y)
    }

    /// The index entry of the section in sight: the last whose top has reached the index.
    pub fn current(&self, count: usize) -> usize {
        let Some(index) = self.scroll.bounds_for_item(1) else {
            return 0;
        };
        let line = self.scroll.bounds().top() + index.size.height + px(8.);
        (0..count)
            .take_while(|at| {
                self.top_of(BEFORE_SECTIONS + at)
                    .is_some_and(|top| top <= line)
            })
            .last()
            .unwrap_or(0)
    }

    /// Where the scroll is, and where it goes for the section of that index entry to be at the
    /// top, under the index: `None` before the page is laid out.
    fn target(&self, entry: usize) -> Option<(f32, f32)> {
        let section = self.scroll.bounds_for_item(BEFORE_SECTIONS + entry)?;
        let index = self.scroll.bounds_for_item(1)?;
        let from = f32::from(self.scroll.offset().y);
        let top = f32::from(section.top() - self.scroll.bounds().top());
        let to = (f32::from(index.size.height) + 8. - top)
            .min(0.)
            .max(-f32::from(self.scroll.max_offset().y));
        Some((from, to))
    }

    /// Scrolls the section of that index entry to the top, under the index.
    pub fn go_to(&mut self, entry: usize, cx: &App) {
        let Some((from, to)) = self.target(entry) else {
            return;
        };
        let mut motion = Motion::settled(0.);
        motion.animate(
            1.,
            PANEL,
            cx.background_executor().now(),
            cx.reduce_motion(),
        );
        self.scrolling = Some((from, to, motion));
    }

    /// Whether the section of that index entry stands where [`Sections::go_to`] puts it, the
    /// page laid out and no scroll on its way; if not, it goes there.
    pub fn arrive_at(&mut self, entry: usize, cx: &App) -> bool {
        if self.scrolling.is_some() {
            return false;
        }
        match self.target(entry) {
            Some((from, to)) if (from - to).abs() < 1. => true,
            Some(_) => {
                self.go_to(entry, cx);
                false
            }
            None => false,
        }
    }

    /// Whether a scroll to a section is on its way.
    pub fn is_scrolling(&self) -> bool {
        self.scrolling.is_some()
    }

    /// Moves the scroll on its way one frame further.
    fn drive(&mut self, window: &mut Window, cx: &App) {
        let Some((from, to, motion)) = &self.scrolling else {
            return;
        };
        let now = cx.background_executor().now();
        let at = from + (to - from) * motion.value(now);
        self.scroll.set_offset(point(px(0.), px(at)));
        if motion.is_running(now) {
            window.request_animation_frame();
        } else {
            self.scrolling = None;
        }
    }

    /// The page: `header`, the index made by `index` (with its id), then `sections`, each
    /// entering after the one before. `key` is the prefix of their entrance's ids.
    #[allow(clippy::too_many_arguments)]
    pub fn page<V: 'static>(
        &mut self,
        id: &'static str,
        key: &str,
        pad: Pixels,
        header: AnyElement,
        index: impl Fn(ElementId, usize) -> PageIndex,
        sections: Vec<(&'static str, AnyElement)>,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
        self.drive(window, cx);
        let current = self.current(sections.len());
        let key = |part: &str| ElementId::Name(format!("enter-{key}-{part}").into());
        let column = |element: AnyElement| {
            div()
                .w_full()
                .max_w(space::COLUMN)
                .flex_none()
                .child(element)
        };
        let mut page = div()
            .id(id)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            // The lit entry and the index kept at the top follow the scroll.
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .flex()
            .flex_col()
            .items_center()
            .px(pad)
            .pt(space::XXL)
            .pb(space::XXXL)
            .child(column(
                enter(key("header"), 0, div().pb(space::LG).child(header)).into_any_element(),
            ))
            .child(column(
                enter(key("index"), 1, index(id_of(id, "index"), current)).into_any_element(),
            ));
        for (at, (name, section)) in sections.into_iter().enumerate() {
            page = page
                .child(column(enter(key(name), 2 + at, section).into_any_element()).pt(space::XXL));
        }
        let stuck = self
            .top_of(1)
            .is_some_and(|top| top < self.scroll.bounds().top());
        let shown = fade(id_of(id, "index-stuck"), stuck, PANEL, window, cx);
        let overlay = (shown > 0.).then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .px(pad)
                .flex()
                .justify_center()
                .opacity(shown)
                .occlude()
                .child(
                    div()
                        .w_full()
                        .max_w(space::COLUMN)
                        .child(index(id_of(id, "index-stuck"), current)),
                )
        });
        div().relative().size_full().child(page).children(overlay)
    }
}

/// `<id>-<part>`.
fn id_of(id: &str, part: &str) -> ElementId {
    ElementId::Name(format!("{id}-{part}").into())
}
