//! The settings of all machines a machine often overrides (`docs/remote.md`, "Settings for all
//! machines"). On All machines, a section for each subject chooses the value every machine
//! follows, saved with `settings.share`, which this device's key signs. On a machine, a section it
//! changed is marked as its own, with the way back to all machines' value (`settings.use_all`),
//! and a value it cannot follow is said there. Everything is read from `settings.shared` and
//! `connection.list`; the daemon checks and keeps.

use std::collections::{BTreeMap, BTreeSet};

use gpui_design_system::{
    Button, Chosen, EmptyLine, Field, Icon, Item, Listing, ModelChoice, ModelEntry, Section,
    Segmented, SetHere, Switch, Toast, palette, presence, presence_of, space, toast,
};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AnyElement, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _, Window,
    div, px,
};
use serde_json::{Map, Value, json};

use crate::about::USE_ALL;
use crate::actions::Command;
use crate::commands::{boxed, line_field, perform_later};

/// The sections of All machines, top to bottom, after About you.
pub const SECTIONS: [&str; 6] = [
    "all-new-sessions",
    "all-models",
    "all-sessions",
    "all-workspaces",
    "all-updates",
    "all-connections",
];

/// The values each section of All machines chooses.
fn names_of(section: &str) -> &'static [&'static str] {
    match section {
        "all-new-sessions" => &["new_session"],
        "all-models" => &[
            "judge",
            "compactor",
            "review.spec_model",
            "review.bugs_model",
            "review.standards_model",
            "review.security_model",
            "review.verifier_model",
        ],
        "all-sessions" => &["sessions.end_inactive_days", "sessions.resume_at_reset"],
        "all-workspaces" => &["workspaces_folder"],
        "all-updates" => &["update.check"],
        "all-connections" => &["connections"],
        _ => &[],
    }
}

/// The label of each model of the "Models" section, and of the new sessions'.
fn model_label(name: &str) -> &'static str {
    match name {
        "new_session" => "Start on",
        "judge" => "Auto judge",
        "compactor" => "Compaction model",
        "review.spec_model" => "Review: the spec",
        "review.bugs_model" => "Review: bugs",
        "review.standards_model" => "Review: project rules",
        "review.security_model" => "Review: security",
        _ => "Review: checking the findings",
    }
}

/// The values a block of a machine's page may hold as its own, by the block's key.
fn names_on_a_machine(part: &str) -> &'static [&'static str] {
    match part {
        "new-sessions" => &["new_session"],
        "judge" => &["judge"],
        "sessions" => &["sessions.end_inactive_days", "sessions.resume_at_reset"],
        "workspaces" => &["workspaces_folder"],
        "version" => &["update.check"],
        "compaction" => &["compactor"],
        "review" => &[
            "review.spec_model",
            "review.bugs_model",
            "review.standards_model",
            "review.security_model",
            "review.verifier_model",
        ],
        "connections" => &["connections"],
        _ => &[],
    }
}

/// What a block of a machine says stays its own, whatever All machines holds.
fn stays(part: &str) -> Option<&'static str> {
    match part {
        "appearance" => Some(APPEARANCE_STAYS),
        "version" => Some(CHANNEL_STAYS),
        _ => None,
    }
}

pub const APPEARANCE_STAYS: &str = "The colours of this machine's desktop theme (Omarchy) stay \
                                    this machine's.";
pub const CHANNEL_STAYS: &str = "The update channel stays this machine's.";
/// What All machines says when none of this machine's connections can be shared.
pub const NO_CONNECTION: &str = "No https:// connection of this machine can be shared: add one \
                                 that names no vault entry.";

/// The choice of a model that shares nothing.
const OWN: &str = "own";
/// The choice of no judge, or of no compaction model.
const NONE: &str = "none";

/// A connection All machines may share: this machine's, or one shared already.
#[derive(Clone, Debug, PartialEq)]
pub struct Shareable {
    pub name: String,
    pub url: String,
    /// What `settings.share` takes for it: its declaration.
    pub declaration: Value,
}

