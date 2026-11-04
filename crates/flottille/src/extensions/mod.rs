//! The extensions (`docs/extensions.md`): parts of Flottille that declare what they bring, which
//! the core runs and the user turns on or off. This module is the registry, the only place of the
//! daemon that names them, and the frame that keeps their switches, their settings and their
//! tables.

mod command;
mod specs;

use std::collections::BTreeMap;
use std::sync::Mutex;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::project::Project;
use flottille_core::store::Store;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::daemon::Daemon;
use crate::projects::{self, Failure};

pub use command::run;

/// Every extension, in the order of their settings' tabs.
pub const ALL: &[&Extension] = &[&specs::EXTENSION];

/// The command lines of the extensions, beside the frame's own `extension`.
#[derive(Subcommand)]
pub enum Command {
    /// Turn a part of Flottille on or off, for all projects or one: specs
    Extension {
        #[command(subcommand)]
        action: command::ExtensionAction,
    },
    /// The specs of your projects: what an agent needs to build a task without guessing
    Spec {
        #[command(subcommand)]
        action: specs::command::SpecAction,
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
    /// Where its settings appear in the window.
    pub place: Place,
    pub settings: &'static [Setting],
    /// Its tables, `ext_<name>_…`, applied in order by the store; never edited once released.
    pub migrations: &'static [&'static str],
    /// The protocol methods it answers, `<prefix>.*`.
    pub methods: Methods,
}

/// What an extension touches: Global comes with the compaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// It is on in every project once on globally, unless a project turns it off.
    Project,
}

/// Where an extension's settings appear: a section of a screen that exists comes with the first
/// extension that needs it.
pub enum Place {
    /// A tab of its own in Settings, at its place among the tabs.
    Tab { title: &'static str, order: u32 },
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A language tag (`en`, `fr-FR`); by default the user's, the locale of the daemon.
    Language,
}

pub struct Methods {
    pub prefix: &'static str,
    pub call: fn(&Daemon, &str, Value) -> Result<Value, Failure>,
}

/// One extension's table, as `extensions.toml` or a project's file holds it, checked.
#[derive(Clone, Debug, Default, PartialEq)]
struct Values {
    enabled: Option<bool>,
    settings: BTreeMap<String, Value>,
}

/// Held while `extensions.toml` is read and written again.
static WRITING: Mutex<()> = Mutex::new(());

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
        let setting = extension
            .settings
            .iter()
            .find(|setting| setting.key == key)
            .ok_or_else(|| {
                let keys: Vec<&str> = std::iter::once("enabled")
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
        toml_edit::Item::ArrayOfTables(_) | toml_edit::Item::None => {
            Err(format!("{at}: an array of tables is not taken"))
        }
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

/// Answers `extension.list`, `extension.set` and the extensions' own methods. The user's door
/// only: no tool of an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "extension.list" => Ok(list(daemon)),
        "extension.set" => set(daemon, serde_json::from_value(params)?),
        method => match owner(method) {
            Some(extension) => (extension.methods.call)(daemon, method, params),
            None => Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            )),
        },
    }
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
                        "key": setting.key,
                        "label": setting.label,
                        "hint": setting.hint,
                        "kind": match setting.kind { Kind::Language => "language" },
                        "per_project": setting.per_project,
                        "value": own.settings.get(setting.key).cloned()
                            .unwrap_or_else(|| default_of(setting.kind)),
                        "set": own.settings.contains_key(setting.key),
                    })
                })
                .collect();
            let projects: Vec<Value> = all
                .iter()
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
            let Place::Tab { title, order } = extension.place;
            json!({
                "name": extension.name,
                "title": extension.title,
                "sentence": extension.sentence,
                "scope": match extension.scope { Scope::Project => "project" },
                "on_by_default": extension.on_by_default,
                "place": {"tab": title, "order": order},
                "enabled": enabled,
                "error": error,
                "settings": settings,
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
        None => {
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
            edit(table);
            let text = document.to_string();
            parse(&text).map_err(|why| Failure::new("invalid-config", why))?;
            std::fs::create_dir_all(&paths.config).map_err(unavailable)?;
            crate::providers::write_in_place(&file(paths), &text).map_err(unavailable)?;
        }
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
            "extensions.toml: spexs: unknown extension: the extensions are specs"
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

    /// The words an extension is named by in code: its name and its methods' prefix.
    fn words_of(extension: &Extension) -> Vec<&'static str> {
        vec![extension.name, extension.methods.prefix]
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
