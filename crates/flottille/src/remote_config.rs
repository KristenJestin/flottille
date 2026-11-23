//! Turning remote on and off, from the window and the command line (`docs/remote.md`, "Identity
//! and keys"; `docs/protocol.md`, "Remote"): `remote.settings` reads whether it is on,
//! `remote.configure` writes it in `daemon.toml` (keeping every other key and comment of the
//! user's file) and takes it at once: the link starts or stops without the daemon being started
//! again. Both are answered to this machine's own clients only.

use std::sync::Arc;

use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;

/// What every message that says remote is off says to do.
pub const OFF: &str =
    "remote is off: turn it on in Settings, Machines, or with flottille remote on";

/// `daemon.toml`'s text with `[remote] on` set to `on`. Every other key and every comment stays as
/// the user wrote it.
fn edited(text: &str, on: bool) -> Result<String, String> {
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            format!(
                "daemon.toml is not valid TOML, so it is left as it is: {}",
                error.message()
            )
        })?;
    if document.get("remote").is_some_and(|item| !item.is_table()) {
        return Err(
            "daemon.toml has a `remote` that is not a table, so it is left as it is".into(),
        );
    }
    let table = document
        .entry("remote")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("daemon.toml has a `remote` that is not a table")?;
    table["on"] = toml_edit::value(on);
    Ok(document.to_string())
}

/// `text` written as `daemon.toml`, whole: a temporary file renamed into place.
fn write(paths: &Paths, text: &str) -> Result<(), String> {
    let file = paths.config.join("daemon.toml");
    std::fs::create_dir_all(&paths.config)
        .map_err(|error| format!("{} is not made: {error}", paths.config.display()))?;
    let temporary = file.with_extension("toml.new");
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, &file))
        .map_err(|error| format!("daemon.toml is not written: {error}"))
}

fn read(paths: &Paths) -> String {
    std::fs::read_to_string(paths.config.join("daemon.toml")).unwrap_or_default()
}

/// What `remote.settings` and `remote.configure` answer.
fn shown(daemon: &Daemon) -> Value {
    json!({"on": crate::remote::on(&daemon.paths)})
}

pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "remote.settings" => Ok(shown(&daemon)),
        "remote.configure" => configure(&daemon, &params).await,
        _ => Err(Failure::new(
            flottille_protocol::code::UNKNOWN_METHOD,
            format!("unknown method {method}"),
        )),
    }
}

async fn configure(daemon: &Arc<Daemon>, params: &Value) -> Result<Value, Failure> {
    let invalid = |why: String| Failure::new(flottille_protocol::code::INVALID_PARAMS, why);
    let on = match &params["on"] {
        Value::Null => return Ok(shown(daemon)),
        Value::Bool(on) => *on,
        _ => return Err(invalid("on is true or false".into())),
    };
    let paths = &daemon.paths;
    let before = read(paths);
    let text = edited(&before, on).map_err(Failure::refused)?;
    write(paths, &text).map_err(Failure::refused)?;
    match apply(daemon).await {
        Ok(()) => Ok(shown(daemon)),
        Err(why) => {
            // What was refused changes nothing: the file is as it was, the link too.
            let _ = write(paths, &before);
            let _ = apply(daemon).await;
            Err(Failure::refused(why))
        }
    }
}

/// Takes what `daemon.toml` says now: the link starts when it is on and none runs, and stops when
/// it is off.
async fn apply(daemon: &Arc<Daemon>) -> Result<(), String> {
    let paths = &daemon.paths;
    let on = crate::remote::on(paths);
    let running = match daemon.link() {
        Some(Ok(link)) => Some(link),
        _ => None,
    };
    match (running, on) {
        (None, false) => {
            // A link that failed to start is no link: it goes with the switch.
            daemon.set_link(None);
            Ok(())
        }
        (Some(link), false) => {
            daemon.set_link(None);
            link.stop().await;
            Ok(())
        }
        (None, true) => start(daemon, paths),
        (Some(_), true) => Ok(()),
    }
}

/// Starts the link as `daemon.toml` says: the machine key is made at the first time.
fn start(daemon: &Arc<Daemon>, paths: &Paths) -> Result<(), String> {
    let key = crate::remote::machine_key(paths)
        .map_err(|error| error.to_string())?
        .ok_or("remote is off")?;
    crate::daemon::start_link(daemon, key).map_err(|why| format!("the link did not start: {why}"))
}
