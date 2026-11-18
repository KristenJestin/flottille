//! What the user leaves in the window and did not finish: a composer's draft (the text and the
//! pictures of a session's composer, or of a project's first message) and the window's own small
//! state (what the sidebar has folded). The daemon keeps them, so that they survive the window, a
//! restart of the daemon, and are the same on every window of the machine (`docs/protocol.md`,
//! "The draft and window methods").
//!
//! A draft is the user's own words: it is never given to an agent, never in an event and never in
//! a log, and its pictures are files of a private folder of the state folder that go with it.

use std::path::PathBuf;

use flottille_core::paths::{Paths, ensure_private_dir};
use flottille_core::protocol::ImageUpload;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::deliveries::{IMAGE_TYPES, MAX_IMAGES};
use crate::projects::Failure;

/// The longest text a draft keeps, in bytes: more than any message the user types.
const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// The key the window's sidebar state is kept under (`window.set`).
const SIDEBAR: &str = "sidebar";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    kind: String,
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    kind: String,
    target: String,
    #[serde(default)]
    text: String,
    /// The pictures that go with the text, all of them; left out, those it has are kept.
    #[serde(default)]
    images: Option<Vec<ImageUpload>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowSet {
    key: String,
    /// `null` forgets the key.
    #[serde(default)]
    value: Value,
}

/// Where the pictures of the drafts are: one folder by composer, private.
fn folder(paths: &Paths, key: &str) -> PathBuf {
    let hex: String = key.bytes().map(|byte| format!("{byte:02x}")).collect();
    paths.state.join("drafts").join(hex)
}

/// The key of the composer `target` of `kind`, once the thing it belongs to is known.
fn key_of(daemon: &Daemon, kind: &str, target: &str) -> Result<String, Failure> {
    if target.is_empty() || target.len() > 200 {
        return Err(Failure::refused("a draft's target is 1 to 200 bytes"));
    }
    match kind {
        "session" => {
            let known = daemon.read(|store| store.agent(target))?;
            if known.is_none() {
                return Err(Failure::new("not-found", format!("no agent {target}")));
            }
        }
        "project" => {
            crate::projects::load_project(daemon, target)?;
        }
        other => {
            return Err(Failure::refused(format!(
                "a draft is of a `session` or a `project`, not `{other}`"
            )));
        }
    }
    Ok(format!("{kind}:{target}"))
}

/// The pictures judged (the composer's own limits) and written to the draft's folder: the names
/// of their files.
fn keep_pictures(paths: &Paths, key: &str, images: &[ImageUpload]) -> Result<Vec<String>, Failure> {
    use base64::Engine as _;
    if images.len() > MAX_IMAGES {
        return Err(Failure::refused(format!(
            "a message carries {MAX_IMAGES} images at most"
        )));
    }
    let mut decoded = Vec::new();
    for image in images {
        let (_, extension) = IMAGE_TYPES
            .iter()
            .find(|(mime, _)| *mime == image.mime)
            .ok_or_else(|| {
                Failure::refused(format!(
                    "an image of type {} is not taken: png, jpeg, gif and webp are",
                    image.mime
                ))
            })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|_| Failure::refused("an image is not valid base64"))?;
        if bytes.is_empty() || bytes.len() > flottille_core::images::MAX_IMAGE_BYTES {
            return Err(Failure::refused(format!(
                "an image is empty or over {} MiB",
                flottille_core::images::MAX_IMAGE_BYTES / 1024 / 1024
            )));
        }
        decoded.push((*extension, bytes));
    }
    if decoded.is_empty() {
        return Ok(Vec::new());
    }
    let place = folder(paths, key);
    ensure_private_dir(&place).map_err(|e| Failure::new("unavailable", e.to_string()))?;
    let stamp = flottille_core::agent::new_id()
        .map_err(|e| Failure::new("unavailable", format!("no stamp: {e}")))?;
    let mut names = Vec::new();
    for (at, (extension, bytes)) in decoded.into_iter().enumerate() {
        let name = format!("{stamp}-{at}.{extension}");
        if let Err(error) = std::fs::write(place.join(&name), bytes) {
            remove_files(paths, key, &names);
            return Err(Failure::new(
                "unavailable",
                format!("a picture of the draft cannot be kept: {error}"),
            ));
        }
        names.push(name);
    }
    Ok(names)
}

fn remove_files(paths: &Paths, key: &str, names: &[String]) {
    let place = folder(paths, key);
    for name in names {
        let _ = std::fs::remove_file(place.join(name));
    }
    // Empty, the folder goes: a draft that holds nothing leaves nothing.
    let _ = std::fs::remove_dir(&place);
}

/// The draft of `key` goes, with its pictures.
fn forget(daemon: &Daemon, key: &str) {
    if let Ok(names) = daemon.write(|store| {
        Ok((
            store.forget_draft(key)?,
            None::<flottille_core::protocol::Event>,
        ))
    }) {
        remove_files(&daemon.paths, key, &names);
    }
}

