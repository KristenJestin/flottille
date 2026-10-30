//! A project's tasks (`docs/projects.md`, "Tasks"): its named command lines. The "Tasks" section
//! of its Settings adds, edits and removes them; the project page lists them with a Run button,
//! which runs one in the project folder, and lists what the user ran there as the session's rail
//! lists the agents' commands, each opening its output, live, in the same panel. The window writes
//! nothing itself: every change and every run is the daemon's (`task.*`, `run.*`).

use std::time::Duration;

use std::collections::HashMap;

use flottille_ui::{
    AgentState, Button, Confirm, EmptyLine, Icon, Item, ListCard, ListRow, Listing, Modal,
    Placement, Section, StateIcon, Status, Switch, Toast, Type, icon, palette, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, WeakEntity,
    Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{
    boxed, empty_note, heading, line_field, perform_later, problem_line, save_or_saving,
};
use crate::conversation::Ending;
use crate::project_settings::Reveal;
use crate::runs::{CommandList, CommandOutput, CommandPanel, CommandRow, arrange_keeping};
use crate::session::seconds;
use crate::shell::{Shell, now_ms};

/// How often the project page reads its runs again while one runs or its output is open.
const RUNS_EVERY: Duration = Duration::from_secs(1);

/// A task as the daemon lists it (`docs/protocol.md`, "The task methods").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskInfo {
    pub name: String,
    pub line: String,
    /// Where it runs; the place's top when none.
    pub repository: Option<String>,
    pub serve: bool,
    pub check: bool,
    pub read_only: bool,
    pub ask_before_running: bool,
}

impl TaskInfo {
    pub fn of(value: &Value) -> Self {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        Self {
            name: text("name"),
            line: text("line"),
            repository: value["repository"].as_str().map(str::to_owned),
            serve: value["serve"] == true,
            check: value["check"] == true,
            read_only: value["read_only"] == true,
            ask_before_running: value["ask_before_running"] == true,
        }
    }

    /// What it is, in a few words: where it runs and its flags.
    pub fn notes(&self) -> String {
        let mut notes = Vec::new();
        if let Some(repository) = &self.repository {
            notes.push(format!("in {repository}"));
        }
        for (set, note) in [
            (self.serve, "keeps running"),
            (self.check, "a check"),
            (self.read_only, "changes nothing"),
            (self.ask_before_running, "judged each time an agent runs it"),
        ] {
            if set {
                notes.push(note.to_owned());
            }
        }
        notes.join(" · ")
    }
}

/// The flags of a task, as the form switches them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    Serve,
    Check,
    ReadOnly,
    AskBeforeRunning,
}

impl Flag {
    const ALL: [Flag; 4] = [
        Flag::Serve,
        Flag::Check,
        Flag::ReadOnly,
        Flag::AskBeforeRunning,
    ];

    fn key(self) -> &'static str {
        match self {
            Flag::Serve => "serve",
            Flag::Check => "check",
            Flag::ReadOnly => "read_only",
            Flag::AskBeforeRunning => "ask_before_running",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Flag::Serve => "Keeps running (a server)",
            Flag::Check => "A check (tests, lint)",
            Flag::ReadOnly => "Changes nothing",
            Flag::AskBeforeRunning => "Judged each time an agent runs it",
        }
    }

    fn of(self, info: &TaskInfo) -> bool {
        match self {
            Flag::Serve => info.serve,
            Flag::Check => info.check,
            Flag::ReadOnly => info.read_only,
            Flag::AskBeforeRunning => info.ask_before_running,
        }
    }
}

struct Fields {
    name: Entity<InputState>,
    line: Entity<InputState>,
    repository: Entity<InputState>,
}

/// The "Tasks" section of the project's Settings.
pub struct TasksSection {
    shell: WeakEntity<Shell>,
    project: String,
    listed: Option<Vec<TaskInfo>>,
    /// The rows as drawn: a removed task folds away before it is gone.
    rows: Listing<TaskInfo>,
    /// What the daemon said when the list could not be read, or a removal was refused.
    problem: Option<SharedString>,
    /// The form is open: on a new task (`Some(None)`) or the one named.
    editing: Option<Option<String>>,
    /// The flags the form shows.
    flags: TaskInfo,
    fields: Option<Fields>,
    saving: bool,
    form_error: Option<SharedString>,
    confirming: Option<String>,
    /// What the confirmation names, kept while it fades away.
    asked: String,
    _subscriptions: Vec<Subscription>,
}

