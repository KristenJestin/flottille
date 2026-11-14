//! The `connections` family of the agents (`docs/tools.md`, "Connections"): `list` and `show` read
//! the declarations, never a secret (a variable is its vault entry's name); `add`, `edit`,
//! `remove`, `set`, `on` and `off` are held for the user in every mode, Full included, since a
//! connection's command runs with the user's rights and its tool reaches every agent of every
//! project. What the user accepts is written by the path of their own Save, and only if it is
//! still what the card showed: the request's words are made again and compared before it is done.

use std::sync::Arc;

use flottille_core::approval::{self, Decision, RequestState};
use serde_json::{Map, Value, json};

use super::{
    Declaration, EXTENSION, KEYS, Reach, SETTINGS, State, client, declaration, declarations,
    entries_checked, row, setting_of, settings_only, state_of,
};
use crate::daemon::Daemon;
use crate::mcp::{self, Outcome};
use crate::projects::Failure;

const ACTIONS: [&str; 8] = ["list", "show", "add", "edit", "remove", "set", "on", "off"];
const WRITES: [&str; 6] = ["add", "edit", "remove", "set", "on", "off"];
/// What an agent may declare; the projects a connection leaves out, its OAuth client and its
/// switch are the user's (the switch through `on` and `off`).
const AGENT_KEYS: [&str; 7] = ["name", "about", "command", "url", "env", "headers", "only"];

/// Whether a call writes a connection: held for the user every time, in every mode.
pub fn writes(tool: &str, args: &Value) -> bool {
    tool == "connections" && args["action"].as_str().is_some_and(|a| WRITES.contains(&a))
}

/// Why the gate holds it, for the card.
pub fn reason(args: &Value) -> String {
    match args["action"].as_str() {
        Some("remove" | "off") => {
            "it takes a connection away from every agent of every project: only the user decides it"
        }
        Some("set") => {
            "it changes how the gate treats a connection's actions for every agent: only the user \
             decides it"
        }
        _ => {
            "a connection's command runs with the user's rights and its tool is given to every \
             agent of every project: only the user decides it"
        }
    }
    .into()
}

fn refused(why: impl Into<String>) -> Outcome {
    Outcome::Refused(format!("refused: {}", why.into()))
}

fn actions() -> String {
    format!("{} and {}", ACTIONS[..7].join(", "), ACTIONS[7])
}

