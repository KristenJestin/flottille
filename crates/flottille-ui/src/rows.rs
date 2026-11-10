//! Rows: [`TreeRow`] for a tree of folding levels and [`ListRow`] for an item of a card or a
//! result of a search.
//!
//! A row's status mark is a view handed in by the screen, so a change of the mark or a tick of
//! its animation redraws that mark and not the row.

use std::f32::consts::{FRAC_PI_2, PI};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, FontWeight, HighlightStyle,
    InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce, ScrollHandle, SharedString,
    Stateful, StatefulInteractiveElement, Styled, StyledText, TestSupportExt, Transformation,
    Window, div, px, radians,
};

use crate::color::mix;
use crate::cut::tipped_when_cut;
use crate::icons::{Icon, icon};
use crate::interact::{choose, fade, feel, wire, wire_with};
use crate::listing::{Item, Placement};
use crate::menu::Menu;
use crate::motion::{HOVER, Motion, PANEL};
use crate::reveal::changed;
use crate::theme::{palette, radius};
use crate::typography::Type;

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type Decorate = Box<dyn FnOnce(Stateful<Div>) -> Stateful<Div> + 'static>;
type OnEnter = Box<dyn Fn(&mut App) + 'static>;

/// A row of a tree. The tree has no fixed depth: a row is placed under another with
/// [`tree_children`], which indents it, and what a level is (a folder, a branch) is said by the
/// row's [`leading`](Self::leading) mark.
#[derive(IntoElement)]
pub struct TreeRow {
    id: ElementId,
    top: bool,
    title: SharedString,
    expanded: Option<bool>,
    /// A chevron on a row that is no first-level row's, which turns up as what it holds unfolds above it.
    upward: bool,
    selected: bool,
    trailing: Option<AnyElement>,
    actions: Option<(AnyElement, bool)>,
    /// A faint line under the title: the row is two lines high.
    second: Option<AnyElement>,
    /// A mark before the title, where a folder or a branch has its icon.
    leading: Option<AnyElement>,
    on_click: Option<OnClick>,
    on_toggle: Option<OnClick>,
}

/// The height of a tree row that holds a second line.
pub const TWO_LINES: f32 = 40.;

/// The element id of a row's chevron, which folds and unfolds the level below.
pub fn toggle_id(row: impl Into<ElementId>) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(row.into()), "toggle".into())
}

impl TreeRow {
    /// A row of the tree, 28 px high.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            top: false,
            title: title.into(),
            expanded: None,
            upward: false,
            selected: false,
            trailing: None,
            actions: None,
            second: None,
            leading: None,
            on_click: None,
            on_toggle: None,
        }
    }

    /// A row of the first level: 32 px, weight 500, a chevron before its title.
    pub fn top(mut self) -> Self {
        self.top = true;
        self
    }

    /// A mark before the title: what kind of row it is, said in its own tooltip.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    /// A faint line under the title (the place an item works in): the row takes two lines, and
    /// its trailing mark and actions stay on the title's.
    pub fn second(mut self, element: impl IntoElement) -> Self {
        self.second = Some(element.into_any_element());
        self
    }

    /// Whether the level below is open; a first-level row turns its chevron with it.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// A row whose group unfolds above it: its chevron turns from right to up.
    pub fn unfolds_upward(mut self) -> Self {
        self.upward = true;
        self
    }

    /// The open row.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// On the right: a state mark when running or waiting, otherwise a time (see [`meta`]).
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }

    /// What the row offers under the pointer (a "⋯" menu, a "+"): it opens beside the trailing
    /// mark over [`HOVER`](crate::motion::HOVER) as the pointer comes and closes as it leaves,
    /// and stays while `keep` (its menu is open).
    pub fn actions(mut self, element: impl IntoElement, keep: bool) -> Self {
        self.actions = Some((element.into_any_element(), keep));
        self
    }

    /// A click on the chevron alone, which then does not reach the row's own click.
    pub fn on_toggle(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }
}

