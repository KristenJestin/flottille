//! The services of the machine (`docs/projects.md`, "Services"): the page that lists them with
//! their state, starts and stops them, shows a service's output in the run drawer, and adds,
//! edits and removes them in a form with a task's fields. The window writes nothing itself:
//! every change, start and stop is the daemon's (`service.*`).

use crate::glyphs;
use crate::state::{AgentState, StateIcon, Status};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_design_system::drawer::Closed;
use gpui_design_system::motion::HOVER;
use gpui_design_system::{
    Button, Confirm, Drawer, EmptyLine, Field, Icon, LabelledLine, ListCard, ListRow, Listing,
    Menu, Modal, PageHeader, Placement, Section, Switch, Toast, Type, enter, fade, palette,
    presence, presence_of, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, WeakEntity,
    Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{boxed, line_field, menu_of, perform_later, problem_line};
use crate::runs::{CommandOutput, CommandPanel, address_link, ready_mark};
use crate::shell::Shell;
use crate::tasks::{
    AddressKind, ReadyFields, ReadyKind, TaskInfo, names_of, needs_field, needs_line,
    ready_and_url, toggle_name,
};

/// How often the open output is read again.
const OUTPUT_EVERY: Duration = Duration::from_secs(1);

/// A service as the daemon lists it (`docs/protocol.md`, "The service methods").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServiceInfo {
    pub name: String,
    /// Where it runs, absolute: the home when the file names none.
    pub folder: Option<String>,
    pub line: String,
    pub needs: Vec<String>,
    pub ready: Option<(ReadyKind, String)>,
    pub url: Option<(AddressKind, String)>,
    pub ask_before_running: bool,
    /// `stopped`, `starting`, `ready`, `running`, `outside` or `failed`.
    pub state: String,
    /// The line that says why it failed.
    pub why: Option<String>,
    /// Its run, going on or the last one.
    pub run: Option<String>,
    /// Its run's address, or the file's.
    pub address: Option<String>,
    /// What it needs that does not run.
    pub needs_stopped: Vec<String>,
    /// The projects whose tasks need it, each with those tasks.
    pub needed_by: Vec<(String, Vec<String>)>,
}

impl ServiceInfo {
    pub fn of(value: &Value) -> Self {
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        // Its `ready` and `url` are a task's.
        let task = TaskInfo::of(value);
        Self {
            name: text("name").unwrap_or_default(),
            folder: text("folder"),
            line: text("line").unwrap_or_default(),
            needs: names_of(&value["needs"]),
            ready: task.ready,
            url: task.url,
            ask_before_running: value["ask_before_running"] == true,
            state: text("state").unwrap_or_default(),
            why: text("why"),
            run: text("run"),
            address: text("address"),
            needs_stopped: names_of(&value["needs_stopped"]),
            needed_by: value["needed_by"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|needed| {
                    let project = needed["project"].as_str().unwrap_or_default().to_owned();
                    (project, names_of(&needed["tasks"]))
                })
                .collect(),
        }
    }

    /// Whether Flottille runs it now: it can be stopped.
    pub fn runs(&self) -> bool {
        matches!(self.state.as_str(), "starting" | "ready" | "running")
    }

    /// Its state in words, as its row says it.
    pub fn state_said(&self) -> &str {
        match self.state.as_str() {
            "outside" => "running outside Flottille",
            state => state,
        }
    }

