//! A project's setup in its Settings (`docs/projects.md`, "The setup"): the steps that prepare
//! each new workspace, in order. The "Setup" section lists them by number; each moves up or down,
//! is edited or removed, and the modal makes one: its kind (copy, link, root entries, run, task),
//! where it is done (the workspace folder, each repository, or one), and a time limit for a run
//! or a task. The window writes nothing itself: every change is the daemon's (`setup.*`), which
//! checks the step as the file is checked, and the section shows the list the daemon gives next.

use flottille_core::project::SetupStep;
use flottille_ui::{Button, Confirm, Icon, Item, Listing, Modal, Type, palette, presence, space};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _,
    WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::Command;
use crate::commands::{
    boxed, empty_note, heading, line_field, perform_later, problem_line, save_or_saving,
};
use crate::project::repository_label;
use crate::shell::Shell;

/// A step of `setup.list`, without the number the daemon adds; `None` for one this window cannot
/// read.
pub fn step_of(value: &Value) -> Option<SetupStep> {
    let mut value = value.clone();
    value.as_object_mut()?.remove("number");
    serde_json::from_value(value).ok()
}

/// A step in words: what it does and where.
pub fn said(step: &SetupStep) -> String {
    let kind = match kind_of(step) {
        Some(kind) => kind,
        None => return "a step this window cannot show: edit the project's file".into(),
    };
    let what = match (kind, step) {
        (Kind::Copy, _) => format!("Copy {}", step.copy.clone().unwrap_or_default().join(", ")),
        (Kind::Link, _) => format!("Link {}", step.link.clone().unwrap_or_default()),
        (Kind::RootEntries, _) => "Link the project folder's other entries".to_owned(),
        (Kind::Run, _) => format!("Run {}", step.run.clone().unwrap_or_default()),
        (Kind::Task, _) => format!("Run the task {}", step.task.clone().unwrap_or_default()),
    };
    let place = match (&step.repository, step.for_each_repository, kind) {
        (_, _, Kind::RootEntries) => String::new(),
        (Some(repository), _, _) => format!(" · in {repository}"),
        (None, true, _) => " · in each repository, leaves first".to_owned(),
        (None, false, Kind::Task) => " · where the task says".to_owned(),
        (None, false, _) => " · in the workspace folder".to_owned(),
    };
    let limit = step
        .time_limit
        .map_or_else(String::new, |limit| format!(" · stopped after {limit} s"));
    format!("{what}{place}{limit}")
}

/// What a step does, as the form chooses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Link,
    RootEntries,
    Run,
    Task,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Copy,
        Kind::Link,
        Kind::RootEntries,
        Kind::Run,
        Kind::Task,
    ];

    fn word(self) -> &'static str {
        match self {
            Kind::Copy => "copy",
            Kind::Link => "link",
            Kind::RootEntries => "root-entries",
            Kind::Run => "run",
            Kind::Task => "task",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Copy => "Copy",
            Kind::Link => "Link",
            Kind::RootEntries => "Root entries",
            Kind::Run => "Run",
            Kind::Task => "Task",
        }
    }

    /// What its field holds.
    fn placeholder(self) -> &'static str {
        match self {
            Kind::Copy => "files from the main checkout, never over one: .env .env.*",
            Kind::Link => "an entry of the main checkout: node_modules",
            Kind::RootEntries => "",
            Kind::Run => "one command line, never run by a shell: npm install",
            Kind::Task => "a task's name: build",
        }
    }

    /// Whether it takes a time limit.
    fn runs(self) -> bool {
        matches!(self, Kind::Run | Kind::Task)
    }
}

fn kind_of(step: &SetupStep) -> Option<Kind> {
    let kinds = [
        (step.copy.is_some(), Kind::Copy),
        (step.link.is_some(), Kind::Link),
        (step.root_entries.is_some(), Kind::RootEntries),
        (step.run.is_some(), Kind::Run),
        (step.task.is_some(), Kind::Task),
    ];
    let mut set = kinds.iter().filter(|(set, _)| *set).map(|(_, kind)| *kind);
    match (set.next(), set.next()) {
        (Some(kind), None) => Some(kind),
        _ => None,
    }
}

