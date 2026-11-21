//! The pictures and videos an agent of another machine showed (`docs/remote.md`, "A file an agent
//! showed"): the window asks that machine for them with `agent.file`, signed by this device and
//! carried by this machine's daemon, a chunk at a time, named by the session and the event that
//! showed the file, never by its path. A picture is drawn in its line once it is in; Open (a
//! picture) and Play (a video) hand the system's default handler a copy written on this machine,
//! as soon as its first chunk is in, the rest arriving into it while it plays.
//!
//! The kit has no video element: a video is played by the user's own player, never in the window.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, WeakEntity};
use serde_json::{Value, json};

use crate::conversation::ShownLink;
use crate::link::{Conn, Connect};
use crate::shell::Shell;

/// How long one chunk may take: half a megabyte, paced by the machine that serves it.
const CHUNK_WITHIN: Duration = Duration::from_secs(30);
/// How many times a chunk is asked again of a machine that is busy, half a second apart.
const BUSY_TRIES: u32 = 20;
/// The most of a picture read for its thumbnail, as for a picture of this machine.
const MOST_PICTURE: u64 = 20 * 1024 * 1024;

/// Whether a target is a video, by its extension: it gets Play, and no thumbnail.
pub fn is_video(target: &str) -> bool {
    matches!(
        Path::new(target)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mp4" | "m4v" | "webm" | "mkv" | "mov" | "ogv")
    )
}

/// How the window reaches the machine shown, and where the copies it opens are written.
pub struct ShownFiles {
    connect: Connect,
    shell: WeakEntity<Shell>,
    /// A folder of this window's own, readable by the user alone, removed with the window; none
    /// when no private folder could be made: nothing is then copied.
    folder: Option<PathBuf>,
    copies: Arc<std::sync::Mutex<Copies>>,
}

impl gpui_kit::Global for ShownFiles {}

impl ShownFiles {
    pub fn new(connect: Connect, shell: WeakEntity<Shell>) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let temp = std::env::temp_dir();
        // What earlier runs left, in every place a window of Flottille ever wrote: its own
        // naming only, and only what is the user's.
        let base = temp.join(format!("flottille-{}", unsafe { libc::geteuid() }));
        for parent in runtime.iter().chain([&base, &temp]) {
            sweep(parent, running);
        }
        let folder = private_folder(runtime.as_deref(), &temp, std::process::id(), stamp).ok();
        ShownFiles {
            connect,
            shell,
            folder,
            copies: Arc::default(),
        }
    }
}