/// What the daemon says (`settings.shared`), and the connections All machines may share.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// All machines' values by name, with a hub.
    pub all: Option<BTreeMap<String, String>>,
    /// The values the machine holds itself.
    pub own: Vec<String>,
    /// Why a value is ignored on the machine, by name.
    pub ignored: BTreeMap<String, String>,
    /// Where a change of all machines went: `given`, `later` or `old-hub`.
    pub hub: Option<String>,
    pub connections: Vec<Shareable>,
}

impl Shown {
    pub fn of(shared: &Value, connections: &Value) -> Shown {
        let texts = |value: &Value| -> Option<BTreeMap<String, String>> {
            Some(
                value
                    .as_object()?
                    .iter()
                    .filter_map(|(name, text)| Some((name.clone(), text.as_str()?.to_owned())))
                    .collect(),
            )
        };
        let all = texts(&shared["all"]);
        let mut shareable: Vec<Shareable> = connections["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(shareable)
            .collect();
        // One shared already that this machine runs otherwise stays listed, as shared.
        for declaration in all
            .iter()
            .filter_map(|all| all.get("connections"))
            .filter_map(|value| serde_json::from_str::<Vec<Value>>(value).ok())
            .flatten()
        {
            let name = declaration["name"].as_str().unwrap_or_default().to_owned();
            let url = declaration["url"].as_str().unwrap_or_default().to_owned();
            match shareable.iter_mut().find(|known| known.name == name) {
                Some(known) => (known.url, known.declaration) = (url, declaration),
                None => shareable.push(Shareable {
                    name,
                    url,
                    declaration,
                }),
            }
        }
        shareable.sort_by(|a, b| a.name.cmp(&b.name));
        Shown {
            all,
            own: shared["own"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            ignored: texts(&shared["ignored"]).unwrap_or_default(),
            hub: shared["hub"].as_str().map(str::to_owned),
            connections: shareable,
        }
    }

    fn value(&self, name: &str) -> &str {
        self.all
            .as_ref()
            .and_then(|all| all.get(name))
            .map_or("", String::as_str)
    }

    /// The names of the connections All machines shares.
    fn shared_connections(&self) -> BTreeSet<String> {
        serde_json::from_str::<Vec<Value>>(self.value("connections"))
            .unwrap_or_default()
            .iter()
            .filter_map(|declaration| declaration["name"].as_str().map(str::to_owned))
            .collect()
    }
}

/// A listed connection that may be shared: reached over `https://`, naming no vault entry, with
/// nothing wrong in its address. Its declaration is the listing's keys alone.
fn shareable(listed: &Value) -> Option<Shareable> {
    let name = listed["name"].as_str()?.to_owned();
    let url = listed["url"].as_str()?.to_owned();
    let named = ["env", "headers", "secrets", "command"]
        .iter()
        .any(|key| !listed[key].is_null());
    if !url.starts_with("https://") || named || !listed["url_fault"].is_null() {
        return None;
    }
    let declaration: Map<String, Value> = [
        "name",
        "about",
        "url",
        "only",
        "except",
        "client_id",
        "enabled",
    ]
    .iter()
    .filter(|key| !listed[**key].is_null())
    .map(|key| (key.to_string(), listed[*key].clone()))
    .collect();
    Some(Shareable {
        name,
        url,
        declaration: Value::Object(declaration),
    })
}

/// A model as a value says it: the choice and the effort.
fn chosen_of(value: &str) -> (Chosen, Option<String>) {
    match value {
        "" => (Chosen::Choice(OWN.into()), None),
        NONE => (Chosen::Choice(NONE.into()), None),
        value => {
            let slot: Value = serde_json::from_str(value).unwrap_or_default();
            let text = |key: &str| slot[key].as_str().map(str::to_owned);
            match text("provider") {
                Some(provider) => (
                    Chosen::Model(provider.into(), text("model").map(Into::into)),
                    text("effort"),
                ),
                None => (Chosen::Choice(OWN.into()), None),
            }
        }
    }
}

/// The value a choice of a model is, as `settings.share` takes it.
fn value_of(chosen: &Chosen, effort: Option<&str>) -> String {
    match chosen {
        Chosen::Choice(choice) if choice.as_ref() == NONE => NONE.to_owned(),
        Chosen::Model(provider, model) => {
            let mut slot = json!({"provider": provider.to_string()});
            if let Some(model) = model {
                slot["model"] = json!(model.to_string());
            }
            if let Some(effort) = effort {
                slot["effort"] = json!(effort);
            }
            slot.to_string()
        }
        _ => String::new(),
    }
}

/// What the sections of All machines hold that is not saved yet.
#[derive(Default)]
struct Edits {
    models: BTreeMap<&'static str, (Chosen, Option<String>)>,
    resume: Option<usize>,
    updates: Option<usize>,
    connections: Option<BTreeSet<String>>,
}

#[derive(Clone)]
struct Fields {
    days: Entity<InputState>,
    folder: Entity<InputState>,
}

/// The two ends of a choice between nothing shared and two values.
const RESUME: [&str; 3] = ["", "true", "false"];
const UPDATES: [&str; 3] = ["", "automatically", "never"];

pub struct AllSettings {
    shell: crate::commands::ShellOf,
    shown: Option<Shown>,
    edits: Edits,
    fields: Option<Fields>,
    /// The connections of All machines' list, as they come and go.
    rows: Listing<Shareable>,
    /// A save, or the way back to all machines' value, waits for the daemon.
    waiting: bool,
    _subscriptions: Vec<Subscription>,
}

impl AllSettings {
    pub fn new(shell: crate::commands::ShellOf) -> Self {
        Self {
            shell,
            shown: None,
            edits: Edits::default(),
            fields: None,
            rows: Listing::default(),
            waiting: false,
            _subscriptions: Vec::new(),
        }
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    pub fn days_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|fields| &fields.days)
    }

    pub fn folder_field(&self) -> Option<&Entity<InputState>> {
        self.fields.as_ref().map(|fields| &fields.folder)
    }

    /// The values of the block `part` of a machine's page that the machine holds itself.
    pub fn own_of(&self, part: &str) -> Vec<String> {
        let Some(shown) = &self.shown else {
            return Vec::new();
        };
        let names = names_on_a_machine(part);
        shown
            .own
            .iter()
            .filter(|own| {
                names.contains(&own.as_str())
                    || (part == "connections" && own.starts_with("connections."))
            })
            .cloned()
            .collect()
    }

    /// Why a value of the block `part` is ignored on the machine, each said once.
    fn ignored_of(&self, part: &str) -> Option<SharedString> {
        let shown = self.shown.as_ref()?;
        let said: Vec<String> = names_on_a_machine(part)
            .iter()
            .filter_map(|name| {
                let why = shown.ignored.get(*name)?;
                Some(match part {
                    "review" => format!("{}: {why}.", model_label(name)),
                    _ => format!("All machines' value: {why}."),
                })
            })
            .collect();
        (!said.is_empty()).then(|| said.join("\n").into())
    }

    /// Drops the machine's own values `names`: they follow all machines again.
    pub fn use_all(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        if self.waiting || names.is_empty() {
            return;
        }
        self.waiting = true;
        perform_later(&self.shell, Command::AllSettingsUseAll { names }, cx);
        cx.notify();
    }

    /// What the daemon holds of `name`, as a field or a choice shows it.
    fn text(&self, name: &str) -> String {
        self.shown
            .as_ref()
            .map_or_else(String::new, |shown| shown.value(name).to_owned())
    }

    /// The values `section` would change, by name: those that differ from all machines'.
    fn changed(&self, section: &str, cx: &gpui_kit::App) -> Vec<(String, String)> {
        let Some(shown) = self.shown.as_ref().filter(|shown| shown.all.is_some()) else {
            return Vec::new();
        };
        let mut changed = Vec::new();
        for name in names_of(section) {
            let was = shown.value(name);
            let now = match *name {
                "sessions.end_inactive_days" => self
                    .fields
                    .as_ref()
                    .map(|fields| fields.days.read(cx).value().trim().to_owned()),
                "workspaces_folder" => self
                    .fields
                    .as_ref()
                    .map(|fields| fields.folder.read(cx).value().trim().to_owned()),
                "sessions.resume_at_reset" => self.edits.resume.map(|at| RESUME[at].to_owned()),
                "update.check" => self.edits.updates.map(|at| UPDATES[at].to_owned()),
                "connections" => self.edits.connections.as_ref().and_then(|chosen| {
                    (*chosen != shown.shared_connections()).then(|| {
                        let declarations: Vec<&Value> = shown
                            .connections
                            .iter()
                            .filter(|connection| chosen.contains(&connection.name))
                            .map(|connection| &connection.declaration)
                            .collect();
                        match declarations.is_empty() {
                            true => String::new(),
                            false => json!(declarations).to_string(),
                        }
                    })
                }),
                name => self.edits.models.get(name).and_then(|(chosen, effort)| {
                    ((chosen.clone(), effort.clone()) != chosen_of(was))
                        .then(|| value_of(chosen, effort.as_deref()))
                }),
            };
            if let Some(now) = now.filter(|now| now != was) {
                changed.push(((*name).to_owned(), now));
            }
        }
        changed
    }

    /// Saves what `section` changed, for all machines.
    pub fn save(&mut self, section: &'static str, cx: &mut Context<Self>) {
        let values = self.changed(section, cx);
        if self.waiting || values.is_empty() {
            return;
        }
        self.waiting = true;
        perform_later(&self.shell, Command::AllSettingsShare { values }, cx);
        cx.notify();
    }

    /// The daemon's answer to a read, a save or the way back (`written`: `Some(true)` a save).
    pub fn answered(
        &mut self,
        said: Result<Shown, String>,
        written: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if written.is_some() {
            self.waiting = false;
        }
        match (written, &said) {
            (Some(true), Ok(shown)) => toast(
                match shown.hub.as_deref() {
                    Some("later") => Toast::success("Saved on this machine").detail(
                        "The hub is out of reach: the other machines get it once it answers.",
                    ),
                    Some("old-hub") => Toast::failure("Saved on this machine only").detail(
                        "The hub is of a version that keeps no settings for all machines: \
                         update it.",
                    ),
                    _ => Toast::success("Saved for all machines")
                        .detail("A machine that chose its own keeps it."),
                },
                cx,
            ),
            (Some(true), Err(why)) => {
                toast(Toast::failure("Not saved").detail(why.clone()), cx);
            }
            (Some(false), Ok(_)) => toast(Toast::success("Using all machines' value"), cx),
            (Some(false), Err(why)) => toast(
                Toast::failure("All machines' value is not used").detail(why.clone()),
                cx,
            ),
            (None, _) => {}
        }
        if let Ok(mut shown) = said {
            // A save's answer names no connection: those read before stay listed.
            if written.is_some()
                && let Some(before) = &self.shown
            {
                for connection in &before.connections {
                    if shown
                        .connections
                        .iter()
                        .all(|known| known.name != connection.name)
                    {
                        shown.connections.push(connection.clone());
                    }
                }
                shown.connections.sort_by(|a, b| a.name.cmp(&b.name));
            }
            if written == Some(true) {
                self.edits = Edits::default();
            }
            if let Some(fields) = &self.fields {
                for (field, name) in [
                    (&fields.days, "sessions.end_inactive_days"),
                    (&fields.folder, "workspaces_folder"),
                ] {
                    let was = self.text(name);
                    let typed = field.read(cx).value().to_string();
                    // Typing in progress is kept when a read comes back.
                    if written == Some(true) || typed == was {
                        let now = shown.value(name).to_owned();
                        field.update(cx, |field, cx| field.set_value(now, window, cx));
                    }
                }
            }
            self.shown = Some(shown);
        }
        cx.notify();
    }

    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Fields {
        if let Some(fields) = &self.fields {
            return fields.clone();
        }
        let days = line_field("30, or off", window, cx);
        let folder = line_field("~/work/<project>-ws", window, cx);
        for (field, name) in [
            (&days, "sessions.end_inactive_days"),
            (&folder, "workspaces_folder"),
        ] {
            let text = self.text(name);
            field.update(cx, |field, cx| field.set_value(text, window, cx));
            self._subscriptions
                .push(cx.subscribe(field, |_, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }));
        }
        let fields = Fields { days, folder };
        self.fields = Some(fields.clone());
        fields
    }