/// The words of a declaration's command, as one line that says them unambiguously: a word that is
/// empty or holds a space, a quote, a backslash, a control or a bidirectional character is
/// written between double quotes, its quotes and backslashes escaped and its control and bidi
/// characters as `\u{..}`, so that no word can pass for two nor hide what it holds.
fn line(words: &[String]) -> String {
    let hidden = |c: char| {
        c.is_control()
            || matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}')
    };
    let plain = |word: &String| {
        !word.is_empty()
            && !word
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\') || hidden(c))
    };
    words
        .iter()
        .map(|word| match plain(word) {
            true => word.clone(),
            false => {
                let mut quoted = String::from('"');
                for c in word.chars() {
                    match c {
                        '"' | '\\' => {
                            quoted.push('\\');
                            quoted.push(c);
                        }
                        c if hidden(c) => quoted.push_str(&format!("\\u{{{:x}}}", c as u32)),
                        c => quoted.push(c),
                    }
                }
                quoted.push('"');
                quoted
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A declaration, field by field, as the card reads it: `(key, label, value)`.
fn fields(declaration: &Declaration) -> Vec<(&'static str, &'static str, String)> {
    let entries = |entries: &std::collections::BTreeMap<String, String>| match entries.is_empty() {
        true => "none".to_owned(),
        false => entries
            .iter()
            .map(|(name, entry)| format!("{name} from the vault entry {entry}"))
            .collect::<Vec<_>>()
            .join("; "),
    };
    let mut fields = vec![("about", "Its sentence", declaration.about.clone())];
    match &declaration.reach {
        Reach::Command(words) => {
            fields.push((
                "command",
                "Its command, run with the user's rights and without a shell",
                line(words),
            ));
            fields.push(("env", "Its variables", entries(&declaration.env)));
        }
        Reach::Url(url) => {
            fields.push(("url", "Its URL", url.clone()));
            fields.push(("headers", "Its headers", entries(&declaration.headers)));
        }
    }
    let only = match declaration.only.is_empty() {
        true => "none".to_owned(),
        false => declaration
            .only
            .iter()
            .map(|(argument, values)| format!("{argument} may only be {}", values.join(", ")))
            .collect::<Vec<_>>()
            .join("; "),
    };
    fields.push(("only", "Its limits", only));
    if !declaration.except.is_empty() {
        fields.push((
            "except",
            "The projects whose agents do not get it",
            declaration.except.join(", "),
        ));
    }
    fields
}

/// The keys whose values differ between two declarations, in the order of the declaration.
fn changed(before: &Declaration, after: &Declaration) -> Vec<&'static str> {
    let (before, after) = (before.json(), after.json());
    KEYS.iter()
        .copied()
        .filter(|key| !matches!(*key, "name" | "enabled") && before.get(key) != after.get(key))
        .collect()
}

/// What a call proposes: the declaration before (none for an add) and after (none for a
/// removal), and the settings a `set` gives. The refusal of a call that does not fit, before the
/// user is asked anything.
struct Proposal {
    action: String,
    name: String,
    before: Option<Declaration>,
    after: Option<Declaration>,
    settings: Map<String, Value>,
}

/// The method params of the user's door an agent's declaration is, checked as the form checks
/// its fields: a command is one line split without a shell, shell syntax refused; a variable or a
/// header names a vault entry every project sees.
fn params_of(daemon: &Daemon, args: &Value, editing: bool) -> Result<Map<String, Value>, String> {
    let given = args.as_object().ok_or("connections takes an object")?;
    let entries = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?;
    let mut params = Map::new();
    for (key, value) in given {
        if matches!(key.as_str(), "action" | "settings") {
            continue;
        }
        if !AGENT_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "{key}: not a key an agent sets; the keys are {}: the user sets the rest in the \
                 window",
                AGENT_KEYS.join(", ")
            ));
        }
        let value = match (key.as_str(), value) {
            ("env" | "headers" | "only", Value::Null) if editing => Value::Null,
            ("command", Value::String(text)) => json!(flottille_core::command::split_line(text)?),
            ("command", _) => {
                return Err("command: one line, the program and its arguments, as a task's".into());
            }
            ("env" | "headers", Value::Object(map)) => {
                let mut named = Map::new();
                for (variable, entry) in map {
                    let entry = entry.as_str().ok_or_else(|| {
                        format!("{key}.{variable}: must name a vault entry, a text")
                    })?;
                    if !entries.iter().any(|known| known.name == entry) {
                        // The value is not said back: it may be a secret written where a name goes.
                        return Err(format!(
                            "{key}.{variable}: names no vault entry: a secret is never written \
                             here, only the name of an entry the vault list gives"
                        ));
                    }
                    named.insert(variable.clone(), json!({"vault": entry}));
                }
                Value::Object(named)
            }
            ("env" | "headers", _) => {
                return Err(format!(
                    "{key}: must be an object, each name to its vault entry's name"
                ));
            }
            (_, value) => value.clone(),
        };
        params.insert(key.clone(), value);
    }
    Ok(params)
}

/// The host an address names, as a connection's URL or a vault entry's `url` gives it: over
/// `https://`, or `http://` on this machine; a bare host is taken as `https://`. Anything else
/// names no host a secret may be sent to.
fn host_of(text: &str) -> Option<String> {
    let text = text.trim();
    let url = match text.contains("://") {
        true => url::Url::parse(text),
        false => url::Url::parse(&format!("https://{text}")),
    }
    .ok()?;
    let local = match url.host()? {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    };
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        _ => return None,
    }
    Some(url.host_str()?.to_ascii_lowercase())
}

/// The host a connection sends its headers to; none for a command.
fn host_of_reach(declaration: &Declaration) -> Option<String> {
    match &declaration.reach {
        Reach::Url(url) => host_of(url),
        Reach::Command(_) => None,
    }
}