/// The session `agent` is forgotten: its draft goes.
pub fn forget_session(daemon: &Daemon, agent: &str) {
    forget(daemon, &format!("session:{agent}"));
}

/// The project `name` is removed, with the sessions `agents` it had: their drafts and what the
/// sidebar kept of them go.
pub fn forget_project(daemon: &Daemon, name: &str, agents: &[String]) {
    forget(daemon, &format!("project:{name}"));
    for agent in agents {
        forget_session(daemon, agent);
    }
    let sidebar = daemon
        .read(|store| store.window_state())
        .ok()
        .and_then(|state| state.into_iter().find(|(key, _)| key == SIDEBAR))
        .map(|(_, value)| value);
    let Some(mut sidebar) = sidebar else { return };
    if let Some(projects) = sidebar["projects"].as_object_mut() {
        projects.remove(name);
    }
    if let Some(leads) = sidebar["leads"].as_object_mut() {
        for agent in agents {
            leads.remove(agent);
        }
    }
    let _ = daemon.write(|store| {
        store.set_window_state(SIDEBAR, Some(&sidebar))?;
        Ok(((), None::<flottille_core::protocol::Event>))
    });
}

/// Answers a `draft.*` or `window.*` request of the user. No tool of an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "draft.get" => {
            let Target { kind, target } = serde_json::from_value(params)?;
            let key = key_of(daemon, &kind, &target)?;
            let found = daemon.read(|store| store.draft(&key))?;
            let Some((text, names)) = found else {
                return Ok(json!({"draft": null}));
            };
            Ok(json!({"draft": {"text": text, "images": pictures_of(daemon, &key, &names)}}))
        }
        "draft.set" => {
            let Set {
                kind,
                target,
                text,
                images,
            } = serde_json::from_value(params)?;
            if text.len() > MAX_TEXT_BYTES {
                return Err(Failure::refused(format!(
                    "a draft's text is {} MiB at most",
                    MAX_TEXT_BYTES / 1024 / 1024
                )));
            }
            let key = key_of(daemon, &kind, &target)?;
            let before = daemon.read(|store| store.draft(&key))?;
            let names = match &images {
                Some(images) => keep_pictures(&daemon.paths, &key, images)?,
                None => before.map(|(_, names)| names).unwrap_or_default(),
            };
            // Nothing to send is no draft.
            if text.trim().is_empty() && names.is_empty() {
                forget(daemon, &key);
                return Ok(json!({"kept": false}));
            }
            let replaced = daemon.write(|store| {
                Ok((
                    store.put_draft(&key, &text, &names)?,
                    None::<flottille_core::protocol::Event>,
                ))
            });
            match replaced {
                Ok(replaced) => {
                    let gone: Vec<String> = replaced
                        .into_iter()
                        .filter(|name| !names.contains(name))
                        .collect();
                    remove_files(&daemon.paths, &key, &gone);
                    Ok(json!({"kept": true}))
                }
                Err(error) => {
                    if images.is_some() {
                        remove_files(&daemon.paths, &key, &names);
                    }
                    Err(error.into())
                }
            }
        }
        "draft.clear" => {
            let Target { kind, target } = serde_json::from_value(params)?;
            forget(
                daemon,
                &key_of(daemon, &kind, &target).or_else(|failure| {
                    // A session or a project that is gone has no composer: its draft goes all the same.
                    match failure.code {
                        "not-found" => Ok(format!("{kind}:{target}")),
                        _ => Err(failure),
                    }
                })?,
            );
            Ok(json!({}))
        }
        "draft.list" => {
            let keys = daemon.read(|store| store.draft_keys())?;
            Ok(json!({"drafts": keys}))
        }
        "window.get" => {
            let state = daemon.read(|store| store.window_state())?;
            Ok(json!({"state": state.into_iter().collect::<serde_json::Map<_, _>>()}))
        }
        "window.set" => {
            let WindowSet { key, value } = serde_json::from_value(params)?;
            if key.is_empty() || key.len() > 100 {
                return Err(Failure::refused(
                    "a key of the window's state is 1 to 100 bytes",
                ));
            }
            daemon.write(|store| {
                store.set_window_state(&key, (!value.is_null()).then_some(&value))?;
                Ok(((), None::<flottille_core::protocol::Event>))
            })?;
            Ok(json!({}))
        }
        other => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{other}`"),
        )),
    }
}

/// The pictures of a draft as the window takes them: the MIME type and the bytes in base64.
fn pictures_of(daemon: &Daemon, key: &str, names: &[String]) -> Vec<Value> {
    use base64::Engine as _;
    let place = folder(&daemon.paths, key);
    names
        .iter()
        .filter_map(|name| {
            let (_, extension) = name.rsplit_once('.')?;
            let (mime, _) = IMAGE_TYPES.iter().find(|(_, e)| *e == extension)?;
            let bytes = std::fs::read(place.join(name)).ok()?;
            Some(json!({"mime": mime,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes)}))
        })
        .collect()
}