    /// The model `name`, as every model of Settings is chosen: each machine's own, none where
    /// that is a choice, or a model of this machine's providers.
    fn model(
        &self,
        name: &'static str,
        models: Vec<ModelEntry>,
        cx: &mut Context<Self>,
    ) -> ModelChoice {
        let (chosen, effort) = self
            .edits
            .models
            .get(name)
            .cloned()
            .unwrap_or_else(|| chosen_of(&self.text(name)));
        let this = cx.entity().downgrade();
        let mut choice = ModelChoice::new(format!("all-{name}"), model_label(name), chosen).choice(
            OWN,
            "Each machine's own",
            "Nothing is shared: each machine keeps what it has.",
        );
        choice = match name {
            "judge" => choice.choice(NONE, "No judge", "Auto asks like Ask."),
            "compactor" => choice.choice(NONE, "No compaction model", "The agents' own."),
            _ => choice,
        };
        choice
            .models(models)
            .effort(effort.map(Into::into))
            .disabled(self.shown.is_none() || self.waiting)
            .on_choose(move |chosen, effort, _, cx| {
                this.update(cx, |this, cx| {
                    let effort = effort.map(|effort| effort.to_string());
                    this.edits.models.insert(name, (chosen, effort));
                    cx.notify();
                })
                .ok();
            })
    }

