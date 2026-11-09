//! A project's tasks (`docs/projects.md`, "Tasks"): its named command lines. The "Tasks" section
//! of its Settings adds, edits and removes them; the project page lists them with a Run button,
//! which runs one in the project folder, and lists what the user ran there as the session's rail
//! lists the agents' commands, each opening its output, live, in the same panel. The window writes
//! nothing itself: every change and every run is the daemon's (`task.*`, `run.*`).

use crate::glyphs;
use crate::state::{AgentState, StateIcon, Status};
use std::time::Duration;

use std::collections::HashMap;

use flottille_ui::drawer::Closed;
use flottille_ui::motion::PANEL;
use flottille_ui::{
    Button, Confirm, Drawer, EmptyLine, Field, Icon, LabelledLine, ListCard, ListRow, Listing,
    Menu, Modal, Placement, RadioChoices, Section, Select, SelectOption, Switch, Toast, Type, fade,
    icon, palette, presence, space, toast,
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

use crate::actions::{Command, RunAct, RunAnswer, RunFrom};
use crate::commands::{boxed, line_field, menu_of, perform_later, problem_line};
use crate::conversation::Ending;
use crate::project::{only_at_root, repository_label};
use crate::project_settings::Reveal;
use crate::runs::{CommandList, CommandOutput, CommandPanel, CommandRow, Watch, arrange};
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
    /// When a run of it is ready, and the text, address or line that says it.
    pub ready: Option<(ReadyKind, String)>,
    /// Where a run of it answers: an address, or a line that prints it.
    pub url: Option<(AddressKind, String)>,
    /// The services of the machine it needs, started first.
    pub needs: Vec<String>,
}

/// How a task says it is ready (`docs/projects.md`, "Tasks").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadyKind {
    Line,
    Url,
    Command,
}

impl ReadyKind {
    pub const ALL: [ReadyKind; 3] = [ReadyKind::Line, ReadyKind::Url, ReadyKind::Command];

    /// Its key in the file and the protocol.
    pub fn key(self) -> &'static str {
        match self {
            ReadyKind::Line => "line",
            ReadyKind::Url => "url",
            ReadyKind::Command => "command",
        }
    }

    fn choice(self) -> (&'static str, &'static str) {
        match self {
            ReadyKind::Line => (
                "A line of its output",
                "Ready once a line it prints holds a text.",
            ),
            ReadyKind::Url => (
                "An address answers",
                "Ready once a request to it is answered.",
            ),
            ReadyKind::Command => ("A command passes", "Ready once a line exits with 0."),
        }
    }

    /// The field's label, its placeholder and its hint.
    pub(crate) fn field(self) -> (&'static str, &'static str, &'static str) {
        match self {
            ReadyKind::Line => (
                "Text in its output",
                "ready in",
                "Plain text, no pattern. For example: ready in",
            ),
            ReadyKind::Url => (
                "Address",
                "{url}/health",
                "Tried every second. For example: {url}/health, or http://localhost:5173",
            ),
            ReadyKind::Command => (
                "Command line",
                "pg_isready -h localhost",
                "Run every second in its folder, never through a shell. For example: \
                 pg_isready -h localhost",
            ),
        }
    }
}

/// How a task says where it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressKind {
    Address,
    Command,
}

impl AddressKind {
    pub const ALL: [AddressKind; 2] = [AddressKind::Address, AddressKind::Command];

