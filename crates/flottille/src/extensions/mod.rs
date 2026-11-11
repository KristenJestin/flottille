//! The extensions (`docs/extensions.md`): parts of Flottille that declare what they bring, which
//! the core runs and the user turns on or off. This module is the registry, the only place of the
//! daemon that names them, and the frame that keeps their switches, their settings and their
//! tables.

mod command;
mod connections;
mod guidance;
mod review;
mod specs;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use clap::Subcommand;
use flottille_core::agent::Agent;
use flottille_core::approval;
use flottille_core::paths::Paths;
use flottille_core::project::Project;
use flottille_core::store::Store;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::daemon::Daemon;
use crate::gate::Verdict;
use crate::mcp::{Definition, Grant, Outcome};
use crate::projects::{self, Failure};
use crate::providers::{self, Provider};

pub use command::run;

/// Every extension, in the order of their settings' tabs.
pub const ALL: &[&Extension] = &[
    &specs::EXTENSION,
    &review::EXTENSION,
    &connections::EXTENSION,
];

/// The command lines of the extensions, beside the frame's own `extension`.
#[derive(Subcommand)]
pub enum Command {
    /// Turn a part of Flottille on or off, for all projects or one: specs, review
    Extension {
        #[command(subcommand)]
        action: command::ExtensionAction,
    },
    /// The specs of your projects: what an agent needs to build a task without guessing
    Spec {
        #[command(subcommand)]
        action: specs::command::SpecAction,
    },
    /// Review a session's change with fresh eyes: what is wrong is delivered to it
    Review {
        /// The session, by its id
        session: String,
        #[arg(long)]
        json: bool,
    },
    /// Your MCP servers, each given to every agent as one tool once you reviewed it
    Connection {
        #[command(subcommand)]
        action: connections::command::ConnectionAction,
    },
}

/// What an extension declares. Each kind enters the type with the first extension that needs it
/// (`docs/extensions.md`, "The declaration").
pub struct Extension {
    /// Stable: its settings' keys, its events, its methods, its store tables.
    pub name: &'static str,
    pub title: &'static str,
    /// What it does and what it costs: the line its settings show alone while it is off.
    pub sentence: &'static str,
    pub scope: Scope,
    pub on_by_default: bool,
    /// The label of its switch: what turning it on does (`Review changes`), never its state.
    pub switch: &'static str,
    /// Where and how it acts, shown on its page even while it is off; the words found elsewhere
    /// in the window are between `**` (`A recap starts when you type **/recap** in a composer.`).
    pub usage: &'static str,
    /// Where its settings appear in the window.
    pub place: Place,
    pub settings: &'static [Setting],
    /// Its tables, `ext_<name>_…`, applied in order by the store; never edited once released.
    pub migrations: &'static [&'static str],
    /// The protocol methods it answers, `<prefix>.*`.
    pub methods: Methods,
    /// The Action trigger: the user's gestures it answers, each by its method
    /// `<prefix>.<action>`, reached too through `extension.act {name, action, …}`.
    pub actions: &'static [&'static str],
    /// The tools it gives to agents while it is active.
    pub tools: &'static [Tool],
    /// Whether the agent is a session it started (`start_session`).
    pub started: fn(&Daemon, &str) -> bool,
    /// The Core event trigger: the events of the core it hears (`item.changed`, `agent.ended`,
    /// `agent.resumed`…), each with its handler, called once the event is stored, on the thread
    /// that stored it: the request that caused it (`agent.stop`, `agent.resume`) answers after the
    /// handlers ran, so a handler stays short. A handler wakes no agent but through a delivery
    /// (`tell`).
    pub heard: &'static [(&'static str, Handler)],
    /// What it adds to what the core says of an agent (`agent.tree`'s `extensions.<name>`), or
    /// nothing.
    pub fact: fn(&Daemon, &str) -> Option<Value>,
    /// Which of its agents the agent is, when it launched it (`launch`): the kind its tools are
    /// given to (`To::Launched`).
    pub launched: fn(&Daemon, &str) -> Option<&'static str>,
    /// The Composer command trigger: each `/<word>` a session's composer runs as Flottille's,
    /// never sent to the agent, with its handler, given the session.
    pub commands: &'static [(&'static str, Typed)],
    /// Its editable guidance, `<name>.md` beside the settings (`<name>.show`, `.set`, `.reset`).
    pub guidance: Option<Guidance>,
    /// What the user declares under its table beyond its settings: a list of tables of its own
    /// (`[[connections.server]]`), checked by the extension.
    pub declared: Option<Declared>,
    /// The tools it gives every agent of every project while it is active, named at run time
    /// from what the user declared (`conn_<name>`).
    pub dynamic: Option<Dynamic>,
    /// Its events the user's inbox lists, each with the text it shows: what needs the user.
    pub needs_you: &'static [(&'static str, Says)],
}

/// A list of tables an extension's table holds (`[[<name>.<key>]]`), each checked by `check`;
/// `Err` names the key at fault, under `at`.
#[derive(Clone, Copy)]
pub struct Declared {
    pub key: &'static str,
    pub check: fn(&[Value], &str) -> Result<(), String>,
}

/// Tools an extension names at run time. Each is served and gated as a declared tool is.
#[derive(Clone, Copy)]
pub struct Dynamic {
    /// The tools it gives every agent of the project now; called only while it is on.
    pub given: fn(&Daemon, &Project) -> Vec<&'static Definition>,
    /// One of its tools by name, given now or not: what a grant serves.
    pub definition: fn(&str) -> Option<&'static Definition>,
    /// The gate's verdict on a call of one of its tools.
    pub classify: fn(&Daemon, &Grant, &str, &Value) -> Verdict,
    pub call: fn(&Arc<Daemon>, &Grant, &str, Value) -> Called,
    /// What the judge reads of a call besides its arguments, in a block of its own, or nothing.
    pub described: fn(&Daemon, &str, &Value) -> Option<String>,
}

/// What the extensions keep while the daemon runs (a connection's server), each its own part.
#[derive(Default)]
pub struct Live {
    connections: connections::Live,
}

/// Stops what the extensions keep running, as the daemon stops.
pub async fn stop_all(daemon: &Arc<Daemon>) {
    connections::stop_all(daemon).await;
}

/// What the inbox shows of an event of an extension's.
pub type Says = fn(&Value) -> String;

/// What runs a composer command, given the session it was typed in.
pub type Typed = fn(Arc<Daemon>, Agent) -> Answered;

/// An extension's guidance (`docs/extensions.md`, "Guidance"): a text its agents follow, inside
/// its fixed text, which the user edits and resets.
pub struct Guidance {
    pub default: &'static str,
    /// Its length at most, in characters.
    pub limit: usize,
    /// How a refusal names it: "your review's guidance".
    pub named: &'static str,
    /// Its parts, each a guidance of its own read after it by the agents the extension says
    /// (Review: its passes), `<name>-<part>.md`, with no default.
    pub parts: &'static [&'static str],
}

/// What hears an event of the core: its data, as `flottille watch` prints it.
pub type Handler = fn(&Arc<Daemon>, &Value);

