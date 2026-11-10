//! The bottom of Work: `3 ended runs`, folded, the last task runs that ended in the project, each
//! with its place and how it ended. A click on one opens its drawer.

use flottille_ui::{
    FlatRow, Fold, Item, Listing, Placement, icon, palette, presence, space, turning,
};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity,
    Window, div, px,
};

use crate::runs::{CommandRow, ended_row, mark_of};
use crate::tasks::ProjectTasks;

/// The rows behind the fold.
pub struct EndedList {
    tasks: WeakEntity<ProjectTasks>,
    runs: Vec<CommandRow>,
    rows: Listing<CommandRow>,
}

impl EndedList {
    fn new(tasks: WeakEntity<ProjectTasks>) -> Self {
        Self {
            tasks,
            runs: Vec::new(),
            rows: Listing::default(),
        }
    }
}

impl Render for EndedList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let runs = self.runs.clone();
        self.rows
            .sync(runs.into_iter().map(|r| (r.run.clone(), r)), at, reduce);
        let drawn: Vec<(CommandRow, Placement)> = self
            .rows
            .rows()
            .map(|row| (row.value.clone(), row.placement))
            .collect();
        div()
            .id("project-ended-list")
            .flex()
            .flex_col()
            .children(drawn.into_iter().map(|(row, placement)| {
                let (mark, ink, said) = mark_of(row.ending(), &p);
                let place = format!("in {}", row.workspace.as_deref().unwrap_or("main"));
                let task = row.name.clone().unwrap_or_else(|| row.line.clone());
                let (tasks, open) = (self.tasks.clone(), row.run.clone());
                let meta = div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("ended-run-{}-place", row.run).into(),
                            ))
                            .aria_label(SharedString::from(place.clone()))
                            .child(SharedString::from(place))
                            .test_support(),
                    )
                    .child(SharedString::from(said));
                let line = FlatRow::new(
                    ElementId::Name(format!("ended-run-{}", row.run).into()),
                    SharedString::from(task),
                )
                .leading(icon(mark).size(px(12.)).text_color(ink))
                .meta(meta)
                .on_click(move |_, _, cx| {
                    tasks
                        .update(cx, |tasks, cx| tasks.open_command(&open, cx))
                        .ok();
                });
                Item::new(
                    ElementId::Name(format!("ended-run-item-{}", row.run).into()),
                    placement,
                    line,
                )
            }))
            .test_support()
    }
}

/// `N ended runs` and its fold.
pub struct EndedRuns {
    list: Entity<EndedList>,
    fold: Entity<Fold>,
    count: usize,
}

impl EndedRuns {
    pub(super) fn new(tasks: WeakEntity<ProjectTasks>, cx: &mut Context<Self>) -> Self {
        let list = cx.new(|_| EndedList::new(tasks));
        let fold = cx.new(|_| Fold::new("project-ended-fold", list.clone(), false));
        cx.observe(&fold, |_, _, cx| cx.notify()).detach();
        Self {
            list,
            fold,
            count: 0,
        }
    }

    /// The runs that ended, the latest first.
    pub(super) fn set(&mut self, runs: Vec<CommandRow>, cx: &mut Context<Self>) {
        let same = {
            let list = self.list.read(cx);
            list.runs.len() == runs.len()
                && list
                    .runs
                    .iter()
                    .zip(&runs)
                    .all(|(a, b)| a.run == b.run && a.state == b.state)
        };
        if same {
            return;
        }
        self.count = runs.len();
        self.list.update(cx, |list, cx| {
            list.runs = runs;
            cx.notify();
        });
        cx.notify();
    }

    pub fn is_open(&self, cx: &gpui_kit::App) -> bool {
        self.fold.read(cx).is_open()
    }
}

impl Render for EndedRuns {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let open = self.fold.read(cx).is_open();
        let turn = turning("project-ended-runs-chevron", open, window, cx);
        let said = match self.count {
            1 => "1 ended run".to_owned(),
            n => format!("{n} ended runs"),
        };
        let fold = self.fold.clone();
        let row = ended_row(
            ElementId::Name("project-ended-runs".into()),
            SharedString::from(said.clone()),
            div().child(SharedString::from(said)),
            open,
            turn,
            p,
            move |_, _, cx| fold.update(cx, |fold, cx| fold.toggle(cx)),
        );
        presence(
            "project-ended-presence",
            self.count > 0,
            div()
                .flex()
                .flex_col()
                .text_color(p.mute)
                .child(row)
                .child(self.fold.clone()),
        )
    }
}