/// Where a step is done, as the form chooses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Where {
    /// The workspace folder (for a task, where the task says).
    Workspace,
    EachRepository,
    Repository(String),
}

impl Where {
    fn of(step: &SetupStep) -> Where {
        match (&step.repository, step.for_each_repository) {
            (Some(repository), _) => Where::Repository(repository.clone()),
            (None, true) => Where::EachRepository,
            (None, false) => Where::Workspace,
        }
    }
}

/// The step the form says, from its kind, its field, where and its time limit.
pub fn step_from(kind: Kind, value: &str, place: &Where, limit: &str) -> Result<SetupStep, String> {
    let value = value.trim();
    let mut step = SetupStep::default();
    match kind {
        Kind::Copy => {
            step.copy = Some(
                value
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|glob| !glob.is_empty())
                    .map(str::to_owned)
                    .collect(),
            )
        }
        Kind::Link => step.link = Some(value.to_owned()),
        Kind::RootEntries => step.root_entries = Some(true),
        Kind::Run => step.run = Some(value.to_owned()),
        Kind::Task => step.task = Some(value.to_owned()),
    }
    if kind != Kind::RootEntries {
        match place {
            Where::Workspace => {}
            Where::EachRepository => step.for_each_repository = true,
            Where::Repository(path) => step.repository = Some(path.clone()),
        }
    }
    let limit = limit.trim();
    if kind.runs() && !limit.is_empty() {
        step.time_limit = Some(
            limit
                .parse()
                .map_err(|_| format!("the time limit is a number of seconds, not \"{limit}\""))?,
        );
    }
    Ok(step)
}

struct Fields {
    value: Entity<InputState>,
    limit: Entity<InputState>,
}

/// The form: a new step (`number` none), or the step of that number.
#[derive(Clone, Debug, PartialEq)]
struct Form {
    number: Option<usize>,
    kind: Kind,
    place: Where,
}

pub struct SetupSection {
    shell: WeakEntity<Shell>,
    project: String,
    listed: Option<Vec<SetupStep>>,
    /// The rows as drawn: a removed step folds away and a moved one slides before it settles.
    rows: Listing<(usize, SetupStep)>,
    /// The project's repositories, for where a step is done.
    repositories: Vec<String>,
    /// What the daemon said when the list could not be read, or a change was refused.
    problem: Option<SharedString>,
    form: Option<Form>,
    fields: Option<Fields>,
    saving: bool,
    form_error: Option<SharedString>,
    confirming: Option<usize>,
    /// What the confirmation names, kept while it fades away.
    asked: usize,
    _subscriptions: Vec<Subscription>,
}