    /// The section `section` of All machines, its models offered from `models` (without
    /// `efforts` for the judge, which takes none).
    pub fn section(
        &mut self,
        section: &'static str,
        models: Vec<ModelEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let card = || {
            div()
                .p(space::LG)
                .flex()
                .flex_col()
                .gap(space::MD)
                .rounded(px(12.))
                .bg(p.card)
                .border_1()
                .border_color(p.line)
        };
        let (title, sentence, body): (&str, &str, AnyElement) = match section {
            "all-new-sessions" => (
                "New sessions",
                "What a session starts on, on every machine that chose nothing.",
                card()
                    .child(self.model("new_session", models, cx))
                    .into_any_element(),
            ),
            "all-models" => {
                let without_efforts: Vec<ModelEntry> = models
                    .iter()
                    .map(|model| model.clone().efforts(Vec::<SharedString>::new()))
                    .collect();
                let mut body = card();
                for name in names_of(section) {
                    let offered = match *name {
                        "judge" => without_efforts.clone(),
                        _ => models.clone(),
                    };
                    body = body.child(self.model(name, offered, cx));
                }
                (
                    "Models",
                    "A machine that lacks a model ignores it there, and says so.",
                    body.into_any_element(),
                )
            }
            "all-sessions" => {
                let fields = self.fields(window, cx);
                let chosen = self.edits.resume.unwrap_or_else(|| {
                    let value = self.text("sessions.resume_at_reset");
                    RESUME.iter().position(|v| *v == value).unwrap_or(0)
                });
                let this = cx.entity().downgrade();
                let resume = Segmented::new(
                    "all-resume",
                    "Resume when the quota resets",
                    [("own", "Each machine's own"), ("on", "On"), ("off", "Off")],
                    chosen,
                )
                .on_choose(move |index, _, cx| {
                    this.update(cx, |this, cx| {
                        this.edits.resume = Some(index);
                        cx.notify();
                    })
                    .ok();
                });
                (
                    "Sessions",
                    "",
                    card()
                        .child(
                            Field::new(
                                "all-days",
                                "End inactive sessions after (days)",
                                boxed("all-days-box", Input::new(&fields.days), cx),
                            )
                            .optional()
                            .hint("1 to 90, or off. Empty: each machine's own."),
                        )
                        .child(resume)
                        .into_any_element(),
                )
            }
            "all-workspaces" => {
                let fields = self.fields(window, cx);
                (
                    "Workspaces folder",
                    "",
                    card()
                        .child(
                            Field::new(
                                "all-folder",
                                "Where workspaces go",
                                boxed("all-folder-box", Input::new(&fields.folder), cx),
                            )
                            .optional()
                            .hint(
                                "A folder under your home holding <project>. Empty: each \
                                 machine's own.",
                            ),
                        )
                        .into_any_element(),
                )
            }
            "all-updates" => {
                let chosen = self.edits.updates.unwrap_or_else(|| {
                    let value = self.text("update.check");
                    UPDATES.iter().position(|v| *v == value).unwrap_or(0)
                });
                let this = cx.entity().downgrade();
                let check = Segmented::new(
                    "all-update-check",
                    "Check for updates",
                    [
                        ("own", "Each machine's own"),
                        ("automatically", "Automatically"),
                        ("never", "Never"),
                    ],
                    chosen,
                )
                .on_choose(move |index, _, cx| {
                    this.update(cx, |this, cx| {
                        this.edits.updates = Some(index);
                        cx.notify();
                    })
                    .ok();
                });
                (
                    "Updates",
                    CHANNEL_STAYS,
                    card().child(check).into_any_element(),
                )
            }
            _ => (
                "Connections",
                "Reached over https:// and naming no vault entry. Each machine signs in for \
                 itself: no sign-in is shared.",
                self.connection_switches(card(), cx),
            ),
        };
        let save = div()
            .flex()
            .justify_end()
            // Not drawn at rest: it unfolds once something differs from what is saved.
            .child(presence(
                ElementId::Name(format!("{section}-save-block").into()),
                !self.changed(section, cx).is_empty(),
                div().pt(space::MD).child(
                    Button::primary(SharedString::from(format!("{section}-save")), "Save")
                        .disabled(self.waiting)
                        .on_click(cx.listener(move |this, _, _, cx| this.save(section, cx))),
                ),
            ));
        let mut drawn = Section::new(SharedString::from(section.to_owned()), title.to_owned());
        if !sentence.is_empty() {
            drawn = drawn.sentence(sentence.to_owned());
        }
        drawn
            .child(div().flex().flex_col().child(body).child(save))
            .into_any_element()
    }

