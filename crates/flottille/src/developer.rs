//! The developer mode (`docs/developer.md`): while it is on, the daemon appends what it decides
//! and why to a log in its state folder, one JSON object per line, every text masked. Off, it
//! costs one atomic read where a line would be written, and nothing is put together to be dropped.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use flottille_core::paths::Paths;
use flottille_core::protocol::Event;
use flottille_core::secrets;
use flottille_core::store::now_ms;
use serde_json::{Map, Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::providers;

/// Past this size the log is moved aside, to `developer.log.1`, and a new one begun: the log and
/// the one before it are all that is kept.
const LIMIT: u64 = 10 * 1024 * 1024;

/// The events the log takes, as they are emitted: what reaches a decision or comes of one. The
/// conversation's own events stay out, and `agent.tool_served` too, which a `call` line says in
/// full.
const EVENTS: [&str; 18] = [
    "daemon.started",
    "daemon.stopping",
    "provider.judge_changed",
    "agent.created",
    "agent.started",
    "agent.resumed",
    "agent.released",
    "agent.died",
    "agent.failed",
    "agent.ended",
    "agent.replaced",
    "agent.gate_mode_changed",
    "agent.permission_answered",
    "agent.provider_error",
    "agent.provider_limit",
    "run.started",
    "run.ended",
    "workspace.step_failed",
];

/// The prefixes of the events the log takes whole: the requests and the deliveries.
const EVENT_PREFIXES: [&str; 2] = ["approval.", "delivery."];

pub struct Log {
    on: AtomicBool,
    file: PathBuf,
    limit: u64,
    /// The open log, once a line was written: one writer at a time, so lines never mix.
    open: Mutex<Option<File>>,
}

impl Log {
    /// The log of these folders, on when `developer.toml` says so.
    pub fn open(paths: &Paths) -> Log {
        Log::at(file(paths), setting(paths).unwrap_or(false), LIMIT)
    }

    fn at(file: PathBuf, on: bool, limit: u64) -> Log {
        Log {
            on: AtomicBool::new(on),
            file,
            limit,
            open: Mutex::new(None),
        }
    }

    /// Whether lines are written: what a caller asks before it puts one together.
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    fn set(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
        if !on {
            // Closed while off: a log removed meanwhile is begun again, not written unlinked.
            *self.open.lock().unwrap() = None;
        }
    }

    /// Appends one line of this kind, every text in it masked of the daemon's secrets and of
    /// `values`. Nothing when the mode is off. A failure to write is said in the daemon's own log
    /// and loses the line, never the call.
    pub fn write(&self, kind: &str, fields: Value, values: &[String]) {
        if !self.on() {
            return;
        }
        let mut line = Map::new();
        line.insert("at".into(), now_ms().into());
        line.insert("kind".into(), kind.into());
        if let Value::Object(fields) = fields {
            line.extend(fields);
        }
        let env = std::env::vars_os().collect();
        let mut text = masked(&Value::Object(line), &env, values).to_string();
        text.push('\n');
        if let Err(error) = self.append(text.as_bytes()) {
            eprintln!(
                "flottille daemon: the developer log {}: {error}",
                self.file.display()
            );
        }
    }

    /// One of the events the log takes, with the agent it is about at the top when it names one.
    pub fn event(&self, event: &Event) {
        if !self.on()
            || !(EVENTS.contains(&event.name.as_str())
                || EVENT_PREFIXES.iter().any(|p| event.name.starts_with(p)))
        {
            return;
        }
        self.write(
            "event",
            json!({"agent": event.data["agent"].as_str(), "seq": event.seq, "name": event.name,
                "data": event.data}),
            &[],
        );
    }

    fn append(&self, line: &[u8]) -> std::io::Result<()> {
        let mut open = self.open.lock().unwrap();
        let size = match open.as_ref() {
            Some(file) => file.metadata()?.len(),
            None => std::fs::metadata(&self.file).map_or(0, |m| m.len()),
        };
        if size > 0 && size + line.len() as u64 > self.limit {
            *open = None;
            std::fs::rename(&self.file, self.file.with_extension("log.1"))?;
        }
        if open.is_none() {
            *open = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .mode(0o600)
                    .open(&self.file)?,
            );
        }
        open.as_mut().expect("opened above").write_all(line)
    }
}