impl SetupSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            listed: None,
            rows: Listing::default(),
            repositories: Vec::new(),
            problem: None,
            form: None,
            fields: None,
            saving: false,
            form_error: None,
            confirming: None,
            asked: 0,
            _subscriptions: Vec::new(),
        }
    }

    /// The steps as last listed; `None` until the daemon has said.
    pub fn steps(&self) -> Option<&[SetupStep]> {
        self.listed.as_deref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn form_error(&self) -> Option<&str> {
        self.form_error.as_deref()
    }

    pub fn is_open(&self) -> bool {
        self.form.is_some()
    }

    pub fn value_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.value)
    }

    pub fn limit_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.limit)
    }

    pub fn set_repositories(&mut self, repositories: Vec<String>, cx: &mut Context<Self>) {
        if self.repositories != repositories {
            self.repositories = repositories;
            cx.notify();
        }
    }

    /// Asks the daemon for the list.
    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(&self.shell, Command::Setup { project }, cx);
    }

    /// The daemon's list.
    pub fn listed(&mut self, listed: Result<Vec<SetupStep>, String>, cx: &mut Context<Self>) {
        match listed {
            Ok(list) => {
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
            let value = line_field("", window, cx);
            let limit = line_field(
                "time limit in seconds; the project's when empty",
                window,
                cx,
            );
            for input in [&value, &limit] {
                self._subscriptions
                    .push(cx.subscribe(input, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) && this.form_error.take().is_some() {
                            cx.notify();
                        }
                    }));
            }
            self.fields = Some(Fields { value, limit });
        }
        self.fields.as_ref().expect("made above")
    }

    /// "New step", or "Edit" on the step of that number: the form, filled with it.
    pub fn open(&mut self, number: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let step = number
            .and_then(|n| self.listed.as_ref()?.get(n - 1).cloned())
            .unwrap_or_else(|| SetupStep {
                copy: Some(Vec::new()),
                ..Default::default()
            });
        let kind = kind_of(&step).unwrap_or(Kind::Copy);
        let value = match kind {
            Kind::Copy => step.copy.clone().unwrap_or_default().join(" "),
            Kind::Link => step.link.clone().unwrap_or_default(),
            Kind::RootEntries => String::new(),
            Kind::Run => step.run.clone().unwrap_or_default(),
            Kind::Task => step.task.clone().unwrap_or_default(),
        };
        let limit = step.time_limit.map(|l| l.to_string()).unwrap_or_default();
        let fields = self.fields(window, cx);
        let (value_field, limit_field) = (fields.value.clone(), fields.limit.clone());
        value_field.update(cx, |f, cx| {
            f.set_placeholder(kind.placeholder(), window, cx);
            f.set_value(value, window, cx);
        });
        limit_field.update(cx, |f, cx| f.set_value(limit, window, cx));
        self.form = Some(Form {
            number,
            kind,
            place: Where::of(&step),
        });
        self.form_error = None;
        self.saving = false;
        let focus = value_field.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self.form_error = None;
        cx.notify();
    }

    pub fn choose_kind(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.kind = kind;
        }
        if let Some(fields) = &self.fields {
            fields.value.update(cx, |f, cx| {
                f.set_placeholder(kind.placeholder(), window, cx);
            });
        }
        self.form_error = None;
        cx.notify();
    }

    pub fn choose_place(&mut self, place: Where, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.place = place;
        }
        cx.notify();
    }

    fn write(&mut self, method: &'static str, mut params: Value, cx: &mut Context<Self>) {
        self.saving = true;
        self.form_error = None;
        self.problem = None;
        params["project"] = self.project.clone().into();
        let command = Command::SetupWrite {
            project: self.project.clone(),
            method,
            params,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// Saves the form: a new last step, or the step being edited.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let (Some(form), Some(fields)) = (self.form.clone(), &self.fields) else {
            return;
        };
        let value = fields.value.read(cx).value().to_string();
        let limit = fields.limit.read(cx).value().to_string();
        let step = match step_from(form.kind, &value, &form.place, &limit) {
            Ok(step) => step,
            Err(why) => {
                self.form_error = Some(why.into());
                cx.notify();
                return;
            }
        };
        match form.number {
            Some(number) => self.write("setup.edit", json!({"number": number, "step": step}), cx),
            None => self.write("setup.add", json!({"step": step}), cx),
        }
    }

    /// Moves the step of that number to another.
    pub fn move_step(&mut self, number: usize, to: usize, cx: &mut Context<Self>) {
        self.write("setup.move", json!({"number": number, "to": to}), cx);
    }

    /// Asks to remove, in a confirmation; called again from it, removes.
    pub fn remove(&mut self, number: usize, cx: &mut Context<Self>) {
        if self.confirming != Some(number) {
            self.confirming = Some(number);
            self.asked = number;
            cx.notify();
            return;
        }
        self.confirming = None;
        self.write("setup.remove", json!({"number": number}), cx);
    }

    /// The daemon's answer to a change: done, and the list is read again; or refused, and why.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        match refused {
            None => {
                self.form = None;
                self.form_error = None;
                self.read(cx);
            }
            Some(why) if self.form.is_none() => self.problem = Some(why.into()),
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    fn row(
        &self,
        number: usize,
        step: &SetupStep,
        count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let id = |part: &str| ElementId::Name(format!("{part}-{number}").into());
        let text = said(step);
        div()
            .id(id("setup-step"))
            .px(space::LG)
            .py(space::MD)
            .rounded(px(10.))
            .border_1()
            .border_color(p.line)
            .bg(p.card)
            .flex()
            .items_center()
            .gap(space::MD)
            .child(
                div()
                    .flex_none()
                    .w(px(22.))
                    .mono(12.)
                    .text_color(p.faint)
                    .child(SharedString::from(format!("{number}."))),
            )
            .child(
                div()
                    .id(id("setup-step-said"))
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.5))
                    .aria_label(SharedString::from(text.clone()))
                    .child(SharedString::from(text))
                    .test_support(),
            )
            .child(
                Button::icon(id("setup-up"), Icon::ChevronUp, "Move up")
                    .disabled(number == 1 || self.saving)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.move_step(number, number - 1, cx)),
                    ),
            )
            .child(
                Button::icon(id("setup-down"), Icon::Chevron, "Move down")
                    .disabled(number == count || self.saving)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.move_step(number, number + 1, cx)),
                    ),
            )
            .child(Button::quiet(id("edit-setup"), "Edit").on_click(
                cx.listener(move |this, _, window, cx| this.open(Some(number), window, cx)),
            ))
            .child({
                Button::quiet(id("remove-setup"), "Remove")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(number, cx)))
                    .into_any_element()
            })
            .test_support()
    }

    /// The confirmation of a removal.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let number = self.asked;
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("setup-removal", format!("Remove setup step {number}?"))
            .sentence("New workspaces skip it from now on; the ones made already keep what it did.")
            .action("Remove step")
            .open(self.confirming.is_some())
            .on_confirm(move |_, cx| {
                this.update(cx, |this, cx| this.remove(number, cx)).ok();
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
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let title = match self.form.as_ref().and_then(|f| f.number) {
            Some(_) => "Edit setup step",
            None => "New setup step",
        };
        let modal = Modal::new("setup-modal", title)
            .wide()
            .open(self.form.is_some())
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let (Some(form), Some(fields)) = (self.form.clone(), self.fields.as_ref()) else {
            return modal;
        };
        let choice = |id: String, label: String, chosen: bool| {
            Button::choice(ElementId::Name(id.into()), label, chosen)
        };
        let kinds = Kind::ALL.into_iter().map(|kind| {
            choice(
                format!("setup-kind-{}", kind.word()),
                kind.label().to_owned(),
                form.kind == kind,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.choose_kind(kind, window, cx)))
        });
        let mut places = vec![
            (
                Where::Workspace,
                "setup-where-workspace".to_owned(),
                match form.kind {
                    Kind::Task => "Where the task says".to_owned(),
                    _ => "Workspace folder".to_owned(),
                },
            ),
            (
                Where::EachRepository,
                "setup-where-each".to_owned(),
                "Each repository".to_owned(),
            ),
        ];
        places.extend(self.repositories.iter().map(|path| {
            (
                Where::Repository(path.clone()),
                format!("setup-where-{path}"),
                repository_label(&self.project, path),
            )
        }));
        let places = places.into_iter().map(|(place, id, label)| {
            let chosen = form.place == place;
            choice(id, label, chosen)
                .on_click(cx.listener(move |this, _, _, cx| this.choose_place(place.clone(), cx)))
        });
        let label = |text: &'static str| div().text_size(px(12.)).text_color(p.faint).child(text);
        let body = div()
            .id("setup-form")
            .flex()
            .flex_col()
            .gap(space::MD)
            .child(label("What it does"))
            .child(div().flex().flex_wrap().gap(space::XS).children(kinds))
            // What the kind needs unfolds as it is chosen and the rest folds away.
            .child(
                presence(
                    "setup-value-block",
                    form.kind != Kind::RootEntries,
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::MD)
                        .child(boxed("setup-value", Input::new(&fields.value), cx))
                        .child(label("Where"))
                        .child(div().flex().flex_wrap().gap(space::XS).children(places)),
                )
                .gap(space::MD),
            )
            .child(
                presence(
                    "setup-root-note",
                    form.kind == Kind::RootEntries,
                    div().text_size(px(12.5)).text_color(p.mute).child(
                        "Links into the workspace folder every top-level entry of the project \
                         folder that is not a repository, .git or .gitignore.",
                    ),
                )
                .gap(space::MD),
            )
            .child(
                presence(
                    "setup-limit-block",
                    form.kind.runs(),
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::MD)
                        .child(label("Time limit, in seconds"))
                        .child(boxed("setup-limit", Input::new(&fields.limit), cx)),
                )
                .gap(space::MD),
            )
            .child(problem_line(
                "setup-error",
                self.form_error.clone(),
                space::MD,
                cx,
            ))
            .test_support();
        modal.child(body).action(save_or_saving(
            "setup",
            self.saving,
            || {
                Button::primary("save-setup", "Save")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
            },
            cx,
        ))
    }
}

