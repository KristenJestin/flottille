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
use flottille_core::vault::{self as uses, Entry, use_said, use_stops};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::deliveries;
use crate::mcp::{Grant, Outcome, arguments};
use crate::projects::Failure;

/// The file of the daemon's configuration folder that chooses the keyring: `backend = "memory"`
/// keeps the secrets in the daemon's memory, for the tests, which must never reach the user's
/// own keyring; a build for users refuses it (`Keyring::chosen`). A file, not a variable: an agent sets the environment of what it runs, never the
/// daemon's configuration.
const CHOICE: &str = "keyring.toml";

/// The attribute every item of Flottille's has in the keyring, and the one naming its entry.
const APPLICATION: (&str, &str) = ("application", "flottille");
const ENTRY: &str = "flottille-entry";

/// The writes in progress, one lock per entry name: a Save, a removal and a share each check
/// what the vault holds, await the keyring, then write the store, and two of them on one name
/// would interleave (a share that saw no entry overwriting the one the user just saved).
static WRITES: std::sync::LazyLock<Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Waits until no other write of the entry `name` is in progress; it is this one's until the
/// guard is dropped.
async fn writing(name: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut writes = WRITES.lock().unwrap();
        // A lock no write holds nor waits for is forgotten: the map keeps the names in use only.
        writes.retain(|_, lock| std::sync::Arc::strong_count(lock) > 1);
        writes.entry(name.to_owned()).or_default().clone()
    };
    lock.lock_owned().await
}

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
        let (keyring, said) = Self::chosen(paths.config.join(CHOICE), cfg!(debug_assertions));
        if let Some(said) = said {
            eprintln!("flottille daemon: {said}");
        }
        keyring
    }

    /// The keyring `file` chooses, and what the daemon says of it. The memory is a test
    /// build's alone (`cargo test` builds without optimisation, a release with): a user's file
    /// that asks for it would keep their secrets in plain text, in its table `kept`.
    fn chosen(file: PathBuf, test_build: bool) -> (Keyring, Option<String>) {
        match setting(&file, "backend").as_deref() {
            Some("memory") if !test_build => {
                let said = format!(
                    "{} asks for the memory keyring, which only a test build has: the system's \
                     keyring is used, and no secret is read from that file",
                    file.display()
                );
                (Keyring::Os, Some(said))
            }
            Some("memory") => {
                let said = format!(
                    "the vault's keyring is the daemon's memory ({}): its secrets are lost when \
                     it stops",
                    file.display()
                );
                let keyring = Keyring::Memory {
                    kept: Mutex::new(HashMap::new()),
                    file,
                };
                (keyring, Some(said))
            }
            _ => (Keyring::Os, None),
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
                // An await, as the system's keyring's call over D-Bus is: two writes that
                // overlap there overlap here too.
                tokio::task::yield_now().await;
                if refuses_a_store(file) {
                    return Err("the keyring refused to store it".to_owned());
                }
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
            Keyring::Memory { kept, file } => {
                // An await, as the system's keyring's search over D-Bus is.
                tokio::task::yield_now().await;
                Ok(Self::memory(kept, file)?
                    .get(name)
                    .cloned()
                    .or_else(|| held_before(file, name)))
            }
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

/// Whether the configuration chooses the memory keyring, and this build is a test's: then a
/// client that keeps a key of its own has no keyring (the daemon's memory is not its), and keeps
/// it in a file as on a machine without one (`flottille remote setup`).
pub fn memory_chosen(paths: &Paths) -> bool {
    cfg!(debug_assertions)
        && setting(&paths.config.join(CHOICE), "backend").as_deref() == Some("memory")
}

/// For a test of a keyring that fails to store: `fail_stores = 2` in the memory keyring's file
/// refuses the next two stores, counting down in the file.
fn refuses_a_store(file: &Path) -> bool {
    let Some(left) = setting(file, "fail_stores")
        .and_then(|left| left.parse::<i64>().ok())
        .filter(|left| *left > 0)
    else {
        return false;
    };
    if let Ok(text) = std::fs::read_to_string(file)
        && let Ok(mut document) = text.parse::<toml_edit::DocumentMut>()
    {
        document["fail_stores"] = toml_edit::value(left - 1);
        let _ = std::fs::write(file, document.to_string());
    }
    true
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
        .or_else(|| item.as_integer().map(|number| number.to_string()))
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

/// What a machine with no keyring says: the vault keeps no secret anywhere else.
pub const NO_KEYRING: &str = "this machine has no keyring: no Secret Service runs in its \
                              session (a headless server), and a vault entry is kept in a \
                              keyring alone, never in a file";

/// A keyring's error, which names what failed and never a secret.
fn said(error: oo7::Error) -> String {
    if crate::remote_command::no_service(&error) {
        return NO_KEYRING.to_owned();
    }
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
    /// Left out, an entry that exists keeps the secret the keyring holds: it is never read to do
    /// so, only the metadata is rewritten.
    secret: Option<String>,
    description: Option<String>,
    url: Option<String>,
    username: Option<String>,
    project: Option<String>,
    /// The one use it is bound to, when it is made from a feature; left out, an entry keeps the
    /// one it has.
    #[serde(rename = "for")]
    bound: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskDismiss {
    ask: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShareWith {
    name: String,
    /// The user's machine it goes to.
    machine: String,
}

/// What a client that does not sign is answered for `vault.share`.
pub const NOT_SIGNED: &str = "sharing a vault entry is signed by the user's device: ask it with `flottille vault share`, or from the window's Settings, Vault";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    name: String,
    /// The user confirmed what stops working: an entry bound to a feature that names it goes.
    #[serde(default)]
    confirmed: bool,
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
                .map(|entry| {
                    // The window offers no project Save would refuse (`bound_checked`).
                    let mut json = entry.to_json();
                    json["every_project_only"] = json!(entry.bound.as_deref().is_some_and(
                        |bound| crate::extensions::bound_given_to_server(&daemon, bound, &entry.name)
                            .unwrap_or(true)
                    ));
                    json
                })
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
        "vault.share" => {
            let ShareWith { name, machine } = serde_json::from_value(params)?;
            share(&daemon, &name, &machine).await
        }
        "vault.asks" => {
            let open = daemon.read(asks)?.open;
            Ok(json!({"asks": open.iter().map(Ask::to_json).collect::<Vec<_>>()}))
        }
        "vault.ask_dismiss" => {
            let AskDismiss { ask } = serde_json::from_value(params)?;
            let request = open_ask(&daemon, ask)?;
            answered(&daemon, &request, None);
            Ok(json!({"dismissed": request.to_json()}))
        }
        "vault.remove" => {
            let Remove { name, confirmed } = serde_json::from_value(params)?;
            let _writing = writing(&name).await;
            let entries = daemon.read(|store| store.vault_entries())?;
            let Some(entry) = entries.iter().find(|entry| entry.name == name) else {
                return Err(Failure::new(
                    "not-found",
                    format!("the vault has no entry {name}"),
                ));
            };
            if let Some(bound) = &entry.bound
                && !confirmed
            {
                let said = use_said(bound).unwrap_or_else(|| bound.clone());
                let stops = use_stops(bound).unwrap_or_else(|| "that it stops working".into());
                match named_by(&daemon, bound, &name) {
                    Ok(false) => {}
                    Ok(true) => {
                        return Err(Failure::refused(format!(
                            "the vault entry {name} is bound to {said}, which names it: removing \
                             it means {stops}; have it name another entry first, or confirm the \
                             removal"
                        )));
                    }
                    // An unreadable file cannot say the entry is not named: the guard holds.
                    Err(why) => {
                        return Err(Failure::refused(format!(
                            "the vault entry {name} is bound to {said}, and whether it names it \
                             cannot be told ({why}): removing it may mean {stops}; mend the file, \
                             or confirm the removal"
                        )));
                    }
                }
            }
            // The secret first: a keyring that refuses (locked) leaves the entry whole, to be
            // removed again once it is unlocked.
            daemon.vault.delete(&name).await.map_err(unavailable)?;
            // An accepted connection's write checks the entries it names are there, then writes:
            // the entry goes before or after, never between (`requests::write`).
            let removed = crate::extensions::between_writes(|| {
                daemon.write(|store| store.remove_vault_entry(&name))
            })?;
            let Some(removed) = removed else {
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
    if !params["secret"].is_string() && !params["secret"].is_null() {
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
    if let Some(secret) = &asked.secret
        && secret.chars().count() < VAULT_SHORTEST
    {
        return Err(Failure::refused(format!(
            "a secret is {VAULT_SHORTEST} characters at least: a shorter one could not be masked \
             without masking ordinary words"
        )));
    }
    let given = |text: Option<String>| {
        text.map(|text| text.trim().to_owned())
            .filter(|t| !t.is_empty())
    };
    // Trimmed first: an empty project is every project's, which no project has to load.
    let project = given(asked.project);
    let _writing = writing(&asked.name).await;
    let existing = daemon
        .read(|store| store.vault_entries())?
        .into_iter()
        .find(|entry| entry.name == asked.name);
    if asked.secret.is_none() && existing.is_none() {
        return Err(Failure::refused(format!(
            "a new entry needs its secret: the vault has no entry {}",
            asked.name
        )));
    }
    // Which username was masked with the secret: an edit without a secret that changes it must
    // mask the new Basic form too.
    let username_was = existing.as_ref().and_then(|entry| entry.username.clone());
    // A copy of another machine's entry stays one while its secret is: a new secret makes it this
    // machine's own.
    let shared_from = existing
        .as_ref()
        .filter(|_| asked.secret.is_none())
        .and_then(|entry| entry.shared_from.clone());
    let had = existing.and_then(|entry| entry.bound);
    let bound = match (given(asked.bound), had) {
        (Some(new), Some(own)) if new != own => {
            return Err(Failure::refused(format!(
                "the vault entry {} is bound to {}: remove it first, or choose another name",
                asked.name,
                use_said(&own).unwrap_or(own.clone())
            )));
        }
        (Some(new), _) => Some(new),
        (None, had) => had,
    };
    let url = given(asked.url);
    let project = checked(
        daemon,
        &asked.name,
        project,
        bound.as_deref(),
        url.as_deref(),
    )?;
    let username = given(asked.username);
    let open = daemon
        .read(asks)
        .map(|state| state.open)
        .unwrap_or_default();
    // The feature that asked for it reads it with its username: an entry without is of no use.
    if username.is_none()
        && open
            .iter()
            .any(|r| r.name == asked.name && r.takes_username)
    {
        return Err(Failure::refused(format!(
            "the vault entry {} needs its username: the feature that asked for it signs in with \
             it",
            asked.name
        )));
    }
    let entry = Entry {
        name: asked.name,
        description: given(asked.description),
        url,
        username,
        project,
        updated_at: now_ms(),
        bound,
        shared_from,
    };
    if let Some(secret) = &asked.secret {
        // Masked before it is kept anywhere, so that nothing written after can hold it.
        secrets::add_vault_login(entry.username.as_deref(), secret);
        daemon
            .vault
            .store(&entry.name, secret)
            .await
            .map_err(unavailable)?;
    } else if entry.username.is_some() && entry.username != username_was {
        // The daemon reads the secret itself, to mask the new login: it goes nowhere else, and a
        // keyring that cannot be read (locked) leaves the edit as it is: the login is masked when
        // a tool uses the entry (`given`). A cleared username leaves no login form to mask.
        match daemon.vault.read(&entry.name).await {
            Ok(Some(secret)) => secrets::add_vault_login(entry.username.as_deref(), &secret),
            Ok(None) => {}
            Err(why) => eprintln!("flottille daemon: vault entry {}: {why}", entry.name),
        }
    }
    daemon.write(|store| store.put_vault_entry(&entry))?;
    retool(daemon);
    // However it was set, from the form its request opened or another way, what asked is told:
    // the agents whose project sees it.
    for request in open.iter().filter(|r| entry.name == r.name) {
        answered(daemon, request, Some(&entry));
    }
    Ok(entry.to_json())
}

/// The entry `name` sent to the user's machine `machine` (`docs/remote.md`, "Vault entries"): only
/// as the request the user's device signed, which goes with it for that machine to check; its
/// secret read from the keyring here and sent with its metadata, in nothing else.
async fn share(daemon: &Daemon, name: &str, machine: &str) -> Result<Value, Failure> {
    let asked = crate::daemon::asked()
        .filter(|asked| {
            serde_json::from_str::<Value>(&asked.signed)
                .is_ok_and(|request| request["signed"]["method"] == "vault.share")
        })
        .ok_or_else(|| Failure::refused(NOT_SIGNED))?;
    let (metadata, secret) = copy_of(daemon, name).await?;
    let message = flottille_link::Share {
        request: serde_json::from_str(&asked.signed)?,
        entry: metadata,
        secret: flottille_link::Secret(secret.clone()),
    };
    crate::link::share(daemon, machine, message)
        .await
        .map_err(|why| Failure::refused(masked_of(&why, &secret)))?;
    eprintln!(
        "flottille daemon: the vault entry {name} was shared with {machine}, asked by {}",
        asked.device
    );
    Ok(json!({"name": name, "machine": machine}))
}

/// The entry `name` as it goes to another machine: its metadata, less where this machine's copy
/// came from (the receiver's to say), and its secret.
pub(crate) async fn copy_of(
    daemon: &Daemon,
    name: &str,
) -> Result<(Value, zeroize::Zeroizing<String>), Failure> {
    // Its metadata and its secret of one moment: a Save of it in progress ends first.
    let _writing = writing(name).await;
    let Some(entry) = daemon
        .read(|store| store.vault_entries())?
        .into_iter()
        .find(|entry| entry.name == name)
    else {
        return Err(Failure::new(
            "not-found",
            format!("the vault has no entry {name}"),
        ));
    };
    let secret = daemon
        .vault
        .read(name)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| {
            Failure::refused(format!(
                "the keyring holds no secret for the vault entry {name}: set it again"
            ))
        })?;
    let secret = zeroize::Zeroizing::new(secret);
    let mut metadata = entry.to_json();
    // Where it came from is the receiver's to say: the sender's own name for it.
    if let Some(object) = metadata.as_object_mut() {
        object.remove("shared_from");
    }
    Ok((metadata, secret))
}

/// The entry `entry` another machine of the user's, `from`, shared with this one, its user's
/// request checked already (`link::Remote::shared`): its metadata checked here as Save checks it
/// (`checked`), never taken on the sender's word; its secret into this machine's keyring, its
/// metadata into the store, marked as shared from `from`. It replaces a copy `from` shared
/// before, never an entry of this machine's own nor another machine's copy. What is refused is
/// said in a sentence that holds no secret, and leaves nothing behind.
pub async fn receive(
    daemon: &std::sync::Arc<Daemon>,
    entry: &Value,
    secret: &str,
    from: &str,
) -> Result<(), String> {
    #[derive(Deserialize)]
    struct Shared {
        name: String,
        description: Option<String>,
        url: Option<String>,
        username: Option<String>,
        project: Option<String>,
        #[serde(rename = "for")]
        bound: Option<String>,
    }
    let shared: Shared = serde_json::from_value(entry.clone())
        .map_err(|_| "refused: this is not a vault entry's metadata".to_owned())?;
    let name = shared.name;
    if !valid_name(&name) {
        return Err("refused: this is not a vault entry's name".to_owned());
    }
    if secret.chars().count() < VAULT_SHORTEST {
        return Err(format!(
            "refused: a secret is {VAULT_SHORTEST} characters at least"
        ));
    }
    let _writing = writing(&name).await;
    let existing = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|entry| entry.name == name);
    match existing.as_ref().map(|entry| entry.shared_from.as_deref()) {
        Some(None) => {
            return Err(format!(
                "this machine has its own vault entry {name}: remove or rename it here first"
            ));
        }
        Some(Some(other)) if other != from => {
            return Err(format!(
                "the vault entry {name} here was shared from {other}: remove it here first"
            ));
        }
        _ => {}
    }
    // What this machine lacks is said as such: the project the entry is for must be here.
    let here = |project: &str| crate::projects::load_project(daemon, project).is_ok();
    if let Some(project) = shared
        .bound
        .as_deref()
        .and_then(|bound| bound.strip_prefix("worklog:"))
        && !here(project)
    {
        return Err(format!(
            "the vault entry {name} is bound to {}, which is not on this machine: add the \
             project here first",
            use_said(shared.bound.as_deref().unwrap_or_default()).unwrap_or_default()
        ));
    }
    if let Some(project) = &shared.project
        && !here(project)
    {
        return Err(format!(
            "the vault entry {name} is the project {project}'s, which is not on this machine: \
             add the project here first"
        ));
    }
    let project = checked(
        daemon,
        &name,
        shared.project,
        shared.bound.as_deref(),
        shared.url.as_deref(),
    )
    .map_err(|failure| failure.message)?;
    let entry = Entry {
        name: name.clone(),
        description: shared.description,
        url: shared.url,
        username: shared.username,
        project,
        updated_at: now_ms(),
        bound: shared.bound,
        shared_from: Some(from.to_owned()),
    };
    // The copy it replaces, to put back if the store refuses the new one.
    let before = match &existing {
        Some(_) => daemon
            .vault
            .read(&name)
            .await
            .map_err(|why| masked_of(&why, secret))?
            .map(zeroize::Zeroizing::new),
        None => None,
    };
    // Masked before it is kept anywhere, so that nothing written after can hold it.
    secrets::add_vault_login(entry.username.as_deref(), secret);
    daemon
        .vault
        .store(&name, secret)
        .await
        .map_err(|why| masked_of(&why, secret))?;
    if let Err(error) = daemon.write(|store| store.put_vault_entry(&entry)) {
        // No secret is left in the keyring that no entry of the store names.
        let undone = match &before {
            Some(before) => daemon.vault.store(&name, before).await,
            None => daemon.vault.delete(&name).await,
        };
        if let Err(why) = undone {
            eprintln!(
                "flottille daemon: the vault entry {name} shared from {from}: {}",
                masked_of(&why, secret)
            );
        }
        return Err(masked_of(&error.to_string(), secret));
    }
    retool(daemon);
    eprintln!("flottille daemon: the vault entry {name} was shared from {from}");
    // An agent that asked for it is told, as when the user sets it here.
    let open = daemon
        .read(asks)
        .map(|state| state.open)
        .unwrap_or_default();
    for request in open.iter().filter(|r| entry.name == r.name) {
        answered(daemon, request, Some(&entry));
    }
    Ok(())
}

/// The user's Share of the entry `name` from an agent's connection request: every project's from
/// now on, the rest of it as it was. A worklog's entry stays its project's alone.
pub async fn shared(daemon: &Daemon, name: &str) -> Result<Value, Failure> {
    rewritten(daemon, name, |entry| {
        if let Some(bound) = entry.bound.as_deref().filter(|b| b.starts_with("worklog:")) {
            return Err(Failure::refused(format!(
                "the vault entry {name} is bound to {}, which is its project's alone: name \
                 another entry for the connection",
                use_said(bound).unwrap_or_else(|| bound.to_owned())
            )));
        }
        entry.project = None;
        Ok(())
    })
    .await
}

/// The user's Bind of the entry `name` from an agent's connection request: its `url` becomes
/// `origin`, the address the request's connection sends it to, read by the daemon; the rest of
/// it as it was.
pub async fn bound_to(daemon: &Daemon, name: &str, origin: &str) -> Result<Value, Failure> {
    rewritten(daemon, name, |entry| {
        entry.url = Some(origin.to_owned());
        Ok(())
    })
    .await
}

/// The entry `name`'s metadata changed by `change` and written, its secret untouched; after a
/// Save of the same entry in progress, never between its read and its write (which would undo
/// this change).
async fn rewritten(
    daemon: &Daemon,
    name: &str,
    change: impl FnOnce(&mut Entry) -> Result<(), Failure>,
) -> Result<Value, Failure> {
    let _writing = writing(name).await;
    let mut entry = daemon
        .read(|store| store.vault_entries())?
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| Failure::new("not-found", format!("the vault has no entry {name}")))?;
    change(&mut entry)?;
    entry.updated_at = now_ms();
    daemon.write(|store| store.put_vault_entry(&entry))?;
    retool(daemon);
    Ok(entry.to_json())
}

/// The project of the entry `name` whose metadata is `project`, `bound` and `url`, as Save and a
/// share from another machine check it: the project is one of this machine's, and the use is one
/// it fits (`bound_checked`).
fn checked(
    daemon: &Daemon,
    name: &str,
    project: Option<String>,
    bound: Option<&str>,
    url: Option<&str>,
) -> Result<Option<String>, Failure> {
    if let Some(project) = &project {
        crate::projects::load_project(daemon, project)?;
    }
    // A worklog's entry is its project's alone: no other project's agent learns it exists.
    match bound {
        Some(bound) => bound_checked(daemon, name, bound, project.as_deref(), url),
        None => Ok(project),
    }
}

/// The project an entry bound to `bound` has, if Flottille knows the use and it fits the entry's
/// `project`: a project's worklog reads only an entry that project sees, so the entry is the
/// project's; a connection's entry is any project's when its login reads it for the calling
/// agent's project, every project's when its server is given it at its start (that server serves
/// every project's agents), and its `url` is no plain `http` to another machine (its login would
/// type the password there in clear).
fn bound_checked(
    daemon: &Daemon,
    name: &str,
    bound: &str,
    project: Option<&str>,
    url: Option<&str>,
) -> Result<Option<String>, Failure> {
    let Some(said) = use_said(bound) else {
        return Err(Failure::refused(format!(
            "an entry is bound to worklog:<project> or connection:<name>: {bound} is neither"
        )));
    };
    if !crate::extensions::bound_possible(bound) {
        return Err(Failure::refused(format!(
            "no connection can be named as {bound} says: a connection's name is lowercase \
             letters, digits and _"
        )));
    }
    if let Some(("worklog", of)) = bound.split_once(':')
        && let Err(failure) = crate::projects::load_project(daemon, of)
    {
        return Err(Failure::refused(match failure.code {
            "not-found" => format!("no project is named {of}"),
            _ => format!(
                "the file of the project {of} cannot be read: {}",
                failure.message
            ),
        }));
    }
    if bound.starts_with("connection:")
        && let Some(url) = url
    {
        if let Some(host) = in_clear(url) {
            return Err(Failure::refused(format!(
                "an entry for {said} cannot be on plain http to {host}, another machine: its \
                 password would travel in clear. Give its https address"
            )));
        }
        // Login reads the URL through `site()`: one it cannot read would be skipped there, long
        // after it was saved.
        if crate::extensions::site_of(url).is_none() {
            return Err(Failure::refused(no_web_address(url)));
        }
    }
    match (bound.split_once(':'), project) {
        (Some(("worklog", of)), Some(own)) if of != own => Err(Failure::refused(format!(
            "an entry the project {own} alone sees cannot be bound to {said}"
        ))),
        (Some(("worklog", of)), _) => Ok(Some(of.to_owned())),
        (Some(("connection", _)), Some(own)) => {
            match crate::extensions::bound_given_to_server(daemon, bound, name) {
                Ok(false) => Ok(Some(own.to_owned())),
                Ok(true) => Err(Failure::refused(format!(
                    "the server of {said} is given the entry {name} when it starts, and serves \
                     every project's agents: the entry is every project's, not the project \
                     {own}'s alone"
                ))),
                Err(why) => Err(Failure::refused(format!(
                    "whether the server of {said} is given the entry {name} cannot be told, so \
                     it cannot be the project {own}'s alone: {why}"
                ))),
            }
        }
        _ => Ok(project.map(str::to_owned)),
    }
}

/// Whether the feature the use `bound` stands for names the entry `name` now; why it cannot be
/// told when the files that declare it cannot be read.
fn named_by(daemon: &Daemon, bound: &str, name: &str) -> Result<bool, String> {
    match bound.split_once(':') {
        // A project that is gone names nothing; one whose file cannot be read cannot say.
        Some(("worklog", project)) => match crate::projects::load_project(daemon, project) {
            Ok(project) => Ok(matches!(&project.tracker, Some(ProjectTracker::Jira {
                worklog_token: Some(token), ..
            }) if token == name)),
            Err(failure) if failure.code == "not-found" => Ok(false),
            Err(failure) => Err(failure.message),
        },
        _ => crate::extensions::bound_names(daemon, bound, name),
    }
}

/// Why `url` is no address a secret may go to: the words of the ask's refusal and of Save's.
fn no_web_address(url: &str) -> String {
    format!("{url} is no web address a secret may go to: https, or http on this machine")
}

/// The host of `url` when it is plain `http` to another machine.
fn in_clear(url: &str) -> Option<String> {
    crate::extensions::in_clear(url)
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
/// of Flottille names one it misses (an agent that has a connection has it too, `agents::tools_of`).
pub fn listed_to(daemon: &Daemon, project: &Project) -> bool {
    daemon
        .read(|store| store.vault_entries())
        .is_ok_and(|entries| entries.iter().any(|entry| entry.seen_by(&project.name)))
        || !needs(daemon, project).is_empty()
}

/// An entry a feature of Flottille names for a project and the vault does not give it: an ask for
/// it is prefilled by the feature, not by the agent.
pub struct Need {
    pub name: String,
    pub description: String,
    /// The use the feature reads it for.
    pub bound: String,
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
                "The Jira API token tickets uses on {site} where acli falls short (time, \
                 worklogs, fields, edits, replies, mentions); its username is the Jira \
                 account's e-mail"
            ),
            bound: uses::worklog(&project.name),
            takes_username: true,
        });
    }
    needs
}

/// What a refusal for the missing entry `name` a feature names adds: the agent may ask the user
/// for it.
pub fn offer(daemon: &Daemon, project: &Project, name: &str) -> String {
    if !needs(daemon, project).iter().any(|need| need.name == name) {
        return String::new();
    }
    format!(
        "; or ask the user for it, who types only the secret: vault {{action: \"ask\", name: \
         \"{name}\", why: \"<what you need it for>\"}}, answered at once, the user's answer \
         coming later as a delivery"
    )
}

/// A request for an entry, open until the user saves it or dismisses the card: its number is the
/// number of its `vault.asked` event. What it holds prefills Settings, Vault's form of a new
/// entry; the user may change all of it.
struct Ask {
    number: u64,
    at: u64,
    agent: String,
    /// The project of the agent that asked: where its card shows.
    project: String,
    name: String,
    why: String,
    description: Option<String>,
    url: Option<String>,
    /// The host the secret goes to, as a connection's login reads `url`.
    host: Option<String>,
    username: Option<String>,
    /// The use it is for, if any.
    bound: Option<String>,
    /// The entry is every project's, not the asking project's alone.
    every: bool,
    /// The connection it is for, declared or asked for.
    connection: Option<String>,
    takes_username: bool,
    /// The agents whose ask joined it while it was open, with their projects.
    joined: Vec<(String, String)>,
}

impl Ask {
    fn to_json(&self) -> Value {
        json!({"ask": self.number, "at": self.at, "agent": self.agent, "project": self.project,
            "name": self.name, "why": self.why, "description": self.description,
            "url": self.url, "host": self.host, "username": self.username, "for": self.bound,
            "every": self.every, "connection": self.connection,
            "takes_username": self.takes_username,
            "joined": self.joined.iter().map(|(agent, _)| agent).collect::<Vec<_>>()})
    }
}

/// The requests still open, as their events say.
struct Asks {
    open: Vec<Ask>,
}

fn asks(store: &Store) -> flottille_core::Result<Asks> {
    let events = store.events_named(
        &["vault.asked", "vault.ask_joined", "vault.ask_answered"],
        0,
    )?;
    let mut state = Asks { open: Vec::new() };
    let text = |data: &Value, key: &str| data[key].as_str().unwrap_or_default().to_owned();
    let given = |data: &Value, key: &str| data[key].as_str().map(str::to_owned);
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
                description: given(data, "description"),
                url: given(data, "url"),
                host: given(data, "host"),
                username: given(data, "username"),
                bound: given(data, "for"),
                every: data["every"] == true,
                connection: given(data, "connection"),
                takes_username: data["takes_username"] == true,
                joined: Vec::new(),
            }),
            "vault.ask_joined" => {
                if let Some(ask) = state.open.iter_mut().find(|a| data["ask"] == a.number) {
                    ask.joined
                        .push((text(data, "agent"), text(data, "project")));
                }
            }
            _ => state.open.retain(|ask| data["ask"] != ask.number),
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