    /// A switch for each connection that may be shared, in a list: one that comes unfolds, one
    /// that goes folds away.
    fn connection_switches(&mut self, card: gpui_kit::Div, cx: &mut Context<Self>) -> AnyElement {
        let Some(shown) = self.shown.clone() else {
            return card.into_any_element();
        };
        let now = cx.background_executor().now();
        let reduce = cx.reduce_motion();
        let listed = shown.connections.iter();
        self.rows.sync(
            listed.map(|connection| (connection.name.clone(), connection.clone())),
            now,
            reduce,
        );
        let chosen = self
            .edits
            .connections
            .clone()
            .unwrap_or_else(|| shown.shared_connections());
        let mut card = card;
        let drawn: Vec<_> = self
            .rows
            .rows()
            .map(|row| (row.placement, row.value.clone()))
            .collect();
        for (placement, connection) in drawn {
            let name = connection.name.clone();
            let on = chosen.contains(&name);
            let this = cx.entity().downgrade();
            let chosen = chosen.clone();
            let switch = Switch::new(
                SharedString::from(format!("all-connection-{name}")),
                on,
                format!("Share {name} with every machine"),
            )
            .hint(connection.url.clone())
            .disabled(self.waiting)
            .on_click(move |_, _, cx| {
                let mut chosen = chosen.clone();
                match on {
                    true => chosen.remove(&name),
                    false => chosen.insert(name.clone()),
                };
                this.update(cx, |this, cx| {
                    this.edits.connections = Some(chosen);
                    cx.notify();
                })
                .ok();
            });
            let id = ElementId::Name(format!("all-connection-{}-row", connection.name).into());
            card = card.child(Item::new(id, placement, switch).gap(space::MD));
        }
        let none = EmptyLine::new("all-connections-none", Icon::Lucide("plug"), NO_CONNECTION);
        div()
            .flex()
            .flex_col()
            .child(presence(
                "all-connections-list-block",
                !self.rows.is_empty(),
                card,
            ))
            .child(presence(
                "all-connections-none-block",
                shown.connections.is_empty(),
                none,
            ))
            .into_any_element()
    }
}