/// An agent's URL connection sends a vault entry in a header only to the entry's own host (its
/// `url`): else an agent could send any token of the user to a server it chose. What the user
/// bound already to the same host stays; the user's own form binds freely.
fn headers_checked(
    daemon: &Daemon,
    before: Option<&Declaration>,
    after: &Declaration,
) -> Result<(), String> {
    let Reach::Url(url) = &after.reach else {
        return Ok(());
    };
    let host = host_of(url).ok_or_else(|| format!("{url:?}: names no host"))?;
    let entries = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?;
    let kept = before.filter(|before| host_of_reach(before).as_deref() == Some(host.as_str()));
    let theirs = "an agent binds an entry only to a connection on the entry's own host (its url); \
                  the user can bind it in their own form, in Settings, Connections";
    for (header, entry) in &after.headers {
        if kept.is_some_and(|before| before.headers.get(header) == Some(entry)) {
            continue;
        }
        let entry_host = entries
            .iter()
            .find(|known| &known.name == entry)
            .and_then(|known| known.url.as_deref())
            .and_then(host_of);
        match entry_host {
            Some(entry_host) if entry_host == host => {}
            Some(entry_host) => {
                return Err(format!(
                    "headers.{header}: the vault entry {entry} is for {entry_host}, and this \
                     connection would send it to {host}: {theirs}"
                ));
            }
            None => {
                return Err(format!(
                    "headers.{header}: the vault entry {entry} has no address (its url, \
                     https://), so nothing says it is for {host}, where this connection would \
                     send it: {theirs}"
                ));
            }
        }
    }
    Ok(())
}

/// Where each vault entry a declaration names goes, for the card: the entry's host (none for an
/// entry with no address) and the connection's (none for a command, whose environment holds it),
/// which differ for every variable of a command.
fn secrets(daemon: &Daemon, declaration: &Declaration) -> Value {
    let entries = daemon
        .read(|store| store.vault_entries())
        .unwrap_or_default();
    let host = host_of_reach(declaration);
    let mut secrets = Vec::new();
    for (key, named) in [("env", &declaration.env), ("headers", &declaration.headers)] {
        for (name, entry) in named {
            let entry_host = entries
                .iter()
                .find(|known| &known.name == entry)
                .and_then(|known| known.url.as_deref())
                .and_then(host_of);
            let differs = host.is_none() || entry_host != host;
            secrets.push(json!({"key": key, "name": name, "entry": entry,
                "entry_host": entry_host, "host": host, "differs": differs}));
        }
    }
    Value::Array(secrets)
}

fn proposal(daemon: &Daemon, args: &Value) -> Result<Proposal, String> {
    let action = args["action"].as_str().unwrap_or_default().to_owned();
    let name = args["name"]
        .as_str()
        .ok_or_else(|| {
            format!("{action} needs name: {{\"action\": \"{action}\", \"name\": \"almanac\"}}")
        })?
        .to_owned();
    let all = declarations(&daemon.paths)?;
    let current = all.iter().find(|d| d.name == name).cloned();
    let unknown = || {
        let names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();
        match names.is_empty() {
            true => format!("there is no connection {name}: none is declared"),
            false => format!(
                "there is no connection {name}: the connections are {}",
                names.join(", ")
            ),
        }
    };
    let mut settings = Map::new();
    let (before, after) = match action.as_str() {
        "show" => (Some(current.ok_or_else(unknown)?), None),
        "add" => {
            if current.is_some() {
                return Err(format!(
                    "a connection {name} is declared already: edit it, or name this one otherwise"
                ));
            }
            let params = params_of(daemon, args, false)?;
            let declaration = Declaration::of(&params, "connection")?;
            entries_checked(daemon, &declaration).map_err(|f| f.message)?;
            headers_checked(daemon, None, &declaration)?;
            (None, Some(declaration))
        }
        "edit" => {
            let old = current.ok_or_else(unknown)?;
            let params = params_of(daemon, args, true)?;
            if params.len() < 2 {
                return Err(format!(
                    "edit needs what it changes: {}",
                    AGENT_KEYS[1..].join(", ")
                ));
            }
            let new = super::edited(&old, &Value::Object(params)).map_err(|f| f.message)?;
            entries_checked(daemon, &new).map_err(|f| f.message)?;
            headers_checked(daemon, Some(&old), &new)?;
            if changed(&old, &new).is_empty() {
                return Err(format!(
                    "the connection {name} already says so: nothing to change"
                ));
            }
            (Some(old), Some(new))
        }
        "remove" => (Some(current.ok_or_else(unknown)?), None),
        "on" | "off" => {
            let old = current.ok_or_else(unknown)?;
            let on = action == "on";
            if old.enabled == on {
                return Err(format!("the connection {name} is {action} already"));
            }
            let mut new = old.clone();
            new.enabled = on;
            (Some(old), Some(new))
        }
        "set" => {
            let old = current.ok_or_else(unknown)?;
            let row = row(daemon, &name);
            // Settings are given to what the user read: never to a server waiting for its review.
            let Some(pin) = row
                .pin
                .as_ref()
                .filter(|_| state_of(true, &old, &row) == State::Ready)
            else {
                return Err(format!(
                    "the connection {name} waits for the user's review, where its actions are set"
                ));
            };
            settings = args["settings"]
                .as_object()
                .filter(|settings| !settings.is_empty())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "set needs settings: {{\"action\": \"set\", \"name\": \"{name}\", \
                         \"settings\": {{\"search\": \"passes\"}}}}, each one of {}",
                        SETTINGS.join(", ")
                    )
                })?;
            for (action, setting) in &settings {
                if !setting.as_str().is_some_and(|s| SETTINGS.contains(&s)) {
                    return Err(format!(
                        "settings.{action}: must be one of {}, not {setting}",
                        SETTINGS.join(", ")
                    ));
                }
                if pin.action(action).is_none() {
                    let names: Vec<&str> = pin.actions.iter().map(|a| a.name.as_str()).collect();
                    return Err(format!(
                        "settings.{action}: {name} has no such action: its actions are {}",
                        names.join(", ")
                    ));
                }
            }
            (Some(old.clone()), Some(old))
        }
        _ => return Err(format!("{action} is not an action of connections")),
    };
    Ok(Proposal {
        action,
        name,
        before,
        after,
        settings,
    })
}