/// A tool an extension gives (`docs/extensions.md`, "Tools"): served as the core's are, and
/// through the gate like any other.
pub struct Tool {
    pub definition: Definition,
    /// Who gets it, and the actions of its `action` argument each of them may call.
    pub given: &'static [(To, &'static [&'static str])],
    /// The actions declared harmless, as the core's own reads are: they read or write
    /// Flottille's store and nothing else, and pass in every mode. Any other action is held as a
    /// call to judge, which the mode lets through, asks the judge or the user about.
    pub harmless: &'static [&'static str],
    /// The extension's own refusals of a call (a lock, a status): refused in every mode, Full
    /// included, before anything is asked or run. `Err` is the sentence that says why; `Ok` lets
    /// nothing through on its own, it leaves the call to the gate.
    pub gate: fn(&Daemon, &Grant, &Value) -> Result<(), String>,
    pub call: fn(&Arc<Daemon>, &Grant, Value) -> Called,
}

/// What a call of an extension's tool answers, once its work is done.
pub type Called = BoxFuture<'static, Outcome>;

/// Who a tool is given to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    /// The roots of a project where the extension is active.
    Roots,
    /// The agents of that kind the extension launched (`Extension::launched`).
    Launched(&'static str),
}

/// What an extension touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// It is on in every project once on globally, unless a project turns it off.
    Project,
    /// App-wide: its switch is the global one alone, and a project's file holds nothing of it.
    Global,
}

/// Where an extension's settings appear: a section of a screen that exists comes with the first
/// extension that needs it.
pub enum Place {
    /// A row of Settings' Features, at its place among the rows, which opens its page: its Lucide
    /// icon (by its file name) and the one line that says what it is for.
    Row {
        icon: &'static str,
        line: &'static str,
        order: u32,
    },
}

pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: Kind,
    /// Whether a project may set its own value, under `[extensions.<name>]` of its file.
    pub per_project: bool,
}

/// The kinds of a setting's value: each comes with its first need.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// A language tag (`en`, `fr-FR`); by default the user's, the locale of the daemon.
    Language,
    /// Some of the values `of`, in their order; `default` when none is set.
    Subset {
        of: &'static [&'static str],
        default: &'static [&'static str],
    },
    /// A model slot: `{provider, model?, effort?}`, each checked when it is used; none set leaves
    /// the choice to the extension (`null`). `rule` says what the extension does with a slot left
    /// unset, and `gives` the model it then runs on for a provider: what `extension.list` says of
    /// it, so that no client holds a table of its own.
    Model {
        rule: &'static str,
        gives: fn(&Daemon, &'static Provider) -> Option<String>,
    },
}

/// What an extension's method answers, once its work is done.
pub type Answered = BoxFuture<'static, Result<Value, Failure>>;

pub struct Methods {
    pub prefix: &'static str,
    pub call: fn(Arc<Daemon>, String, Value) -> Answered,
}

/// One extension's table, as `extensions.toml` or a project's file holds it, checked.
#[derive(Clone, Debug, Default, PartialEq)]
struct Values {
    enabled: Option<bool>,
    settings: BTreeMap<String, Value>,
    /// Its declared tables (`Extension::declared`), as written.
    declared: Vec<Value>,
}

/// Held while `extensions.toml` is read and written again.
static WRITING: Mutex<()> = Mutex::new(());

/// Refuses extensions whose tools could not be told apart from another tool: one named as a tool
/// of the core, which it would shadow, or as another extension's. Checked at the daemon's start.
pub fn check_tools(extensions: &[&Extension]) -> Result<(), String> {
    let mut names = Vec::new();
    for extension in extensions {
        for tool in extension.tools {
            let name = tool.definition.name;
            if crate::mcp::serves(name) {
                return Err(format!(
                    "the extension {} gives a tool {name}, the name of a core tool",
                    extension.name
                ));
            }
            if names.contains(&name) {
                return Err(format!(
                    "the extension {} gives a tool {name}, which another extension gives",
                    extension.name
                ));
            }
            names.push(name);
        }
    }
    Ok(())
}

/// Brings every extension's tables up to date, at the daemon's start: they are made whether the
/// extension is on or off, and kept when it is turned off.
pub fn migrate(store: &mut Store) -> flottille_core::Result<()> {
    for extension in ALL {
        store.migrate_extension(extension.name, extension.migrations)?;
    }
    Ok(())
}

fn file(paths: &Paths) -> std::path::PathBuf {
    paths.config.join("extensions.toml")
}

fn find(name: &str) -> Result<&'static Extension, String> {
    ALL.iter()
        .copied()
        .find(|extension| extension.name == name)
        .ok_or_else(|| {
            let names: Vec<&str> = ALL.iter().map(|extension| extension.name).collect();
            format!(
                "{name}: unknown extension: the extensions are {}",
                names.join(", ")
            )
        })
}

/// A setting's value checked against its kind; the error names what is wrong, not the key.
fn checked(kind: Kind, value: &Value) -> Result<Value, String> {
    match kind {
        Kind::Language => match value.as_str() {
            Some(tag) if language_tag(tag) => Ok(json!(tag)),
            _ => Err(format!(
                "must be a language tag such as en or fr-FR, not {value}"
            )),
        },
        Kind::Subset { of, .. } => {
            let given: Vec<&str> = value
                .as_array()
                .and_then(|values| values.iter().map(Value::as_str).collect())
                .ok_or_else(|| format!("must be a list among {}, not {value}", of.join(", ")))?;
            if let Some(unknown) = given.iter().find(|word| !of.contains(word)) {
                return Err(format!("{unknown} is not one of {}", of.join(", ")));
            }
            // In the declaration's order, each once.
            Ok(json!(
                of.iter()
                    .filter(|word| given.contains(word))
                    .collect::<Vec<_>>()
            ))
        }
        Kind::Model { .. } => {
            let refuse = || {
                format!(
                    "must be a model slot such as {{provider = \"claude\", model = \"opus\", \
                     effort = \"high\"}}, the model and the effort optional, not {value}"
                )
            };
            let slot = value.as_object().ok_or_else(refuse)?;
            if !slot["provider"].is_string()
                || slot.iter().any(|(key, value)| {
                    !["provider", "model", "effort"].contains(&key.as_str()) || !value.is_string()
                })
            {
                return Err(refuse());
            }
            Ok(value.clone())
        }
    }
}