    /// The projects whose tasks need it: `app (api, web); site (dev)`.
    pub fn needed_said(&self) -> String {
        self.needed_by
            .iter()
            .map(|(project, tasks)| format!("{project} ({})", tasks.join(", ")))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The mark its state is drawn with: the spinner while it starts, under the solid dot of
    /// one that runs.
    fn mark(&self) -> AgentState {
        match self.state.as_str() {
            "starting" | "ready" | "running" => AgentState::Running,
            "outside" => AgentState::Completed,
            "failed" => AgentState::Failed,
            _ => AgentState::Stopped,
        }
    }
}

/// What a write was, for the toast its answer says.
#[derive(Clone, Debug, PartialEq)]
enum Writing {
    Added(String),
    Saved(String),
    Removed(String),
}

// ── The form ───────────────────────────────────────────────────────────────────────────────────

struct Fields {
    name: Entity<InputState>,
    line: Entity<InputState>,
    folder: Entity<InputState>,
    ready: ReadyFields,
}

/// The dialog that adds a service or edits one: a task's fields, with its folder and what it
/// needs. In its own view, as its text fields redraw on each blink of their caret.
pub struct ServiceForm {
    page: WeakEntity<ServicesPage>,
    /// Open on a new service (`Some(None)`) or the one named.
    editing: Option<Option<String>>,
    /// The flags, the needs and the kinds of ready and address the form shows.
    shown: ServiceInfo,
    fields: Option<Fields>,
    /// The other services, which it may need.
    services: Vec<String>,
    saving: bool,
    form_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl ServiceForm {
    fn new(page: WeakEntity<ServicesPage>) -> Self {
        Self {
            page,
            editing: None,
            shown: ServiceInfo::default(),
            fields: None,
            services: Vec::new(),
            saving: false,
            form_error: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.editing.is_some()
    }

    pub fn form_error(&self) -> Option<&str> {
        self.form_error.as_deref()
    }

    pub fn name_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.name)
    }

    pub fn line_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.line)
    }

    pub fn folder_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.folder)
    }

    pub fn ready_field(&self, kind: ReadyKind) -> Option<&Entity<InputState>> {
        let at = ReadyKind::ALL.iter().position(|k| *k == kind)?;
        self.fields.as_ref().map(|f| &f.ready.ready[at])
    }

    pub fn url_field(&self, kind: AddressKind) -> Option<&Entity<InputState>> {
        let at = AddressKind::ALL.iter().position(|k| *k == kind)?;
        self.fields.as_ref().map(|f| &f.ready.url[at])
    }

    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        if self.fields.is_none() {
            let name = line_field("mongo", window, cx);
            let line = line_field("docker compose up", window, cx);
            let folder = line_field("~/tools/docker/mongo", window, cx);
            let ready = ReadyFields::new(true, window, cx);
            // Typing clears what a refusal said.
            for input in [&name, &line, &folder].into_iter().chain(ready.inputs()) {
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
                folder,
                ready,
            });
        }
        self.fields.as_ref().expect("made above")
    }

    /// "New service", or Edit on one: the form, filled with it; `services` are the machine's.
    fn open(
        &mut self,
        info: Option<&ServiceInfo>,
        services: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fields(window, cx);
        let fields = self.fields.as_ref().expect("made above");
        let shown = info.cloned().unwrap_or_default();
        for (field, text) in [
            (&fields.name, &shown.name),
            (&fields.line, &shown.line),
            (
                &fields.folder,
                shown.folder.as_ref().unwrap_or(&String::new()),
            ),
        ] {
            let text = text.clone();
            field.update(cx, |f, cx| f.set_value(text, window, cx));
        }
        fields.ready.fill(&shown.ready, &shown.url, window, cx);
        let focus = if info.is_some() {
            &fields.line
        } else {
            &fields.name
        }
        .read(cx)
        .focus_handle(cx);
        window.focus(&focus, cx);
        self.services = services;
        self.editing = Some(info.map(|info| info.name.clone()));
        self.shown = shown;
        self.form_error = None;
        self.saving = false;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.form_error = None;
        cx.notify();
    }

    fn toggle_ask(&mut self, cx: &mut Context<Self>) {
        self.shown.ask_before_running = !self.shown.ask_before_running;
        cx.notify();
    }

    fn toggle_need(&mut self, service: &str, cx: &mut Context<Self>) {
        toggle_name(&mut self.shown.needs, service);
        cx.notify();
    }

    fn choose_ready(&mut self, kind: Option<ReadyKind>, cx: &mut Context<Self>) {
        self.shown.ready = kind.map(|kind| (kind, String::new()));
        self.form_error = None;
        cx.notify();
    }

    fn choose_url(&mut self, kind: Option<AddressKind>, cx: &mut Context<Self>) {
        self.shown.url = kind.map(|kind| (kind, String::new()));
        self.form_error = None;
        cx.notify();
    }

    /// Saves the form: a new service, or the one being edited.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let (Some(original), Some(fields)) = (self.editing.clone(), self.fields.as_ref()) else {
            return;
        };
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let name = original.clone().unwrap_or_else(|| value(&fields.name));
        let ready = self.shown.ready.as_ref().map(|(kind, _)| *kind);
        let url = self.shown.url.as_ref().map(|(kind, _)| *kind);
        let (ready, url) = fields.ready.params(ready, url, cx);
        let mut params = json!({
            "name": name,
            "line": value(&fields.line),
            "needs": self.shown.needs,
            "ready": ready,
            "url": url,
            "ask_before_running": self.shown.ask_before_running,
        });
        // The daemon gives the folder resolved (`~` expanded, the home for none): sent back as it
        // is, it would rewrite the file's `~/…` or give a folder to one that had none. It is sent
        // only when the user changed it; `""` puts it back in the home.
        let folder = value(&fields.folder);
        if original.is_none() || Some(&folder) != self.shown.folder.as_ref() {
            params["folder"] = folder.into();
        }
        let (method, writing) = match original {
            Some(_) => ("service.edit", Writing::Saved(name)),
            None => ("service.add", Writing::Added(name)),
        };
        self.saving = true;
        self.form_error = None;
        self.page
            .update(cx, |page, cx| page.write(method, params, writing, cx))
            .ok();
        cx.notify();
    }

    /// The daemon's answer to the form's write: it closes, or says why under its fields.
    fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        match refused {
            None => self.editing = None,
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }
}