/// What a write proposes, in words, for the card and for the comparison made before it is done:
/// every field of what it leaves, with the old value where it changes one.
fn words(daemon: &Daemon, proposal: &Proposal) -> Vec<String> {
    let name = &proposal.name;
    let tool = format!("{}{name}", super::PREFIX);
    let mut says = Vec::new();
    match (&proposal.before, &proposal.after) {
        (None, Some(after)) => {
            says.push(format!(
                "Add the connection {name}: its tool {tool} is given to every agent of every \
                 project, every action judged"
            ));
            says.extend(
                fields(after)
                    .into_iter()
                    .map(|(_, label, value)| format!("{label}: {value}")),
            );
        }
        (Some(before), None) => {
            says.push(format!(
                "Remove the connection {name}: its server is stopped and its tool {tool} leaves \
                 every agent"
            ));
            says.extend(
                fields(before)
                    .into_iter()
                    .map(|(_, label, value)| format!("{label}: {value}")),
            );
        }
        (Some(before), Some(after)) if proposal.action == "edit" => {
            says.push(format!(
                "Edit the connection {name}: {}",
                changed(before, after).join(", ")
            ));
            let old = fields(before);
            for (key, label, value) in fields(after) {
                match old.iter().find(|(k, _, _)| *k == key) {
                    Some((_, _, was)) if *was != value => {
                        says.push(format!("{label}: {was} → {value}"));
                    }
                    Some(_) => says.push(format!("{label}: {value}")),
                    None => says.push(format!("{label}: {value} (new)")),
                }
            }
            for (key, label, was) in &old {
                if !fields(after).iter().any(|(k, _, _)| k == key) {
                    says.push(format!("{label}: {was} → removed"));
                }
            }
        }
        (Some(before), Some(_)) if proposal.action == "set" => {
            let row = row(daemon, name);
            for (action, setting) in &proposal.settings {
                says.push(format!(
                    "Set the action {action} of the connection {name} to {}, from {}",
                    setting.as_str().unwrap_or_default(),
                    setting_of(&row, action)
                ));
            }
            says.extend(
                fields(before)
                    .into_iter()
                    .map(|(_, label, value)| format!("{label}: {value}")),
            );
        }
        (Some(before), Some(_)) => {
            says.push(match proposal.action.as_str() {
                "on" => format!(
                    "Turn the connection {name} on: its tool {tool} is given to every agent of \
                     every project"
                ),
                _ => format!("Turn the connection {name} off: its tool {tool} leaves every agent"),
            });
            says.extend(
                fields(before)
                    .into_iter()
                    .map(|(_, label, value)| format!("{label}: {value}")),
            );
        }
        (None, None) => {}
    }
    says
}

