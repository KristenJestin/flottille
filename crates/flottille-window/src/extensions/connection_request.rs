//! An agent's request to write a connection (#854, `docs/extensions.md`, "Connections"): its card
//! waits in the rail as every held request does, and opens this dialog instead of being allowed
//! from there. The dialog shows what the daemon says the request would write
//! (`connection.proposed`): the fields of "New server", filled and read-only, an edit's changed
//! fields with their old value, a `set`'s settings, where each vault entry it names goes, then
//! Accept, Edit and Refuse. Accept and Refuse answer the request (Refuse writes nothing, and asks
//! nothing more, as the rail's Deny); Edit opens the server's own form with the agent's values,
//! whose Save answers it. The results are toasts.

use gpui_design_system::{
    Button, Confirm, Field, Icon, Modal, Skeleton, Tag, Toast, awaiting, palette, presence_of,
    space, toast,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity,
    Window, div, px,
};
use serde_json::{Value, json};

use super::connections::{Server, ServerSecret, command_line};
use crate::actions::Command;
use crate::commands::{perform_later, problem_line};
use crate::shell::Shell;

/// What to do with the daemon's answer to a question of the dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `connection.proposed` of the request `number`.
    Proposed { number: u64 },
    /// `approval.approve` (`accepted`) or `approval.deny` of the request `number`.
    Decided { number: u64, accepted: bool },
    /// `connection.share_entry` or `connection.bind_entry` of an entry the request `number` names.
    Widened { number: u64 },
}

/// What the user gives a vault entry from the request's dialog, behind the confirmation: the
/// entry every project's, or bound to the host the connection sends it to.
#[derive(Clone, Debug, PartialEq)]
pub enum Widen {
    Share {
        entry: String,
        project: String,
    },
    /// `was` is the entry's address before, `origin` the one it gets.
    Bind {
        entry: String,
        host: String,
        was: Option<String>,
        origin: String,
    },
}

/// What a request would write, as `connection.proposed` says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub action: String,
    pub name: String,
    pub before: Option<Server>,
    pub after: Option<Server>,
    /// The keys an edit changes.
    pub changed: Vec<String>,
    /// A `set`'s settings, by action.
    pub settings: Vec<(String, String)>,
    /// Where each vault entry the declaration names goes.
    pub secrets: Vec<Secret>,
    /// Still what the request says: one that is not would not be done.
    pub current: bool,
    pub why: Option<String>,
    /// The connection waits for the user's review (#1031): the review is the first thing the
    /// dialog offers, and a `set` waits for it.
    pub review: bool,
}

/// A vault entry a declaration names, as `connection.proposed` says where it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Secret {
    /// `env` or `headers`.
    pub key: String,
    /// The variable or the header.
    pub name: String,
    pub entry: String,
    /// The host of the entry's `url`; none when it has no address.
    pub entry_host: Option<String>,
    /// The host the connection sends it to; none for a command, whose environment holds it.
    pub host: Option<String>,
    /// Sent elsewhere than the entry's address says, or given to a program.
    pub differs: bool,
    /// The one project the entry is alone to, when it is: a connection needs it for every project.
    pub project: Option<String>,
    /// The user may share it from the request: the asking agent's own project's alone.
    pub share: bool,
    /// Accept waits for the user to bind the entry to the connection's host.
    pub to_bind: bool,
    /// The entry's address as it is.
    pub entry_url: Option<String>,
    /// The connection's origin, the address a Bind gives the entry.
    pub origin: Option<String>,
    /// The vault holds no entry of that name yet.
    pub missing: bool,
}

impl Secret {
    fn of(value: &Value) -> Self {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        Self {
            key: text("key"),
            name: text("name"),
            entry: text("entry"),
            entry_host: value["entry_host"].as_str().map(str::to_owned),
            host: value["host"].as_str().map(str::to_owned),
            differs: value["differs"] != false,
            project: value["project"].as_str().map(str::to_owned),
            share: value["share"] == true,
            to_bind: value["to_bind"] == true,
            entry_url: value["entry_url"].as_str().map(str::to_owned),
            origin: value["origin"].as_str().map(str::to_owned),
            missing: value["missing"] == true,
        }
    }

    /// Sent to a host by an entry that is bound to no address: nothing says it may go there, which
    /// is not the same as an address that says it may not.
    fn unbound(&self) -> bool {
        self.host.is_some() && self.entry_host.is_none()
    }