impl RenderOnce for TreeRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let (state, now) = feel(&self.id, window, cx);
        let top = self.top;
        let chosen = choose(&state, self.selected, window, cx);
        let ink = if top {
            p.text
        } else {
            mix(mix(p.mute, p.text, now.hover), p.text, chosen)
        };
        let background = mix(p.hover.opacity(now.hover), p.sel, chosen);
        let chevron_turn = self
            .expanded
            .map(|open| turning(self.id.clone(), open, window, cx));

        let mut row = div()
            .id(self.id.clone())
            .aria_label(self.title.clone())
            .w_full()
            .flex()
            .items_center()
            .gap(px(if top { 8. } else { 7. }))
            .h(px(match (top, self.second.is_some()) {
                (true, _) => 32.,
                (false, false) => 28.,
                (false, true) => TWO_LINES,
            }))
            .px(px(8.))
            .rounded(radius::ROW)
            .bg(background)
            .text_color(ink)
            .ui_text()
            .text_size(px(if top { 13.5 } else { 13. }))
            .when(top, |d| d.font_weight(FontWeight::MEDIUM))
            .when(self.expanded.is_some(), |d| {
                d.aria_expanded(self.expanded == Some(true))
            });
        // A row turns a chevron when it folds a level of its own (`on_toggle`). It sits
        // after the title, so that every row of a level starts its title at the same x, with
        // or without children under it.
        let folds = self.on_toggle.is_some();
        let mut fold = None;
        if let Some(turn) = chevron_turn.filter(|_| top || self.upward || folds) {
            let angle = match self.upward {
                false => -FRAC_PI_2 * turn,
                // Folded (1) points right as a first-level row's does; open (0) points up.
                true => -PI + FRAC_PI_2 * turn,
            };
            let chevron = icon(Icon::Chevron)
                .size(px(12.))
                .text_color(p.faint)
                .with_transformation(Transformation::rotate(radians(angle)));
            let chevron = match self.on_toggle {
                Some(on_toggle) => div()
                    .id(toggle_id(self.id.clone()))
                    .flex_none()
                    .p(px(4.))
                    .m(px(-4.))
                    .cursor_pointer()
                    // The row's own click is the page; this one only folds.
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        on_toggle(event, window, cx);
                    })
                    .child(chevron)
                    .test_support()
                    .into_any_element(),
                None => chevron.into_any_element(),
            };
            match top || self.upward {
                true => row = row.child(chevron),
                false => fold = Some(chevron),
            }
        }
        if let Some(leading) = self.leading {
            row = row.child(leading);
        }
        // A title that changes (one the user or a program named) fades to the new one.
        let title_id = ElementId::Name(format!("{}-title", self.id).into());
        let opacity = changed(&title_id, &self.title, window, cx);
        let title = tipped_when_cut(
            div()
                .id(title_id)
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .opacity(opacity)
                .child(self.title.clone()),
            self.title.clone(),
            window,
            cx,
        );
        let trailing = self
            .trailing
            .map(|trailing| div().flex_none().flex().items_center().child(trailing));
        let actions = self.actions.and_then(|(actions, keep)| {
            let t = if keep { 1.0 } else { now.hover };
            (t > 0.0).then(|| {
                div()
                    .id(ElementId::NamedChild(
                        std::sync::Arc::new(self.id.clone()),
                        "actions".into(),
                    ))
                    .flex_none()
                    .flex()
                    .items_center()
                    .w(px(24. * t))
                    .opacity(t)
                    // What the row offers is not a click on the row.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(actions)
            })
        });
        row = match self.second {
            None => row
                .child(title)
                .children(trailing)
                .children(actions)
                .children(fold),
            Some(second) => row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .h(px(18.))
                            .child(title)
                            .children(trailing)
                            .children(actions)
                            .children(fold),
                    )
                    .child(second),
            ),
        };
        if let Some(on_click) = self.on_click {
            row = row.on_click(on_click);
        }
        wire(row, state, false).test_support()
    }
}

/// How far a chevron has turned from open (0) to folded (1), turning over [`PANEL`] when `open`
/// changes. `id` is the chevron's, which keeps the state.
pub fn turning(id: impl Into<ElementId>, open: bool, window: &mut Window, cx: &mut App) -> f32 {
    let angle = window.use_keyed_state(
        ElementId::NamedChild(std::sync::Arc::new(id.into()), "chevron".into()),
        cx,
        |_, _| Motion::settled(if open { 0.0 } else { 1.0 }),
    );
    let t = cx.background_executor().now();
    let reduce = cx.reduce_motion();
    let target = if open { 0.0 } else { 1.0 };
    if angle.read(cx).target() != target {
        angle.update(cx, |m, _| m.animate(target, PANEL, t, reduce));
    }
    let m = *angle.read(cx);
    if m.is_running(t) {
        window.request_animation_frame();
    }
    m.value(t)
}

