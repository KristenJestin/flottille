//! A project's procedures on its page (`docs/procedures.md`), and their runs. The window writes
//! nothing itself: every add, edit and removal is the daemon's (`procedure.*`), and the section
//! shows the list the daemon gives next.
//!
//! * [`ProceduresSection`]: the "Procedures" section of the project page, with the modal that adds
//!   one (from the list of templates, or blank) or edits one.
//! * [`RunProcedure`]: the modal that runs one, picked with `/` in a session's composer, in that
//!   session's workspace. It starts nothing before Confirm.
//! * [`run_block`]: the run a session belongs to, in its rail's WORKSPACE section.

use flottille_core::procedure::TEMPLATES;
use flottille_ui::{Button, Icon, ListRow, Modal, Type, icon, palette};
use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::{Command, ProcedureWrite};
use crate::shell::Shell;

/// A procedure as the daemon shows it (`docs/protocol.md`, "The procedure methods").
#[derive(Clone, Debug, PartialEq)]
pub struct ProcedureInfo {
    pub name: String,
    pub description: String,
    /// Empty unless the daemon gave it whole (`procedure.show`).
    pub instructions: String,
}

impl ProcedureInfo {
    pub fn of(value: &Value) -> Self {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        Self {
            name: text(&value["name"]),
            description: text(&value["description"]),
            instructions: text(&value["instructions"]),
        }
    }
}

/// Asks the daemon through the shell, once the update running now is over: the shell hands its
/// answers to these views while it is itself being updated, and a view cannot update it then.
pub(crate) fn perform_later(shell: &WeakEntity<Shell>, command: Command, cx: &mut App) {
    let shell = shell.clone();
    cx.defer(move |cx| {
        shell.update(cx, |shell, _| shell.perform(command)).ok();
    });
}

pub(crate) fn editor_style(cx: &App) -> InputEditorStyle {
    let p = palette(cx);
    InputEditorStyle {
        foreground: p.text,
        muted_foreground: p.faint,
        selection: p.run.opacity(0.35),
        caret: p.text,
        ..Default::default()
    }
}

/// A one-line field with the window's editor style.
pub(crate) fn line_field(
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut field = InputState::new(window, cx).placeholder(placeholder);
        field.set_editor_style(editor_style(cx));
        field
    })
}

/// A field in the modals: a box around an input.
pub(crate) fn boxed(id: &'static str, field: impl IntoElement, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id(id)
        .min_h(px(32.))
        .px(px(10.))
        .py(px(5.))
        .rounded(px(8.))
        .bg(p.field)
        .border_1()
        .border_color(p.line2)
        .flex()
        .items_center()
        .mono(12.5)
        .child(field)
        .test_support()
}

/// The heading of a section of the project page.
pub(crate) fn heading(
    title: &'static str,
    count: usize,
    action: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .mt(px(34.))
        .mb(px(12.))
        .text_size(px(13.))
        .semibold()
        .child(title)
        .child(
            div()
                .mono(11.)
                .font_weight(gpui_kit::FontWeight::NORMAL)
                .text_color(p.faint)
                .child(count.to_string()),
        )
        .child(div().flex_1())
        .child(action)
}

// ── The section ────────────────────────────────────────────────────────────────────────────────

/// What the modal is doing.
#[derive(Clone, Debug, PartialEq)]
enum Form {
    Closed,
    /// Which template, or a blank one.
    Choosing,
    /// A procedure written out: a new one, or the one named.
    Editing(Option<String>),
}

struct Fields {
    name: Entity<InputState>,
    description: Entity<InputState>,
    instructions: Entity<TextareaState>,
}

