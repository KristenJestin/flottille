//! A project's tasks (`docs/projects.md`, "Tasks"): its named command lines. The "Tasks" section
//! of its Settings adds, edits and removes them; the project page lists them with a Run button,
//! which runs one in the project folder, and lists what the user ran there as the session's rail
//! lists the agents' commands, each opening its output, live, in the same panel. The window writes
//! nothing itself: every change and every run is the daemon's (`task.*`, `run.*`).

use std::time::Duration;

use std::collections::HashMap;

use flottille_ui::drawer::Closed;
use flottille_ui::{
    Button, Confirm, Drawer, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal,
    Section, Select, SelectOption, Switch, Toast, Type, palette, presence, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{boxed, heading, line_field, menu_of, perform_later, problem_line};
use crate::project::repository_label;
use crate::runs::{CommandList, CommandOutput, CommandPanel, CommandRow};
use crate::shell::Shell;

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
    pub const ALL: [Flag; 4] = [
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

    pub fn label(self) -> &'static str {
        match self {
            Flag::Serve => "Keeps running",
            Flag::Check => "A check",
            Flag::ReadOnly => "Changes nothing",
            Flag::AskBeforeRunning => "Ask before each run",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Flag::Serve => "A server: it is listed under Runs until it is stopped.",
            Flag::Check => "Tests or lint: its result is told to the agent that ran it.",
            Flag::ReadOnly => "Agents run it without asking, even in main.",
            Flag::AskBeforeRunning => "Held for you (or the judge) every time an agent runs it.",
        }
    }

    /// The tag a row shows for it.
    fn tag(self) -> &'static str {
        match self {
            Flag::Serve => "server",
            Flag::Check => "check",
            Flag::ReadOnly => "read-only",
            Flag::AskBeforeRunning => "asks",
        }
    }

    pub fn key_of(self) -> &'static str {
        self.key()
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
}

/// What a write was, for the toast its answer says.
#[derive(Clone, Debug, PartialEq)]
enum Writing {
    Added(String),
    Saved(String),
    Removed(String),
}

/// The "Tasks" section of the project's Settings.
pub struct TasksSection {
    shell: WeakEntity<Shell>,
    project: String,
    listed: Option<Vec<TaskInfo>>,
    /// The rows as drawn: a removed task folds away before it is gone.
    rows: Listing<TaskInfo>,
    menus: HashMap<String, Entity<Menu>>,
    /// The project's repositories, where a task may run.
    repositories: Vec<String>,
    /// Why the list cannot be read.
    problem: Option<SharedString>,
    /// The form is open: on a new task (`Some(None)`) or the one named.
    editing: Option<Option<String>>,
    /// The flags and the repository the form shows.
    flags: TaskInfo,
    fields: Option<Fields>,
    saving: bool,
    writing: Option<Writing>,
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
            menus: HashMap::new(),
            repositories: Vec::new(),
            problem: None,
            editing: None,
            flags: TaskInfo::default(),
            fields: None,
            saving: false,
            writing: None,
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

    pub fn is_confirming(&self) -> bool {
        self.confirming.is_some()
    }

    /// Where the form runs the task: a repository, or the place's folder.
    pub fn repository(&self) -> Option<&str> {
        self.flags.repository.as_deref()
    }

    pub fn set_repositories(&mut self, repositories: Vec<String>, cx: &mut Context<Self>) {
        if self.repositories != repositories {
            self.repositories = repositories;
            cx.notify();
        }
    }

