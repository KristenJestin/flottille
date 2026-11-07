//! Flottille's own compaction (`docs/design.md` §5.8): the compactor, a model the user chose as
//! they choose the Auto judge, summarises an agent whose context passed the threshold, at the end
//! of a turn or when the user asks; the agent then goes on in a new session of its CLI, as a
//! replaced agent does, from its brief, the summary and its latest messages word for word.
//!
//! Its settings are `compaction.toml` of Flottille's configuration folder, its guidance
//! `compaction.md` beside it (`docs/protocol.md`, "The compaction methods"). A compaction that
//! fails changes nothing: the agent goes on in its session, and its CLI's own compaction stays.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::agent::AgentState;
use flottille_core::paths::Paths;
use flottille_core::protocol::Event;
use serde_json::{Value, json};

use crate::agents::{self, Order};
use crate::daemon::Daemon;
use crate::judge::{self, Model};
use crate::projects::Failure;
use crate::providers;
use crate::tree;

/// The compactor's fixed instructions: what it is, what it answers, and that what the agent wrote
/// is data. The user's guidance takes the place of `{guidance}`.
pub const FIXED: &str = include_str!("../roles/compactor-base.md");

/// The guidance the compactor follows until the user writes their own.
pub const DEFAULT: &str = include_str!("../roles/compaction.md");

/// The longest guidance, in characters.
pub const LIMIT: usize = 4_000;

/// The share of the context window past which an agent is compacted, until the user says another.
pub const THRESHOLD: u64 = 70;
/// The thresholds the user may choose, in percent.
pub const THRESHOLDS: std::ops::RangeInclusive<u64> = 10..=95;
/// The tokens of the latest messages given again word for word, until the user says another.
pub const KEPT: u64 = 20_000;
/// The most the user may keep word for word.
pub const KEPT_MAX: u64 = 200_000;
/// The smallest maximum size, in tokens.
pub const MAXIMUM_MIN: u64 = 1_000;
/// A tool result in the compactor's transcript is cut to this many characters.
const RESULT_CHARS: usize = 500;
/// Characters per token, as Flottille counts the size of a text it writes.
const CHARS_PER_TOKEN: usize = 4;

/// What the comment above a compactor Flottille picked says.
const PICKED: &str = "# Picked by Flottille as the compactor: the smallest model of the first \
                      provider installed and signed in.\n# Change it in Settings, or choose none \
                      there (compactor = \"none\").\n";

/// The model that writes the summaries, as `[compactor]` holds it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Compactor {
    pub provider: String,
    /// The provider's default model when none is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model's default effort when none is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Flottille picked it, and the user has not yet seen the notice that says so.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub picked: bool,
}

/// `compaction.toml`.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// None while the user chose none, or never chose and none could be picked.
    pub compactor: Option<Compactor>,
    /// Whether the file says anything of the compactor, one or `compactor = "none"`: Flottille
    /// picks one only while it does not.
    pub compactor_set: bool,
    pub automatic: bool,
    /// Percent of the model's context window.
    pub threshold: u64,
    /// A ceiling in tokens; none is no ceiling.
    pub maximum: Option<u64>,
    /// Tokens of the latest messages given again word for word.
    pub kept: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            compactor: None,
            compactor_set: false,
            automatic: true,
            threshold: THRESHOLD,
            maximum: None,
            kept: KEPT,
        }
    }
}

fn file(paths: &Paths) -> std::path::PathBuf {
    paths.config.join("compaction.toml")
}

fn guidance_file(paths: &Paths) -> std::path::PathBuf {
    paths.config.join("compaction.md")
}

impl Settings {
    /// Reads `compaction.toml`: the defaults when there is none.
    pub fn load(paths: &Paths) -> Result<Settings, String> {
        match std::fs::read_to_string(file(paths)) {
            Ok(text) => Settings::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(error) => Err(format!("compaction.toml: {error}")),
        }
    }