fn language_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let first = parts.next().unwrap_or_default();
    (2..=3).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|part| {
            (2..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

/// The user's language, from the locale of the daemon's environment (`fr_FR.UTF-8` is `fr-FR`);
/// English when it says none.
fn user_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .map(|locale| {
            let base = locale.split(['.', '@']).next().unwrap_or_default();
            base.replace('_', "-")
        })
        .filter(|tag| language_tag(tag))
        .unwrap_or_else(|| "en".into())
}

fn default_of(kind: Kind) -> Value {
    match kind {
        Kind::Language => json!(user_language()),
        Kind::Subset { default, .. } => json!(default),
        Kind::Model { .. } => Value::Null,
    }
}

/// What a model slot left on its default does, for `extension.list`: its rule, and the model it
/// runs on for each provider ready (`null`: the provider's own). `null` for any other kind.
fn default_says(daemon: &Daemon, ready: &[&'static Provider], kind: Kind) -> Value {
    let Kind::Model { rule, gives } = kind else {
        return Value::Null;
    };
    let gives: Map<String, Value> = ready
        .iter()
        .map(|provider| (provider.name.to_owned(), json!(gives(daemon, provider))))
        .collect();
    json!({"rule": rule, "gives": gives})
}

fn kind_word(kind: Kind) -> Value {
    match kind {
        Kind::Language => json!("language"),
        Kind::Subset { of, .. } => json!({"subset": of}),
        Kind::Model { .. } => json!("model"),
    }
}

/// One extension's table checked key by key against its declaration: `enabled` and its settings.
/// `at` is how the table is named in an error (`specs`, `extensions.specs`).
fn values(extension: &Extension, table: &Map<String, Value>, at: &str) -> Result<Values, String> {
    let mut values = Values::default();
    for (key, value) in table {
        if key == "enabled" {
            values.enabled = Some(
                value
                    .as_bool()
                    .ok_or_else(|| format!("{at}.enabled: must be true or false, not {value}"))?,
            );
            continue;
        }
        if let Some(declared) = extension.declared.filter(|declared| declared.key == key) {
            let tables = value
                .as_array()
                .filter(|all| all.iter().all(Value::is_object))
                .ok_or_else(|| {
                    format!("{at}.{key}: must be tables, [[{at}.{key}]], not {value}")
                })?;
            (declared.check)(tables, &format!("{at}.{key}"))?;
            values.declared = tables.clone();
            continue;
        }
        let setting = extension
            .settings
            .iter()
            .find(|setting| setting.key == key)
            .ok_or_else(|| {
                let keys: Vec<&str> = std::iter::once("enabled")
                    .chain(extension.declared.map(|declared| declared.key))
                    .chain(extension.settings.iter().map(|setting| setting.key))
                    .collect();
                format!("{at}.{key}: unknown key: the keys are {}", keys.join(", "))
            })?;
        let value = checked(setting.kind, value).map_err(|why| format!("{at}.{key}: {why}"))?;
        values.settings.insert(key.clone(), value);
    }
    Ok(values)
}

/// `extensions.toml` as a TOML item turned into JSON, for the checks every table goes through.
/// `at` is how the item is named in an error.
fn json_of(item: &toml_edit::Item, at: &str) -> Result<Value, String> {
    match item {
        toml_edit::Item::Value(value) => Ok(json_of_value(value)),
        toml_edit::Item::Table(table) => table
            .iter()
            .map(|(key, item)| Ok((key.to_owned(), json_of(item, &format!("{at}.{key}"))?)))
            .collect::<Result<Map<_, _>, String>>()
            .map(Value::Object),
        toml_edit::Item::ArrayOfTables(tables) => tables
            .iter()
            .map(|table| json_of(&toml_edit::Item::Table(table.clone()), at))
            .collect::<Result<Vec<_>, String>>()
            .map(Value::Array),
        toml_edit::Item::None => Err(format!("{at}: an array of tables is not taken")),
    }
}

/// A setting's value as `extensions.toml` writes it. `null` is taken as the whole value alone (the
/// setting back to its default): TOML has no `null` to keep inside an array or a table.
fn toml_of(value: &Value) -> Result<toml_edit::Value, String> {
    Ok(match value {
        Value::Null => return Err("null is taken only as the whole value, for the default".into()),
        Value::Bool(flag) => (*flag).into(),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => integer.into(),
            None => number
                .as_f64()
                .ok_or_else(|| format!("{number}: not a number TOML can write"))?
                .into(),
        },
        Value::String(text) => text.as_str().into(),
        Value::Array(values) => {
            toml_edit::Value::Array(values.iter().map(toml_of).collect::<Result<_, String>>()?)
        }
        Value::Object(map) => toml_edit::Value::InlineTable(
            map.iter()
                .map(|(key, value)| Ok((key.as_str(), toml_of(value)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}

fn json_of_value(value: &toml_edit::Value) -> Value {
    match value {
        toml_edit::Value::String(text) => json!(text.value()),
        toml_edit::Value::Integer(number) => json!(number.value()),
        toml_edit::Value::Float(number) => json!(number.value()),
        toml_edit::Value::Boolean(flag) => json!(flag.value()),
        toml_edit::Value::Datetime(at) => json!(at.value().to_string()),
        toml_edit::Value::Array(array) => Value::Array(array.iter().map(json_of_value).collect()),
        toml_edit::Value::InlineTable(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| (key.to_owned(), json_of_value(value)))
                .collect(),
        ),
    }
}

/// The text of `extensions.toml`, checked: an extension Flottille does not know, a key it does not
/// take, a value of the wrong kind are refused, naming the key at fault.
fn parse(text: &str) -> Result<BTreeMap<&'static str, Values>, String> {
    let refuse = |message: String| format!("extensions.toml: {message}");
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| refuse(error.message().to_owned()))?;
    let mut all = BTreeMap::new();
    for (name, item) in document.iter() {
        let extension = find(name).map_err(refuse)?;
        let Value::Object(table) = json_of(item, name).map_err(refuse)? else {
            return Err(refuse(format!("{name}: must be a table")));
        };
        all.insert(
            extension.name,
            values(extension, &table, name).map_err(refuse)?,
        );
    }
    Ok(all)
}

/// What `extensions.toml` says; nothing when there is no file.
fn global(paths: &Paths) -> Result<BTreeMap<&'static str, Values>, String> {
    match std::fs::read_to_string(file(paths)) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(format!("extensions.toml: {error}")),
    }
}

/// The project's own table for the extension, checked.
fn of_project(extension: &Extension, project: &Project) -> Result<Values, String> {
    let at = format!("extensions.{}", extension.name);
    match project.extensions.get(extension.name) {
        None => Ok(Values::default()),
        Some(_) if extension.scope == Scope::Global => Err(format!(
            "projects/{}.toml: {at}: {} is app-wide: a project's file holds nothing of it",
            project.name, extension.title
        )),
        Some(table) => values(extension, table, &at)
            .map_err(|why| format!("projects/{}.toml: {why}", project.name)),
    }
}

fn globally_on(extension: &Extension, global: &BTreeMap<&'static str, Values>) -> bool {
    global
        .get(extension.name)
        .and_then(|values| values.enabled)
        .unwrap_or(extension.on_by_default)
}

/// Whether the extension is active in the project, as its files say now; a file that cannot be
/// read leaves it off.
fn active_in(paths: &Paths, extension: &Extension, project: &Project) -> bool {
    let Ok(global) = global(paths) else {
        return false;
    };
    globally_on(extension, &global)
        && (extension.scope == Scope::Global
            || of_project(extension, project).is_ok_and(|own| own.enabled != Some(false)))
}

/// Whether the feature of an extension a vault entry is bound to (`bound`, `connection:<name>`)
/// names the entry `entry` now; why it cannot be told when its files cannot be read.
pub fn bound_names(daemon: &Daemon, bound: &str, entry: &str) -> Result<bool, String> {
    match bound.split_once(':') {
        Some(("connection", name)) => connections::names(daemon, name, entry),
        _ => Ok(false),
    }
}

/// Whether the use of an extension's feature a vault entry would be bound to (`bound`,
/// `connection:<name>`) can exist: a connection's name is checked as connections check theirs.
pub fn bound_possible(bound: &str) -> bool {
    match bound.split_once(':') {
        Some(("connection", name)) => connections::name_ok(name),
        _ => true,
    }
}

/// What the user declared under the extension's table of `extensions.toml`
/// (`Extension::declared`), checked; nothing when the file holds none.
pub fn declared(paths: &Paths, extension: &Extension) -> Result<Vec<Value>, String> {
    Ok(global(paths)?
        .remove(extension.name)
        .map(|values| values.declared)
        .unwrap_or_default())
}

/// Whether the extension is on, as `extensions.toml` says now; a file that cannot be read leaves
/// it off.
pub fn is_on(paths: &Paths, extension: &Extension) -> bool {
    global(paths).is_ok_and(|global| globally_on(extension, &global))
}

/// Rewrites the extension's table of `extensions.toml` with `edit`, checked as a whole before
/// it is written, then gives every running session the tools it has now.
pub fn rewrite(
    daemon: &Daemon,
    extension: &Extension,
    edit: impl FnOnce(&mut toml_edit::Table) -> Result<(), Failure>,
) -> Result<(), Failure> {
    {
        let _writing = WRITING.lock().unwrap();
        let paths = &daemon.paths;
        let unavailable = |error: std::io::Error| {
            Failure::new("unavailable", format!("extensions.toml: {error}"))
        };
        let text = match std::fs::read_to_string(file(paths)) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(unavailable(error)),
        };
        parse(&text).map_err(|why| Failure::new("invalid-config", why))?;
        let mut document: toml_edit::DocumentMut =
            text.parse().map_err(|error: toml_edit::TomlError| {
                Failure::new(
                    "invalid-config",
                    format!("extensions.toml: {}", error.message()),
                )
            })?;
        let table = document
            .entry(extension.name)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                Failure::new(
                    "invalid-config",
                    format!("extensions.toml: {}: must be a table", extension.name),
                )
            })?;
        edit(table)?;
        let text = document.to_string();
        parse(&text).map_err(Failure::refused)?;
        std::fs::create_dir_all(&paths.config).map_err(unavailable)?;
        crate::providers::write_in_place(&file(paths), &text).map_err(unavailable)?;
    }
    // Before the answer: the next call of a session is served the tools it has now.
    projects::tools_changed(daemon);
    Ok(())
}

