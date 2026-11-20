//! `web {action: "show_link"}`: an agent hands the user a link (`docs/tools.md`, "Showing a link").
//! The daemon opens nothing: it checks the link and says it in an event, `agent.link_shown`, and
//! the window offers the user an Open button (`docs/events.md`).

use std::path::PathBuf;
use std::sync::Arc;

use flottille_core::{secrets, shown};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::daemon::Daemon;
use crate::gate::Verdict;
use crate::mcp::{Grant, Outcome, arguments};
use crate::place::Place;

/// The action's name in `web`.
pub const ACTION: &str = "show_link";
/// A title is a short line: the toast and the card show it whole.
const TITLE_CHARS: usize = 120;
/// What a link may be long: it rides in an event.
const URL_CHARS: usize = 2_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    #[allow(dead_code)]
    action: String,
    url: Option<String>,
    path: Option<String>,
    title: Option<String>,
}

/// Whether a call of `web` is this action.
pub fn is(args: &Value) -> bool {
    args["action"] == ACTION
}

/// What is shown, checked: an address or a file of the agent's place.
enum Link {
    Url { url: Url, host: String },
    File { real: PathBuf, shown: String },
}

fn refuse(why: impl std::fmt::Display) -> String {
    format!("refused: {why}; nothing was shown")
}

/// The link and its title, or the refusal saying why and what to give instead.
fn asked(place: &Place, args: &Value) -> Result<(Link, Option<String>), String> {
    let args: Arguments = match arguments("web", args.clone()) {
        Ok(args) => args,
        Err(Outcome::Invalid(why)) => return Err(why),
        Err(_) => unreachable!("arguments only refuse"),
    };
    let title = args
        .title
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty());
    if title
        .as_ref()
        .is_some_and(|t| t.chars().count() > TITLE_CHARS)
    {
        return Err(refuse(format!(
            "the title is longer than {TITLE_CHARS} characters: give a short one"
        )));
    }
    let link = match (args.url, args.path) {
        (Some(url), None) => address(&url)?,
        (None, Some(path)) => file(place, &path)?,
        _ => {
            return Err(refuse(
                "web show_link takes a url (http or https) or a path of your place, one of them",
            ));
        }
    };
    Ok((link, title))
}

fn address(text: &str) -> Result<Link, String> {
    if text.chars().count() > URL_CHARS {
        return Err(refuse(format!(
            "this URL is longer than {URL_CHARS} characters"
        )));
    }
    let url = Url::parse(text.trim())
        .map_err(|error| refuse(format!("{text:?} is not a URL ({error})")))?;
    let host = url.host_str().unwrap_or_default().to_owned();
    if !matches!(url.scheme(), "http" | "https") || host.is_empty() {
        return Err(refuse(format!(
            "show_link shows an http or https URL, and {text:?} is not one"
        )));
    }
    let env = std::env::vars_os().collect();
    if secrets::mask(url.as_str(), &env) != url.as_str() {
        return Err(refuse("this URL holds what looks like a secret"));
    }
    Ok(Link::Url { url, host })
}

fn file(place: &Place, named: &str) -> Result<Link, String> {
    let resolved = place.resolve(&place.root, named);
    if !resolved.inside || resolved.unfollowed.is_some() {
        return Err(refuse(format!(
            "{} is not inside your place ({}): show_link shows a file of your own workspace or \
             checkout, where it really leads once links and .. are followed",
            resolved.shown,
            place.root.display()
        )));
    }
    if let Err(why) = shown::document(&resolved.path) {
        return Err(refuse(format!(
            "{}: {why}; show_link shows a document, never a program",
            resolved.shown
        )));
    }
    Ok(Link::File {
        real: resolved.path,
        shown: resolved.shown,
    })
}

/// The gate's verdict: it changes nothing, so a link that holds passes in every mode.
pub fn verdict(place: &Place, args: &Value) -> Verdict {
    match asked(place, args) {
        Ok(_) => Verdict::Allow,
        Err(why) => Verdict::Deny(why),
    }
}