    /// The text of `compaction.toml`, checked: an unknown key or a value out of its range is
    /// refused, never ignored.
    pub fn parse(text: &str) -> Result<Settings, String> {
        let refuse = |message: String| format!("compaction.toml: {message}");
        let document: toml_edit::DocumentMut = text
            .parse()
            .map_err(|e: toml_edit::TomlError| refuse(e.message().into()))?;
        let number = |item: &toml_edit::Item| item.as_integer().and_then(|n| u64::try_from(n).ok());
        let mut settings = Settings::default();
        for (key, item) in document.iter() {
            match key {
                "compactor" => {
                    settings.compactor_set = true;
                    if item.as_str() == Some("none") {
                        continue;
                    }
                    let table = item
                        .as_table_like()
                        .ok_or_else(|| refuse("compactor is a table, or \"none\"".into()))?;
                    let mut compactor = Compactor {
                        provider: String::new(),
                        model: None,
                        effort: None,
                        picked: false,
                    };
                    for (key, item) in table.iter() {
                        let text = || {
                            item.as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| refuse(format!("compactor.{key} is a text")))
                        };
                        match key {
                            "provider" => compactor.provider = text()?,
                            "model" => compactor.model = Some(text()?),
                            "effort" => compactor.effort = Some(text()?),
                            "picked" => {
                                compactor.picked = item.as_bool().ok_or_else(|| {
                                    refuse("compactor.picked is true or false".into())
                                })?
                            }
                            key => return Err(refuse(format!("unknown key `compactor.{key}`"))),
                        }
                    }
                    if providers::get(&compactor.provider).is_none() {
                        return Err(refuse(format!(
                            "compactor.provider: unknown provider `{}`",
                            compactor.provider
                        )));
                    }
                    settings.compactor = Some(compactor);
                }
                "automatic" => {
                    settings.automatic = item
                        .as_bool()
                        .ok_or_else(|| refuse("automatic is true or false".into()))?;
                }
                "threshold" => {
                    settings.threshold = number(item)
                        .filter(|n| THRESHOLDS.contains(n))
                        .ok_or_else(|| refuse(threshold_range()))?;
                }
                "maximum" => {
                    settings.maximum = Some(
                        number(item)
                            .filter(|n| *n >= MAXIMUM_MIN)
                            .ok_or_else(|| refuse(maximum_range()))?,
                    );
                }
                "kept" => {
                    settings.kept = number(item)
                        .filter(|n| *n <= KEPT_MAX)
                        .ok_or_else(|| refuse(kept_range()))?;
                }
                key => return Err(refuse(format!("unknown key `{key}`"))),
            }
        }
        Ok(settings)
    }
}

fn threshold_range() -> String {
    format!(
        "threshold is a percent of the context window from {} to {}",
        THRESHOLDS.start(),
        THRESHOLDS.end()
    )
}

fn maximum_range() -> String {
    format!("maximum is a number of tokens from {MAXIMUM_MIN}, or \"none\" for no ceiling")
}

fn kept_range() -> String {
    format!("kept is a number of tokens from 0 to {KEPT_MAX}")
}

/// Whether the user chose a compactor: then each CLI is set to compact as late as it allows, so
/// that its own compaction only acts when Flottille's could not (`docs/agents.md`). One not yet
/// picked counts as none.
pub fn chosen(paths: &Paths) -> bool {
    Settings::load(paths).is_ok_and(|settings| settings.compactor.is_some())
}

/// Held while `compaction.toml` or `compaction.md` is written: two edits at once leave one whole.
static WRITING: Mutex<()> = Mutex::new(());

/// Changes `compaction.toml` with `edit`, given the settings it held: written when `edit` says it
/// changed it, after the text is read back as settings. Whether it changed.
fn edit(
    paths: &Paths,
    edit: impl FnOnce(&Settings, &mut toml_edit::DocumentMut) -> Result<bool, Failure>,
) -> Result<bool, Failure> {
    let _writing = WRITING.lock().unwrap();
    let file = file(paths);
    let unavailable =
        |error: std::io::Error| Failure::new("unavailable", format!("compaction.toml: {error}"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(unavailable(error)),
    };
    let before = Settings::parse(&text).map_err(|error| Failure::new("invalid-config", error))?;
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            Failure::new(
                "invalid-config",
                format!("compaction.toml: {}", error.message()),
            )
        })?;
    if !edit(&before, &mut document)? {
        return Ok(false);
    }
    let text = document.to_string();
    Settings::parse(&text).map_err(|error| Failure::new("invalid-config", error))?;
    std::fs::create_dir_all(&paths.config).map_err(unavailable)?;
    providers::write_in_place(&file, &text).map_err(unavailable)?;
    Ok(true)
}

/// `[compactor]` as `compaction.toml` holds it.
fn compactor_table(compactor: &Compactor) -> toml_edit::Item {
    let mut table = toml_edit::Table::new();
    table.insert("provider", toml_edit::value(compactor.provider.as_str()));
    if let Some(model) = &compactor.model {
        table.insert("model", toml_edit::value(model.as_str()));
    }
    if let Some(effort) = &compactor.effort {
        table.insert("effort", toml_edit::value(effort.as_str()));
    }
    if compactor.picked {
        table.insert("picked", toml_edit::value(true));
    }
    toml_edit::Item::Table(table)
}