pub struct ProceduresSection {
    shell: WeakEntity<Shell>,
    project: String,
    listed: Option<Vec<ProcedureInfo>>,
    /// What the daemon said when the list could not be read, or a removal was refused.
    problem: Option<SharedString>,
    form: Form,
    /// Made when the modal first opens, which is when a window is at hand.
    fields: Option<Fields>,
    saving: bool,
    form_error: Option<SharedString>,
    /// The procedure being read to be edited.
    reading: Option<String>,
    confirming: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ProceduresSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            listed: None,
            problem: None,
            form: Form::Closed,
            fields: None,
            saving: false,
            form_error: None,
            reading: None,
            confirming: None,
            _subscriptions: Vec::new(),
        }
    }

    /// The procedures as last listed; `None` until the daemon has said.
    pub fn procedures(&self) -> Option<&[ProcedureInfo]> {
        self.listed.as_deref()
    }

    pub fn is_open(&self) -> bool {
        self.form != Form::Closed
    }

    pub fn form_error(&self) -> Option<&str> {
        self.form_error.as_deref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn description_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.description)
    }

    pub fn name_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.name)
    }

    pub fn instructions_field(&self) -> Option<&Entity<TextareaState>> {
        self.fields.as_ref().map(|f| &f.instructions)
    }

    /// Asks the daemon for the list.
    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(&self.shell, Command::Procedures { project }, cx);
    }

    /// The daemon's list.
    pub fn listed(&mut self, listed: Result<Vec<ProcedureInfo>, String>, cx: &mut Context<Self>) {
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
            let name = line_field("name, for example open-prs", window, cx);
            let description = line_field("one line: what it does", window, cx);
            let instructions = cx.new(|cx| {
                let mut field = TextareaState::new(window, cx)
                    .auto_grow(4, 10)
                    .placeholder("what the agent is told to do");
                field.set_editor_style(editor_style(cx));
                field
            });
            // Typing clears what a refusal said.
            for input in [&name, &description] {
                self._subscriptions
                    .push(cx.subscribe(input, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) && this.form_error.take().is_some() {
                            cx.notify();
                        }
                    }));
            }
            self.fields = Some(Fields {
                name,
                description,
                instructions,
            });
        }
        self.fields.as_ref().expect("made above")
    }

    /// "New procedure": the list of the templates, and a blank one.
    pub fn add(&mut self, cx: &mut Context<Self>) {
        self.form = Form::Choosing;
        self.form_error = None;
        self.saving = false;
        cx.notify();
    }

    fn blank(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fill(None, window, cx);
    }

    /// Opens the form on `info`, or empty.
    fn fill(&mut self, info: Option<&ProcedureInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields(window, cx);
        let (name, description, instructions) = (
            fields.name.clone(),
            fields.description.clone(),
            fields.instructions.clone(),
        );
        let text = |f: fn(&ProcedureInfo) -> &str| info.map(f).unwrap_or_default().to_owned();
        name.update(cx, |f, cx| {
            f.set_value(text(|i| &i.name), window, cx);
        });
        description.update(cx, |f, cx| {
            f.set_value(text(|i| &i.description), window, cx);
        });
        instructions.update(cx, |f, cx| {
            f.set_value(text(|i| &i.instructions), window, cx);
        });
        self.form = Form::Editing(info.map(|info| info.name.clone()));
        self.form_error = None;
        self.saving = false;
        let focus = if info.is_some() { description } else { name }
            .read(cx)
            .focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Reads the procedure whole, then opens it for editing.
    pub fn edit(&mut self, name: &str, cx: &mut Context<Self>) {
        self.reading = Some(name.to_owned());
        self.problem = None;
        let command = Command::ProcedureRead {
            project: self.project.clone(),
            name: name.to_owned(),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's reading of the procedure to edit.
    pub fn read_whole(
        &mut self,
        name: &str,
        read: Result<ProcedureInfo, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reading.as_deref() != Some(name) {
            return;
        }
        self.reading = None;
        match read {
            Ok(info) => self.fill(Some(&info), window, cx),
            Err(why) => {
                self.problem = Some(why.into());
                cx.notify();
            }
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.form = Form::Closed;
        self.form_error = None;
        cx.notify();
    }

    fn write(&mut self, write: ProcedureWrite, cx: &mut Context<Self>) {
        self.saving = true;
        self.form_error = None;
        self.problem = None;
        let command = Command::ProcedureWrite {
            project: self.project.clone(),
            write,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    fn template(&mut self, template: &str, cx: &mut Context<Self>) {
        self.write(
            ProcedureWrite::Template {
                template: template.to_owned(),
            },
            cx,
        );
    }

    /// Saves the form: a new procedure, or the one being edited.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Form::Editing(original) = self.form.clone() else {
            return;
        };
        let Some(fields) = &self.fields else {
            return;
        };
        let name = match &original {
            Some(name) => name.clone(),
            None => fields.name.read(cx).value().trim().to_owned(),
        };
        let (description, instructions) = (
            fields.description.read(cx).value().trim().to_owned(),
            fields.instructions.read(cx).value().trim().to_owned(),
        );
        let write = match original {
            Some(_) => ProcedureWrite::Edit {
                name,
                description,
                instructions,
            },
            None => ProcedureWrite::Add {
                name,
                description,
                instructions,
            },
        };
        self.write(write, cx);
    }

    /// Asks to remove; the second click confirms.
    pub fn remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(name) {
            self.confirming = Some(name.to_owned());
            cx.notify();
            return;
        }
        self.confirming = None;
        self.write(
            ProcedureWrite::Remove {
                name: name.to_owned(),
            },
            cx,
        );
    }

    /// The daemon's answer to a write: done, and the list is read again; or refused, and why.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        match refused {
            None => {
                self.form = Form::Closed;
                self.form_error = None;
                self.read(cx);
            }
            Some(why) if self.form == Form::Closed => self.problem = Some(why.into()),
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    fn row(&self, info: &ProcedureInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let name = info.name.clone();
        let id = |part: &str| ElementId::Name(format!("{part}-{}", info.name).into());
        let confirming = self.confirming.as_deref() == Some(&info.name);
        div()
            .id(id("procedure"))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(p.line)
            .bg(p.card)
            .flex()
            .items_start()
            .gap(px(10.))
            .child(
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
                            .child(SharedString::from(name.clone())),
                    )
                    .child(
                        div()
                            .id(id("procedure-description"))
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .aria_label(SharedString::from(info.description.clone()))
                            .child(SharedString::from(info.description.clone()))
                            .test_support(),
                    ),
            )
            .child({
                let edit = name.clone();
                Button::quiet(id("edit-procedure"), "Edit")
                    .disabled(self.reading.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| this.edit(&edit, cx)))
            })
            .child(if confirming {
                let (cancel, remove) = (name.clone(), name.clone());
                div()
                    .flex()
                    .gap(px(4.))
                    .child(
                        Button::quiet(id("cancel-remove-procedure"), "Cancel").on_click(
                            cx.listener(move |this, _, _, cx| {
                                if this.confirming.as_deref() == Some(&cancel) {
                                    this.confirming = None;
                                    cx.notify();
                                }
                            }),
                        ),
                    )
                    .child(
                        Button::secondary(id("confirm-remove-procedure"), format!("Remove {name}"))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx))),
                    )
                    .into_any_element()
            } else {
                let remove = name.clone();
                Button::quiet(id("remove-procedure"), "Remove")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx)))
                    .into_any_element()
            })
            .test_support()
    }

    fn modal(&mut self, window: &Window, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let open = self.form != Form::Closed;
        let title = match &self.form {
            Form::Editing(Some(_)) => "Edit procedure",
            _ => "New procedure",
        };
        let modal = Modal::new("procedure-modal", title)
            .width(px(600.))
            .open(open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let error = self.form_error.clone().map(|error| {
            div()
                .id("procedure-error")
                .text_size(px(12.5))
                .text_color(p.fail)
                .child(error)
                .test_support()
        });
        match self.form.clone() {
            Form::Closed => modal,
            Form::Choosing => {
                let templates = TEMPLATES.into_iter().map(|template| {
                    let description = flottille_core::procedure::template(template)
                        .map(|t| t.description)
                        .unwrap_or_default();
                    ListRow::new(
                        ElementId::Name(format!("template-{template}").into()),
                        template,
                    )
                    .leading(icon(Icon::Branch).size(px(14.)).text_color(p.faint))
                    .line(description)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.saving {
                            this.template(template, cx)
                        }
                    }))
                });
                let blank = ListRow::new("blank-procedure", "Blank procedure")
                    .leading(icon(Icon::Plus).size(px(14.)).text_color(p.faint))
                    .line("Write your own: a name, what it does, and what the agent is told")
                    .on_click(cx.listener(|this, _, window, cx| this.blank(window, cx)));
                modal
                    .child(
                        div()
                            .id("procedure-templates")
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(templates)
                            .child(blank)
                            .test_support(),
                    )
                    .children(error)
            }
            Form::Editing(original) => {
                let fields = self.fields.as_ref().expect("made when the form opens");
                let body = div()
                    .id("procedure-form")
                    .max_h(window.viewport_size().height - px(200.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(match &original {
                        Some(name) => div()
                            .mono(12.5)
                            .semibold()
                            .child(SharedString::from(name.clone()))
                            .into_any_element(),
                        None => {
                            boxed("procedure-name", Input::new(&fields.name), cx).into_any_element()
                        }
                    })
                    .child(boxed(
                        "procedure-description-field",
                        Input::new(&fields.description),
                        cx,
                    ))
                    .child(
                        div()
                            .id("procedure-instructions")
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(8.))
                            .bg(p.field)
                            .border_1()
                            .border_color(p.line2)
                            .text_size(px(12.5))
                            .child(fields.instructions.clone())
                            .test_support(),
                    )
                    .test_support();
                modal.child(body).children(error).action(if self.saving {
                    div()
                        .mono(11.5)
                        .text_color(p.faint)
                        .child("Saving…")
                        .into_any_element()
                } else {
                    Button::primary("save-procedure", "Save")
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                        .into_any_element()
                })
            }
        }
    }
}

