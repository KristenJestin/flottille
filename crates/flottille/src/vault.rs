//! The vault (`docs/vault.md`): the user's secrets, kept by the operating system's keyring, that
//! agents use through Flottille's tools and never see. The store keeps the entries' metadata;
//! a secret goes from a client to the keyring, and from the keyring to where a tool puts it,
//! never back out: no method, tool, event, log nor error says one. Every value is masked in what
//! the daemon stores and shows (`secrets::add_vault_value`).

use std::collections::HashMap;
use std::sync::Mutex;

use flottille_core::paths::Paths;
use flottille_core::project::valid_name;
use flottille_core::secrets::{self, VAULT_SHORTEST};
use flottille_core::store::now_ms;
use flottille_core::vault::Entry;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
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
    /// The daemon's memory: lost when it stops.
    Memory(Mutex<HashMap<String, String>>),
}

impl Keyring {
    /// The keyring the configuration chooses: the system's unless the tests' file says otherwise.
    pub fn open(paths: &Paths) -> Keyring {
        let chosen = std::fs::read_to_string(paths.config.join(CHOICE))
            .ok()
            .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
            .and_then(|document| document["backend"].as_str().map(str::to_owned));
        match chosen.as_deref() {
            Some("memory") => Keyring::Memory(Mutex::new(HashMap::new())),
            _ => Keyring::Os,
        }
    }

    async fn store(&self, name: &str, secret: &str) -> Result<(), String> {
        match self {
            Keyring::Memory(kept) => {
                kept.lock()
                    .unwrap()
                    .insert(name.to_owned(), secret.to_owned());
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

    async fn read(&self, name: &str) -> Result<Option<String>, String> {
        match self {
            Keyring::Memory(kept) => Ok(kept.lock().unwrap().get(name).cloned()),
            Keyring::Os => {
                let keyring = os().await?;
                let items = keyring
                    .search_items(&[APPLICATION, (ENTRY, name)])
                    .await
                    .map_err(said)?;
                let Some(item) = items.first() else {
                    return Ok(None);
                };
                let secret = item.secret().await.map_err(said)?;
                String::from_utf8(secret.as_bytes().to_vec())
                    .map(Some)
                    .map_err(|_| "the keyring holds a secret that is not text".to_owned())
            }
        }
    }

    async fn delete(&self, name: &str) -> Result<(), String> {
        match self {
            Keyring::Memory(kept) => {
                kept.lock().unwrap().remove(name);
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
        "vault.remove" => {
            let Remove { name } = serde_json::from_value(params)?;
            let removed = daemon.write(|store| store.remove_vault_entry(&name))?;
            let Some(removed) = removed else {
                return Err(Failure::new(
                    "not-found",
                    format!("the vault has no entry {name}"),
                ));
            };
            daemon.vault.delete(&name).await.map_err(unavailable)?;
            retool(&daemon);
            Ok(json!({"removed": removed.to_json()}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

async fn set(daemon: &Daemon, params: Value) -> Result<Value, Failure> {
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
    Ok(entry.to_json())
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

/// Whether the agents of `project` see an entry: they have the `vault` tool.
pub fn seen_by(daemon: &Daemon, project: &str) -> bool {
    daemon
        .read(|store| store.vault_entries())
        .is_ok_and(|entries| entries.iter().any(|entry| entry.seen_by(project)))
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

/// The secret of the entry `name`, for a tool of Flottille's to put where it goes (an HTTP header,
/// a command's standard input) on behalf of an agent of `project`: never to be returned, logged
/// or shown. An entry of another project is no entry for it.
#[allow(
    dead_code,
    reason = "the tools that use a secret come with their own issues (#383)"
)]
pub async fn secret(daemon: &Daemon, project: &str, name: &str) -> Result<String, String> {
    let entries = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?;
    if !entries
        .iter()
        .any(|entry| entry.name == name && entry.seen_by(project))
    {
        return Err(format!("the vault has no entry {name} for this project"));
    }
    daemon
        .vault
        .read(name)
        .await?
        .ok_or_else(|| format!("the keyring no longer holds the secret of {name}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    action: String,
}

/// `vault`: what the agent's project sees of the vault, metadata only. No action gives a secret.
pub fn tool(daemon: &Daemon, grant: &Grant, args: Value) -> Outcome {
    let asked: Asked = match arguments("vault", args) {
        Ok(asked) => asked,
        Err(refused) => return refused,
    };
    if asked.action != "list" {
        return Outcome::Refused(
            "refused: vault only lists the entries; a secret is never shown: give an entry's \
             name to the tool that needs it"
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