/// The settings now, a compactor picked first for a user who never chose one (as the Auto judge
/// is, `providers::smallest_ready`): written once, marked `picked` until the user saw the notice,
/// and said once with `compaction.changed`.
pub fn settings(daemon: &Daemon) -> Result<Settings, Failure> {
    let settings =
        Settings::load(&daemon.paths).map_err(|error| Failure::new("invalid-config", error))?;
    if settings.compactor_set {
        return Ok(settings);
    }
    let providers = flottille_core::provider::Settings::load(&daemon.paths).unwrap_or_default();
    let Some((provider, model)) = providers::smallest_ready(&providers) else {
        return Ok(settings);
    };
    let compactor = Compactor {
        provider: provider.to_owned(),
        model: Some(model.to_owned()),
        effort: None,
        picked: true,
    };
    let written = edit(&daemon.paths, |before, document| {
        if before.compactor_set {
            return Ok(false);
        }
        let mut table = compactor_table(&compactor);
        if let Some(table) = table.as_table_mut() {
            let blank = if document.is_empty() { "" } else { "\n" };
            table.decor_mut().set_prefix(format!("{blank}{PICKED}"));
        }
        document.insert("compactor", table);
        Ok(true)
    })?;
    if written {
        announce(daemon)?;
    }
    Settings::load(&daemon.paths).map_err(|error| Failure::new("invalid-config", error))
}

/// The user's guidance, if any: none when it is blank. A file that cannot be read whole, or holds
/// more than the limit, is an error with what of it could be read: a compaction then fails rather
/// than follow guidance the user did not write.
fn own(paths: &Paths) -> Result<Option<String>, (String, String)> {
    let file = guidance_file(paths);
    let text = match std::fs::read(&file) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| {
            (
                String::new(),
                format!(
                    "your compactor's guidance ({}) is not UTF-8 text: write it again, or reset it",
                    file.display()
                ),
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err((
                String::new(),
                format!(
                    "your compactor's guidance ({}) cannot be read: {error}",
                    file.display()
                ),
            ));
        }
    };
    let count = text.chars().count();
    if count > LIMIT {
        let why = format!(
            "your compactor's guidance ({}) is {count} characters long and may hold {LIMIT}: \
             shorten it, or reset it",
            file.display()
        );
        return Err((text, why));
    }
    Ok(Some(text).filter(|text| !text.trim().is_empty()))
}

/// The compactor's instructions with the guidance read now.
fn instructions(paths: &Paths) -> Result<String, String> {
    let guidance = own(paths)
        .map_err(|(_, why)| why)?
        .unwrap_or_else(|| DEFAULT.to_owned());
    let (before, after) = FIXED
        .split_once("{guidance}")
        .expect("the compactor's fixed instructions hold the guidance's place");
    Ok(format!("{before}{}{after}", guidance.trim()))
}

fn announce(daemon: &Daemon) -> Result<(), Failure> {
    let shown = show(daemon)?;
    daemon
        .emit("compaction.changed", shown)
        .map(drop)
        .map_err(|error| Failure::new("unavailable", error.to_string()))
}

/// `compaction.show`'s answer, without picking.
fn show(daemon: &Daemon) -> Result<Value, Failure> {
    let settings =
        Settings::load(&daemon.paths).map_err(|error| Failure::new("invalid-config", error))?;
    let (text, source, error) = match own(&daemon.paths) {
        Ok(Some(text)) => (text, "global", None),
        Ok(None) => (DEFAULT.to_owned(), "default", None),
        Err((text, why)) => (text, "global", Some(why)),
    };
    Ok(json!({
        "compactor": settings.compactor,
        "automatic": settings.automatic,
        "threshold": settings.threshold,
        "maximum": settings.maximum,
        "kept": settings.kept,
        "guidance": {"text": text, "source": source, "error": error, "default": DEFAULT},
    }))
}

