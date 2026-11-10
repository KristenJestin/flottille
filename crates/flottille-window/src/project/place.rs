//! A place of the project page's Work: its row (its name, whole, and the chips of what describes
//! it; under the pointer ▶ Run a task here, + New session here and its "⋯"), a workspace's faint
//! line of where it starts from and where each repository stands, its setup while it is
//! prepared, the sessions that work there, each once, and the task runs going on there. Each
//! place is its own view: a session changing redraws its block, not the page.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::glyphs;
use crate::state::{AgentState, StateIcon, Status};
use crate::steps::{StepRow, StepTone};

use flottille_core::workspace::StepState;
use gpui_design_system::color::mix;
use gpui_design_system::motion::PANEL;
use gpui_design_system::{
    Button, Chip, Confirm, Field, FlatRow, Icon, Item, Listing, Menu, Modal, Placement, Skeleton,
    Toast, Type, awaiting, caption, enter, fade, hoverable, icon, palette, presence, presence_of,
    space, toast,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, ElementId, Entity, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, WeakEntity,
    Window, canvas, div, px,
};

use super::{PlaceData, ProjectPage, SessionLine, input_frame, removal_refused, repository_label};
use crate::actions::{Changes, Command, RepositoryStatus};
use crate::composer::Choice;
use crate::model::{Preparation, SetupLine, ago};
use crate::project_settings::Reveal;
use crate::runs::{CommandRow, address_link, ready_mark};
use crate::session::{WorkspaceActions, removal_toast, seconds, workspace_menu};
use crate::session_link::{PullRequest, TicketFacts};
use crate::shell::{Selection, Shell, now_ms};
use crate::tasks::{ProjectTasks, TaskInfo};
use crate::tree::place_name;

/// How long a place a "leads" link scrolled to stays highlighted.
const HIGHLIGHT: Duration = Duration::from_millis(1200);

/// The element id of a place's block: `place-main` or `place-<workspace>`.
pub fn block_id(name: &str) -> ElementId {
    ElementId::Name(format!("place-{name}").into())
}

/// The element id of a session's row in its place.
pub fn session_row_id(id: &str) -> ElementId {
    ElementId::Name(format!("place-session-{id}").into())
}

/// The element id of the "⋯" menu a session's row offers.
pub fn session_menu_id(id: &str) -> ElementId {
    ElementId::Name(format!("place-session-menu-{id}").into())
}

/// The element id of End session in that menu.
pub fn session_end_id(id: &str) -> ElementId {
    ElementId::Name(format!("place-session-end-{id}").into())
}

/// What the block's own action is doing, while the daemon answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Removing,
    Preparing,
}

/// What the daemon says of a workspace beyond the shell's read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Facts {
    /// Lines added and removed.
    pub changes: Option<Changes>,
    pub pulls: Vec<PullRequest>,
    /// Where each of its repositories stands.
    pub statuses: Vec<RepositoryStatus>,
    pub ticket: Option<TicketFacts>,
    /// Its forge has not answered yet.
    pub pulls_pending: bool,
    /// Its ticket is linked and its tracker has not answered yet.
    pub ticket_pending: bool,
}

/// The state a step's mark says.
fn step_state(state: StepState) -> AgentState {
    match state {
        StepState::Done | StepState::Skipped => AgentState::Completed,
        StepState::Running => AgentState::Running,
        StepState::Pending => AgentState::Idle,
        StepState::Failed => AgentState::Failed,
    }
}

fn step_word(state: StepState) -> &'static str {
    match state {
        StepState::Done => "done",
        StepState::Skipped => "skipped",
        StepState::Running => "running",
        StepState::Pending => "to do",
        StepState::Failed => "failed",
    }
}

/// How long a step took, or has been running.
fn step_time(step: &SetupLine, now: u64) -> Option<String> {
    match (step.state, step.started_at, step.ended_at) {
        (StepState::Skipped, ..) => Some("skipped".into()),
        (StepState::Running, Some(started), _) => Some(seconds(now.saturating_sub(started))),
        (StepState::Done | StepState::Failed, Some(started), Some(ended)) => {
            Some(seconds(ended.saturating_sub(started)))
        }
        _ => None,
    }
}

/// `1 commit ahead`, `5 files changed`, `clean`: where a repository stands, in words.
fn standing(status: &RepositoryStatus) -> String {
    let mut parts = Vec::new();
    match status.ahead {
        Some(0) | None => {}
        Some(1) => parts.push("1 commit ahead".to_owned()),
        Some(n) => parts.push(format!("{n} commits ahead")),
    }
    match status.uncommitted {
        Some(0) | None => {}
        Some(1) => parts.push("1 file changed".to_owned()),
        Some(n) => parts.push(format!("{n} files changed")),
    }
    if parts.is_empty() && (status.ahead.is_some() || status.uncommitted.is_some()) {
        parts.push("clean".to_owned());
    }
    // Why it started from its local branch, when it did.
    parts.extend(status.note.clone());
    parts.join(", ")
}

