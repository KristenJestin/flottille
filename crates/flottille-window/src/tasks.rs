//! A project's tasks (`docs/projects.md`, "Tasks"): its named command lines. The "Tasks" section
//! of its Settings adds, edits and removes them; the project page lists them with a Run button,
//! which runs one in the project folder, and lists what the user ran there as the session's rail
//! lists the agents' commands, each opening its output, live, in the same panel. The window writes
//! nothing itself: every change and every run is the daemon's (`task.*`, `run.*`).

use std::time::Duration;

use flottille_ui::{Button, Modal, Switch, Type, palette};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{boxed, heading, line_field, perform_later};
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
    _subscriptions: Vec<Subscription>,
}

impl TasksSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            listed: None,
            problem: None,
            editing: None,
            flags: TaskInfo::default(),
            fields: None,
            saving: false,
            form_error: None,
            confirming: None,
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

    /// Asks to remove; the second click confirms.
    pub fn remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(name) {
            self.confirming = Some(name.to_owned());
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
        let confirming = self.confirming.as_deref() == Some(&info.name);
        let name = info.name.clone();
        div()
            .id(id("task"))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(p.line)
            .bg(p.card)
            .flex()
            .items_start()
            .gap(px(10.))
            .child(task_text(info, &p))
            .child({
                let edit = info.clone();
                Button::quiet(id("edit-task"), "Edit").on_click(
                    cx.listener(move |this, _, window, cx| this.open(Some(&edit), window, cx)),
                )
            })
            .child(if confirming {
                let (cancel, remove) = (name.clone(), name.clone());
                div()
                    .flex()
                    .gap(px(4.))
                    .child(
                        Button::quiet(id("cancel-remove-task"), "Cancel").on_click(cx.listener(
                            move |this, _, _, cx| {
                                if this.confirming.as_deref() == Some(&cancel) {
                                    this.confirming = None;
                                    cx.notify();
                                }
                            },
                        )),
                    )
                    .child(
                        Button::secondary(id("confirm-remove-task"), format!("Remove {name}"))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx))),
                    )
                    .into_any_element()
            } else {
                Button::quiet(id("remove-task"), "Remove")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(&name, cx)))
                    .into_any_element()
            })
            .test_support()
    }

    fn modal(&mut self, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let title = match &self.editing {
            Some(Some(_)) => "Edit task",
            _ => "New task",
        };
        let modal = Modal::new("task-modal", title)
            .width(px(600.))
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
            .gap(px(10.))
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
            .child(div().flex().flex_col().gap(px(6.)).children(switches))
            .test_support();
        let error = self.form_error.clone().map(|error| {
            div()
                .id("task-error")
                .text_size(px(12.5))
                .text_color(p.fail)
                .child(error)
                .test_support()
        });
        modal.child(body).children(error).action(if self.saving {
            div()
                .mono(11.5)
                .text_color(p.faint)
                .child("Saving…")
                .into_any_element()
        } else {
            Button::primary("save-task", "Save")
                .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                .into_any_element()
        })
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
        .gap(px(3.))
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
        let rows: Vec<_> = self
            .listed
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|info| self.row(info, cx).into_any_element())
            .collect();
        let count = rows.len();
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
                    .gap(px(8.))
                    .children(rows)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .id("no-tasks")
                                .text_size(px(12.5))
                                .text_color(p.faint)
                                .child(
                                    "No task yet: name the lines the project runs (tests, lint, a \
                                     server). Agents run them by name, and the project page has \
                                     a Run button for each.",
                                )
                                .test_support(),
                        )
                    })
                    .when_some(self.problem.clone(), |d, problem| {
                        d.child(
                            div()
                                .id("tasks-problem")
                                .text_size(px(12.5))
                                .text_color(p.fail)
                                .child(problem)
                                .test_support(),
                        )
                    }),
            )
            .child(modal)
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
    /// The run whose output is open.
    watching: Option<String>,
    /// Reads the runs again each second while one runs or its output is open.
    ticker: Option<Task<()>>,
}

impl ProjectTasks {
    pub fn new(shell: WeakEntity<Shell>, project: String, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        Self {
            shell,
            project,
            listed: None,
            problem: None,
            starting: None,
            commands: cx.new(|_| CommandList::new(this.clone())),
            panel: cx.new(|_| CommandPanel::new(this)),
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

    /// Opens a run's output in the panel, followed live.
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
            return div()
                .id("project-tasks")
                .child(self.panel.clone())
                .into_any_element();
        }
        let rows = tasks.iter().map(|info| {
            let name = info.name.clone();
            let starting = self.starting.as_deref() == Some(&info.name);
            div()
                .id(ElementId::Name(
                    format!("project-task-{}", info.name).into(),
                ))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(10.))
                .border_1()
                .border_color(p.line)
                .bg(p.card)
                .flex()
                .items_center()
                .gap(px(10.))
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
                    .gap(px(8.))
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
            .child(self.panel.clone())
            .into_any_element()
    }
}