/// The level below a tree row, indented on a 1 px guide line.
pub fn tree_children(children: impl IntoIterator<Item = AnyElement>, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .ml(px(14.))
        .pl(px(6.))
        .my(px(2.))
        .border_l_1()
        .border_color(p.line)
        .flex()
        .flex_col()
        .children(children)
}

/// Where a row sits among the rows its lead leads, so that a connector can tie it
/// to the row above: one entry of `through` for each ancestor under the first row of the tree,
/// true while that ancestor has a sibling below it (its line goes on past this row), and whether
/// this row is the last of its lead's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tie {
    pub through: Vec<bool>,
    pub last: bool,
}

impl Tie {
    /// How many levels the row is under the first row of the tree.
    pub fn depth(&self) -> usize {
        self.through.len() + 1
    }
}

/// How far each level of led rows is indented: `space::XL`, the step the window draws it with.
const TIE_INDENT: f32 = 24.;
/// Where the connector's first line runs: under the first letters of a row of the first level.
const TIE_ROOT: f32 = 14.;
/// How round the connector turns.
const TIE_TURN: f32 = 6.;
/// How far past the indent the connector runs into its row.
const TIE_INTO: f32 = 4.;

/// The element id of a row's connector, whose label says whether the row is the last one.
pub fn tie_id(row: impl Into<ElementId>) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(row.into()), "tie".into())
}

/// The element id of the turn of a row's connector.
pub fn tie_turn_id(row: impl Into<ElementId>) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(tie_id(row)), "turn".into())
}

/// The element id of the line that goes on below a row's turn, to the next row of its lead's.
pub fn tie_below_id(row: impl Into<ElementId>) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(tie_id(row)), "below".into())
}

/// The element id of the line that goes on past a row for its ancestor of this level.
pub fn tie_through_id(row: impl Into<ElementId>, level: usize) -> ElementId {
    ElementId::NamedChild(
        std::sync::Arc::new(tie_id(row)),
        format!("through-{level}").into(),
    )
}

/// The connector of a led row to its lead's: a thin line that comes down from the row
/// above and turns right into the row, at the middle of its first line (`first_line`, from the
/// row's top), going on below it while the row has a sibling after it, and the lines of the
/// ancestors that go on past it. Draw it over the row, in a block that starts where the row's
/// indent starts; `shown` fades it with the row. The line ends where it turns into the row.
pub fn tree_tie(
    row: impl Into<ElementId>,
    tie: &Tie,
    first_line: f32,
    shown: f32,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    let row = row.into();
    let own = TIE_ROOT + TIE_INDENT * (tie.depth() - 1) as f32;
    let line = |x: f32| {
        div()
            .absolute()
            .left(px(x))
            .top_0()
            .bottom_0()
            .w(px(1.))
            .bg(p.line)
    };
    div()
        .id(tie_id(row.clone()))
        .aria_label(if tie.last { "Last branch" } else { "Branch" })
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w_full()
        .opacity(shown)
        .children(
            tie.through
                .iter()
                .enumerate()
                .filter(|(_, on)| **on)
                .map(|(level, _)| {
                    line(TIE_ROOT + TIE_INDENT * level as f32)
                        .id(tie_through_id(row.clone(), level))
                        .test_support()
                }),
        )
        .child(
            div()
                .id(tie_turn_id(row.clone()))
                .absolute()
                .left(px(own))
                .top_0()
                .w(px(TIE_INDENT * tie.depth() as f32 + TIE_INTO - own))
                .h(px(first_line))
                .border_l_1()
                .border_b_1()
                .border_color(p.line)
                .rounded_bl(px(TIE_TURN))
                .test_support(),
        )
        .when(!tie.last, |d| {
            d.child(
                line(own)
                    .top(px(first_line))
                    .id(tie_below_id(row.clone()))
                    .test_support(),
            )
        })
        .test_support()
}