/// `section`, the block `part` of a machine's page, with what all machines have to do with it
/// while the machine has a hub: what stays the machine's, why a value of all machines is ignored
/// there, and, under it, the mark of what the machine holds itself with the way back. Each unfolds
/// and folds away.
pub fn around(
    shared: &Entity<AllSettings>,
    part: &'static str,
    hub: bool,
    machine: &str,
    section: AnyElement,
    cx: &gpui_kit::App,
) -> AnyElement {
    if stays(part).is_none() && names_on_a_machine(part).is_empty() {
        return section;
    }
    let p = palette(cx);
    let read = shared.read(cx);
    let own = match hub {
        true => read.own_of(part),
        false => Vec::new(),
    };
    let ignored = read.ignored_of(part).filter(|_| hub);
    let line = |id: String, said: SharedString| {
        div().pb(space::SM).child(
            div()
                .id(ElementId::Name(id.into()))
                .aria_label(said.clone())
                .text_size(px(12.5))
                .text_color(p.mute)
                .child(said)
                .test_support(),
        )
    };
    let stays = stays(part).map(|said| {
        presence(
            ElementId::Name(format!("settings-stays-{part}-block").into()),
            hub,
            line(format!("settings-stays-{part}"), said.into()),
        )
    });
    let mine = !own.is_empty();
    let to = shared.downgrade();
    let marked = SetHere::new(
        ElementId::Name(format!("shared-{part}").into()),
        section,
        format!("Changed on {machine}"),
        USE_ALL,
    )
    .set(mine)
    .on_reset(move |_, cx| {
        let own = own.clone();
        to.update(cx, |shared, cx| shared.use_all(own, cx)).ok();
    });
    let muted = p.mute;
    div()
        .flex()
        .flex_col()
        .children(stays)
        .child(presence_of(
            ElementId::Name(format!("shared-{part}-ignored-block").into()),
            ignored,
            move |said: SharedString| {
                div()
                    .pb(space::SM)
                    .child(
                        div()
                            .id(ElementId::Name(format!("shared-{part}-ignored").into()))
                            .aria_label(said.clone())
                            .text_size(px(12.5))
                            .text_color(muted)
                            .child(said)
                            .test_support(),
                    )
                    .into_any_element()
            },
        ))
        .child(marked)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_s_value_says_its_choice_and_is_said_back() {
        for value in [
            "",
            "none",
            r#"{"effort":"high","model":"large","provider":"claude"}"#,
        ] {
            let (chosen, effort) = chosen_of(value);
            let said = value_of(&chosen, effort.as_deref());
            assert_eq!(chosen_of(&said), (chosen, effort), "{value}");
        }
    }

    #[test]
    fn only_a_connection_naming_nothing_over_https_may_be_shared() {
        let listed = |extra: Value| {
            let mut listed = json!({"name": "notes", "about": "Notes.",
                "url": "https://notes.invalid/mcp", "url_fault": null, "state": "ready"});
            for (key, value) in extra.as_object().unwrap() {
                listed[key] = value.clone();
            }
            shareable(&listed)
        };
        let plain = listed(json!({})).unwrap();
        assert_eq!(
            plain.declaration,
            json!({"name": "notes", "about": "Notes.", "url": "https://notes.invalid/mcp"})
        );
        assert!(listed(json!({"secrets": [{"entry": "token"}]})).is_none());
        assert!(listed(json!({"url": "http://127.0.0.1/mcp"})).is_none());
        assert!(listed(json!({"url_fault": "url: no"})).is_none());
    }
}