/// `$XDG_STATE_HOME/flottille/developer.log`.
pub fn file(paths: &Paths) -> PathBuf {
    paths.state.join("developer.log")
}

/// `$XDG_CONFIG_HOME/flottille/developer.toml`.
fn config(paths: &Paths) -> PathBuf {
    paths.config.join("developer.toml")
}

/// What `developer.toml` says of the log: off when there is no file.
fn setting(paths: &Paths) -> Result<bool, String> {
    let text = match std::fs::read_to_string(config(paths)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("developer.toml: {error}")),
    };
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| format!("developer.toml: {}", e.message()))?;
    match document.get("log") {
        None => Ok(false),
        Some(log) => log
            .as_bool()
            .ok_or_else(|| "developer.toml: log: it is true or false".to_owned()),
    }
}

/// Every text of `value` masked.
fn masked(
    value: &Value,
    env: &std::collections::BTreeMap<OsString, OsString>,
    values: &[String],
) -> Value {
    match value {
        Value::String(text) => Value::String(secrets::mask_also(text, env, values)),
        Value::Array(items) => Value::Array(items.iter().map(|v| masked(v, env, values)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), masked(v, env, values)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    log: bool,
}

/// Answers a `developer.*` request of the user: `developer.show`, `developer.set`. No tool of an
/// agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    match method {
        // The file is the user's to edit by hand too: read again, it is what the log follows.
        "developer.show" => {
            let on = setting(paths).map_err(|e| Failure::new("invalid-config", e))?;
            daemon.developer.set(on);
        }
        "developer.set" => {
            let Set { log } = serde_json::from_value(params)?;
            std::fs::create_dir_all(&paths.config).map_err(|e| {
                Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
            })?;
            providers::write_in_place(&config(paths), &format!("log = {log}\n"))
                .map_err(|e| Failure::new("unavailable", format!("developer.toml: {e}")))?;
            daemon.developer.set(log);
            daemon
                .emit("developer.changed", json!({"log": log}))
                .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        }
        method => {
            return Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            ));
        }
    }
    Ok(json!({"log": daemon.developer.on(), "file": file(paths)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(file: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn the_log_rotates_past_its_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("developer.log");
        let log = Log::at(file.clone(), true, 300);

        for n in 0..10 {
            log.write("call", json!({"n": n, "padding": "x".repeat(40)}), &[]);
        }

        let old = file.with_extension("log.1");
        assert!(std::fs::metadata(&file).unwrap().len() <= 300);
        assert!(std::fs::metadata(&old).unwrap().len() <= 300);
        let (old, new) = (lines(&old), lines(&file));
        assert_eq!(new.last().unwrap()["n"], 9);
        // Nothing lost between the two, and only the one before is kept.
        assert_eq!(old.last().unwrap()["n"].as_u64().unwrap() + 1, new[0]["n"]);
        assert!(old[0]["n"].as_u64().unwrap() > 0, "{old:?}");
    }

    #[test]
    fn off_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("developer.log");
        let log = Log::at(file.clone(), false, LIMIT);

        log.write("call", json!({"n": 1}), &[]);

        assert!(!file.exists());
    }

    #[test]
    fn a_value_given_is_masked_wherever_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("developer.log");
        let log = Log::at(file.clone(), true, LIMIT);

        log.write(
            "call",
            json!({"arguments": {"line": "echo s3cr3t-value"}, "list": ["s3cr3t-value"]}),
            &["s3cr3t-value".to_owned()],
        );

        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("s3cr3t-value"), "{text}");
        assert!(text.contains("[masked]"), "{text}");
    }
}