/// Answers a `compaction.*` request of the user. No tool of an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    let changed = match method {
        "compaction.show" => {
            settings(daemon)?;
            false
        }
        "compaction.set" => {
            let asked = Asked::of(&params)?;
            edit(paths, |before, document| {
                let mut changed = false;
                if let Some(compactor) = asked.compactor
                    && (!before.compactor_set || before.compactor != compactor)
                {
                    document.remove("compactor");
                    match &compactor {
                        Some(compactor) => document.insert("compactor", compactor_table(compactor)),
                        None => document.insert("compactor", toml_edit::value("none")),
                    };
                    changed = true;
                }
                let mut set = |key: &str, value: Option<toml_edit::Item>| {
                    let item = document.remove(key);
                    match value {
                        Some(value) => {
                            let same = item.as_ref().map(|i| i.to_string().trim().to_owned())
                                == Some(value.to_string().trim().to_owned());
                            document.insert(key, value);
                            changed |= !same;
                        }
                        None => changed |= item.is_some(),
                    }
                };
                if let Some(automatic) = asked.automatic {
                    set("automatic", Some(toml_edit::value(automatic)));
                }
                if let Some(threshold) = asked.threshold {
                    set("threshold", Some(toml_edit::value(threshold as i64)));
                }
                if let Some(maximum) = asked.maximum {
                    set("maximum", maximum.map(|m| toml_edit::value(m as i64)));
                }
                if let Some(kept) = asked.kept {
                    set("kept", Some(toml_edit::value(kept as i64)));
                }
                // The tables last, whatever order the keys came in.
                if let Some(compactor) = document.remove("compactor") {
                    document.insert("compactor", compactor);
                }
                Ok(changed)
            })?
        }
        "compaction.set_guidance" | "compaction.reset_guidance" => {
            let text = match method {
                "compaction.set_guidance" => {
                    let text = params["text"].as_str().ok_or_else(|| {
                        Failure::new("invalid-params", "text is the guidance, a text")
                    })?;
                    let count = text.chars().count();
                    if count > LIMIT {
                        return Err(Failure::refused(format!(
                            "the compactor's guidance is {count} characters long and may hold \
                             {LIMIT}"
                        )));
                    }
                    Some(text.to_owned()).filter(|text| !text.trim().is_empty())
                }
                _ => None,
            };
            let _writing = WRITING.lock().unwrap();
            match text {
                Some(text) => {
                    std::fs::create_dir_all(&paths.config).map_err(|e| {
                        Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
                    })?;
                    providers::write_in_place(&guidance_file(paths), &text)
                        .map_err(|e| Failure::new("unavailable", format!("compaction.md: {e}")))?;
                }
                None => match std::fs::remove_file(guidance_file(paths)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(Failure::new("unavailable", format!("compaction.md: {e}")));
                    }
                    _ => {}
                },
            }
            true
        }
        "compaction.noticed" => edit(paths, |before, document| {
            if !before.compactor.as_ref().is_some_and(|c| c.picked) {
                return Ok(false);
            }
            Ok(document["compactor"]
                .as_table_like_mut()
                .is_some_and(|table| table.remove("picked").is_some()))
        })?,
        method => {
            return Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            ));
        }
    };
    if changed {
        announce(daemon)?;
    }
    show(daemon)
}

/// What `compaction.set` asks: each field left out stays as it is. `None` inside is a choice of
/// none (`"none"`).
struct Asked {
    compactor: Option<Option<Compactor>>,
    automatic: Option<bool>,
    threshold: Option<u64>,
    maximum: Option<Option<u64>>,
    kept: Option<u64>,
}

impl Asked {
    fn of(params: &Value) -> Result<Asked, Failure> {
        let wrong = |why: String| Failure::new("invalid-params", why);
        let Some(params) = params.as_object() else {
            return Err(wrong("the parameters are an object".into()));
        };
        let mut asked = Asked {
            compactor: None,
            automatic: None,
            threshold: None,
            maximum: None,
            kept: None,
        };
        for (key, value) in params {
            match key.as_str() {
                "compactor" if value == "none" => asked.compactor = Some(None),
                "compactor" => {
                    #[derive(serde::Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Chosen {
                        provider: String,
                        #[serde(default)]
                        model: Option<String>,
                        #[serde(default)]
                        effort: Option<String>,
                    }
                    let chosen: Chosen = serde_json::from_value(value.clone()).map_err(|e| {
                        wrong(format!(
                            "compactor is {{provider, model, effort}} or \"none\": {e}"
                        ))
                    })?;
                    let provider = providers::get(&chosen.provider).ok_or_else(|| {
                        Failure::new(
                            "not-found",
                            format!("there is no provider {}", chosen.provider),
                        )
                    })?;
                    let blank = |text: Option<String>| text.filter(|t| !t.trim().is_empty());
                    asked.compactor = Some(Some(Compactor {
                        provider: provider.name.to_owned(),
                        model: blank(chosen.model),
                        effort: blank(chosen.effort),
                        picked: false,
                    }));
                }
                "automatic" => {
                    asked.automatic = Some(
                        value
                            .as_bool()
                            .ok_or_else(|| wrong("automatic is true or false".into()))?,
                    );
                }
                "threshold" => {
                    asked.threshold = Some(
                        value
                            .as_u64()
                            .filter(|n| THRESHOLDS.contains(n))
                            .ok_or_else(|| wrong(threshold_range()))?,
                    );
                }
                "maximum" if value == "none" => asked.maximum = Some(None),
                "maximum" => {
                    asked.maximum = Some(Some(
                        value
                            .as_u64()
                            .filter(|n| *n >= MAXIMUM_MIN)
                            .ok_or_else(|| wrong(maximum_range()))?,
                    ));
                }
                "kept" => {
                    asked.kept = Some(
                        value
                            .as_u64()
                            .filter(|n| *n <= KEPT_MAX)
                            .ok_or_else(|| wrong(kept_range()))?,
                    );
                }
                key => return Err(wrong(format!("unknown field `{key}`"))),
            }
        }
        Ok(asked)
    }
}

