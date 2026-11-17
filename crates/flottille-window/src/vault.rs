//! The vault in Settings (`docs/vault.md`): the user's secrets, kept by the keyring, that agents
//! use through Flottille's tools and never see. The page lists the entries' metadata; a secret is
//! typed in a masked field, sent to the daemon (`vault.set`) and cleared from the field at once,
//! and never shown again: editing an entry keeps its secret unless a new one is typed. The window
//! writes nothing itself, and shows the list the daemon gives next.

use std::collections::HashMap;

use gpui_design_system::{
    Button, Confirm, EmptyLine, Field, Icon, ListCard, ListRow, Listing, Menu, Modal, Section,
    Select, SelectOption, Toast, icon, palette, presence, presence_of, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};
use serde_json::Value;

use crate::actions::{Command, VaultWrite};
use crate::commands::{boxed, line_field, menu_of, perform_later, problem_line, spaced};
use crate::project_settings::Reveal as ProjectReveal;
use crate::shell::Shell;

/// What the section says the vault is for.
pub const SENTENCE: &str = "Secrets agents use without seeing them. Your keyring keeps them; \
                            agents see only their names and what they are for.";

/// How an entry is used, said once under the sentence (rule 14).
pub const USAGE: &str = "An agent names an entry; Flottille puts the secret where it goes. A \
                         project's tracker and a connection pick theirs here.";

/// What the dialog says of the secret, under its field.
pub const SECRET_HINT: &str = "Kept by your keyring and never shown again, here or anywhere.";

/// What it says of the secret of an entry that exists.
pub const KEEP_SECRET_HINT: &str = "Leave empty to keep the current secret";

/// What the form says of an entry of every project, opened from one project's Vault tab.
pub const EVERY_PROJECT_NOTE: &str = "This entry is every project's: changing it changes it \
                                      for all of them.";

/// What the form says of a new entry an agent asked for a connection: why it is every project's.
pub fn connection_note(connection: &str) -> String {
    format!(
        "For the connection {connection}: a connection is given to every project, so the entries \
         it names are every project's."
    )
}

/// What the project select calls no project.
pub const EVERY_PROJECT: &str = "Every project";

/// An entry as the daemon lists it: its metadata, never its secret.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VaultEntry {
    pub name: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub username: Option<String>,
    pub project: Option<String>,
    /// The one use that reads it (`docs/vault.md`, "Bound to one use").
    pub bound: Option<String>,
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
            bound: text("for"),
        }
    }
}

/// What the dialog is doing.
#[derive(Clone, Debug, PartialEq)]
enum Form {
    Closed,
    /// A new entry, or the one named edited.
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
    Saved(String),
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
    /// The use a new entry is made for, when the dialog was opened from that feature.
    bound: Option<String>,
    /// Why an agent asked for the new entry, when the dialog was opened from its request.
    why: Option<String>,
    /// The host the request's URL sends the secret to, as the daemon read it, and that URL: the
    /// host is said while the URL is the one asked.
    host: Option<(String, String)>,
    /// The connection an agent asked the new entry for: the form says why it is every project's.
    connection: Option<String>,
    saving: bool,
    writing: Option<Writing>,
    form_error: Option<SharedString>,
    confirming: Option<String>,
    /// The project page the dialog was opened from (its Tracker, or its Vault tab): once it is
    /// closed, the window goes back there.
    come_back: Option<(String, ProjectReveal)>,
    /// The agent's request to write a connection the dialog was opened from, its missing entry:
    /// once it is saved or closed, the request's dialog is back.
    request_back: Option<u64>,
    /// The entry opened from a project's Vault tab is every project's: the form says so.
    every_project: bool,
    /// The entry the confirmation names, kept while it fades away.
    asked: String,
    _subscriptions: Vec<Subscription>,
}

/// One `label: value` line of the form's facts, said to the screen reader as one.
fn line(
    id: &'static str,
    label: &'static str,
    value: SharedString,
    mute: gpui_kit::Hsla,
) -> gpui_kit::AnyElement {
    div()
        .id(id)
        .flex()
        .gap(space::SM)
        .aria_label(SharedString::from(format!("{label}: {value}")))
        .child(div().text_color(mute).child(label))
        .child(div().min_w_0().child(value))
        .test_support()
        .into_any_element()
}