impl TasksSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            listed: None,
            rows: Listing::default(),
            problem: None,
            editing: None,
            flags: TaskInfo::default(),
            fields: None,
            saving: false,
            form_error: None,
            confirming: None,
            asked: String::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// The tasks as last listed; `None` until the daemon has said.
    pub fn tasks(&self) -> Option<&[TaskInfo]> {
        self.listed.as_deref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn form_error(&self) -> Option<&str> {
        self.form_error.as_deref()
    }

    pub fn is_open(&self) -> bool {
        self.editing.is_some()
    }

    pub fn name_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.name)
    }

    pub fn line_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.line)
    }

    pub fn repository_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.repository)
    }

    /// Asks the daemon for the list.
    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(&self.shell, Command::Tasks { project }, cx);
    }

    /// The daemon's list.
    pub fn listed(&mut self, listed: Result<Vec<TaskInfo>, String>, cx: &mut Context<Self>) {
        match listed {
            Ok(list) => {
                self.problem = None;
                if self.listed.as_ref() != Some(&list) {
                    self.listed = Some(list);
                }
            }
            Err(why) => self.problem = Some(why.into()),
        }
        cx.notify();
    }

    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        if self.fields.is_none() {
            let name = line_field("name, as agents run it: test", window, cx);
            let line = line_field(
                "one command line, never run by a shell: cargo test",
                window,
                cx,
            );
            let repository = line_field(
                "repository, for example api; the place's top when empty",
                window,
                cx,
            );
            // Typing clears what a refusal said.
            for input in [&name, &line, &repository] {
                self._subscriptions
                    .push(cx.subscribe(input, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) && this.form_error.take().is_some() {
                            cx.notify();
                        }
                    }));
            }
            self.fields = Some(Fields {
                name,
                line,
                repository,
            });
        }
        self.fields.as_ref().expect("made above")
    }

    /// "New task", or "Edit" on one: the form, filled with it.
    pub fn open(&mut self, info: Option<&TaskInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields(window, cx);
        let (name, line, repository) = (
            fields.name.clone(),
            fields.line.clone(),
            fields.repository.clone(),
        );
        let shown = info.cloned().unwrap_or_default();
        name.update(cx, |f, cx| f.set_value(shown.name.clone(), window, cx));
        line.update(cx, |f, cx| f.set_value(shown.line.clone(), window, cx));
        repository.update(cx, |f, cx| {
            f.set_value(shown.repository.clone().unwrap_or_default(), window, cx)
        });
        self.flags = shown;
        self.editing = Some(info.map(|info| info.name.clone()));
        self.form_error = None;
        self.saving = false;
        let focus = if info.is_some() { line } else { name }
            .read(cx)
            .focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.form_error = None;
        cx.notify();
    }

    pub fn toggle(&mut self, flag: Flag, cx: &mut Context<Self>) {
        let on = !flag.of(&self.flags);
        match flag {
            Flag::Serve => self.flags.serve = on,
            Flag::Check => self.flags.check = on,
            Flag::ReadOnly => self.flags.read_only = on,
            Flag::AskBeforeRunning => self.flags.ask_before_running = on,
        }
        cx.notify();
    }

    fn write(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        self.saving = true;
        self.form_error = None;
        self.problem = None;
        let command = Command::TaskWrite {
            project: self.project.clone(),
            method,
            params,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// Saves the form: a new task, or the one being edited.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(original) = self.editing.clone() else {
            return;
        };
        let Some(fields) = &self.fields else {
            return;
        };
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let name = original.clone().unwrap_or_else(|| value(&fields.name));
        let repository = value(&fields.repository);
        let mut params = json!({
            "project": self.project,
            "name": name,
            "line": value(&fields.line),
            "repository": if repository.is_empty() { Value::Null } else { repository.into() },
        });
        for flag in Flag::ALL {
            params[flag.key()] = flag.of(&self.flags).into();
        }
        let method = match original {
            Some(_) => "task.edit",
            None => "task.add",
        };
        self.write(method, params, cx);
    }

    /// Asks to remove, in a confirmation; called again from it, removes.
    pub fn remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(name) {
            self.confirming = Some(name.to_owned());
            self.asked = name.to_owned();
            cx.notify();
            return;
        }
        self.confirming = None;
        let params = json!({"project": self.project, "name": name});
        self.write("task.remove", params, cx);
    }

    /// The daemon's answer to a write: done, and the list is read again; or refused, and why.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        match refused {
            None => {
                self.editing = None;
                self.form_error = None;
                self.read(cx);
            }
            Some(why) if self.editing.is_none() => self.problem = Some(why.into()),
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    fn row(&self, info: &TaskInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let id = |part: &str| ElementId::Name(format!("{part}-{}", info.name).into());
        let name = info.name.clone();
        div()
            .id(id("task"))
            .px(space::LG)
            .py(space::MD)
            .rounded(px(10.))
            .border_1()
            .border_color(p.line)
            .bg(p.card)
            .flex()
            .items_start()
            .gap(space::MD)
            .child(task_text(info, &p))
            .child({
                let edit = info.clone();
                Button::quiet(id("edit-task"), "Edit").on_click(
                    cx.listener(move |this, _, window, cx| this.open(Some(&edit), window, cx)),
                )
            })
            .child({
                Button::quiet(id("remove-task"), "Remove")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(&name, cx)))
                    .into_any_element()
            })
            .test_support()
    }

    /// The confirmation of a removal.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let name = self.asked.clone();
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("task-removal", format!("Remove the task {name}?"))
            .sentence("Agents can no longer run it, and it leaves the project's page.")
            .action("Remove task")
            .open(self.confirming.is_some())
            .on_confirm(move |_, cx| {
                let name = name.clone();
                this.update(cx, |this, cx| this.remove(&name, cx)).ok();
            })
            .on_cancel(move |_, cx| {
                again
                    .update(cx, |this, cx| {
                        this.confirming = None;
                        cx.notify();
                    })
                    .ok();
            })
    }

    fn modal(&mut self, cx: &mut Context<Self>) -> Modal {
        let this = cx.entity().downgrade();
        let title = match &self.editing {
            Some(Some(_)) => "Edit task",
            _ => "New task",
        };
        let modal = Modal::new("task-modal", title)
            .wide()
            .open(self.editing.is_some())
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let (Some(original), Some(fields)) = (self.editing.clone(), self.fields.as_ref()) else {
            return modal;
        };
        let switches = Flag::ALL.into_iter().map(|flag| {
            Switch::new(
                ElementId::Name(format!("task-flag-{}", flag.key()).into()),
                flag.of(&self.flags),
                flag.label(),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(flag, cx)))
        });
        let body = div()
            .id("task-form")
            .flex()
            .flex_col()
            .gap(space::MD)
            .child(match &original {
                Some(name) => div()
                    .mono(12.5)
                    .semibold()
                    .child(SharedString::from(name.clone()))
                    .into_any_element(),
                None => boxed("task-name", Input::new(&fields.name), cx).into_any_element(),
            })
            .child(boxed("task-line", Input::new(&fields.line), cx))
            .child(boxed("task-repository", Input::new(&fields.repository), cx))
            .child(div().flex().flex_col().gap(space::SM).children(switches))
            .child(problem_line(
                "task-error",
                self.form_error.clone(),
                space::MD,
                cx,
            ))
            .test_support();
        modal.child(body).action(save_or_saving(
            "task",
            self.saving,
            || {
                Button::primary("save-task", "Save")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
            },
            cx,
        ))
    }
}

