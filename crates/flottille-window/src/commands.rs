//! A project's commands in its Settings (`docs/commands.md`): named instructions that `/name`
//! sends to the session's agent. The window writes nothing itself: every add, edit and removal is
//! the daemon's (`command.*`), and the section shows the list the daemon gives next. The
//! "Commands" section has the modal that adds one (from the list of templates, or blank) or edits
//! one.

use std::collections::HashMap;

use flottille_core::commands::TEMPLATES;
use flottille_ui::{
    Button, Confirm, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal, Section,
    Toast, Type, icon, palette, presence, presence_of, space, toast,
};
use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState, TextareaState};
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::{ClaudeKind, Command, CommandWrite};
use crate::claude::{Found, found_block};
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
        .px(space::MD)
        .py(space::XS)
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

/// A line that says what went wrong, in the alarm colour: it unfolds when there is one and folds
/// away when it is cleared. `id` is the line's; `gap` is what the parent puts between its children.
pub(crate) fn problem_line(
    id: &'static str,
    problem: Option<SharedString>,
    gap: Pixels,
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
    .gap(gap)
}

/// The "⋯" menu of a row of a list, made once per row and kept: its button is
/// `<prefix>-more-<key>`. `make` gets the key and the empty menu.
pub(crate) fn menu_of<V: 'static>(
    menus: &mut HashMap<String, Entity<Menu>>,
    prefix: &str,
    key: &str,
    cx: &mut Context<V>,
    make: impl FnOnce(&str, Menu) -> Menu,
) -> Entity<Menu> {
    if let Some(menu) = menus.get(key) {
        return menu.clone();
    }
    let id = ElementId::Name(format!("{prefix}-more-{key}").into());
    let menu = cx.new(|cx| make(key, Menu::new(id, "More", cx)));
    menus.insert(key.to_owned(), menu.clone());
    menu
}

// ── The section ────────────────────────────────────────────────────────────────────────────────

/// What the section says a command is.
pub const SENTENCE: &str = "Instructions you send to a session by typing /name in its composer, \
                            so you never type them again.";

/// What the dialog says a command is, before its templates.
pub const EXPLAINED: &str = "A command is instructions you send to a session by typing /name in \
                             its composer. Start from one of these, or from nothing; you can edit \
                             it before saving.";

/// What the dialog is doing.
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

/// What a write was, for the toast its answer says.
#[derive(Clone, Debug, PartialEq)]
enum Writing {
    Added(String),
    Saved(String),
    Removed(String),
}

pub struct CommandsSection {
    shell: WeakEntity<Shell>,
    project: String,
    listed: Option<Vec<CommandInfo>>,
    /// The rows as drawn: a removed command folds away before it is gone.
    rows: Listing<CommandInfo>,
    menus: HashMap<String, Entity<Menu>>,
    /// Why the list cannot be read.
    problem: Option<SharedString>,
    form: Form,
    /// Made when the dialog first opens, which is when a window is at hand.
    fields: Option<Fields>,
    saving: bool,
    writing: Option<Writing>,
    form_error: Option<SharedString>,
    /// The command being read to be edited.
    reading: Option<String>,
    confirming: Option<String>,
    /// The command the confirmation names, kept while it fades away.
    asked: String,
    /// What the project's `.claude/commands` holds, as the daemon last found it.
    found: Option<Vec<Found>>,
    found_rows: Listing<Found>,
    importing: bool,
    _subscriptions: Vec<Subscription>,
}

impl CommandsSection {
    pub fn new(shell: WeakEntity<Shell>, project: String) -> Self {
        Self {
            shell,
            project,
            listed: None,
            rows: Listing::default(),
            menus: HashMap::new(),
            problem: None,
            form: Form::Closed,
            fields: None,
            saving: false,
            writing: None,
            form_error: None,
            reading: None,
            confirming: None,
            asked: String::new(),
            found: None,
            found_rows: Listing::default(),
            importing: false,
            _subscriptions: Vec::new(),
        }
    }

    /// The commands as last listed; `None` until the daemon has said.
    pub fn commands(&self) -> Option<&[CommandInfo]> {
        self.listed.as_deref()
    }

    /// What `.claude/commands` holds, as the daemon last found it.
    pub fn found(&self) -> Option<&[Found]> {
        self.found.as_deref()
    }

    pub fn is_open(&self) -> bool {
        self.form != Form::Closed
    }