impl Render for ProceduresSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let modal = self.modal(window, cx);
        div()
            .id("procedures")
            .child(heading(
                "Procedures",
                count,
                Button::secondary("add-procedure", "New procedure")
                    .leading(flottille_ui::Icon::Plus)
                    .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
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
                                .id("no-procedures")
                                .text_size(px(12.5))
                                .text_color(p.faint)
                                .child(
                                    "No procedure yet: add one from a template to open the pull \
                                     requests of a workspace, merge a stack or close it.",
                                )
                                .test_support(),
                        )
                    })
                    .when_some(self.problem.clone(), |d, problem| {
                        d.child(
                            div()
                                .id("procedures-problem")
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

/// What the run modal says of what a run may do: nothing more than any agent.
const RUN_SAYS: &str =
    "It grants nothing: each call the run makes goes through the session's mode, like any other.";

// ── The run modal ──────────────────────────────────────────────────────────────────────────────

/// The modal that runs a procedure picked with `/` in the workspace of the session it was typed
/// in: the procedure, the workspace, and Confirm. The daemon is asked to show the procedure
/// (`procedure.show`), which starts nothing; `procedure.start` is asked only when the user
/// confirms.
pub struct RunProcedure {
    shell: WeakEntity<Shell>,
    open: bool,
    project: String,
    workspace: String,
    chosen: Option<String>,
    /// The procedure `chosen`, as the daemon shows it.
    shown: Option<ProcedureInfo>,
    starting: bool,
    error: Option<SharedString>,
}

impl RunProcedure {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            open: false,
            project: String::new(),
            workspace: String::new(),
            chosen: None,
            shown: None,
            starting: false,
            error: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    pub fn chosen(&self) -> Option<&str> {
        self.chosen.as_deref()
    }

    /// The procedure the daemon showed for the choice.
    pub fn shown(&self) -> Option<&ProcedureInfo> {
        self.shown.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_starting(&self) -> bool {
        self.starting
    }

    /// Opens the modal on a procedure to run in a workspace, nothing running: the daemon is asked
    /// to show it.
    pub fn open(&mut self, project: &str, workspace: &str, name: &str, cx: &mut Context<Self>) {
        self.open = true;
        self.project = project.to_owned();
        self.workspace = workspace.to_owned();
        self.starting = false;
        self.choose(name, cx);
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.error = None;
        cx.notify();
    }

    /// Chooses a procedure: the daemon shows it. Nothing starts.
    pub fn choose(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.starting {
            return;
        }
        self.chosen = Some(name.to_owned());
        self.shown = None;
        self.error = None;
        let command = Command::ProcedureShow {
            project: self.project.clone(),
            name: name.to_owned(),
            workspace: self.workspace.clone(),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The chosen procedure, as the daemon shows it.
    pub fn shown_by_daemon(
        &mut self,
        project: &str,
        name: &str,
        workspace: &str,
        shown: Result<ProcedureInfo, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.open
            || self.project != project
            || self.chosen.as_deref() != Some(name)
            || self.workspace != workspace
        {
            return;
        }
        match shown {
            Ok(shown) => self.shown = Some(shown),
            Err(why) => self.error = Some(why.into()),
        }
        cx.notify();
    }

    /// Confirm: the run of the procedure that was shown starts.
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        let (Some(shown), false) = (self.shown.clone(), self.starting) else {
            return;
        };
        self.starting = true;
        self.error = None;
        let command = Command::ProcedureStart {
            project: self.project.clone(),
            name: shown.name.clone(),
            workspace: self.workspace.clone(),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to Confirm: the modal closes (the shell opens the session), or it says
    /// why nothing started.
    pub fn started(&mut self, project: &str, refused: Option<String>, cx: &mut Context<Self>) {
        if self.project != project {
            return;
        }
        self.starting = false;
        match refused {
            None => self.open = false,
            Some(why) => self.error = Some(why.into()),
        }
        cx.notify();
    }
}

impl Render for RunProcedure {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let mut modal = Modal::new("run-procedure-modal", "Run procedure")
            .width(px(540.))
            .open(self.open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        if self.open || self.shown.is_some() {
            modal = modal.child(
                div()
                    .id("run-workspace")
                    .flex()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .child(div().text_color(p.faint).child("in workspace"))
                    .child(
                        div()
                            .mono(12.)
                            .child(SharedString::from(self.workspace.clone())),
                    )
                    .test_support(),
            );
            if let Some(shown) = &self.shown {
                modal = modal.child(
                    div()
                        .id("run-shown")
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .mono(12.5)
                                .semibold()
                                .child(SharedString::from(shown.name.clone())),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(p.mute)
                                .child(SharedString::from(shown.description.clone())),
                        )
                        .child(
                            div()
                                .id("run-says")
                                .aria_label(RUN_SAYS)
                                .text_size(px(12.))
                                .text_color(p.faint)
                                .child(RUN_SAYS)
                                .test_support(),
                        )
                        .test_support(),
                );
            }
            modal = modal
                .child(
                    div()
                        .id("run-notice")
                        .aria_label("Nothing starts until you confirm.")
                        .text_size(px(12.))
                        .text_color(p.faint)
                        .child("Nothing starts until you confirm.")
                        .test_support(),
                )
                .when_some(self.error.clone(), |m, error| {
                    m.child(
                        div()
                            .id("run-error")
                            .text_size(px(12.5))
                            .text_color(p.fail)
                            .child(error)
                            .test_support(),
                    )
                })
                .action(if self.starting {
                    div()
                        .mono(11.5)
                        .text_color(p.faint)
                        .child("Starting…")
                        .into_any_element()
                } else {
                    Button::primary("run-confirm", "Confirm")
                        .disabled(self.shown.is_none())
                        .on_click(cx.listener(|this, _, _, cx| this.confirm(cx)))
                        .into_any_element()
                });
        }
        div().id("run-procedure").child(modal)
    }
}

// ── The rail ───────────────────────────────────────────────────────────────────────────────────

/// The run a session is the root of, as its rail shows it while it goes on.
#[derive(Clone, Debug, PartialEq)]
pub struct RunFacts {
    pub procedure: String,
}

impl RunFacts {
    /// From `procedure.of`'s `run`; `None` when there is none, or it is over.
    pub fn of(run: &Value) -> Option<Self> {
        if run.is_null() || run["ended"] == true {
            return None;
        }
        Some(Self {
            procedure: run["procedure"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// The run, in the WORKSPACE section of the rail: the procedure it runs.
pub fn run_block(run: &RunFacts, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id("rail-run")
        .px(px(8.))
        .py(px(4.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .id("rail-run-title")
                .aria_label(SharedString::from(format!("Running {}", run.procedure)))
                .flex()
                .gap(px(6.))
                .text_size(px(11.5))
                .child(div().text_color(p.faint).child("Running"))
                .child(
                    div()
                        .mono(11.5)
                        .child(SharedString::from(run.procedure.clone())),
                )
                .test_support(),
        )
        .test_support()
}