/// A task's name, line and notes, as both lists show them.
fn task_text(info: &TaskInfo, p: &flottille_ui::Palette) -> impl IntoElement {
    let id = |part: &str| ElementId::Name(format!("{part}-{}", info.name).into());
    let notes = info.notes();
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(space::XS)
        .child(
            div()
                .mono(12.5)
                .semibold()
                .child(SharedString::from(info.name.clone())),
        )
        .child(
            div()
                .id(id("task-line"))
                .mono(12.)
                .text_color(p.mute)
                .overflow_hidden()
                .text_ellipsis()
                .aria_label(SharedString::from(info.line.clone()))
                .child(SharedString::from(info.line.clone()))
                .test_support(),
        )
        .when(!notes.is_empty(), |d| {
            d.child(
                div()
                    .id(id("task-notes"))
                    .text_size(px(11.5))
                    .text_color(p.faint)
                    .child(SharedString::from(notes)),
            )
        })
}

impl Render for TasksSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows
                .sync(listed.into_iter().map(|t| (t.name.clone(), t)), now, reduce);
        }
        let rows: Vec<_> = self
            .rows
            .rows()
            .map(|row| {
                Item::new(
                    ElementId::Name(format!("task-item-{}", row.key).into()),
                    row.placement,
                    self.row(row.value, cx),
                )
                .gap(space::SM)
            })
            .collect();
        let count = self.rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(cx);
        div()
            .id("tasks")
            .child(heading(
                "Tasks",
                count,
                Button::secondary("add-task", "New task")
                    .leading(flottille_ui::Icon::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.open(None, window, cx))),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .children(rows)
                    .child(empty_note(
                        "no-tasks",
                        empty,
                        "No task yet: name the lines the project runs (tests, lint, a server). \
                         Agents run them by name, and the project page has a Run button for each.",
                        cx,
                    ))
                    .child(problem_line(
                        "tasks-problem",
                        self.problem.clone(),
                        space::SM,
                        cx,
                    )),
            )
            .child(modal)
            .child(self.confirm(cx))
    }
}