/// Says the link in an event; the user opens it, or not.
pub fn show(daemon: &Arc<Daemon>, grant: &Grant, args: Value) -> Outcome {
    let (link, title) = match asked(&grant.place, &args) {
        Ok(asked) => asked,
        Err(why) => return Outcome::Refused(why),
    };
    let env = std::env::vars_os().collect();
    let title = title.map(|title| secrets::mask_also(&title, &env, &grant.place.secret_values()));
    let (named, data) = match &link {
        Link::Url { url, host } => (
            title.clone().unwrap_or_else(|| host.clone()),
            json!({"agent": grant.agent, "kind": "url", "target": url.as_str(),
                "title": title, "host": host}),
        ),
        Link::File { real, shown } => {
            // What a machine serving it later tells the file shown from another at the same path.
            let (dev, ino) = identity(real).unzip();
            (
                title.clone().unwrap_or_else(|| shown.clone()),
                json!({"agent": grant.agent, "kind": "file", "target": real, "title": title,
                    "host": null, "shown": shown, "dev": dev, "ino": ino}),
            )
        }
    };
    let said = daemon.write(|store| {
        let event = store.append_agent_event(&grant.agent, "agent.link_shown", data, false)?;
        Ok(((), event))
    });
    match said {
        Ok(()) => Outcome::Done(format!(
            "Shown to the user: {named}. They open it with a click; nothing opened by itself."
        )),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

// ── A file shown, served to another device ───────────────────────────────────────────────────

/// The most of a file one answer of `agent.file` carries: in base64, with its JSON, it stays
/// within the frame a link takes (`flottille_link::FRAME_LIMIT`, 1 MiB).
pub const CHUNK: u64 = 512 * 1024;
/// The largest file served: a video of a session, not a disk.
const MOST_SERVED: u64 = 2 * 1024 * 1024 * 1024;
/// The bytes per second this machine serves, all devices together: a video plays at it, and a
/// device that asks without end takes no more of the machine and less than a third of the
/// requests its signature allows in five minutes (`flottille_link::NONCES_PER_DEVICE`).
const RATE: u64 = 2 * 1024 * 1024;

/// The longest a chunk waits for the machine's rate: a chunk that would wait more is refused at
/// once, and the asker tries again. The wait of one asker never grows into the others'.
const MOST_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
/// The calls of `agent.file` the machine holds at once, each with its chunk in memory, whoever
/// asks (the daemon does not know the device here): the others are refused at once.
const MOST_IN_FLIGHT: usize = 8;

/// The calls of `agent.file` in flight.
static IN_FLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One call in flight, counted until it is dropped.
struct InFlight;

impl InFlight {
    fn enter() -> Option<InFlight> {
        use std::sync::atomic::Ordering::SeqCst;
        IN_FLIGHT
            .fetch_update(SeqCst, SeqCst, |held| {
                (held < MOST_IN_FLIGHT).then_some(held + 1)
            })
            .ok()
            .map(|_| InFlight)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// What `agent.file` serves, by the extension of the file: a picture or a video.
pub fn media_type(path: &str) -> Option<&'static str> {
    let extension = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "ogv" => "video/ogg",
        _ => return None,
    })
}

/// When the next chunk may leave: the time the chunks served so far take at [`RATE`].
static PACE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// How long a chunk of `bytes` waits before it leaves, the chunks before it taking their time;
/// none, and nothing reserved, when that is more than [`MOST_WAIT`].
fn wait_for(
    bytes: u64,
    now: std::time::Instant,
    next: &mut Option<std::time::Instant>,
) -> Option<std::time::Duration> {
    let start = next.filter(|next| *next > now).unwrap_or(now);
    if start - now > MOST_WAIT {
        return None;
    }
    *next = Some(start + std::time::Duration::from_secs_f64(bytes as f64 / RATE as f64));
    Some(start - now)
}

/// The device and inode of the file at `path`, which tell it from another file of the same path.
fn identity(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// `agent.file`: a chunk of a picture or a video `agent` showed, named by the number of the event
/// that said it (`docs/remote.md`, "A file an agent showed"). Only that file, as it was when it
/// was shown: the path is never the asker's, the file is opened without following a link and must
/// still be the very path recorded, a regular file with no execute bit.
pub async fn serve_file(
    daemon: &Arc<Daemon>,
    asked: flottille_core::protocol::AgentFile,
) -> Result<Value, crate::projects::Failure> {
    use crate::projects::Failure;
    let agent = asked.agent;
    let shown = asked.shown;
    let none = || {
        Failure::new(
            "not-found",
            format!("{agent} showed no file as {shown}: only a file a session showed is served"),
        )
    };
    let event = daemon
        .read(|store| store.agent_events(&agent, shown.saturating_sub(1), 1))
        .map_err(|error| Failure::new("internal", error.to_string()))?
        .into_iter()
        .next()
        .filter(|event| {
            event.seq == shown && event.name == "agent.link_shown" && event.data["kind"] == "file"
        })
        .ok_or_else(none)?;
    let target = event.data["target"].as_str().ok_or_else(none)?.to_owned();
    let mime = media_type(&target).ok_or_else(|| {
        Failure::refused(format!(
            "{target} is no picture or video: only a picture or a video a session showed is served"
        ))
    })?;
    let identity = event.data["dev"].as_u64().zip(event.data["ino"].as_u64());
    let busy = || {
        Failure::refused(
            "busy: this machine is serving as many files as it takes at once; ask again in a moment",
        )
    };
    let _held = InFlight::enter().ok_or_else(busy)?;
    let (offset, length) = (asked.offset, asked.length.unwrap_or(CHUNK).min(CHUNK));
    let read = tokio::task::spawn_blocking(move || read_chunk(&target, identity, offset, length))
        .await
        .map_err(|error| Failure::new("internal", error.to_string()))?
        .map_err(Failure::refused)?;
    let (size, bytes) = read;
    let wait = wait_for(
        bytes.len() as u64,
        std::time::Instant::now(),
        &mut PACE.lock().unwrap(),
    )
    .ok_or_else(busy)?;
    tokio::time::sleep(wait).await;
    use base64::Engine as _;
    Ok(json!({"mime": mime, "size": size, "offset": offset,
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes)}))
}

/// At most `length` bytes of the file at `target` from `offset`, and its size; why not, otherwise.
fn read_chunk(
    target: &str,
    identity: Option<(u64, u64)>,
    offset: u64,
    length: u64,
) -> Result<(u64, Vec<u8>), String> {
    use std::io::{Read as _, Seek as _};
    let refused = |why: &str| format!("refused: {target} is not served: {why}");
    let file = shown::open_document(std::path::Path::new(target)).map_err(|why| refused(&why))?;
    // A folder of the path made a link since it was shown leads elsewhere: the file opened must
    // be the very one recorded.
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd as _;
        let opened = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .map_err(|_| refused("where it leads cannot be read"))?;
        if opened != std::path::Path::new(target) {
            return Err(refused("it is no longer the file shown"));
        }
    }
    let meta = file.metadata().map_err(|_| refused("it cannot be read"))?;
    // A hard link in the place to a file outside it has its real path in the place: it is the
    // file shown, and nothing else, that is served, and a file with another name is not one.
    {
        use std::os::unix::fs::MetadataExt as _;
        if identity != Some((meta.dev(), meta.ino())) {
            return Err(refused("it is no longer the file shown"));
        }
        if meta.nlink() > 1 {
            return Err(refused(
                "it is a hard link: another name may lead out of the place",
            ));
        }
    }
    let size = meta.len();
    if size > MOST_SERVED {
        return Err(refused("it is larger than 2 GiB"));
    }
    let mut file = file;
    if offset >= size {
        return Ok((size, Vec::new()));
    }
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|_| refused("it cannot be read"))?;
    let mut bytes = Vec::new();
    file.take(length)
        .read_to_end(&mut bytes)
        .map_err(|_| refused("it cannot be read"))?;
    Ok((size, bytes))
}

