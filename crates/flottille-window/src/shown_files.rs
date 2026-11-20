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
    /// A folder of this window's own, readable by the user alone, removed with the window.
    folder: PathBuf,
}

impl gpui_kit::Global for ShownFiles {}

impl ShownFiles {
    pub fn new(connect: Connect, shell: WeakEntity<Shell>) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let folder =
            std::env::temp_dir().join(format!("flottille-shown-{}-{stamp}", std::process::id()));
        ShownFiles {
            connect,
            shell,
            folder,
        }
    }
}

impl Drop for ShownFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
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
        let answer: Value = conn.ask_within(
            "agent.file",
            json!({"agent": link.agent, "shown": link.seq, "offset": offset}),
            CHUNK_WITHIN,
        )??;
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

/// Writes the first chunk of `link` into `path`, made for the user alone.
fn begin(connect: &Connect, link: &ShownLink, path: &Path) -> Result<Begun, String> {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    let folder = path.parent().ok_or("no folder")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
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
pub fn open(link: &ShownLink, cx: &mut App) {
    let Some(files) = cx.try_global::<ShownFiles>() else {
        return;
    };
    let (connect, shell) = (files.connect.clone(), files.shell.clone());
    let name = Path::new(&link.target)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let path = files
        .folder
        .join(format!("{}-{}-{name}", link.agent, link.seq));
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
                cx.update(|cx| crate::tunnels::failed(&why, cx));
                return;
            }
        };
        shell.update(cx, |shell, _| shell.open_local(&path)).ok();
        if let Err(why) = executor.spawn(async move { rest(begun, &link) }).await {
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
    })
    .detach();
}

/// A machine's agent id goes into a file name only when it is a plain name.
pub fn is_plain_agent(_agent: &str) -> bool {
    todo!("round 2, point 5")
}

/// The folder this window writes its copies in, made now: in `runtime` (`$XDG_RUNTIME_DIR`) when
/// the user has one, else in a folder of `temp` made for the user alone.
pub fn private_folder(
    _runtime: Option<&Path>,
    _temp: &Path,
    _pid: u32,
    _stamp: u128,
) -> std::io::Result<PathBuf> {
    todo!("round 2, point 3")
}

/// Removes, in `parent`, the folders of this window's naming that earlier runs left.
pub fn sweep(_parent: &Path, _alive: impl Fn(u32) -> bool) {
    todo!("round 2, point 4")
}

/// What an Open of a link finds: its copy to fetch, being fetched, or there.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    Fetch,
    Wait,
    Open,
}

/// The copies this window made or is making.
#[derive(Default)]
pub struct Copies;

impl Copies {
    pub fn claim(&mut self, _path: &Path) -> Claim {
        todo!("round 2, point 4")
    }
    pub fn begun(&mut self, _path: &Path) {
        todo!("round 2, point 4")
    }
    pub fn complete(&mut self, _path: &Path) {
        todo!("round 2, point 4")
    }
    pub fn failed(&mut self, _path: &Path) {
        todo!("round 2, point 4")
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