/// A place of Work.
pub struct PlaceBlock {
    shell: WeakEntity<Shell>,
    page: WeakEntity<ProjectPage>,
    project: String,
    /// The page's tasks and runs, and the drawer a run opens in.
    tasks: WeakEntity<ProjectTasks>,
    data: PlaceData,
    pub(super) facts: Facts,
    /// The project has repositories other than its folder: a status names its repository.
    several: bool,
    /// The project's tasks, which ▶ offers.
    task_list: Vec<TaskInfo>,
    /// The task runs going on here, the user's and its agents'.
    runs: Vec<CommandRow>,
    /// The removal's confirmation is open.
    confirming: bool,
    busy: Option<Busy>,
    icons: HashMap<String, Entity<StateIcon>>,
    /// Each step's mark, by its position: it turns to the step's new state when it changes.
    marks: HashMap<u32, (Entity<Status>, Entity<StateIcon>)>,
    /// Each running run's spinner.
    spinners: HashMap<String, Entity<StateIcon>>,
    /// The steps, the sessions and the runs as last drawn: a row that comes unfolds, one that
    /// goes folds away. The order never changes but by what the daemon says.
    step_rows: Listing<SetupLine>,
    session_rows: Listing<SessionLine>,
    run_rows: Listing<CommandRow>,
    /// Each session row's "⋯" menu, made when the row is first drawn.
    session_menus: HashMap<String, (Entity<Menu>, Subscription)>,
    /// Its "⋯" and ▶, made again when what they offer changes.
    menu: Entity<Menu>,
    run_menu: Option<Entity<Menu>>,
    _menus: Vec<Subscription>,
    _run_menu: Option<Subscription>,
    /// A ticket is linked (or being read), as the menu says.
    linked: bool,
    /// "Link a ticket" is open, its field made the first time.
    linking: bool,
    ticket_field: Option<Entity<InputState>>,
    ticket_error: Option<SharedString>,
    /// A "leads" link scrolled to it: it is drawn in the accent for a moment.
    highlighted: bool,
    _highlight: Option<Task<()>>,
    /// Where it was last drawn, which the page scrolls to.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _subscriptions: Vec<Subscription>,
    shown_times: Vec<String>,
    renders: usize,
}

impl PlaceBlock {
    pub(super) fn new(
        shell: WeakEntity<Shell>,
        page: WeakEntity<ProjectPage>,
        project: String,
        tasks: WeakEntity<ProjectTasks>,
        data: PlaceData,
        cx: &mut Context<Self>,
    ) -> Self {
        let this = cx.entity().downgrade();
        let name = place_name(&data.place).to_owned();
        let removable = data.place.is_some();
        let menu = cx.new(|cx| Self::workspace_menu(&this, &name, removable, false, cx));
        let watch = cx.observe(&menu, |_, _, cx| cx.notify());
        let mut block = Self {
            shell,
            page,
            project,
            tasks,
            data: PlaceData {
                sessions: Vec::new(),
                ..data.clone()
            },
            facts: Facts::default(),
            several: false,
            task_list: Vec::new(),
            runs: Vec::new(),
            confirming: false,
            busy: None,
            icons: HashMap::new(),
            marks: HashMap::new(),
            spinners: HashMap::new(),
            step_rows: Listing::default(),
            session_rows: Listing::default(),
            run_rows: Listing::default(),
            session_menus: HashMap::new(),
            menu,
            run_menu: None,
            _menus: vec![watch],
            _run_menu: None,
            linked: false,
            linking: false,
            ticket_field: None,
            ticket_error: None,
            highlighted: false,
            _highlight: None,
            bounds: Rc::new(Cell::new(None)),
            _subscriptions: Vec::new(),
            shown_times: Vec::new(),
            renders: 0,
        };
        block.set(data, cx);
        block
    }

    pub fn data(&self) -> &PlaceData {
        &self.data
    }

    pub fn changes(&self) -> Option<Changes> {
        self.facts.changes
    }

    pub fn pulls(&self) -> &[PullRequest] {
        &self.facts.pulls
    }

    pub fn statuses(&self) -> &[RepositoryStatus] {
        &self.facts.statuses
    }

    pub fn ticket(&self) -> Option<&TicketFacts> {
        self.facts.ticket.as_ref()
    }

    /// The task runs going on here.
    pub fn runs(&self) -> &[CommandRow] {
        &self.runs
    }

    /// Its "⋯" menu, opened.
    pub fn open_menu(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu.update(cx, |m, cx| m.open(window, cx));
    }

    /// The removal's confirmation, opened as "Remove workspace…" does.
    pub fn ask_removal(&mut self, cx: &mut Context<Self>) {
        self.confirming = true;
        cx.notify();
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    /// Where it was last drawn.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds.get()
    }

