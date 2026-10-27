//! A project's procedures on its page (`docs/procedures.md`), and their runs. The window writes
//! nothing itself: every add, edit and removal is the daemon's (`procedure.*`), and the section
//! shows the list the daemon gives next.
//!
//! * [`ProceduresSection`]: the "Procedures" section of the project page, with the modal that adds
//!   one (from a template or blank) or edits one.
//! * [`RunProcedure`]: the modal that runs one in a workspace. It shows the grants in words and
//!   starts nothing before Confirm.
//! * [`run_block`]: the grants of the run a session belongs to, in its rail's WORKSPACE section.

use std::collections::BTreeMap;

use flottille_core::procedure::{Grant, Mode, TEMPLATES};
use flottille_ui::{Button, Modal, Switch, Type, palette};
use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::{Value, json};

use crate::actions::{Command, ProcedureWrite};
use crate::shell::Shell;

/// What a grant's mode means, in words: `allow` runs without asking, `ask` is held for the user's
/// answer, every time.
pub fn mode_words(mode: &str) -> &'static str {
    match mode {
        "allow" => "runs without asking",
        "ask" => "held for your answer",
        _ => "not granted",
    }
}

/// One grant of a procedure: the sentence the daemon gives for what it lets through.
#[derive(Clone, Debug, PartialEq)]
pub struct GrantLine {
    pub grant: String,
    pub mode: String,
    pub says: String,
}

impl GrantLine {
    /// `push: runs without asking · push the workspace's branch…`
    pub fn text(&self) -> String {
        format!("{}: {} · {}", self.grant, mode_words(&self.mode), self.says)
    }
}

/// A procedure as the daemon shows it (`docs/protocol.md`, "The procedure methods").
#[derive(Clone, Debug, PartialEq)]
pub struct ProcedureInfo {
    pub name: String,
    pub description: String,
    /// Empty unless the daemon gave it whole (`procedure.show`).
    pub instructions: String,
    pub grants: Vec<GrantLine>,
    pub children: bool,
}

impl ProcedureInfo {
    pub fn of(value: &Value) -> Self {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        Self {
            name: text(&value["name"]),
            description: text(&value["description"]),
            instructions: text(&value["instructions"]),
            grants: value["says"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|line| GrantLine {
                    grant: text(&line["grant"]),
                    mode: text(&line["mode"]),
                    says: text(&line["says"]),
                })
                .collect(),
            children: value["children"] == true,
        }
    }

    /// The grants as the daemon takes them back: grant to mode.
    pub fn grants_json(&self) -> Value {
        Value::Object(
            self.grants
                .iter()
                .map(|line| (line.grant.clone(), json!(line.mode)))
                .collect(),
        )
    }

    /// The mode a grant has, if it is given.
    pub fn mode_of(&self, grant: &str) -> Option<&str> {
        self.grants
            .iter()
            .find(|line| line.grant == grant)
            .map(|line| line.mode.as_str())
    }

    /// Each grant in words, then whether the children get them.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self.grants.iter().map(GrantLine::text).collect();
        lines.push(children_words(self.children, !self.grants.is_empty()).into());
        lines
    }
}