/// Where each agent's compaction stands.
#[derive(Default)]
pub struct Live {
    phases: Mutex<HashMap<String, Phase>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Asked by hand during a turn: it starts once the turn ended.
    Asked,
    /// The compactor works: the agent's prompts wait for its new session.
    Running,
}

/// Whether the agent's prompts wait: its compaction is under way.
pub fn holding(daemon: &Daemon, agent: &str) -> bool {
    daemon.compactions.phases.lock().unwrap().get(agent) == Some(&Phase::Running)
}

/// The compaction is over, done or failed: what waited for it goes to the agent.
pub fn finish(daemon: &Arc<Daemon>, agent: &str) {
    daemon.compactions.phases.lock().unwrap().remove(agent);
    agents::order(daemon, agent, Order::Ring);
}

/// `agent.compact`: the user asks for a compaction now, or at the end of the turn that runs.
/// Whether it waits for the turn's end.
pub fn ask(daemon: &Arc<Daemon>, agent: &flottille_core::agent::Agent) -> Result<bool, Failure> {
    if settings(daemon)?.compactor.is_none() {
        return Err(Failure::refused(
            "there is no compactor: choose one in Settings, Compaction, and the agent's CLI \
             compacts on its own until then",
        ));
    }
    let in_turn = agent.state == AgentState::Working;
    let mut phases = daemon.compactions.phases.lock().unwrap();
    if phases.contains_key(&agent.id) {
        return Ok(in_turn);
    }
    if in_turn {
        phases.insert(agent.id.clone(), Phase::Asked);
        return Ok(true);
    }
    phases.insert(agent.id.clone(), Phase::Running);
    drop(phases);
    tokio::spawn(run(daemon.clone(), agent.id.clone()));
    Ok(false)
}

/// The agent's turn ended: its compaction starts when the user asked for one during the turn, or
/// when its context passed the threshold or the maximum size and the compaction is automatic.
pub fn turn_ended(daemon: &Arc<Daemon>, agent: &str) {
    let asked = daemon
        .compactions
        .phases
        .lock()
        .unwrap()
        .get(agent)
        .copied();
    if asked == Some(Phase::Running) {
        return;
    }
    if asked.is_none() {
        let Ok(settings) = settings(daemon) else {
            return;
        };
        if settings.compactor.is_none() || !settings.automatic {
            return;
        }
        let Some((used, size)) = usage(daemon, agent) else {
            return;
        };
        let past_threshold = size.is_some_and(|size| used * 100 >= size * settings.threshold);
        let past_maximum = settings.maximum.is_some_and(|maximum| used >= maximum);
        if !past_threshold && !past_maximum {
            return;
        }
    }
    daemon
        .compactions
        .phases
        .lock()
        .unwrap()
        .insert(agent.to_owned(), Phase::Running);
    tokio::spawn(run(daemon.clone(), agent.to_owned()));
}

/// The context the agent used at its last report, and its window when known. A report made
/// before its last compaction is the old session's: none.
fn usage(daemon: &Daemon, agent: &str) -> Option<(u64, Option<u64>)> {
    let last = |name| {
        daemon
            .read(|store| store.last_agent_events(agent, name, 1))
            .ok()
            .and_then(|events| events.into_iter().next())
    };
    let report = last("agent.usage")?;
    if last("agent.compacted").is_some_and(|compacted| compacted.seq > report.seq) {
        return None;
    }
    Some((report.data["used"].as_u64()?, report.data["size"].as_u64()))
}

