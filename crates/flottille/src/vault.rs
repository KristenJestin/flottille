//! The vault (`docs/vault.md`): the user's secrets, kept by the operating system's keyring, that
//! agents use through Flottille's tools and never see. The store keeps the entries' metadata;
//! a secret goes from a client to the keyring, and from the keyring to where a tool puts it,
//! never back out: no method, tool, event, log nor error says one. Every value is masked in what
//! the daemon stores and shows (`secrets::add_vault_value`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flottille_core::approval::BETWEEN_TURNS;
use flottille_core::declared::ProjectTracker;
use flottille_core::paths::Paths;
use flottille_core::project::{Project, valid_name};
use flottille_core::secrets::{self, VAULT_SHORTEST};
use flottille_core::store::{Store, now_ms};
use flottille_core::vault::Entry;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::deliveries;
use crate::mcp::{Grant, Outcome, arguments};
use crate::projects::Failure;

/// The file of the daemon's configuration folder that chooses the keyring: `backend = "memory"`
/// keeps the secrets in the daemon's memory, for the tests, which must never reach the user's
/// own keyring. A file, not a variable: an agent sets the environment of what it runs, never the
/// daemon's configuration.
const CHOICE: &str = "keyring.toml";

/// The attribute every item of Flottille's has in the keyring, and the one naming its entry.
const APPLICATION: (&str, &str) = ("application", "flottille");
const ENTRY: &str = "flottille-entry";

/// Where the secrets are.
pub enum Keyring {
    /// The Secret Service of the session (`oo7`).
    Os,
    /// The daemon's memory: lost when it stops. Its file is read again at each use: `locked =
    /// true` there plays a locked keyring, its table `kept` (name = secret) the secrets a keyring
    /// held before the daemon started, for the tests.
    Memory {
        kept: Mutex<HashMap<String, String>>,
        file: PathBuf,
    },
}

/// What a locked keyring answers: nothing waits for the user to unlock it.
pub const LOCKED: &str = "the keyring is locked: the user must unlock it in their session, then \
                          try again";

impl Keyring {
    /// The keyring the configuration chooses: the system's unless the tests' file says otherwise.
    pub fn open(paths: &Paths) -> Keyring {
        let file = paths.config.join(CHOICE);
        match setting(&file, "backend").as_deref() {
            Some("memory") => {
                eprintln!(
                    "flottille daemon: the vault's keyring is the daemon's memory ({}): its \
                     secrets are lost when it stops",
                    file.display()
                );
                Keyring::Memory {
                    kept: Mutex::new(HashMap::new()),
                    file,
                }
            }
            _ => Keyring::Os,
        }
    }

    /// The memory's map, or `LOCKED` when its file says so.
    fn memory<'a>(
        kept: &'a Mutex<HashMap<String, String>>,
        file: &Path,
    ) -> Result<std::sync::MutexGuard<'a, HashMap<String, String>>, String> {
        if setting(file, "locked").as_deref() == Some("true") {
            return Err(LOCKED.to_owned());
        }
        Ok(kept.lock().unwrap())
    }

    async fn store(&self, name: &str, secret: &str) -> Result<(), String> {
        match self {
            Keyring::Memory { kept, file } => {
                Self::memory(kept, file)?.insert(name.to_owned(), secret.to_owned());
                Ok(())
            }
            Keyring::Os => {
                let keyring = os().await?;
                keyring
                    .create_item(
                        &format!("Flottille: {name}"),
                        &[APPLICATION, (ENTRY, name)],
                        secret,
                        true,
                    )
                    .await
                    .map_err(said)
            }
        }
    }

    /// The secret of `name`, or `LOCKED` at once: a locked item is never read, which would ask
    /// the user to unlock it and wait.
    async fn read(&self, name: &str) -> Result<Option<String>, String> {
        match self {
            Keyring::Memory { kept, file } => Ok(Self::memory(kept, file)?
                .get(name)
                .cloned()
                .or_else(|| held_before(file, name))),
            Keyring::Os => {
                let keyring = os().await?;
                let items = keyring
                    .search_items(&[APPLICATION, (ENTRY, name)])
                    .await
                    .map_err(said)?;
                let Some(item) = items.first() else {
                    return Ok(None);
                };
                if item.is_locked().await.map_err(said)? {
                    return Err(LOCKED.to_owned());
                }
                let secret = item.secret().await.map_err(said)?;
                String::from_utf8(secret.as_bytes().to_vec())
                    .map(Some)
                    .map_err(|_| "the keyring holds a secret that is not text".to_owned())
            }
        }
    }

    async fn delete(&self, name: &str) -> Result<(), String> {
        match self {
            Keyring::Memory { kept, file } => {
                Self::memory(kept, file)?.remove(name);
                Ok(())
            }
            Keyring::Os => os()
                .await?
                .delete(&[APPLICATION, (ENTRY, name)])
                .await
                .map_err(said),
        }
    }
}