/// The names of the tools the active extensions give an agent of the project: a root, another
/// agent, or one an extension launched (`launched`, the extension's name and its agent's kind)
/// (`agents::tools_of`).
pub fn tools_for(
    daemon: &Daemon,
    project: &Project,
    root: bool,
    launched: Option<(&str, &str)>,
) -> Vec<&'static str> {
    let active: Vec<&Extension> = ALL
        .iter()
        .copied()
        .filter(|extension| active_in(&daemon.paths, extension, project))
        .collect();
    let mut tools: Vec<&'static str> = active
        .iter()
        .flat_map(|extension| {
            extension.tools.iter().filter(move |tool| {
                tool.given
                    .iter()
                    .any(|(to, _)| gets(*to, extension.name, root, launched))
            })
        })
        .map(|tool| tool.definition.name)
        .collect();
    // An agent an extension launched has only the tools that extension gives it.
    if launched.is_none() {
        for dynamic in active.iter().filter_map(|extension| extension.dynamic) {
            tools.extend(
                (dynamic.given)(daemon, project)
                    .iter()
                    .map(|tool| tool.name),
            );
        }
    }
    tools
}

/// The extension whose run-time tools hold `name`.
fn dynamic_of(name: &str) -> Option<Dynamic> {
    if crate::mcp::serves(name) {
        return None;
    }
    ALL.iter()
        .filter_map(|extension| extension.dynamic)
        .find(|dynamic| (dynamic.definition)(name).is_some())
}

/// What the judge reads of a call of an extension's tool besides its arguments, or nothing.
pub fn described(daemon: &Daemon, name: &str, args: &Value) -> Option<String> {
    (dynamic_of(name)?.described)(daemon, name, args)
}

/// The events of the extensions the user's inbox lists, each with the kind it is listed as and
/// the text it shows.
pub fn needs_you() -> Vec<(&'static str, &'static str, Says)> {
    ALL.iter()
        .flat_map(|extension| {
            extension
                .needs_you
                .iter()
                .map(|(event, text)| (*event, extension.name, *text))
        })
        .collect()
}

/// Whether a tool given `to` reaches the agent: a root that no extension launched, or an agent of
/// that kind the extension launched.
fn gets(to: To, extension: &str, root: bool, launched: Option<(&str, &str)>) -> bool {
    match (to, launched) {
        (To::Roots, None) => root,
        (To::Launched(kind), Some((by, its))) => by == extension && kind == its,
        _ => false,
    }
}

/// The extension that launched the agent, and which of its agents it is: none for another agent.
pub fn launched(daemon: &Daemon, agent: &str) -> Option<(&'static Extension, &'static str)> {
    ALL.iter()
        .find_map(|extension| Some((*extension, (extension.launched)(daemon, agent)?)))
}

/// Whether `child` is an agent an extension launched under `parent`, which no extension launched:
/// the parent's `agent_*` tools do not reach it, and it is told nothing of it as of its children
/// (`docs/extensions.md`, "Open questions", 1).
pub fn hidden(daemon: &Daemon, parent: &str, child: &str) -> bool {
    launched(daemon, child).is_some() && launched(daemon, parent).is_none()
}

/// Launches an agent of `extension` as a child of `parent` (`tree::launch_for_extension`), on the
/// model slot given; `before` records it as the extension's. Refused at the children cap with
/// `unavailable`, otherwise with `refused`.
#[allow(clippy::too_many_arguments)]
pub fn launch(
    daemon: &Arc<Daemon>,
    extension: &'static Extension,
    parent: &Agent,
    kind: &'static str,
    title: &str,
    brief: &str,
    instructions: String,
    slot: crate::providers::Asked,
    before: impl FnOnce(&Agent) -> Result<(), String>,
) -> Result<Agent, Failure> {
    crate::tree::launch_for_extension(
        daemon,
        extension.name,
        parent,
        kind,
        title,
        brief,
        instructions,
        slot,
        before,
    )
}

/// The agent has reported what it was launched for: its session ends with its turn, as a child's
/// does after `report`.
pub fn reported(daemon: &Daemon, agent: &str) -> Result<(), Failure> {
    daemon.write(|store| {
        let (_, event) = store.change_agent(
            agent,
            flottille_core::store::AgentChange {
                reported_at: Some(Some(flottille_core::store::now_ms())),
                ..Default::default()
            },
            None,
        )?;
        Ok(((), event))
    })?;
    Ok(())
}