#[cfg(test)]
mod served {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn chunks_leave_at_the_rate_whoever_asks() {
        let now = Instant::now();
        let mut next = None;
        assert_eq!(wait_for(RATE, now, &mut next), Some(Duration::ZERO));
        // A second chunk asked at once waits for the first one's second.
        assert_eq!(
            wait_for(RATE / 2, now, &mut next),
            Some(Duration::from_secs(1))
        );
        // Once the time has passed, nothing waits.
        assert_eq!(
            wait_for(1, now + Duration::from_secs(5), &mut next),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn a_chunk_that_would_wait_long_is_refused_and_reserves_nothing() {
        let now = Instant::now();
        let mut next = None;
        assert!(wait_for(RATE * 4, now, &mut next).is_some());
        let reserved = next;
        // The machine is booked for four seconds: nothing more is taken in, and nothing is booked.
        assert_eq!(wait_for(RATE, now, &mut next), None);
        assert_eq!(next, reserved);
    }

    #[test]
    fn only_a_picture_or_a_video_is_served() {
        assert_eq!(media_type("/w/shot.PNG"), Some("image/png"));
        assert_eq!(media_type("/w/clip.mp4"), Some("video/mp4"));
        assert_eq!(media_type("/w/notes.md"), None);
        assert_eq!(media_type("/w/clip.mp4.sh"), None);
    }
}
