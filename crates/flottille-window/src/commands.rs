//! A project's commands in its Settings (`docs/commands.md`): named instructions that `/name`
//! sends to the session's agent. The window writes nothing itself: every add, edit and removal is
//! the daemon's (`command.*`), and the section shows the list the daemon gives next. The
//! "Commands" section has the modal that adds one (from the list of templates, or blank) or edits
//! one.

use flottille_core::commands::TEMPLATES;
use flottille_ui::{
    Button, Icon, Item, ListRow, Listing, Modal, Type, enter, icon, palette, presence, presence_of,
};
use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState, TextareaState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _, WeakEntity, Window, div,
    px,
};
use serde_json::Value;

use crate::actions::{Command, CommandWrite};
use crate::shell::Shell;

/// A command as the daemon shows it (`docs/protocol.md`, "The command methods").
#[derive(Clone, Debug, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    /// Empty unless the daemon gave it whole (`command.show`).
    pub instructions: String,
}

impl CommandInfo {
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

/// A section's line saying there is nothing yet: it unfolds when the list empties and folds away
/// when a row comes. `id` is the line's, for the tests.
pub(crate) fn empty_note(
    id: &'static str,
    empty: bool,
    text: &'static str,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    presence(
        format!("{id}-block"),
        empty,
        div()
            .id(id)
            .text_size(px(12.5))
            .text_color(p.faint)
            .child(text)
            .test_support(),
    )
    .gap(px(8.))
}

/// A line that says what went wrong, in the alarm colour: it unfolds when there is one and folds
/// away when it is cleared. `id` is the line's; `gap` is what the parent puts between its children.
pub(crate) fn problem_line(
    id: &'static str,
    problem: Option<SharedString>,
    gap: f32,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    presence_of(format!("{id}-block"), problem, move |problem| {
        div()
            .id(id)
            .text_size(px(12.5))
            .text_color(p.fail)
            .child(problem)
            .test_support()
            .into_any_element()
    })
    .gap(px(gap))
}

/// What a modal's action holds: "Saving…" while the daemon works, else `save`.
pub(crate) fn save_or_saving(
    id: &'static str,
    saving: bool,
    save: impl FnOnce() -> Button,
    cx: &App,
) -> AnyElement {
    let p = palette(cx);
    if saving {
        enter(
            format!("{id}-saving"),
            0,
            div().mono(11.5).text_color(p.faint).child("Saving…"),
        )
        .into_any_element()
    } else {
        save().into_any_element()
    }
}

/// The heading of a section of a project's pages.
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
    /// A command written out: a new one, or the one named.
    Editing(Option<String>),
}

struct Fields {
    name: Entity<InputState>,
    description: Entity<InputState>,
    instructions: Entity<TextareaState>,
}

pub struct CommandsSection {
    shell: WeakEntity<Shell>,
    project: String,
    listed: Option<Vec<CommandInfo>>,
    /// The rows as drawn: a removed command folds away before it is gone.
    rows: Listing<CommandInfo>,
    /// What the daemon said when the list could not be read, or a removal was refused.
    problem: Option<SharedString>,
    form: Form,
    /// Made when the modal first opens, which is when a window is at hand.
    fields: Option<Fields>,
    saving: bool,
    form_error: Option<SharedString>,
    /// The command being read to be edited.
    reading: Option<String>,
    confirming: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl CommandsSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            listed: None,
            rows: Listing::default(),
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

    /// The commands as last listed; `None` until the daemon has said.
    pub fn commands(&self) -> Option<&[CommandInfo]> {
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
        perform_later(&self.shell, Command::Commands { project }, cx);
    }