/// A call checked before the gate is asked: what a write changes, in words, or the tool's
/// refusal. A child reads the connections and changes none.
pub fn says(daemon: &Daemon, root: bool, args: &Value) -> Result<Vec<String>, Outcome> {
    let action = args["action"].as_str().unwrap_or_default();
    if !ACTIONS.contains(&action) {
        return Err(refused(format!(
            "connections has no action {action:?}: its actions are {}",
            actions()
        )));
    }
    if writes("connections", args) && !root {
        return Err(refused(
            "a child reads the connections; the agent the user talks to proposes a change of \
             them: ask your parent (ask)",
        ));
    }
    if !WRITES.contains(&action) {
        return Ok(Vec::new());
    }
    let proposal = proposal(daemon, args).map_err(refused)?;
    Ok(words(daemon, &proposal))
}

/// The connection's lines, as `list` gives them: never a secret, its state and its actions.
fn listed(daemon: &Daemon, declaration: &Declaration, on: bool) -> String {
    let row = row(daemon, &declaration.name);
    let state = state_of(on, declaration, &row).word();
    let actions = row.pin.as_ref().map_or(0, |pin| pin.actions.len());
    let mut line = format!(
        "- {} ({}{}): {state}, {actions} actions. {}",
        declaration.name,
        super::PREFIX,
        declaration.name,
        declaration.about
    );
    if let Some(failing) = client::failing(daemon, &declaration.name) {
        line.push_str(&format!(" Failing: {failing}"));
    }
    line
}

/// `list` and `show`, and a write the user accepted, done by the path of their own Save.
pub async fn tool(daemon: &Arc<Daemon>, args: Value) -> Outcome {
    let on = super::super::is_on(&daemon.paths, &EXTENSION);
    match args["action"].as_str().unwrap_or_default() {
        "list" => match declarations(&daemon.paths) {
            Err(why) => Outcome::Failed(why),
            Ok(all) if all.is_empty() => Outcome::Done(format!(
                "No connection is declared. Connections are {}.",
                if on { "on" } else { "off" }
            )),
            Ok(all) => {
                let mut text = format!(
                    "Connections are {}.\n",
                    if on {
                        "on"
                    } else {
                        "off: no connection is given to agents"
                    }
                );
                for declaration in &all {
                    text.push_str(&listed(daemon, declaration, on));
                    text.push('\n');
                }
                Outcome::Done(text)
            }
        },
        "show" => match proposal(daemon, &args) {
            Err(why) => refused(why),
            Ok(Proposal {
                before: Some(declaration),
                ..
            }) => {
                let row = row(daemon, &declaration.name);
                let mut text = listed(daemon, &declaration, on);
                for (_, label, value) in fields(&declaration) {
                    text.push_str(&format!("\n{label}: {value}"));
                }
                if let Some(pin) = &row.pin {
                    let server = &pin.server;
                    text.push_str(&format!(
                        "\nIts server, as the user accepted it: {} {}",
                        server["name"].as_str().unwrap_or_default(),
                        server["version"].as_str().unwrap_or_default()
                    ));
                    for action in &pin.actions {
                        text.push_str(&format!(
                            "\n- {}: {}",
                            action.name,
                            setting_of(&row, &action.name)
                        ));
                    }
                }
                Outcome::Done(text)
            }
            Ok(_) => refused("nothing to show"),
        },
        _ => write(daemon, args).await,
    }
}

