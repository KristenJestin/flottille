//! An agent's request to write a connection (#854, `docs/extensions.md`, "Connections"): its card
//! waits in the rail as every held request does, and opens this dialog instead of being allowed
//! from there. The dialog shows what the daemon says the request would write
//! (`connection.proposed`): the fields of "New server", filled and read-only, an edit's changed
//! fields with their old value, a `set`'s settings, then Accept, Edit and Refuse. Accept and
//! Refuse answer the request (Refuse writes nothing, and asks nothing more, as the rail's Deny);
//! Edit opens the server's own
//! form with the agent's values, whose Save answers it. The results are toasts.

use gpui_design_system::{
    Button, Field, Modal, Skeleton, Tag, Toast, awaiting, palette, space, toast,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity,
    Window, div, px,
};
use serde_json::{Value, json};

use super::connections::{Server, command_line};
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
    /// Still what the request says: one that is not would not be done.
    pub current: bool,
    pub why: Option<String>,
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
            current: value["current"] == true,
            why: value["why"].as_str().map(str::to_owned),
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
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
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
        cx.notify();
    }

    /// What the daemon answered to something the dialog asked.
    pub fn answered(&mut self, then: Then, answer: Result<Value, String>, cx: &mut Context<Self>) {
        match then {
            Then::Proposed { number } => {
                if self.number == Some(number) {
                    self.proposal = Some(answer.map(|value| Proposal::of(&value)));
                }
            }
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
                    Ok(_) => {
                        if self.number == Some(number) {
                            self.open = false;
                        }
                        let said = match (accepted, name) {
                            (true, Some((name, action))) => {
                                let doing = match action.as_str() {
                                    "add" => "is being added",
                                    "edit" => "is being saved",
                                    "remove" => "is being removed",
                                    "on" => "is being turned on",
                                    "off" => "is being turned off",
                                    _ => "is being set",
                                };
                                Toast::success(format!(
                                    "Request accepted: the server {name} {doing}"
                                ))
                                .detail("The agent is told how it went.")
                            }
                            (true, None) => Toast::success(format!("Request #{number} accepted")),
                            (false, _) => Toast::success("Request refused: nothing was written")
                                .detail("The agent is told you refused it."),
                        };
                        toast(said, cx);
                    }
                    Err(why) => match self.number == Some(number) && self.open {
                        true => self.error = Some(why.into()),
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
            let entries = match s.url {
                Some(_) => &s.headers,
                None => &s.env,
            };
            match entries.is_empty() {
                true => "None".to_owned(),
                false => entries
                    .iter()
                    .map(|(name, entry)| format!("{name} from the vault entry {entry}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
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
            Some(_) => ("Headers of its requests", "headers"),
            None => ("Variables of its environment", "env"),
        };
        body = body.child(self.field(
            "variables",
            variables,
            entries(server),
            was(key, &entries),
            cx,
        ));
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
        let stale_line = div()
            .id("connection-request-stale")
            .aria_label(stale.clone().unwrap_or_default())
            .child(problem_line(
                "connection-request-stale-line",
                stale.clone(),
                space::LG,
                cx,
            ))
            .test_support();
        let error = problem_line(
            "connection-request-error",
            self.error.clone(),
            space::LG,
            cx,
        );
        modal = modal.child(body).kept(stale_line).kept(error);
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
                .disabled(decided)
                .on_click(cx.listener(|this, _, _, cx| this.accept(cx))),
            );
        }
        modal
    }
}