impl Render for ServiceForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let title = match &self.editing {
            Some(Some(name)) => format!("Service {name}"),
            _ => "New service".to_owned(),
        };
        let modal = Modal::new("service-modal", title)
            .sentence(
                "A program of the machine every project shares: run once, started by the tasks \
                 that need it. Never through a shell.",
            )
            .open(self.editing.is_some())
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let (Some(original), Some(fields)) = (self.editing.clone(), self.fields.as_ref()) else {
            return modal;
        };
        let p = palette(cx);
        let name = match &original {
            Some(name) => div()
                .id("service-name")
                .h(px(32.))
                .flex()
                .items_center()
                .mono(12.5)
                .text_color(p.mute)
                .child(SharedString::from(name.clone()))
                .test_support()
                .into_any_element(),
            None => boxed("service-name", Input::new(&fields.name), cx).into_any_element(),
        };
        let ask = Switch::new(
            "service-flag-ask_before_running",
            self.shown.ask_before_running,
            "Ask before each start",
        )
        .hint("Each start by an agent waits for you, or for the judge in Auto.")
        .on_click(cx.listener(|this, _, _, cx| this.toggle_ask(cx)));
        let body = div()
            .id("service-form")
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(
                Field::new("service-name-field", "Name", name)
                    .hint("Tasks and agents name it by this name."),
            )
            .child(Field::new(
                "service-line-field",
                "Command line",
                boxed("service-line", Input::new(&fields.line), cx),
            ))
            .child(
                Field::new(
                    "service-folder-field",
                    "Folder",
                    boxed("service-folder", Input::new(&fields.folder), cx),
                )
                .optional()
                .hint("Where it runs: absolute, or ~/…. The home when left empty."),
            )
            .child(needs_field(
                "service",
                Some(Ok(&self.services)),
                original.as_deref().or(Some(self.shown.name.as_str())),
                &self.shown.needs,
                |this: &mut Self, name, cx| this.toggle_need(name, cx),
                cx,
            ))
            .child(ready_and_url(
                "service",
                true,
                &fields.ready,
                self.shown.ready.as_ref().map(|(kind, _)| *kind),
                self.shown.url.as_ref().map(|(kind, _)| *kind),
                |this: &mut Self, kind, cx| this.choose_ready(kind, cx),
                |this: &mut Self, kind, cx| this.choose_url(kind, cx),
                cx,
            ))
            .child(Field::new("service-kind", "When an agent starts it", ask))
            .child(problem_line(
                "service-error",
                self.form_error.clone(),
                space::LG,
                cx,
            ))
            .test_support();
        modal.child(body).action(
            Button::primary(
                "save-service",
                if original.is_some() { "Save" } else { "Add" },
            )
            .disabled(self.saving)
            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
        )
    }
}