// ── The project page ───────────────────────────────────────────────────────────────────────────

/// The finished runs the project page keeps in sight under the running ones.
pub const KEPT_ON_THE_PAGE: usize = 5;

/// The project page's Runs and Tasks: each task with Run, and what the user ran, running first
/// then the last five finished, each opening its detail in the run drawer.
pub struct ProjectTasks {
    shell: WeakEntity<Shell>,
    project: String,
    listed: Option<Vec<TaskInfo>>,
    /// Why the list could not be read.
    problem: Option<SharedString>,
    /// The task being started.
    starting: Option<String>,
    commands: Entity<CommandList>,
    panel: Entity<CommandPanel>,
    /// The runs as drawn: one that leaves the five folds away.
    runs: Listing<CommandRow>,
    /// Each run's mark, kept while it is listed: it turns to how the run ended.
    marks: HashMap<String, (Entity<Status>, Entity<StateIcon>)>,
    rows: Listing<TaskInfo>,
    /// The run whose output is open.
    watching: Option<String>,
    /// Reads the runs again each second while one runs or its output is open.
    ticker: Option<Task<()>>,
    _commands: Subscription,
}

impl ProjectTasks {
    pub fn new(shell: WeakEntity<Shell>, project: String, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let commands = cx.new(|_| CommandList::new(this.clone()));
        // The list's own ticker moves the times of the runs drawn here.
        let observed = cx.observe(&commands, |_, _, cx| cx.notify());
        Self {
            shell,
            project,
            listed: None,
            problem: None,
            starting: None,
            commands,
            panel: cx.new(|cx| CommandPanel::new(this, cx)),
            runs: Listing::default(),
            marks: HashMap::new(),
            rows: Listing::default(),
            watching: None,
            ticker: None,
            _commands: observed,
        }
    }