impl Drop for ShownFiles {
    fn drop(&mut self) {
        if let Some(folder) = &self.folder {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

/// The connection that asks the machine shown for its files, when the window has one.
pub fn connect(cx: &App) -> Option<Connect> {
    cx.try_global::<ShownFiles>()
        .map(|files| files.connect.clone())
}

/// Reads the file `link` names from its machine, chunk by chunk from `offset`, giving each to
/// `each` with the file's size until it says stop or the file ends.
fn fetch(
    conn: &mut Conn,
    link: &ShownLink,
    mut offset: u64,
    mut each: impl FnMut(&[u8], u64) -> Result<bool, String>,
) -> Result<u64, String> {
    use base64::Engine as _;
    loop {
        // A machine that is serving as much as it takes says it is busy: asked again shortly.
        let mut tries = 0;
        let answer: Value = loop {
            match conn.ask_within(
                "agent.file",
                json!({"agent": link.agent, "shown": link.seq, "offset": offset}),
                CHUNK_WITHIN,
            )? {
                Err(why) if why.contains("busy:") && tries < BUSY_TRIES => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(500));
                }
                answer => break answer?,
            }
        };
        let size = answer["size"].as_u64().ok_or("the machine sent no size")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(answer["data"].as_str().unwrap_or_default())
            .map_err(|_| "the machine sent what is no file".to_owned())?;
        offset += bytes.len() as u64;
        if bytes.is_empty() || !each(&bytes, size)? || offset >= size {
            return Ok(offset);
        }
    }
}

/// The picture `link` names, from its machine, as a thumbnail of `pixels`: none when it cannot be
/// read, is too large or is no picture.
pub fn picture(
    connect: &Connect,
    link: &ShownLink,
    mime: &str,
    pixels: (u32, u32),
) -> Option<Arc<gpui_kit::Image>> {
    let mut conn = Conn::new(connect().ok()?).ok()?;
    let mut bytes = Vec::new();
    fetch(&mut conn, link, 0, |chunk, size| {
        if size > MOST_PICTURE {
            return Err("too large".to_owned());
        }
        bytes.extend_from_slice(chunk);
        Ok(true)
    })
    .ok()?;
    let small = flottille_core::images::thumbnail(mime, &bytes, pixels).ok()?;
    Some(Arc::new(gpui_kit::Image::from_bytes(
        gpui_kit::ImageFormat::Png,
        small,
    )))
}

/// A copy of `link` being written on this machine: its first chunk is in.
struct Begun {
    conn: Conn,
    file: std::fs::File,
    offset: u64,
    size: u64,
}

/// Writes the first chunk of `link` into `path`, a new file made for the user alone: one that is
/// there already, or a link in its place, is an error, never written through.
fn begin(connect: &Connect, link: &ShownLink, path: &Path) -> Result<Begun, String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .mode(0o600)
        .open(path)
        .map_err(|error| error.to_string())?;
    let mut conn = Conn::new(connect().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let mut size = 0;
    let offset = fetch(&mut conn, link, 0, |chunk, whole| {
        size = whole;
        file.write_all(chunk).map_err(|error| error.to_string())?;
        Ok(false)
    })?;
    file.flush().map_err(|error| error.to_string())?;
    Ok(Begun {
        conn,
        file,
        offset,
        size,
    })
}

/// The rest of a copy begun, written as it arrives.
fn rest(begun: Begun, link: &ShownLink) -> Result<(), String> {
    let Begun {
        mut conn,
        mut file,
        offset,
        size,
    } = begun;
    if offset >= size {
        return Ok(());
    }
    fetch(&mut conn, link, offset, |chunk, _| {
        file.write_all(chunk).map_err(|error| error.to_string())?;
        Ok(true)
    })?;
    Ok(())
}

/// Open (a picture) or Play (a video) of a file of another machine: a copy is written on this
/// machine, the system's handler is given it once its first chunk is in, and the rest follows.
/// A link opened again is given the copy it has, never written over or fetched again.
pub fn open(link: &ShownLink, cx: &mut App) {
    let Some(files) = cx.try_global::<ShownFiles>() else {
        return;
    };
    if !is_plain_agent(&link.agent) {
        crate::tunnels::failed("that machine named a session in a way that is no name", cx);
        return;
    }
    let Some(folder) = files.folder.clone() else {
        crate::tunnels::failed("no private folder to copy it into", cx);
        return;
    };
    let (connect, shell, copies) = (
        files.connect.clone(),
        files.shell.clone(),
        files.copies.clone(),
    );
    let name = Path::new(&link.target)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let path = folder.join(format!("{}-{}-{name}", link.agent, link.seq));
    let claim = copies.lock().unwrap().claim(&path);
    match claim {
        Claim::Wait => return,
        Claim::Open => {
            shell.update(cx, |shell, _| shell.open_local(&path)).ok();
            return;
        }
        Claim::Fetch => {}
    }
    let link = link.clone();
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let (to, of) = (path.clone(), link.clone());
        let begun = {
            let connect = connect.clone();
            executor
                .spawn(async move { begin(&connect, &of, &to) })
                .await
        };
        let begun = match begun {
            Ok(begun) => begun,
            Err(why) => {
                copies.lock().unwrap().failed(&path);
                let _ = std::fs::remove_file(&path);
                cx.update(|cx| crate::tunnels::failed(&why, cx));
                return;
            }
        };
        copies.lock().unwrap().begun(&path);
        shell.update(cx, |shell, _| shell.open_local(&path)).ok();
        match executor.spawn(async move { rest(begun, &link) }).await {
            Ok(()) => copies.lock().unwrap().complete(&path),
            Err(why) => {
                copies.lock().unwrap().failed(&path);
                let _ = std::fs::remove_file(&path);
                cx.update(|cx| {
                    gpui_design_system::toast(
                        gpui_design_system::Toast::failure(format!(
                            "Not all of {name} arrived: {why}",
                            name = path.file_name().and_then(|n| n.to_str()).unwrap_or("it")
                        )),
                        cx,
                    )
                });
            }
        }
    })
    .detach();
}