/// Mono, faint metadata: a time ago, a model, a count.
pub fn meta(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .mono(11.)
        .text_color(p.faint)
        .whitespace_nowrap()
        .child(text.into())
}

/// A section label: mono 11 px, uppercase, faint (`PLACES`, `RECENT`).
pub fn section_label(text: &str, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .px(px(8.))
        .pt(px(10.))
        .pb(px(6.))
        .mono(11.)
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.faint)
        .child(text.to_uppercase())
}

#[derive(IntoElement)]
pub struct ListRow {
    id: ElementId,
    leading: Option<AnyElement>,
    title: SharedString,
    mono: bool,
    tags: Vec<SharedString>,
    detail: Option<SharedString>,
    line: Option<SharedString>,
    meta: Option<SharedString>,
    trailing: Option<AnyElement>,
    more: Option<Entity<Menu>>,
    below: Option<AnyElement>,
    mono_line: bool,
    leading_wide: bool,
    decorate: Option<Decorate>,
    in_card: bool,
    /// Rounded at the top, at the bottom: the first and the last row of a card.
    corners: (bool, bool),
    selected: bool,
    marked: bool,
    pieces: Option<Vec<(String, bool)>>,
    on_enter: Option<OnEnter>,
    on_click: Option<OnClick>,
}