    /// The daemon's list.
    pub fn listed(&mut self, listed: Result<Vec<CommandInfo>, String>, cx: &mut Context<Self>) {
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
            let name = line_field("name, typed after / to send it: open-prs", window, cx);
            let description = line_field("one line: what it does", window, cx);
            let instructions = cx.new(|cx| {
                let mut field = TextareaState::new(window, cx)
                    .auto_grow(4, 10)
                    .placeholder("what the agent is sent");
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

    /// "New command": the list of the templates, and a blank one.
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
    fn fill(&mut self, info: Option<&CommandInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields(window, cx);
        let (name, description, instructions) = (
            fields.name.clone(),
            fields.description.clone(),
            fields.instructions.clone(),
        );
        let text = |f: fn(&CommandInfo) -> &str| info.map(f).unwrap_or_default().to_owned();
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

    /// Reads the command whole, then opens it for editing.
    pub fn edit(&mut self, name: &str, cx: &mut Context<Self>) {
        self.reading = Some(name.to_owned());
        self.problem = None;
        let command = Command::CommandRead {
            project: self.project.clone(),
            name: name.to_owned(),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's reading of the command to edit.
    pub fn read_whole(
        &mut self,
        name: &str,
        read: Result<CommandInfo, String>,
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

    fn write(&mut self, write: CommandWrite, cx: &mut Context<Self>) {
        self.saving = true;
        self.form_error = None;
        self.problem = None;
        let command = Command::CommandWrite {
            project: self.project.clone(),
            write,
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    fn template(&mut self, template: &str, cx: &mut Context<Self>) {
        self.write(
            CommandWrite::Template {
                template: template.to_owned(),
            },
            cx,
        );
    }

    /// Saves the form: a new command, or the one being edited.
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
            Some(_) => CommandWrite::Edit {
                name,
                description,
                instructions,
            },
            None => CommandWrite::Add {
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
            CommandWrite::Remove {
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

    fn row(&self, info: &CommandInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let name = info.name.clone();
        let id = |part: &str| ElementId::Name(format!("{part}-{}", info.name).into());
        let confirming = self.confirming.as_deref() == Some(&info.name);
        div()
            .id(id("command"))
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
                            .id(id("command-description"))
                            .text_size(px(12.5))
                            .text_color(p.mute)
                            .aria_label(SharedString::from(info.description.clone()))
                            .child(SharedString::from(info.description.clone()))
                            .test_support(),
                    ),
            )
            .child({
                let edit = name.clone();
                Button::quiet(id("edit-command"), "Edit")
                    .disabled(self.reading.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| this.edit(&edit, cx)))
            })
            .child(if confirming {
                let (cancel, remove) = (name.clone(), name.clone());
                let group = div()
                    .flex()
                    .gap(px(4.))
                    .child(
                        Button::quiet(id("cancel-remove-command"), "Cancel").on_click(cx.listener(
                            move |this, _, _, cx| {
                                if this.confirming.as_deref() == Some(&cancel) {
                                    this.confirming = None;
                                    cx.notify();
                                }
                            },
                        )),
                    )
                    .child(
                        Button::secondary(id("confirm-remove-command"), format!("Remove {name}"))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx))),
                    );
                enter(id("confirm-command"), 0, group).into_any_element()
            } else {
                let remove = name.clone();
                Button::quiet(id("remove-command"), "Remove")
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
            Form::Editing(Some(_)) => "Edit command",
            _ => "New command",
        };
        let modal = Modal::new("command-modal", title)
            .width(px(600.))
            .open(open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let error = problem_line("command-error", self.form_error.clone(), 14., cx);
        match self.form.clone() {
            Form::Closed => modal,
            Form::Choosing => {
                let templates = TEMPLATES.into_iter().map(|template| {
                    let description = flottille_core::commands::template(template)
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
                let blank = ListRow::new("blank-command", "Blank command")
                    .leading(icon(Icon::Plus).size(px(14.)).text_color(p.faint))
                    .line("Write your own: a name, what it does, and what the agent is sent")
                    .on_click(cx.listener(|this, _, window, cx| this.blank(window, cx)));
                modal
                    .child(
                        div()
                            .id("command-templates")
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(templates)
                            .child(blank)
                            .test_support(),
                    )
                    .child(error)
            }
            Form::Editing(original) => {
                let fields = self.fields.as_ref().expect("made when the form opens");
                let body = div()
                    .id("command-form")
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
                            boxed("command-name", Input::new(&fields.name), cx).into_any_element()
                        }
                    })
                    .child(boxed(
                        "command-description-field",
                        Input::new(&fields.description),
                        cx,
                    ))
                    .child(
                        div()
                            .id("command-instructions")
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
                modal.child(body).child(error).action(save_or_saving(
                    "command",
                    self.saving,
                    || {
                        Button::primary("save-command", "Save")
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    },
                    cx,
                ))
            }
        }
    }
}

impl Render for CommandsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows
                .sync(listed.into_iter().map(|c| (c.name.clone(), c)), now, reduce);
        }
        let rows: Vec<_> = self
            .rows
            .rows()
            .map(|row| {
                Item::new(
                    ElementId::Name(format!("command-item-{}", row.key).into()),
                    row.placement,
                    self.row(row.value, cx),
                )
                .gap(px(8.))
            })
            .collect();
        let count = self.rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(window, cx);
        div()
            .id("commands")
            .child(heading(
                "Commands",
                count,
                Button::secondary("add-command", "New command")
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
                    .child(empty_note(
                        "no-commands",
                        empty,
                        "No command yet: add one from a template to open the pull requests of \
                         a workspace, merge a stack or close it. /name in a session's composer \
                         sends it to the agent.",
                        cx,
                    ))
                    .child(problem_line(
                        "commands-problem",
                        self.problem.clone(),
                        8.,
                        cx,
                    )),
            )
            .child(modal)
    }
}