    pub fn tasks(&self) -> Option<&[TaskInfo]> {
        self.listed.as_deref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn commands(&self) -> &Entity<CommandList> {
        &self.commands
    }

    /// The run drawer: the page draws it over itself.
    pub fn panel(&self) -> &Entity<CommandPanel> {
        &self.panel
    }

    /// Asks the daemon for the tasks and the runs.
    pub fn read(&self, cx: &mut App) {
        perform_later(
            &self.shell,
            Command::Tasks {
                project: self.project.clone(),
            },
            cx,
        );
        self.read_runs(cx);
    }

    fn read_runs(&self, cx: &mut App) {
        perform_later(
            &self.shell,
            Command::ProjectRuns {
                project: self.project.clone(),
                output: self.watching.clone(),
            },
            cx,
        );
    }

    pub fn listed(&mut self, listed: Result<Vec<TaskInfo>, String>, cx: &mut Context<Self>) {
        match listed {
            Ok(list) => {
                self.problem = None;
                if self.listed.as_ref() != Some(&list) {
                    self.listed = Some(list);
                    cx.notify();
                }
            }
            Err(why) => {
                self.problem = Some(why.into());
                cx.notify();
            }
        }
    }

    /// The user's Run: the task starts in the project folder, and its row joins the runs.
    pub fn run(&mut self, name: &str, cx: &mut Context<Self>) {
        self.starting = Some(name.to_owned());
        let command = Command::RunTask {
            project: self.project.clone(),
            name: name.to_owned(),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon started the task, or refused with why: a toast says which.
    pub fn started(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        let name = self.starting.take().unwrap_or_default();
        toast(
            match refused {
                None => Toast::success(format!("{name} started"))
                    .detail("In the project folder. Its output is under Runs."),
                Some(why) => Toast::failure(format!("{name} did not start")).detail(why),
            },
            cx,
        );
        self.read_runs(cx);
        cx.notify();
    }

    /// The runs the daemon listed, and the output of the one open.
    pub fn runs_listed(
        &mut self,
        listed: Result<Vec<CommandRow>, String>,
        output: Option<CommandOutput>,
        cx: &mut Context<Self>,
    ) {
        let running = match listed {
            Ok(rows) => {
                let running = rows.iter().any(CommandRow::running);
                self.commands.update(cx, |list, cx| list.set(rows, cx));
                running
            }
            Err(why) => {
                self.commands
                    .update(cx, |list, cx| list.failed(Some(why), cx));
                false
            }
        };
        if let Some(output) = output {
            self.panel.update(cx, |panel, cx| panel.set(output, cx));
        }
        self.follow(running || self.watching.is_some(), cx);
    }

    /// Reads the runs each second while `on`.
    fn follow(&mut self, on: bool, cx: &mut Context<Self>) {
        if !on {
            self.ticker = None;
            return;
        }
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RUNS_EVERY).await;
                if this.update(cx, |this, cx| this.read_runs(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Opens a run's detail in the drawer, its output followed live.
    pub fn open_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.watching = Some(run.to_owned());
        self.panel.update(cx, |panel, cx| panel.open(run, cx));
        self.read_runs(cx);
        self.follow(true, cx);
    }

    pub fn close_command(&mut self, cx: &mut Context<Self>) {
        self.watching = None;
        self.panel.update(cx, |panel, cx| panel.close(cx));
    }

    pub fn stop_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.answer_run(run, false, cx);
    }

    pub fn dismiss_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.answer_run(run, true, cx);
    }

    fn answer_run(&mut self, run: &str, dismiss: bool, cx: &mut Context<Self>) {
        let command = Command::ProjectRun {
            project: self.project.clone(),
            run: run.to_owned(),
            dismiss,
        };
        perform_later(&self.shell, command, cx);
    }

    /// What the daemon answered to a stop or a dismissal: a refusal is a toast.
    pub fn run_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        if let Some(why) = refused {
            toast(Toast::failure("The run was not stopped").detail(why), cx);
        }
        self.read_runs(cx);
    }

    fn mark(&mut self, row: &CommandRow, cx: &mut Context<Self>) -> Entity<StateIcon> {
        let state = match row.ending() {
            Ending::Running => AgentState::Running,
            Ending::Exited(0) => AgentState::Completed,
            Ending::Exited(_) | Ending::Killed(_) => AgentState::Failed,
            Ending::Stopped | Ending::Interrupted => AgentState::Stopped,
        };
        let (status, icon) = self.marks.entry(row.run.clone()).or_insert_with(|| {
            let status = cx.new(|_| Status::new(state));
            let icon = {
                let status = status.clone();
                cx.new(|cx| StateIcon::new(status, cx))
            };
            (status, icon)
        });
        if status.read(cx).state() != state {
            status.update(cx, |status, cx| status.set(state, cx));
        }
        icon.clone()
    }

    fn runs_section(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows = self.commands.read(cx).rows().to_vec();
        let arranged = arrange_keeping(&rows, None, KEPT_ON_THE_PAGE);
        let running = arranged.running.len();
        let shown: Vec<(String, CommandRow)> = arranged
            .running
            .iter()
            .chain(&arranged.kept)
            .map(|row| (row.run.clone(), (*row).clone()))
            .collect();
        let at = cx.background_executor().now();
        self.runs.sync(shown, at, cx.reduce_motion());
        let listed: Vec<(String, CommandRow, Placement)> = self
            .runs
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        self.marks
            .retain(|run, _| listed.iter().any(|(key, ..)| key == run));
        if listed.is_empty() {
            return None;
        }
        let p = palette(cx);
        let now = now_ms();
        let mut card = ListCard::new("project-runs-list");
        for (key, row, placement) in listed {
            let mark = self.mark(&row, cx);
            let who = match row.agent.as_str() {
                "" => "by you".to_owned(),
                agent => format!("by agent {agent}"),
            };
            let detail = match &row.name {
                Some(task) => format!("task {task} · {who} · {}", seconds(row.elapsed_ms(now))),
                None => format!("{who} · {}", seconds(row.elapsed_ms(now))),
            };
            let page = cx.entity().downgrade();
            let open = row.run.clone();
            let stop = row.running().then(|| {
                let (page, run) = (page.clone(), row.run.clone());
                Button::quiet(
                    ElementId::Name(format!("command-stop-{}", row.run).into()),
                    "Stop",
                )
                .leading(Icon::Stop)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    page.update(cx, |tasks, cx| tasks.stop_command(&run, cx))
                        .ok();
                })
            });
            let mut list_row = ListRow::new(
                ElementId::Name(format!("command-{}", row.run).into()),
                SharedString::from(row.line.clone()),
            )
            .mono()
            .leading(mark)
            .detail(SharedString::from(detail))
            .trailing(
                div()
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .children(stop)
                    .child(
                        icon(Icon::Chevron)
                            .size(px(13.))
                            .text_color(p.faint)
                            .with_transformation(gpui_kit::Transformation::rotate(
                                gpui_kit::radians(-std::f32::consts::FRAC_PI_2),
                            )),
                    ),
            )
            .on_click(move |_, _, cx| {
                page.update(cx, |tasks, cx| tasks.open_command(&open, cx))
                    .ok();
            });
            let last = flottille_core::ansi::strip(&row.last_line);
            if !last.is_empty() {
                list_row = list_row.line(SharedString::from(last));
            }
            card = card.item(
                ElementId::Name(format!("command-item-{key}").into()),
                placement,
                list_row,
            );
        }
        let count = match running {
            0 => format!("{} finished", arranged.kept.len()),
            n => format!("{n} running"),
        };
        Some(
            Section::new("project-runs", "Runs")
                .count(count)
                .child(card)
                .into_any_element(),
        )
    }
}