/// Runs `/<word>`, typed alone in a session's composer, when an extension active in the session's
/// project declares it: never sent to the agent. `None` for any other message.
pub async fn typed(daemon: &Arc<Daemon>, agent: &Agent, message: &str) -> Option<Answered> {
    let word = message.trim().strip_prefix('/')?;
    let project = projects::load_project(daemon, &agent.project).ok()?;
    ALL.iter()
        .filter(|extension| active_in(&daemon.paths, extension, &project))
        .flat_map(|extension| extension.commands.iter())
        .find(|(name, _)| *name == word)
        .map(|(_, run)| run(daemon.clone(), agent.clone()))
}

fn tool(name: &str) -> Option<&'static Tool> {
    if crate::mcp::serves(name) {
        return None;
    }
    ALL.iter()
        .flat_map(|extension| extension.tools)
        .find(|tool| tool.definition.name == name)
}

/// The definition of a tool an extension gives, active or not: a grant holds only those its
/// agent was given.
pub fn definition(name: &str) -> Option<&'static Definition> {
    tool(name)
        .map(|tool| &tool.definition)
        .or_else(|| (dynamic_of(name)?.definition)(name))
}

/// Whether the tool is one an extension gives.
pub fn gives(name: &str) -> bool {
    tool(name).is_some() || dynamic_of(name).is_some()
}

/// The gate's verdict on a call of a tool an extension gives, `None` for another tool.
pub fn classify(daemon: &Daemon, grant: &Grant, name: &str, args: &Value) -> Option<Verdict> {
    if let Some(dynamic) = dynamic_of(name) {
        return Some((dynamic.classify)(daemon, grant, name, args));
    }
    let tool = tool(name)?;
    let owner = ALL
        .iter()
        .find(|extension| extension.tools.iter().any(|its| std::ptr::eq(its, tool)))?;
    let root = daemon
        .read(|store| store.agent(&grant.agent))
        .ok()
        .flatten()
        .is_some_and(|agent| agent.parent.is_none());
    let launched = launched(daemon, &grant.agent).map(|(by, kind)| (by.name, kind));
    Some(verdict(tool, owner.name, root, launched, args, || {
        (tool.gate)(daemon, grant, args)
    }))
}

/// The verdict on a call of `tool` by a root or another agent: an action it is not given, or
/// one the extension refuses, is refused in every mode; an action the extension declares
/// harmless passes; any other is held as a call to judge.
fn verdict(
    tool: &Tool,
    extension: &str,
    root: bool,
    launched: Option<(&str, &str)>,
    args: &Value,
    refusal: impl FnOnce() -> Result<(), String>,
) -> Verdict {
    let name = tool.definition.name;
    let actions: Vec<&str> = tool
        .given
        .iter()
        .filter(|(to, _)| gets(*to, extension, root, launched))
        .flat_map(|(_, actions)| actions.iter().copied())
        .collect();
    let action = args["action"].as_str().unwrap_or_default();
    if !actions.contains(&action) {
        return Verdict::Deny(format!(
            "refused: {name} takes the actions {}, not {}",
            actions.join(", "),
            args["action"]
        ));
    }
    if let Err(why) = refusal() {
        return Verdict::Deny(format!("refused: {why}"));
    }
    if tool.harmless.contains(&action) {
        return Verdict::Allow;
    }
    Verdict::Ask(crate::gate::Ask {
        reasons: vec![format!(
            "it calls {name} {action}, an action of an extension's tool not declared harmless"
        )],
        class: crate::gate::Class::Judge,
        kind: approval::Kind::Call,
    })
}

/// Performs a call of a tool an extension gives, once the gate let it through.
pub async fn perform(daemon: &Arc<Daemon>, grant: &Grant, name: &str, args: Value) -> Outcome {
    match (tool(name), dynamic_of(name)) {
        (Some(tool), _) => (tool.call)(daemon, grant, args).await,
        (None, Some(dynamic)) => (dynamic.call)(daemon, grant, name, args).await,
        (None, None) => Outcome::Failed(format!("{name} is not served")),
    }
}

/// Starts a `lead` session for the user, as `agent_launch` starts a root, in the project's main
/// checkout or in `workspace`, a workspace just made: its first message, `message`, said to be
/// Flottille's, waits until the workspace is prepared. `before` runs once the agent is made and
/// before it is written: its refusal leaves no agent behind. The extension then knows the
/// session as its own (`Extension::started`), so a daemon stopped before its first message
/// delivers it at the next start (`tree::brief_at_start`).
pub fn start_session(
    daemon: &Arc<Daemon>,
    project: &Project,
    workspace: Option<&str>,
    title: &str,
    message: &str,
    before: impl FnOnce(&Agent) -> Result<(), Failure>,
) -> Result<Agent, Failure> {
    let role = crate::roles::role(crate::roles::LEAD).expect("the lead role");
    let agent = crate::agents::made(
        daemon,
        project,
        crate::agents::New {
            id: None,
            workspace: workspace.map(str::to_owned),
            preparing: workspace.is_some(),
            role,
            parent: None,
            title: Some(title.to_owned()),
            brief: message.to_owned(),
            provider: None,
            model: None,
            effort: None,
            gate_mode: None,
            launched_by: None,
            led: false,
            launched: None,
        },
    )?;
    before(&agent)?;
    crate::agents::write_new(daemon, &agent)?;
    match workspace {
        Some(_) => {
            tokio::spawn(crate::tree::brief_once_ready(daemon.clone(), agent.clone()));
        }
        None => crate::tree::brief(daemon, &agent),
    }
    Ok(agent)
}

/// Whether an extension started the agent's session (`start_session`): its first message is
/// Flottille's, not the user's.
pub fn started(daemon: &Daemon, agent: &str) -> bool {
    ALL.iter()
        .any(|extension| (extension.started)(daemon, agent))
}