/// A write the user accepted: what it says was compared with what they read before this is
/// called (the request's words, made again).
async fn write(daemon: &Arc<Daemon>, args: Value) -> Outcome {
    let proposal = match proposal(daemon, &args) {
        Ok(proposal) => proposal,
        Err(why) => return refused(why),
    };
    let name = proposal.name.clone();
    let answered = match proposal.action.as_str() {
        "add" => match params_of(daemon, &args, false) {
            Ok(params) => super::add(daemon, Value::Object(params)).await,
            Err(why) => return refused(why),
        },
        // The whole declaration the card showed, not the agent's keys merged on what is there
        // by then: a key it does not hold is removed.
        "edit" => match &proposal.after {
            Some(after) => {
                let mut params = after.json();
                // A command given drops the URL and the other way round: neither is nulled.
                for key in KEYS
                    .iter()
                    .filter(|key| !matches!(**key, "command" | "url"))
                {
                    if params.get(*key).is_none() {
                        params[*key] = Value::Null;
                    }
                }
                super::edit(daemon, params).await
            }
            None => return refused("nothing to write"),
        },
        "remove" => super::remove(daemon, json!({"name": name})).await,
        "on" | "off" => {
            super::edit(
                daemon,
                json!({"name": name, "enabled": proposal.action == "on"}),
            )
            .await
        }
        "set" => settings_only(daemon, &name, &json!({"actions": proposal.settings}))
            .await
            .map(|()| {
                crate::projects::tools_changed(daemon);
                super::changed_event(daemon, &name, None);
                Value::Null
            }),
        _ => return refused("not a write"),
    };
    match answered {
        Err(failure) => Outcome::Failed(failure.message),
        Ok(shown) => Outcome::Done(match proposal.action.as_str() {
            "add" | "edit" => given_said(&name, &shown),
            "remove" => format!("Removed the connection {name}: its tool left every agent."),
            "on" => format!(
                "Turned the connection {name} on. {}",
                given_said(&name, &shown)
            ),
            "off" => format!("Turned the connection {name} off: its tool left every agent."),
            _ => format!("Set the actions of the connection {name} as asked."),
        }),
    }
}

/// Whether the write gave the connection to agents, and why not.
fn given_said(name: &str, shown: &Value) -> String {
    if shown["given"] == true {
        return format!(
            "The connection {name} is written and given to agents: its tool {}{name}, every \
             action judged.",
            super::PREFIX
        );
    }
    let why = match (shown["state"].as_str(), shown["failing"].as_str()) {
        (_, Some(failing)) => format!("its server did not start: {failing}"),
        (Some("off"), _) => "it is off, or Connections are".to_owned(),
        (Some("ready"), _) => {
            return format!("The connection {name} is written; it was given already.");
        }
        _ => "its server announces a change the user reviews first".to_owned(),
    };
    format!("The connection {name} is written, not given to agents yet: {why}.")
}

// ── The user's door ──────────────────────────────────────────────────────────────────────────

/// `connection.add` and `.edit` take the number of an agent's request the user answers with
/// values of their own ("Edit" on its card): the params without it, and the number.
pub(super) fn taken(mut params: Value) -> Result<(Value, Option<u64>), Failure> {
    let Some(map) = params.as_object_mut() else {
        return Ok((params, None));
    };
    match map.remove("request") {
        None | Some(Value::Null) => Ok((params, None)),
        Some(number) => number
            .as_u64()
            .map(|number| (params, Some(number)))
            .ok_or_else(|| Failure::refused("request: the number of an agent's request")),
    }
}

/// Refuses a request the user's own Save cannot answer: another tool's, another action's, an
/// edit of another connection, or one already decided.
pub(super) fn answerable(
    daemon: &Daemon,
    number: u64,
    action: &str,
    name: &str,
) -> Result<(), Failure> {
    let request = daemon
        .read(|store| store.request(number))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no request #{number}")))?;
    // The user may name the server they add otherwise; an edit is of the connection it names.
    if request.tool != "connections"
        || request.arguments["action"] != action
        || (action == "edit" && request.arguments["name"] != name)
        || request.held_by != approval::USER
    {
        return Err(Failure::refused(format!(
            "request #{number} is not an agent's {action} of the connection {name}"
        )));
    }
    if request.state != RequestState::Pending {
        return Err(Failure::refused(format!(
            "request #{number} is already decided: it was {}",
            request.state.as_str()
        )));
    }
    Ok(())
}

/// The request the user answers by writing their own values, decided before it is written; one
/// decided meanwhile (an Accept of it) is refused, and the Save writes nothing.
pub(super) fn claimed(daemon: &Daemon, number: u64) -> Result<approval::Request, Failure> {
    daemon
        .write(|store| {
            Ok(
                match store.decide(number, Decision::Once, approval::USER)? {
                    Some((request, event)) => (Some(request), Some(event)),
                    None => (None, None),
                },
            )
        })?
        .ok_or_else(|| {
            Failure::refused(format!(
                "request #{number} is already decided: nothing was written"
            ))
        })
}