    /// Drawn in the accent for a moment, which fades: a link scrolled to it.
    pub fn highlight(&mut self, cx: &mut Context<Self>) {
        self.highlighted = true;
        cx.notify();
        self._highlight = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HIGHLIGHT).await;
            this.update(cx, |this, cx| {
                this.highlighted = false;
                cx.notify();
            })
            .ok();
        }));
    }

    /// The place's "⋯": Open in editor, and for a workspace Link a ticket… (Change the ticket…
    /// once one is linked), Prepare again and Remove workspace….
    fn workspace_menu(
        this: &WeakEntity<Self>,
        name: &str,
        workspace: bool,
        linked: bool,
        cx: &mut Context<Menu>,
    ) -> Menu {
        let (opening, linking, preparing, removing) =
            (this.clone(), this.clone(), this.clone(), this.clone());
        workspace_menu(
            &format!("workspace-menu-{name}"),
            WorkspaceActions {
                open_in_editor: Box::new(move |cx| {
                    opening
                        .update(cx, |block, cx| block.open_in_editor(cx))
                        .ok();
                }),
                ticket_linked: linked,
                link_ticket: workspace.then(|| {
                    Box::new(move |cx: &mut gpui_kit::App| {
                        linking
                            .update(cx, |block, cx| {
                                block.linking = true;
                                block.ticket_error = None;
                                cx.notify();
                            })
                            .ok();
                    }) as Box<dyn Fn(&mut gpui_kit::App)>
                }),
                unlink_ticket: None,
                prepare: workspace.then(|| {
                    Box::new(move |cx: &mut gpui_kit::App| {
                        preparing
                            .update(cx, |block, cx| block.perform(Busy::Preparing, cx))
                            .ok();
                    }) as Box<dyn Fn(&mut gpui_kit::App)>
                }),
                remove: workspace.then(|| {
                    Box::new(move |cx: &mut gpui_kit::App| {
                        removing
                            .update(cx, |block, cx| {
                                block.confirming = true;
                                cx.notify();
                            })
                            .ok();
                    }) as Box<dyn Fn(&mut gpui_kit::App)>
                }),
            },
            cx,
        )
    }

    /// The menu says what applies to the ticket: it is made again when one is linked.
    fn sync_menu(&mut self, cx: &mut Context<Self>) {
        let linked = self.facts.ticket.is_some() || self.facts.ticket_pending;
        let Some(name) = self.data.place.clone().filter(|_| self.linked != linked) else {
            return;
        };
        self.linked = linked;
        let this = cx.entity().downgrade();
        self.menu = cx.new(|cx| Self::workspace_menu(&this, &name, true, linked, cx));
        let watch = cx.observe(&self.menu, |_, _, cx| cx.notify());
        self._menus = vec![watch];
    }

    /// What the daemon says of the workspace beyond the shell's read.
    pub(super) fn set_facts(&mut self, facts: Facts, several: bool, cx: &mut Context<Self>) {
        if self.facts == facts && self.several == several {
            return;
        }
        self.facts = facts;
        self.several = several;
        self.sync_menu(cx);
        cx.notify();
    }

    /// The project's tasks, which ▶ offers: its list is made again.
    pub(super) fn set_tasks(&mut self, tasks: &[TaskInfo], cx: &mut Context<Self>) {
        if self.task_list != tasks {
            self.task_list = tasks.to_vec();
            self.run_menu = None;
            cx.notify();
        }
    }

    /// The task runs going on here.
    pub(super) fn set_runs(&mut self, runs: Vec<CommandRow>, cx: &mut Context<Self>) {
        if self.runs != runs {
            self.runs = runs;
            cx.notify();
        }
    }

    /// ▶ Run a task here: the project's tasks, each with its tag; a pick runs it in this place.
    /// With no task, New task opens the project's settings at its tasks.
    fn run_menu(&mut self, cx: &mut Context<Self>) -> Entity<Menu> {
        if let Some(menu) = &self.run_menu {
            return menu.clone();
        }
        let name = place_name(&self.data.place).to_owned();
        let tasks = self.tasks.clone();
        let workspace = self.data.place.clone();
        let list = self.task_list.clone();
        let (shell, project) = (self.shell.clone(), self.project.clone());
        let menu = cx.new(|cx| {
            let menu = Menu::new(
                ElementId::Name(format!("place-run-{name}").into()),
                "Run a task here",
                cx,
            )
            .glyph(Icon::Play)
            .width(px(240.));
            if list.is_empty() {
                return menu.entry(
                    ElementId::Name(format!("place-run-{name}-new").into()),
                    "New task in Settings",
                    Some(Icon::Plus),
                    move |window, cx| {
                        let project = project.clone();
                        shell
                            .update(cx, |shell, cx| {
                                shell.open_project_settings_at(&project, Reveal::Tasks, window, cx)
                            })
                            .ok();
                    },
                );
            }
            list.into_iter().fold(menu, |menu, info| {
                let (tasks, workspace, task) =
                    (tasks.clone(), workspace.clone(), info.name.clone());
                let tag = match (info.serve, info.check) {
                    (true, _) => Some("server"),
                    (_, true) => Some("check"),
                    _ => None,
                };
                let menu = menu.entry(
                    ElementId::Name(format!("place-run-{name}-{}", info.name).into()),
                    info.name.clone(),
                    None,
                    move |_, cx| {
                        tasks
                            .update(cx, |tasks, cx| tasks.run(&task, workspace.clone(), cx))
                            .ok();
                    },
                );
                match tag {
                    Some(tag) => menu.detail(tag),
                    None => menu,
                }
            })
        });
        self._run_menu = Some(cx.observe(&menu, |_, _, cx| cx.notify()));
        self.run_menu = Some(menu.clone());
        menu
    }

    pub(super) fn set(&mut self, data: PlaceData, cx: &mut Context<Self>) {
        for session in &data.sessions {
            let status = session.status.clone();
            self.icons
                .entry(session.id.clone())
                .or_insert_with(|| cx.new(|cx| StateIcon::new(status, cx)));
        }
        for step in &data.steps {
            let (at, state) = (step.position, step_state(step.state));
            match self.marks.get(&at) {
                Some((status, _)) => {
                    if status.read(cx).state() != state {
                        status.update(cx, |status, cx| status.set(state, cx));
                    }
                }
                None => {
                    let status = cx.new(|_| Status::new(state));
                    let icon = {
                        let status = status.clone();
                        cx.new(|cx| StateIcon::new(status, cx))
                    };
                    self.marks.insert(at, (status, icon));
                }
            }
        }
        // A preparation started again is under way.
        if self.data.preparation != data.preparation && self.busy == Some(Busy::Preparing) {
            self.busy = None;
        }
        self.data = data;
        cx.notify();
    }

    /// The "⋯" menu of a session's row: End session, which asks first (the confirmation of the
    /// sidebar and of the session page, counting the agents that end). A session cannot be
    /// removed: the daemon has no such operation, ending is what there is.
    fn session_menu(&mut self, session: &str, cx: &mut Context<Self>) {
        if self.session_menus.contains_key(session) {
            return;
        }
        let (shell, id) = (self.shell.clone(), session.to_owned());
        let menu = cx.new(|cx| {
            Menu::new(session_menu_id(&id), "Session actions", cx).entry(
                session_end_id(&id),
                "End session…",
                Some(glyphs::STOP),
                {
                    let id = id.clone();
                    move |_, cx| {
                        shell.update(cx, |shell, cx| shell.ask_end_of(&id, cx)).ok();
                    }
                },
            )
        });
        let watch = cx.observe(&menu, |_, _, cx| cx.notify());
        self.session_menus.insert(session.to_owned(), (menu, watch));
    }

    /// What the daemon answered to this place's removal or new preparation: a toast says it.
    pub fn answered(&mut self, refused: Option<&str>, cx: &mut Context<Self>) {
        let name = place_name(&self.data.place).to_owned();
        let said = match (self.busy.take(), refused) {
            (Some(Busy::Removing), refused) => removal_toast(&name, refused),
            (_, None) => Toast::success(format!("Preparing {name} again"))
                .detail("Its place on the project page lists the steps."),
            (_, Some(why)) => {
                Toast::failure(format!("{name} was not prepared again")).detail(why.to_owned())
            }
        };
        toast(said, cx);
        self.confirming = false;
        cx.notify();
    }

    /// What the daemon answered to "Link a ticket".
    pub fn ticket_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        if !self.linking {
            return;
        }
        match refused {
            None => {
                let typed = self
                    .ticket_field
                    .as_ref()
                    .map(|field| field.read(cx).value().trim().to_owned())
                    .unwrap_or_default();
                let name = place_name(&self.data.place).to_owned();
                toast(
                    Toast::success(format!("{name} linked to {typed}"))
                        .detail("Its place on the project page shows the ticket."),
                    cx,
                );
                self.linking = false;
                self.ticket_error = None;
            }
            Some(why) => self.ticket_error = Some(why.into()),
        }
        cx.notify();
    }

    fn open_in_editor(&self, cx: &mut Context<Self>) {
        let folder = self.data.folder.clone();
        self.shell
            .update(cx, |shell, _| shell.open_link(&folder))
            .ok();
    }

    fn link_ticket(&mut self, cx: &mut Context<Self>) {
        let (Some(name), Some(field)) = (self.data.place.clone(), self.ticket_field.clone()) else {
            return;
        };
        let typed = field.read(cx).value().trim().to_owned();
        if typed.is_empty() {
            self.ticket_error = Some("Type a ticket: #123 or ABC-123.".into());
            cx.notify();
            return;
        }
        let command = Command::LinkTicket {
            project: self.project.clone(),
            name,
            ticket: Some(typed),
        };
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
    }

    /// Whether a text that moves with the time changed since it was drawn.
    pub(super) fn times_changed(&self, now: u64) -> bool {
        self.times(now) != self.shown_times
    }

    /// The texts that move with the time: the sessions' times ago, the running step's and how
    /// long each run has run.
    fn times(&self, now: u64) -> Vec<String> {
        self.data
            .sessions
            .iter()
            .filter(|s| s.mark.is_none())
            .map(|s| ago(now, s.active_at))
            .chain(
                self.data
                    .steps
                    .iter()
                    .filter(|step| step.state == StepState::Running)
                    .filter_map(|step| step_time(step, now)),
            )
            .chain(self.runs.iter().map(|run| seconds(run.elapsed_ms(now))))
            .collect()
    }

    fn perform(&mut self, busy: Busy, cx: &mut Context<Self>) {
        let Some(name) = self.data.place.clone() else {
            return;
        };
        let project = self.project.clone();
        let command = match busy {
            Busy::Removing => Command::RemoveWorkspace { project, name },
            Busy::Preparing => Command::PrepareWorkspace { project, name },
        };
        self.busy = Some(busy);
        self.confirming = false;
        self.shell
            .update(cx, |shell, _| shell.perform(command))
            .ok();
        cx.notify();
    }

    fn removal_refused(&self) -> Option<SharedString> {
        let living: Vec<String> = self.data.sessions.iter().map(|s| s.title.clone()).collect();
        removal_refused(&self.project, &self.facts.statuses, &living)
    }

    /// The setup's steps while it is prepared, and what failed: the block stays built once it
    /// is ready, so that it folds away instead of vanishing.
    fn setup(&mut self, name: &str, now: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let steps = self.data.steps.clone();
        self.step_rows.sync(
            steps.into_iter().map(|s| (s.position.to_string(), s)),
            at,
            reduce,
        );
        if self.step_rows.is_empty() {
            return None;
        }
        let p = palette(cx);
        let total = self.data.steps.len();
        let failed = matches!(self.data.preparation, Preparation::Failed { .. });
        let at = self
            .data
            .steps
            .iter()
            .position(|step| !matches!(step.state, StepState::Done | StepState::Skipped))
            .unwrap_or(total.saturating_sub(1));
        let count = match failed {
            true => format!("failed at step {} of {total}", at + 1),
            false => format!("step {} of {total}", at + 1),
        };
        let rows: Vec<_> = self
            .step_rows
            .rows()
            .map(|row| (row.value.clone(), row.placement))
            .collect();
        let rows = rows.into_iter().enumerate().map(|(i, (step, placement))| {
            let tone = match step.state {
                StepState::Done | StepState::Skipped => StepTone::Done,
                StepState::Running => StepTone::Now,
                StepState::Pending => StepTone::Todo,
                StepState::Failed => StepTone::Failed,
            };
            let mark = div()
                .id(ElementId::Name(format!("setup-mark-{name}-{i}").into()))
                .aria_label(step_word(step.state))
                .children(self.marks.get(&step.position).map(|(_, icon)| icon.clone()))
                .test_support();
            let mut row = StepRow::new(
                ElementId::Name(format!("setup-step-{name}-{i}").into()),
                mark,
                SharedString::from(step.verb.clone()),
                tone,
                ElementId::Name(format!("setup-time-{name}-{i}").into()),
            );
            if let Some(object) = &step.object {
                row = row.object(SharedString::from(object.clone()));
            }
            if let Some(time) = step_time(&step, now) {
                row = row.time(SharedString::from(time));
            }
            // A command that ran or runs opens in the run drawer, and so does a step that failed
            // (the drawer says why); one still to do, or skipped, did nothing yet to show, and a
            // copy or a link that went well has no output.
            if match step.state {
                StepState::Running | StepState::Done => step.runs,
                StepState::Failed => true,
                _ => false,
            } {
                let (tasks, workspace, position) =
                    (self.tasks.clone(), name.to_owned(), step.position);
                row = row.on_click(move |_, _, cx| {
                    tasks
                        .update(cx, |tasks, cx| tasks.open_step(&workspace, position, cx))
                        .ok();
                });
            }
            Item::new(
                ElementId::Name(format!("step-item-{name}-{}", step.position).into()),
                placement,
                row,
            )
        });
        let output = match &self.data.preparation {
            Preparation::Failed { output, .. } => Some(SharedString::from(output.clone())),
            _ => None,
        };
        let failure = {
            let (name, again) = (
                name.to_owned(),
                cx.listener(|this: &mut Self, _, _, cx| this.perform(Busy::Preparing, cx)),
            );
            let busy = self.busy;
            let (field, fail_line, mute) = (p.field, p.fail_line, p.mute);
            presence_of(
                ElementId::Name(format!("failure-{name}").into()),
                output,
                move |output| {
                    div()
                        .px(space::MD)
                        .pt(space::XS)
                        .flex()
                        .flex_col()
                        .gap(space::SM)
                        .child(
                            div()
                                .id(ElementId::Name(format!("failure-output-{name}").into()))
                                .max_h(px(160.))
                                .overflow_y_scroll()
                                .py(space::SM)
                                .px(space::MD)
                                .rounded(px(8.))
                                .bg(field)
                                .border_1()
                                .border_color(fail_line)
                                .mono(11.5)
                                .text_color(mute)
                                .child(output)
                                .test_support(),
                        )
                        .child(
                            div().flex().justify_end().child(
                                Button::secondary(
                                    ElementId::Name(format!("retry-{name}").into()),
                                    if busy == Some(Busy::Preparing) {
                                        "Preparing again…"
                                    } else {
                                        "Prepare again"
                                    },
                                )
                                .disabled(busy.is_some())
                                .on_click(again),
                            ),
                        )
                        .into_any_element()
                },
            )
        };
        Some(
            div()
                .pt(space::XS)
                .pb(space::SM)
                .flex()
                .flex_col()
                .child(caption(
                    ElementId::Name(format!("setup-{name}").into()),
                    "Setup",
                    SharedString::from(count),
                    cx,
                ))
                .children(rows)
                .child(failure)
                .into_any_element(),
        )
    }

    fn link_modal(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) -> Modal {
        if self.linking && self.ticket_field.is_none() {
            let field =
                cx.new(|cx| crate::project_settings::styled_input("#123 or ABC-123", window, cx));
            self._subscriptions
                .push(cx.subscribe_in(&field, window, |this, _, event, _, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.link_ticket(cx)
                    }
                }));
            let focus = field.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            self.ticket_field = Some(field);
        }
        let this = cx.entity().downgrade();
        let mut modal = Modal::new(
            ElementId::Name(format!("link-ticket-modal-{name}").into()),
            format!("Link {name} to a ticket"),
        )
        .sentence(
            "Its place and its sessions show the ticket; agents read it with the tickets tool.",
        )
        .open(self.linking)
        .on_close(move |_, cx| {
            this.update(cx, |this, cx| {
                this.linking = false;
                cx.notify();
            })
            .ok();
        })
        .action(
            Button::primary(
                ElementId::Name(format!("link-ticket-{name}-action").into()),
                "Link",
            )
            .on_click(cx.listener(|this, _, _, cx| this.link_ticket(cx))),
        );
        if let Some(field) = &self.ticket_field {
            modal = modal.child(
                Field::new(
                    ElementId::Name(format!("link-ticket-{name}-field").into()),
                    "Ticket",
                    input_frame(
                        ElementId::Name(format!("link-ticket-{name}-input").into()),
                        field,
                        cx,
                    ),
                )
                .error(self.ticket_error.clone()),
            );
        }
        modal
    }

    /// The chips on the right of the row: its pull requests with their checks' marks, its ticket,
    /// the lines it changed, each fading in as it becomes known; `no session` for a workspace
    /// where none works.
    fn chips(&self, name: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = palette(cx);
        let mut chips = Vec::new();
        if self.data.place.is_some() {
            let pulls = (!self.facts.pulls.is_empty()).then(|| {
                div()
                    .id(ElementId::Name(format!("place-pull-{name}").into()))
                    .flex()
                    .gap(space::XS)
                    .children(self.facts.pulls.iter().map(|pull| {
                        let label = repository_label(&self.project, &pull.repository);
                        let (shell, url) = (self.shell.clone(), pull.url.clone());
                        let row = crate::session::pull_row(
                            ElementId::Name(format!("pull-{name}-{label}").into()),
                            pull,
                            cx,
                        )
                        .h(px(24.))
                        .px(space::SM)
                        .rounded(px(6.))
                        .border_1()
                        .border_color(p.line)
                        .text_color(p.mute)
                        .on_click(move |_, _, cx| {
                            shell.update(cx, |shell, _| shell.open_link(&url)).ok();
                        });
                        crate::session::pull_hover(row, cx)
                    }))
                    .test_support()
                    .into_any_element()
            });
            chips.push(
                awaiting(
                    ElementId::Name(format!("place-pull-wait-{name}").into()),
                    self.facts.pulls_pending,
                    Skeleton::new(format!("place-pull-skeleton-{name}"))
                        .w(px(56.))
                        .h(px(20.)),
                    pulls.unwrap_or_else(|| div().into_any_element()),
                )
                .into_any_element(),
            );
            let ticket = self.facts.ticket.as_ref().map(|ticket| {
                let mut chip = Chip::new(
                    ElementId::Name(format!("ticket-{name}").into()),
                    glyphs::TICKET,
                    SharedString::from(ticket.reference.clone()),
                );
                if let Some(url) = ticket.url.clone() {
                    let shell = self.shell.clone();
                    chip = chip.on_click(move |_, _, cx| {
                        shell.update(cx, |shell, _| shell.open_link(&url)).ok();
                    });
                }
                chip.into_any_element()
            });
            if self.facts.ticket_pending || ticket.is_some() {
                chips.push(
                    awaiting(
                        ElementId::Name(format!("place-ticket-wait-{name}").into()),
                        self.facts.ticket_pending,
                        Skeleton::new(format!("place-ticket-skeleton-{name}"))
                            .w(px(64.))
                            .h(px(20.)),
                        ticket.unwrap_or_else(|| div().into_any_element()),
                    )
                    .into_any_element(),
                );
            }
        }
        if let Some(Changes {
            added,
            removed,
            capped,
        }) = self.facts.changes.filter(|c| c.added > 0 || c.removed > 0)
        {
            // Capped: the daemon stopped counting new files, there are more.
            let at_least = if capped { "≥" } else { "" };
            chips.push(
                enter(
                    ElementId::Name(format!("place-changes-enter-{name}").into()),
                    0,
                    div()
                        .id(ElementId::Name(format!("changes-{name}").into()))
                        .aria_label(SharedString::from(format!("{at_least}+{added} −{removed}")))
                        .flex_none()
                        .flex()
                        .gap(space::XS)
                        .mono(11.5)
                        .child(div().text_color(p.ok).child(format!("{at_least}+{added}")))
                        .child(div().text_color(p.fail).child(format!("−{removed}")))
                        .test_support(),
                )
                .into_any_element(),
            );
        }
        if self.data.place.is_some() && self.data.sessions.is_empty() {
            chips.push(
                enter(
                    ElementId::Name(format!("place-no-session-enter-{name}").into()),
                    0,
                    div()
                        .id(ElementId::Name(format!("place-no-session-{name}").into()))
                        .aria_label("no session")
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(p.faint)
                        .child("no session")
                        .test_support(),
                )
                .into_any_element(),
            );
        }
        match &self.data.preparation {
            Preparation::Preparing { .. } => chips.push(
                div()
                    .mono(11.5)
                    .text_color(p.faint)
                    .child("preparing")
                    .into_any_element(),
            ),
            Preparation::Failed { .. } => chips.push(
                div()
                    .mono(11.5)
                    .text_color(p.fail)
                    .child("failed")
                    .into_any_element(),
            ),
            Preparation::Ready => {}
        }
        chips
    }

    /// A workspace's faint line, in labelled values: where it starts from, and where each
    /// repository stands (a project of one repository does not name it).
    fn facts_line(&self, name: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let meta = self.data.meta.clone()?;
        let p = palette(cx);
        let mut values: Vec<(Option<String>, String)> = vec![(None, meta)];
        for status in &self.facts.statuses {
            let said = standing(status);
            if said.is_empty() {
                continue;
            }
            let label = self
                .several
                .then(|| repository_label(&self.project, &status.path));
            values.push((label, said));
        }
        let text = values
            .iter()
            .map(|(label, value)| match label {
                Some(label) => format!("{label}: {value}"),
                None => value.clone(),
            })
            .collect::<Vec<_>>()
            .join(" · ");
        let count = values.len();
        Some(
            div()
                .id(ElementId::Name(format!("place-facts-{name}").into()))
                .aria_label(SharedString::from(text))
                .pl(space::XXL)
                .flex()
                .flex_wrap()
                .gap_x(space::SM)
                .text_size(px(12.))
                .text_color(p.faint)
                .children(values.into_iter().enumerate().map(|(i, (label, value))| {
                    div()
                        .flex()
                        .gap(space::XS)
                        .when_some(label, |d, label| d.child(format!("{label}:")))
                        .child(div().text_color(p.mute).child(value))
                        .when(i + 1 < count, |d| d.child("·"))
                }))
                .test_support()
                .into_any_element(),
        )
    }

    /// The lines under a session's row: whom it leads, its lead, and what it asks the user.
    fn session_lines(&self, session: &SessionLine, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let leads = (!session.leads.is_empty()).then(|| {
            let mut places: Vec<String> = Vec::new();
            for (_, place) in &session.leads {
                let name = place_name(place).to_owned();
                if !places.contains(&name) {
                    places.push(name);
                }
            }
            let said = format!("leads {}: {}", session.leads.len(), places.join(", "));
            let count = places.len();
            div()
                .id(ElementId::Name(
                    format!("place-leads-{}", session.id).into(),
                ))
                .aria_label(SharedString::from(said))
                .flex()
                .flex_wrap()
                .gap_x(space::XS)
                .child(format!("leads {}:", session.leads.len()))
                .children(places.into_iter().enumerate().map(|(i, name)| {
                    let page = self.page.clone();
                    let place = (name != "main").then(|| name.clone());
                    let text = if i + 1 < count {
                        format!("{name},")
                    } else {
                        name.clone()
                    };
                    let link = div()
                        .id(ElementId::Name(
                            format!("place-leads-{}-{name}", session.id).into(),
                        ))
                        .aria_label(SharedString::from(name.clone()))
                        .cursor_pointer()
                        .child(text)
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            page.update(cx, |page, cx| page.scroll_to(&place, cx)).ok();
                        });
                    hoverable(link, move |d, t| d.text_color(mix(p.acc, p.text, t)))
                }))
                .test_support()
        });
        let led = session.pilot.as_ref().map(|(pilot, title)| {
            let (shell, pilot) = (self.shell.clone(), pilot.clone());
            let said = format!("led by {title}");
            hoverable(
                div()
                    .id(ElementId::Name(
                        format!("place-led-by-{}", session.id).into(),
                    ))
                    .aria_label(SharedString::from(said))
                    .flex()
                    .gap(space::XS)
                    .cursor_pointer()
                    .child("↳ led by")
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(title.clone())),
                    )
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let session = pilot.clone();
                        shell
                            .update(cx, |shell, cx| {
                                shell.open(Selection::Session(session), window, cx)
                            })
                            .ok();
                    }),
                move |d, t| d.text_color(mix(p.acc, p.text, t)),
            )
        });
        let question = session.question.clone().map(SharedString::from);
        let id = session.id.clone();
        div()
            .pl(space::XL + space::LG)
            .flex()
            .flex_col()
            .text_size(px(12.))
            .text_color(p.faint)
            .children(leads)
            .children(led)
            .child(presence_of(
                ElementId::Name(format!("place-question-presence-{id}").into()),
                question,
                move |question| {
                    div()
                        .id(ElementId::Name(format!("place-question-{id}").into()))
                        .aria_label(question.clone())
                        .pb(space::XS)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(question)
                        .test_support()
                        .into_any_element()
                },
            ))
            .into_any_element()
    }

    /// The sessions working here, each once, in their `Item`s.
    fn sessions(&mut self, now: u64, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let listed = self.data.sessions.clone();
        self.session_rows
            .sync(listed.into_iter().map(|s| (s.id.clone(), s)), at, reduce);
        // A row that leaves keeps its mark and its menu until it has folded away.
        let drawn: Vec<String> = self.session_rows.rows().map(|r| r.key.to_owned()).collect();
        self.icons.retain(|id, _| drawn.contains(id));
        self.session_menus.retain(|id, _| drawn.contains(id));
        for key in &drawn {
            self.session_menu(key, cx);
        }
        let shown: Vec<(SessionLine, Placement)> = self
            .session_rows
            .rows()
            .map(|row| (row.value.clone(), row.placement))
            .collect();
        shown
            .into_iter()
            .map(|(session, placement)| {
                let shell = self.shell.clone();
                let open = session.id.clone();
                let meta = match session.mark {
                    Some(_) => String::new(),
                    None => ago(now, session.active_at),
                };
                let row = FlatRow::new(
                    session_row_id(&session.id),
                    SharedString::from(session.title.clone()),
                )
                .when_some(self.icons.get(&session.id).cloned(), |row, icon| {
                    row.leading(icon)
                })
                .meta(div().child(SharedString::from(meta)))
                .when_some(
                    self.session_menus.get(&session.id).map(|(m, _)| m.clone()),
                    |row, menu| row.more(menu),
                )
                .on_click(move |_, window, cx| {
                    let session = open.clone();
                    shell
                        .update(cx, |shell, cx| {
                            shell.open(Selection::Session(session), window, cx)
                        })
                        .ok();
                });
                let lines = self.session_lines(&session, cx);
                Item::new(
                    ElementId::Name(format!("session-item-{}", session.id).into()),
                    placement,
                    div().flex().flex_col().child(row).child(lines),
                )
                .into_any_element()
            })
            .collect()
    }

    fn spinner(&mut self, run: &str, cx: &mut Context<Self>) -> Entity<StateIcon> {
        self.spinners
            .entry(run.to_owned())
            .or_insert_with(|| {
                let status = cx.new(|_| Status::new(AgentState::Running));
                cx.new(|cx| StateIcon::new(status, cx))
            })
            .clone()
    }

    /// The task runs going on here: the task, its address, how long, and Stop.
    fn run_rows(
        &mut self,
        now: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (at, reduce) = (cx.background_executor().now(), cx.reduce_motion());
        let runs = self.runs.clone();
        self.run_rows
            .sync(runs.into_iter().map(|r| (r.run.clone(), r)), at, reduce);
        let drawn: Vec<(CommandRow, Placement)> = self
            .run_rows
            .rows()
            .map(|row| (row.value.clone(), row.placement))
            .collect();
        self.spinners
            .retain(|run, _| drawn.iter().any(|(row, _)| &row.run == run));
        let p = palette(cx);
        drawn
            .into_iter()
            .map(|(row, placement)| {
                let spinner = self.spinner(&row.run, cx);
                let mark = match row.readiness() {
                    Some(ready) => ready_mark(
                        ElementId::Name(format!("place-run-mark-{}", row.run).into()),
                        ElementId::Name(format!("place-run-dot-{}", row.run).into()),
                        ready == "ready",
                        Some(spinner.into_any_element()),
                        &p,
                        window,
                        cx,
                    ),
                    None => spinner.into_any_element(),
                };
                let task = row.name.clone().unwrap_or_else(|| row.line.clone());
                let tasks = self.tasks.clone();
                let link = row.address().map(|url| {
                    let tasks = tasks.clone();
                    address_link(
                        ElementId::Name(format!("place-run-link-{}", row.run).into()),
                        url,
                        11.5,
                        move |url, cx| {
                            if let Some(tasks) = tasks.upgrade() {
                                tasks.read(cx).open_link(url, cx);
                            }
                        },
                        window,
                        cx,
                    )
                });
                let stop = {
                    let (tasks, run) = (tasks.clone(), row.run.clone());
                    Button::icon(
                        ElementId::Name(format!("command-stop-{}", row.run).into()),
                        glyphs::STOP,
                        SharedString::from(format!("Stop {task}")),
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        tasks
                            .update(cx, |tasks, cx| tasks.stop_command(&run, cx))
                            .ok();
                    })
                };
                let open = row.run.clone();
                let line = div()
                    .id(ElementId::Name(format!("place-run-row-{}", row.run).into()))
                    .aria_label(SharedString::from(task.clone()))
                    .h(px(32.))
                    .px(space::MD)
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .cursor_pointer()
                    .child(
                        div()
                            .w(px(16.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(mark),
                    )
                    .child(
                        div()
                            .flex_none()
                            .mono(12.)
                            .text_color(p.text)
                            .child(SharedString::from(task)),
                    )
                    .child(div().flex_1().min_w_0().children(link))
                    .child(
                        div()
                            .flex_none()
                            .mono(11.)
                            .text_color(p.faint)
                            .child(seconds(row.elapsed_ms(now))),
                    )
                    .child(stop)
                    .on_click(move |_, _, cx| {
                        tasks
                            .update(cx, |tasks, cx| tasks.open_command(&open, cx))
                            .ok();
                    });
                Item::new(
                    ElementId::Name(format!("place-run-item-{}", row.run).into()),
                    placement,
                    hoverable(line, move |d, t| d.bg(p.hover.opacity(t))),
                )
                .into_any_element()
            })
            .collect()
    }
}