/// Hands events of the core, just stored, to the extensions that hear them (the Core event
/// trigger). Each handler says for itself whether its extension is active where the event is.
pub fn heard(daemon: &Arc<Daemon>, events: &[(&'static str, Value)]) {
    for extension in ALL {
        for (name, data) in events {
            for (heard, handler) in extension.heard {
                if heard == name {
                    handler(daemon, data);
                }
            }
        }
    }
}

/// What the extensions add to what the core says of an agent, each under its name.
pub fn facts(daemon: &Daemon, agent: &str) -> Value {
    Value::Object(
        ALL.iter()
            .filter_map(|extension| {
                Some((extension.name.to_owned(), (extension.fact)(daemon, agent)?))
            })
            .collect(),
    )
}

/// Tells an agent what happened, as a delivery of the extension's kind (`[flottille:<name>]`):
/// read between its turns, it never waits on a tool call.
pub fn tell(daemon: &Arc<Daemon>, extension: &Extension, agent: &str, body: &str) {
    crate::deliveries::deliver(
        daemon,
        agent,
        extension.name,
        None,
        body,
        approval::BETWEEN_TURNS,
    );
}

/// The project, when the extension is active there: on globally and not turned off in it. The
/// refusal names the switch that turns it on.
pub fn active(daemon: &Daemon, extension: &Extension, project: &str) -> Result<Project, Failure> {
    let global = global(&daemon.paths).map_err(|why| Failure::new("invalid-config", why))?;
    let project = projects::load_project(daemon, project)?;
    let own = of_project(extension, &project).map_err(|why| Failure::new("invalid-config", why))?;
    let Extension { name, title, .. } = extension;
    if !globally_on(extension, &global) {
        return Err(Failure::refused(format!(
            "{title} is off: turn it on with `flottille extension on {name}` (`enabled` under \
             [{name}] of extensions.toml)"
        )));
    }
    if own.enabled == Some(false) {
        return Err(Failure::refused(format!(
            "{title} is off in project {}: turn it on there with `flottille extension on {name} \
             --project {}` (`enabled` under [extensions.{name}] of projects/{}.toml)",
            project.name, project.name, project.name
        )));
    }
    Ok(project)
}

/// Whether the extension is on globally; the refusal names the switch.
pub fn on(daemon: &Daemon, extension: &Extension) -> Result<(), Failure> {
    let global = global(&daemon.paths).map_err(|why| Failure::new("invalid-config", why))?;
    if globally_on(extension, &global) {
        return Ok(());
    }
    let Extension { name, title, .. } = extension;
    Err(Failure::refused(format!(
        "{title} is off: turn it on with `flottille extension on {name}` (`enabled` under \
         [{name}] of extensions.toml)"
    )))
}

/// A setting's value for a project: the project's own, else the one of `extensions.toml`, else
/// its default.
pub fn value(
    daemon: &Daemon,
    extension: &Extension,
    project: &Project,
    key: &str,
) -> Result<Value, Failure> {
    let invalid = |why| Failure::new("invalid-config", why);
    let setting = extension
        .settings
        .iter()
        .find(|setting| setting.key == key)
        .expect("a setting the extension declares");
    let global = global(&daemon.paths).map_err(invalid)?;
    let own = of_project(extension, project).map_err(invalid)?;
    Ok(own
        .settings
        .get(key)
        .or_else(|| global.get(extension.name)?.settings.get(key))
        .cloned()
        .unwrap_or_else(|| default_of(setting.kind)))
}

/// Whether the daemon routes `method` here: the frame's `extension.*`, or an extension's own.
pub fn answers(method: &str) -> bool {
    method.starts_with("extension.") || owner(method).is_some()
}

fn owner(method: &str) -> Option<&'static Extension> {
    let (prefix, _) = method.split_once('.')?;
    ALL.iter()
        .copied()
        .find(|extension| extension.methods.prefix == prefix)
}

/// Answers `extension.list`, `extension.set`, `extension.act` and the extensions' own methods.
/// The user's door only: no tool of an agent reaches them.
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "extension.list" => Ok(list(&daemon)),
        "extension.set" => set(&daemon, serde_json::from_value(params)?),
        "extension.act" => act(daemon, params).await,
        method => match owner(method) {
            Some(extension) if guidance::answers(extension, method) => {
                guidance::call(&daemon, extension, method, params)
            }
            Some(extension) => (extension.methods.call)(daemon, method.to_owned(), params).await,
            None => Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            )),
        },
    }
}

/// `extension.act {name, action, …}`: a gesture of the user, answered by the extension's method
/// of that action with the rest of the parameters.
async fn act(daemon: Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let Value::Object(mut params) = params else {
        return Err(Failure::refused("extension.act takes an object"));
    };
    let mut text = |key: &str| match params.remove(key) {
        Some(Value::String(text)) => Ok(text),
        _ => Err(Failure::refused(format!(
            "extension.act: `{key}` is a text"
        ))),
    };
    let (name, action) = (text("name")?, text("action")?);
    let extension = find(&name).map_err(|why| Failure::new("not-found", why))?;
    if !extension.actions.contains(&action.as_str()) {
        return Err(Failure::refused(format!(
            "{name} has no action {action}: its actions are {}",
            extension.actions.join(", ")
        )));
    }
    let method = format!("{}.{action}", extension.methods.prefix);
    (extension.methods.call)(daemon, method, Value::Object(params)).await
}

fn list(daemon: &Daemon) -> Value {
    let global = global(&daemon.paths);
    let all = projects::all(daemon);
    let mut errors: Vec<String> = Vec::new();
    for project in &all {
        for name in project.extensions.keys() {
            if let Err(why) = find(name) {
                errors.push(format!("projects/{}.toml: extensions.{why}", project.name));
            }
        }
    }
    let ready = providers::ready(daemon);
    let extensions: Vec<Value> = ALL
        .iter()
        .map(|extension| {
            let (enabled, own, error) = match &global {
                Ok(global) => (
                    globally_on(extension, global),
                    global.get(extension.name).cloned().unwrap_or_default(),
                    None,
                ),
                Err(why) => (false, Values::default(), Some(why.clone())),
            };
            let settings: Vec<Value> = extension
                .settings
                .iter()
                .map(|setting| {
                    json!({
                        "default": default_says(daemon, &ready, setting.kind),
                        "key": setting.key,
                        "label": setting.label,
                        "hint": setting.hint,
                        "kind": kind_word(setting.kind),
                        "per_project": setting.per_project,
                        "value": own.settings.get(setting.key).cloned()
                            .unwrap_or_else(|| default_of(setting.kind)),
                        "set": own.settings.contains_key(setting.key),
                    })
                })
                .collect();
            let projects: Vec<Value> = all
                .iter()
                .filter(|_| extension.scope == Scope::Project)
                .map(|project| match of_project(extension, project) {
                    Ok(values) => json!({
                        "project": project.name,
                        "enabled": enabled && values.enabled != Some(false),
                        "switch": values.enabled,
                        "settings": values.settings,
                        "error": null,
                    }),
                    Err(why) => json!({
                        "project": project.name,
                        "enabled": false,
                        "switch": null,
                        "settings": {},
                        "error": why,
                    }),
                })
                .collect();
            let Place::Row { icon, line, order } = extension.place;
            json!({
                "name": extension.name,
                "title": extension.title,
                "sentence": extension.sentence,
                "scope": match extension.scope {
                    Scope::Project => "project",
                    Scope::Global => "global",
                },
                "on_by_default": extension.on_by_default,
                "switch": extension.switch,
                "usage": extension.usage,
                "place": {"icon": icon, "line": line, "order": order},
                "enabled": enabled,
                "error": error,
                "settings": settings,
                "guidance": extension.guidance.is_some(),
                "guidance_parts": extension.guidance.as_ref().map_or(&[][..], |g| g.parts),
                "commands": extension.commands.iter().map(|(word, _)| word).collect::<Vec<_>>(),
                "projects": projects,
            })
        })
        .collect();
    json!({"extensions": extensions, "errors": errors})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    name: String,
    project: Option<String>,
    enabled: Option<bool>,
    /// A key set to `null` goes back to its default.
    #[serde(default)]
    settings: Map<String, Value>,
}