impl Render for ProjectTasks {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let runs = self.runs_section(cx);
        let tasks = self.listed.clone().unwrap_or_default();
        let at = cx.background_executor().now();
        self.rows.sync(
            tasks.iter().map(|info| (info.name.clone(), info.clone())),
            at,
            cx.reduce_motion(),
        );
        let listed: Vec<(String, TaskInfo, Placement)> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let mut card = ListCard::new("project-tasks-list");
        for (key, info, placement) in listed {
            let name = info.name.clone();
            let starting = self.starting.as_deref() == Some(&info.name);
            let mut row = ListRow::new(
                ElementId::Name(format!("project-task-{}", info.name).into()),
                SharedString::from(info.name.clone()),
            )
            .mono()
            .line(SharedString::from(info.line.clone()))
            .trailing(
                Button::secondary(
                    ElementId::Name(format!("run-task-{}", info.name).into()),
                    if starting { "Starting…" } else { "Run" },
                )
                .leading(Icon::Play)
                .disabled(self.starting.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.run(&name, cx))),
            );
            for tag in [
                info.serve.then_some("server"),
                info.check.then_some("check"),
                info.read_only.then_some("changes nothing"),
            ]
            .into_iter()
            .flatten()
            {
                row = row.tag(tag);
            }
            if let Some(repository) = &info.repository {
                row = row.tag(SharedString::from(format!("in {repository}")));
            }
            card = card.item(
                ElementId::Name(format!("project-task-item-{key}").into()),
                placement,
                row,
            );
        }
        let (shell, project) = (self.shell.clone(), self.project.clone());
        let edit = Button::quiet("edit-tasks-in-settings", "Edit in Settings").on_click(
            move |_, window, cx| {
                let project = project.clone();
                shell
                    .update(cx, |shell, cx| {
                        shell.open_project_settings_at(&project, Reveal::Tasks, window, cx)
                    })
                    .ok();
            },
        );
        let mut section = Section::new("project-tasks", "Tasks")
            .count(tasks.len().to_string())
            .action(edit);
        section = match (&self.problem, tasks.is_empty()) {
            (Some(problem), _) => section.sentence(problem.clone()),
            (None, true) => section.child(EmptyLine::new(
                "no-project-tasks",
                Icon::Play,
                "No task yet: a task is a command line of the project, run here or by an agent. \
                 Add one in Settings.",
            )),
            (None, false) => section.child(card),
        };
        div()
            .id("project-tasks")
            .flex()
            .flex_col()
            .gap(space::XXL)
            .text_color(p.text)
            .children(runs)
            .child(section)
    }
}
