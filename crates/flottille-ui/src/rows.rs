//! Rows: [`TreeRow`] for the left tree (project › place › session) and [`ListRow`] for a session
//! in a card, a sub-agent, a running agent.
//!
//! A row's state mark is a [`StateIcon`](crate::StateIcon) or [`Pill`](crate::Pill) view handed in
//! by the screen, so a state change or a spinner tick redraws that mark and not the row.

use std::f32::consts::FRAC_PI_2;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, RenderOnce, SharedString, Stateful, StatefulInteractiveElement,
    Styled, TestSupportExt, Transformation, Window, div, px, radians,
};

use crate::color::mix;
use crate::icons::{Icon, icon};
use crate::interact::{choose, feel, wire};
use crate::listing::{Item, Placement};
use crate::menu::Menu;
use crate::motion::{Motion, PANEL};
use crate::theme::{palette, radius};
use crate::typography::Type;

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type Decorate = Box<dyn FnOnce(Stateful<Div>) -> Stateful<Div> + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeLevel {
    /// 32 px, weight 500, a chevron.
    Project,
    /// The project's own folder, `main`: a folder icon.
    Main,
    /// A workspace: a git-branch icon.
    Workspace,
    Session,
}

#[derive(IntoElement)]
pub struct TreeRow {
    id: ElementId,
    level: TreeLevel,
    title: SharedString,
    expanded: Option<bool>,
    selected: bool,
    trailing: Option<AnyElement>,
    actions: Option<(AnyElement, bool)>,
    on_click: Option<OnClick>,
    on_toggle: Option<OnClick>,
}

/// The element id of a project row's chevron, which folds and unfolds the level below.
pub fn toggle_id(row: impl Into<ElementId>) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(row.into()), "toggle".into())
}

impl TreeRow {
    pub fn new(id: impl Into<ElementId>, level: TreeLevel, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            level,
            title: title.into(),
            expanded: None,
            selected: false,
            trailing: None,
            actions: None,
            on_click: None,
            on_toggle: None,
        }
    }

    /// Whether the level below is open; a project row turns its chevron with it.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// The open session.
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
        let project = self.level == TreeLevel::Project;
        let chosen = choose(&state, self.selected, window, cx);
        let ink = if project {
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
            .gap(px(if project { 8. } else { 7. }))
            .h(px(if project { 32. } else { 28. }))
            .px(px(8.))
            .rounded(radius::ROW)
            .bg(background)
            .text_color(ink)
            .ui_text()
            .text_size(px(if project { 13.5 } else { 13. }))
            .when(project, |d| d.font_weight(FontWeight::MEDIUM))
            .when(self.expanded.is_some(), |d| {
                d.aria_expanded(self.expanded == Some(true))
            });
        if let Some(turn) = chevron_turn.filter(|_| project) {
            let chevron = icon(Icon::Chevron)
                .size(px(12.))
                .text_color(p.faint)
                .with_transformation(Transformation::rotate(radians(-FRAC_PI_2 * turn)));
            row = row.child(match self.on_toggle {
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
            });
        }
        match self.level {
            TreeLevel::Main => row = row.child(icon(Icon::Folder).text_color(p.faint)),
            TreeLevel::Workspace => row = row.child(icon(Icon::Branch).text_color(p.faint)),
            TreeLevel::Project | TreeLevel::Session => {}
        }
        row = row
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.title.clone()),
            )
            .when_some(self.trailing, |d, trailing| {
                d.child(div().flex_none().flex().items_center().child(trailing))
            })
            .when_some(self.actions, |d, (actions, keep)| {
                let t = if keep { 1.0 } else { now.hover };
                d.when(t > 0.0, |d| {
                    d.child(
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
                            .child(actions),
                    )
                })
            });
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

/// Mono, faint metadata: a time ago, a model, a count.
pub fn meta(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .mono(11.)
        .text_color(p.faint)
        .whitespace_nowrap()
        .child(text.into())
}

/// A section label: mono 11 px, uppercase, faint (`PROJECTS`, `AGENTS`).
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
            on_click: None,
        }
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
        let background = mix(
            p.hover.opacity(now.hover),
            p.card2,
            choose(&state, self.selected, window, cx),
        );
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
        let tags = self.tags.into_iter().map(|tag| {
            div()
                .flex_none()
                .mono(11.)
                .text_color(p.faint)
                .px(px(6.))
                .py(px(1.))
                .rounded(px(5.))
                .border_1()
                .border_color(p.line)
                .whitespace_nowrap()
                .child(tag)
        });
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
                let name = div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_ellipsis()
                    .overflow_hidden()
                    .when(mono, |d| d.mono(13.).font_weight(FontWeight::MEDIUM))
                    .child(self.title.clone());
                let title = div()
                    .flex()
                    .items_baseline()
                    .gap(px(if self.in_card { 8. } else { 6. }))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(name)
                    .children(tags)
                    .when_some(self.detail, |d, detail| {
                        d.child(div().mono(11.5).text_color(p.faint).child(detail))
                    });
                let mono_line = self.mono_line;
                let line = self.line.map(|line| {
                    div()
                        .id(ElementId::Name(format!("{}-line", self.id).into()))
                        .aria_label(line.clone())
                        .text_size(px(12.5))
                        .when(mono_line, |d| d.mono(12.))
                        .text_color(p.mute)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(line)
                        .test_support()
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
        wire(row, state, false).test_support()
    }
}

/// A list of configured items in one card (repositories, skills, commands, tasks, setup steps):
/// each a [`ListRow`] with its "⋯", a line between two rows. An empty list is an
/// [`EmptyLine`](crate::EmptyLine), not an empty card.
#[derive(IntoElement)]
pub struct ListCard {
    id: ElementId,
    rows: Vec<(ListRow, Option<(ElementId, Placement)>)>,
}

impl ListCard {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            rows: Vec::new(),
        }
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
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .rounded(radius::CARD)
            .children(children)
            .test_support()
    }
}
