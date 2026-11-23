//! The daemon's side of an update (`docs/releases.md`, "Updating"): it checks the channel
//! `flottille update` follows, at its start and every 6 hours unless `updates.toml` says never,
//! downloads the latest build beside the binary when the user asks (`update.download`), and
//! restarts itself to apply it when the user asks, now or once no agent runs. Nothing is
//! downloaded or applied without the user's asking. The requests go through `crate::update`, the
//! code of `flottille update`, with the user's `curl`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use flottille_core::agent::AgentState;
use flottille_core::paths::Paths;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::update;
use flottille_core::store::Store;

/// How often the daemon checks, after the check at its start.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// How often a restart asked for when idle looks whether an agent still runs.
const IDLE_POLL: Duration = Duration::from_millis(500);

/// How often the progress of a download is told, at most.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// What the start of this daemon refused of a staged build, said in the update's place.
static REFUSED: OnceLock<String> = OnceLock::new();

/// The daemon is asked to start again from its binary once it stopped (`daemon::run`).
static RESTART: AtomicBool = AtomicBool::new(false);

/// What the start found staged and refused: kept to be said.
pub fn refused_at_start(why: String) {
    let _ = REFUSED.set(why);
}

/// Whether the daemon stopped to start again.
pub fn restart_asked() -> bool {
    RESTART.load(Ordering::SeqCst)
}

/// What the daemon knows of the update, beside what is on disk.
#[derive(Default)]
pub struct Live {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The channel's latest build, when it is newer than this one.
    latest: Option<String>,
    /// The channel's latest build, when this build is newer than it.
    ahead_of: Option<String>,
    /// Why the last check failed, said once in the update's place.
    failed: Option<String>,
    checking: bool,
    /// The bytes read, and of how many when known.
    downloading: Option<(u64, Option<u64>)>,
    /// The user cancelled the download that runs.
    cancelling: bool,
    /// Why the last download failed.
    download_failed: Option<String>,
    /// A restart asked for once no agent runs.
    when_idle: bool,
}

/// `updates.toml`'s `check`: automatically (the default) or never.
pub(crate) fn checks_automatically(paths: &Paths) -> Result<bool, String> {
    let file = paths.config.join("updates.toml");
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(format!("updates.toml: {error}")),
    };
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| format!("updates.toml: {}", e.message()))?;
    match document.get("check").map(|check| check.as_str()) {
        None => Ok(true),
        Some(Some("automatically")) => Ok(true),
        Some(Some("never")) => Ok(false),
        Some(_) => Err("updates.toml: check: it is \"automatically\" or \"never\"".into()),
    }
}

/// The binary the daemon runs from, where a build is staged.
fn binary() -> Option<PathBuf> {
    update::binary().ok()
}

/// This build's commit, as `--version` says it.
fn commit() -> Option<&'static str> {
    crate::VERSION
        .rsplit_once(", ")
        .map(|(_, commit)| commit.trim_end_matches(')'))
}

/// What `update.show` answers.
fn shown(daemon: &Daemon) -> Value {
    let paths = &daemon.paths;
    let state = daemon.updates.state.lock().unwrap();
    let staged = binary().and_then(|binary| update::staged(&binary));
    let (channel, branch) = update::followed_now(paths);
    let automatically = checks_automatically(paths);
    json!({
        "version": update::VERSION,
        "full": crate::VERSION,
        "build_channel": update::CHANNEL,
        "commit": commit(),
        "channel": channel,
        "branch": branch,
        // A file that cannot be read means never: what the user wrote there is not overridden.
        "check": match automatically {
            Ok(true) => "automatically",
            _ => "never",
        },
        "latest": state.latest,
        "ahead_of": state.ahead_of,
        "failed": automatically.err().or_else(|| state.failed.clone()),
        "checking": state.checking,
        "staged": staged,
        "downloading": state.downloading.map(|(done, total)| json!({"done": done, "total": total})),
        "download_failed": state.download_failed,
        "restart": state.when_idle.then_some("idle"),
        "refused": REFUSED.get(),
        "stops": stops(daemon),
    })
}