/// An entry's row, as Settings' Vault and a project's Vault tab draw it: its name, its project
/// (or every project's), and one line of what it is for, never a value.
pub fn entry_row(id: ElementId, entry: &VaultEntry, cx: &App) -> ListRow {
    let p = palette(cx);
    let line = [
        entry.description.clone(),
        entry.url.clone(),
        entry.username.clone(),
    ]
    .into_iter()
    .flatten()
    .next();
    let used = entry
        .bound
        .as_deref()
        .and_then(flottille_core::vault::use_said);
    let line = match (used, line) {
        (Some(used), Some(line)) => format!("For {used} alone · {line}"),
        (Some(used), None) => format!("For {used} alone"),
        (None, line) => line.unwrap_or_else(|| "No description".to_owned()),
    };
    ListRow::new(id, SharedString::from(entry.name.clone()))
        .mono()
        .leading(icon(Icon::Key).size(px(14.)).text_color(p.faint))
        .tag(SharedString::from(
            entry
                .project
                .clone()
                .unwrap_or_else(|| EVERY_PROJECT.to_owned()),
        ))
        .line(SharedString::from(line))
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
            bound: None,
            why: None,
            host: None,
            connection: None,
            saving: false,
            writing: None,
            form_error: None,
            confirming: None,
            come_back: None,
            request_back: None,
            every_project: false,
            asked: String::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// The entries as last listed; `None` until the daemon has said.
    pub fn entries(&self) -> Option<&[VaultEntry]> {
        self.listed.as_deref()
    }

    /// The project the dialog sets the entry to; every project's when none.
    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    pub fn is_open(&self) -> bool {
        self.form != Form::Closed
    }

    pub fn is_confirming(&self) -> bool {
        self.confirming.is_some()
    }

    /// The dialog's title, `None` while it is closed.
    pub fn form_title(&self) -> Option<String> {
        match &self.form {
            Form::Closed => None,
            Form::Editing(Some(name)) => Some(format!("Edit {name}")),
            Form::Editing(None) => Some("New entry".to_owned()),
        }
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

    pub fn url_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.url)
    }

    pub fn username_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|f| &f.username)
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
                gpui_design_system::follow_theme(&mut field, cx);
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
            // The host asked is said while the URL is the one asked.
            self._subscriptions
                .push(cx.subscribe(&url, |this, _, event, cx| {
                    if matches!(event, InputEvent::Change) && this.host.is_some() {
                        cx.notify();
                    }
                }));
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

    /// "New entry", the dialog on `prefill` (its name, metadata, project and use) with the secret
    /// to type: what a project's Tracker or Vault tab, or an agent's request, already knows of it.
    /// `why` is an agent's reason, said under what it is for; `back` is the project page to go
    /// back to once the dialog is closed.
    pub fn add_prefilled(
        &mut self,
        prefill: &VaultEntry,
        why: Option<String>,
        back: Option<(String, ProjectReveal)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fill(Some(prefill), window, cx);
        self.form = Form::Editing(None);
        self.bound = prefill.bound.clone();
        self.why = why;
        self.come_back = back;
    }

    /// A row of a project's Vault tab: the entry in the same dialog, and back to that tab once it
    /// is closed. An entry of every project says so on the form. Whether it opened: the entry may
    /// be gone since the row was drawn.
    pub fn replace_in(
        &mut self,
        project: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.replace(name, window, cx);
        let opened = self.is_open();
        if opened {
            self.every_project = self.project.is_none();
            self.come_back = Some((project.to_owned(), ProjectReveal::Vault));
        }
        opened
    }

    /// "New entry" from an agent's request (`vault_ask.rs`): `entry` prefilled, `why` under what it
    /// is for, and `host`, where the daemon reads its URL sends the secret; for a `connection`,
    /// every project's, and the form says why. The user may change every field before Add,
    /// remove the binding, or close it.
    pub fn add_asked(
        &mut self,
        entry: &VaultEntry,
        why: String,
        host: Option<String>,
        connection: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_prefilled(entry, Some(why), None, window, cx);
        self.host = host.zip(entry.url.clone());
        self.connection = connection;
    }

    /// "New entry…" on an agent's request to write a connection that names `entry`, not made yet:
    /// its name and, as its address, the `host` the request sends it to; the secret is typed here.
    pub fn add_for_request(
        &mut self,
        number: u64,
        entry: &str,
        host: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefill = VaultEntry {
            name: entry.to_owned(),
            url: host.clone(),
            ..VaultEntry::default()
        };
        self.add_prefilled(&prefill, None, None, window, cx);
        self.host = host.zip(prefill.url);
        self.request_back = Some(number);
    }

    /// The request whose dialog to open again, once the entry made for it is saved.
    pub fn take_request_return(&mut self) -> Option<u64> {
        match self.form {
            Form::Closed => self.request_back.take(),
            Form::Editing(_) => None,
        }
    }

    /// The project page to go back to, once the dialog it was opened for is closed (an entry set).
    pub fn take_return(&mut self) -> Option<(String, ProjectReveal)> {
        match self.form {
            Form::Closed => self.come_back.take(),
            Form::Editing(_) => None,
        }
    }

    /// "Edit": the dialog on the entry's metadata, its secret left empty to keep it.
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
        self.come_back = None;
        self.request_back = None;
        self.every_project = false;
        self.bound = None;
        self.why = None;
        self.host = None;
        self.connection = None;
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
        let focus = if entry.is_some_and(|entry| !entry.name.is_empty()) {
            secret
        } else {
            name
        }
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
        if let Some((project, at)) = self.come_back.take() {
            self.shell
                .update(cx, |shell, cx| {
                    shell.open_project_settings_at(&project, at, window, cx)
                })
                .ok();
        }
        if let Some(number) = self.request_back.take() {
            self.shell
                .update(cx, |shell, cx| shell.open_connection_request(number, cx))
                .ok();
        }
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

    /// Saves the dialog: a new entry, or the one edited.
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
        if name.is_empty() || (original.is_none() && secret.is_empty()) {
            self.form_error = Some("An entry needs a name and a secret.".into());
            cx.notify();
            return;
        }
        let write = VaultWrite::Set {
            name: name.clone(),
            // An empty one on an entry that exists is the one it has, kept.
            secret: Some(secret).filter(|secret| !secret.is_empty()),
            description: value(&fields.description),
            url: value(&fields.url),
            username: value(&fields.username),
            project: self.project.clone(),
            bound: self.bound.clone(),
        };
        let writing = match original {
            Some(_) => Writing::Saved(name),
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
                confirmed: true,
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
                    Some(Writing::Saved(name)) => Toast::success(format!("Entry {name} saved")),
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
                "Edit",
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
        let name = entry.name.clone();
        let menu = self.menu(&name, cx);
        let id = ElementId::Name(format!("vault-{name}").into());
        entry_row(id, entry, cx)
            .more(menu)
            .on_click(cx.listener(move |this, _, window, cx| {
                if !this.saving {
                    this.replace(&name, window, cx)
                }
            }))
    }

    /// The confirmation of a removal.
    fn confirm(&self, cx: &mut Context<Self>) -> Confirm {
        let name = self.asked.clone();
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        // A bound entry says what stops working without it.
        let stops = self
            .listed
            .iter()
            .flatten()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.bound.as_deref())
            .and_then(flottille_core::vault::use_stops);
        let sentence = match stops {
            Some(stops) => format!(
                "Its secret is deleted from your keyring, and {stops} while it names this entry."
            ),
            None => "Its secret is deleted from your keyring, and the tools that use it can no \
                     longer."
                .to_owned(),
        };
        Confirm::new("vault-removal", format!("Remove the entry {name}?"))
            .sentence(sentence)
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

    /// What a new entry is bound to, said in the dialog, with the way to bind it to no use; it
    /// folds away once the binding is removed.
    fn binding(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mute = palette(cx).mute;
        let used = self
            .bound
            .as_deref()
            .map(|bound| flottille_core::vault::use_said(bound).unwrap_or_else(|| bound.to_owned()))
            .map(SharedString::from);
        let this = cx.entity().downgrade();
        let saving = self.saving;
        presence_of("vault-binding-block", used, move |used| {
            div()
                .id("vault-binding")
                .flex()
                .flex_col()
                .child(line("vault-binding-for", "For", used, mute))
                .child(spaced(
                    space::SM,
                    Button::quiet("vault-binding-remove", "Remove the binding")
                        .disabled(saving)
                        .on_click(move |_, _, cx| {
                            this.update(cx, |this, cx| {
                                this.bound = None;
                                cx.notify();
                            })
                            .ok();
                        }),
                ))
                .into_any_element()
        })
        .space(space::LG)
    }

    /// Where the secret goes, as the daemon read the URL asked (what a connection's login would
    /// use), under the URL's field whether the entry is bound or not: the URL's text can read
    /// like another host. It folds away once the URL is no longer the one asked.
    fn goes_to(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mute = palette(cx).mute;
        let url = self
            .fields
            .as_ref()
            .map(|fields| fields.url.read(cx).value().trim().to_owned());
        let host = self
            .host
            .clone()
            .filter(|(_, asked)| url.as_deref() == Some(asked.trim()))
            .map(|(host, _)| SharedString::from(host));
        presence_of("vault-goes-to-block", host, move |host| {
            line("vault-goes-to", "Goes to", host, mute)
        })
        .space(space::XS)
    }

    fn modal(&mut self, window: &Window, cx: &mut Context<Self>) -> Modal {
        let p = palette(cx);
        let this = cx.entity().downgrade();
        let open = self.form != Form::Closed;
        let title = self.form_title().unwrap_or_else(|| "New entry".to_owned());
        let modal = Modal::new("vault-modal", title)
            .open(open)
            .on_close(move |window, cx| {
                this.update(cx, |this, cx| this.close(window, cx)).ok();
            });
        let Form::Editing(original) = self.form.clone() else {
            return modal;
        };
        let select = self.project_select(cx);
        let binding = self.binding(cx);
        let goes_to = self.goes_to(cx);
        let error = problem_line("vault-error", self.form_error.clone(), space::LG, cx);
        let fields = self.fields.as_ref().expect("made when the form opens");
        let description = Field::new(
            "vault-description-field",
            "What it is for",
            boxed("vault-description", Input::new(&fields.description), cx),
        )
        .optional();
        let description = match &self.why {
            Some(why) => description.hint(SharedString::from(format!("Why the agent asks: {why}"))),
            None => description,
        };
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
            .child(Field::new("vault-name-field", "Name", name).hint("How agents ask for it."))
            .child(spaced(
                space::LG,
                Field::new(
                    "vault-secret-field",
                    "Secret",
                    boxed("vault-secret", Input::new(&fields.secret), cx),
                )
                .hint(if original.is_some() {
                    KEEP_SECRET_HINT
                } else {
                    SECRET_HINT
                }),
            ))
            .child(spaced(space::LG, description))
            .child(spaced(
                space::LG,
                div()
                    .flex()
                    .flex_col()
                    .child(
                        Field::new(
                            "vault-url-field",
                            "URL",
                            boxed("vault-url", Input::new(&fields.url), cx),
                        )
                        .optional(),
                    )
                    .child(goes_to),
            ))
            .child(spaced(
                space::LG,
                Field::new(
                    "vault-username-field",
                    "Username",
                    boxed("vault-username", Input::new(&fields.username), cx),
                )
                .optional(),
            ))
            .child(binding)
            .child(spaced(
                space::LG,
                // The note goes once a project is chosen: it said what the entry was when the form
                // opened, not what it is about to become.
                div()
                    .flex()
                    .flex_col()
                    .child(Field::new(
                        "vault-project-field",
                        "Seen by agents of",
                        select,
                    ))
                    .child(
                        presence_of(
                            "vault-connection-note-block",
                            self.connection
                                .as_deref()
                                .map(|connection| SharedString::from(connection_note(connection))),
                            move |note| {
                                div()
                                    .id("vault-connection-note")
                                    .aria_label(note.clone())
                                    .text_size(px(12.5))
                                    .text_color(p.mute)
                                    .child(note)
                                    .test_support()
                                    .into_any_element()
                            },
                        )
                        .space(space::XS),
                    )
                    .child(
                        presence_of(
                            "vault-every-project-note-block",
                            (self.every_project && self.project.is_none())
                                .then(|| SharedString::from(EVERY_PROJECT_NOTE)),
                            move |note| {
                                div()
                                    .id("vault-every-project-note")
                                    .aria_label(note.clone())
                                    .text_size(px(12.5))
                                    .text_color(p.mute)
                                    .child(note)
                                    .test_support()
                                    .into_any_element()
                            },
                        )
                        .space(space::XS),
                    ),
            ))
            .test_support();
        modal
            .sentence(
                "Agents see its name, what it is for, its URL and its username; never its secret.",
            )
            .child(div().flex().flex_col().child(body).child(error))
            .action(
                Button::primary(
                    "save-vault-entry",
                    if original.is_some() { "Save" } else { "Add" },
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(crate::extensions::usage_line("vault-usage", USAGE, cx))
                    .child(presence("vault-list-block", count > 0, list).space(space::MD))
                    .child(
                        presence(
                            "no-vault-block",
                            empty,
                            EmptyLine::new(
                                "no-vault",
                                Icon::Key,
                                "No entry yet. Add a secret an agent needs, such as a tracker's \
                                 token: agents use it through Flottille's tools and never see it.",
                            ),
                        )
                        .space(space::MD),
                    )
                    .child(problem_line(
                        "vault-problem",
                        self.problem.clone(),
                        space::MD,
                        cx,
                    )),
            );
        div()
            .text_color(p.text)
            .child(section)
            .child(modal)
            .child(self.confirm(cx))
    }
}