/// The request is answered, with the entry `set` or dismissed: its card folds away, and each agent
/// that asked is told what the user did, never the secret; of an entry set, only the agents whose
/// project sees it.
fn answered(daemon: &std::sync::Arc<Daemon>, request: &Ask, set: Option<&Entry>) {
    let outcome = if set.is_some() { "set" } else { "dismissed" };
    let data = json!({"agent": request.agent, "ask": request.number, "project": request.project,
        "name": request.name, "outcome": outcome});
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
    let body = match set {
        Some(_) => format!(
            "the user set the vault entry `{}`: give its name to the tool that needs it",
            request.name
        ),
        None => format!(
            "the user dismissed the request for the vault entry `{}`",
            request.name
        ),
    };
    let asked = std::iter::once((&request.agent, &request.project)).chain(
        request
            .joined
            .iter()
            .map(|(agent, project)| (agent, project)),
    );
    for (agent, project) in asked {
        if set.is_some_and(|entry| !entry.seen_by(project)) {
            continue;
        }
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
            Ok(Some(secret)) => {
                // A connection's sign-in keeps its tokens in one value: each is masked alone too.
                for inner in crate::extensions::secrets_in(&secret) {
                    secrets::add_vault_value(&inner);
                }
                secrets::add_vault_login(entry.username.as_deref(), &secret);
            }
            Ok(None) => {}
            Err(why) => eprintln!("flottille daemon: vault entry {}: {why}", entry.name),
        }
    }
}

