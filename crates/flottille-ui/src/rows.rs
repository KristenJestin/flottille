//! Rows: [`TreeRow`] for the left tree (project › place › session) and [`ListRow`] for a session
//! in a card, a sub-agent, a running agent.
//!
//! A row's state mark is a [`StateIcon`](crate::StateIcon) or [`Pill`](crate::Pill) view handed in
//! by the screen, so a state change or a spinner tick redraws that mark and not the row.

use std::f32::consts::FRAC_PI_2;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, TestSupportExt,
    Transformation, Window, div, px, radians,
};

use crate::icons::{Icon, icon};
use crate::interact::{feel, wire};
use crate::motion::{Motion, PANEL};
use crate::theme::{palette, radius};
use crate::typography::Type;

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

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
    on_click: Option<OnClick>,
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
            on_click: None,
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
}

impl RenderOnce for TreeRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let (state, now) = feel(&self.id, window, cx);
        let project = self.level == TreeLevel::Project;
        let ink = if project || self.selected {
            p.text
        } else {
            crate::color::mix(p.mute, p.text, now.hover)
        };
        let background = if self.selected {
            p.sel
        } else {
            p.hover.opacity(now.hover)
        };
        let chevron_turn = self.expanded.map(|open| {
            let angle = window.use_keyed_state(
                ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "chevron".into()),
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
        });

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
            row = row.child(
                icon(Icon::Chevron)
                    .size(px(12.))
                    .text_color(p.faint)
                    .with_transformation(Transformation::rotate(radians(-FRAC_PI_2 * turn))),
            );
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
            });
        if let Some(on_click) = self.on_click {
            row = row.on_click(on_click);
        }
        wire(row, state, false).test_support()
    }
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
    detail: Option<SharedString>,
    line: Option<SharedString>,
    meta: Option<SharedString>,
    trailing: Option<AnyElement>,
    selected: bool,
    on_click: Option<OnClick>,
}

impl ListRow {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            leading: None,
            title: title.into(),
            detail: None,
            line: None,
            meta: None,
            trailing: None,
            selected: false,
            on_click: None,
        }
    }

    /// The state icon (or another 16 px mark) on the left.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    /// Mono, beside the title: a model.
    pub fn detail(mut self, text: impl Into<SharedString>) -> Self {
        self.detail = Some(text.into());
        self
    }

    /// A sentence under the title: what a procedure or a template does. It is the element
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
        let background = if self.selected {
            p.card2
        } else {
            p.hover.opacity(now.hover)
        };
        let mut row = div()
            .id(self.id.clone())
            .aria_label(self.title.clone())
            .w_full()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(radius::LIST_ROW)
            .bg(background)
            .text_color(p.text)
            .ui_text()
            .child(
                div()
                    .w(px(16.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .children(self.leading),
            )
            .child({
                let title = div()
                    .flex()
                    .items_baseline()
                    .gap(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(self.title.clone()),
                    )
                    .when_some(self.detail, |d, detail| {
                        d.child(div().mono(11.5).text_color(p.faint).child(detail))
                    });
                let line = self.line.map(|line| {
                    div()
                        .id(ElementId::Name(format!("{}-line", self.id).into()))
                        .aria_label(line.clone())
                        .text_size(px(12.5))
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
            });
        if let Some(on_click) = self.on_click {
            row = row.on_click(on_click);
        }
        wire(row, state, false).test_support()
    }
}