/// `extension.set`: the switch and the settings, globally in `extensions.toml` or in the project's
/// file, checked against the declaration before anything is written.
fn set(daemon: &Daemon, asked: Set) -> Result<Value, Failure> {
    let extension = find(&asked.name).map_err(|why| Failure::new("not-found", why))?;
    if asked.project.is_some() && extension.scope == Scope::Global {
        return Err(Failure::refused(format!(
            "{} is app-wide: it has no switch nor setting of a project",
            extension.title
        )));
    }
    let at = match &asked.project {
        Some(_) => format!("extensions.{}", extension.name),
        None => extension.name.to_owned(),
    };
    let given: Map<String, Value> = asked
        .settings
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    values(extension, &given, &at).map_err(Failure::refused)?;
    for key in asked.settings.keys() {
        if !extension.settings.iter().any(|setting| setting.key == key) {
            return Err(Failure::refused(format!("{at}.{key}: unknown key")));
        }
    }
    let mut written = Vec::new();
    for (key, value) in &asked.settings {
        let value = match value {
            Value::Null => None,
            value => {
                Some(toml_of(value).map_err(|why| Failure::refused(format!("{at}.{key}: {why}")))?)
            }
        };
        written.push((key, value));
    }
    let edit = |table: &mut toml_edit::Table| {
        if let Some(enabled) = asked.enabled {
            table.insert("enabled", toml_edit::value(enabled));
        }
        for (key, value) in &written {
            match value {
                Some(value) => {
                    table.insert(key, toml_edit::value(value.clone()));
                }
                None => {
                    table.remove(key);
                }
            }
        }
    };
    match &asked.project {
        Some(project) => {
            projects::rewrite(daemon, project, |_, document| {
                let root = document.as_table_mut();
                let extensions = root
                    .entry("extensions")
                    .or_insert_with(|| {
                        let mut table = toml_edit::Table::new();
                        table.set_implicit(true);
                        toml_edit::Item::Table(table)
                    })
                    .as_table_mut()
                    .ok_or_else(|| {
                        Failure::new(
                            "invalid-config",
                            format!("projects/{project}.toml: extensions: must be a table"),
                        )
                    })?;
                let table = extensions
                    .entry(extension.name)
                    .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                    .as_table_mut()
                    .ok_or_else(|| {
                        Failure::new(
                            "invalid-config",
                            format!("projects/{project}.toml: {at}: must be a table"),
                        )
                    })?;
                edit(table);
                Ok(())
            })?;
        }
        // A project's own switch is its file's, announced when it is rewritten.
        None => rewrite(daemon, extension, |table| {
            edit(table);
            Ok(())
        })?,
    }
    daemon
        .emit(
            "extension.changed",
            json!({"name": extension.name, "project": asked.project}),
        )
        .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    let listed = list(daemon);
    Ok(listed["extensions"]
        .as_array()
        .and_then(|all| all.iter().find(|entry| entry["name"] == extension.name))
        .cloned()
        .unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_extension_key_or_kind_in_extensions_toml_is_refused_naming_it() {
        assert_eq!(
            parse("[spexs]\nenabled = true\n").unwrap_err(),
            "extensions.toml: spexs: unknown extension: the extensions are specs, review, \
             connections"
        );
        assert_eq!(
            parse("[specs]\nenable = true\n").unwrap_err(),
            "extensions.toml: specs.enable: unknown key: the keys are enabled, language"
        );
        assert_eq!(
            parse("[specs]\nenabled = \"yes\"\n").unwrap_err(),
            "extensions.toml: specs.enabled: must be true or false, not \"yes\""
        );
        assert!(
            parse("[specs]\nlanguage = 3\n")
                .unwrap_err()
                .starts_with("extensions.toml: specs.language: must be a language tag"),
        );
        assert_eq!(
            parse("specs = 1\n").unwrap_err(),
            "extensions.toml: specs: must be a table"
        );
        let read = parse("# mine\n[specs]\nenabled = true\nlanguage = \"fr-FR\"\n").unwrap();
        assert_eq!(read["specs"].enabled, Some(true));
        assert_eq!(read["specs"].settings["language"], "fr-FR");
    }

    #[test]
    fn a_language_is_a_tag_and_the_user_s_comes_from_the_locale() {
        for tag in ["en", "fr-FR", "pt-BR", "zh-Hant"] {
            assert!(language_tag(tag), "{tag}");
        }
        for tag in ["", "e", "FR", "fr_FR", "fr-", "english language"] {
            assert!(!language_tag(tag), "{tag}");
        }
    }

    #[test]
    fn a_null_inside_a_setting_s_value_is_refused_not_dropped() {
        assert!(toml_of(&json!(["a", null])).is_err());
        assert!(toml_of(&json!({"a": 1, "b": null})).is_err());
        assert!(toml_of(&json!({"a": [1, "b"]})).is_ok());
    }

    #[test]
    fn each_extension_keeps_its_tables_in_its_own_space() {
        for extension in ALL {
            let name = extension.name;
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "{name}: the store takes letters and digits alone"
            );
            let own = format!("ext_{}_", extension.name);
            for migration in extension.migrations {
                for word in migration.split_whitespace() {
                    if word.starts_with("ext_") || word.contains("ext_") {
                        let table = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                        assert!(table.starts_with(&own), "{} names {table}", extension.name);
                    }
                }
                let creates = migration.matches("CREATE TABLE").count();
                let own_tables = migration.matches(&format!("CREATE TABLE {own}")).count();
                assert_eq!(creates, own_tables, "{migration}");
            }
        }
    }

    // ── The core names no extension ─────────────────────────────────────────────────────────

    /// The words of the core's own that an extension took for its name: a pull request's review is
    /// the forge's (`forge::review`), core since before Review. For them the scan looks for the
    /// extension's module path alone (`extensions::review`), not every identifier.
    /// A connection is the core's word too: the daemon's socket takes its clients' connections.
    const CORE_WORDS: &[&str] = &["review", "connection", "connections"];

    /// The words an extension is named by in code: its name and its methods' prefix.
    fn words_of(extension: &Extension) -> Vec<&'static str> {
        vec![extension.name, extension.methods.prefix]
            .into_iter()
            .filter(|word| !CORE_WORDS.contains(word))
            .collect()
    }

    /// The lines of code of `source` that name the extension module of one of `words`.
    fn pathing(source: &str, words: &[&str]) -> Vec<(usize, String)> {
        source
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                let code = line.split("//").next().unwrap_or_default();
                words
                    .iter()
                    .any(|word| code.contains(&format!("extensions::{word}")))
            })
            .map(|(at, line)| (at + 1, line.trim().to_owned()))
            .collect()
    }

    /// The words of an identifier, lower case: `spec_list` and `SpecList` are `spec` and `list`.
    fn split(identifier: &str) -> Vec<String> {
        let mut words = Vec::new();
        for part in identifier.split('_').filter(|part| !part.is_empty()) {
            let mut word = String::new();
            let mut previous_lower = false;
            for c in part.chars() {
                if c.is_ascii_uppercase() && previous_lower {
                    words.push(std::mem::take(&mut word));
                }
                previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
                word.push(c.to_ascii_lowercase());
            }
            words.push(word);
        }
        words
    }

    /// The lines of code of `source` that name one of `words` as a module path or an identifier:
    /// comments and the texts of strings left out.
    fn naming(source: &str, words: &[&str]) -> Vec<(usize, String)> {
        let mut found = Vec::new();
        for (at, line) in source.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            let mut outside = String::new();
            let mut in_string = false;
            let mut escaped = false;
            for c in code.chars() {
                match (in_string, c) {
                    (true, '\\') if !escaped => escaped = true,
                    (true, '"') if !escaped => in_string = false,
                    (true, _) => escaped = false,
                    (false, '"') => {
                        in_string = true;
                        outside.push(' ');
                    }
                    (false, c) => outside.push(c),
                }
            }
            let named = outside
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .filter(|token| !token.is_empty())
                .any(|token| {
                    split(token)
                        .iter()
                        .any(|word| words.contains(&word.as_str()))
                });
            if named {
                found.push((at + 1, line.trim().to_owned()));
            }
        }
        found
    }

    fn sources(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }

    fn no_schema() -> Value {
        json!({"type": "object"})
    }

    fn never_refused(_: &Daemon, _: &Grant, _: &Value) -> Result<(), String> {
        Ok(())
    }

    fn not_called(_: &Arc<Daemon>, _: &Grant, _: Value) -> Called {
        Box::pin(async { Outcome::Done(String::new()) })
    }

    /// A tool named `name`, whose actions `read` and `send` roots get, `read` declared harmless.
    const fn tool_named(name: &'static str) -> Tool {
        Tool {
            definition: Definition {
                name,
                title: "A test tool",
                description: "A tool of a test.",
                read_only: false,
                idempotent: false,
                destructive: false,
                schema: no_schema,
            },
            given: &[(To::Roots, &["read", "send"])],
            harmless: &["read"],
            gate: never_refused,
            call: not_called,
        }
    }

    #[test]
    fn an_extension_tool_action_not_declared_harmless_is_held_in_ask_mode() {
        let tool = tool_named("notes");
        let send = json!({"action": "send"});
        // Held as a call to judge: Ask holds it for the user, Auto asks the judge, Full lets it
        // through (`mcp::serve_call`), as any other call to judge.
        match verdict(&tool, "t", true, None, &send, || Ok(())) {
            Verdict::Ask(ask) => {
                assert_eq!(ask.class, crate::gate::Class::Judge);
                assert!(ask.reasons[0].contains("send"), "{:?}", ask.reasons);
            }
            other => panic!("not held: {}", other.word()),
        }
        let read = json!({"action": "read"});
        assert!(matches!(
            verdict(&tool, "t", true, None, &read, || Ok(())),
            Verdict::Allow
        ));
        // The extension's refusal is the gate's, before any mode is looked at: Full refuses it
        // too, and a harmless action as well.
        for args in [&send, &read] {
            match verdict(&tool, "t", true, None, args, || Err("it is locked".into())) {
                Verdict::Deny(why) => assert_eq!(why, "refused: it is locked"),
                other => panic!("not refused: {}", other.word()),
            }
        }
        // An action it is not given is refused, harmless or not.
        assert!(matches!(
            verdict(&tool, "t", false, None, &read, || Ok(())),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn an_extension_tool_cannot_take_a_core_tool_s_name() {
        assert_eq!(check_tools(ALL), Ok(()));
        static RUN: [Tool; 1] = [tool_named("run")];
        let shadowing = Extension {
            tools: &RUN,
            ..specs::EXTENSION
        };
        let refused = check_tools(&[&shadowing]).unwrap_err();
        assert!(refused.contains("run"), "{refused}");
        assert!(refused.contains("core"), "{refused}");
        static NOTES: [Tool; 1] = [tool_named("notes")];
        let one = Extension {
            tools: &NOTES,
            ..specs::EXTENSION
        };
        let refused = check_tools(&[&one, &one]).unwrap_err();
        assert!(refused.contains("notes"), "{refused}");
    }

    #[test]
    fn code_that_names_an_extension_is_found() {
        let words = words_of(&specs::EXTENSION);
        let sample = "use crate::extensions::specs;\n\
                      let ready = SpecStatus::Ready;\n\
                      fn spec_ready() {}\n\
                      // the spec, in a comment\n\
                      let method = \"spec.ready\";\n\
                      let inspect = respects();";
        let found: Vec<usize> = naming(sample, &words)
            .into_iter()
            .map(|(at, _)| at)
            .collect();
        assert_eq!(found, [1, 2, 3]);
        let sample = "use crate::extensions::review;\n\
                      let decision = forge::review::head_of(pull);";
        let found: Vec<usize> = pathing(sample, CORE_WORDS)
            .into_iter()
            .map(|(at, _)| at)
            .collect();
        assert_eq!(found, [1]);
    }

    /// Connections is named in `extensions/` alone: the core reaches it through the registry.
    /// `connection` stays the core's own word for its clients and hyper's, so the scan looks for
    /// the module's path, as for Review.
    #[test]
    fn the_core_names_no_connection() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &mut files);
        let found: Vec<String> = files
            .iter()
            .filter(|file| !file.starts_with(root.join("extensions")))
            .flat_map(|file| {
                let text = std::fs::read_to_string(file).unwrap();
                pathing(&text, &["connections"])
                    .into_iter()
                    .map(|(at, line)| format!("{}:{at}: {line}", file.display()))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(found.is_empty(), "{}", found.join("\n"));
        let sample = "use crate::extensions::connections;\nlet connection = 2;";
        assert_eq!(pathing(sample, &["connections"]).len(), 1);
    }

    /// The daemon's core modules name no extension: outside `extensions/`, nothing names one as a
    /// module path or an identifier. The judge, built before the frame, is the one known exception
    /// until it moves into it (`docs/extensions.md`, "The last step"): it is not in `ALL`, so its
    /// words are not looked for yet.
    #[test]
    fn the_core_names_no_extension() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &mut files);
        assert!(files.len() > 40, "the sources were found: {}", files.len());
        let words: Vec<&str> = ALL
            .iter()
            .flat_map(|extension| words_of(extension))
            .collect();
        let found: Vec<String> = files
            .iter()
            .filter(|file| !file.starts_with(root.join("extensions")))
            .flat_map(|file| {
                let text = std::fs::read_to_string(file).unwrap();
                naming(&text, &words)
                    .into_iter()
                    .chain(pathing(&text, CORE_WORDS))
                    .map(|(at, line)| format!("{}:{at}: {line}", file.display()))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            found.is_empty(),
            "the core names no extension: it reaches them through the registry \
             (`extensions/mod.rs`):\n{}",
            found.join("\n")
        );
    }
}
