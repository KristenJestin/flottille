//! An extension's guidance (`docs/extensions.md`, "Guidance"): `<name>.md` in the configuration
//! folder, absent while the default holds, read at each use, edited and reset by the user through
//! `<name>.show`, `<name>.set {text}` and `<name>.reset`. Written from the judge's (`judge.rs`),
//! which keeps its own until it moves into the frame.

use std::sync::Mutex;

use serde_json::{Value, json};

use super::{Extension, Guidance};
use crate::daemon::Daemon;
use crate::projects::Failure;

/// Held while a guidance is written or removed: two edits at once leave one of them whole.
static WRITING: Mutex<()> = Mutex::new(());

fn file(daemon: &Daemon, extension: &Extension) -> std::path::PathBuf {
    daemon.paths.config.join(format!("{}.md", extension.name))
}

/// The user's guidance: none while the default holds; `Err((what could be read, why))` when it
/// cannot be followed as written. Never cut, never replaced by the default.
fn own(
    daemon: &Daemon,
    extension: &Extension,
    guidance: &Guidance,
) -> Result<Option<String>, (String, String)> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let file = file(daemon, extension);
    let named = guidance.named;
    let limit = guidance.limit;
    let cannot = |error: std::io::Error| {
        (
            String::new(),
            format!("{named} ({}) cannot be read: {error}", file.display()),
        )
    };
    // A pipe would hold the use until someone writes to it: opened without waiting.
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&file);
    let opened = match opened {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(cannot(error)),
    };
    if !opened.metadata().map_err(cannot)?.is_file() {
        return Err((
            String::new(),
            format!(
                "{named} ({}) cannot be read: it is not a regular file; remove it, or reset it",
                file.display()
            ),
        ));
    }
    let most = (limit as u64 + 1) * 4;
    let mut bytes = Vec::new();
    opened.take(most).read_to_end(&mut bytes).map_err(cannot)?;
    let text = String::from_utf8(bytes).map_err(|error| {
        (
            String::from_utf8_lossy(error.as_bytes()).into_owned(),
            format!(
                "{named} ({}) is not UTF-8 text: write it again, or reset it",
                file.display()
            ),
        )
    })?;
    let count = text.chars().count();
    if count > limit {
        return Err((
            text,
            format!(
                "{named} ({}) is {count} characters long and may hold {limit}: shorten it, or \
                 reset it",
                file.display()
            ),
        ));
    }
    Ok(Some(text).filter(|text| !text.trim().is_empty()))
}

/// The guidance the next use follows: the user's, else the default; why not when the user's
/// cannot be followed.
pub fn text(daemon: &Daemon, extension: &Extension) -> Result<String, String> {
    let Some(guidance) = &extension.guidance else {
        return Ok(String::new());
    };
    Ok(own(daemon, extension, guidance)
        .map_err(|(_, why)| why)?
        .unwrap_or_else(|| guidance.default.to_owned()))
}

/// Whether the method is a guidance's of an extension that has one.
pub fn answers(extension: &Extension, method: &str) -> bool {
    extension.guidance.is_some()
        && method
            .strip_prefix(extension.name)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|verb| ["show", "set", "reset"].contains(&verb))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    text: String,
}

/// `<name>.show`, `<name>.set {text}` (an empty text resets it) and `<name>.reset`: each answers
/// `{"text", "source", "error", "default"}`, the shape of `judge.show`.
pub fn call(
    daemon: &Daemon,
    extension: &Extension,
    method: &str,
    params: Value,
) -> Result<Value, Failure> {
    let guidance = extension.guidance.as_ref().expect("a guidance");
    let verb = &method[extension.name.len() + 1..];
    if verb != "show" {
        let text = match verb {
            "set" => {
                let Set { text } = serde_json::from_value(params)?;
                let count = text.chars().count();
                if count > guidance.limit {
                    return Err(Failure::refused(format!(
                        "{} is {count} characters long and may hold {}: keep what matters",
                        guidance.named, guidance.limit
                    )));
                }
                Some(text).filter(|text| !text.trim().is_empty())
            }
            _ => None,
        };
        let _writing = WRITING.lock().unwrap();
        let path = file(daemon, extension);
        let unavailable =
            |e: std::io::Error| Failure::new("unavailable", format!("{}.md: {e}", extension.name));
        match text {
            Some(text) => {
                std::fs::create_dir_all(&daemon.paths.config).map_err(unavailable)?;
                crate::providers::write_in_place(&path, &text).map_err(unavailable)?;
            }
            None => match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(unavailable(e)),
                _ => {}
            },
        }
        daemon
            .emit(&format!("{}.changed", extension.name), json!({}))
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    }
    let (text, source, error) = match own(daemon, extension, guidance) {
        Ok(Some(text)) => (text, "global", None),
        Ok(None) => (guidance.default.to_owned(), "default", None),
        Err((text, why)) => (text, "global", Some(why)),
    };
    Ok(json!({"text": text, "source": source, "error": error, "default": guidance.default}))
}