    /// Its line on the card: the entry, its address, and where it goes.
    pub fn said(&self) -> String {
        match (&self.host, &self.entry_host) {
            (Some(host), None) => format!(
                "{}: the vault entry {} (bound to no address) will be sent to {host}",
                self.name, self.entry
            ),
            (Some(host), Some(address)) => format!(
                "{}: the vault entry {} ({address}), sent to {host}",
                self.name, self.entry
            ),
            (None, address) => format!(
                "{}: the vault entry {} ({}), given to the program in its environment",
                self.name,
                self.entry,
                address.as_deref().unwrap_or("no address")
            ),
        }
    }
}

impl Proposal {
    pub fn of(value: &Value) -> Self {
        let server = |value: &Value| (!value.is_null()).then(|| Server::of(value));
        Self {
            action: value["action"].as_str().unwrap_or_default().to_owned(),
            name: value["name"].as_str().unwrap_or_default().to_owned(),
            before: server(&value["before"]),
            after: server(&value["after"]),
            changed: value["changed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|key| key.as_str().map(str::to_owned))
                .collect(),
            settings: value["settings"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(action, setting)| {
                    (action.clone(), setting.as_str().unwrap_or_default().into())
                })
                .collect(),
            secrets: value["secrets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(Secret::of)
                .collect(),
            current: value["current"] == true,
            why: value["why"].as_str().map(str::to_owned),
            review: value["review"] == true,
        }
    }

    /// The dialog's title: what the agent asks, in the window's words.
    pub fn title(&self) -> String {
        let name = &self.name;
        match self.action.as_str() {
            "add" => format!("Add the server {name}"),
            "edit" => format!("Edit the server {name}"),
            "remove" => format!("Remove the server {name}"),
            "on" => format!("Turn the server {name} on"),
            "off" => format!("Turn the server {name} off"),
            _ => format!("Set the actions of {name}"),
        }
    }

    /// The declaration the dialog shows: what it leaves, or what it removes.
    fn shown(&self) -> Option<&Server> {
        self.after.as_ref().or(self.before.as_ref())
    }

    /// Whether the server's own form can be opened on it, with the agent's values.
    pub fn editable(&self) -> bool {
        matches!(self.action.as_str(), "add" | "edit") && self.after.is_some()
    }
}

/// The words of an action's setting, as the review says them.
fn setting_said(setting: &str) -> &'static str {
    match setting {
        "passes" => "Allowed",
        "asked" => "Asked",
        "off" => "Not offered",
        _ => "Judged",
    }
}

pub struct ConnectionRequest {
    shell: WeakEntity<Shell>,
    /// The request open, kept while the dialog fades out.
    number: Option<u64>,
    open: bool,
    proposal: Option<Result<Proposal, String>>,
    deciding: bool,
    error: Option<SharedString>,
    /// What the confirmation asks before it is given, while it is open.
    confirming: Option<Widen>,
    /// What the confirmation said last, kept while it fades out.
    confirmed: Option<Widen>,
    /// A Share or a Bind sent, until the daemon answers.
    widening: bool,
    /// The lines of entries to mend shown since the dialog opened, `(id, sentence, action, what
    /// it gives)`: kept so that one mended folds away.
    lines: Vec<(String, String, SharedString, Widen)>,
    /// The entries an Accept answered it waits for, one `entry\thost` line each: what the
    /// **New entry…** buttons are made of, kept by their block while it folds away.
    waits_for: Option<SharedString>,
}

/// The vault entries a request names that the vault does not hold yet, each once, with the host
/// the request sends it to.
fn missing_entries(proposal: &Proposal) -> Vec<(String, Option<String>)> {
    let mut missing: Vec<(String, Option<String>)> = Vec::new();
    for secret in proposal.secrets.iter().filter(|secret| secret.missing) {
        if !missing.iter().any(|(entry, _)| *entry == secret.entry) {
            missing.push((secret.entry.clone(), secret.host.clone()));
        }
    }
    missing
}

/// The missing entries as one line each, `entry\thost`: what `presence_of` keeps while it folds.
fn said_entries(missing: &[(String, Option<String>)]) -> SharedString {
    let lines: Vec<String> = missing
        .iter()
        .map(|(entry, host)| format!("{entry}\t{}", host.as_deref().unwrap_or_default()))
        .collect();
    lines.join("\n").into()
}

fn entries_said(said: &str) -> Vec<(String, Option<String>)> {
    said.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(entry, host)| {
            let host = (!host.is_empty()).then(|| host.to_owned());
            (entry.to_owned(), host)
        })
        .collect()
}