    fn key(self) -> &'static str {
        match self {
            AddressKind::Address => "address",
            AddressKind::Command => "command",
        }
    }

    /// `service`: a service of the machine, which has no workspace.
    fn choice(self, service: bool) -> (&'static str, &'static str) {
        match (self, service) {
            (AddressKind::Address, false) => {
                ("An address", "The same for every run, or templated.")
            }
            (AddressKind::Address, true) => ("An address", "Where it answers."),
            (AddressKind::Command, false) => (
                "A command prints it",
                "For an address given by a tool, per workspace.",
            ),
            (AddressKind::Command, true) => {
                ("A command prints it", "For an address given by a tool.")
            }
        }
    }

    fn field(self, service: bool) -> (&'static str, &'static str, &'static str) {
        match self {
            AddressKind::Address if service => (
                "Address",
                "http://localhost:3001",
                "For example: http://localhost:3001",
            ),
            AddressKind::Address => (
                "Address",
                "https://{workspace}.app.localhost",
                "For example: https://{workspace}.app.localhost",
            ),
            AddressKind::Command => (
                "Command line",
                "portless get web",
                "Run once as it starts; its first line is the address. For example: portless \
                 get web",
            ),
        }
    }
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
            ready: ReadyKind::ALL.into_iter().find_map(|kind| {
                value["ready"][kind.key()]
                    .as_str()
                    .map(|text| (kind, text.to_owned()))
            }),
            url: match &value["url"] {
                Value::String(url) => Some((AddressKind::Address, url.clone())),
                url => url["command"]
                    .as_str()
                    .map(|line| (AddressKind::Command, line.to_owned())),
            },
            needs: names_of(&value["needs"]),
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

/// A list of names as the daemon gives it.
pub(crate) fn names_of(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect()
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
    ready: ReadyFields,
}

/// The fields of when a run is ready and where it answers, one for each kind of ready and of
/// address: a kind keeps what was typed for it. A task's form and a service's share them.
pub(crate) struct ReadyFields {
    pub ready: [Entity<InputState>; 3],
    pub url: [Entity<InputState>; 2],
}

impl ReadyFields {
    pub fn new(service: bool, window: &mut Window, cx: &mut App) -> Self {
        Self {
            ready: ReadyKind::ALL.map(|kind| line_field(kind.field().1, window, cx)),
            url: AddressKind::ALL.map(|kind| line_field(kind.field(service).1, window, cx)),
        }
    }

    pub fn inputs(&self) -> impl Iterator<Item = &Entity<InputState>> {
        self.ready.iter().chain(&self.url)
    }

    /// Fills each field with what `ready` and `url` say, and empties the others.
    pub fn fill(
        &self,
        ready: &Option<(ReadyKind, String)>,
        url: &Option<(AddressKind, String)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (at, field) in self.ready.iter().enumerate() {
            let text = match ready {
                Some((kind, text)) if ready_at(*kind) == at => text.clone(),
                _ => String::new(),
            };
            field.update(cx, |f, cx| f.set_value(text, window, cx));
        }
        for (at, field) in self.url.iter().enumerate() {
            let text = match url {
                Some((kind, text)) if url_at(*kind) == at => text.clone(),
                _ => String::new(),
            };
            field.update(cx, |f, cx| f.set_value(text, window, cx));
        }
    }

    /// `ready` and `url` as a write gives them. None is `""`: the link drops a `null`, which
    /// would leave it as it was.
    pub fn params(
        &self,
        ready: Option<ReadyKind>,
        url: Option<AddressKind>,
        cx: &App,
    ) -> (Value, Value) {
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let ready = match ready {
            Some(kind) => json!({kind.key(): value(&self.ready[ready_at(kind)])}),
            None => "".into(),
        };
        let url = match url {
            Some(AddressKind::Address) => value(&self.url[0]).into(),
            Some(AddressKind::Command) => json!({"command": value(&self.url[1])}),
            None => "".into(),
        };
        (ready, url)
    }
}

fn ready_at(kind: ReadyKind) -> usize {
    ReadyKind::ALL.iter().position(|k| *k == kind).unwrap_or(0)
}

fn url_at(kind: AddressKind) -> usize {
    AddressKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(0)
}