impl ListRow {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            leading: None,
            title: title.into(),
            mono: false,
            tags: Vec::new(),
            detail: None,
            line: None,
            meta: None,
            trailing: None,
            more: None,
            below: None,
            mono_line: false,
            leading_wide: false,
            decorate: None,
            in_card: false,
            corners: (false, false),
            selected: false,
            marked: false,
            pieces: None,
            on_enter: None,
            on_click: None,
        }
    }

    /// The title in pieces, each with whether it matched what was typed
    /// ([`Match::pieces`](crate::fuzzy::Match::pieces)): those that did are drawn in the accent.
    /// The row's label stays its whole title.
    pub fn pieces(mut self, pieces: Vec<(String, bool)>) -> Self {
        self.pieces = Some(pieces);
        self
    }

    /// The row the keys move to in a list they move in (a search's results): filled as a
    /// selected row, the fill moving over [`HOVER`] as the mark goes from row to row.
    pub fn marked(mut self, marked: bool) -> Self {
        self.marked = marked;
        self
    }

    /// The pointer entered the row: a list the keys move in marks it.
    pub fn on_enter(mut self, f: impl Fn(&mut App) + 'static) -> Self {
        self.on_enter = Some(Box::new(f));
        self
    }

    /// The title in the mono face: a name that is an identifier (`style`, `open-prs`).
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// A small bordered mono tag after the title: a path, a kind.
    pub fn tag(mut self, tag: impl Into<SharedString>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Its "⋯" menu (Edit, the moves, "Remove…"), shown while the pointer is on the row or the
    /// menu is open: it fades in and out with the row's hover, in `<id>-actions`.
    pub fn more(mut self, menu: Entity<Menu>) -> Self {
        self.more = Some(menu);
        self
    }

    /// A row of a [`ListCard`]: full width, 52 px at least, a line between rows.
    pub(crate) fn in_card(mut self) -> Self {
        self.in_card = true;
        self
    }

    /// The state icon (or another 16 px mark) on the left.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    /// A mark wider than 16 px on the left: a setup step's grip and number.
    pub fn leading_wide(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self.leading_wide = true;
        self
    }

    /// The line under the title in the mono face: a command line.
    pub fn mono_line(mut self) -> Self {
        self.mono_line = true;
        self
    }

    /// Under the title and its line: labelled values (a repository's base branch, remote and
    /// forge), a state with its mark.
    pub fn below(mut self, element: impl IntoElement) -> Self {
        self.below = Some(element.into_any_element());
        self
    }

    /// What the row's own element gets besides: where a dragged row is dropped (a setup step).
    pub fn decorate(mut self, f: impl FnOnce(Stateful<Div>) -> Stateful<Div> + 'static) -> Self {
        self.decorate = Some(Box::new(f));
        self
    }

    /// Mono, beside the title: a model.
    pub fn detail(mut self, text: impl Into<SharedString>) -> Self {
        self.detail = Some(text.into());
        self
    }

    /// A sentence under the title: what a command or a template does. It is the element
    /// `<id>-line`.
    pub fn line(mut self, text: impl Into<SharedString>) -> Self {
        self.line = Some(text.into());
        self
    }

    /// Mono, before the trailing mark: what it is doing.
    pub fn meta(mut self, text: impl Into<SharedString>) -> Self {
        self.meta = Some(text.into());
        self
    }

    /// The pill or a time, on the right.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for ListRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let (state, now) = feel(&self.id, window, cx);
        let marked = fade(
            ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "marked".into()),
            self.marked,
            HOVER,
            window,
            cx,
        );
        let chosen = choose(&state, self.selected, window, cx).max(marked);
        let background = mix(p.hover.opacity(now.hover), p.card2, chosen);
        let menu_open = self
            .more
            .as_ref()
            .is_some_and(|menu| menu.read(cx).is_open());
        let actions = self.more.map(|menu| {
            let shown = if menu_open { 1.0 } else { now.hover };
            div()
                .id(ElementId::Name(format!("{}-actions", self.id).into()))
                .flex_none()
                .opacity(shown)
                // The menu's clicks are its own, not the row's (which edits it).
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(menu)
                .test_support()
        });
        let mut row = div()
            .id(self.id.clone())
            .aria_label(self.title.clone())
            .w_full()
            .flex()
            .items_center()
            .bg(background)
            .text_color(p.text)
            .ui_text();
        row = if self.in_card {
            row.gap(px(12.))
                .min_h(px(52.))
                .pt(px(10.))
                .pb(px(10.))
                .pl(px(14.))
                .pr(px(8.))
                .when(self.corners.0, |d| d.rounded_t(px(11.)))
                .when(self.corners.1, |d| d.rounded_b(px(11.)))
        } else {
            row.gap(px(10.))
                .px(px(10.))
                .py(px(8.))
                .rounded(radius::LIST_ROW)
        };
        // A tag is a word or a path someone wrote: it is cut with an ellipsis at 160 px (its
        // whole text in a tooltip) so that the title keeps its room, rather than push the row
        // out of its place.
        let tags: Vec<AnyElement> = self
            .tags
            .into_iter()
            .enumerate()
            .map(|(index, tag)| {
                let element = div()
                    .id(ElementId::Name(format!("{}-tag-{index}", self.id).into()))
                    .aria_label(tag.clone())
                    .min_w_0()
                    .max_w(px(160.))
                    .overflow_hidden()
                    .mono(11.)
                    .text_color(p.faint)
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(p.line)
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(tag.clone());
                tipped_when_cut(element, tag, window, cx)
            })
            .collect();
        let mono = self.mono;
        let mut row = row
            .child(
                div()
                    .when(!self.leading_wide, |d| d.w(px(16.)).justify_center())
                    .flex_none()
                    .flex()
                    .items_center()
                    .children(self.leading),
            )
            .child({
                let mut whole = self.title.clone();
                let name = div()
                    .id(ElementId::Name(format!("{}-title", self.id).into()))
                    .font_weight(FontWeight::MEDIUM)
                    .text_ellipsis()
                    .overflow_hidden()
                    .when(mono, |d| d.mono(13.).font_weight(FontWeight::MEDIUM))
                    .child(match self.pieces {
                        Some(pieces) => {
                            let mut at = 0;
                            let marks: Vec<_> = pieces
                                .iter()
                                .filter_map(|(piece, hit)| {
                                    let range = at..at + piece.len();
                                    at = range.end;
                                    hit.then_some((
                                        range,
                                        HighlightStyle {
                                            color: Some(p.acc),
                                            font_weight: Some(FontWeight::SEMIBOLD),
                                            ..Default::default()
                                        },
                                    ))
                                })
                                .collect();
                            let text: String = pieces.into_iter().map(|(piece, _)| piece).collect();
                            whole = text.clone().into();
                            StyledText::new(text)
                                .with_highlights(marks)
                                .into_any_element()
                        }
                        None => self.title.clone().into_any_element(),
                    });
                let name = tipped_when_cut(name, whole, window, cx);
                let title = div()
                    .flex()
                    .items_baseline()
                    .gap(px(if self.in_card { 8. } else { 6. }))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(name)
                    .children(tags)
                    .when_some(self.detail, |d, detail| {
                        d.child(
                            div()
                                .id(ElementId::Name(format!("{}-detail", self.id).into()))
                                .mono(11.5)
                                .text_color(p.faint)
                                .aria_label(detail.clone())
                                .child(detail)
                                .test_support(),
                        )
                    });
                let mono_line = self.mono_line;
                let line = self.line.map(|line| {
                    let said = div()
                        .id(ElementId::Name(format!("{}-line", self.id).into()))
                        .aria_label(line.clone())
                        .text_size(px(12.5))
                        .when(mono_line, |d| d.mono(12.))
                        .text_color(p.mute)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(line.clone());
                    tipped_when_cut(said, line, window, cx)
                });
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(title)
                    .children(line)
                    .children(self.below)
            })
            .when_some(self.meta, |d, text| {
                d.child(
                    div()
                        .flex_none()
                        .mono(11.)
                        .text_color(p.faint)
                        .whitespace_nowrap()
                        .child(text),
                )
            })
            .when_some(self.trailing, |d, trailing| {
                d.child(div().flex_none().child(trailing))
            })
            .children(actions);
        if let Some(on_click) = self.on_click {
            row = row.cursor_pointer().on_click(on_click);
        }
        if let Some(decorate) = self.decorate {
            row = decorate(row);
        }
        match self.on_enter {
            Some(on_enter) => wire_with(row, state, false, move |hovered, cx| {
                if hovered {
                    on_enter(cx);
                }
            }),
            None => wire(row, state, false),
        }
        .test_support()
    }
}