impl Render for SetupSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            // A step has no name: its words tell it from the others, and a repeat is numbered.
            let mut seen: std::collections::HashMap<String, usize> = Default::default();
            let keyed = listed.into_iter().enumerate().map(|(index, step)| {
                let words = said(&step);
                let again = seen.entry(words.clone()).or_default();
                *again += 1;
                (format!("{words}#{again}"), (index + 1, step))
            });
            self.rows.sync(keyed, now, reduce);
        }
        let count = self.rows.len();
        let rows: Vec<_> = self
            .rows
            .rows()
            .map(|row| {
                let (number, step) = row.value;
                Item::new(
                    ElementId::Name(format!("setup-item-{}", row.key).into()),
                    row.placement,
                    self.row(*number, step, count, cx),
                )
                .gap(space::SM)
            })
            .collect();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(cx);
        div()
            .id("setup")
            .child(heading(
                "Setup",
                count,
                Button::secondary("add-setup", "New step")
                    .leading(Icon::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.open(None, window, cx))),
                cx,
            ))
            .child(
                div()
                    .mb(px(10.))
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child(
                        "Done in order for each new workspace. A workspace keeps the setup it was \
                         made with.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::SM)
                    .children(rows)
                    .child(empty_note(
                        "no-setup",
                        empty,
                        "No step yet: a new workspace is its worktrees alone. Copy the .env \
                         files, link what is shared, run the install.",
                        cx,
                    ))
                    .child(problem_line(
                        "setup-problem",
                        self.problem.clone(),
                        space::SM,
                        cx,
                    )),
            )
            .child(modal)
            .child(self.confirm(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_makes_each_kind_of_step_where_it_is_done() {
        let each = Where::EachRepository;
        let copy = step_from(Kind::Copy, ".env, .env.*", &each, "").unwrap();
        assert_eq!(copy.copy, Some(vec![".env".into(), ".env.*".into()]));
        assert!(copy.for_each_repository);
        let run = step_from(Kind::Run, "npm ci", &Where::Repository("web".into()), "60").unwrap();
        assert_eq!(
            (
                run.run.as_deref(),
                run.repository.as_deref(),
                run.time_limit
            ),
            (Some("npm ci"), Some("web"), Some(60))
        );
        let root = step_from(Kind::RootEntries, "", &each, "5").unwrap();
        assert_eq!(root.root_entries, Some(true));
        assert!(!root.for_each_repository && root.time_limit.is_none());
        let link = step_from(Kind::Link, "node_modules", &Where::Workspace, "9").unwrap();
        assert_eq!(link.time_limit, None, "a link takes no time limit");
        assert!(step_from(Kind::Task, "build", &Where::Workspace, "soon").is_err());
    }

    #[test]
    fn a_step_reads_back_from_the_daemon_s_list_and_is_said_in_words() {
        let step =
            step_of(&json!({"number": 2, "copy": [".env"], "for_each_repository": true})).unwrap();
        assert_eq!(said(&step), "Copy .env · in each repository, leaves first");
        let task = step_of(&json!({"number": 1, "task": "build", "time_limit": 30})).unwrap();
        assert_eq!(
            said(&task),
            "Run the task build · where the task says · stopped after 30 s"
        );
    }
}