/// What a write was, for the toast its answer says.
#[derive(Clone, Debug, PartialEq)]
enum Writing {
    Added(String),
    Saved(String),
    Removed(String),
    /// The task and the project it goes to.
    Copied(String, String),
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
    /// The services of the machine, which a task may need, or why they cannot be read; `None`
    /// until the daemon has said.
    services: Option<Result<Vec<String>, String>>,
    saving: bool,
    writing: Option<Writing>,
    form_error: Option<SharedString>,
    confirming: Option<String>,
    /// What the confirmation names, kept while it fades away.
    asked: String,
    /// The task whose "Copy to…" dialog is open.
    copying: Option<String>,
    /// What the dialog names, kept while it fades away.
    copy_of: String,
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
            services: None,
            saving: false,
            writing: None,
            form_error: None,
            confirming: None,
            asked: String::new(),
            copying: None,
            copy_of: String::new(),
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

    /// The field of a kind of ready.
    pub fn ready_field(&self, kind: ReadyKind) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.ready.ready[ready_at(kind)])
    }

    /// The field of a kind of address.
    pub fn url_field(&self, kind: AddressKind) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.ready.url[url_at(kind)])
    }

    /// The services the form says the task needs.
    pub fn needs(&self) -> &[String] {
        &self.flags.needs
    }

    /// The services of the machine the daemon listed, which the form offers, or why it could not.
    pub fn set_services(&mut self, services: Result<Vec<String>, String>, cx: &mut Context<Self>) {
        if self.services.as_ref() != Some(&services) {
            self.services = Some(services);
            cx.notify();
        }
    }

    /// The task needs the service, or no longer does.
    pub fn toggle_need(&mut self, service: &str, cx: &mut Context<Self>) {
        toggle_name(&mut self.flags.needs, service);
        cx.notify();
    }

    /// The kind of ready the form says, none when it says nothing.
    pub fn ready_kind(&self) -> Option<ReadyKind> {
        self.flags.ready.as_ref().map(|(kind, _)| *kind)
    }

    /// Says when the task is ready, or not.
    pub fn choose_ready(&mut self, kind: Option<ReadyKind>, cx: &mut Context<Self>) {
        self.flags.ready = kind.map(|kind| (kind, String::new()));
        self.form_error = None;
        cx.notify();
    }

    /// Says where the task answers, or not.
    pub fn choose_url(&mut self, kind: Option<AddressKind>, cx: &mut Context<Self>) {
        self.flags.url = kind.map(|kind| (kind, String::new()));
        self.form_error = None;
        cx.notify();
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
            let ready = ReadyFields::new(false, window, cx);
            // Typing clears what a refusal said.
            for input in [&name, &line].into_iter().chain(ready.inputs()) {
                self._subscriptions
                    .push(cx.subscribe(input, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) && this.form_error.take().is_some() {
                            cx.notify();
                        }
                    }));
            }
            self.fields = Some(Fields { name, line, ready });
        }
        self.fields.as_ref().expect("made above")
    }

    /// "New task", or "Edit" on one: the form, filled with it.
    pub fn open(&mut self, info: Option<&TaskInfo>, window: &mut Window, cx: &mut Context<Self>) {
        self.fields(window, cx);
        let fields = self.fields.as_ref().expect("made above");
        let (name, line) = (fields.name.clone(), fields.line.clone());
        let shown = info.cloned().unwrap_or_default();
        name.update(cx, |f, cx| f.set_value(shown.name.clone(), window, cx));
        line.update(cx, |f, cx| f.set_value(shown.line.clone(), window, cx));
        fields.ready.fill(&shown.ready, &shown.url, window, cx);
        // The services a task may need: the file is the user's to edit by hand too.
        perform_later(&self.shell, Command::Services, cx);
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
        let url = self.flags.url.as_ref().map(|(kind, _)| *kind);
        (params["ready"], params["url"]) = fields.ready.params(self.ready_kind(), url, cx);
        params["needs"] = json!(self.flags.needs);
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

    /// Opens the dialog that copies a task to another project.
    pub fn copy(&mut self, name: &str, cx: &mut Context<Self>) {
        self.copying = Some(name.to_owned());
        self.copy_of = name.to_owned();
        cx.notify();
    }

    /// Whether the "Copy to…" dialog is open.
    pub fn is_copying(&self) -> bool {
        self.copying.is_some()
    }

    /// Copies the task of the open dialog to the project `to`.
    pub fn copy_to(&mut self, to: &str, cx: &mut Context<Self>) {
        let Some(name) = self.copying.clone() else {
            return;
        };
        if self.saving {
            return;
        }
        let params = json!({"project": self.project, "name": name, "to": to});
        self.write(
            "task.copy",
            params,
            Writing::Copied(name, to.to_owned()),
            cx,
        );
    }

    /// The daemon's answer to a write: a toast says it, and the list is read again; a refusal of
    /// the dialog's stays in it.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        let writing = self.writing.take();
        if let Some(Writing::Copied(name, to)) = &writing {
            self.copying = None;
            toast(
                match refused {
                    None => Toast::success(format!("Task {name} copied to {to}")),
                    Some(why) => {
                        Toast::failure(format!("Task {name} was not copied to {to}")).detail(why)
                    }
                },
                cx,
            );
            cx.notify();
            return;
        }
        match refused {
            None => {
                self.editing = None;
                self.form_error = None;
                let said = match writing {
                    Some(Writing::Added(name)) => Toast::success(format!("Task {name} added"))
                        .detail("Agents run it by its name; the project page has a Run for it."),
                    Some(Writing::Saved(name)) => Toast::success(format!("Task {name} saved")),
                    Some(Writing::Removed(name)) => Toast::success(format!("Task {name} removed")),
                    // A copy is answered above.
                    Some(Writing::Copied(..)) | None => Toast::success("Tasks saved"),
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

    /// Run in a task's "⋯": the task starts in the project folder, a toast says so.
    pub fn run(&self, name: &str, cx: &mut Context<Self>) {
        let command = Command::RunTask {
            project: self.project.clone(),
            name: name.to_owned(),
            from: RunFrom::Settings,
        };
        perform_later(&self.shell, command, cx);
    }

    fn menu(&mut self, name: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let this = cx.entity().downgrade();
        menu_of(&mut self.menus, "task", name, cx, move |name, menu| {
            let (run, edit, copy, remove) =
                (this.clone(), this.clone(), this.clone(), this.clone());
            let (to_run, to_edit, to_copy, to_remove) = (
                name.to_owned(),
                name.to_owned(),
                name.to_owned(),
                name.to_owned(),
            );
            menu.entry(
                format!("menu-run-task-{name}"),
                "Run",
                Some(Icon::Play),
                move |_, cx| {
                    run.update(cx, |this, cx| this.run(&to_run, cx)).ok();
                },
            )
            .entry(
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
            .entry(
                format!("copy-task-{name}"),
                "Copy to…",
                None,
                move |_, cx| {
                    copy.update(cx, |this, cx| this.copy(&to_copy, cx)).ok();
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
        // The folder is the only repository: a task "in" it runs where every task runs.
        let alone = only_at_root(&self.repositories);
        row = row.detail(SharedString::from(match &info.repository {
            Some(repository) if !alone => {
                format!("in {}", repository_label(&self.project, repository))
            }
            _ => "in each place's folder".to_owned(),
        }));
        if !info.needs.is_empty() {
            row = row.below(needs_line(
                ElementId::Name(format!("task-needs-{}", info.name).into()),
                &info.needs,
            ));
        }
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

    /// The dialog that picks the project a task is copied to: the others, a click on one copies.
    fn copy_modal(&self, cx: &mut Context<Self>) -> Modal {
        let this = cx.entity().downgrade();
        let modal = Modal::new("task-copy-modal", format!("Copy the task {}", self.copy_of))
            .sentence("Pick the project that gets it, with the same line and flags.")
            .open(self.copying.is_some())
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.copying = None;
                    cx.notify();
                })
                .ok();
            });
        let others: Vec<(String, String)> = self
            .shell
            .upgrade()
            .map(|shell| {
                shell
                    .read(cx)
                    .snapshot()
                    .projects
                    .iter()
                    .filter(|project| project.name != self.project)
                    .map(|project| (project.name.clone(), project.folder.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let empty = others.is_empty();
        let list =
            ListCard::new("task-copy-list").rows(others.into_iter().map(|(name, folder)| {
                let to = name.clone();
                ListRow::new(
                    ElementId::Name(format!("copy-task-to-{name}").into()),
                    SharedString::from(name),
                )
                .line(SharedString::from(folder))
                .mono_line()
                .on_click(cx.listener(move |this, _, _, cx| this.copy_to(&to, cx)))
            }));
        modal.child(
            div()
                .flex()
                .flex_col()
                .child(presence("task-copy-list-block", !empty, list))
                .child(presence(
                    "task-copy-none",
                    empty,
                    EmptyLine::new(
                        "task-copy-none-line",
                        Icon::Folder,
                        "No other project yet: add one to copy tasks to it.",
                    ),
                )),
        )
    }

    fn modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Modal {
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
        // The folder is the only repository: the task runs in the place's folder, no choice.
        let asked = fade(
            "task-place-asked",
            !only_at_root(&self.repositories),
            PANEL,
            window,
            cx,
        );
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
                    .child(
                        div().flex_1().min_w_0().child(
                            Field::new("task-name-field", "Name", name)
                                .hint("Agents run it by this name."),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex_grow(asked)
                            .overflow_hidden()
                            .ml(space::MD * asked)
                            .opacity(asked)
                            .when(asked > 0., |d| {
                                d.child(
                                    Field::new("task-place-field", "Runs in", place)
                                        .hint("Or one repository of the place."),
                                )
                            }),
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
            .child(needs_field(
                "task",
                self.services
                    .as_ref()
                    .map(|listed| listed.as_deref().map_err(String::as_str)),
                None,
                &self.flags.needs,
                |this: &mut Self, name, cx| this.toggle_need(name, cx),
                cx,
            ))
            .child(ready_and_url(
                "task",
                false,
                &fields.ready,
                self.ready_kind(),
                self.flags.url.as_ref().map(|(kind, _)| *kind),
                |this: &mut Self, kind, cx| this.choose_ready(kind, cx),
                |this: &mut Self, kind, cx| this.choose_url(kind, cx),
                cx,
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

/// Adds `name` to `names`, or takes it out when it is there.
pub(crate) fn toggle_name(names: &mut Vec<String>, name: &str) {
    match names.iter().position(|n| n == name) {
        Some(at) => {
            names.remove(at);
        }
        None => names.push(name.to_owned()),
    }
}

/// What a task or a service needs, as a row says it under its line: `Needs mongo, redis`.
pub(crate) fn needs_line(id: ElementId, needs: &[String]) -> LabelledLine {
    let said = needs.join(", ");
    LabelledLine::new(id, Icon::Service, "Needs", SharedString::from(said.clone()))
        .child(SharedString::from(said))
}

/// The services a task or a service needs, chosen from those of the machine (`<prefix>-need-<name>`
/// switches each); `own` is a service's own name, never offered to itself. `offered` is `None`
/// until the daemon has listed them, and why it could not when it failed.
pub(crate) fn needs_field<V: 'static>(
    prefix: &'static str,
    offered: Option<Result<&[String], &str>>,
    own: Option<&str>,
    chosen: &[String],
    toggle: fn(&mut V, &str, &mut Context<V>),
    cx: &mut Context<V>,
) -> AnyElement {
    let p = palette(cx);
    let offered_or_why = offered;
    let offered: Vec<String> = offered
        .and_then(Result::ok)
        .unwrap_or_default()
        .iter()
        .filter(|name| Some(name.as_str()) != own)
        .cloned()
        .collect();
    let switches = offered.iter().map(|name| {
        let toggled = name.clone();
        Switch::new(
            ElementId::Name(format!("{prefix}-need-{name}").into()),
            chosen.contains(name),
            SharedString::from(name.clone()),
        )
        .on_click(cx.listener(move |this, _, _, cx| toggle(this, &toggled, cx)))
    });
    let none = offered.is_empty().then(|| {
        let (said, ink) = match offered_or_why {
            None => ("Reading the services of the machine…".to_owned(), p.faint),
            Some(Err(why)) => (format!("The services cannot be read: {why}"), p.fail),
            Some(Ok(_)) => (
                "No service to need: the Services page adds them.".to_owned(),
                p.faint,
            ),
        };
        let said = SharedString::from(said);
        div()
            .id(ElementId::Name(format!("{prefix}-no-needs").into()))
            .aria_label(said.clone())
            .text_size(px(12.5))
            .text_color(ink)
            .child(said)
            .test_support()
    });
    Field::new(
        ElementId::Name(format!("{prefix}-needs-field").into()),
        "Needs",
        div()
            .flex()
            .flex_col()
            .gap(space::MD)
            .children(switches)
            .children(none),
    )
    .optional()
    .hint("The services of the machine it needs: started first, and waited for until ready.")
    .into_any_element()
}

/// When a run is ready, and where it answers: each a choice, and under it the field the choice
/// needs, which unfolds with it. A task's form (`prefix` `task`) and a service's draw it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ready_and_url<V: 'static>(
    prefix: &'static str,
    service: bool,
    fields: &ReadyFields,
    ready_kind: Option<ReadyKind>,
    url_kind: Option<AddressKind>,
    choose_ready: fn(&mut V, Option<ReadyKind>, &mut Context<V>),
    choose_url: fn(&mut V, Option<AddressKind>, &mut Context<V>),
    cx: &mut Context<V>,
) -> AnyElement {
    let this = cx.entity().downgrade();
    let mut ready = RadioChoices::new(format!("{prefix}-ready"), "Ready when").choice(
        "none",
        "Not said",
        "A run of it is running, and that is all.",
    );
    for kind in ReadyKind::ALL {
        let (title, line) = kind.choice();
        ready = ready.choice(kind.key(), title, line);
    }
    let ready = ready
        .chosen(ready_kind.map_or(0, |kind| ready_at(kind) + 1))
        .on_choose(move |index, _, cx| {
            let kind = index.checked_sub(1).map(|at| ReadyKind::ALL[at]);
            this.update(cx, |this, cx| choose_ready(this, kind, cx))
                .ok();
        });
    let this = cx.entity().downgrade();
    let mut url = RadioChoices::new(format!("{prefix}-url"), "Its address").choice(
        "none",
        "None",
        "It serves nothing to open.",
    );
    for kind in AddressKind::ALL {
        let (title, line) = kind.choice(service);
        url = url.choice(kind.key(), title, line);
    }
    let url = url
        .chosen(url_kind.map_or(0, |kind| url_at(kind) + 1))
        .on_choose(move |index, _, cx| {
            let kind = index.checked_sub(1).map(|at| AddressKind::ALL[at]);
            this.update(cx, |this, cx| choose_url(this, kind, cx)).ok();
        });
    // `<prefix>-<choice>-<kind>` is the block and its field; the box is `<id>-input`.
    let block = |id: String, shown: bool, field: (&str, &str, &str), input| {
        let (label, _, hint) = field;
        presence(
            ElementId::Name(format!("{id}-block").into()),
            shown,
            Field::new(
                ElementId::Name(format!("{id}-text").into()),
                label.to_owned(),
                boxed(
                    ElementId::Name(format!("{id}-input").into()),
                    Input::new(input),
                    cx,
                ),
            )
            .hint(hint.to_owned()),
        )
        .gap(space::MD)
    };
    let ready_blocks: Vec<_> = ReadyKind::ALL
        .into_iter()
        .map(|kind| {
            block(
                format!("{prefix}-ready-{}", kind.key()),
                ready_kind == Some(kind),
                kind.field(),
                &fields.ready[ready_at(kind)],
            )
        })
        .collect();
    let url_blocks: Vec<_> = AddressKind::ALL
        .into_iter()
        .map(|kind| {
            block(
                format!("{prefix}-url-{}", kind.key()),
                url_kind == Some(kind),
                kind.field(service),
                &fields.url[url_at(kind)],
            )
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(space::LG)
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    Field::new(
                        ElementId::Name(format!("{prefix}-ready-field").into()),
                        "Ready when",
                        ready,
                    )
                    .optional(),
                )
                .children(ready_blocks),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    Field::new(
                        ElementId::Name(format!("{prefix}-url-field").into()),
                        "Its address",
                        url,
                    )
                    .optional()
                    .hint("Shown on its run, a link that opens it."),
                )
                .children(url_blocks),
        )
        .into_any_element()
}

impl Render for TasksSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let modal = self.modal(window, cx);
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
            .child(self.copy_modal(cx))
            .child(self.confirm(cx))
    }
}

// ── The project page ───────────────────────────────────────────────────────────────────────────

/// The toast of a task the user ran from the settings (`place` `None`) or a session's rail (its
/// workspace, `Some(None)` for `main`): where it runs and where its output is, or why it did not
/// start.
pub fn ran(name: &str, place: Option<Option<&str>>, refused: Option<String>, cx: &mut App) {
    let detail = match place {
        None => "In the project folder. Its output is under Runs on the project page.".to_owned(),
        Some(None) => "In the project folder. Its output is under Runs.".to_owned(),
        Some(Some(workspace)) => format!("In workspace {workspace}. Its output is under Runs."),
    };
    toast(
        match refused {
            None => Toast::success(format!("{name} started")).detail(detail),
            Some(why) => Toast::failure(format!("{name} did not start")).detail(why),
        },
        cx,
    );
}

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
    /// The drawer a run's detail slides in with, drawn by the page over all of it.
    drawer: Entity<Drawer>,
    _drawer: Subscription,
    /// The run, or the setup step, whose output is open.
    watching: Option<Watch>,
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
        let panel = cx.new(|_| CommandPanel::new(this));
        let drawer = cx.new(|cx| Drawer::detail("run-drawer", panel.clone(), cx));
        let _drawer = cx.subscribe(&drawer, |this, _, _: &Closed, cx| this.close_command(cx));
        Self {
            shell,
            project,
            listed: None,
            problem: None,
            starting: None,
            commands,
            panel,
            runs: Listing::default(),
            marks: HashMap::new(),
            rows: Listing::default(),
            drawer,
            _drawer,
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

    /// Opens a run's address with the system's default handler.
    pub fn open_link(&self, url: &str, cx: &App) {
        if let Some(shell) = self.shell.upgrade() {
            shell.read(cx).open_link(url);
        }
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
            from: RunFrom::Page,
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

    /// Opens a run's detail in the drawer, its output followed live.
    pub fn open_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.open_watch(Watch::Run(run.to_owned()), cx);
    }

    /// Opens a step of a workspace's setup in the same drawer: what it runs and what it writes.
    pub fn open_step(&mut self, workspace: &str, position: u32, cx: &mut Context<Self>) {
        self.open_watch(
            Watch::Step {
                workspace: workspace.to_owned(),
                position,
            },
            cx,
        );
    }

    /// The new run of a restart replaces the open command, its row and its output staying drawn
    /// until its own arrive.
    fn follow_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.open_watch_as(Watch::Run(run.to_owned()), true, cx);
    }

    fn open_watch(&mut self, watch: Watch, cx: &mut Context<Self>) {
        self.open_watch_as(watch, false, cx);
    }

    fn open_watch_as(&mut self, watch: Watch, in_place: bool, cx: &mut Context<Self>) {
        let key = watch.key();
        self.watching = Some(watch);
        let rows = self.commands.read(cx).rows().to_vec();
        self.panel.update(cx, |panel, cx| {
            if in_place {
                panel.follow(&key, cx);
            } else {
                panel.open(&key, cx);
            }
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
        self.answer_run(run, RunAct::Stop, cx);
    }

    /// The user runs a command again, in the same place: the panel shows the new run once the
    /// daemon answers.
    pub fn restart_command(&mut self, run: &str, cx: &mut Context<Self>) {
        self.answer_run(run, RunAct::Restart, cx);
    }

    fn answer_run(&mut self, run: &str, act: RunAct, cx: &mut Context<Self>) {
        let command = Command::ProjectRun {
            project: self.project.clone(),
            run: run.to_owned(),
            act,
        };
        perform_later(&self.shell, command, cx);
    }

    /// What the daemon answered to a stop, a restart or a dismissal: a refusal is a toast, and the
    /// panel shows the new run of a restart.
    pub fn run_answered(&mut self, answer: RunAnswer, cx: &mut Context<Self>) {
        let RunAnswer {
            act,
            run,
            refused,
            restarted,
            notice,
        } = answer;
        if let Some(why) = refused {
            let title = match act {
                RunAct::Restart => "The run was not restarted",
                _ => "The run was not stopped",
            };
            toast(Toast::failure(title).detail(why), cx);
            if act == RunAct::Restart {
                self.panel
                    .update(cx, |panel, cx| panel.restart_refused(&run, cx));
            }
        }
        if let Some(run) = restarted {
            self.follow_command(&run, cx);
        }
        if let Some(notice) = notice {
            toast(Toast::success("The run is now yours").detail(notice), cx);
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

    fn runs_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows = self.commands.read(cx).rows().to_vec();
        let arranged = arrange(&rows);
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
            let mark = match row.readiness() {
                Some(ready) => crate::runs::ready_mark(
                    ElementId::Name(format!("project-run-mark-{}", row.run).into()),
                    ElementId::Name(format!("project-run-dot-{}", row.run).into()),
                    ready == "ready",
                    Some(mark.into_any_element()),
                    &p,
                    window,
                    cx,
                ),
                None => mark.into_any_element(),
            };
            let who = match row.agent.as_str() {
                "" => "by you".to_owned(),
                agent => format!("by agent {agent}"),
            };
            let time = seconds(row.elapsed_ms(now));
            let detail = match (&row.name, row.readiness()) {
                // The dot says it is ready.
                (Some(task), Some("ready")) => format!("task {task} · {time}"),
                (Some(task), Some(ready)) => format!("task {task} · {ready} · {time}"),
                (Some(task), None) => format!("task {task} · {who} · {time}"),
                (None, _) => format!("{who} · {time}"),
            };
            let page = cx.entity().downgrade();
            let open = row.run.clone();
            let stop = row.running().then(|| {
                let (page, run) = (page.clone(), row.run.clone());
                Button::quiet(
                    ElementId::Name(format!("command-stop-{}", row.run).into()),
                    "Stop",
                )
                .leading(glyphs::STOP)
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
            // A server's address takes the place of its last line, which its output still shows.
            if let Some(url) = row.address() {
                let page = cx.entity().downgrade();
                list_row = list_row.below(crate::runs::address_link(
                    ElementId::Name(format!("project-run-link-{}", row.run).into()),
                    url,
                    11.5,
                    move |url, cx| {
                        if let Some(page) = page.upgrade() {
                            page.read(cx).open_link(url, cx);
                        }
                    },
                    window,
                    cx,
                ));
            } else if !last.is_empty() {
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let runs = self.runs_section(window, cx);
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