    /// Runs the task in `repository`, or in the place's folder.
    pub fn choose_repository(&mut self, repository: Option<String>, cx: &mut Context<Self>) {
        self.flags.repository = repository;
        cx.notify();
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
            let name = line_field("test", window, cx);
            let line = line_field("pnpm test", window, cx);
            // Typing clears what a refusal said.
            for input in [&name, &line] {
                self._subscriptions
                    .push(cx.subscribe(input, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) && this.form_error.take().is_some() {
                            cx.notify();
                        }
                    }));
            }
            self.fields = Some(Fields { name, line });
        }
        self.fields.as_ref().expect("made above")
    }

    /// "New task", or "Edit" on one: the form, filled with it.
    pub fn open(&mut self, info: Option<&TaskInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields(window, cx);
        let (name, line) = (fields.name.clone(), fields.line.clone());
        let shown = info.cloned().unwrap_or_default();
        name.update(cx, |f, cx| f.set_value(shown.name.clone(), window, cx));
        line.update(cx, |f, cx| f.set_value(shown.line.clone(), window, cx));
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

    fn write(
        &mut self,
        method: &'static str,
        params: Value,
        writing: Writing,
        cx: &mut Context<Self>,
    ) {
        self.saving = true;
        self.writing = Some(writing);
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
        let mut params = json!({
            "project": self.project,
            "name": name,
            "line": value(&fields.line),
            "repository": self.flags.repository,
        });
        for flag in Flag::ALL {
            params[flag.key()] = flag.of(&self.flags).into();
        }
        let (method, writing) = match original {
            Some(_) => ("task.edit", Writing::Saved(name)),
            None => ("task.add", Writing::Added(name)),
        };
        self.write(method, params, writing, cx);
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
        let writing = Writing::Removed(name.to_owned());
        self.write("task.remove", params, writing, cx);
    }

    /// The daemon's answer to a write: a toast says it, and the list is read again; a refusal of
    /// the dialog's stays in it.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        let writing = self.writing.take();
        match refused {
            None => {
                self.editing = None;
                self.form_error = None;
                let said = match writing {
                    Some(Writing::Added(name)) => Toast::success(format!("Task {name} added"))
                        .detail("Agents run it by its name; the project page has a Run for it."),
                    Some(Writing::Saved(name)) => Toast::success(format!("Task {name} saved")),
                    Some(Writing::Removed(name)) => Toast::success(format!("Task {name} removed")),
                    None => Toast::success("Tasks saved"),
                };
                toast(said, cx);
                self.read(cx);
            }
            Some(why) if self.editing.is_none() => {
                toast(Toast::failure("The task was not removed").detail(why), cx)
            }
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    fn menu(&mut self, name: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let this = cx.entity().downgrade();
        menu_of(&mut self.menus, "task", name, cx, move |name, menu| {
            let (edit, remove) = (this.clone(), this.clone());
            let (to_edit, to_remove) = (name.to_owned(), name.to_owned());
            menu.entry(
                format!("edit-task-{name}"),
                "Edit",
                None,
                move |window, cx| {
                    edit.update(cx, |this, cx| {
                        let info = this
                            .listed
                            .iter()
                            .flatten()
                            .find(|t| t.name == to_edit)
                            .cloned();
                        if let Some(info) = info {
                            this.open(Some(&info), window, cx);
                        }
                    })
                    .ok();
                },
            )
            .danger(format!("remove-task-{name}"), "Remove…", move |_, cx| {
                remove
                    .update(cx, |this, cx| this.remove(&to_remove, cx))
                    .ok();
            })
        })
    }

    fn row(&mut self, info: &TaskInfo, cx: &mut Context<Self>) -> ListRow {
        let menu = self.menu(&info.name, cx);
        let mut row = ListRow::new(
            ElementId::Name(format!("task-{}", info.name).into()),
            SharedString::from(info.name.clone()),
        )
        .mono()
        .line(SharedString::from(info.line.clone()))
        .mono_line()
        .more(menu);
        for flag in Flag::ALL {
            if flag.of(info) {
                row = row.tag(flag.tag());
            }
        }
        row = row.detail(SharedString::from(match &info.repository {
            Some(repository) => format!("in {}", repository_label(&self.project, repository)),
            None => "in each place's folder".to_owned(),
        }));
        let edit = info.clone();
        row.on_click(cx.listener(move |this, _, window, cx| this.open(Some(&edit), window, cx)))
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
            Some(Some(name)) => format!("Task {name}"),
            _ => "New task".to_owned(),
        };
        let modal = Modal::new("task-modal", title)
            .sentence("A command line the project runs, by name. Never through a shell.")
            .open(self.editing.is_some())
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let (Some(original), Some(fields)) = (self.editing.clone(), self.fields.as_ref()) else {
            return modal;
        };
        let p = palette(cx);
        let switches = Flag::ALL.into_iter().map(|flag| {
            Switch::new(
                ElementId::Name(format!("task-flag-{}", flag.key()).into()),
                flag.of(&self.flags),
                flag.label(),
            )
            .hint(flag.hint())
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(flag, cx)))
        });
        let mut places = vec![
            SelectOption::new("place", "The place's folder")
                .line("The project folder, or the workspace's"),
        ];
        places.extend(self.repositories.iter().map(|path| {
            SelectOption::new(
                SharedString::from(format!("repository-{path}")),
                SharedString::from(repository_label(&self.project, path)),
            )
            .line("Its folder in the place")
        }));
        let chosen = self
            .flags
            .repository
            .as_ref()
            .and_then(|r| self.repositories.iter().position(|p| p == r))
            .map_or(0, |at| at + 1);
        let repositories = self.repositories.clone();
        let this = cx.entity().downgrade();
        let place = Select::new("task-place", places, chosen).on_choose(move |index, _, cx| {
            let repository = index
                .checked_sub(1)
                .and_then(|at| repositories.get(at).cloned());
            this.update(cx, |this, cx| this.choose_repository(repository, cx))
                .ok();
        });
        let name = match &original {
            Some(name) => div()
                .id("task-name")
                .h(px(32.))
                .flex()
                .items_center()
                .mono(12.5)
                .text_color(p.mute)
                .child(SharedString::from(name.clone()))
                .test_support()
                .into_any_element(),
            None => boxed("task-name", Input::new(&fields.name), cx).into_any_element(),
        };
        let body = div()
            .id("task-form")
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(
                div()
                    .flex()
                    .gap(space::MD)
                    .child(
                        div().flex_1().min_w_0().child(
                            Field::new("task-name-field", "Name", name)
                                .hint("Agents run it by this name."),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Field::new("task-place-field", "Runs in", place)
                                .hint("Or one repository of the place."),
                        ),
                    ),
            )
            .child(Field::new(
                "task-line-field",
                "Command line",
                boxed("task-line", Input::new(&fields.line), cx),
            ))
            .child(Field::new(
                "task-kind",
                "What it is",
                div().flex().flex_col().gap(space::MD).children(switches),
            ))
            .child(problem_line(
                "task-error",
                self.form_error.clone(),
                space::LG,
                cx,
            ))
            .test_support();
        modal.child(body).action(
            Button::primary(
                "save-task",
                if original.is_some() {
                    "Save"
                } else {
                    "Add task"
                },
            )
            .disabled(self.saving)
            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
        )
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
        let p = palette(cx);
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows
                .sync(listed.into_iter().map(|t| (t.name.clone(), t)), now, reduce);
        }
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("task-list");
        for (key, placement, info) in drawn {
            let row = self.row(&info, cx);
            list = list.item(
                ElementId::Name(format!("task-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(cx);
        let section = Section::new("tasks", "Tasks")
            .count(count.to_string())
            .sentence(
                "The command lines the project runs. Agents run them by name; the project page \
                 has a Run for each.",
            )
            .action(
                Button::secondary("add-task", "New task")
                    .leading(Icon::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.open(None, window, cx))),
            )
            .child(presence("task-list-block", count > 0, list).gap(space::MD))
            .child(
                presence(
                    "no-tasks-block",
                    empty,
                    EmptyLine::new(
                        "no-tasks",
                        Icon::Play,
                        "No task yet: name the lines the project runs (tests, lint, a server). \
                         Agents run them by name, and the project page has a Run for each.",
                    ),
                )
                .gap(space::MD),
            )
            .child(problem_line(
                "tasks-problem",
                self.problem.clone(),
                space::MD,
                cx,
            ));
        div()
            .text_color(p.text)
            .child(section)
            .child(modal)
            .child(self.confirm(cx))
    }
}

// ── The project page ───────────────────────────────────────────────────────────────────────────

/// The project page's tasks: each with Run, then what the user ran, as the rail lists commands.
pub struct ProjectTasks {
    shell: WeakEntity<Shell>,
    project: String,
    listed: Option<Vec<TaskInfo>>,
    /// Why a task did not start, or the list could not be read.
    problem: Option<SharedString>,
    /// The task being started.
    starting: Option<String>,
    commands: Entity<CommandList>,
    panel: Entity<CommandPanel>,
    /// The drawer a run's detail slides in with, drawn by the page over all of it.
    drawer: Entity<Drawer>,
    _drawer: Subscription,
    /// The run whose output is open.
    watching: Option<String>,
    /// Reads the runs again each second while one runs or its output is open.
    ticker: Option<Task<()>>,
}

impl ProjectTasks {
    pub fn new(shell: WeakEntity<Shell>, project: String, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let panel = cx.new(|_| CommandPanel::new(this.clone()));
        let drawer = cx.new(|cx| Drawer::detail("run-drawer", panel.clone(), cx));
        let _drawer = cx.subscribe(&drawer, |this, _, _: &Closed, cx| this.close_command(cx));
        Self {
            shell,
            project,
            listed: None,
            problem: None,
            starting: None,
            commands: cx.new(|_| CommandList::new(this)),
            panel,
            drawer,
            _drawer,
            watching: None,
            ticker: None,
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

    pub fn panel(&self) -> &Entity<CommandPanel> {
        &self.panel
    }

    /// The drawer of a run's detail, which the page draws over itself.
    pub fn drawer(&self) -> &Entity<Drawer> {
        &self.drawer
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

    /// The user's Run: the task starts in the project folder, and its row joins the list.
    pub fn run(&mut self, name: &str, cx: &mut Context<Self>) {
        self.starting = Some(name.to_owned());
        self.problem = None;
        let command = Command::RunTask {
            project: self.project.clone(),
            name: name.to_owned(),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon started the task, or refused with why.
    pub fn started(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.starting = None;
        self.problem = refused.map(Into::into);
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
                self.panel.update(cx, |panel, cx| panel.set_rows(&rows, cx));
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

    /// Opens a run's detail in its drawer, its output followed live.
    pub fn open_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.watching = Some(run.to_owned());
        let rows = self.commands.read(cx).rows().to_vec();
        self.panel.update(cx, |panel, cx| {
            panel.open(run, cx);
            panel.set_rows(&rows, cx);
        });
        self.drawer.update(cx, |drawer, cx| drawer.open(cx));
        self.read_runs(cx);
        self.follow(true, cx);
    }

    pub fn close_command(&mut self, cx: &mut Context<Self>) {
        self.watching = None;
        self.panel.update(cx, |panel, cx| panel.close(cx));
        self.drawer.update(cx, |drawer, cx| drawer.close(cx));
    }

    pub fn stop_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.answer_run(run, false, cx);
    }

    fn answer_run(&mut self, run: &str, dismiss: bool, cx: &mut Context<Self>) {
        self.commands.update(cx, |list, cx| list.failed(None, cx));
        let command = Command::ProjectRun {
            project: self.project.clone(),
            run: run.to_owned(),
            dismiss,
        };
        perform_later(&self.shell, command, cx);
    }

    /// What the daemon answered to a stop or a dismissal.
    pub fn run_answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.commands
            .update(cx, |list, cx| list.failed(refused, cx));
        self.read_runs(cx);
    }
}

impl Render for ProjectTasks {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let tasks = self.listed.clone().unwrap_or_default();
        if tasks.is_empty() && self.commands.read(cx).rows().is_empty() {
            return div().id("project-tasks").into_any_element();
        }
        let rows = tasks.iter().map(|info| {
            let name = info.name.clone();
            let starting = self.starting.as_deref() == Some(&info.name);
            div()
                .id(ElementId::Name(
                    format!("project-task-{}", info.name).into(),
                ))
                .px(space::LG)
                .py(space::MD)
                .rounded(px(10.))
                .border_1()
                .border_color(p.line)
                .bg(p.card)
                .flex()
                .items_center()
                .gap(space::MD)
                .child(task_text(info, &p))
                .child(
                    Button::secondary(
                        ElementId::Name(format!("run-task-{}", info.name).into()),
                        if starting { "Starting…" } else { "Run" },
                    )
                    .disabled(self.starting.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| this.run(&name, cx))),
                )
                .test_support()
        });
        let count = tasks.len();
        div()
            .id("project-tasks")
            .child(heading("Tasks", count, div(), cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .children(rows)
                    .when_some(self.problem.clone(), |d, problem| {
                        d.child(
                            div()
                                .id("project-tasks-problem")
                                .text_size(px(12.5))
                                .text_color(p.fail)
                                .child(problem)
                                .test_support(),
                        )
                    }),
            )
            .child(div().max_w(px(520.)).child(self.commands.clone()))
            .into_any_element()
    }
}