/// The sessions a restart stops: each whose agent, or one of its sub-agents, has a process,
/// whatever its state (a session waiting for its user keeps its process until it is released).
fn stops(daemon: &Daemon) -> Vec<Value> {
    let alive = daemon.agents.with_process();
    if alive.is_empty() {
        return Vec::new();
    }
    let Ok(agents) = daemon.read(Store::agents) else {
        return Vec::new();
    };
    let by_id: std::collections::HashMap<&str, &flottille_core::agent::Agent> = agents
        .iter()
        .map(|agent| (agent.id.as_str(), agent))
        .collect();
    let mut roots: Vec<&flottille_core::agent::Agent> = Vec::new();
    for id in &alive {
        let mut agent = by_id.get(id.as_str()).copied();
        while let Some(parent) = agent.and_then(|a| a.parent.as_deref()) {
            match by_id.get(parent) {
                Some(found) => agent = Some(found),
                None => break,
            }
        }
        if let Some(root) = agent.filter(|root| !roots.iter().any(|r| r.id == root.id)) {
            roots.push(root);
        }
    }
    roots.sort_by_key(|root| root.created_at);
    roots
        .iter()
        .map(|root| json!({"id": root.id, "title": root.title, "project": root.project}))
        .collect()
}

fn changed(daemon: &Daemon) {
    if let Err(error) = daemon.emit("update.changed", shown(daemon)) {
        eprintln!("flottille daemon: update.changed was not stored: {error}");
    }
}

/// Checks at the start, then every `EVERY`, while `updates.toml` does not say never; ends with
/// the daemon.
pub async fn watch(daemon: Arc<Daemon>, mut stopping: watch::Receiver<bool>) {
    loop {
        if checks_automatically(&daemon.paths).unwrap_or(false) {
            check(&daemon).await;
        }
        tokio::select! {
            _ = tokio::time::sleep(EVERY) => {}
            _ = stopping.wait_for(|stopping| *stopping) => return,
        }
    }
}

/// One check of the channel, off the daemon's workers. A result is kept only for the channel
/// still chosen when it ends: when another was chosen meanwhile, it is dropped, and the chosen
/// one is checked in its turn (a check that runs is not told to stop: it is not restarted, it is
/// followed by another, which the choice's own request found running and left to this loop).
async fn check(daemon: &Arc<Daemon>) {
    {
        let mut state = daemon.updates.state.lock().unwrap();
        if state.checking {
            return;
        }
        state.checking = true;
    }
    changed(daemon);
    loop {
        let paths = daemon.paths.clone();
        let followed = update::followed_now(&paths);
        let found = tokio::task::spawn_blocking(move || {
            update::checked(&paths).map_err(|error| error.to_string())
        })
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
        {
            let mut state = daemon.updates.state.lock().unwrap();
            // Compared under the lock the choice's reset takes: no result of a channel left is
            // stored after the reset, and none is shown.
            if update::followed_now(&daemon.paths) != followed {
                continue;
            }
            state.checking = false;
            match found {
                Ok(found) => {
                    state.latest = found.latest.map(|version| version.to_string());
                    state.ahead_of = found.ahead_of.map(|version| version.to_string());
                    state.failed = None;
                }
                Err(why) => {
                    if state.failed.is_none() {
                        eprintln!("flottille daemon: the check for an update failed: {why}");
                    }
                    state.failed = Some(why);
                }
            }
        }
        break;
    }
    changed(daemon);
}

