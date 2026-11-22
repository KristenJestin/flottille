//! The pictures and pasted texts of a message that came in chunks (`agent.upload`, `docs/remote.md`,
//! "Pictures sent to another machine"): a message to another machine is one signed request, one
//! frame of the link, so its files come first, each chunk a request of its own, and the message
//! names their upload (`agent.send` and `agent.start`, `upload`). The bytes come from another
//! machine: every bound of a local message holds before any of them is kept, the names on disk
//! are this daemon's, and what the sender said is never said back but its upload's name, which
//! is letters and digits only.

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flottille_core::pasted;
use flottille_core::paths::ensure_private_dir;
use flottille_core::protocol::{AgentUpload, ImageUpload, TextUpload, UploadKind};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::deliveries::{IMAGE_TYPES, MAX_IMAGE_BYTES, MAX_IMAGES};
use crate::projects::Failure;

/// The most of a file one chunk carries (`flottille_link::UPLOAD_CHUNK`).
const CHUNK: usize = flottille_link::UPLOAD_CHUNK;
/// The uploads one sender (a device, or this machine's clients) has waiting for their messages.
const PER_SENDER: usize = 2;
/// The uploads the machine keeps at once, every sender together.
const PER_MACHINE: usize = 8;
/// How long an upload waits for its message after its last chunk: then it is dropped.
const LASTS: Duration = Duration::from_secs(120);

fn lasts() -> Duration {
    crate::test_time::tuned("FLOTTILLE_TEST_UPLOAD_MS", LASTS)
}

/// The uploads waiting for their messages, by sender and name.
#[derive(Default)]
pub struct Uploads(Held);

struct Upload {
    folder: PathBuf,
    files: BTreeMap<(Kind, u32), File>,
    last: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Picture,
    Text,
}

impl Kind {
    fn of(kind: UploadKind) -> Kind {
        match kind {
            UploadKind::Image => Kind::Picture,
            UploadKind::Text => Kind::Text,
        }
    }

    fn said(self) -> &'static str {
        match self {
            Kind::Picture => "picture",
            Kind::Text => "pasted text",
        }
    }
}

struct File {
    mime: Option<String>,
    size: u64,
    received: u64,
}

/// Who sent a request being served: the key of the device that signed it, else this machine's
/// own clients. An upload is its sender's alone.
fn sender() -> String {
    match crate::daemon::asked() {
        Some(asked) => asked
            .key
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        None => "local".to_owned(),
    }
}

/// The name of an upload, the sender's word: refused unless 1 to 64 letters, digits, `_` or `-`,
/// so that it is never a path, and never said back unless it is one.
fn named(upload: &str) -> Result<(), Failure> {
    let fine = (1..=64).contains(&upload.len())
        && upload
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if !fine {
        return Err(Failure::new(
            "invalid-params",
            "an upload is named by 1 to 64 letters, digits, _ or -",
        ));
    }
    Ok(())
}

/// Removes every upload a daemon that stopped left behind: their messages never come.
pub fn clear(daemon: &Daemon) {
    let _ = std::fs::remove_dir_all(daemon.paths.uploads());
}

/// The uploads, shared with the task that drops each one late.
type Held = Arc<Mutex<HashMap<(String, String), Upload>>>;

/// `agent.upload`: one chunk kept, `{"upload", "file", "received"}`.
pub async fn chunk(daemon: &Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let asked: AgentUpload = serde_json::from_value(params)?;
    let sender = sender();
    let (daemon, held) = (daemon.clone(), daemon.uploads.0.clone());
    let kept = held.clone();
    let (answer, made) = tokio::task::spawn_blocking(move || keep(&daemon, &kept, &sender, asked))
        .await
        .map_err(|e| Failure::new("unavailable", format!("the chunk was not kept: {e}")))??;
    if let Some(key) = made {
        tokio::spawn(drop_when_late(held, key));
    }
    Ok(answer)
}

/// Drops the upload once no chunk came for [`lasts`] and its message did not take it.
async fn drop_when_late(held: Held, key: (String, String)) {
    let lasts = lasts();
    let mut wait = lasts;
    loop {
        tokio::time::sleep(wait).await;
        let mut uploads = held.lock().unwrap();
        let Some(upload) = uploads.get(&key) else {
            return;
        };
        let idle = upload.last.elapsed();
        if idle >= lasts {
            // The folder goes under the guard: a chunk that starts an upload of the same name
            // waits for it, and never finds this one's folder to lose.
            let upload = uploads.remove(&key).expect("it is there");
            let _ = std::fs::remove_dir_all(&upload.folder);
            return;
        }
        wait = lasts - idle;
    }
}

