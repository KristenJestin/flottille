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
    Declaration, EXTENSION, KEYS, Reach, SETTINGS, State, Still, declaration, declarations, row,
    setting_of, settings_only, state_of,
};
use crate::daemon::Daemon;
use crate::mcp::{self, Outcome};
use crate::projects::Failure;

const ACTIONS: [&str; 8] = ["list", "show", "add", "edit", "remove", "set", "on", "off"];
const WRITES: [&str; 6] = ["add", "edit", "remove", "set", "on", "off"];
/// What an agent may declare; the projects a connection leaves out, its OAuth client and its
/// switch are the user's (the switch through `on` and `off`).
const AGENT_KEYS: [&str; 8] = [
    "name", "about", "command", "url", "env", "headers", "secrets", "only",
];

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
        Reach::Url(_) => {
            fields.push((
                "url",
                "Its URL",
                declaration.marked_url().unwrap_or_default(),
            ));
            let secrets = match declaration.secrets.is_empty() {
                true => "none".to_owned(),
                false => declaration
                    .secrets
                    .iter()
                    .map(|secret| {
                        let prefix = match secret.prefix.is_empty() {
                            true => String::new(),
                            false => format!(" after {:?}", secret.prefix),
                        };
                        format!(
                            "{}{prefix} from the vault entry {}",
                            secret.said(),
                            secret.entry
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            };
            fields.push(("secrets", "Its secrets", secrets));
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
            ("env" | "headers" | "secrets" | "only", Value::Null) if editing => Value::Null,
            ("secrets", Value::Array(lines)) => {
                for (n, line) in lines.iter().enumerate() {
                    let entry = line["entry"].as_str().unwrap_or_default();
                    if !nameable(&entries, entry, true) {
                        // The value is not said back: it may be a secret written where a name goes.
                        return Err(format!(
                            "secrets[{n}].entry: names no vault entry: a secret is never written \
                             here, only the name of an entry the vault list gives (or, on a URL's \
                             secret line, of one the user is to make)"
                        ));
                    }
                }
                value.clone()
            }
            ("secrets", _) => {
                return Err("secrets: must be a list of {in, name, prefix, entry}".into());
            }
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
                    if !nameable(&entries, entry, key == "headers") {
                        // The value is not said back: it may be a secret written where a name goes.
                        return Err(format!(
                            "{key}.{variable}: names no vault entry: a secret is never written \
                             here, only the name of an entry the vault list gives (or, on a URL's \
                             secret line, of one the user is to make)"
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

/// Whether an agent may name `entry`: one of the vault, or, on a secret line of a URL's requests
/// (`to_make`), one the user is to make before they accept (`waits`), whose name is an entry's and
/// nothing that looks like a secret. A command's variables name entries that exist.
fn nameable(entries: &[flottille_core::vault::Entry], entry: &str, to_make: bool) -> bool {
    entries.iter().any(|known| known.name == entry)
        || (to_make
            && flottille_core::project::valid_name(entry)
            && !random_like(entry)
            && flottille_core::secrets::mask(entry, &std::env::vars_os().collect()) == entry)
}

/// The most characters a word of a name to make has between `-`, `_` and `.`: a longer one is
/// taken for a secret written where a name goes (a 40-character hexadecimal token), which the
/// user would otherwise be asked to save under it, in clear in the file and the vault's list.
const NAME_WORD: usize = 19;

/// The most hexadecimal digits a name to make holds, its separators taken out, when they are most
/// of it: more is a token split where a name goes (`0123456789-abcdef0123-…`, a UUID).
const NAME_HEX: usize = 15;

/// The shortest word of a name to make that is random when letters and numbers alternate in it
/// `NAME_NUMBERS` times or more (`Zq7Wx3Vp9`): a token's piece, where a word of a name holds one
/// number, at its end (`v2`, `token-2`).
const NAME_MIXED: usize = 8;
const NAME_NUMBERS: usize = 3;

fn random_like(name: &str) -> bool {
    let joined: Vec<char> = name
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .collect();
    let hex = joined.iter().filter(|c| c.is_ascii_hexdigit()).count();
    let mixed = |word: &str| {
        let numbers = word
            .split(|c: char| !c.is_ascii_digit())
            .filter(|number| !number.is_empty())
            .count();
        word.chars().count() >= NAME_MIXED
            && word.chars().any(|c| c.is_alphabetic())
            && numbers >= NAME_NUMBERS
    };
    name.split(['-', '_', '.'])
        .any(|word| word.chars().count() > NAME_WORD || mixed(word))
        || (hex > NAME_HEX && hex * 5 >= joined.len() * 4)
}

/// The vault entries a declaration names that the vault does not hold yet.
fn missing(daemon: &Daemon, declaration: &Declaration) -> Vec<String> {
    let entries = daemon
        .read(|store| store.vault_entries())
        .unwrap_or_default();
    let mut missing: Vec<String> = declaration
        .entries()
        .filter(|entry| !entries.iter().any(|known| &&known.name == entry))
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

/// Why the user's Accept of an agent's request waits: a vault entry it names is not right for it
/// yet (`entry_problems`). The request stays held, and is accepted once its entries are right.
pub fn waits(daemon: &Daemon, args: &Value) -> Option<String> {
    // Settings are given to what the user read: a `set` waits, held, for the review of the
    // connection, which its card offers.
    if args["action"] == "set"
        && let Some(name) = args["name"].as_str()
        && let Some((_, _, state)) = super::now_of(daemon, name)
        && state != State::Ready
    {
        return Some(format!(
            "Accept waits: review the connection {name} first, where its actions are set"
        ));
    }
    if !matches!(args["action"].as_str(), Some("add" | "edit")) {
        return None;
    }
    let proposal = proposal(daemon, args).ok()?;
    let problems = entry_problems(daemon, proposal.before.as_ref(), proposal.after.as_ref()?);
    (!problems.is_empty()).then(|| format!("Accept waits: {}", problems.join("; ")))
}

/// What stands between a declaration and the user's Accept in the vault entries it names, each
/// said with what the user does about it: an entry still to make, one project's alone (a
/// connection is every project's), one bound to another use, or, on a URL's secret line an agent
/// chose, one bound to no address or to another host than the one this connection sends it to
/// (`address_problem`). None of them is the declaration's own: the request stays what the agent
/// asked, and only the user's own act (a save in the vault, Share or Bind on the request) mends
/// them.
fn entry_problems(
    daemon: &Daemon,
    before: Option<&Declaration>,
    after: &Declaration,
) -> Vec<String> {
    let entries = daemon
        .read(|store| store.vault_entries())
        .unwrap_or_default();
    let mut problems = Vec::new();
    let missing = missing(daemon, after);
    if !missing.is_empty() {
        problems.push(format!(
            "the vault entr{} {} {} not saved yet: save {} in the vault (New entry…)",
            if missing.len() == 1 { "y" } else { "ies" },
            missing.join(", "),
            if missing.len() == 1 { "is" } else { "are" },
            if missing.len() == 1 { "it" } else { "them" },
        ));
    }
    let this = flottille_core::vault::connection(&after.name);
    let mut named: Vec<&String> = after.entries().collect();
    named.sort();
    named.dedup();
    for name in named {
        let Some(entry) = entries.iter().find(|entry| &entry.name == name) else {
            continue;
        };
        if let Some(project) = &entry.project {
            problems.push(format!(
                "the vault entry {name} is {project}'s alone; a connection needs it for every \
                 project: the user shares it with every project (Share, on this request, when \
                 the entry is the asking agent's project's; else Every project, in its dialog in \
                 Settings, Vault)"
            ));
        }
        if let Err(why) = entry.usable_for(&this) {
            problems.push(why);
        }
    }
    for secret in &after.secrets {
        if let Some(problem) = address_problem(&entries, before, after, secret) {
            problems.push(problem);
        }
    }
    problems
}

/// Why an agent's URL connection may not send this secret line's entry yet: the entry's `url` is
/// where its secret may go, and it names another host, or none. Else an agent could send any
/// token of the user to a server it chose. What the user bound already to the same host stays,
/// an entry still to make is checked once it exists, and the user's own form binds freely.
fn address_problem(
    entries: &[flottille_core::vault::Entry],
    before: Option<&Declaration>,
    after: &Declaration,
    secret: &super::Secret,
) -> Option<String> {
    // Not the URL itself: its query may hold a secret.
    let host = host_of_reach(after)?;
    let kept = before.filter(|before| host_of_reach(before).as_deref() == Some(host.as_str()));
    if kept.is_some_and(|before| before.secrets.contains(secret)) {
        return None;
    }
    let (said, entry) = (secret.said(), &secret.entry);
    let known = entries.iter().find(|known| &known.name == entry)?;
    let bind = format!("the user binds it to {host} (Bind, on this request)");
    match known.url.as_deref().and_then(host_of) {
        Some(entry_host) if entry_host == host => None,
        Some(entry_host) => Some(format!(
            "{said}: the vault entry {entry} is for {entry_host}, and this connection would send \
             it to {host}: {bind}"
        )),
        None => Some(format!(
            "{said}: the vault entry {entry} has no address (its url: https://…, or http:// on \
             this machine), so nothing says it is for {host}, where this connection would send \
             it: {bind}"
        )),
    }
}

/// What an agent is told with its held `add` or `edit` when the declaration names vault entries:
/// what they must be for the user's Accept to go through, and how to ask for one the vault lacks.
pub fn held_note(args: &Value) -> Option<String> {
    let names = ["env", "headers", "secrets"]
        .iter()
        .any(|key| match &args[*key] {
            Value::Object(map) => !map.is_empty(),
            Value::Array(lines) => !lines.is_empty(),
            _ => false,
        });
    let name = args["name"].as_str()?;
    (matches!(args["action"].as_str(), Some("add" | "edit")) && names).then(|| {
        format!(
            "The vault entries a connection names are every project's, since a connection is \
             given to every project, and a URL's secret goes only to its entry's own address \
             (its url): the user's Accept waits until they are, and the user sets them from \
             the request. One the vault lacks, ask the user for it: vault {{\"action\": \
             \"ask\", \"name\": \"<entry>\", \"why\": \"<what it is for>\", \
             \"connection\": \"{name}\"}}."
        )
    })
}

/// The address an entry made for a connection is given: its URL's origin (scheme, host and port),
/// for a URL a secret may go to; none for a command.
fn origin_of(declaration: &Declaration) -> Option<String> {
    let Reach::Url(url) = &declaration.reach else {
        return None;
    };
    host_of(url)?;
    let origin = url::Url::parse(url).ok()?.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
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
    if !carries_secrets(&url) {
        return None;
    }
    Some(url.host_str()?.to_ascii_lowercase())
}

/// Whether a secret may go to `url`: over `https`, or `http` on this machine (`localhost`, a
/// loopback address); plain `http` to another host would carry it in clear.
pub(super) fn carries_secrets(url: &url::Url) -> bool {
    let local = match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => return false,
    };
    match url.scheme() {
        "https" => true,
        "http" => local,
        _ => false,
    }
}

/// The host a connection sends its headers to; none for a command.
fn host_of_reach(declaration: &Declaration) -> Option<String> {
    match &declaration.reach {
        Reach::Url(url) => host_of(url),
        Reach::Command(_) => None,
    }
}

/// Where each vault entry a declaration names goes, for the card: the entry's host (none for an
/// entry with no address) and the connection's (none for a command, whose environment holds it),
/// which differ for every variable of a command; the one project an entry is alone to; whether a
/// URL's secret line waits for the user to bind its entry to the connection's host (`to_bind`),
/// whether the user may share it from the request (`share`: the asking agent's own project's
/// alone), and the connection's origin, the address an entry made or bound for it is given. An
/// entry another use holds, or another project's, is offered neither: the request cannot take
/// it.
fn secrets(
    daemon: &Daemon,
    declaration: &Declaration,
    before: Option<&Declaration>,
    asker: Option<&str>,
) -> Value {
    let entries = daemon
        .read(|store| store.vault_entries())
        .unwrap_or_default();
    let host = host_of_reach(declaration);
    let origin = origin_of(declaration);
    let this = flottille_core::vault::connection(&declaration.name);
    let mut secrets = Vec::new();
    let lines = declaration
        .env
        .iter()
        .map(|(name, entry)| ("env", name.clone(), entry, None))
        .chain(declaration.secrets.iter().map(|secret| {
            // A header's key is the one `headers` always had.
            let key = match secret.place {
                super::Carried::Header => "headers",
                place => place.word(),
            };
            (key, secret.name.clone(), &secret.entry, Some(secret))
        }));
    for (key, name, entry, secret) in lines {
        let known = entries.iter().find(|known| &known.name == entry);
        let entry_host = known
            .and_then(|known| known.url.as_deref())
            .and_then(host_of);
        let differs = host.is_none() || entry_host != host;
        let takes = known.is_some_and(|known| {
            known.usable_for(&this).is_ok()
                && known
                    .project
                    .as_deref()
                    .is_none_or(|own| Some(own) == asker)
        });
        let to_bind = takes
            && secret.is_some_and(|secret| {
                address_problem(&entries, before, declaration, secret).is_some()
            });
        let share = takes && known.is_some_and(|known| known.project.is_some());
        secrets.push(json!({"key": key, "name": name, "entry": entry,
            "entry_host": entry_host, "entry_url": known.and_then(|known| known.url.clone()),
            "host": host, "differs": differs, "missing": known.is_none(),
            "project": known.and_then(|known| known.project.clone()), "share": share,
            "to_bind": to_bind, "origin": origin}));
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
            if let Some(why) = declaration.url_fault() {
                return Err(why);
            }
            // Its entries are checked before the user's Accept (`waits`): one that is not right
            // yet is the user's to mend, and leaves the request as the agent asked it.
            if let Reach::Url(url) = &declaration.reach
                && host_of(url).is_none()
            {
                return Err("url: names no host".into());
            }
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
            if let Reach::Url(url) = &new.reach
                && host_of(url).is_none()
            {
                return Err("url: names no host".into());
            }
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
            // Held like any request, whatever the connection's state: the card says that the
            // review comes first, and its Accept waits for it (`waits`). Settings are given to
            // what the user read, so the actions are checked against the pin when there is one.
            let pin = row.pin.as_ref();
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
                if let Some(pin) = pin
                    && pin.action(action).is_none()
                {
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
/// refusal. A child reads the connections and changes none; an agent of `project` names no entry
/// another project holds alone.
pub fn says(
    daemon: &Daemon,
    root: bool,
    project: &str,
    args: &Value,
) -> Result<Vec<String>, Outcome> {
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
    // An entry another use holds is no entry for a connection: the agent named the wrong one. One
    // another project holds alone is not this agent's to offer: the user would be asked to give
    // a project's secret to a connection another project's agent chose.
    if let Some(after) = proposal
        .after
        .as_ref()
        .filter(|_| matches!(action, "add" | "edit"))
    {
        let entries = daemon
            .read(|store| store.vault_entries())
            .map_err(|error| refused(error.to_string()))?;
        let this = flottille_core::vault::connection(&after.name);
        for name in after.entries() {
            if let Some(entry) = entries.iter().find(|entry| &entry.name == name) {
                entry.usable_for(&this).map_err(refused)?;
                if entry.project.as_deref().is_some_and(|own| own != project) {
                    return Err(refused(format!(
                        "the vault entry {name} belongs to one project, not this one: name an \
                         entry this project sees, or ask for a new one (vault, action ask)"
                    )));
                }
            }
        }
    }
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
    if let Some(failing) = super::failing(daemon, declaration) {
        line.push_str(&format!(" Failing: {failing}"));
    }
    line
}

/// `list` and `show`, and a write the user accepted, done by the path of their own Save:
/// `approved` is what the user read on its card, masked for the agent's `place`.
pub async fn tool(
    daemon: &Arc<Daemon>,
    args: Value,
    approved: &[String],
    place: &crate::place::Place,
) -> Outcome {
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
        _ => write(daemon, args, approved, place).await,
    }
}

/// A write the user accepted: what it says was compared with what they read before this is
/// called (the request's words, made again), and is compared once more where it is written,
/// under the file's lock or the server's turn, so that no write of the user's lands between.
async fn write(
    daemon: &Arc<Daemon>,
    args: Value,
    approved: &[String],
    place: &crate::place::Place,
) -> Outcome {
    let proposal = match proposal(daemon, &args) {
        Ok(proposal) => proposal,
        Err(why) => return refused(why),
    };
    let still = || {
        let now = self::proposal(daemon, &args).map_err(Failure::refused)?;
        // An entry deleted since the Accept: its host is checked once it is made again, by
        // another Accept, never skipped by this write.
        if let Some(after) = &now.after
            && matches!(now.action.as_str(), "add" | "edit")
        {
            let missing = missing(daemon, after);
            if !missing.is_empty() {
                return Err(Failure::refused(format!(
                    "the vault entr{} {} it names {} no longer there: nothing is written",
                    if missing.len() == 1 { "y" } else { "ies" },
                    missing.join(", "),
                    if missing.len() == 1 { "is" } else { "are" },
                )));
            }
            // An entry changed since the Accept (moved to one project, bound elsewhere): the
            // checks of the Accept hold where it is written.
            let problems = entry_problems(daemon, now.before.as_ref(), after);
            if !problems.is_empty() {
                return Err(Failure::refused(format!(
                    "nothing is written: {}",
                    problems.join("; ")
                )));
            }
        }
        let said: Vec<String> = words(daemon, &now)
            .iter()
            .map(|said| mcp::masked(said, place))
            .collect();
        match !approved.is_empty()
            && crate::actions::compared(&said) == crate::actions::compared(approved)
        {
            true => Ok(()),
            false => Err(Failure::refused(format!(
                "what it does changed since the user allowed it; it would now: {}",
                said.join("; ")
            ))),
        }
    };
    let still: Still<'_> = &still;
    let name = proposal.name.clone();
    let answered = match proposal.action.as_str() {
        "add" => match params_of(daemon, &args, false) {
            Ok(params) => super::add_if(daemon, Value::Object(params), still).await,
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
                super::edit_if(daemon, params, still).await
            }
            None => return refused("nothing to write"),
        },
        "remove" => super::remove_if(daemon, json!({"name": name}), still).await,
        "on" | "off" => {
            super::edit_if(
                daemon,
                json!({"name": name, "enabled": proposal.action == "on"}),
                still,
            )
            .await
        }
        "set" => settings_only(daemon, &name, &json!({"actions": proposal.settings}), still)
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
            "add" | "edit" => given_said(daemon, &name, &shown),
            "remove" => format!("Removed the connection {name}: its tool left every agent."),
            "on" => format!(
                "Turned the connection {name} on. {}",
                given_said(daemon, &name, &shown)
            ),
            "off" => format!("Turned the connection {name} off: its tool left every agent."),
            _ => format!("Set the actions of the connection {name} as asked."),
        }),
    }
}

/// Whether the write gave the connection to agents, and why not.
fn given_said(daemon: &Daemon, name: &str, shown: &Value) -> String {
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
        _ => {
            // The agent is told what is happening, never sent looking for a menu: the user is
            // led into the review at once, and a delivery says when it was done.
            let unread = unread(daemon, name);
            let what = match unread.as_slice() {
                [] => "a change".to_owned(),
                [one] => format!("the action {one}, new or changed"),
                many => format!("the actions {}, new or changed", many.join(", ")),
            };
            return format!(
                "The connection {name} is written, not given to agents yet: its server announces \
                 {what}, which the user must review first. The user is shown the review now and \
                 asked for it; a delivery tells you once they reviewed it or left it off. Until \
                 then do not use {what}, and do not ask again."
            );
        }
    };
    format!("The connection {name} is written, not given to agents yet: {why}.")
}

// ── The review an accepted request leads to (#1031) ──────────────────────────────────────────

/// The actions the connection's server announces that the user has not read: those of its last
/// review that are new or differ from the pin.
fn unread(daemon: &Daemon, name: &str) -> Vec<String> {
    let Some((declaration, row, _)) = super::now_of(daemon, name) else {
        return Vec::new();
    };
    let Some(seen) = row
        .seen
        .as_ref()
        .filter(|_| row.seen_for.as_deref() == Some(&declaration.digest()))
    else {
        return Vec::new();
    };
    let pin = row.pin.as_ref();
    seen.actions
        .iter()
        .filter(|action| pin.and_then(|pin| pin.action(&action.name)) != Some(*action))
        .map(|action| action.name.clone())
        .collect()
}

/// What the answer to the user's Accept of the request says of the connection it wrote:
/// `{name, state, review}`, `review` being whether it now waits for a review the user can make
/// (a server that does not start waits for no one). A write that leaves it waiting is said where
/// the user looks: the inbox, and the session of the agent that asked (`connection.review_waits`,
/// a card). Whatever was waiting for a review this write settled is closed first.
pub fn settled(daemon: &Arc<Daemon>, request: &approval::Request, done: bool) -> Option<Value> {
    let args = &request.arguments;
    if !writes(&request.tool, args) {
        return None;
    }
    let name = args["name"].as_str()?;
    review_closed(daemon, name);
    let (declaration, _, state) = super::now_of(daemon, name)?;
    let review = state == State::Awaiting && super::failing(daemon, &declaration).is_none();
    if review && done && matches!(args["action"].as_str(), Some("add" | "edit" | "on")) {
        review_waits(daemon, request, name);
    }
    Some(json!({"name": name, "state": state.word(), "review": review}))
}

/// The user is to review `name`, after the request of `request.agent`: the card of its session,
/// and the inbox, as a connection cut off is said.
fn review_waits(daemon: &Arc<Daemon>, request: &approval::Request, name: &str) {
    let actions = unread(daemon, name);
    let data = json!({"agent": request.agent, "name": name, "request": request.number,
        "actions": actions});
    let written = daemon.write(|store| {
        let event =
            store.append_agent_event(&request.agent, "connection.review_waits", data, false)?;
        Ok(((), event))
    });
    if let Err(error) = written {
        eprintln!("flottille daemon: connection {name}: {error}");
    }
    let event = json!({"name": name, "tool": format!("{}{name}", super::PREFIX),
        "actions": actions});
    if let Err(error) = super::write_row(
        daemon,
        name,
        |_| {},
        Some(("connection.awaiting_review", event)),
    ) {
        eprintln!("flottille daemon: connection {name}: {error}");
    }
}

/// The reviews `name` waits for that an agent's request led to, as `(request, agent, actions)`:
/// the events that said so and no event that closed them.
fn open_reviews(daemon: &Daemon, name: &str) -> Vec<(u64, String, Vec<String>)> {
    let events = daemon
        .read(|store| store.events_named(&["connection.review_waits", "connection.review_done"], 0))
        .unwrap_or_default();
    let mut open = Vec::new();
    for event in events.iter().filter(|event| event.data["name"] == name) {
        let request = event.data["request"].as_u64().unwrap_or_default();
        match event.name.as_str() {
            "connection.review_waits" => open.push((
                request,
                event.data["agent"].as_str().unwrap_or_default().to_owned(),
                event.data["actions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|action| action.as_str().map(str::to_owned))
                    .collect(),
            )),
            _ => open.retain(|(number, ..)| *number != request),
        }
    }
    open
}

/// After a write of the user's or an accepted request's: a connection that was waiting for a
/// review an agent's request led to is reviewed, removed or turned off, so the card in the agent's
/// session folds away and the agent is told, once. One that still waits keeps its card.
pub(super) fn review_closed(daemon: &Arc<Daemon>, name: &str) {
    let open = open_reviews(daemon, name);
    if open.is_empty() {
        return;
    }
    let outcome = match super::now_of(daemon, name) {
        Some((_, _, State::Ready)) => "reviewed",
        Some((_, _, State::Off)) | None => "left",
        Some(_) => return,
    };
    for (request, agent, actions) in open {
        let data = json!({"agent": agent, "name": name, "request": request,
            "outcome": outcome, "actions": actions});
        let written = daemon.write(|store| {
            let event = store.append_agent_event(&agent, "connection.review_done", data, false)?;
            Ok(((), event))
        });
        if let Err(error) = written {
            eprintln!("flottille daemon: connection {name}: {error}");
        }
        let what = match actions.as_slice() {
            [] => "its change".to_owned(),
            actions => format!("its actions {}", actions.join(", ")),
        };
        let body = match outcome {
            "reviewed" => format!(
                "The user reviewed the connection {name}: {what} are given to agents now, \
                 judged as the user set them. You may use them."
            ),
            _ => format!(
                "The connection {name} was left off: the user removed it or turned it off \
                 without reviewing {what}. Nothing of it is given to agents."
            ),
        };
        crate::deliveries::deliver(
            daemon,
            &agent,
            "connections",
            Some(format!("connection-review-{request}")),
            &body,
            approval::BETWEEN_TURNS,
        );
    }
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

/// What the user's Save that answers a request writes (`after`, over `before`) is held to
/// Accept's rules for secrets, checked under the file's lock: it waits for every entry it names,
/// and for each to be fit for it as Accept waits (`entry_problems`): a line new to the connection
/// goes only to its entry's own host. Else Edit then Save would write a token's line for a host
/// its entry, made later, is not bound to.
pub(super) fn saved_checked(
    daemon: &Daemon,
    before: Option<&Declaration>,
    after: &Declaration,
) -> Result<(), Failure> {
    let missing = missing(daemon, after);
    if !missing.is_empty() {
        return Err(Failure::refused(format!(
            "Save waits for the vault entr{} {}: save {} in the vault, then save",
            if missing.len() == 1 { "y" } else { "ies" },
            missing.join(", "),
            if missing.len() == 1 { "it" } else { "them" },
        )));
    }
    let problems = entry_problems(daemon, before, after);
    match problems.is_empty() {
        true => Ok(()),
        false => Err(Failure::refused(format!(
            "Save waits: {}",
            problems.join("; ")
        ))),
    }
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
                    .ok().map(|d| d.served_marked()),
                "after": null, "changed": [], "settings": {}, "secrets": [], "current": false,
                "why": why}),
            );
        }
    };
    let place = place_of(daemon, &request);
    let current = current(daemon, &request, &proposal, place.as_ref());
    let asker = place.as_ref().map(|place| place.project.as_str());
    let changed = match (&proposal.before, &proposal.after) {
        (Some(before), Some(after)) => changed(before, after),
        _ => Vec::new(),
    };
    Ok(json!({
        "number": number,
        "action": proposal.action,
        "name": proposal.name,
        "before": proposal.before.as_ref().map(Declaration::served_marked),
        "after": proposal.after.as_ref().map(Declaration::served_marked),
        "changed": changed,
        "settings": proposal.settings,
        "secrets": proposal.after.as_ref().or(proposal.before.as_ref())
            .map_or(json!([]), |shown| secrets(daemon, shown, proposal.before.as_ref(), asker)),
        "current": current,
        "why": null,
        "waits": waits(daemon, args),
        // The connection waits for the user's review: its card says it first, with Review.
        "review": super::now_of(daemon, &proposal.name).is_some_and(|(declaration, _, state)| {
            state == State::Awaiting && super::failing(daemon, &declaration).is_none()
        }),
    }))
}

/// Where the agent that made `request` runs, while it is there.
fn place_of(daemon: &Daemon, request: &approval::Request) -> Option<crate::place::Place> {
    daemon
        .read(|store| store.agent(&request.agent))
        .ok()
        .flatten()
        .and_then(|agent| mcp::grant_of(daemon, &agent).ok())
        .map(|grant| grant.place)
}

/// Whether `proposal` still says what `request` said when the agent asked: one that does not
/// would not be done.
fn current(
    daemon: &Daemon,
    request: &approval::Request,
    proposal: &Proposal,
    place: Option<&crate::place::Place>,
) -> bool {
    place.is_some_and(|place| {
        let now: Vec<String> = words(daemon, proposal)
            .iter()
            .map(|said| mcp::masked(said, place))
            .collect();
        crate::actions::compared(&now) == crate::actions::compared(&request.says)
    })
}

/// `connection.share_entry {number, entry}` and `connection.bind_entry {number, entry, host}`: the user's **Share with
/// every project** and **Bind it to <host>** on an agent's request (its dialog, behind the
/// window's confirmation): the vault entry the request names is made every project's, or bound
/// to the address the request's own connection sends it to. Only an entry the request names and
/// can take (not another use's, not another project's than the asking agent's), only while it
/// waits for the user and still says what the agent asked, and only to the origin the daemon
/// reads in its declaration, whose host is the one the confirmation showed (`host`): no word of
/// the window's or of an agent's chooses where a secret goes, and agents reach no method of the
/// user's door.
pub(super) fn widened(daemon: &Daemon, method: &str, params: &Value) -> Result<Value, Failure> {
    let number = params["number"]
        .as_u64()
        .ok_or_else(|| Failure::refused("the request's `number` is required"))?;
    let entry = params["entry"]
        .as_str()
        .ok_or_else(|| Failure::refused("the vault entry's `entry` is required"))?;
    let request = daemon
        .read(|store| store.request(number))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no request #{number}")))?;
    if request.tool != "connections"
        || !matches!(request.arguments["action"].as_str(), Some("add" | "edit"))
        || request.held_by != approval::USER
    {
        return Err(Failure::refused(format!(
            "request #{number} is no agent's request to add or edit a connection"
        )));
    }
    if request.state != RequestState::Pending {
        return Err(Failure::refused(format!(
            "request #{number} is already decided: it was {}",
            request.state.as_str()
        )));
    }
    let proposal = proposal(daemon, &request.arguments).map_err(Failure::refused)?;
    let after = proposal
        .after
        .as_ref()
        .filter(|after| after.entries().any(|named| named == entry))
        .ok_or_else(|| {
            Failure::refused(format!(
                "request #{number} names no vault entry {entry}: nothing was changed"
            ))
        })?;
    let place = place_of(daemon, &request);
    if !current(daemon, &request, &proposal, place.as_ref()) {
        return Err(Failure::refused(format!(
            "request #{number} changed since the agent asked: it would not be done, and nothing \
             was changed; refuse it, and let the agent ask again"
        )));
    }
    let known = daemon
        .read(|store| store.vault_entries())?
        .into_iter()
        .find(|known| known.name == entry)
        .ok_or_else(|| Failure::new("not-found", format!("the vault has no entry {entry}")))?;
    known
        .usable_for(&flottille_core::vault::connection(&after.name))
        .map_err(|why| Failure::refused(format!("{why}: nothing was changed")))?;
    let asker = place.as_ref().map(|place| place.project.as_str());
    if let Some(own) = known.project.as_deref().filter(|own| Some(*own) != asker) {
        return Err(Failure::refused(format!(
            "the vault entry {entry} is {own}'s alone, and the agent asking is not of {own}: \
             nothing was changed"
        )));
    }
    match method {
        "connection.share_entry" => crate::vault::shared(daemon, entry),
        _ => {
            // Only a URL's secret line sends an entry to a host.
            let (origin, host) = origin_of(after)
                .zip(host_of_reach(after))
                .filter(|_| after.secrets.iter().any(|secret| secret.entry == entry))
                .ok_or_else(|| {
                    Failure::refused(format!(
                        "the connection {} sends the vault entry {entry} to no address: nothing \
                         was changed",
                        after.name
                    ))
                })?;
            // The host the user confirmed, not one read since: a request moved meanwhile binds
            // nothing.
            if params["host"].as_str() != Some(host.as_str()) {
                return Err(Failure::refused(format!(
                    "the connection {} sends the vault entry {entry} to {host}, not to the host \
                     confirmed: nothing was changed",
                    after.name
                )));
            }
            crate::vault::bound_to(daemon, entry, &origin)
        }
    }
}

/// The connection `name` the vault entry `entry` may be asked for (`vault {action: "ask",
/// connection}`) by the agent `agent`: the one declared, or the one its own request waiting for
/// the user would add,
/// and only when its declaration gives that entry to its server (a variable, a header, a query
/// parameter, a cookie, basic): such an entry is every project's. Its origin, the address the
/// entry is given (none for a command); `None` when there is no such connection, and why when it
/// names no such entry: a login's entry (`for: "connection:<name>"`) keeps the project asked.
pub fn asked_for(
    daemon: &Daemon,
    agent: &str,
    name: &str,
    entry: &str,
) -> Result<Option<Option<String>>, String> {
    let declared = declarations(&daemon.paths)?
        .into_iter()
        .find(|declaration| declaration.name == name);
    let named: Vec<Declaration> = declared
        .into_iter()
        .chain(
            waiting(daemon, agent)
                .into_iter()
                .filter(|after| after.name == name),
        )
        .collect();
    if named.is_empty() {
        return Ok(None);
    }
    named
        .iter()
        .find(|declaration| declaration.entries().any(|named| named == entry))
        .map(|declaration| Some(origin_of(declaration)))
        .ok_or_else(|| {
            format!(
                "the connection {name} gives its server no vault entry {entry}: connection is for \
                 an entry its declaration names (declare it there first); a login's entry is \
                 asked with for: \"connection:{name}\""
            )
        })
}

/// The connection a request of the agent `agent` waiting for the user names the vault entry
/// `entry` in, and its origin: what that agent's ask for that entry is for.
pub fn naming(daemon: &Daemon, agent: &str, entry: &str) -> Option<(String, Option<String>)> {
    waiting(daemon, agent)
        .into_iter()
        .find(|after| after.entries().any(|named| named == entry))
        .map(|after| {
            let origin = origin_of(&after);
            (after.name, origin)
        })
}

/// What the `add` and `edit` requests of the agent `agent` waiting for the user would leave:
/// another agent's, maybe of another project, never chooses what this one's ask is for.
fn waiting(daemon: &Daemon, agent: &str) -> Vec<Declaration> {
    daemon
        .read(|store| store.pending_requests(approval::USER))
        .unwrap_or_default()
        .into_iter()
        .filter(|request| {
            request.agent == agent
                && request.tool == "connections"
                && matches!(request.arguments["action"].as_str(), Some("add" | "edit"))
        })
        .filter_map(|request| proposal(daemon, &request.arguments).ok()?.after)
        .collect()
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

    /// A daemon on temporary folders whose `extensions.toml` declares the connection almanac.
    fn a_daemon(dir: &std::path::Path) -> Arc<Daemon> {
        let paths = flottille_core::paths::Paths {
            runtime: dir.join("runtime"),
            state: dir.join("state"),
            config: dir.join("config"),
            data: dir.join("data"),
        };
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("extensions.toml"),
            "[[connections.server]]\nname = \"almanac\"\nabout = \"Notes.\"\n\
             command = [\"notes\"]\n",
        )
        .unwrap();
        let store = flottille_core::store::Store::open_in_memory().unwrap();
        let projects = crate::projects::Projects::open(&paths).unwrap();
        Arc::new(Daemon::new(&paths, store, projects, 0, None))
    }

    #[tokio::test]
    async fn an_accepted_write_whose_words_changed_where_it_writes_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let place = crate::place::Place {
            project: "app".into(),
            workspace: None,
            root: dir.path().into(),
            repositories: Vec::new(),
            branch: String::new(),
            variables: Default::default(),
            home: None,
        };
        let file = || std::fs::read_to_string(dir.path().join("config/extensions.toml")).unwrap();
        let before = file();
        let args = json!({"action": "edit", "name": "almanac", "about": "Decisions."});
        // The user read another declaration than the one there by the time it is written.
        let read = vec!["Edit the connection almanac: about".to_owned()];

        let outcome = tool(&daemon, args.clone(), &read, &place).await;
        assert!(
            matches!(&outcome, Outcome::Refused(why) | Outcome::Failed(why)
                if why.contains("changed since")),
            "refused"
        );
        // Nor without the words the user read.
        let outcome = tool(&daemon, args, &[], &place).await;
        assert!(
            matches!(outcome, Outcome::Refused(_) | Outcome::Failed(_)),
            "refused"
        );
        assert_eq!(file(), before);
    }

    #[tokio::test]
    async fn an_entry_deleted_after_accept_is_refused_at_write() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let place = crate::place::Place {
            project: "app".into(),
            workspace: None,
            root: dir.path().into(),
            repositories: Vec::new(),
            branch: String::new(),
            variables: Default::default(),
            home: None,
        };
        let entry = flottille_core::vault::Entry {
            name: "notes-token".into(),
            description: None,
            url: Some("https://notes.example.com".into()),
            username: None,
            project: None,
            updated_at: 0,
            bound: None,
        };
        daemon.write(|store| store.put_vault_entry(&entry)).unwrap();
        let file = || std::fs::read_to_string(dir.path().join("config/extensions.toml")).unwrap();
        let before = file();
        let args = json!({"action": "add", "name": "notes", "about": "Notes.",
            "url": "https://notes.example.com/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "notes-token"}]});
        // What the user read and accepted, the entry there.
        let read: Vec<String> = words(&daemon, &proposal(&daemon, &args).unwrap())
            .iter()
            .map(|said| mcp::masked(said, &place))
            .collect();
        // Deleted before the write: made again on another host, it would skip the host check.
        daemon
            .write(|store| store.remove_vault_entry("notes-token"))
            .unwrap();

        let outcome = tool(&daemon, args, &read, &place).await;
        assert!(
            matches!(&outcome, Outcome::Refused(why) | Outcome::Failed(why)
                if why.contains("notes-token")),
            "refused"
        );
        assert_eq!(file(), before);
    }

    /// The user's removal of an entry and the write of an accepted request, which checks the
    /// entries it names are there (`still`), are not interleaved: the removal waits for the file.
    #[test]
    fn a_vault_entry_is_removed_under_the_extensions_file_lock() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(
            dir.path().join("config/keyring.toml"),
            "backend = \"memory\"\n",
        )
        .unwrap();
        let daemon = a_daemon(dir.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(crate::vault::call(
                daemon.clone(),
                "vault.set",
                json!({"name": "notes-token", "secret": "a-secret-of-the-test",
                    "url": "https://notes.example.com"}),
            ))
            .map_err(|failure| failure.message)
            .unwrap();
        // Whether the keyring still holds its secret: the removal takes it first, then waits
        // for the file's lock to take the row.
        let kept = |daemon: &Daemon| {
            runtime
                .block_on(crate::vault::every_project_s(
                    daemon,
                    "notes",
                    "notes-token",
                ))
                .map_err(|why| assert!(why.contains("no longer holds"), "{why}"))
                .is_ok()
        };
        assert!(kept(&daemon));
        let there = |daemon: &Daemon| {
            daemon
                .read(|store| store.vault_entries())
                .unwrap()
                .iter()
                .any(|entry| entry.name == "notes-token")
        };
        let (taken, held) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let writer = std::thread::spawn(move || {
            let _writing = crate::extensions::WRITING.lock().unwrap();
            taken.send(()).unwrap();
            released.recv().unwrap();
        });
        held.recv().unwrap();
        let remover = daemon.clone();
        let removing = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(crate::vault::call(
                    remover,
                    "vault.remove",
                    json!({"name": "notes-token"}),
                ))
                .map_err(|failure| failure.message)
        });
        // The removal has reached the lock once the secret is gone: the row is still there then.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while kept(&daemon) {
            assert!(
                std::time::Instant::now() < deadline,
                "the removal never took the secret"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let during = there(&daemon);
        release.send(()).unwrap();
        writer.join().unwrap();
        removing.join().unwrap().unwrap();
        assert!(during, "removed while the file was being written");
        assert!(!there(&daemon));
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