impl ConnectionRequest {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            number: None,
            open: false,
            proposal: None,
            deciding: false,
            error: None,
            confirming: None,
            confirmed: None,
            widening: false,
            lines: Vec::new(),
            waits_for: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// What the daemon answered to the last Accept or Refuse, when it refused it.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn ask(&self, method: &str, params: Value, then: Then, cx: &mut Context<Self>) {
        let command = Command::Ask {
            method: method.into(),
            params,
            then: super::Then::ConnectionRequest(then),
        };
        perform_later(&self.shell, command, cx);
    }

    /// Opens the dialog on the request `number`: the daemon says what it would write.
    pub fn open(&mut self, number: u64, cx: &mut Context<Self>) {
        self.number = Some(number);
        self.open = true;
        self.proposal = None;
        self.deciding = false;
        self.error = None;
        self.confirming = None;
        self.widening = false;
        self.lines.clear();
        self.waits_for = None;
        self.ask(
            "connection.proposed",
            json!({"number": number}),
            Then::Proposed { number },
            cx,
        );
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.confirming = None;
        cx.notify();
    }

    /// What the daemon answered to something the dialog asked.
    pub fn answered(&mut self, then: Then, answer: Result<Value, String>, cx: &mut Context<Self>) {
        match then {
            Then::Proposed { number } => {
                if self.number == Some(number) {
                    let proposal = answer.map(|value| Proposal::of(&value));
                    let now = proposal
                        .as_ref()
                        .map_or_else(|_| Vec::new(), Self::widenings_of);
                    // Each line acts as the latest reading says; one gone keeps its last words
                    // while it folds away.
                    for line in &now {
                        match self.lines.iter_mut().find(|(id, ..)| *id == line.0) {
                            Some(known) => *known = line.clone(),
                            None => self.lines.push(line.clone()),
                        }
                    }
                    // A confirmation of what the request no longer asks closes: confirmed, it
                    // would give what the user did not read.
                    if self
                        .confirming
                        .as_ref()
                        .is_some_and(|widen| !now.iter().any(|(.., is)| is == widen))
                    {
                        self.confirming = None;
                    }
                    // A request no longer current offers nothing to mend: its New entry… block
                    // folds away with the rest.
                    if proposal.as_ref().is_ok_and(|p| !p.current) {
                        self.waits_for = None;
                    }
                    self.proposal = Some(proposal);
                }
            }
            Then::Widened { number } => self.widened(number, answer, cx),
            Then::Decided { number, accepted } => {
                if self.number == Some(number) {
                    self.deciding = false;
                }
                let name = self
                    .proposal
                    .as_ref()
                    .and_then(|p| p.as_ref().ok())
                    .filter(|_| self.number == Some(number))
                    .map(|p| (p.name.clone(), p.action.clone()));
                match answer {
                    Ok(answer) => {
                        if self.number == Some(number) {
                            self.open = false;
                        }
                        // The daemon waited for the write: the toast says how it ended.
                        let failure = (accepted && answer["outcome"].as_str() != Some("done"))
                            .then(|| answer["failure"].as_str().map(str::to_owned))
                            .flatten();
                        let said = match (accepted, name, failure) {
                            (true, Some((name, _)), Some(why)) => {
                                Toast::failure(format!("The server {name} was not written"))
                                    .detail(why)
                            }
                            (true, Some((name, action)), None) => {
                                let done = match action.as_str() {
                                    "add" => "was added",
                                    "edit" => "was saved",
                                    "remove" => "was removed",
                                    "on" => "was turned on",
                                    "off" => "was turned off",
                                    _ => "was set",
                                };
                                Toast::success(format!(
                                    "Request accepted: the server {name} {done}"
                                ))
                                .detail("The agent is told how it went.")
                            }
                            (true, None, _) => {
                                Toast::success(format!("Request #{number} accepted"))
                            }
                            (false, ..) => Toast::success("Request refused: nothing was written")
                                .detail("The agent is told you refused it."),
                        };
                        toast(said, cx);
                    }
                    Err(why) => match self.number == Some(number) && self.open {
                        true => {
                            self.error = Some(why.into());
                            // Accept is refused for a missing entry before anything else.
                            self.waits_for = self
                                .proposal
                                .as_ref()
                                .and_then(|p| p.as_ref().ok())
                                .map(missing_entries)
                                .filter(|missing| accepted && !missing.is_empty())
                                .map(|missing| said_entries(&missing));
                        }
                        false => toast(
                            Toast::failure(format!("Request #{number} was not answered"))
                                .detail(why),
                            cx,
                        ),
                    },
                }
            }
        }
        cx.notify();
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        let Some(number) = self.number.filter(|_| !self.deciding) else {
            return;
        };
        self.deciding = true;
        self.error = None;
        self.waits_for = None;
        self.ask(
            "approval.approve",
            json!({"number": number}),
            Then::Decided {
                number,
                accepted: true,
            },
            cx,
        );
        cx.notify();
    }

    fn refuse(&mut self, cx: &mut Context<Self>) {
        let Some(number) = self.number.filter(|_| !self.deciding) else {
            return;
        };
        self.deciding = true;
        self.error = None;
        self.waits_for = None;
        self.ask(
            "approval.deny",
            json!({"number": number}),
            Then::Decided {
                number,
                accepted: false,
            },
            cx,
        );
        cx.notify();
    }

    /// "New entry…" beside the answer that waits for an entry: the vault's own dialog on `entry`,
    /// its address the `host` the request sends it to. This dialog gives way to it and is back,
    /// reading the request again, once the entry is saved or the dialog closed.
    fn new_entry(
        &mut self,
        entry: String,
        host: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(number) = self.number else {
            return;
        };
        self.open = false;
        self.shell
            .update(cx, |shell, cx| {
                shell.add_vault_entry_for_request(number, &entry, host, window, cx)
            })
            .ok();
        cx.notify();
    }

    /// "Review": the connection's review opens over the page, first: the request stays held.
    fn review(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.shell
            .update(cx, |shell, cx| shell.review_connection(&name, window, cx))
            .ok();
        cx.notify();
    }

    /// "Edit": the server's own form, on the agent's values; its Save answers the request.
    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((number, Ok(proposal))) = self.number.zip(self.proposal.clone()) else {
            return;
        };
        let Some(server) = proposal.after.clone().filter(|_| proposal.editable()) else {
            return;
        };
        self.open = false;
        let adding = proposal.action == "add";
        self.shell
            .update(cx, |shell, cx| {
                shell.answer_connection_request(number, &server, adding, window, cx)
            })
            .ok();
        cx.notify();
    }

    // ── Share and Bind ───────────────────────────────────────────────────────────────────────

    /// The action of an entry's line: the confirmation, which says what it gives and to where.
    fn ask_to_widen(&mut self, widen: Widen, cx: &mut Context<Self>) {
        self.confirmed = Some(widen.clone());
        self.confirming = Some(widen);
        cx.notify();
    }

    /// The confirmation's action: the daemon widens the entry, reading where from the request
    /// itself; the dialog reads the request again once it is done.
    fn widen(&mut self, cx: &mut Context<Self>) {
        let (Some(number), Some(widen)) = (self.number, self.confirming.take()) else {
            return;
        };
        // A Bind names the host the confirmation showed: the daemon binds nothing if it reads
        // another one.
        let (method, params) = match &widen {
            Widen::Share { entry, .. } => (
                "connection.share_entry",
                json!({"number": number, "entry": entry}),
            ),
            Widen::Bind { entry, host, .. } => (
                "connection.bind_entry",
                json!({"number": number, "entry": entry, "host": host}),
            ),
        };
        self.widening = true;
        self.error = None;
        self.ask(method, params, Then::Widened { number }, cx);
        cx.notify();
    }

    fn widened(&mut self, number: u64, answer: Result<Value, String>, cx: &mut Context<Self>) {
        if self.number != Some(number) {
            return;
        }
        self.widening = false;
        // Done or refused, the request is read again: a refusal is often that it changed.
        if let Err(why) = answer {
            self.error = Some(why.into());
        }
        self.ask(
            "connection.proposed",
            json!({"number": number}),
            Then::Proposed { number },
            cx,
        );
    }

    /// The lines a proposal asks of the user, each `(id, sentence, action, what it gives)`: an
    /// entry the asking agent's project's alone (Share), one bound to no address or another host
    /// (Bind). None for a request that would no longer be done.
    fn widenings_of(proposal: &Proposal) -> Vec<(String, String, SharedString, Widen)> {
        let mut lines: Vec<(String, String, SharedString, Widen)> = Vec::new();
        if !proposal.current {
            return lines;
        }
        for secret in &proposal.secrets {
            let entry = &secret.entry;
            let id = format!("connection-request-share-{entry}");
            if secret.share
                && let Some(project) = &secret.project
                && !lines.iter().any(|(known, ..)| *known == id)
            {
                lines.push((
                    id,
                    format!(
                        "the vault entry {entry} is {project}'s alone; a connection needs it for \
                         every project"
                    ),
                    "Share with every project".into(),
                    Widen::Share {
                        entry: entry.clone(),
                        project: project.clone(),
                    },
                ));
            }
            let id = format!("connection-request-bind-{entry}");
            if secret.to_bind
                && let Some(host) = &secret.host
                && let Some(origin) = &secret.origin
                && !lines.iter().any(|(known, ..)| *known == id)
            {
                let said = match &secret.entry_host {
                    Some(entry_host) => format!(
                        "the vault entry {entry} is for {entry_host}; this connection would send \
                         it to {host}"
                    ),
                    None => format!(
                        "the vault entry {entry} has no address; this connection would send it \
                         to {host}"
                    ),
                };
                lines.push((
                    id,
                    said,
                    SharedString::from(format!("Bind it to {host}")),
                    Widen::Bind {
                        entry: entry.clone(),
                        host: host.clone(),
                        was: secret.entry_url.clone(),
                        origin: origin.clone(),
                    },
                ));
            }
        }
        lines
    }

    /// The lines of the entries the user may mend from here, each with its action: one project's
    /// alone (**Share with every project**), bound to no address or to another host than the one
    /// this connection sends it to (**Bind it to <host>**). Each unfolds and folds away with its
    /// gap, as the dialog reads the request again.
    fn widenings(&self, proposal: &Proposal, cx: &Context<Self>) -> AnyElement {
        let wait = palette(cx).wait;
        let current = Self::widenings_of(proposal);
        let this = cx.entity().downgrade();
        let busy = self.widening || self.deciding;
        let line = move |id: String,
                         said: Option<String>,
                         action: SharedString,
                         widen: Widen|
              -> AnyElement {
            let this = this.clone();
            presence_of(
                SharedString::from(format!("{id}-block")),
                said.map(SharedString::from),
                move |said| {
                    div()
                        .flex()
                        .items_start()
                        .gap(space::SM)
                        .child(
                            div()
                                .id(SharedString::from(id.clone()))
                                .flex_1()
                                .min_w_0()
                                .whitespace_normal()
                                .text_color(wait)
                                .aria_label(said.clone())
                                .child(said)
                                .test_support(),
                        )
                        .child(
                            Button::secondary(SharedString::from(format!("{id}-action")), action)
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    this.update(cx, |this, cx| {
                                        this.ask_to_widen(widen.clone(), cx)
                                    })
                                    .ok();
                                }),
                        )
                        .into_any_element()
                },
            )
            .space(space::SM)
            .into_any_element()
        };
        let mut lines = div().id("connection-request-widenings").flex().flex_col();
        // Every line shown since the dialog opened is built on every frame: one whose entry is
        // mended folds away instead of vanishing.
        for (id, _, action, widen) in &self.lines {
            let said = current
                .iter()
                .find(|(now, ..)| now == id)
                .map(|(_, said, ..)| said.clone());
            lines = lines.child(line(id.clone(), said, action.clone(), widen.clone()));
        }
        lines.test_support().into_any_element()
    }

    /// The one confirmation of a Share or a Bind: the question names the entry, the sentence what
    /// it gives and to where.
    fn confirmation(&self, cx: &Context<Self>) -> Confirm {
        let connection = self
            .proposal
            .as_ref()
            .and_then(|p| p.as_ref().ok())
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let (question, sentence, action) = match &self.confirmed {
            Some(Widen::Share { entry, project }) => (
                format!("Share the entry {entry} with every project?"),
                format!(
                    "Every project's agents will see its name, what it is for, its URL and its \
                     username, and every feature of every project may use it, the connection \
                     {connection} among them; it will no longer be {project}'s alone. Its \
                     secret stays in your keyring. This stays so even if you then refuse the \
                     request."
                ),
                "Share with every project".to_owned(),
            ),
            Some(Widen::Bind {
                entry,
                host,
                was,
                origin,
            }) => (
                format!("Bind the entry {entry} to {host}?"),
                format!(
                    "Its address, now {}, becomes {origin}, the one of the connection \
                     {connection}, and its secret will be sent to {host} in that connection's \
                     requests, as an agent asked. This stays so even if you then refuse the \
                     request.",
                    was.as_deref().unwrap_or("no address")
                ),
                format!("Bind it to {host}"),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        let (this, again) = (cx.entity().downgrade(), cx.entity().downgrade());
        Confirm::new("connection-request-widen", question)
            .sentence(sentence)
            .action(action)
            .open(self.confirming.is_some())
            .on_confirm(move |_, cx| {
                this.update(cx, |this, cx| this.widen(cx)).ok();
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

    // ── Drawing ──────────────────────────────────────────────────────────────────────────────

    /// One read-only field: its label, its value, and for a changed one the old value above it
    /// and a tag.
    fn field(
        &self,
        key: &str,
        label: &'static str,
        value: String,
        was: Option<String>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let id = |part: &str| SharedString::from(format!("connection-request-{part}"));
        let changed = was.is_some();
        let text = |id: SharedString, text: String| -> Stateful<Div> {
            div()
                .id(id)
                .min_w_0()
                .whitespace_normal()
                .aria_label(SharedString::from(text.clone()))
                .child(SharedString::from(text))
        };
        let shown = div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .children(was.map(|was| {
                // The old value as the review's diff says it: `−`, quieter.
                div()
                    .flex()
                    .gap(space::XS)
                    .text_color(p.faint)
                    .child("−")
                    .child(text(id(&format!("was-{key}")), was).test_support())
            }))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(space::SM)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .gap(space::XS)
                            .text_color(p.text)
                            .when(changed, |d| d.child("+"))
                            .child(text(id(key), value).test_support()),
                    )
                    .children(changed.then(|| Tag::new(id(&format!("changed-{key}")), "Changed"))),
            );
        Field::new(id(&format!("{key}-field")), label, shown).into_any_element()
    }

    fn fields(&self, proposal: &Proposal, cx: &Context<Self>) -> AnyElement {
        let p = palette(cx);
        let Some(server) = proposal.shown() else {
            return div().into_any_element();
        };
        let was = |key: &str, of: &dyn Fn(&Server) -> String| -> Option<String> {
            proposal
                .changed
                .iter()
                .any(|changed| changed == key)
                .then(|| proposal.before.as_ref().map(of))
                .flatten()
        };
        let reach = |s: &Server| match s.url {
            Some(_) => "A URL".to_owned(),
            None => "A program".to_owned(),
        };
        let entries = |s: &Server| {
            let lines: Vec<String> = match s.url {
                Some(_) => s
                    .headers
                    .iter()
                    .map(|(name, entry)| ServerSecret {
                        place: "header".into(),
                        name: name.clone(),
                        prefix: String::new(),
                        entry: entry.clone(),
                    })
                    .chain(s.secrets.clone())
                    .map(|secret| secret.said())
                    .collect(),
                None => s
                    .env
                    .iter()
                    .map(|(name, entry)| format!("{name} from the vault entry {entry}"))
                    .collect(),
            };
            match lines.is_empty() {
                true => "None".to_owned(),
                false => lines.join("\n"),
            }
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(space::LG)
            .child(self.field("name", "Name", server.name.clone(), None, cx))
            .child(self.field(
                "about",
                "What it is for",
                server.about.clone(),
                was("about", &|s| s.about.clone()),
                cx,
            ))
            .child(
                self.field(
                    "reach",
                    "How it is reached",
                    reach(server),
                    proposal
                        .before
                        .as_ref()
                        .filter(|before| reach(before) != reach(server))
                        .map(reach),
                    cx,
                ),
            );
        let command = |s: &Server| s.command.as_deref().map(command_line).unwrap_or_default();
        body = match &server.url {
            None => body.child(self.field(
                "command",
                "Command",
                command(server),
                was("command", &command),
                cx,
            )),
            Some(url) => body.child(self.field(
                "url",
                "URL",
                url.clone(),
                was("url", &|s| s.url.clone().unwrap_or_default()),
                cx,
            )),
        };
        let (variables, key) = match server.url {
            Some(_) => ("Secrets of its requests", "secrets"),
            None => ("Variables of its environment", "env"),
        };
        body = body.child(self.field(
            "variables",
            variables,
            entries(server),
            was(key, &entries),
            cx,
        ));
        if !proposal.secrets.is_empty() {
            body = body.child(self.secrets(proposal, cx));
        }
        let limits_changed = proposal.changed.iter().any(|k| k == "only");
        let limits = div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .when(server.only.is_empty(), |d| {
                d.child(div().text_color(p.faint).child("None"))
            })
            .children(server.only.iter().map(|(argument, values)| {
                let said = format!("{argument}: {}", values.join(", "));
                div()
                    .id(SharedString::from(format!(
                        "connection-request-limit-{argument}"
                    )))
                    .aria_label(SharedString::from(said.clone()))
                    .child(SharedString::from(said))
                    .test_support()
            }))
            .children(
                proposal
                    .before
                    .as_ref()
                    .filter(|_| limits_changed)
                    .map(|before| {
                        let said = match before.only.is_empty() {
                            true => "None".to_owned(),
                            false => before
                                .only
                                .iter()
                                .map(|(a, v)| format!("{a}: {}", v.join(", ")))
                                .collect::<Vec<_>>()
                                .join("; "),
                        };
                        div()
                            .id("connection-request-was-only")
                            .flex()
                            .gap(space::XS)
                            .text_color(p.faint)
                            .aria_label(SharedString::from(said.clone()))
                            .child("−")
                            .child(SharedString::from(said))
                            .test_support()
                    }),
            )
            .children(
                limits_changed.then(|| Tag::new("connection-request-changed-only", "Changed")),
            );
        body = body.child(Field::new(
            "connection-request-limits-field",
            "Limits on arguments",
            limits,
        ));
        if !proposal.settings.is_empty() {
            let settings =
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .children(proposal.settings.iter().map(|(action, setting)| {
                        let had = server
                            .actions
                            .iter()
                            .find(|(name, _)| name == action)
                            .map_or("judged", |(_, had)| had.as_str());
                        let said = format!(
                            "{action}: {} → {}",
                            setting_said(had),
                            setting_said(setting)
                        );
                        div()
                            .id(SharedString::from(format!(
                                "connection-request-setting-{action}"
                            )))
                            .aria_label(SharedString::from(said.clone()))
                            .child(SharedString::from(said))
                            .test_support()
                    }));
            body = body.child(Field::new(
                "connection-request-settings-field",
                super::connections::WHEN_CALLED,
                settings,
            ));
        }
        body.into_any_element()
    }

    /// Where each vault entry goes, one line each: in the warning colour, tagged, when it goes
    /// elsewhere than the entry's address says, or to a program's environment.
    fn secrets(&self, proposal: &Proposal, cx: &Context<Self>) -> AnyElement {
        let p = palette(cx);
        let lines = div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .children(proposal.secrets.iter().map(|secret| {
                let id = format!("connection-request-secret-{}-{}", secret.key, secret.name);
                let said = secret.said();
                let tag = match (secret.differs, secret.unbound()) {
                    (_, true) => Some(("unbound", "No address")),
                    (true, false) => Some((
                        "elsewhere",
                        match &secret.host {
                            Some(_) => "Another host",
                            None => "A program",
                        },
                    )),
                    (false, false) => None,
                };
                div()
                    .flex()
                    .items_start()
                    .gap(space::SM)
                    .child(
                        div()
                            .id(SharedString::from(id.clone()))
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_color(match secret.differs {
                                true => p.wait,
                                false => p.text,
                            })
                            .aria_label(SharedString::from(said.clone()))
                            .child(SharedString::from(said))
                            .test_support(),
                    )
                    .children(tag.map(|(part, said)| {
                        Tag::new(SharedString::from(format!("{id}-{part}")), said)
                    }))
            }));
        // Each sentence says what is true of the lines it is about.
        let elsewhere = proposal
            .secrets
            .iter()
            .any(|s| s.differs && !s.unbound())
            .then_some(
                "A secret here goes elsewhere than the address its vault entry names: accept only \
                 if you meant to give it there.",
            );
        let unbound = proposal.secrets.iter().any(Secret::unbound).then_some(
            "A vault entry here is bound to no address, so nothing says it may go to the host \
             this request sends it to: accept only if you meant to give it there.",
        );
        let warning = match (elsewhere, unbound) {
            (None, None) => None,
            (a, b) => Some(SharedString::from(
                [a, b].into_iter().flatten().collect::<Vec<_>>().join(" "),
            )),
        };
        Field::new(
            "connection-request-secrets-field",
            "Where its secrets go",
            div()
                .flex()
                .flex_col()
                .child(lines)
                .child(self.widenings(proposal, cx)),
        )
        .warning(warning)
        .into_any_element()
    }

    /// What the request does that the fields do not say: a removal, a switch.
    fn sentence(proposal: Option<&Proposal>) -> &'static str {
        match proposal.map(|p| p.action.as_str()) {
            Some("remove") => {
                "An agent asks to remove it: its server is stopped and its tool leaves every \
                 agent. Nothing is done until you accept."
            }
            Some("on") => {
                "An agent asks to turn it on: its tool is given to every agent of every project. \
                 Nothing is done until you accept."
            }
            Some("off") => {
                "An agent asks to turn it off: its tool leaves every agent. Nothing is done until \
                 you accept."
            }
            _ => {
                "An agent asks for it. Its command runs with your rights and its tool reaches \
                 every agent of every project: nothing is written until you accept, and what is \
                 written is what you read here."
            }
        }
    }
}

impl Render for ConnectionRequest {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let proposal = self.proposal.clone();
        let ready = proposal.as_ref().and_then(|p| p.as_ref().ok());
        let title = ready.map_or_else(|| "An agent's request".to_owned(), Proposal::title);
        let this = cx.entity().downgrade();
        let mut modal = Modal::new("connection-request", title)
            .wide()
            .open(self.open)
            .sentence(Self::sentence(ready))
            .on_close(move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            });
        let skeleton = div()
            .flex()
            .flex_col()
            .gap(space::LG)
            .children((0..4).map(|n| {
                Skeleton::new(ElementId::Name(
                    format!("connection-request-skeleton-{n}").into(),
                ))
                .h(px(40.))
            }));
        let content = match &proposal {
            Some(Ok(proposal)) => self.fields(proposal, cx),
            Some(Err(why)) => div()
                .id("connection-request-unread")
                .text_color(palette(cx).fail)
                .aria_label(SharedString::from(why.clone()))
                .child(SharedString::from(why.clone()))
                .test_support()
                .into_any_element(),
            None => div().into_any_element(),
        };
        let body = div()
            .id("connection-request-body")
            .max_h(window.viewport_size().height - px(240.))
            .overflow_y_scroll()
            .child(
                div()
                    .id("connection-request-fields")
                    .child(awaiting(
                        "connection-request-awaiting",
                        proposal.is_none(),
                        skeleton,
                        content,
                    ))
                    .test_support(),
            );
        // A request whose declaration changed since it was asked would not be done: it is said,
        // and Accept is not offered.
        let stale = ready.filter(|p| !p.current).map(|p| {
            SharedString::from(match &p.why {
                Some(why) => format!("This request can no longer be done: {why}."),
                None => "The declaration changed since the agent asked: accepting would do \
                         nothing. Refuse it, and let the agent ask again."
                    .to_owned(),
            })
        });
        // What a modal keeps in sight is a column without a gap: nothing to fold but the lines.
        let stale_line = div()
            .id("connection-request-stale")
            .aria_label(stale.clone().unwrap_or_default())
            .child(problem_line(
                "connection-request-stale-line",
                stale.clone(),
                px(0.),
                cx,
            ))
            .test_support();
        let error = problem_line("connection-request-error", self.error.clone(), px(0.), cx);
        // Beside the answer that waits for an entry: a button to make each one the request names.
        let this = cx.entity().downgrade();
        let new_entries = presence_of(
            "connection-request-new-entries",
            self.waits_for.clone(),
            move |said| {
                let missing = entries_said(&said);
                let several = missing.len() > 1;
                div()
                    .flex()
                    .flex_wrap()
                    .gap(space::SM)
                    .children(missing.into_iter().map(|(entry, host)| {
                        let label = match several {
                            true => format!("New entry {entry}…"),
                            false => "New entry…".to_owned(),
                        };
                        let this = this.clone();
                        Button::secondary(
                            SharedString::from(format!("connection-request-new-entry-{entry}")),
                            SharedString::from(label),
                        )
                        .leading(Icon::Plus)
                        .on_click(move |_, window, cx| {
                            this.update(cx, |this, cx| {
                                this.new_entry(entry.clone(), host.clone(), window, cx)
                            })
                            .ok();
                        })
                    }))
                    .into_any_element()
            },
        )
        .space(space::SM);
        // A connection that waits for the user's review: said first, with the way into it.
        let this = cx.entity().downgrade();
        let review_first = presence_of(
            "connection-request-review-first",
            ready
                .filter(|p| p.review)
                .map(|p| SharedString::from(p.name.clone())),
            move |name| {
                let (said, open) = (
                    format!("{name} waits for your review of its actions first"),
                    name.to_string(),
                );
                let this = this.clone();
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::SM)
                    .child(
                        div()
                            .id("connection-request-review-said")
                            .aria_label(SharedString::from(said.clone()))
                            .child(SharedString::from(said))
                            .test_support(),
                    )
                    .child(
                        Button::secondary("connection-request-review", "Review").on_click(
                            move |_, window, cx| {
                                let open = open.clone();
                                this.update(cx, |this, cx| this.review(open, window, cx))
                                    .ok();
                            },
                        ),
                    )
                    .into_any_element()
            },
        )
        .space(space::SM);
        modal = modal
            .kept(review_first)
            .child(body)
            .kept(stale_line)
            .kept(error)
            .kept(new_entries);
        let decided = self.deciding;
        modal = modal.action(
            Button::quiet("connection-request-refuse", "Refuse")
                .disabled(decided || self.number.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.refuse(cx))),
        );
        if ready.is_some_and(Proposal::editable) {
            modal = modal.action(
                Button::secondary("connection-request-edit", "Edit")
                    .disabled(decided)
                    .on_click(cx.listener(|this, _, window, cx| this.edit(window, cx))),
            );
        }
        if ready.is_some_and(|p| p.current) {
            modal = modal.action(
                Button::primary(
                    "connection-request-accept",
                    match decided {
                        true => "Accepting…",
                        false => "Accept",
                    },
                )
                .disabled(decided || self.widening)
                .on_click(cx.listener(|this, _, _, cx| this.accept(cx))),
            );
        }
        div().child(modal).child(self.confirmation(cx))
    }
}