/// The agent told what the user wrote instead of its request, or that their write failed.
pub(super) fn told(
    daemon: &Arc<Daemon>,
    request: &approval::Request,
    written: Result<&Declaration, &str>,
) {
    let number = request.number;
    let body = match written {
        Ok(written) => {
            let mut body = format!(
                "Request #{number} (connections) was not done as you asked: the user wrote the \
                 connection {} themselves, with changes of their own. It now says:",
                written.name
            );
            for (_, label, value) in fields(written) {
                body.push_str(&format!("\n{label}: {value}"));
            }
            body
        }
        Err(why) => format!(
            "Request #{number} (connections) was not done: the user wrote the connection \
             themselves, and their write failed: {why}"
        ),
    };
    crate::approvals::store_answer(daemon, request, "done", &body);
}

/// `connection.proposed {number}`: what an agent's request would write, for its card: the
/// declaration before and after, the keys it changes, the settings of a `set`, and whether it is
/// still what the request said (`current`): one that is not would not be done.
pub(super) fn proposed(daemon: &Daemon, params: &Value) -> Result<Value, Failure> {
    let number = params["number"]
        .as_u64()
        .ok_or_else(|| Failure::refused("the request's `number` is required"))?;
    let request = daemon
        .read(|store| store.request(number))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no request #{number}")))?;
    if request.tool != "connections" || !writes("connections", &request.arguments) {
        return Err(Failure::refused(format!(
            "request #{number} proposes no connection"
        )));
    }
    let args = &request.arguments;
    let proposal = match proposal(daemon, args) {
        Ok(proposal) => proposal,
        Err(why) => {
            return Ok(
                json!({"number": number, "action": args["action"], "name": args["name"],
                "before": declaration(&daemon.paths, args["name"].as_str().unwrap_or_default())
                    .ok().map(|d| d.json()),
                "after": null, "changed": [], "settings": {}, "secrets": [], "current": false,
                "why": why}),
            );
        }
    };
    let place = daemon
        .read(|store| store.agent(&request.agent))
        .ok()
        .flatten()
        .and_then(|agent| mcp::grant_of(daemon, &agent).ok())
        .map(|grant| grant.place);
    let current = place.is_some_and(|place| {
        let now: Vec<String> = words(daemon, &proposal)
            .iter()
            .map(|said| mcp::masked(said, &place))
            .collect();
        crate::actions::compared(&now) == crate::actions::compared(&request.says)
    });
    let changed = match (&proposal.before, &proposal.after) {
        (Some(before), Some(after)) => changed(before, after),
        _ => Vec::new(),
    };
    Ok(json!({
        "number": number,
        "action": proposal.action,
        "name": proposal.name,
        "before": proposal.before.as_ref().map(Declaration::json),
        "after": proposal.after.as_ref().map(Declaration::json),
        "changed": changed,
        "settings": proposal.settings,
        "secrets": proposal.after.as_ref().or(proposal.before.as_ref())
            .map_or(json!([]), |shown| secrets(daemon, shown)),
        "current": current,
        "why": null,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(words: &[&str]) -> String {
        line(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn a_command_line_is_shown_unambiguously() {
        assert_ne!(said(&["'x", "y'"]), said(&["x y"]));
        assert_eq!(said(&["notes", "--dir", "a b"]), r#"notes --dir "a b""#);
        assert_eq!(said(&["say \"hi\" \\ there"]), r#""say \"hi\" \\ there""#);
        assert_eq!(said(&["it's"]), r#""it's""#);
        assert_eq!(said(&[""]), r#""""#);
        assert_eq!(said(&["a\u{202e}b"]), r#""a\u{202e}b""#);
        assert_eq!(said(&["x\ny"]), r#""x\u{a}y""#);
    }

    #[test]
    fn an_address_names_its_host_over_https_or_on_this_machine() {
        assert_eq!(
            host_of("https://NOTES.example.com/mcp").as_deref(),
            Some("notes.example.com")
        );
        assert_eq!(
            host_of("notes.example.com").as_deref(),
            Some("notes.example.com")
        );
        assert_eq!(
            host_of("http://127.0.0.1:7000").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(host_of("http://notes.example.com"), None);
        assert_eq!(host_of("ftp://notes.example.com"), None);
        assert_eq!(host_of(""), None);
    }
}