fn children_words(children: bool, grants: bool) -> &'static str {
    match (grants, children) {
        (false, _) => "It grants nothing beyond what agents may always do.",
        (true, true) => "The agents the run starts get these grants too.",
        (true, false) => "The agents the run starts get none of them.",
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
    grants: BTreeMap<Grant, Mode>,
    children: bool,
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
            grants: BTreeMap::new(),
            children: false,
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

    /// The mode the form gives a grant now.
    pub fn grant_in_form(&self, grant: Grant) -> Option<Mode> {
        self.grants.get(&grant).copied()
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

    /// "Add procedure": the choice of a template or a blank one.
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
        self.grants = info
            .map(|info| {
                info.grants
                    .iter()
                    .filter_map(|line| Some((Grant::of(&line.grant)?, Mode::of(&line.mode)?)))
                    .collect()
            })
            .unwrap_or_default();
        self.children = info.is_some_and(|info| info.children);
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
        let grants: Value = Value::Object(
            self.grants
                .iter()
                .map(|(grant, mode)| (grant.word().to_owned(), json!(mode.word())))
                .collect(),
        );
        let name = match &original {
            Some(name) => name.clone(),
            None => fields.name.read(cx).value().trim().to_owned(),
        };
        let (description, instructions) = (
            fields.description.read(cx).value().trim().to_owned(),
            fields.instructions.read(cx).value().trim().to_owned(),
        );
        let children = self.children;
        let write = match original {
            Some(_) => ProcedureWrite::Edit {
                name,
                description,
                instructions,
                grants,
                children,
            },
            None => ProcedureWrite::Add {
                name,
                description,
                instructions,
                grants,
                children,
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

    fn set_grant(&mut self, grant: Grant, mode: Option<Mode>, cx: &mut Context<Self>) {
        match mode {
            Some(mode) => self.grants.insert(grant, mode),
            None => self.grants.remove(&grant),
        };
        cx.notify();
    }

    fn row(&self, info: &ProcedureInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let name = info.name.clone();
        let id = |part: &str| ElementId::Name(format!("{part}-{}", info.name).into());
        let confirming = self.confirming.as_deref() == Some(&info.name);
        let grants = if info.grants.is_empty() {
            "no grants".to_owned()
        } else {
            info.grants
                .iter()
                .map(|line| format!("{} ({})", line.grant, line.mode))
                .collect::<Vec<_>>()
                .join(" · ")
        };
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
                    )
                    .child({
                        let grants = SharedString::from(format!(
                            "{grants}{}",
                            if info.children {
                                " · children too"
                            } else {
                                ""
                            }
                        ));
                        div()
                            .id(id("procedure-grants"))
                            .mono(11.)
                            .text_color(p.faint)
                            .aria_label(grants.clone())
                            .child(grants)
                            .test_support()
                    }),
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
        let modal = Modal::new("procedure-modal", "Procedure")
            .width(px(560.))
            .open(open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        match self.form.clone() {
            Form::Closed => modal,
            Form::Choosing => {
                let mut modal = modal.child(
                    div()
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child("Start from one of Flottille's templates, or write your own."),
                );
                for template in TEMPLATES {
                    modal = modal.child(
                        Button::secondary(
                            ElementId::Name(format!("template-{template}").into()),
                            template,
                        )
                        .full_width()
                        .disabled(self.saving)
                        .on_click(cx.listener(move |this, _, _, cx| this.template(template, cx))),
                    );
                }
                modal
                    .child(
                        Button::quiet("blank-procedure", "Blank procedure")
                            .full_width()
                            .on_click(cx.listener(|this, _, window, cx| this.blank(window, cx))),
                    )
                    .when_some(self.form_error.clone(), |m, error| {
                        m.child(
                            div()
                                .id("procedure-error")
                                .text_size(px(12.5))
                                .text_color(p.fail)
                                .child(error)
                                .test_support(),
                        )
                    })
            }
            Form::Editing(original) => {
                let fields = self.fields.as_ref().expect("made when the form opens");
                let grants = Grant::ALL.into_iter().map(|grant| {
                    let word = grant.word();
                    let mode = self.grants.get(&grant).copied();
                    let choice =
                        |label: &'static str, this: Option<Mode>| {
                            let button = ElementId::Name(
                                format!("grant-{word}-{}", this.map_or("off", Mode::word)).into(),
                            );
                            let button = if mode == this {
                                Button::secondary(button, label)
                            } else {
                                Button::quiet(button, label)
                            };
                            // One width each, so that the columns stay aligned whichever is chosen.
                            div()
                                .w(px(56.))
                                .flex()
                                .justify_center()
                                .child(button.on_click(cx.listener(move |this_, _, _, cx| {
                                    this_.set_grant(grant, this, cx)
                                })))
                        };
                    div()
                        .id(ElementId::Name(format!("grant-{word}").into()))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().mono(12.).child(word))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(p.faint)
                                        .child(grant.says()),
                                ),
                        )
                        .child(choice("off", None))
                        .child(choice("allow", Some(Mode::Allow)))
                        .child(choice("ask", Some(Mode::Ask)))
                        .test_support()
                });
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
                    .child(
                        div()
                            .mt(px(4.))
                            .text_size(px(12.))
                            .semibold()
                            .child("Grants for the agents of a run"),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(p.faint)
                            .child("allow: runs without asking · ask: held for your answer"),
                    )
                    .children(grants)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                Switch::new(
                                    "procedure-children",
                                    self.children,
                                    "Children get the grants",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.children = !this.children;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .child("The agents the run starts get its grants too"),
                            ),
                    )
                    .test_support();
                modal
                    .child(body)
                    .when_some(self.form_error.clone(), |m, error| {
                        m.child(
                            div()
                                .id("procedure-error")
                                .text_size(px(12.5))
                                .text_color(p.fail)
                                .child(error)
                                .test_support(),
                        )
                    })
                    .action(if self.saving {
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
                Button::secondary("add-procedure", "Add procedure")
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

// ── The run modal ──────────────────────────────────────────────────────────────────────────────

/// The modal that runs a procedure in a workspace: the procedure, the workspace, every grant in
/// words, and Confirm. The daemon is asked what the run would grant (`procedure.run`), which
/// starts nothing; `procedure.start` is asked only when the user confirms.
pub struct RunProcedure {
    shell: WeakEntity<Shell>,
    open: bool,
    project: String,
    workspace: String,
    /// The project's procedures; `None` until the daemon has listed them.
    procedures: Option<Vec<ProcedureInfo>>,
    chosen: Option<String>,
    /// What running `chosen` would grant, as the daemon shows it.
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
            procedures: None,
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

    /// The procedure the daemon showed for the choice, with its grants.
    pub fn shown(&self) -> Option<&ProcedureInfo> {
        self.shown.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_starting(&self) -> bool {
        self.starting
    }

    /// Opens the modal on a workspace, nothing running. The shell asks the daemon for the
    /// procedures: it opens the modal while it is being updated itself.
    pub fn open(&mut self, project: &str, workspace: &str, cx: &mut Context<Self>) {
        self.open = true;
        self.project = project.to_owned();
        self.workspace = workspace.to_owned();
        self.procedures = None;
        self.chosen = None;
        self.shown = None;
        self.starting = false;
        self.error = None;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.error = None;
        cx.notify();
    }

    /// The project's procedures: when the modal waits for them, and there is only one, it is the
    /// choice (still to be confirmed).
    pub fn listed(
        &mut self,
        project: &str,
        listed: Result<Vec<ProcedureInfo>, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.open || self.project != project || self.procedures.is_some() {
            return;
        }
        match listed {
            Ok(list) => {
                let only = (list.len() == 1).then(|| list[0].name.clone());
                self.procedures = Some(list);
                if let Some(only) = only {
                    self.choose(&only, cx);
                }
            }
            Err(why) => self.error = Some(why.into()),
        }
        cx.notify();
    }

    /// Chooses a procedure: the daemon says what a run of it would grant. Nothing starts.
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

    /// What a run of the chosen procedure would grant.
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

    /// Confirm: the run starts with the grants that were shown.
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
            grants: shown.grants_json(),
            children: shown.children,
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
            let list = self.procedures.clone().unwrap_or_default();
            if self.procedures.as_ref().is_some_and(Vec::is_empty) {
                modal = modal.child(
                    div()
                        .id("run-no-procedure")
                        .text_size(px(12.5))
                        .text_color(p.faint)
                        .child("This project has no procedure yet: add one on its page.")
                        .test_support(),
                );
            }
            if list.len() > 1 {
                modal = modal.child(div().flex().flex_col().gap(px(4.)).children(
                    list.into_iter().map(|info| {
                        let name = info.name.clone();
                        let chosen = self.chosen.as_deref() == Some(&info.name);
                        let button = ElementId::Name(format!("run-pick-{}", info.name).into());
                        let button = if chosen {
                            Button::secondary(button, info.name.clone())
                        } else {
                            Button::quiet(button, info.name.clone())
                        };
                        button
                            .full_width()
                            .on_click(cx.listener(move |this, _, _, cx| this.choose(&name, cx)))
                    }),
                ));
            }
            if let Some(shown) = &self.shown {
                modal =
                    modal.child(
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
                                    .mt(px(4.))
                                    .text_size(px(12.))
                                    .semibold()
                                    .child("This run is granted"),
                            )
                            .child(
                                div()
                                    .id("run-grants")
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .children(shown.grants.iter().map(|line| {
                                        div()
                                            .id(ElementId::Name(
                                                format!("run-grant-{}", line.grant).into(),
                                            ))
                                            .aria_label(SharedString::from(line.text()))
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .flex()
                                                    .gap(px(8.))
                                                    .child(div().mono(12.).child(
                                                        SharedString::from(line.grant.clone()),
                                                    ))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .text_color(if line.mode == "ask" {
                                                                p.wait
                                                            } else {
                                                                p.mute
                                                            })
                                                            .child(mode_words(&line.mode)),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.5))
                                                    .text_color(p.faint)
                                                    .child(SharedString::from(line.says.clone())),
                                            )
                                            .test_support()
                                    })),
                            )
                            .child(
                                div()
                                    .id("run-children")
                                    .aria_label(children_words(
                                        shown.children,
                                        !shown.grants.is_empty(),
                                    ))
                                    .text_size(px(12.))
                                    .text_color(p.faint)
                                    .child(children_words(shown.children, !shown.grants.is_empty()))
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
    pub grants: Vec<GrantLine>,
    pub children: bool,
}

impl RunFacts {
    /// From `procedure.of`'s `run`; `None` when there is none, or it is over.
    pub fn of(run: &Value) -> Option<Self> {
        if run.is_null() || run["ended"] == true {
            return None;
        }
        let info = ProcedureInfo::of(run);
        Some(Self {
            procedure: run["procedure"].as_str().unwrap_or_default().to_owned(),
            grants: info.grants,
            children: info.children,
        })
    }
}

/// The grants of the run, in the WORKSPACE section of the rail: the procedure, each grant in
/// words, and whether the children get them.
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
        .children(run.grants.iter().map(|line| {
            div()
                .id(ElementId::Name(
                    format!("rail-run-grant-{}", line.grant).into(),
                ))
                .aria_label(SharedString::from(format!(
                    "{}: {}",
                    line.grant,
                    mode_words(&line.mode)
                )))
                .flex()
                .gap(px(6.))
                .mono(11.)
                .text_color(p.mute)
                .child(SharedString::from(line.grant.clone()))
                .child(
                    div()
                        .text_color(if line.mode == "ask" { p.wait } else { p.faint })
                        .child(mode_words(&line.mode)),
                )
                .test_support()
        }))
        .child(
            div()
                .id("rail-run-children")
                .text_size(px(11.))
                .text_color(p.faint)
                .child(children_words(run.children, !run.grants.is_empty()))
                .test_support(),
        )
        .test_support()
}