impl Render for PlaceBlock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let p = palette(cx);
        let now = now_ms();
        self.shown_times = self.times(now);
        let place = self.data.place.clone();
        let name = place_name(&place).to_owned();
        let ready = matches!(self.data.preparation, Preparation::Ready);
        let id = |part: &str| ElementId::Name(format!("{part}-{name}").into());

        // Under the pointer: ▶ Run a task here, + New session here, and the place's "⋯".
        let run_menu = self.run_menu(cx);
        let kept = self.menu.read(cx).is_open() || run_menu.read(cx).is_open();
        let new_session = {
            let (shell, project) = (self.shell.clone(), self.project.clone());
            let choice = match &place {
                None => Choice::Main,
                Some(name) => Choice::Workspace(name.clone()),
            };
            Button::icon(id("place-new-session"), Icon::Plus, "New session here")
                .disabled(!ready)
                .on_click(move |_, window, cx| {
                    let choice = choice.clone();
                    shell
                        .update(cx, |shell, cx| {
                            shell.compose_in(&project, choice, window, cx)
                        })
                        .ok();
                })
        };
        let actions = div()
            .id(id("place-actions"))
            .flex_none()
            .flex()
            .items_center()
            .gap(space::XS)
            // What the row offers is not a click on the row.
            .on_click(|_, _, cx| cx.stop_propagation())
            .when(ready, |d| d.child(run_menu))
            .child(new_session)
            .child(self.menu.clone())
            .test_support();
        let row = div()
            .id(id("place-row"))
            .aria_label(SharedString::from(name.clone()))
            .h(px(36.))
            .px(space::SM)
            .rounded(px(8.))
            .flex()
            .items_center()
            .gap(space::SM)
            // 16 px wide: the name starts where the line under it does.
            .child(
                div().w(px(16.)).flex_none().flex().justify_center().child(
                    icon(if place.is_none() {
                        Icon::Folder
                    } else {
                        Icon::Branch
                    })
                    .size(px(14.))
                    .text_color(p.faint),
                ),
            )
            // The name is whole, never cut for what follows it.
            .child(
                div()
                    .flex_none()
                    .medium()
                    .text_size(px(14.))
                    .text_color(p.text)
                    .child(SharedString::from(name.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .children(self.chips(&name, cx)),
            );
        let row = hoverable(row, move |d, t| {
            let shown = if kept { 1.0 } else { t };
            d.bg(p.hover.opacity(t)).child(actions.opacity(shown))
        });
        let highlight = fade(id("place-highlight"), self.highlighted, PANEL, window, cx);

        let facts = self.facts_line(&name, cx);
        let steps = self.setup(&name, now, cx);
        let setup = presence(
            id("place-setup"),
            !ready && steps.is_some(),
            div().pl(space::MD).children(steps),
        );
        let sessions = self.sessions(now, cx);
        let runs = self.run_rows(now, window, cx);

        let confirm = {
            let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
            Confirm::new(
                id("workspace-removal"),
                format!("Remove the workspace {name}?"),
            )
            .sentence(
                "Its folder goes, and each repository's branch that holds no commit of its own. \
                 Flottille removes it only once all its work is committed and pushed.",
            )
            .action("Remove workspace")
            .refused(self.removal_refused())
            .open(self.confirming)
            .on_confirm(move |_, cx| {
                this.update(cx, |this, cx| this.perform(Busy::Removing, cx))
                    .ok();
            })
            .on_cancel(move |_, cx| {
                again
                    .update(cx, |this, cx| {
                        this.confirming = false;
                        cx.notify();
                    })
                    .ok();
            })
        };
        let link = place.is_some().then(|| self.link_modal(&name, window, cx));
        let bounds = self.bounds.clone();
        div()
            .id(super::place::block_id(&name))
            .relative()
            .rounded(px(10.))
            .border_1()
            .border_color(mix(p.line.opacity(0.), p.focus_line, highlight))
            .bg(p.hover.opacity(highlight * 0.5))
            .py(space::XS)
            .flex()
            .flex_col()
            .child(
                canvas(move |at, _, _| bounds.set(Some(at)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(confirm)
            .children(link)
            .child(row)
            .children(facts)
            .child(setup)
            .children(sessions)
            .children(runs)
            .test_support()
    }
}
