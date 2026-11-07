//! The vault in Settings (`docs/vault.md`): the user's secrets, kept by the keyring, that agents
//! use through Flottille's tools and never see. The page lists the entries' metadata; a secret is
//! typed in a masked field, sent to the daemon (`vault.set`) and cleared from the field at once,
//! and never shown again: replacing an entry means typing its secret anew. The window writes
//! nothing itself, and shows the list the daemon gives next.

use std::collections::HashMap;

use flottille_ui::{
    Button, Confirm, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal, Section,
    Select, SelectOption, Toast, icon, palette, presence, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::{Command, VaultWrite};
use crate::commands::{boxed, line_field, menu_of, perform_later, problem_line};
use crate::shell::Shell;

/// What the section says the vault is for.
pub const SENTENCE: &str = "Secrets agents use without seeing them: a tracker's token, a test \
                            account's login. Your keyring keeps them; agents see the names and \
                            what they are for, and a tool of Flottille's puts the secret where it \
                            goes.";

/// What the dialog says of the secret, under its field.
pub const SECRET_HINT: &str = "Kept by your keyring and never shown again, here or anywhere. \
                               Replacing the entry means typing it again.";

/// What the project select calls no project.
pub const EVERY_PROJECT: &str = "Every project";

/// An entry as the daemon lists it: its metadata, never its secret.
#[derive(Clone, Debug, PartialEq)]
pub struct VaultEntry {
    pub name: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub username: Option<String>,
    pub project: Option<String>,
}

impl VaultEntry {
    pub fn of(value: &Value) -> Self {
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        Self {
            name: text("name").unwrap_or_default(),
            description: text("description"),
            url: text("url"),
            username: text("username"),
            project: text("project"),
        }
    }
}

/// What the dialog is doing.
#[derive(Clone, Debug, PartialEq)]
enum Form {
    Closed,
    /// A new entry, or the one named replaced whole.
    Editing(Option<String>),
}

struct Fields {
    name: Entity<InputState>,
    secret: Entity<InputState>,
    description: Entity<InputState>,
    url: Entity<InputState>,
    username: Entity<InputState>,
}

/// What a write was, for the toast its answer says.
#[derive(Clone, Debug, PartialEq)]
enum Writing {
    Added(String),
    Replaced(String),
    Removed(String),
}

pub struct VaultSection {
    shell: WeakEntity<Shell>,
    listed: Option<Vec<VaultEntry>>,
    /// The rows as drawn: a removed entry folds away before it is gone.
    rows: Listing<VaultEntry>,
    menus: HashMap<String, Entity<Menu>>,
    /// Why the list cannot be read.
    problem: Option<SharedString>,
    form: Form,
    /// Made when the dialog first opens, which is when a window is at hand.
    fields: Option<Fields>,
    /// The project the entry is for, chosen in the dialog; every project's when `None`.
    project: Option<String>,
    saving: bool,
    writing: Option<Writing>,
    form_error: Option<SharedString>,
    confirming: Option<String>,
    /// The entry the confirmation names, kept while it fades away.
    asked: String,
    _subscriptions: Vec<Subscription>,
}

impl VaultSection {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            listed: None,
            rows: Listing::default(),
            menus: HashMap::new(),
            problem: None,
            form: Form::Closed,
            fields: None,
            project: None,
            saving: false,
            writing: None,
            form_error: None,
            confirming: None,
            asked: String::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// The entries as last listed; `None` until the daemon has said.
    pub fn entries(&self) -> Option<&[VaultEntry]> {
        self.listed.as_deref()
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

    pub fn name_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.name)
    }

    pub fn secret_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.secret)
    }

    pub fn description_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.description)
    }

    /// Asks the daemon for the list.
    pub fn read(&self, cx: &mut Context<Self>) {
        perform_later(&self.shell, Command::Vault, cx);
    }

    /// The daemon's list.
    pub fn listed(&mut self, listed: Result<Vec<VaultEntry>, String>, cx: &mut Context<Self>) {
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
            let name = line_field("jira", window, cx);
            let secret = cx.new(|cx| {
                let mut field = InputState::new(window, cx).masked(true);
                flottille_ui::follow_theme(&mut field, cx);
                field
            });
            let description = line_field("Logs time on Jira", window, cx);
            let url = line_field("https://tracker.example.com", window, cx);
            let username = line_field("me@example.com", window, cx);
            // Typing clears what a refusal said.
            for input in [&name, &secret, &description, &url, &username] {
                self._subscriptions
                    .push(cx.subscribe(input, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) && this.form_error.take().is_some() {
                            cx.notify();
                        }
                    }));
            }
            self.fields = Some(Fields {
                name,
                secret,
                description,
                url,
                username,
            });
        }
        self.fields.as_ref().expect("made above")
    }

    /// "New entry": the dialog, empty.
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fill(None, window, cx);
    }

    /// "Replace": the dialog on the entry's metadata, its secret to type again.
    pub fn replace(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self
            .listed
            .iter()
            .flatten()
            .find(|entry| entry.name == name)
            .cloned();
        if let Some(entry) = entry {
            self.fill(Some(&entry), window, cx);
        }
    }

    fn fill(&mut self, entry: Option<&VaultEntry>, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields(window, cx);
        let (name, secret, description, url, username) = (
            fields.name.clone(),
            fields.secret.clone(),
            fields.description.clone(),
            fields.url.clone(),
            fields.username.clone(),
        );
        let text =
            |f: fn(&VaultEntry) -> Option<&String>| entry.and_then(f).cloned().unwrap_or_default();
        for (field, value) in [
            (&name, entry.map(|e| e.name.clone()).unwrap_or_default()),
            (&secret, String::new()),
            (&description, text(|e| e.description.as_ref())),
            (&url, text(|e| e.url.as_ref())),
            (&username, text(|e| e.username.as_ref())),
        ] {
            field.update(cx, |f, cx| f.set_value(value, window, cx));
        }
        self.project = entry.and_then(|entry| entry.project.clone());
        self.form = Form::Editing(entry.map(|entry| entry.name.clone()));
        self.form_error = None;
        self.saving = false;
        let focus = if entry.is_some() { secret } else { name }
            .read(cx)
            .focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = Form::Closed;
        self.form_error = None;
        self.clear_secret(window, cx);
        cx.notify();
    }

    /// The secret leaves the field as soon as it is sent, or the dialog closed.
    fn clear_secret(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(fields) = &self.fields {
            fields
                .secret
                .update(cx, |f, cx| f.set_value("", window, cx));
        }
    }

    fn write(&mut self, write: VaultWrite, writing: Writing, cx: &mut Context<Self>) {
        self.saving = true;
        self.writing = Some(writing);
        self.form_error = None;
        self.problem = None;
        perform_later(&self.shell, Command::VaultWrite(write), cx);
        cx.notify();
    }

    /// Saves the dialog: a new entry, or the one replaced.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Form::Editing(original) = self.form.clone() else {
            return;
        };
        if self.saving {
            return;
        }
        let Some(fields) = &self.fields else {
            return;
        };
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let name = original.clone().unwrap_or_else(|| value(&fields.name));
        // The secret as typed: spaces may be part of it.
        let secret = fields.secret.read(cx).value().to_string();
        if name.is_empty() || secret.is_empty() {
            self.form_error = Some("An entry needs a name and a secret.".into());
            cx.notify();
            return;
        }
        let write = VaultWrite::Set {
            name: name.clone(),
            secret,
            description: value(&fields.description),
            url: value(&fields.url),
            username: value(&fields.username),
            project: self.project.clone(),
        };
        let writing = match original {
            Some(_) => Writing::Replaced(name),
            None => Writing::Added(name),
        };
        self.clear_secret(window, cx);
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
            VaultWrite::Remove {
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
                    Some(Writing::Added(name)) => Toast::success(format!("Entry {name} added"))
                        .detail("Its secret is in your keyring."),
                    Some(Writing::Replaced(name)) => {
                        Toast::success(format!("Entry {name} replaced"))
                    }
                    Some(Writing::Removed(name)) => Toast::success(format!("Entry {name} removed"))
                        .detail("Its secret is gone from your keyring."),
                    None => Toast::success("Vault saved"),
                };
                toast(said, cx);
                self.read(cx);
            }
            Some(why) if self.form == Form::Closed => {
                toast(Toast::failure("The entry was not removed").detail(why), cx)
            }
            Some(why) => self.form_error = Some(why.into()),
        }
        cx.notify();
    }

    fn menu(&mut self, name: &str, cx: &mut Context<Self>) -> Entity<Menu> {
        let this = cx.entity().downgrade();
        menu_of(&mut self.menus, "vault", name, cx, move |name, menu| {
            let (replace, remove) = (this.clone(), this.clone());
            let (to_replace, to_remove) = (name.to_owned(), name.to_owned());
            menu.entry(
                format!("replace-vault-{name}"),
                "Replace",
                None,
                move |window, cx| {
                    replace
                        .update(cx, |this, cx| this.replace(&to_replace, window, cx))
                        .ok();
                },
            )
            .danger(format!("remove-vault-{name}"), "Remove…", move |_, cx| {
                remove
                    .update(cx, |this, cx| this.remove(&to_remove, cx))
                    .ok();
            })
        })
    }

    fn row(&mut self, entry: &VaultEntry, cx: &mut Context<Self>) -> ListRow {
        let p = palette(cx);
        let name = entry.name.clone();
        let menu = self.menu(&name, cx);
        let line = [
            entry.description.clone(),
            entry.url.clone(),
            entry.username.clone(),
        ]
        .into_iter()
        .flatten()
        .next()
        .unwrap_or_else(|| "No description".to_owned());
        let row = ListRow::new(
            ElementId::Name(format!("vault-{name}").into()),
            SharedString::from(name.clone()),
        )
        .mono()
        .leading(icon(Icon::Key).size(px(14.)).text_color(p.faint))
        .tag(SharedString::from(
            entry
                .project
                .clone()
                .unwrap_or_else(|| EVERY_PROJECT.to_owned()),
        ))
        .line(SharedString::from(line))
        .more(menu);
        row.on_click(cx.listener(move |this, _, window, cx| {
            if !this.saving {
                this.replace(&name, window, cx)
            }
        }))
    }

    /// The confirmation of a removal.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let name = self.asked.clone();
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("vault-removal", format!("Remove the entry {name}?"))
            .sentence(
                "Its secret is deleted from your keyring, and the tools that use it can no longer.",
            )
            .action("Remove entry")
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

    fn project_select(&self, cx: &mut Context<Self>) -> Select {
        let mut names: Vec<String> = self
            .shell
            .upgrade()
            .map(|shell| {
                shell
                    .read(cx)
                    .snapshot()
                    .projects
                    .iter()
                    .map(|project| project.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        // A project that is gone still names the entry's own.
        if let Some(own) = &self.project
            && !names.contains(own)
        {
            names.push(own.clone());
        }
        let mut choices: Vec<Option<String>> = vec![None];
        choices.extend(names.into_iter().map(Some));
        let options = choices.iter().map(|choice| match choice {
            None => {
                SelectOption::new("every", EVERY_PROJECT).line("Every project's agents see it.")
            }
            Some(name) => SelectOption::new(
                SharedString::from(name.clone()),
                SharedString::from(name.clone()),
            )
            .line("Only this project's agents see it."),
        });
        let chosen = choices
            .iter()
            .position(|choice| *choice == self.project)
            .unwrap_or(0);
        let this = cx.entity().downgrade();
        Select::new("vault-project", options, chosen)
            .disabled(self.saving)
            .on_choose(move |index, _, cx| {
                let project = choices[index].clone();
                this.update(cx, |this, cx| {
                    this.project = project;
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
            Form::Editing(Some(name)) => format!("Replace {name}"),
            _ => "New entry".to_owned(),
        };
        let modal = Modal::new("vault-modal", title)
            .open(open)
            .on_close(move |window, cx| {
                this.update(cx, |this, cx| this.close(window, cx)).ok();
            });
        let Form::Editing(original) = self.form.clone() else {
            return modal;
        };
        let select = self.project_select(cx);
        let error = problem_line("vault-error", self.form_error.clone(), space::LG, cx);
        let fields = self.fields.as_ref().expect("made when the form opens");
        let name = match &original {
            Some(name) => div()
                .id("vault-name")
                .h(px(32.))
                .flex()
                .items_center()
                .text_color(p.mute)
                .child(SharedString::from(name.clone()))
                .test_support()
                .into_any_element(),
            None => boxed("vault-name", Input::new(&fields.name), cx).into_any_element(),
        };
        let body = div()
            .id("vault-form")
            .max_h(window.viewport_size().height - px(240.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(
                Field::new("vault-name-field", "Name", name)
                    .hint("What agents give a tool that needs the secret."),
            )
            .child(
                Field::new(
                    "vault-secret-field",
                    "Secret",
                    boxed("vault-secret", Input::new(&fields.secret), cx),
                )
                .hint(SECRET_HINT),
            )
            .child(
                Field::new(
                    "vault-description-field",
                    "What it is for",
                    boxed("vault-description", Input::new(&fields.description), cx),
                )
                .optional(),
            )
            .child(
                Field::new(
                    "vault-url-field",
                    "URL",
                    boxed("vault-url", Input::new(&fields.url), cx),
                )
                .optional(),
            )
            .child(
                Field::new(
                    "vault-username-field",
                    "Username",
                    boxed("vault-username", Input::new(&fields.username), cx),
                )
                .optional(),
            )
            .child(Field::new("vault-project-field", "Seen by", select))
            .test_support();
        modal
            .sentence(
                "Agents see its name, what it is for, its URL and its username; never its secret.",
            )
            .child(body)
            .child(error)
            .action(
                Button::primary(
                    "save-vault-entry",
                    if original.is_some() {
                        "Replace"
                    } else {
                        "Add entry"
                    },
                )
                .disabled(self.saving)
                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
    }
}

impl Render for VaultSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if let Some(listed) = self.listed.clone() {
            let now = cx.background_executor().now();
            let reduce = cx.reduce_motion();
            self.rows
                .sync(listed.into_iter().map(|e| (e.name.clone(), e)), now, reduce);
        }
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.key.to_owned(), row.placement, row.value.clone()))
            .collect();
        let mut list = ListCard::new("vault-list");
        for (key, placement, entry) in drawn {
            let row = self.row(&entry, cx);
            list = list.item(
                ElementId::Name(format!("vault-item-{key}").into()),
                placement,
                row,
            );
        }
        let count = self.rows.len();
        let empty = self.listed.as_ref().is_some_and(Vec::is_empty);
        let modal = self.modal(window, cx);
        let section = Section::new("vault", "Vault")
            .count(count.to_string())
            .sentence(SENTENCE)
            .action(
                Button::secondary("add-vault-entry", "New entry")
                    .leading(Icon::Plus)
                    .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
            )
            .child(presence("vault-list-block", count > 0, list).gap(space::MD))
            .child(
                presence(
                    "no-vault-block",
                    empty,
                    EmptyLine::new(
                        "no-vault",
                        Icon::Key,
                        "No entry yet. Add a secret an agent needs, such as a tracker's token: \
                         agents use it through Flottille's tools and never see it.",
                    ),
                )
                .gap(space::MD),
            )
            .child(problem_line(
                "vault-problem",
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