/// A list of configured items in one card (repositories, skills, commands, tasks, setup steps):
/// each a [`ListRow`] with its "⋯", a line between two rows. An empty list is an
/// [`EmptyLine`](crate::EmptyLine), not an empty card.
#[derive(IntoElement)]
pub struct ListCard {
    id: ElementId,
    rows: Vec<(ListRow, Option<(ElementId, Placement)>)>,
    scrolling: Option<(ScrollHandle, Pixels)>,
}

impl ListCard {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            rows: Vec::new(),
            scrolling: None,
        }
    }

    /// A list longer than `max_height` scrolls inside its card, which keeps its frame; `scroll`
    /// is the screen's, so that it keeps the place across renders and can take it back to the top.
    pub fn scrolling(mut self, scroll: &ScrollHandle, max_height: Pixels) -> Self {
        self.scrolling = Some((scroll.clone(), max_height));
        self
    }

    pub fn row(mut self, row: ListRow) -> Self {
        self.rows.push((row.in_card(), None));
        self
    }

    pub fn rows(mut self, rows: impl IntoIterator<Item = ListRow>) -> Self {
        self.rows
            .extend(rows.into_iter().map(|row| (row.in_card(), None)));
        self
    }

    /// A row of a [`Listing`](crate::Listing): it unfolds when it arrives, folds away when it
    /// leaves and slides when it moves, the line above it going with it. `id` is its [`Item`]'s.
    pub fn item(mut self, id: impl Into<ElementId>, placement: Placement, row: ListRow) -> Self {
        self.rows
            .push((row.in_card(), Some((id.into(), placement))));
        self
    }
}

impl RenderOnce for ListCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let mut children: Vec<AnyElement> = Vec::new();
        let last = self.rows.len().saturating_sub(1);
        for (index, (mut row, item)) in self.rows.into_iter().enumerate() {
            row.corners = (index == 0, index == last);
            let line = (index > 0).then(|| div().flex_none().h(px(1.)).bg(p.line));
            let content = div().flex().flex_col().children(line).child(row);
            children.push(match item {
                Some((id, placement)) => Item::new(id, placement, content).into_any_element(),
                None => content.into_any_element(),
            });
        }
        let frame = div()
            .id(self.id.clone())
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .rounded(radius::CARD)
            .overflow_hidden();
        match self.scrolling {
            Some((scroll, max_height)) => frame.child(
                div()
                    .id(ElementId::NamedChild(self.id.into(), "scroll".into()))
                    .flex()
                    .flex_col()
                    .max_h(max_height)
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .children(children),
            ),
            None => frame.flex().flex_col().children(children),
        }
        .test_support()
    }
}