/// A key of the keyring's file, as text.
fn setting(file: &Path, key: &str) -> Option<String> {
    let document = std::fs::read_to_string(file)
        .ok()?
        .parse::<toml_edit::DocumentMut>()
        .ok()?;
    let item = document.get(key)?;
    item.as_str()
        .map(str::to_owned)
        .or_else(|| item.as_bool().map(|on| on.to_string()))
}

/// The secret of `name` in the table `kept` of the memory keyring's file.
fn held_before(file: &Path, name: &str) -> Option<String> {
    let document = std::fs::read_to_string(file)
        .ok()?
        .parse::<toml_edit::DocumentMut>()
        .ok()?;
    document.get("kept")?.get(name)?.as_str().map(str::to_owned)
}

async fn os() -> Result<oo7::Keyring, String> {
    oo7::Keyring::new().await.map_err(said)
}

/// A keyring's error, which names what failed and never a secret.
fn said(error: oo7::Error) -> String {
    format!("the keyring: {error}")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    project: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    name: String,
    secret: String,
    description: Option<String>,
    url: Option<String>,
    username: Option<String>,
    project: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskSet {
    ask: u64,
    secret: String,
    username: Option<String>,
    /// Another name than the one asked, for an entry whose name another project uses.
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskDismiss {
    ask: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    name: String,
}

pub async fn call(
    daemon: std::sync::Arc<Daemon>,
    method: &str,
    params: Value,
) -> Result<Value, Failure> {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "vault.list" => {
            let List { project } = serde_json::from_value(params)?;
            let entries = daemon.read(|store| store.vault_entries())?;
            Ok(json!({"entries": entries
                .iter()
                .filter(|entry| project.as_deref().is_none_or(|project| entry.seen_by(project)))
                .map(Entry::to_json)
                .collect::<Vec<_>>()}))
        }
        "vault.set" => {
            // Whatever goes wrong is said without the secret the call carried.
            let given = params["secret"].as_str().unwrap_or_default().to_owned();
            set(&daemon, params).await.map_err(|failure| Failure {
                message: masked_of(&failure.message, &given),
                ..failure
            })
        }
        "vault.asks" => {
            let open = daemon.read(asks)?.open;
            Ok(json!({"asks": open.iter().map(Ask::to_json).collect::<Vec<_>>()}))
        }
        "vault.ask_set" => {
            let given = params["secret"].as_str().unwrap_or_default().to_owned();
            ask_set(&daemon, params).await.map_err(|failure| Failure {
                message: masked_of(&failure.message, &given),
                ..failure
            })
        }
        "vault.ask_dismiss" => {
            let AskDismiss { ask } = serde_json::from_value(params)?;
            let request = open_ask(&daemon, ask)?;
            answered(&daemon, &request, "dismissed", &request.name);
            Ok(json!({"dismissed": request.to_json()}))
        }
        "vault.remove" => {
            let Remove { name } = serde_json::from_value(params)?;
            let entries = daemon.read(|store| store.vault_entries())?;
            if !entries.iter().any(|entry| entry.name == name) {
                return Err(Failure::new(
                    "not-found",
                    format!("the vault has no entry {name}"),
                ));
            }
            // The secret first: a keyring that refuses (locked) leaves the entry whole, to be
            // removed again once it is unlocked.
            daemon.vault.delete(&name).await.map_err(unavailable)?;
            let Some(removed) = daemon.write(|store| store.remove_vault_entry(&name))? else {
                return Err(Failure::new(
                    "not-found",
                    format!("the vault has no entry {name}"),
                ));
            };
            retool(&daemon);
            Ok(json!({"removed": removed.to_json()}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

async fn set(daemon: &std::sync::Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    if !params["secret"].is_string() {
        return Err(Failure::new("invalid-params", "the secret is a text"));
    }
    let asked: Set = serde_json::from_value(params)?;
    if !valid_name(&asked.name) {
        return Err(Failure::refused(format!(
            "an entry's name is letters, digits, . _ and -, starting with a letter or a digit, \
             64 at most: {} is not",
            asked.name
        )));
    }
    if asked.secret.chars().count() < VAULT_SHORTEST {
        return Err(Failure::refused(format!(
            "a secret is {VAULT_SHORTEST} characters at least: a shorter one could not be masked \
             without masking ordinary words"
        )));
    }
    if let Some(project) = &asked.project {
        crate::projects::load_project(daemon, project)?;
    }
    let given = |text: Option<String>| {
        text.map(|text| text.trim().to_owned())
            .filter(|t| !t.is_empty())
    };
    let entry = Entry {
        name: asked.name,
        description: given(asked.description),
        url: given(asked.url),
        username: given(asked.username),
        project: given(asked.project),
        updated_at: now_ms(),
    };
    // Masked before it is kept anywhere, so that nothing written after can hold it.
    secrets::add_vault_value(&asked.secret);
    daemon
        .vault
        .store(&entry.name, &asked.secret)
        .await
        .map_err(unavailable)?;
    daemon.write(|store| store.put_vault_entry(&entry))?;
    retool(daemon);
    // However it was set, from its request's card or from Settings, what asked for it is told.
    if let Ok(state) = daemon.read(asks) {
        for request in state.open.iter().filter(|r| entry.name == r.name) {
            if entry.seen_by(&request.project) {
                answered(daemon, request, "set", &entry.name);
            }
        }
    }
    Ok(entry.to_json())
}

/// Why the entry `name` cannot be set for `project` from a request: an entry of that name exists
/// and is not the project's. A request never changes an existing entry's project, so the user
/// chooses another name.
fn taken(entries: &[Entry], project: &str, name: &str) -> Option<String> {
    let entry = entries.iter().find(|entry| entry.name == name)?;
    match entry.project.as_deref() {
        Some(own) if own == project => None,
        Some(other) => Some(format!(
            "`{name}` is used by the project {other}: choose another name"
        )),
        None => Some(format!(
            "`{name}` is used by every project: choose another name"
        )),
    }
}

/// `vault.ask_set`: the entry of an open request set with the secret the user typed, and what
/// Flottille prefilled; the same checks as `vault.set`, and never an entry of another project.
async fn ask_set(daemon: &std::sync::Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let AskSet {
        ask,
        secret,
        username,
        name,
    } = serde_json::from_value(params)?;
    let request = open_ask(daemon, ask)?;
    let name = name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| request.name.clone());
    let entries = daemon.read(|store| store.vault_entries())?;
    if let Some(why) = taken(&entries, &request.project, &name) {
        return Err(Failure::refused(why));
    }
    if request.takes_username && username.as_deref().is_none_or(|u| u.trim().is_empty()) {
        return Err(Failure::refused(format!(
            "the entry {} needs a username: {}",
            request.name, request.description
        )));
    }
    let set = set(
        daemon,
        json!({"name": name, "secret": secret, "description": request.description,
            "username": username, "project": request.project}),
    )
    .await?;
    // Under the name asked, `set` answered the request; under another, nothing named it.
    if name != request.name {
        answered(daemon, &request, "set", &name);
    }
    Ok(set)
}

fn unavailable(why: String) -> Failure {
    Failure::new("unavailable", why)
}

/// `text` with `secret` masked, however short, and whatever the vault masks.
fn masked_of(text: &str, secret: &str) -> String {
    let text = if secret.is_empty() {
        text.to_owned()
    } else {
        text.replace(secret, secrets::MASK)
    };
    secrets::mask(&text, &Default::default())
}

/// The agents of every project get the tools their roles have now: `vault` is listed only to an
/// agent whose project sees an entry.
fn retool(daemon: &Daemon) {
    let projects = daemon.projects.lock().unwrap();
    let Ok(loaded) = projects.registry().load_all() else {
        return;
    };
    for project in &loaded.projects {
        crate::mcp::project_changed(daemon, project);
    }
}

/// Whether the agents of `project` have the `vault` tool: the project sees an entry, or a feature
/// of Flottille names one it may ask the user for.
pub fn listed_to(daemon: &Daemon, project: &Project) -> bool {
    daemon
        .read(|store| store.vault_entries())
        .is_ok_and(|entries| entries.iter().any(|entry| entry.seen_by(&project.name)))
        || !needs(daemon, project).is_empty()
}

/// An entry a feature of Flottille names for a project and the vault does not give it: the only
/// kind an agent may ask the user for, described by the daemon, never by the agent.
pub struct Need {
    pub name: String,
    pub description: String,
    /// The entry is of no use without its username (Jira's API takes the account's e-mail).
    pub takes_username: bool,
}

/// The entries the features of `project` name and its agents cannot use: the tracker's
/// `worklog_token` naming no entry the project sees.
pub fn needs(daemon: &Daemon, project: &Project) -> Vec<Need> {
    let entries = daemon
        .read(|store| store.vault_entries())
        .unwrap_or_default();
    let mut needs = Vec::new();
    if let Some(ProjectTracker::Jira {
        site,
        worklog_token: Some(name),
        ..
    }) = &project.tracker
        && !entries
            .iter()
            .any(|entry| &entry.name == name && entry.seen_by(&project.name))
    {
        needs.push(Need {
            name: name.clone(),
            description: format!(
                "The Jira API token tickets log_time logs time on {site} with; its username is \
                 the Jira account's e-mail"
            ),
            takes_username: true,
        });
    }
    needs
}

/// What a refusal for the missing entry `name` adds: the agent may ask the user for it, unless
/// the user dismissed it and nothing changed since.
pub fn offer(daemon: &Daemon, project: &Project, name: &str) -> String {
    let dismissed = daemon
        .read(asks)
        .is_ok_and(|state| state.dismissed(&project.name, name));
    if dismissed || !needs(daemon, project).iter().any(|need| need.name == name) {
        return String::new();
    }
    format!(
        "; or ask the user for it, who types only the secret: vault {{action: \"ask\", name: \
         \"{name}\", why: \"<what you need it for>\"}}, answered at once, the user's answer \
         coming later as a delivery"
    )
}

/// A request for an entry, open until the user sets it or dismisses it: its number is the
/// number of its `vault.asked` event.
struct Ask {
    number: u64,
    at: u64,
    agent: String,
    project: String,
    name: String,
    why: String,
    description: String,
    takes_username: bool,
    /// Why the name cannot be set for the project, when another project uses it.
    taken: Option<String>,
    /// The agents whose ask joined it while it was open.
    joined: Vec<String>,
}

impl Ask {
    fn to_json(&self) -> Value {
        json!({"ask": self.number, "at": self.at, "agent": self.agent, "project": self.project,
            "name": self.name, "why": self.why, "description": self.description,
            "takes_username": self.takes_username, "taken": self.taken, "joined": self.joined})
    }
}

/// The requests as their events say: those still open, and the entries dismissed since nothing
/// changed of them.
struct Asks {
    open: Vec<Ask>,
    /// `(project, entry)`, until the project's settings or the entry change.
    dismissed: Vec<(String, String)>,
}

impl Asks {
    fn dismissed(&self, project: &str, name: &str) -> bool {
        self.dismissed
            .iter()
            .any(|(p, n)| p == project && n == name)
    }
}

fn asks(store: &Store) -> flottille_core::Result<Asks> {
    let events = store.events_named(
        &[
            "vault.asked",
            "vault.ask_joined",
            "vault.ask_answered",
            "vault.changed",
            "project.changed",
        ],
        0,
    )?;
    let mut state = Asks {
        open: Vec::new(),
        dismissed: Vec::new(),
    };
    let text = |data: &Value, key: &str| data[key].as_str().unwrap_or_default().to_owned();
    for event in events {
        let data = &event.data;
        match event.name.as_str() {
            "vault.asked" => state.open.push(Ask {
                number: event.seq,
                at: event.at,
                agent: text(data, "agent"),
                project: text(data, "project"),
                name: text(data, "name"),
                why: text(data, "why"),
                description: text(data, "description"),
                takes_username: data["takes_username"] == true,
                taken: data["taken"].as_str().map(str::to_owned),
                joined: Vec::new(),
            }),
            "vault.ask_joined" => {
                if let Some(ask) = state.open.iter_mut().find(|a| data["ask"] == a.number) {
                    ask.joined.push(text(data, "agent"));
                }
            }
            "vault.ask_answered" => {
                state.open.retain(|ask| data["ask"] != ask.number);
                if data["outcome"] == "dismissed" {
                    state
                        .dismissed
                        .push((text(data, "project"), text(data, "name")));
                }
            }
            "vault.changed" => state.dismissed.retain(|(_, name)| data["name"] != *name),
            _ => state
                .dismissed
                .retain(|(project, _)| data["name"] != *project),
        }
    }
    Ok(state)
}

fn open_ask(daemon: &Daemon, number: u64) -> Result<Ask, Failure> {
    daemon
        .read(asks)?
        .open
        .into_iter()
        .find(|ask| ask.number == number)
        .ok_or_else(|| {
            Failure::new(
                "not-found",
                format!("no request #{number} for a vault entry is open"),
            )
        })
}

/// The request is answered: its card folds away, and each agent that asked is told what the user
/// did, never the secret.
/// `entry` is the entry the user set, under the name asked or another.
fn answered(daemon: &std::sync::Arc<Daemon>, request: &Ask, outcome: &str, entry: &str) {
    let data = json!({"agent": request.agent, "ask": request.number, "project": request.project,
        "name": request.name, "entry": entry, "outcome": outcome});
    let written = daemon.write(|store| {
        let event = store.append_agent_event(&request.agent, "vault.ask_answered", data, false)?;
        Ok(((), event))
    });
    if let Err(error) = written {
        eprintln!(
            "flottille daemon: the request for the vault entry {}: {error}",
            request.name
        );
        return;
    }
    let body = match outcome {
        "set" if entry != request.name => format!(
            "the user set the vault entry `{entry}` instead of `{}`, whose name was taken: the \
             project's setting still names `{}` until it is changed to `{entry}`; tell the user",
            request.name, request.name
        ),
        "set" => format!(
            "the user set the vault entry `{}`: give its name to the tool that needs it",
            request.name
        ),
        _ => format!(
            "the user dismissed the request for the vault entry `{}`: it is not asked again \
             until the setting that names it changes",
            request.name
        ),
    };
    for agent in std::iter::once(&request.agent).chain(&request.joined) {
        deliveries::deliver(
            daemon,
            agent,
            "vault",
            Some(format!("vault-ask-{}-{agent}", request.number)),
            &body,
            BETWEEN_TURNS,
        );
    }
}

/// Masks the value of every entry from the daemon's start: they are read from the keyring once,
/// off the requests' way (the keyring may ask the user to unlock it).
pub async fn load(daemon: &Daemon) {
    let Ok(entries) = daemon.read(|store| store.vault_entries()) else {
        return;
    };
    for entry in entries {
        match daemon.vault.read(&entry.name).await {
            Ok(Some(secret)) => secrets::add_vault_value(&secret),
            Ok(None) => {}
            Err(why) => eprintln!("flottille daemon: vault entry {}: {why}", entry.name),
        }
    }
}

/// How long a tool's use of a secret waits for the keyring: no tool call waits for a human, and a
/// keyring that asks to be unlocked is answered `LOCKED` at once.
const SECRET_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// The secret of the entry `name`, for a tool of Flottille's to put where it goes (an HTTP header,
/// a command's standard input) on behalf of an agent of `project`: never to be returned, logged
/// or shown. An entry of another project is no entry for it.
pub async fn secret(daemon: &Daemon, project: &str, name: &str) -> Result<String, String> {
    let entries = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?;
    given(&entries, &daemon.vault, project, name).await
}

/// The secret of the entry `name`, which must be every project's (no `project` on it), for what
/// the daemon runs for all of them (a connection's server): never to be returned, logged or shown.
/// Masked from now on before it is used: the keyring may have been locked at the daemon's start.
pub async fn every_project_s(daemon: &Daemon, name: &str) -> Result<String, String> {
    let entries = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?;
    match entries.iter().find(|entry| entry.name == name) {
        None => return Err(format!("the vault has no entry {name}")),
        Some(entry) if entry.project.is_some() => {
            return Err(format!(
                "the vault entry {name} is one project's alone, not every project's"
            ));
        }
        Some(_) => {}
    }
    tokio::time::timeout(SECRET_WAIT, daemon.vault.read(name))
        .await
        .map_err(|_| format!("the keyring did not answer in time: {LOCKED}"))??
        .ok_or_else(|| format!("the keyring no longer holds the secret of {name}"))
        .inspect(|secret| secrets::add_vault_value(secret))
}

/// `secret`, with the entries and the keyring at hand.
async fn given(
    entries: &[Entry],
    keyring: &Keyring,
    project: &str,
    name: &str,
) -> Result<String, String> {
    if !entries
        .iter()
        .any(|entry| entry.name == name && entry.seen_by(project))
    {
        return Err(format!("the vault has no entry {name} for this project"));
    }
    tokio::time::timeout(SECRET_WAIT, keyring.read(name))
        .await
        .map_err(|_| format!("the keyring did not answer in time: {LOCKED}"))??
        .ok_or_else(|| format!("the keyring no longer holds the secret of {name}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    action: String,
    name: Option<String>,
    why: Option<String>,
}

/// `vault`: what the agent's project sees of the vault, metadata only. No action gives a secret.
pub fn tool(daemon: &Daemon, grant: &Grant, args: Value) -> Outcome {
    let asked: Asked = match arguments("vault", args) {
        Ok(asked) => asked,
        Err(refused) => return refused,
    };
    if asked.action == "ask" {
        return ask(daemon, grant, asked);
    }
    if asked.action != "list" {
        return Outcome::Refused(
            "refused: vault lists the entries, or asks the user for one a refusal said is \
             missing; a secret is never shown: give an entry's name to the tool that needs it"
                .into(),
        );
    }
    let entries = match daemon.read(|store| store.vault_entries()) {
        Ok(entries) => entries,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    // A line a field: a URL and a username in one word would read as a URL's password, masked.
    let mut text = String::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.seen_by(&grant.place.project))
    {
        text.push_str(&format!("- {}", entry.name));
        if let Some(description) = &entry.description {
            text.push_str(&format!(": {description}"));
        }
        for (label, value) in [("url", &entry.url), ("username", &entry.username)] {
            if let Some(value) = value {
                text.push_str(&format!("\n  {label}: {value}"));
            }
        }
        text.push('\n');
    }
    if text.is_empty() {
        return Outcome::Done("The vault holds no entry for this project.".into());
    }
    Outcome::Done(format!(
        "The vault's entries (give a name to the tool that needs its secret):\n{text}"
    ))
}

/// `vault {action: "ask"}`: the user is asked for an entry a feature of Flottille names and that
/// is missing, answered at once; a second ask while one is open joins it.
fn ask(daemon: &Daemon, grant: &Grant, asked: Asked) -> Outcome {
    let (Some(name), Some(why)) = (
        asked.name.filter(|n| !n.trim().is_empty()),
        asked.why.filter(|w| !w.trim().is_empty()),
    ) else {
        return Outcome::Refused(
            "refused: ask needs name, the entry a refusal said is missing, and why, what you \
             need it for"
                .into(),
        );
    };
    let project = match crate::projects::load_project(daemon, &grant.place.project) {
        Ok(project) => project,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    let Some(need) = needs(daemon, &project)
        .into_iter()
        .find(|need| need.name == name)
    else {
        return Outcome::Refused(format!(
            "refused: nothing of Flottille's needs a vault entry {name} for this project: ask \
             names only an entry a refusal said is missing"
        ));
    };
    let why = crate::mcp::masked(why.trim(), &grant.place);
    let written = daemon.write(|store| {
        let state = asks(store)?;
        if state.dismissed(&project.name, &name) {
            return Ok((Err(()), None));
        }
        let open = state
            .open
            .iter()
            .find(|ask| ask.project == project.name && ask.name == name);
        let (event, joined) = match open {
            Some(open) => (
                store.append_agent_event(
                    &grant.agent,
                    "vault.ask_joined",
                    json!({"agent": grant.agent, "ask": open.number, "project": project.name,
                        "name": name, "why": why}),
                    false,
                )?,
                true,
            ),
            None => (
                store.append_agent_event(
                    &grant.agent,
                    "vault.asked",
                    json!({"agent": grant.agent, "project": project.name, "name": name,
                        "why": why, "description": need.description,
                        "takes_username": need.takes_username,
                        "taken": taken(&store.vault_entries()?, &project.name, &name)}),
                    false,
                )?,
                false,
            ),
        };
        Ok((Ok(joined), Some(event)))
    });
    match written {
        Err(error) => Outcome::Failed(error.to_string()),
        Ok(Err(())) => Outcome::Refused(format!(
            "refused: the user dismissed the request for the vault entry {name}: it is not asked \
             again until they change the setting that names it or set it themselves"
        )),
        Ok(Ok(true)) => Outcome::Done(format!(
            "The user is already asked for the vault entry {name}: yours joins that request. \
             Nothing waits: continue with what does not need it; a delivery says when the user \
             sets it or dismisses it."
        )),
        Ok(Ok(false)) => Outcome::Done(format!(
            "The user is asked for the vault entry {name}. Nothing waits: continue with what \
             does not need it; a delivery says when the user sets it or dismisses it. You never \
             see the secret: once set, give its name to the tool that needs it."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, project: Option<&str>) -> Entry {
        Entry {
            name: name.into(),
            description: None,
            url: None,
            username: None,
            project: project.map(str::to_owned),
            updated_at: 0,
        }
    }

    fn memory(file: &Path) -> Keyring {
        Keyring::Memory {
            kept: Mutex::new(HashMap::from([(
                "deploy".to_owned(),
                "dep10y-key".to_owned(),
            )])),
            file: file.to_owned(),
        }
    }

    #[test]
    fn a_name_is_taken_by_every_entry_that_is_not_the_asking_projects() {
        let entries = [
            entry("mine", Some("app")),
            entry("theirs", Some("other")),
            entry("shared", None),
        ];
        assert_eq!(taken(&entries, "app", "mine"), None);
        assert_eq!(taken(&entries, "app", "free"), None);
        assert_eq!(
            taken(&entries, "app", "theirs").as_deref(),
            Some("`theirs` is used by the project other: choose another name")
        );
        assert_eq!(
            taken(&entries, "app", "shared").as_deref(),
            Some("`shared` is used by every project: choose another name")
        );
    }

    #[tokio::test]
    async fn another_project_s_entry_is_not_given_to_a_tool() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = memory(&dir.path().join("keyring.toml"));
        let entries = [entry("deploy", Some("app"))];

        let refused = given(&entries, &keyring, "other", "deploy")
            .await
            .unwrap_err();
        assert!(refused.contains("no entry deploy"), "{refused}");
        assert!(!refused.contains("dep10y-key"), "{refused}");
        assert_eq!(
            given(&entries, &keyring, "app", "deploy").await.unwrap(),
            "dep10y-key"
        );
        assert!(given(&entries, &keyring, "app", "unknown").await.is_err());
    }

    #[tokio::test]
    async fn a_locked_keyring_is_answered_at_once_never_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("keyring.toml");
        std::fs::write(&file, "backend = \"memory\"\nlocked = true\n").unwrap();
        let keyring = memory(&file);
        let entries = [entry("deploy", None)];

        let asked = std::time::Instant::now();
        let refused = given(&entries, &keyring, "app", "deploy")
            .await
            .unwrap_err();
        assert!(asked.elapsed() < SECRET_WAIT, "answered at once");
        assert_eq!(refused, LOCKED);
    }
}