/// How long a tool's use of a secret waits for the keyring: no tool call waits for a human, and a
/// keyring that asks to be unlocked is answered `LOCKED` at once.
const SECRET_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// The secret of the entry `name`, for a tool of Flottille's to put where it goes (an HTTP header,
/// a command's standard input) on behalf of an agent of `project` and for the use `bound` (the
/// feature reading it): never to be returned, logged or shown. An entry of another project, or
/// bound to another use, is no entry for it.
pub async fn secret(
    daemon: &Daemon,
    project: &str,
    bound: &str,
    name: &str,
) -> Result<String, String> {
    let entries = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?;
    given(&entries, &daemon.vault, project, bound, name).await
}

/// The secret of the entry `name`, which must be every project's (no `project` on it), for the
/// server of the connection `connection`, which the daemon runs for all of them: never to be
/// returned, logged or shown. Masked from now on before it is used: the keyring may have been
/// locked at the daemon's start.
pub async fn every_project_s(
    daemon: &Daemon,
    connection: &str,
    name: &str,
) -> Result<String, String> {
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
        Some(entry) => entry.usable_for(&uses::connection(connection))?,
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
    bound: &str,
    name: &str,
) -> Result<String, String> {
    let Some(entry) = entries
        .iter()
        .find(|entry| entry.name == name && entry.seen_by(project))
    else {
        return Err(format!("the vault has no entry {name} for this project"));
    };
    entry.usable_for(bound)?;
    let secret = tokio::time::timeout(SECRET_WAIT, keyring.read(name))
        .await
        .map_err(|_| format!("the keyring did not answer in time: {LOCKED}"))??
        .ok_or_else(|| format!("the keyring no longer holds the secret of {name}"))?;
    // Masked at the moment of use: a login the edit could not mask (a locked keyring, an edit at
    // the same time as another) is masked before the tool writes anything.
    secrets::add_vault_login(entry.username.as_deref(), &secret);
    Ok(secret)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    action: String,
    name: Option<String>,
    why: Option<String>,
    url: Option<String>,
    username: Option<String>,
    #[serde(rename = "for")]
    bound: Option<String>,
    project: Option<String>,
    connection: Option<String>,
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
            "refused: vault lists the entries, or asks the user to add one; a secret is never \
             shown: give an entry's name to the tool that needs it"
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

/// `vault {action: "ask"}`: the user is offered Settings, Vault's form of a new entry, prefilled
/// with what the agent gave (or, for an entry a feature names, with what the feature says),
/// answered at once; a second ask for a name while one is open joins it.
fn ask(daemon: &Daemon, grant: &Grant, asked: Asked) -> Outcome {
    let given = |text: Option<String>| {
        text.map(|text| text.trim().to_owned())
            .filter(|t| !t.is_empty())
    };
    let (Some(name), Some(why)) = (given(asked.name), given(asked.why)) else {
        return Outcome::Refused(
            "refused: ask needs name, the new entry's, and why, what you need it for".into(),
        );
    };
    if !valid_name(&name) {
        return Outcome::Refused(format!(
            "refused: an entry's name is letters, digits, . _ and -, starting with a letter or a \
             digit, 64 at most: {name} is not"
        ));
    }
    let project = match crate::projects::load_project(daemon, &grant.place.project) {
        Ok(project) => project,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    let entries = match daemon.read(|store| store.vault_entries()) {
        Ok(entries) => entries,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    // An entry is never replaced from an ask; another project's is not described to this one.
    if let Some(entry) = entries.iter().find(|entry| entry.name == name) {
        return Outcome::Refused(if entry.seen_by(&project.name) {
            format!(
                "refused: the vault already has the entry {name}: use it (vault list says what \
                 it is for), or ask for another name"
            )
        } else {
            format!("refused: the name {name} is taken in the vault: ask under another name")
        });
    }
    let need = needs(daemon, &project)
        .into_iter()
        .find(|need| need.name == name);
    let bound = match (&need, given(asked.bound).as_deref()) {
        (Some(need), _) => Some(need.bound.clone()),
        (None, None) => None,
        (None, Some("worklog")) => Some(uses::worklog(&project.name)),
        (None, Some(bound))
            if bound.starts_with("connection:") && crate::extensions::bound_possible(bound) =>
        {
            let connection = &bound["connection:".len()..];
            match crate::extensions::bound_exists(daemon, bound) {
                Ok(true) => Some(bound.to_owned()),
                Ok(false) => {
                    return Outcome::Refused(format!(
                        "refused: there is no connection {connection}: for names one the user \
                         declared"
                    ));
                }
                Err(why) => return Outcome::Failed(why),
            }
        }
        (None, Some(bound)) => {
            return Outcome::Refused(format!(
                "refused: for is worklog (this project's) or connection:<name>, not {bound}"
            ));
        }
    };
    // The connection whose server is given the entry: named, or the one this agent's own request
    // waiting for the user names it in. Its entry is every project's, at its address unless the
    // agent gave one. A login's entry (`for`) is not one: its project is left as asked here.
    let connection = match given(asked.connection) {
        Some(connection) => {
            match crate::extensions::asked_for(daemon, &grant.agent, &connection, &name) {
                Ok(Some(origin)) => Some((connection, origin)),
                Ok(None) => {
                    return Outcome::Refused(format!(
                        "refused: there is no connection {connection}, declared or asked for: \
                         connection names one the user declared, or one your request asks to add"
                    ));
                }
                Err(why) => return Outcome::Refused(format!("refused: {why}")),
            }
        }
        // A use asked for a connection whose server is given the entry is that connection's;
        // any other (a login's) keeps the project asked.
        None => crate::extensions::wanting(daemon, &grant.agent, &name).or_else(|| {
            let connection = bound.as_deref()?.strip_prefix("connection:")?;
            match crate::extensions::asked_for(daemon, &grant.agent, connection, &name) {
                Ok(Some(origin)) => Some((connection.to_owned(), origin)),
                Ok(None) | Err(_) => None,
            }
        }),
    };
    let project_asked = given(asked.project);
    if connection.is_some() && project_asked.as_deref() == Some("this") {
        return Outcome::Refused(
            "refused: a connection's entry is every project's, since a connection is given to \
             every project: leave project out"
                .into(),
        );
    }
    // A connection's server is given to every project: its entry is every project's.
    let every = match (project_asked.as_deref(), &bound) {
        (Some("every"), Some(b)) if b.starts_with("worklog:") => {
            return Outcome::Refused(
                "refused: an entry for this project's worklog is this project's alone".into(),
            );
        }
        (Some("every"), _) => true,
        (None | Some("this"), _) => connection.is_some(),
        (Some(other), _) => {
            return Outcome::Refused(format!("refused: project is this or every, not {other}"));
        }
    };
    let masked = |text: String| crate::mcp::masked(&text, &grant.place);
    let why = masked(why);
    let description = need.as_ref().map(|need| need.description.clone());
    let takes_username = need.as_ref().is_some_and(|need| need.takes_username);
    let (url, username) = (
        given(asked.url)
            .map(masked)
            .or_else(|| connection.as_ref().and_then(|(_, origin)| origin.clone())),
        given(asked.username).map(masked),
    );
    let connection = connection.map(|(connection, _)| connection);
    // Where the secret goes, as a connection's login reads the URL: the card says that host,
    // never its own reading of the text.
    let host = match url.as_deref() {
        None => None,
        Some(url) => match (in_clear(url), crate::extensions::site_of(url)) {
            (Some(host), _) => {
                return Outcome::Refused(format!(
                    "refused: {url} is plain http to {host}, another machine: a secret would \
                     travel in clear; ask with its https address"
                ));
            }
            (None, None) => {
                return Outcome::Refused(format!("refused: {}", no_web_address(url)));
            }
            (None, Some(host)) => Some(host),
        },
    };
    let for_connection = match &connection {
        Some(connection) => format!(", every project's, for the connection {connection}"),
        None => String::new(),
    };
    let written = daemon.write(|store| {
        let state = asks(store)?;
        let open = state.open.iter().find(|ask| ask.name == name);
        // Another project's request is not joined, nor described: its entry would not be this
        // project's.
        if open.is_some_and(|open| !open.every && open.project != project.name) {
            return Ok((None, None));
        }
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
                        "why": why, "description": description, "url": url, "host": host,
                        "username": username, "for": bound, "every": every,
                        "connection": connection, "takes_username": takes_username}),
                    false,
                )?,
                false,
            ),
        };
        Ok((Some(joined), Some(event)))
    });
    match written {
        Err(error) => Outcome::Failed(error.to_string()),
        Ok(None) => Outcome::Refused(format!(
            "refused: the name {name} is taken: ask under another name"
        )),
        Ok(Some(true)) => Outcome::Done(format!(
            "The user is already asked for the vault entry {name}: yours joins that request. \
             Nothing waits: continue with what does not need it; a delivery says when the user \
             sets it or dismisses it."
        )),
        Ok(Some(false)) => Outcome::Done(format!(
            "The user is offered the vault's form of a new entry {name}{for_connection}, \
             prefilled with what you gave. Nothing waits: continue with what does not need it; a delivery says when \
             the user sets it or dismisses it. You never see the secret: once set, give its name \
             to the tool that needs it."
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
            bound: None,
            shared_from: None,
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
    fn the_memory_keyring_is_refused_outside_tests() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("keyring.toml");
        std::fs::write(
            &file,
            "backend = \"memory\"\n[kept]\ndeploy = \"dep10y-key\"\n",
        )
        .unwrap();

        let (keyring, said) = Keyring::chosen(file.clone(), false);
        assert!(
            matches!(keyring, Keyring::Os),
            "a build for users keeps nothing in a file"
        );
        let said = said.unwrap();
        assert!(said.contains("only a test build"), "{said}");
        assert!(!said.contains("dep10y-key"), "{said}");

        let (keyring, _) = Keyring::chosen(file, true);
        assert!(matches!(keyring, Keyring::Memory { .. }));
    }

    #[tokio::test]
    async fn another_project_s_entry_is_not_given_to_a_tool() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = memory(&dir.path().join("keyring.toml"));
        let entries = [entry("deploy", Some("app"))];

        let refused = given(&entries, &keyring, "other", "worklog:other", "deploy")
            .await
            .unwrap_err();
        assert!(refused.contains("no entry deploy"), "{refused}");
        assert!(!refused.contains("dep10y-key"), "{refused}");
        assert_eq!(
            given(&entries, &keyring, "app", "worklog:app", "deploy")
                .await
                .unwrap(),
            "dep10y-key"
        );
        assert!(
            given(&entries, &keyring, "app", "worklog:app", "unknown")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_locked_keyring_is_answered_at_once_never_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("keyring.toml");
        std::fs::write(&file, "backend = \"memory\"\nlocked = true\n").unwrap();
        let keyring = memory(&file);
        let entries = [entry("deploy", None)];

        let asked = std::time::Instant::now();
        let refused = given(&entries, &keyring, "app", "worklog:app", "deploy")
            .await
            .unwrap_err();
        assert!(asked.elapsed() < SECRET_WAIT, "answered at once");
        assert_eq!(refused, LOCKED);
    }

    /// A daemon on temporary folders whose keyring is its memory.
    fn a_daemon(dir: &Path) -> std::sync::Arc<Daemon> {
        let paths = Paths {
            runtime: dir.join("runtime"),
            state: dir.join("state"),
            config: dir.join("config"),
            data: dir.join("data"),
        };
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join(CHOICE), "backend = \"memory\"\n").unwrap();
        let store = flottille_core::store::Store::open_in_memory().unwrap();
        let projects = crate::projects::Projects::open(&paths).unwrap();
        std::sync::Arc::new(Daemon::new(&paths, store, projects, 0, None))
    }

    /// The value of a call that succeeded; a failure says its message.
    fn ok<T>(result: Result<T, Failure>) -> T {
        result.unwrap_or_else(|failure| panic!("{}", failure.message))
    }

    async fn listed(daemon: &std::sync::Arc<Daemon>) -> Vec<Entry> {
        daemon.read(|store| store.vault_entries()).unwrap()
    }

    #[tokio::test]
    async fn editing_an_entry_without_a_secret_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "jira", "secret": "k3pt-s3cret-value", "description": "Logs time",
                "url": "https://tracker.example.com"}),
        )
        .await);

        // The keyring refuses every store from here on: an edit that stored the secret again
        // would fail, so the edits below prove that nothing is stored.
        std::fs::write(
            dir.path().join("config/keyring.toml"),
            "backend = \"memory\"\nfail_stores = 1000\n",
        )
        .unwrap();
        // The secret is absent, or null: the metadata is rewritten, the keyring item stays.
        for absent in [json!({}), json!({"secret": null})] {
            let mut params = json!({"name": "jira", "description": "Logs time on Jira",
                "url": "https://tracker.example.org"});
            params
                .as_object_mut()
                .unwrap()
                .extend(absent.as_object().unwrap().clone());
            ok(call(daemon.clone(), "vault.set", params).await);
            let entries = listed(&daemon).await;
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].description.as_deref(), Some("Logs time on Jira"));
            assert_eq!(
                entries[0].url.as_deref(),
                Some("https://tracker.example.org")
            );
            assert_eq!(
                secret(&daemon, "any", "worklog:any", "jira").await.unwrap(),
                "k3pt-s3cret-value"
            );
        }
    }

    #[tokio::test]
    async fn editing_an_entry_with_a_secret_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        for (secret_of, url) in [
            ("f1rst-s3cret-value", "https://a.example.com"),
            ("n3xt-s3cret-value", "https://b.example.com"),
        ] {
            call(
                daemon.clone(),
                "vault.set",
                json!({"name": "jira", "secret": secret_of, "url": url}),
            )
            .await
            .map_err(|failure| failure.message)
            .unwrap();
        }
        assert_eq!(
            listed(&daemon).await[0].url.as_deref(),
            Some("https://b.example.com")
        );
        assert_eq!(
            secret(&daemon, "any", "worklog:any", "jira").await.unwrap(),
            "n3xt-s3cret-value"
        );
    }

    #[tokio::test]
    async fn editing_only_the_username_masks_the_new_basic_form() {
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let secret_of = "gr4ce-only-s3cret-value";
        let form_of = |user: &str| {
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{secret_of}"))
        };
        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "jira", "secret": secret_of, "username": "ada"}),
        )
        .await);
        let written = |form: &str| {
            flottille_core::secrets::mask(
                // Not in an `Authorization` header, which is masked whatever it holds: the
                // form alone is what the vault's own masks have to catch.
                &format!("the tool sent {form} and went on"),
                &Default::default(),
            )
        };
        assert!(!written(&form_of("ada")).contains(&form_of("ada")));
        assert!(
            written(&form_of("grace")).contains(&form_of("grace")),
            "the form of a username nobody set yet is not masked"
        );

        // No secret is typed: the daemon reads the one it keeps, to mask the new login.
        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "jira", "username": "grace"}),
        )
        .await);
        assert!(!written(&form_of("grace")).contains(&form_of("grace")));
    }

    #[tokio::test]
    async fn a_username_edit_while_the_keyring_is_locked_is_masked_at_use() {
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let keyring_file = dir.path().join("config/keyring.toml");
        let secret_of = "l0cked-edit-s3cret-value";
        let form_of = |user: &str| {
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{secret_of}"))
        };
        let written = |form: &str| {
            flottille_core::secrets::mask(
                // Not in an `Authorization` header, which is masked whatever it holds.
                &format!("the tool sent {form} and went on"),
                &Default::default(),
            )
        };
        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "jira", "secret": secret_of, "username": "ada"}),
        )
        .await);

        // The keyring is locked during the edit: it is saved, but the secret cannot be read to
        // mask the new login.
        std::fs::write(&keyring_file, "backend = \"memory\"\nlocked = true\n").unwrap();
        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "jira", "username": "grace"}),
        )
        .await);
        assert_eq!(listed(&daemon).await[0].username.as_deref(), Some("grace"));
        assert!(
            written(&form_of("grace")).contains(&form_of("grace")),
            "nothing could mask the new form yet"
        );

        // Unlocked, a tool uses the entry: the secret is read, and the form is masked from then.
        std::fs::write(&keyring_file, "backend = \"memory\"\n").unwrap();
        assert_eq!(
            secret(&daemon, "any", "worklog:any", "jira").await.unwrap(),
            secret_of
        );
        assert!(!written(&form_of("grace")).contains(&form_of("grace")));
    }

    #[tokio::test]
    async fn clearing_the_username_reads_no_secret() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let secret_of = "n3ver-read-s3cret-value";
        // The secret is in the keyring from before the daemon started, so that nothing has
        // masked it: only a read of it would.
        std::fs::write(
            dir.path().join("config/keyring.toml"),
            format!("backend = \"memory\"\n[kept]\njira = \"{secret_of}\"\n"),
        )
        .unwrap();
        daemon
            .write(|store| {
                store.put_vault_entry(&Entry {
                    username: Some("ada".into()),
                    ..entry("jira", None)
                })
            })
            .unwrap();
        let written =
            || flottille_core::secrets::mask(&format!("it said {secret_of}"), &Default::default());
        assert!(written().contains(secret_of), "nothing masks it yet");

        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "jira", "username": ""}),
        )
        .await);
        assert_eq!(listed(&daemon).await[0].username, None);
        assert!(
            written().contains(secret_of),
            "the secret was read though no login form was left to mask"
        );
    }

    /// A secret another machine shares, invented.
    const SHARED: &str = "sh4red-t0ken-9Qx2";

    #[tokio::test]
    async fn a_share_does_not_replace_this_machine_s_own_entry() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "tracker", "secret": "own-t0ken-Zr81", "url": "https://own.example.com"}),
        )
        .await);

        let refused = receive(
            &daemon,
            &json!({"name": "tracker", "url": "https://theirs.example.com"}),
            SHARED,
            "pc",
        )
        .await
        .unwrap_err();

        assert!(
            refused.contains(
                "this machine has its own vault entry tracker: remove or rename it here first"
            ),
            "{refused}"
        );
        let entries = listed(&daemon).await;
        assert_eq!(entries[0].url.as_deref(), Some("https://own.example.com"));
        assert_eq!(entries[0].shared_from, None);
        assert_eq!(
            daemon.vault.read("tracker").await.unwrap().as_deref(),
            Some("own-t0ken-Zr81")
        );
    }

    #[tokio::test]
    async fn a_share_replaces_a_copy_from_the_same_machine_only() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        receive(&daemon, &json!({"name": "tracker"}), SHARED, "pc")
            .await
            .unwrap();

        // Shared again from pc: the copy is replaced.
        receive(
            &daemon,
            &json!({"name": "tracker", "description": "Again"}),
            "s3cond-t0ken-Lm4P",
            "pc",
        )
        .await
        .unwrap();
        assert_eq!(
            listed(&daemon).await[0].description.as_deref(),
            Some("Again")
        );
        // From laptop: pc's copy is not laptop's to replace.
        let refused = receive(&daemon, &json!({"name": "tracker"}), SHARED, "laptop")
            .await
            .unwrap_err();
        assert!(
            refused.contains("the vault entry tracker here was shared from pc"),
            "{refused}"
        );
        assert_eq!(listed(&daemon).await[0].shared_from.as_deref(), Some("pc"));
        assert_eq!(
            daemon.vault.read("tracker").await.unwrap().as_deref(),
            Some("s3cond-t0ken-Lm4P")
        );
    }

    /// What this machine refuses of the metadata another machine sent: what Save would refuse.
    #[tokio::test]
    async fn a_share_s_metadata_is_checked_as_save_checks_it() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let cases = [
            (
                json!({"name": "wiki", "url": "http://wiki.example.com/", "for": "connection:wiki"}),
                "cannot be on plain http to wiki.example.com",
            ),
            (
                json!({"name": "jira", "for": "worklog:gone"}),
                "the vault entry jira is bound to the worklog of the project gone, which is not on \
                 this machine: add the project here first",
            ),
            (
                json!({"name": "notes", "project": "gone"}),
                "the vault entry notes is the project gone's, which is not on this machine: add \
                 the project here first",
            ),
            (json!({"name": "odd", "for": "bogus"}), "bogus is neither"),
        ];
        for (entry, said) in cases {
            let refused = receive(&daemon, &entry, SHARED, "pc").await.unwrap_err();
            assert!(refused.contains(said), "{entry}: {refused}");
            let name = entry["name"].as_str().unwrap();
            assert_eq!(daemon.vault.read(name).await.unwrap(), None, "{name}");
        }
        assert!(listed(&daemon).await.is_empty(), "nothing was kept");
    }

    #[tokio::test]
    async fn a_share_the_store_refuses_leaves_no_secret_behind() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("store.db");
        let paths = Paths {
            runtime: dir.path().join("runtime"),
            state: dir.path().join("state"),
            config: dir.path().join("config"),
            data: dir.path().join("data"),
        };
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join(CHOICE), "backend = \"memory\"\n").unwrap();
        let store = flottille_core::store::Store::open(&file).unwrap();
        let projects = crate::projects::Projects::open(&paths).unwrap();
        let daemon = std::sync::Arc::new(Daemon::new(&paths, store, projects, 0, None));
        // A disk that refuses the write, played by the store's own refusal.
        rusqlite::Connection::open(&file)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER full BEFORE INSERT ON vault_entries \
                 BEGIN SELECT RAISE(ABORT, 'the disk is full'); END",
            )
            .unwrap();

        let refused = receive(&daemon, &json!({"name": "tracker"}), SHARED, "pc")
            .await
            .unwrap_err();

        assert!(refused.contains("the disk is full"), "{refused}");
        assert_eq!(daemon.vault.read("tracker").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_share_at_the_moment_of_a_save_keeps_the_user_s_entry() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let mut set = std::pin::pin!(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "tracker", "secret": "own-t0ken-Zr81", "url": "https://own.example.com"}),
        ));
        let theirs = json!({"name": "tracker", "url": "https://theirs.example.com"});
        let mut share = std::pin::pin!(receive(&daemon, &theirs, SHARED, "pc",));
        // The user's Save waits on the keyring when the share comes, which waits on it too.
        assert!(futures::poll!(set.as_mut()).is_pending());
        assert!(futures::poll!(share.as_mut()).is_pending());
        ok(set.await);
        let shared = share.await;

        assert!(
            shared.is_err_and(|refused| refused.contains("this machine has its own vault entry")),
            "the share is refused"
        );
        let entries = listed(&daemon).await;
        assert_eq!(entries[0].url.as_deref(), Some("https://own.example.com"));
        assert_eq!(entries[0].shared_from, None);
        assert_eq!(
            daemon.vault.read("tracker").await.unwrap().as_deref(),
            Some("own-t0ken-Zr81")
        );
    }

    #[tokio::test]
    async fn a_bind_at_the_moment_of_a_save_keeps_the_user_s_bind() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "tracker", "secret": "own-t0ken-Zr81"}),
        )
        .await);
        // The user saves a new secret while they bind the entry from a connection's request.
        let mut set = std::pin::pin!(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "tracker", "secret": "new-t0ken-Qd52"}),
        ));
        assert!(futures::poll!(set.as_mut()).is_pending());
        let mut bound = std::pin::pin!(bound_to(&daemon, "tracker", "https://notes.example.com"));
        assert!(futures::poll!(bound.as_mut()).is_pending());
        ok(set.await);
        ok(bound.await);

        let entries = listed(&daemon).await;
        assert_eq!(
            entries[0].url.as_deref(),
            Some("https://notes.example.com"),
            "the Save did not undo the bind"
        );
    }

    #[tokio::test]
    async fn a_share_sent_at_the_moment_of_a_save_is_one_entry_s_copy() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        ok(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "tracker", "secret": "old-t0ken-Zr81", "url": "https://old.example.com"}),
        )
        .await);
        let mut set = std::pin::pin!(call(
            daemon.clone(),
            "vault.set",
            json!({"name": "tracker", "secret": "new-t0ken-Qd52", "url": "https://new.example.com"}),
        ));
        let mut copy = std::pin::pin!(copy_of(&daemon, "tracker"));
        assert!(futures::poll!(set.as_mut()).is_pending());
        assert!(futures::poll!(copy.as_mut()).is_pending());
        ok(set.await);
        let (metadata, secret) = ok(copy.await);

        let pair = (metadata["url"].as_str().unwrap(), secret.as_str());
        assert!(
            pair == ("https://new.example.com", "new-t0ken-Qd52")
                || pair == ("https://old.example.com", "old-t0ken-Zr81"),
            "the metadata and the secret of one entry: {}",
            pair.0
        );
    }

    #[tokio::test]
    async fn a_share_answers_the_ask_waiting_for_its_entry() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        daemon
            .write(|store| {
                store.create_agent(
                    &crate::daemon::tests::an_agent("a1"),
                    ("agent.created", json!({})),
                )?;
                let event = store.append_event(
                    "vault.asked",
                    json!({"agent": "a1", "project": "app", "name": "tracker",
                        "why": "read the tracker"}),
                )?;
                Ok(((), event))
            })
            .unwrap();
        assert_eq!(daemon.read(asks).unwrap().open.len(), 1);

        receive(&daemon, &json!({"name": "tracker"}), SHARED, "pc")
            .await
            .unwrap();

        assert!(
            daemon.read(asks).unwrap().open.is_empty(),
            "the ask is answered"
        );
    }

    #[tokio::test]
    async fn a_new_entry_still_needs_its_secret() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let refused = call(
            daemon.clone(),
            "vault.set",
            json!({"name": "jira", "url": "https://a.example.com"}),
        )
        .await
        .unwrap_err();
        assert!(refused.message.contains("secret"), "{}", refused.message);
        assert!(listed(&daemon).await.is_empty(), "nothing was kept");
    }
}