// ── The page ───────────────────────────────────────────────────────────────────────────────────

/// The services page, reached from the top bar: the machine's, not a project's.
pub struct ServicesPage {
    shell: WeakEntity<Shell>,
    listed: Option<Vec<ServiceInfo>>,
    /// Why the list cannot be read.
    problem: Option<SharedString>,
    /// The rows as drawn: a removed service folds away before it is gone.
    rows: Listing<ServiceInfo>,
    menus: HashMap<String, Entity<Menu>>,
    /// Each service's mark, kept while it is listed: it turns as its state changes.
    marks: HashMap<String, (Entity<Status>, Entity<StateIcon>)>,
    form: Entity<ServiceForm>,
    writing: Option<Writing>,
    /// The service whose removal is asked.
    confirming: Option<String>,
    /// What the confirmation names, kept while it fades away.
    asked: String,
    /// The services being started or stopped: their buttons wait.
    acting: HashSet<String>,
    panel: Entity<CommandPanel>,
    /// The drawer a service's output slides in with, over the page.
    drawer: Entity<Drawer>,
    _drawer: Subscription,
    /// The run whose output is open.
    watching: Option<String>,
    /// Reads the open output again each second.
    ticker: Option<Task<()>>,
    narrow: bool,
    scroll: ScrollHandle,
}

impl ServicesPage {
    pub fn new(shell: WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let form = cx.new(|_| ServiceForm::new(this.clone()));
        let panel = cx.new(|_| CommandPanel::new(this));
        let drawer = cx.new(|cx| Drawer::detail("service-drawer", panel.clone(), cx));
        let _drawer = cx.subscribe(&drawer, |this, _, _: &Closed, cx| this.close_output(cx));
        Self {
            shell,
            listed: None,
            problem: None,
            rows: Listing::default(),
            menus: HashMap::new(),
            marks: HashMap::new(),
            form,
            writing: None,
            confirming: None,
            asked: String::new(),
            acting: HashSet::new(),
            panel,
            drawer,
            _drawer,
            watching: None,
            ticker: None,
            narrow: false,
            scroll: ScrollHandle::new(),
        }
    }