/// A machine's agent id goes into a file name only when it is a plain name: what the remote
/// sends is never taken as a path.
pub fn is_plain_agent(agent: &str) -> bool {
    (1..=64).contains(&agent.len())
        && agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// What this window's folders are called, followed by the pid and a stamp.
const FOLDER: &str = "flottille-shown-";

/// A folder of `parent` made now, for the user alone: made one level only, never through a link.
fn make_private(parent: &Path, name: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt as _;
    let path = parent.join(name);
    std::fs::DirBuilder::new().mode(0o700).create(&path)?;
    Ok(path)
}

/// Whether `path` is a real folder (no link) of the user, closed to everyone else.
fn is_the_users_alone(path: &Path) -> bool {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    std::fs::symlink_metadata(path).is_ok_and(|meta| {
        meta.is_dir()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.permissions().mode() & 0o077 == 0
    })
}

/// The folder this window writes its copies in, made now: in `runtime` (`$XDG_RUNTIME_DIR`) when
/// the user has one, else in a folder of `temp` made for the user alone (made, or checked to be
/// a folder of the user's that nobody else can enter), never one another user could have
/// prepared.
pub fn private_folder(
    runtime: Option<&Path>,
    temp: &Path,
    pid: u32,
    stamp: u128,
) -> std::io::Result<PathBuf> {
    let name = format!("{FOLDER}{pid}-{stamp}");
    if let Some(runtime) = runtime.filter(|runtime| is_the_users_alone(runtime)) {
        return make_private(runtime, &name);
    }
    let base = temp.join(format!("flottille-{}", unsafe { libc::geteuid() }));
    match make_private(temp, &format!("flottille-{}", unsafe { libc::geteuid() })) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if !is_the_users_alone(&base) {
        return Err(std::io::Error::other(format!(
            "{} is not a folder of yours that no one else can enter",
            base.display()
        )));
    }
    make_private(&base, &name)
}

/// Removes, in `parent`, the folders of this window's naming that earlier runs left: real folders
/// of the user's, of a window that is not running any more (`alive` tells by its pid). A link, a
/// file and everything of another name stay.
pub fn sweep(parent: &Path, alive: impl Fn(u32) -> bool) {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let me = unsafe { libc::geteuid() };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|name| name.strip_prefix(FOLDER)) else {
            continue;
        };
        let Some(pid) = rest
            .split('-')
            .next()
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let owned = entry
            .metadata()
            .is_ok_and(|meta| meta.is_dir() && meta.uid() == me);
        if owned && !alive(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether a process of that pid runs.
fn running(pid: u32) -> bool {
    pid == std::process::id() || Path::new(&format!("/proc/{pid}")).exists()
}

/// What an Open of a link finds: its copy to fetch, being fetched, or there.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// Nobody made it: this Open does, and opens it once its first chunk is in.
    Fetch,
    /// Its first chunk is not in yet: the Open that began it will open it.
    Wait,
    /// It is there, whole or arriving: open it as it is, write nothing.
    Open,
}

#[derive(Clone, Copy)]
enum State {
    Starting,
    Writing,
    Complete,
}

/// The copies this window made or is making, so two Opens of a link share one and a whole one is
/// not fetched again.
#[derive(Default)]
pub struct Copies(std::collections::HashMap<PathBuf, State>);

impl Copies {
    pub fn claim(&mut self, path: &Path) -> Claim {
        match self.0.get(path) {
            None => {
                self.0.insert(path.to_owned(), State::Starting);
                Claim::Fetch
            }
            Some(State::Starting) => Claim::Wait,
            Some(State::Writing | State::Complete) => Claim::Open,
        }
    }

    /// The first chunk is written.
    pub fn begun(&mut self, path: &Path) {
        self.0.insert(path.to_owned(), State::Writing);
    }

    /// Every chunk is written.
    pub fn complete(&mut self, path: &Path) {
        self.0.insert(path.to_owned(), State::Complete);
    }

    /// The copy is not whole and will not be: the next Open makes it again.
    pub fn failed(&mut self, path: &Path) {
        self.0.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_id_of_a_remote_goes_into_a_file_name_only_when_plain() {
        assert!(is_plain_agent("agent-7_B"));
        assert!(is_plain_agent(&"a".repeat(64)));
        assert!(!is_plain_agent(&"a".repeat(65)));
        assert!(!is_plain_agent(""));
        assert!(!is_plain_agent("../../etc"));
        assert!(!is_plain_agent("a/b"));
        assert!(!is_plain_agent("a b"));
        assert!(!is_plain_agent("a\u{e9}"));
        assert!(!is_plain_agent(".."));
    }

    #[test]
    fn copies_go_in_the_runtime_folder_else_in_a_private_one_of_the_temp_folder() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("run");
        let temp = root.path().join("tmp");
        std::fs::create_dir_all(&runtime).unwrap();
        // As the system makes the user's runtime folder.
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir_all(&temp).unwrap();

        let folder = private_folder(Some(&runtime), &temp, 7, 1).unwrap();
        assert_eq!(folder.parent(), Some(runtime.as_path()));
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );

        // No runtime folder: a folder of the user's, 0700, in the temp folder, then the window's.
        let folder = private_folder(None, &temp, 7, 1).unwrap();
        let base = folder.parent().unwrap();
        assert_eq!(base.parent(), Some(temp.as_path()));
        assert_eq!(
            std::fs::metadata(base).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // A second window shares the base.
        assert_eq!(
            private_folder(None, &temp, 8, 2).unwrap().parent(),
            Some(base)
        );
    }

    #[test]
    fn a_folder_of_the_temp_folder_that_is_not_the_users_alone_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let temp = root.path().join("tmp");
        std::fs::create_dir_all(&temp).unwrap();
        let folder = private_folder(None, &temp, 7, 1).unwrap();
        let base = folder.parent().unwrap().to_owned();

        // Opened up to others since: not trusted.
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_folder(None, &temp, 8, 2).is_err());
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(private_folder(None, &temp, 8, 2).is_ok());

        // A link put in its place by another user is not followed.
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &base).unwrap();
        assert!(private_folder(None, &temp, 9, 3).is_err());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn the_leftovers_of_earlier_runs_are_removed_when_the_window_starts() {
        let parent = tempfile::tempdir().unwrap();
        let at = |name: &str| parent.path().join(name);
        // Earlier runs, whose window is gone (pid 11 and 12 are not alive).
        for name in ["flottille-shown-11-100", "flottille-shown-12-200"] {
            std::fs::create_dir_all(at(name)).unwrap();
            std::fs::write(at(name).join("1-2-shot.png"), "x").unwrap();
        }
        // A window still running, another folder, a file of the naming and a link of the naming.
        std::fs::create_dir_all(at("flottille-shown-99-300")).unwrap();
        std::fs::create_dir_all(at("something-else")).unwrap();
        std::fs::write(at("flottille-shown-13-400"), "a file").unwrap();
        let aside = parent.path().join("aside");
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("keep"), "x").unwrap();
        std::os::unix::fs::symlink(&aside, at("flottille-shown-14-500")).unwrap();

        sweep(parent.path(), |pid| pid == 99);

        assert!(!at("flottille-shown-11-100").exists());
        assert!(!at("flottille-shown-12-200").exists());
        assert!(at("flottille-shown-99-300").exists(), "a running window's");
        assert!(at("something-else").exists());
        assert!(at("flottille-shown-13-400").exists());
        assert!(aside.join("keep").exists(), "a link is never followed");
    }

    #[test]
    fn two_opens_of_a_link_share_one_copy_and_a_complete_one_is_not_fetched_again() {
        let path = Path::new("/run/flottille-shown-1-1/a-7-clip.mp4");
        let mut copies = Copies::default();
        assert_eq!(copies.claim(path), Claim::Fetch);
        // A second click while the first chunk is not in: nothing is written over the copy.
        assert_eq!(copies.claim(path), Claim::Wait);
        copies.begun(path);
        // While it is written, the player is given the same file, never a truncated one.
        assert_eq!(copies.claim(path), Claim::Open);
        copies.complete(path);
        assert_eq!(copies.claim(path), Claim::Open);
        // Another link has its own copy.
        assert_eq!(
            copies.claim(Path::new("/run/flottille-shown-1-1/a-8-clip.mp4")),
            Claim::Fetch
        );
    }

    #[test]
    fn a_copy_that_failed_is_fetched_again_by_the_next_open() {
        let path = Path::new("/run/flottille-shown-1-1/a-7-clip.mp4");
        let mut copies = Copies::default();
        assert_eq!(copies.claim(path), Claim::Fetch);
        copies.failed(path);
        assert_eq!(copies.claim(path), Claim::Fetch);
        copies.begun(path);
        copies.failed(path);
        assert_eq!(copies.claim(path), Claim::Fetch);
    }

    #[test]
    fn a_video_is_told_by_its_extension() {
        assert!(is_video("/w/clip.MP4"));
        assert!(is_video("/w/clip.webm"));
        assert!(!is_video("/w/shot.png"));
        assert!(!is_video("/w/clip.mp4.sh"));
    }
}