    pub fn is_confirming(&self) -> bool {
        self.confirming.is_some()
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

    /// Asks the daemon for the list, and for what `.claude/commands` holds.
    pub fn read(&self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        perform_later(
            &self.shell,
            Command::Commands {
                project: project.clone(),
            },
            cx,
        );
        let kind = ClaudeKind::Command;
        perform_later(&self.shell, Command::ClaudeFound { project, kind }, cx);
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

    /// What the daemon found in `.claude/commands`. A folder that cannot be read offers nothing.
    pub fn found_listed(&mut self, found: Result<Vec<Found>, String>, cx: &mut Context<Self>) {
        let found = found.unwrap_or_default();
        if self.found.as_ref() != Some(&found) {
            self.found = Some(found);
            cx.notify();
        }
    }

    /// Imports `name` from `.claude/commands`, or all that can be.
    pub fn import(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.importing {
            return;
        }
        self.importing = true;
        let command = Command::ClaudeImport {
            project: self.project.clone(),
            kind: ClaudeKind::Command,
            names: name.map(|name| vec![name]),
        };
        perform_later(&self.shell, command, cx);
        cx.notify();
    }

    /// The daemon's answer to an import: a toast says it, and the lists are read again.
    pub fn imported(&mut self, read: Result<Value, String>, cx: &mut Context<Self>) {
        self.importing = false;
        toast(
            crate::claude::imported_toast(ClaudeKind::Command, &read),
            cx,
        );
        self.read(cx);
        cx.notify();
    }

    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        if self.fields.is_none() {
            let name = line_field("open-prs", window, cx);
            let description = line_field("Open the workspace's pull requests", window, cx);
            let instructions = cx.new(|cx| {
                let mut field = TextareaState::new(window, cx)
                    .auto_grow(4, 10)
                    .placeholder("What the agent is sent");
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

    /// "New command": what a command is, the templates, and a blank one.
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
                toast(
                    Toast::failure(format!("The command {name} cannot be read")).detail(why),
                    cx,
                );
                cx.notify();
            }
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.form = Form::Closed;
        self.form_error = None;
        cx.notify();
    }

    fn write(&mut self, write: CommandWrite, writing: Writing, cx: &mut Context<Self>) {
        self.saving = true;
        self.writing = Some(writing);
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
            Writing::Added(template.to_owned()),
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
        let (write, writing) = match original {
            Some(_) => (
                CommandWrite::Edit {
                    name: name.clone(),
                    description,
                    instructions,
                },
                Writing::Saved(name),
            ),
            None => (
                CommandWrite::Add {
                    name: name.clone(),
                    description,
                    instructions,
                },
                Writing::Added(name),
            ),
        };
        self.write(write, writing, cx);
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
        self.write(
            CommandWrite::Remove {
                name: name.to_owned(),
            },
            Writing::Removed(name.to_owned()),
            cx,
        );
    }

    /// The daemon's answer to a write: a toast says it, and the list is read again; a refusal of
    /// the dialog's stays in it.
    pub fn written(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.saving = false;
        let writing = self.writing.take();
        match refused {
            None => {
                self.form = Form::Closed;
                self.form_error = None;
                let said = match writing {
                    Some(Writing::Added(name)) => Toast::success(format!("Command {name} added"))
                        .detail(format!("Type /{name} in a session's composer to send it.")),
                    Some(Writing::Saved(name)) => Toast::success(format!("Command {name} saved")),
                    Some(Writing::Removed(name)) => {
                        Toast::success(format!("Command {name} removed"))
                    }
                    None => Toast::success("Commands saved"),
                };
                toast(said, cx);
                self.read(cx);
            }
            Some(why) if self.form == Form::Closed => toast(
                Toast::failure("The command was not removed").detail(why),
                cx,
            ),
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    fn menu(&mut self, name: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let this = cx.entity().downgrade();
        menu_of(&mut self.menus, "command", name, cx, move |name, menu| {
            let (edit, remove) = (this.clone(), this.clone());
            let (to_edit, to_remove) = (name.to_owned(), name.to_owned());
            menu.entry(
                format!("edit-command-{name}"),
                "Edit",
                None,
                move |_, cx| {
                    edit.update(cx, |this, cx| this.edit(&to_edit, cx)).ok();
                },
            )
            .danger(
                format!("remove-command-{name}"),
                "Remove…",
                move |_, cx| {
                    remove
                        .update(cx, |this, cx| this.remove(&to_remove, cx))
                        .ok();
                },
            )
        })
    }

    fn row(&mut self, info: &CommandInfo, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let name = info.name.clone();
        let menu = self.menu(&name, cx);
        ListRow::new(
            ElementId::Name(format!("command-{name}").into()),
            SharedString::from(format!("/{name}")),
        )
        .mono()
        .leading(icon(Icon::Slash).size(px(14.)).text_color(p.faint))
        .line(SharedString::from(info.description.clone()))
        .more(menu)
        .on_click(cx.listener(move |this, _, _, cx| {
            if this.reading.is_none() {
                this.edit(&name, cx)
            }
        }))
    }

    /// The confirmation of a removal.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let name = self.asked.clone();
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("command-removal", format!("Remove the command {name}?"))
            .sentence(format!(
                "Typing /{name} in a composer sends it as text from now on."
            ))
            .action("Remove command")
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

    fn modal(&mut self, window: &Window, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let open = self.form != Form::Closed;
        let title = match &self.form {
            Form::Editing(Some(name)) => format!("Command /{name}"),
            _ => "New command".to_owned(),
        };
        let modal = Modal::new("command-modal", title)
            .open(open)
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let error = problem_line("command-error", self.form_error.clone(), space::LG, cx);
        match self.form.clone() {
            Form::Closed => modal,
            Form::Choosing => {
                let templates = TEMPLATES.into_iter().map(|template| {
                    let shown = flottille_core::commands::template(template);
                    let description = shown
                        .as_ref()
                        .map(|t| t.description.clone())
                        .unwrap_or_default();
                    ListRow::new(
                        ElementId::Name(format!("template-{template}").into()),
                        format!("/{template}"),
                    )
                    .mono()
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
                    .line(
                        "A name, one line on what it does, and the instructions the agent is sent.",
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.blank(window, cx)));
                modal
                    .sentence(EXPLAINED)
                    .child(
                        div()
                            .id("command-templates")
                            .flex()
                            .flex_col()
                            .gap(space::XS)
                            .children(templates)
                            .child(blank)
                            .test_support(),
                    )
                    .child(error)
            }
            Form::Editing(original) => {
                let fields = self.fields.as_ref().expect("made when the form opens");
                let name = match &original {
                    Some(name) => div()
                        .id("command-name")
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .mono(12.5)
                        .text_color(p.mute)
                        .child(SharedString::from(format!("/{name}")))
                        .test_support()
                        .into_any_element(),
                    None => boxed("command-name", Input::new(&fields.name), cx).into_any_element(),
                };
                let body = div()
                    .id("command-form")
                    .max_h(window.viewport_size().height - px(240.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(space::LG)
                    .child(
                        Field::new("command-name-field", "Name", name)
                            .hint("Typed after / in a session's composer."),
                    )
                    .child(Field::new(
                        "command-description",
                        "What it does",
                        boxed(
                            "command-description-field",
                            Input::new(&fields.description),
                            cx,
                        ),
                    ))
                    .child(Field::new(
                        "command-instructions-field",
                        "Instructions",
                        div()
                            .id("command-instructions")
                            .min_h(px(96.))
                            .px(space::MD)
                            .py(space::SM)
                            .rounded(px(8.))
                            .bg(p.field)
                            .border_1()
                            .border_color(p.line2)
                            .text_size(px(12.5))
                            .child(fields.instructions.clone())
                            .test_support(),
                    ))
                    .test_support();
                modal
                    .sentence("Instructions you send to a session by typing /name in its composer.")
                    .child(body)
                    .child(error)
                    .action(
                        Button::primary(
                            "save-command",
                            if original.is_some() {
                                "Save"
                            } else {
                                "Add command"
                            },
                        )
                        .disabled(self.saving)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
            }
        }
    }
}

impl Render for CommandsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows
                .sync(listed.into_iter().map(|c| (c.name.clone(), c)), now, reduce);
        }
        crate::claude::sync(&mut self.found_rows, self.found.as_deref(), cx);
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("command-list");
        for (key, placement, info) in drawn {
            let row = self.row(&info, cx);
            list = list.item(
                ElementId::Name(format!("command-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(window, cx);
        let this = cx.entity().downgrade();
        let found = found_block(
            "command",
            "commands",
            &self.found_rows,
            self.importing,
            move |name, _, cx| {
                this.update(cx, |this, cx| this.import(name, cx)).ok();
            },
            cx,
        );
        let section = Section::new("commands", "Commands")
            .count(count.to_string())
            .sentence(SENTENCE)
            .action(
                Button::secondary("add-command", "New command")
                    .leading(Icon::Plus)
                    .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
            )
            .child(presence("command-list-block", count > 0, list).gap(space::MD))
            .child(
                presence(
                    "no-commands-block",
                    empty,
                    EmptyLine::new(
                        "no-commands",
                        Icon::Slash,
                        "No command yet. Start from a template (open the pull requests, merge a \
                         stack, close a workspace) or write your own.",
                    ),
                )
                .gap(space::MD),
            )
            .child(found)
            .child(problem_line(
                "commands-problem",
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