    /// The services as last listed; `None` until the daemon has said.
    pub fn services(&self) -> Option<&[ServiceInfo]> {
        self.listed.as_deref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn form(&self) -> &Entity<ServiceForm> {
        &self.form
    }

    pub fn panel(&self) -> &Entity<CommandPanel> {
        &self.panel
    }

    pub fn is_confirming(&self) -> bool {
        self.confirming.is_some()
    }

    /// Why the service whose removal is asked cannot be removed: the tasks and the services
    /// that need it. The confirmation says it, and its button is disabled then.
    pub fn removal_refused(&self) -> Option<String> {
        let name = &self.asked;
        let listed = self.listed.as_deref().unwrap_or_default();
        let mut needing: Vec<String> = Vec::new();
        if let Some(info) = listed.iter().find(|s| &s.name == name)
            && !info.needed_by.is_empty()
        {
            needing.push(format!("the tasks of {}", info.needed_said()));
        }
        let services: Vec<&str> = listed
            .iter()
            .filter(|s| s.needs.contains(name))
            .map(|s| s.name.as_str())
            .collect();
        if !services.is_empty() {
            needing.push(format!("the services {}", services.join(", ")));
        }
        (!needing.is_empty()).then(|| {
            format!(
                "{} need it: change them first.",
                capitalized(&needing.join(" and "))
            )
        })
    }

    /// Whether the open output is read again each second.
    pub fn follows_output(&self) -> bool {
        self.ticker.is_some()
    }

    pub fn set_narrow(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if self.narrow != narrow {
            self.narrow = narrow;
            cx.notify();
        }
    }

    /// Opened afresh: its rows enter again, one after another, and the list is read.
    pub fn opened(&mut self, cx: &mut Context<Self>) {
        self.rows = Listing::default();
        self.read(cx);
        cx.notify();
    }

    /// Asks the daemon for the list.
    pub fn read(&self, cx: &mut App) {
        perform_later(&self.shell, Command::Services, cx);
    }

    /// The daemon's list.
    pub fn listed(&mut self, listed: Result<Vec<ServiceInfo>, String>, cx: &mut Context<Self>) {
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

    /// Opens the form on a new service, or on one.
    pub fn edit(&mut self, name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let listed = self.listed.clone().unwrap_or_default();
        let info = name.and_then(|name| listed.iter().find(|s| s.name == name));
        let names = listed.iter().map(|s| s.name.clone()).collect();
        self.form
            .update(cx, |form, cx| form.open(info, names, window, cx));
    }

    fn write(&mut self, method: &'static str, params: Value, writing: Writing, cx: &mut App) {
        self.writing = Some(writing);
        perform_later(&self.shell, Command::ServiceWrite { method, params }, cx);
    }

    /// The daemon's answer to a write: a toast says it, and the list is read again; a refusal of
    /// the form's stays in it.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        let writing = self.writing.take();
        if matches!(writing, Some(Writing::Added(_) | Writing::Saved(_))) {
            let refused = refused.clone();
            self.form.update(cx, |form, cx| form.written(refused, cx));
        }
        match (refused, writing) {
            (None, writing) => {
                let said = match writing {
                    Some(Writing::Added(name)) => Toast::success(format!("Service {name} added"))
                        .detail("A task that needs it starts it; Start runs it now."),
                    Some(Writing::Saved(name)) => Toast::success(format!("Service {name} saved"))
                        .detail("A change reaches its next start."),
                    Some(Writing::Removed(name)) => {
                        Toast::success(format!("Service {name} removed"))
                    }
                    None => Toast::success("Services saved"),
                };
                toast(said, cx);
                self.read(cx);
            }
            (Some(why), Some(Writing::Removed(name))) => toast(
                Toast::failure(format!("The service {name} was not removed")).detail(why),
                cx,
            ),
            (Some(_), _) => {}
        }
        cx.notify();
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
        let writing = Writing::Removed(name.to_owned());
        self.write("service.remove", json!({"name": name}), writing, cx);
        cx.notify();
    }

    /// Starts the service, and what it needs first.
    pub fn start(&mut self, name: &str, cx: &mut Context<Self>) {
        self.act(name, true, cx);
    }

    /// Stops the service. It asks nothing: it is the user who clicks.
    pub fn stop(&mut self, name: &str, cx: &mut Context<Self>) {
        self.act(name, false, cx);
    }

    fn act(&mut self, name: &str, start: bool, cx: &mut Context<Self>) {
        if !self.acting.insert(name.to_owned()) {
            return;
        }
        let name = name.to_owned();
        perform_later(&self.shell, Command::ServiceAct { name, start }, cx);
        cx.notify();
    }

    /// The daemon started or stopped the service, or refused with why: a toast says it.
    pub fn acted(
        &mut self,
        name: &str,
        start: bool,
        answer: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        self.acting.remove(name);
        let said = match (start, answer) {
            (true, Ok(answer)) => {
                let started = names_of(&answer["started"]);
                let others: Vec<&String> = started.iter().filter(|s| *s != name).collect();
                match answer["service"]["state"].as_str() {
                    Some("outside") => Toast::success(format!("{name} runs outside Flottille"))
                        .detail("It answered already: nothing was started."),
                    _ if started.is_empty() => Toast::success(format!("{name} runs already")),
                    _ if others.is_empty() => Toast::success(format!("{name} started")),
                    _ => Toast::success(format!("{name} started")).detail(format!(
                        "After what it needs: {}.",
                        others
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
            (false, Ok(_)) => Toast::success(format!("{name} stopped"))
                .detail("What it needs and what needs it go on."),
            (true, Err(why)) => Toast::failure(format!("{name} did not start")).detail(why),
            (false, Err(why)) => Toast::failure(format!("{name} was not stopped")).detail(why),
        };
        toast(said, cx);
        self.read(cx);
        cx.notify();
    }

    /// The service whose run is `run`.
    fn service_of_run(&self, run: &str) -> Option<String> {
        self.listed
            .iter()
            .flatten()
            .find(|s| s.run.as_deref() == Some(run))
            .map(|s| s.name.clone())
    }

    /// The drawer's Stop.
    pub fn stop_run(&mut self, run: &str, cx: &mut Context<Self>) {
        if let Some(name) = self.service_of_run(run) {
            self.stop(&name, cx);
        }
    }

    /// The drawer's Restart: a service runs once, so it starts it again when it no longer runs.
    pub fn start_run(&mut self, run: &str, cx: &mut Context<Self>) {
        if let Some(name) = self.service_of_run(run) {
            self.start(&name, cx);
        }
        self.panel
            .update(cx, |panel, cx| panel.restart_refused(run, cx));
    }

    /// Opens a service's output in the drawer, followed live.
    pub fn open_output(&mut self, name: &str, cx: &mut Context<Self>) {
        let run = self
            .listed
            .iter()
            .flatten()
            .find(|s| s.name == name)
            .and_then(|s| s.run.clone());
        if let Some(run) = run {
            self.open_run(&run, cx);
        }
    }

    pub fn open_run(&mut self, run: &str, cx: &mut Context<Self>) {
        self.watching = Some(run.to_owned());
        self.panel.update(cx, |panel, cx| panel.open(run, cx));
        self.drawer.update(cx, |drawer, cx| drawer.open(cx));
        self.read_output(cx);
        self.follow(true, cx);
    }

    pub fn close_output(&mut self, cx: &mut Context<Self>) {
        self.watching = None;
        self.follow(false, cx);
        self.panel.update(cx, |panel, cx| panel.close(cx));
        self.drawer.update(cx, |drawer, cx| drawer.close(cx));
    }

    fn read_output(&self, cx: &mut App) {
        if let Some(run) = self.watching.clone() {
            perform_later(&self.shell, Command::ServiceOutput { run }, cx);
        }
    }

    /// Reads the open output each second while `on`.
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
                cx.background_executor().timer(OUTPUT_EVERY).await;
                if this.update(cx, |this, cx| this.read_output(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// The output the daemon gave of the open run.
    pub fn output(&mut self, output: Option<CommandOutput>, cx: &mut Context<Self>) {
        if let Some(output) = output {
            let row = output.row.clone();
            self.panel.update(cx, |panel, cx| {
                panel.set_rows(&[row], cx);
                panel.set(output, cx);
            });
        }
    }

    /// Opens an address with the system's default handler.
    pub fn open_link(&self, url: &str, cx: &App) {
        if let Some(shell) = self.shell.upgrade() {
            shell.read(cx).open_link(url);
        }
    }

    fn menu(&mut self, name: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let this = cx.entity().downgrade();
        menu_of(&mut self.menus, "service", name, cx, move |name, menu| {
            let (edit, remove) = (this.clone(), this.clone());
            let (to_edit, to_remove) = (name.to_owned(), name.to_owned());
            menu.entry(
                format!("edit-service-{name}"),
                "Edit",
                None,
                move |window, cx| {
                    edit.update(cx, |this, cx| this.edit(Some(&to_edit), window, cx))
                        .ok();
                },
            )
            .danger(
                format!("remove-service-{name}"),
                "Remove…",
                move |_, cx| {
                    remove
                        .update(cx, |this, cx| this.remove(&to_remove, cx))
                        .ok();
                },
            )
        })
    }

    fn mark(&mut self, info: &ServiceInfo, cx: &mut Context<Self>) -> Entity<StateIcon> {
        let state = info.mark();
        let (status, icon) = self.marks.entry(info.name.clone()).or_insert_with(|| {
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

    /// Start, or Stop once it runs: the one fades into the other.
    fn act_button(
        &self,
        info: &ServiceInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = &info.name;
        let runs = info.runs();
        let t = fade(
            ElementId::Name(format!("service-act-{name}").into()),
            runs,
            HOVER,
            window,
            cx,
        );
        let waiting = self.acting.contains(name);
        let start = {
            let name = name.clone();
            Button::secondary(
                ElementId::Name(format!("service-start-{name}").into()),
                "Start",
            )
            .leading(Icon::Play)
            .disabled(waiting)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.start(&name, cx);
            }))
        };
        let stop = {
            let name = name.clone();
            Button::quiet(
                ElementId::Name(format!("service-stop-{name}").into()),
                "Stop",
            )
            .leading(glyphs::STOP)
            .disabled(waiting)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.stop(&name, cx);
            }))
        };
        // The one leaving sits over the one coming, which gives the frame its size.
        let layer = |shown: f32, over: bool| {
            div()
                .opacity(shown)
                .when(over, |d| d.absolute().top_0().right_0())
        };
        div()
            .relative()
            .flex_none()
            .when(t < 1.0, |d| d.child(layer(1.0 - t, runs).child(start)))
            .when(t > 0.0, |d| d.child(layer(t, !runs).child(stop)))
            .into_any_element()
    }

    fn row(&mut self, info: &ServiceInfo, window: &mut Window, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let name = info.name.clone();
        let menu = self.menu(&name, cx);
        let icon = self.mark(info, cx);
        let mark = div()
            .id(ElementId::Name(format!("service-mark-{name}").into()))
            .aria_label(SharedString::from(info.state_said().to_owned()))
            .child(ready_mark(
                ElementId::Name(format!("service-state-{name}").into()),
                ElementId::Name(format!("service-dot-{name}").into()),
                // Up: ready, or running when it says no ready. Nothing turns while it runs.
                matches!(info.state.as_str(), "ready" | "running"),
                Some(icon.into_any_element()),
                &p,
                window,
                cx,
            ))
            .test_support();
        let id = |part: &str| ElementId::Name(format!("service-{part}-{name}").into());
        let folder = info.folder.clone().unwrap_or_else(|| "the home".into());
        let mut lines: Vec<AnyElement> = vec![
            LabelledLine::new(
                id("folder"),
                Icon::Folder,
                "Folder",
                SharedString::from(folder.clone()),
            )
            .child(SharedString::from(folder))
            .into_any_element(),
        ];
        if !info.needs.is_empty() {
            lines.push(needs_line(id("needs"), &info.needs).into_any_element());
        }
        if !info.needed_by.is_empty() {
            let said = info.needed_said();
            lines.push(
                LabelledLine::new(
                    id("needed"),
                    Icon::Repository,
                    "Needed by",
                    SharedString::from(said.clone()),
                )
                .child(SharedString::from(said))
                .into_any_element(),
            );
        }
        // What comes and goes with its state unfolds and folds away.
        let page = cx.entity().downgrade();
        let link_id = id("link");
        let address = info.address.as_ref().map(|url| {
            let page = page.clone();
            LabelledLine::new(
                id("address"),
                Icon::OpenInEditor,
                "Address",
                SharedString::from(url.clone()),
            )
            .child(address_link(
                link_id.clone(),
                url,
                12.,
                move |url, cx| {
                    if let Some(page) = page.upgrade() {
                        page.read(cx).open_link(url, cx);
                    }
                },
                window,
                cx,
            ))
            .into_any_element()
        });
        lines.push(
            presence(
                id("address-line"),
                address.is_some(),
                address.unwrap_or_else(|| div().into_any_element()),
            )
            .into_any_element(),
        );
        let why = (info.state == "failed").then(|| info.why.clone().unwrap_or_default());
        let why_id = id("why");
        lines.push(
            presence_of(id("why-line"), why.map(SharedString::from), move |why| {
                LabelledLine::new(why_id.clone(), Icon::Warning, "Why", why.clone())
                    .child(div().text_color(p.fail).child(why))
                    .into_any_element()
            })
            .into_any_element(),
        );
        let outside = (info.state == "outside").then(|| {
            format!(
                "It answered before Flottille started it: Flottille did not start it and does \
                 not stop it. Start runs {} once it no longer answers.",
                info.line
            )
        });
        let outside_id = id("outside");
        lines.push(
            presence_of(
                id("outside-line"),
                outside.map(SharedString::from),
                move |said| {
                    div()
                        .id(outside_id.clone())
                        .aria_label(said.clone())
                        .pt(space::XS)
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(said)
                        .test_support()
                        .into_any_element()
                },
            )
            .into_any_element(),
        );
        let stopped = (!info.needs_stopped.is_empty() && info.runs()).then(|| {
            format!(
                "What it needs is not running: {}.",
                info.needs_stopped.join(", ")
            )
        });
        let stopped_id = id("needs-stopped");
        lines.push(
            presence_of(
                id("needs-stopped-line"),
                stopped.map(SharedString::from),
                move |said| {
                    div()
                        .id(stopped_id.clone())
                        .aria_label(said.clone())
                        .text_size(px(12.5))
                        .text_color(p.wait)
                        .child(said)
                        .test_support()
                        .into_any_element()
                },
            )
            .into_any_element(),
        );
        let output = info.run.is_some().then(|| {
            let name = info.name.clone();
            Button::icon(
                ElementId::Name(format!("service-output-{name}").into()),
                Icon::Terminal,
                "Output",
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.open_output(&name, cx);
            }))
        });
        let trailing = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .children(output)
            .child(self.act_button(info, window, cx));
        let mut row = ListRow::new(
            ElementId::Name(format!("service-{}", info.name).into()),
            SharedString::from(info.name.clone()),
        )
        .mono()
        .leading(mark)
        .meta(SharedString::from(info.state_said().to_owned()))
        .line(SharedString::from(info.line.clone()))
        .mono_line()
        .below(div().pt(space::XS).flex().flex_col().children(lines))
        .trailing(trailing)
        .more(menu);
        if info.ask_before_running {
            row = row.tag("asks");
        }
        let edit = info.name.clone();
        row.on_click(cx.listener(move |this, _, window, cx| this.edit(Some(&edit), window, cx)))
    }

    /// The confirmation of a removal: one that tasks or services need cannot be done, and says
    /// which.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let name = self.asked.clone();
        let refused = self.removal_refused().map(SharedString::from);
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("service-removal", format!("Remove the service {name}?"))
            .sentence("No task can need it any more, and it leaves services.toml.")
            .action("Remove service")
            .refused(refused)
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
}

fn capitalized(text: &str) -> String {
    let mut letters = text.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

impl Render for ServicesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows
                .sync(listed.into_iter().map(|s| (s.name.clone(), s)), now, reduce);
        }
        let drawn: Vec<(String, Placement, ServiceInfo)> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        self.marks
            .retain(|name, _| drawn.iter().any(|(key, ..)| key == name));
        let mut list = ListCard::new("service-list");
        for (key, placement, info) in drawn {
            let row = self.row(&info, window, cx);
            list = list.item(
                ElementId::Name(format!("service-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let this = cx.entity().downgrade();
        let header = PageHeader::new("services-header", "Services").action(
            "add-service",
            "New service",
            Some(Icon::Plus),
            move |window, cx| {
                this.update(cx, |this, cx| this.edit(None, window, cx)).ok();
            },
        );
        let section = Section::new("services", "On this machine")
            .count(count.to_string())
            .sentence(
                "The programs every project shares: a database, a cache, an API. Each runs once \
                 for the whole machine; a task that needs one starts it first.",
            )
            .child(presence("service-list-block", count > 0, list).gap(space::MD))
            .child(
                presence(
                    "no-services-block",
                    empty,
                    EmptyLine::new(
                        "no-services",
                        Icon::Service,
                        "No service yet: add the programs your projects lean on (a database, a \
                         cache, an API), so that their tasks start them when they need them.",
                    ),
                )
                .gap(space::MD),
            )
            .child(problem_line(
                "services-problem",
                self.problem.clone(),
                space::MD,
                cx,
            ));
        let pad = if self.narrow { space::LG } else { space::XXL };
        let key = |part: &str| ElementId::Name(format!("enter-services-{part}").into());
        let column = div()
            .w_full()
            .max_w(space::COLUMN)
            .flex()
            .flex_col()
            .gap(space::XXL)
            .child(enter(key("header"), 0, header))
            .child(enter(key("list"), 1, section));
        div()
            .id("services-page-frame")
            .relative()
            .size_full()
            .text_color(p.text)
            .child(
                div()
                    .id("services-page")
                    .track_scroll(&self.scroll)
                    .size_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(pad)
                    .pt(space::XXL)
                    .pb(space::XXXL)
                    .child(column),
            )
            .child(self.form.clone())
            .child(self.confirm(cx))
            // A service's output slides in over the page, which keeps its scroll beneath it.
            .child(self.drawer.clone())
    }
}