/// How long the compactor may take before its compaction is taken for a failure.
fn timeout() -> Duration {
    std::env::var("FLOTTILLE_COMPACTOR_TIMEOUT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(10 * 60))
}

/// A summary written and checked: what the agent's task needs to start its new session.
pub struct Compacted {
    pub summary: String,
    /// The context in use before, in tokens.
    pub before: u64,
}

/// One compaction of the agent, in the background: the summary, then the new session through
/// the agent's task. A failure is said and changes nothing.
async fn run(daemon: Arc<Daemon>, agent: String) {
    let started = std::time::Instant::now();
    let outcome = match tokio::time::timeout(timeout(), compact(&daemon, &agent)).await {
        Ok(outcome) => outcome,
        Err(_) => Err(format!(
            "the compactor did not answer within {} seconds",
            timeout().as_secs()
        )),
    };
    match outcome {
        Ok(compacted) => {
            daemon.developer.write(
                "compaction",
                json!({"agent": agent, "before": compacted.before,
                    "summary_chars": compacted.summary.chars().count(),
                    "duration_ms": started.elapsed().as_millis() as u64}),
                &[],
            );
            agents::order(&daemon, &agent, Order::Compact(Box::new(compacted)));
        }
        Err(reason) => {
            daemon.developer.write(
                "compaction",
                json!({"agent": agent, "failed": reason,
                    "duration_ms": started.elapsed().as_millis() as u64}),
                &[],
            );
            agents::record(
                &daemon,
                &agent,
                "agent.compaction_failed",
                json!({"reason": reason}),
                false,
            );
            finish(&daemon, &agent);
        }
    }
}

/// The summary: the compactor given the transcript since the last compaction, the previous
/// summary and what only Flottille knows, then checked.
async fn compact(daemon: &Arc<Daemon>, agent: &str) -> Result<Compacted, String> {
    let settings = settings(daemon).map_err(|failure| failure.message)?;
    let compactor = settings
        .compactor
        .ok_or("there is no compactor: none was chosen")?;
    let instructions = instructions(&daemon.paths)?;
    let found = daemon
        .read(|store| store.agent(agent))
        .map_err(|e| e.to_string())?
        .ok_or("the agent is gone")?;
    let read = Read::of(daemon, agent)?;
    let transcript = read.transcript();
    let before = usage(daemon, agent).map_or(0, |(used, _)| used);
    let previous = read.previous.as_deref();
    let prompt = prompt(previous, &state(daemon, &found), &transcript);
    let model = Model {
        provider: &compactor.provider,
        model: compactor.model.as_deref(),
        effort: compactor.effort.as_deref(),
    };
    let turned = judge::one_turn(daemon, &model, "compactor", &instructions, &prompt).await?;
    let summary = turned.said.trim().to_owned();
    check(
        &summary,
        &turned.stop,
        turned.tool,
        previous.map_or(0, str::len) + transcript.len(),
    )?;
    Ok(Compacted { summary, before })
}

/// Why a summary is refused, if it is: cut off by its length, a call to a tool, empty, or not
/// smaller than what it replaces (`replaced` characters).
pub fn check(summary: &str, stop: &str, tool: bool, replaced: usize) -> Result<(), String> {
    if stop == "max_tokens" || stop == "max_turn_requests" {
        return Err("the summary was cut off by its length".into());
    }
    if tool {
        return Err("the compactor tried to call a tool, which it does not have".into());
    }
    if summary.is_empty() {
        return Err("the compactor wrote no summary".into());
    }
    if summary.len() >= replaced {
        return Err(format!(
            "the summary ({} characters) is not smaller than what it replaces ({replaced})",
            summary.len()
        ));
    }
    Ok(())
}

/// What the agent's events since Flottille last compacted it say: the summary that compaction
/// wrote, and the conversation since.
struct Read {
    previous: Option<String>,
    /// Each message `(who, text)`, or a line of a tool call (`who` empty), oldest first.
    entries: Vec<(&'static str, String)>,
}

impl Read {
    fn of(daemon: &Daemon, agent: &str) -> Result<Read, String> {
        let mut read = Read {
            previous: None,
            entries: Vec::new(),
        };
        let mut after = 0;
        loop {
            let page: Vec<Event> = daemon
                .read(|store| store.agent_events(agent, after, 1_000))
                .map_err(|e| e.to_string())?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.seq;
            for event in &page {
                read.take(event);
            }
        }
        Ok(read)
    }

    fn take(&mut self, event: &Event) {
        let data = &event.data;
        let text = |key: &str| data[key].as_str().unwrap_or_default().to_owned();
        match event.name.as_str() {
            "agent.compacted" if data["by"] == "flottille" => {
                self.previous = Some(text("summary"));
                self.entries.clear();
            }
            // The new session's first prompt holds the summary and the messages kept: they are
            // the previous summary's, not news.
            "agent.prompted"
                if data["deliveries"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(is_resume)) => {}
            "agent.prompted" => self.entries.push(("to the agent", text("text"))),
            // A message streamed in pieces is one.
            "agent.message" => match self.entries.last_mut() {
                Some((who, said)) if *who == "the agent" => said.push_str(&text("text")),
                _ => self.entries.push(("the agent", text("text"))),
            },
            "agent.tool_call" if data["update"] != true => {
                let input = data
                    .get("raw_input")
                    .map(|input| cut(&input.to_string()))
                    .unwrap_or_default();
                self.entries
                    .push(("", format!("[tool call] {} {input}", text("title"))));
            }
            "agent.tool_call" if data["status"] == "completed" || data["status"] == "failed" => {
                let result = data
                    .get("raw_output")
                    .or(data.get("content"))
                    .map(|output| match output.as_str() {
                        Some(text) => cut(text),
                        None => cut(&output.to_string()),
                    })
                    .unwrap_or_default();
                self.entries
                    .push(("", format!("[tool result, {}] {result}", text("status"))));
            }
            _ => {}
        }
    }

    /// The conversation as text: each message under who said it, each tool call on its line.
    fn transcript(&self) -> String {
        let shown: Vec<String> = self
            .entries
            .iter()
            .map(|(who, text)| match *who {
                "" => text.clone(),
                who => format!("[{who}]\n{text}"),
            })
            .collect();
        shown.join("\n\n")
    }

    /// The messages, without the tool calls: what is kept word for word.
    fn messages(&self) -> impl DoubleEndedIterator<Item = &(&'static str, String)> {
        self.entries.iter().filter(|(who, _)| !who.is_empty())
    }
}

fn is_resume(id: &Value) -> bool {
    id.as_str().is_some_and(|id| id.starts_with("resume-"))
}

/// A tool's input or result as the compactor sees it: its first characters, and how many more.
fn cut(text: &str) -> String {
    let count = text.chars().count();
    if count <= RESULT_CHARS {
        return text.to_owned();
    }
    let kept: String = text.chars().take(RESULT_CHARS).collect();
    format!("{kept}… ({} more characters)", count - RESULT_CHARS)
}

/// What can be written in a block of the prompt: nothing in it closes or opens one.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

/// The compactor's prompt: its instructions are its session's, the data in blocks of their own.
fn prompt(previous: Option<&str>, state: &str, transcript: &str) -> String {
    let previous = match previous {
        Some(previous) => format!(
            "The summary you wrote at the last compaction, to update:\n\n<previous_summary>\n{}\n</previous_summary>",
            escaped(previous)
        ),
        None => "There is no previous summary: this is the agent's first compaction.".to_owned(),
    };
    format!(
        "{previous}\n\nWhat Flottille holds of the agent now:\n\n<flottille_state>\n{}\n</flottille_state>\n\n\
         The conversation since, as Flottille recorded it (thinking left out, each tool's input and \
         result cut short):\n\n<transcript>\n{}\n</transcript>\n\nWrite the summary now.",
        escaped(state),
        escaped(transcript.trim_end()),
    )
}

/// What only Flottille knows of the agent: its brief, its place, its children, its subscriptions,
/// its requests and the messages waiting for it.
fn state(daemon: &Daemon, agent: &flottille_core::agent::Agent) -> String {
    let mut lines = Vec::new();
    let (_, brief) = tree::first_message(daemon, agent);
    lines.push(format!("Brief:\n{brief}"));
    lines.push(format!(
        "Place: {} ({})",
        agent.workspace.as_deref().unwrap_or("the project folder"),
        agent.folder.display()
    ));
    if let Some(workspace) = &agent.workspace
        && let Ok(Some(found)) = daemon.read(|store| store.workspace(&agent.project, workspace))
    {
        lines.push(format!("Workspace branch: {}", found.branch));
    }
    if let Ok(tree) = tree::tree(daemon, agent) {
        let children: Vec<String> = tree
            .iter()
            .filter(|node| node["parent"] == agent.id.as_str())
            .map(|node| {
                format!(
                    "- {} \"{}\": {}",
                    node["id"].as_str().unwrap_or_default(),
                    node["title"].as_str().unwrap_or_default(),
                    node["state"].as_str().unwrap_or_default()
                )
            })
            .collect();
        if !children.is_empty() {
            lines.push(format!("Children:\n{}", children.join("\n")));
        }
    }
    if let Ok(listed) =
        crate::subscriptions::call(daemon, "subscription.list", json!({"tree": agent.id}))
    {
        let mine: Vec<String> = listed["subscriptions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["agent"] == agent.id.as_str())
            .map(|s| format!("- {s}"))
            .collect();
        if !mine.is_empty() {
            lines.push(format!("Subscriptions:\n{}", mine.join("\n")));
        }
    }
    if let Ok(queued) = daemon.read(|store| store.deliveries(&agent.id, "queued")) {
        let waiting: Vec<String> = queued
            .iter()
            .map(|d| format!("- {}: {}", d.kind, cut(&d.body)))
            .collect();
        if !waiting.is_empty() {
            lines.push(format!("Messages waiting for it:\n{}", waiting.join("\n")));
        }
    }
    lines.join("\n\n")
}

/// What the new session is sent after the brief: the summary, then the latest messages word for
/// word, oldest first, as many as fit in `kept` tokens. Its size in tokens, with the brief's.
pub fn resumed(daemon: &Daemon, agent: &str, summary: &str, kept: u64) -> (String, u64) {
    let read = Read::of(daemon, agent).unwrap_or(Read {
        previous: None,
        entries: Vec::new(),
    });
    let room = kept as usize * CHARS_PER_TOKEN;
    let mut used = 0;
    let mut latest = Vec::new();
    for (who, text) in read.messages().rev() {
        used += text.len();
        if used > room {
            break;
        }
        latest.push(format!("[{who}]\n{text}"));
    }
    latest.reverse();
    let mut body = format!(
        "Your conversation was compacted by Flottille: this new session starts from your brief \
         (above), a summary of what came before, and your latest messages as they were.\n\n\
         Summary:\n\n{summary}"
    );
    if !latest.is_empty() {
        body.push_str("\n\nYour latest messages, word for word, oldest first:\n\n");
        body.push_str(&latest.join("\n\n"));
    }
    body.push_str(
        "\n\nGo on where you stopped. Check the real state of your place before you act again.",
    );
    let brief = daemon
        .read(|store| store.agent(agent))
        .ok()
        .flatten()
        .and_then(|agent| agent.brief)
        .map_or(0, |brief| brief.len());
    let after = ((brief + body.len()) / CHARS_PER_TOKEN) as u64;
    (body, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_cut_off_calling_a_tool_empty_or_not_smaller_is_refused() {
        assert!(check("short", "end_turn", false, 100).is_ok());
        assert!(
            check("short", "max_tokens", false, 100)
                .unwrap_err()
                .contains("cut off")
        );
        assert!(
            check("short", "end_turn", true, 100)
                .unwrap_err()
                .contains("tool")
        );
        assert!(check("", "end_turn", false, 100).is_err());
        assert!(
            check(&"x".repeat(100), "end_turn", false, 100)
                .unwrap_err()
                .contains("not smaller")
        );
    }

    #[test]
    fn the_settings_are_read_with_their_defaults_and_refused_out_of_range() {
        let settings = Settings::parse("").unwrap();
        assert_eq!(settings, Settings::default());
        assert!(settings.automatic);
        assert_eq!(settings.threshold, 70);
        assert_eq!(settings.kept, 20_000);
        let none = Settings::parse("compactor = \"none\"\n").unwrap();
        assert!(none.compactor_set && none.compactor.is_none());
        let chosen = Settings::parse(
            "automatic = false\nthreshold = 80\nmaximum = 150000\n[compactor]\nprovider = \"claude\"\nmodel = \"haiku\"\neffort = \"low\"\n",
        )
        .unwrap();
        assert_eq!(chosen.maximum, Some(150_000));
        assert_eq!(chosen.compactor.unwrap().effort.as_deref(), Some("low"));
        for wrong in [
            "threshold = 100\n",
            "kept = -1\n",
            "who = 1\n",
            "[compactor]\nprovider = \"x\"\n",
        ] {
            assert!(Settings::parse(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn the_guidance_sits_inside_the_fixed_text() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            runtime: dir.path().join("runtime"),
            state: dir.path().join("state"),
            config: dir.path().join("config"),
            data: dir.path().join("data"),
        };
        let text = instructions(&paths).unwrap();
        assert!(text.starts_with("You are Flottille's compactor."));
        assert!(text.contains("**Goal**"));
        assert!(!text.contains("{guidance}"));
    }

    #[test]
    fn a_tool_s_result_is_cut_short_and_says_how_much_was_left_out() {
        assert_eq!(cut("abc"), "abc");
        let long = "y".repeat(RESULT_CHARS + 7);
        assert!(cut(&long).ends_with("(7 more characters)"));
    }
}
