//! An extension's guidance (`docs/extensions.md`, "Guidance"): `<name>.md` in the configuration
//! folder, absent while the default holds, read at each use, edited and reset by the user through
//! `<name>.show`, `<name>.set {text}` and `<name>.reset`. Written from the judge's (`judge.rs`),
//! which keeps its own until it moves into the frame.
//!
//! A guidance may have parts the extension names (Review's checks): `<name>-<part>.md`, with no
//! default, absent while empty, the same methods given `part`.

use std::sync::Mutex;

use serde_json::{Value, json};

use super::{Extension, Guidance};
use crate::daemon::Daemon;
use crate::projects::Failure;

/// Held while a guidance is written or removed: two edits at once leave one of them whole.
static WRITING: Mutex<()> = Mutex::new(());

fn file(daemon: &Daemon, extension: &Extension, part: Option<&str>) -> std::path::PathBuf {
    let name = match part {
        Some(part) => format!("{}-{part}.md", extension.name),
        None => format!("{}.md", extension.name),
    };
    daemon.paths.config.join(name)
}

/// The user's guidance: none while the default holds; `Err((what could be read, why))` when it
/// cannot be followed as written. Never cut, never replaced by the default.
fn own(
    daemon: &Daemon,
    extension: &Extension,
    guidance: &Guidance,
    part: Option<&str>,
) -> Result<Option<String>, (String, String)> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let file = file(daemon, extension, part);
    let named = match part {
        Some(part) => format!("{} for {part}", guidance.named),
        None => guidance.named.to_owned(),
    };
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
    Ok(own(daemon, extension, guidance, None)
        .map_err(|(_, why)| why)?
        .unwrap_or_else(|| guidance.default.to_owned()))
}

/// The user's text of the part `part` the next use follows, none while it is empty; why not when
/// it cannot be followed as written.
pub fn part_text(
    daemon: &Daemon,
    extension: &Extension,
    part: &str,
) -> Result<Option<String>, String> {
    let Some(guidance) = &extension.guidance else {
        return Ok(None);
    };
    own(daemon, extension, guidance, Some(part)).map_err(|(_, why)| why)
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
    part: Option<String>,
    text: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Of {
    part: Option<String>,
}

/// `<name>.show {part?}`, `<name>.set {part?, text}` (an empty text resets it) and
/// `<name>.reset {part?}`: each answers `{"text", "source", "error", "default"}`, the shape of
/// `judge.show`; a part's default is empty.
pub fn call(
    daemon: &Daemon,
    extension: &Extension,
    method: &str,
    params: Value,
) -> Result<Value, Failure> {
    let guidance = extension.guidance.as_ref().expect("a guidance");
    let verb = &method[extension.name.len() + 1..];
    let (part, text) = match verb {
        "set" => {
            let Set { part, text } = serde_json::from_value(params)?;
            (part, Some(text))
        }
        _ if params.is_null() => (None, None),
        _ => (serde_json::from_value::<Of>(params)?.part, None),
    };
    if let Some(part) = &part
        && !guidance.parts.contains(&part.as_str())
    {
        return Err(Failure::refused(format!(
            "{} has no part {part}: its parts are {}",
            guidance.named,
            guidance.parts.join(", ")
        )));
    }
    let part = part.as_deref();
    if verb != "show" {
        let text = match text {
            Some(text) => {
                let count = text.chars().count();
                if count > guidance.limit {
                    return Err(Failure::refused(format!(
                        "{} is {count} characters long and may hold {}: keep what matters",
                        guidance.named, guidance.limit
                    )));
                }
                Some(text).filter(|text| !text.trim().is_empty())
            }
            None => None,
        };
        let _writing = WRITING.lock().unwrap();
        let path = file(daemon, extension, part);
        let shown = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let unavailable = |e: std::io::Error| Failure::new("unavailable", format!("{shown}: {e}"));
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
        let data = match part {
            Some(part) => json!({"part": part}),
            None => json!({}),
        };
        daemon
            .emit(&format!("{}.changed", extension.name), data)
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    }
    // A part has no default text: empty while the user wrote none.
    let default = match part {
        Some(_) => "",
        None => guidance.default,
    };
    let (text, source, error) = match own(daemon, extension, guidance, part) {
        Ok(Some(text)) => (text, "global", None),
        Ok(None) => (default.to_owned(), "default", None),
        Err((text, why)) => (text, "global", Some(why)),
    };
    Ok(json!({"text": text, "source": source, "error": error, "default": default}))
}