/// Downloads and stages the latest build, off the daemon's workers, its progress told.
async fn download(daemon: Arc<Daemon>) {
    let paths = daemon.paths.clone();
    let telling = daemon.clone();
    let told = Mutex::new(std::time::Instant::now());
    let staged = tokio::task::spawn_blocking(move || {
        update::stage(
            &paths,
            &|done, total| {
                telling.updates.state.lock().unwrap().downloading = Some((done, total));
                let mut told = told.lock().unwrap();
                if told.elapsed() >= PROGRESS_EVERY {
                    *told = std::time::Instant::now();
                    changed(&telling);
                }
            },
            &|| telling.updates.state.lock().unwrap().cancelling,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .unwrap_or_else(|error| Err(error.to_string()));
    {
        let mut state = daemon.updates.state.lock().unwrap();
        state.downloading = None;
        // A download the user cancelled did not fail, whatever stopped it.
        state.download_failed = staged.err().filter(|_| !state.cancelling);
        state.cancelling = false;
    }
    changed(&daemon);
}

fn running(daemon: &Daemon) -> Result<usize, Failure> {
    let agents = daemon
        .read(Store::agents)
        .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    Ok(agents
        .iter()
        .filter(|agent| matches!(agent.state, AgentState::Starting | AgentState::Working))
        .count())
}

/// Stops the daemon to start it again from its binary, which applies what is staged.
fn restart(daemon: &Daemon) {
    eprintln!("flottille daemon: restarting to apply the build staged");
    RESTART.store(true, Ordering::SeqCst);
    daemon.stop();
}

/// Restarts once no agent runs, unless the daemon stops first.
async fn when_idle(daemon: Arc<Daemon>, mut stopping: watch::Receiver<bool>) {
    loop {
        if running(&daemon).is_ok_and(|running| running == 0) {
            restart(&daemon);
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(IDLE_POLL) => {}
            _ = stopping.wait_for(|stopping| *stopping) => return,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    /// `automatically` or `never`.
    check: Option<String>,
    /// `stable` or `beta`: the others are `flottille update --channel`'s, alpha with its branch.
    channel: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Restart {
    when: String,
}

/// Answers an `update.*` request of the user: `update.show`, `update.set`, `update.check`,
/// `update.download`, `update.cancel`, `update.restart`. No tool of an agent reaches them.
pub fn call(daemon: &Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "update.show" => {}
        "update.set" => {
            let Set { check, channel } = serde_json::from_value(params)?;
            if check.is_none() && channel.is_none() {
                return Err(Failure::new(
                    "invalid-params",
                    "set the check or the channel",
                ));
            }
            if check
                .as_deref()
                .is_some_and(|check| !matches!(check, "automatically" | "never"))
            {
                return Err(Failure::new(
                    "invalid-params",
                    "check is \"automatically\" or \"never\"",
                ));
            }
            if channel
                .as_deref()
                .is_some_and(|channel| !matches!(channel, "stable" | "beta"))
            {
                return Err(Failure::new(
                    "invalid-params",
                    "channel is \"stable\" or \"beta\": an alpha is chosen with \
                     `flottille update --channel alpha --branch <name>`",
                ));
            }
            let paths = &daemon.paths;
            if let Some(check) = &check {
                std::fs::create_dir_all(&paths.config).map_err(|e| {
                    Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
                })?;
                crate::providers::write_in_place(
                    &paths.config.join("updates.toml"),
                    &format!("check = \"{check}\"\n"),
                )
                .map_err(|e| Failure::new("unavailable", format!("updates.toml: {e}")))?;
            }
            if let Some(channel) = &channel {
                update::choose(paths, channel)
                    .map_err(|e| Failure::new("unavailable", format!("the channel: {e}")))?;
                // What was found belongs to the channel left: the check of the new one says it.
                let mut state = daemon.updates.state.lock().unwrap();
                state.latest = None;
                state.ahead_of = None;
                state.failed = None;
            }
            // A channel chosen is checked at once, the check's setting aside.
            if channel.is_some() || check.as_deref() == Some("automatically") {
                let checking = daemon.clone();
                tokio::spawn(async move { self::check(&checking).await });
            }
            changed(daemon);
        }
        "update.check" => {
            let checking = daemon.clone();
            tokio::spawn(async move { self::check(&checking).await });
        }
        "update.download" => {
            {
                let mut state = daemon.updates.state.lock().unwrap();
                if state.downloading.is_some() {
                    return Ok(shown_unlocked(state, daemon));
                }
                state.downloading = Some((0, None));
                state.cancelling = false;
                state.download_failed = None;
            }
            changed(daemon);
            tokio::spawn(download(daemon.clone()));
        }
        "update.cancel" => {
            {
                let mut state = daemon.updates.state.lock().unwrap();
                // Nothing to cancel is remembered for no download to come.
                state.cancelling = state.downloading.is_some();
            }
            changed(daemon);
        }
        "update.restart" => {
            let Restart { when } = serde_json::from_value(params)?;
            if binary()
                .and_then(|binary| update::staged(&binary))
                .is_none()
            {
                return Err(Failure::new(
                    "nothing-staged",
                    "no build is staged: Download it first",
                ));
            }
            match when.as_str() {
                "now" => restart(daemon),
                "idle" => {
                    let already = std::mem::replace(
                        &mut daemon.updates.state.lock().unwrap().when_idle,
                        true,
                    );
                    if !already {
                        tokio::spawn(when_idle(daemon.clone(), daemon.stopping()));
                    }
                    changed(daemon);
                }
                _ => {
                    return Err(Failure::new(
                        "invalid-params",
                        "when is \"now\" or \"idle\"",
                    ));
                }
            }
        }
        method => {
            return Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            ));
        }
    }
    Ok(shown(daemon))
}

/// `shown`, the state's lock let go first.
fn shown_unlocked(state: std::sync::MutexGuard<'_, State>, daemon: &Daemon) -> Value {
    drop(state);
    shown(daemon)
}