/// The chunk judged and written: the answer, and the upload's key when the chunk made it.
fn keep(
    daemon: &Daemon,
    held: &Held,
    sender: &str,
    asked: AgentUpload,
) -> Result<(Value, Option<(String, String)>), Failure> {
    use base64::Engine as _;
    named(&asked.upload)?;
    let kind = Kind::of(asked.kind);
    let (number, said) = (asked.file, kind.said());
    let (most, most_bytes) = match kind {
        Kind::Picture => (MAX_IMAGES, MAX_IMAGE_BYTES),
        Kind::Text => (pasted::MAX_TEXTS, pasted::MAX_TEXT_BYTES),
    };
    if number == 0 || number as usize > most {
        return Err(Failure::refused(format!(
            "a message carries {most} {said}s at most"
        )));
    }
    match (kind, asked.mime.as_deref()) {
        (Kind::Picture, Some(mime)) if IMAGE_TYPES.iter().any(|(taken, _)| *taken == mime) => {}
        (Kind::Picture, _) => {
            return Err(Failure::refused(format!(
                "{said} {number} is of a type not taken: png, jpeg, gif and webp are"
            )));
        }
        (Kind::Text, None) => {}
        (Kind::Text, Some(_)) => {
            return Err(Failure::new("invalid-params", "a pasted text has no mime"));
        }
    }
    if asked.size == 0 || asked.size > most_bytes as u64 {
        return Err(Failure::refused(format!(
            "{said} {number} is empty or over {} MiB",
            most_bytes / 1024 / 1024
        )));
    }
    // Judged before it is decoded: a huge request is not turned into bytes to be refused.
    if asked.data.len() > CHUNK.div_ceil(3) * 4 {
        return Err(Failure::refused(format!(
            "{said} {number}: a chunk is {} KiB at most",
            CHUNK / 1024
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&asked.data)
        .map_err(|_| Failure::refused(format!("{said} {number}: a chunk is not valid base64")))?;
    if bytes.is_empty() || bytes.len() > CHUNK {
        return Err(Failure::refused(format!(
            "{said} {number}: a chunk is empty or over {} KiB",
            CHUNK / 1024
        )));
    }

    let mut uploads = held.lock().unwrap();
    let key = (sender.to_owned(), asked.upload.clone());
    let made = !uploads.contains_key(&key);
    if made {
        if uploads.keys().filter(|(of, _)| of == sender).count() >= PER_SENDER {
            return Err(Failure::refused(format!(
                "{PER_SENDER} uploads of this device wait for their messages already: send them, \
                 or wait {} s for them to be dropped",
                lasts().as_secs()
            )));
        }
        if uploads.len() >= PER_MACHINE {
            return Err(Failure::refused(
                "busy: this machine holds as many uploads as it takes: try again in a while",
            ));
        }
    }
    let expected = uploads
        .get(&key)
        .and_then(|upload| upload.files.get(&(kind, number)))
        .map_or(0, |file| file.received);
    if let Some(file) = uploads
        .get(&key)
        .and_then(|upload| upload.files.get(&(kind, number)))
        && (file.size != asked.size || file.mime != asked.mime)
    {
        return Err(Failure::refused(format!(
            "{said} {number} changed its size or its type between its chunks"
        )));
    }
    if asked.offset != expected {
        return Err(Failure::refused(format!(
            "{said} {number}: its next chunk starts at offset {expected}"
        )));
    }
    let received = expected + bytes.len() as u64;
    if received > asked.size {
        return Err(Failure::refused(format!(
            "{said} {number} is longer than the size it said"
        )));
    }
    let folder = match uploads.get(&key) {
        Some(upload) => upload.folder.clone(),
        None => daemon
            .paths
            .uploads()
            .join(format!("{sender}-{}", asked.upload)),
    };
    // Recorded before anything is written: whatever the write leaves on disk is then known, and
    // bound, by the table.
    let upload = uploads.entry(key.clone()).or_insert_with(|| Upload {
        folder: folder.clone(),
        files: BTreeMap::new(),
        last: Instant::now(),
    });
    upload.last = Instant::now();
    if let Err(error) = write(&folder, kind, number, expected, &bytes) {
        // A chunk that failed may have written a part of itself: the upload is not trusted any
        // more, nor its folder; the sender sends it again from its start.
        uploads.remove(&key);
        let _ = std::fs::remove_dir_all(&folder);
        return Err(Failure::new(
            "unavailable",
            format!("{said} {number} cannot be kept: {error}"),
        ));
    }
    let upload = uploads.get_mut(&key).expect("recorded above");
    upload
        .files
        .entry((kind, number))
        .or_insert(File {
            mime: asked.mime,
            size: asked.size,
            received: 0,
        })
        .received = received;
    let answer = json!({"upload": asked.upload, "file": number, "received": received});
    Ok((answer, made.then_some(key)))
}

/// The chunk `bytes` of a file written at its end in `folder`, which is made if it is not there.
/// The first chunk makes the file and refuses one that is there: what a stopped daemon left, or
/// what an upload dropped just before left, is never a start to append to. Never through a link.
fn write(
    folder: &std::path::Path,
    kind: Kind,
    number: u32,
    expected: u64,
    bytes: &[u8],
) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    ensure_private_dir(folder).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut options = std::fs::OpenOptions::new();
    if expected == 0 {
        options.write(true).create_new(true);
    } else {
        options.append(true);
    }
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(folder.join(file_name(kind, number)))?
        .write_all(bytes)
}

/// The file of a picture or a text of an upload: this daemon's name, never the sender's.
fn file_name(kind: Kind, number: u32) -> String {
    match kind {
        Kind::Picture => format!("picture-{number}"),
        Kind::Text => format!("text-{number}"),
    }
}

/// The pictures and texts of the upload `upload`, which its message takes, as `agent.send` takes
/// them inline: the upload is gone once they are read. `not-found` when the sender has no such
/// upload; `refused` when a file of it arrived in part, the upload then kept for the rest.
pub async fn take(
    daemon: &Arc<Daemon>,
    upload: &str,
) -> Result<(Vec<ImageUpload>, Vec<TextUpload>), Failure> {
    named(upload)?;
    let key = (sender(), upload.to_owned());
    let held = daemon.uploads.0.clone();
    tokio::task::spawn_blocking(move || read(&held, key))
        .await
        .map_err(|e| Failure::new("unavailable", format!("the upload was not read: {e}")))?
}

fn read(
    held: &Held,
    key: (String, String),
) -> Result<(Vec<ImageUpload>, Vec<TextUpload>), Failure> {
    use base64::Engine as _;
    let upload = {
        let mut uploads = held.lock().unwrap();
        let Some(upload) = uploads.get(&key) else {
            return Err(Failure::new(
                "not-found",
                format!(
                    "no upload {} waits for its message: one is dropped {} s after its last chunk",
                    key.1,
                    lasts().as_secs()
                ),
            ));
        };
        if let Some(((kind, number), file)) = upload
            .files
            .iter()
            .find(|(_, file)| file.received < file.size)
        {
            return Err(Failure::refused(format!(
                "{} {number} arrived in part: {} of its {} bytes",
                kind.said(),
                file.received,
                file.size
            )));
        }
        uploads.remove(&key).expect("it is there")
    };
    let taken = (|| {
        let (mut images, mut texts) = (Vec::new(), Vec::new());
        for ((kind, number), file) in &upload.files {
            let bytes =
                std::fs::read(upload.folder.join(file_name(*kind, *number))).map_err(|e| {
                    Failure::new(
                        "unavailable",
                        format!("{} {number} cannot be read: {e}", kind.said()),
                    )
                })?;
            match kind {
                Kind::Picture => images.push(ImageUpload {
                    mime: file.mime.clone().unwrap_or_default(),
                    data: base64::engine::general_purpose::STANDARD.encode(bytes),
                }),
                Kind::Text => texts.push(TextUpload {
                    name: pasted::NAME.to_owned(),
                    text: String::from_utf8(bytes).map_err(|_| {
                        Failure::refused(format!("pasted text {number} is not UTF-8 text"))
                    })?,
                }),
            }
        }
        Ok((images, texts))
    })();
    let _ = std::fs::remove_dir_all(&upload.folder);
    taken
}
