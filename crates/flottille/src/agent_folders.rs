//! The agent folders of a place (`.claude`, `.claude.json`, `.codex`, wherever they are in it) as
//! they were before an agent's run, put back as they were after it. A line may write there
//! without naming a path in one (a patch applied, a branch checked out or merged, a stash popped,
//! a folder copied, an archive unpacked): the gate, which refuses every write of such a folder it
//! sees, cannot see those, so what the run changed there is undone once it ends and the run's
//! result says so (`docs/design.md` §7.1).

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::gate::{AGENT_FOLDERS, AGENT_LOGINS};

/// The most entries of the place looked at: past it, the rest of the place is not, and the
/// result says so.
#[cfg(not(test))]
const ENTRIES_MAX: usize = 500_000;
#[cfg(test)]
const ENTRIES_MAX: usize = 200;

/// The largest file of an agent folder kept to be put back: a larger one changed is said, not
/// put back.
const KEPT_MAX: u64 = 1 << 20;

/// What an entry of an agent folder is.
#[derive(Clone, PartialEq)]
enum Kind {
    Folder,
    /// What tells a change: a file rewritten with the same size and time has a new `ctime`.
    File {
        len: u64,
        modified: (i64, i64),
        changed: (i64, i64),
        inode: u64,
        mode: u32,
    },
    Link(PathBuf),
    /// A pipe, a socket or a device: never opened, and never made again.
    Special,
}

/// The agent folders of a place before a run.
pub struct Snapshot {
    root: PathBuf,
    /// The home, whose own agent folders are left out when the place holds it: the user's, which
    /// their own agents write while the run goes on, and a login's place, never read.
    home: Option<PathBuf>,
    /// Where the put-back keeps what it removes, one folder per run: never looked at, when the
    /// place holds it (a project that is the home).
    kept: PathBuf,
    /// Relative to `root`.
    entries: BTreeMap<PathBuf, Kind>,
    /// Whether every entry of the place was looked at.
    complete: bool,
    /// The content of each file, when it is not too large to keep.
    contents: BTreeMap<PathBuf, Vec<u8>>,
}

/// What `Snapshot::restore` found the run changed, relative to the place.
#[derive(Debug, Default, PartialEq)]
pub struct Restored {
    pub put_back: Vec<String>,
    /// Changed, but too large to have been kept, below a folder that is no longer a real one, or
    /// the disk refused to put it back.
    pub left: Vec<String>,
    /// Where what was removed or overwritten was moved, when there was any.
    pub kept: Option<PathBuf>,
    /// The place has more entries than are looked at: what is past them was not checked.
    pub unchecked: bool,
}

impl Snapshot {
    /// The agent folders of `root`, the home's and `kept` left out.
    pub fn take(root: &Path, home: Option<&Path>, kept: &Path) -> Snapshot {
        let root = real(root);
        let home = home.map(real);
        let kept = match (kept.parent(), kept.file_name()) {
            (Some(parent), Some(name)) => real(parent).join(name),
            _ => kept.to_owned(),
        };
        let (entries, complete) = walk(&root, home.as_deref(), &kept);
        let contents = entries
            .iter()
            .filter_map(|(path, kind)| match kind {
                Kind::File { len, .. } if *len <= KEPT_MAX => {
                    Some((path.clone(), read_file(&root.join(path)).ok()?))
                }
                _ => None,
            })
            .collect();
        Snapshot {
            root,
            home,
            kept,
            entries,
            complete,
            contents,
        }
    }

    /// Puts back what changed in the agent folders since the snapshot: what was added is
    /// removed, what was changed or removed is written again. What is removed or overwritten is
    /// moved first to the folder `run` of the snapshot's `kept`, never lost.
    pub fn restore(&self, run: &str) -> Restored {
        let kept = &self.kept.join(run);
        let (after, complete) = walk(&self.root, self.home.as_deref(), &self.kept);
        let mut restored = Restored {
            unchecked: !self.complete || !complete,
            ..Restored::default()
        };
        let shown = |path: &Path| path.display().to_string();
        // What was added, the deepest first; a folder is said only when nothing below it is.
        // Past the bound, what the snapshot did not see may have been there before.
        for (path, kind) in after.iter().rev().filter(|_| self.complete) {
            if self.entries.contains_key(path) {
                continue;
            }
            let removed = match kind {
                Kind::Folder => self.below(path).and_then(std::fs::remove_dir),
                _ => self.keep(path, kept, &mut restored.kept),
            };
            let said = *kind != Kind::Folder
                || !after
                    .keys()
                    .any(|below| below != path && below.starts_with(path));
            match (removed, said) {
                (Ok(()), true) => restored.put_back.push(shown(path)),
                // A folder still holding what is never touched (a login file) stays.
                (Ok(()), false) | (Err(_), false) => {}
                (Err(_), true) if *kind == Kind::Folder => {}
                (Err(_), true) => restored.left.push(shown(path)),
            }
        }
        // What was changed or removed, the shallowest first.
        for (path, kind) in &self.entries {
            // Past the bound, an entry not seen after the run is looked at alone.
            let looked;
            let now = match after.get(path) {
                None if !complete => {
                    looked = self.below(path).ok().and_then(|at| kind_of(&at));
                    looked.as_ref()
                }
                now => now,
            };
            if now == Some(kind) {
                continue;
            }
            match (kind, now) {
                (Kind::Folder, Some(Kind::Folder)) | (Kind::Folder, None) => continue,
                (Kind::Special, Some(Kind::Special)) => continue,
                (Kind::File { .. }, Some(Kind::File { .. }))
                    if self.contents.get(path).is_some_and(|before| {
                        self.below(path)
                            .and_then(|at| read_file(&at))
                            .is_ok_and(|content| content == *before)
                    }) =>
                {
                    continue;
                }
                _ => {}
            }
            let put = match now {
                Some(Kind::Folder) => self.below(path).and_then(std::fs::remove_dir),
                Some(_) => self.keep(path, kept, &mut restored.kept),
                None => Ok(()),
            }
            .and_then(|()| self.write(path, kind));
            match put {
                Ok(()) => restored.put_back.push(shown(path)),
                Err(_) => restored.left.push(shown(path)),
            }
        }
        restored
    }

    /// Where `path` is on the disk, once each folder above it, below the place, is a real one: a
    /// folder made a link since would lead the put-back elsewhere (the home).
    fn below(&self, path: &Path) -> std::io::Result<PathBuf> {
        let mut at = self.root.clone();
        let mut parts = path.components().peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                return Ok(at.join(part));
            }
            at.push(part);
            if !std::fs::symlink_metadata(&at)?.file_type().is_dir() {
                return Err(std::io::Error::other("no longer a folder"));
            }
        }
        Ok(at)
    }

    /// Moves the entry at `path` below `kept`, where the user finds what the put-back removed.
    fn keep(&self, path: &Path, kept: &Path, said: &mut Option<PathBuf>) -> std::io::Result<()> {
        let at = self.below(path)?;
        let to = kept.join(path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        *said = Some(kept.to_owned());
        match std::fs::rename(&at, &to) {
            Err(error) if error.kind() == ErrorKind::CrossesDevices => {}
            moved => return moved,
        }
        // On another disk, its copy is made first.
        let kind = std::fs::symlink_metadata(&at)?.file_type();
        if kind.is_file() {
            write_new(&to, &read_file(&at)?, 0o600)?;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&at)?, &to)?;
        }
        std::fs::remove_file(&at)
    }

    /// Writes an entry as it was, making the folders above it that are gone.
    fn write(&self, path: &Path, kind: &Kind) -> std::io::Result<()> {
        let mut made = PathBuf::new();
        for part in path.parent().into_iter().flat_map(Path::components) {
            made.push(part);
            match self.below(&made) {
                Ok(at) if std::fs::symlink_metadata(&at).is_ok() => {}
                Ok(at) => std::fs::create_dir(&at)?,
                Err(error) => return Err(error),
            }
        }
        let at = self.below(path)?;
        if std::fs::symlink_metadata(&at).is_ok_and(|meta| !meta.is_dir()) {
            return Err(std::io::Error::other("something else is there"));
        }
        match kind {
            Kind::Folder => match std::fs::create_dir(&at) {
                Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
                done => done,
            },
            Kind::Link(target) => std::os::unix::fs::symlink(target, &at),
            Kind::File { mode, .. } => {
                let before = self
                    .contents
                    .get(path)
                    .ok_or_else(|| std::io::Error::other("too large to have been kept"))?;
                write_new(&at, before, *mode)
            }
            Kind::Special => Err(std::io::Error::other("never made again")),
        }
    }
}

/// `path` with its links followed, as the walk and the home are compared.
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// Whether a name is an agent's folder, in any case: a file system may not tell them apart.
fn agent_folder(name: &OsStr) -> bool {
    AGENT_FOLDERS.iter().any(|folder| {
        name.as_encoded_bytes()
            .eq_ignore_ascii_case(folder.as_bytes())
    })
}

/// Whether a name below an agent folder is a login's, never read.
fn login(name: &OsStr) -> bool {
    AGENT_LOGINS.iter().any(|file| {
        name.as_encoded_bytes()
            .eq_ignore_ascii_case(file.as_bytes())
    })
}

/// What is at `at`, its last part not followed.
fn kind_of(at: &Path) -> Option<Kind> {
    let meta = std::fs::symlink_metadata(at).ok()?;
    let kind = meta.file_type();
    Some(if kind.is_dir() {
        Kind::Folder
    } else if kind.is_symlink() {
        Kind::Link(std::fs::read_link(at).ok()?)
    } else if kind.is_file() {
        Kind::File {
            len: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
            inode: meta.ino(),
            mode: meta.mode() & 0o7777,
        }
    } else {
        Kind::Special
    })
}

/// The content of the file at `at`: never through a link, never waiting on a pipe put there
/// since it was looked at.
fn read_file(at: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(at)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a file"));
    }
    let mut content = Vec::new();
    file.read_to_end(&mut content)?;
    Ok(content)
}

/// Writes a file that is not there yet, never through a link.
fn write_new(at: &Path, content: &[u8], mode: u32) -> std::io::Result<()> {
    let mut file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(at)?;
    file.write_all(content)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
}

/// Every entry of an agent folder below `root`, relative to it: `.git`, the home's and `kept`
/// left out, links not followed, and no login file of an agent folder, which is never read. Whether every
/// entry of the place was looked at.
fn walk(root: &Path, home: Option<&Path>, kept: &Path) -> (BTreeMap<PathBuf, Kind>, bool) {
    let mut entries = BTreeMap::new();
    let mut seen = 0;
    let mut pending = vec![(PathBuf::new(), false)];
    while let Some((folder, agent)) = pending.pop() {
        let Ok(read) = std::fs::read_dir(root.join(&folder)) else {
            continue;
        };
        for entry in read.flatten() {
            seen += 1;
            if seen > ENTRIES_MAX {
                return (entries, false);
            }
            let name = entry.file_name();
            let path = folder.join(&name);
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let home_s = agent_folder(&name) && home.is_some_and(|home| root.join(&folder) == home);
            if (!agent && name.as_encoded_bytes().eq_ignore_ascii_case(b".git"))
                || home_s
                || root.join(&path) == kept
            {
                continue;
            }
            let agent = agent || agent_folder(&name);
            if agent && login(&name) {
                continue;
            }
            if kind.is_dir() {
                pending.push((path.clone(), agent));
            }
            if !agent {
                continue;
            }
            if let Some(kind) = kind_of(&entry.path()) {
                entries.insert(path, kind);
            }
        }
    }
    (entries, true)
}

/// What the run's result says of the agent folders it changed, once put back.
pub fn said(restored: &Restored) -> Option<String> {
    let mut text = String::new();
    if !restored.put_back.is_empty() || !restored.left.is_empty() {
        text = format!(
            "\nflottille: refused: this run wrote in a repository's agent folder ({}): its \
             settings and hooks run programs the next time the user opens that repository with \
             the agent's own command line, outside the gate: an agent never writes there, in any \
             mode",
            restored
                .put_back
                .iter()
                .chain(&restored.left)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !restored.put_back.is_empty() {
            text.push_str(&format!(
                "; Flottille put back as they were: {}",
                restored.put_back.join(", ")
            ));
        }
        if !restored.left.is_empty() {
            text.push_str(&format!(
                "; Flottille could not put back: {}, tell the user",
                restored.left.join(", ")
            ));
        }
        if let Some(kept) = &restored.kept {
            text.push_str(&format!(
                "; what it removed or replaced there is kept in {}",
                kept.display()
            ));
        }
    }
    if restored.unchecked {
        text.push_str(&format!(
            "{}the agent folders of this place could not all be checked: it holds more than \
             {ENTRIES_MAX} entries, and past them a change there is neither seen nor put back, \
             tell the user",
            if text.is_empty() {
                "\nflottille: refused: "
            } else {
                "; "
            }
        ));
    }
    if text.is_empty() {
        return None;
    }
    if !restored.put_back.is_empty() || !restored.left.is_empty() {
        text.push_str("; ask the user to make the change");
    }
    text.push('\n');
    Some(text)
}

/// The agent folders of each place an agent's run goes on in. Runs that overlap in a place share
/// one snapshot: the first to start takes it, the last to end puts it back. One taken while
/// another run goes on would hold what that run wrote, and put it back.
#[derive(Default)]
pub struct Guards {
    places: std::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<Guarded>>>>,
}

#[derive(Default)]
struct Guarded {
    runs: usize,
    snapshot: Option<Arc<Snapshot>>,
}

/// One run's share of its place's guard: `leave` once it ends.
pub struct Guard {
    place: Arc<tokio::sync::Mutex<Guarded>>,
    /// The run's id: where its put-back keeps what it removes, if it is the last.
    run: String,
    left: bool,
}

impl Guards {
    /// Guards the agent folders of `root` while the run `run` goes on there. `kept`, where a
    /// put-back keeps what it removes, a folder per run.
    pub async fn enter(&self, root: &Path, home: Option<&Path>, kept: &Path, run: &str) -> Guard {
        let root = real(root);
        let place = self
            .places
            .lock()
            .unwrap()
            .entry(root.clone())
            .or_default()
            .clone();
        let mut guarded = place.lock().await;
        if guarded.runs == 0 {
            let (home, kept) = (home.map(Path::to_owned), kept.to_owned());
            guarded.snapshot = tokio::task::spawn_blocking(move || {
                Arc::new(Snapshot::take(&root, home.as_deref(), &kept))
            })
            .await
            .ok();
        }
        guarded.runs += 1;
        drop(guarded);
        Guard {
            place,
            run: run.to_owned(),
            left: false,
        }
    }
}

impl Guard {
    /// The run ended: the last one of its place puts the agent folders back.
    pub async fn leave(mut self) -> Option<Restored> {
        self.left = true;
        leave(self.place.clone(), self.run.clone()).await
    }
}

impl Drop for Guard {
    /// A run that never started still leaves its place.
    fn drop(&mut self) {
        if !self.left {
            tokio::spawn(leave(self.place.clone(), self.run.clone()));
        }
    }
}

async fn leave(place: Arc<tokio::sync::Mutex<Guarded>>, run: String) -> Option<Restored> {
    // Held while it puts back: a run that starts meanwhile takes its snapshot after.
    let mut guarded = place.lock().await;
    guarded.runs -= 1;
    if guarded.runs > 0 {
        return None;
    }
    let snapshot = guarded.snapshot.take()?;
    tokio::task::spawn_blocking(move || snapshot.restore(&run))
        .await
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn what_a_run_adds_changes_or_removes_in_an_agent_folder_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        made(&root.join(".claude/settings.json"), "{}\n");
        made(&root.join("sub/.codex/config.toml"), "model = \"m\"\n");
        made(&root.join("src/main.rs"), "fn main() {}\n");
        let snapshot = Snapshot::take(root, None, &dir.path().join("kept"));

        made(&root.join(".claude/settings.json"), "{\"hooks\": {}}\n");
        made(&root.join(".claude/hooks/start.sh"), "true\n");
        std::fs::remove_file(root.join("sub/.codex/config.toml")).unwrap();
        made(&root.join("src/.claude.json"), "{}\n");
        made(&root.join("src/main.rs"), "fn main() { changed() }\n");
        let restored = snapshot.restore("run");

        assert_eq!(
            std::fs::read_to_string(root.join(".claude/settings.json")).unwrap(),
            "{}\n"
        );
        assert!(!root.join(".claude/hooks").exists());
        assert!(!root.join("src/.claude.json").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("sub/.codex/config.toml")).unwrap(),
            "model = \"m\"\n"
        );
        // What is outside the agent folders stays as the run left it.
        assert!(
            std::fs::read_to_string(root.join("src/main.rs"))
                .unwrap()
                .contains("changed")
        );
        let mut put_back = restored.put_back.clone();
        put_back.sort();
        assert_eq!(
            put_back,
            [
                ".claude/hooks/start.sh",
                ".claude/settings.json",
                "src/.claude.json",
                "sub/.codex/config.toml"
            ]
        );
        assert!(restored.left.is_empty());
        assert_eq!(snapshot.restore("again"), Restored::default());
    }

    #[test]
    fn a_login_file_and_the_home_s_agent_folders_are_never_read_nor_touched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        made(&root.join(".claude/.credentials.json"), "before");
        let snapshot = Snapshot::take(root, Some(&root.join("home")), &dir.path().join("kept"));
        assert!(snapshot.contents.is_empty());

        made(&root.join(".claude/.credentials.json"), "after");
        made(&root.join("home/.claude/settings.json"), "{}\n");
        made(&root.join(".codex/auth.json"), "{}\n");

        assert_eq!(snapshot.restore("run"), Restored::default());
        assert!(root.join("home/.claude/settings.json").exists());
    }

    #[test]
    fn an_agent_folder_made_a_link_is_put_back_as_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        made(&root.join(".claude/settings.json"), "{}\n");
        made(&dir.path().join("elsewhere/settings.json"), "outside\n");
        let snapshot = Snapshot::take(&root, None, &dir.path().join("kept"));

        std::fs::remove_dir_all(root.join(".claude")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), root.join(".claude")).unwrap();
        snapshot.restore("run");

        assert!(root.join(".claude").is_dir() && !root.join(".claude").is_symlink());
        assert_eq!(
            std::fs::read_to_string(root.join(".claude/settings.json")).unwrap(),
            "{}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("elsewhere/settings.json")).unwrap(),
            "outside\n"
        );
    }

    #[test]
    fn the_put_back_never_writes_through_a_symlinked_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        let home = dir.path().join("home");
        made(&root.join("docs/.claude/settings.json"), "{}\n");
        std::fs::create_dir_all(&home).unwrap();
        let snapshot = Snapshot::take(&root, None, &dir.path().join("kept"));

        std::fs::remove_dir_all(root.join("docs")).unwrap();
        std::os::unix::fs::symlink(&home, root.join("docs")).unwrap();
        let restored = snapshot.restore("run");

        assert!(!home.join(".claude").exists());
        assert_eq!(restored.left, ["docs/.claude/settings.json"]);
    }

    #[test]
    fn a_fifo_in_an_agent_folder_does_not_hang_the_put_back_and_an_added_one_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        made(&root.join(".claude/settings.json"), "{}\n");
        let fifo = |path: &Path| {
            let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        };
        fifo(&root.join(".claude/before"));
        let snapshot = Snapshot::take(root, None, &dir.path().join("kept"));

        fifo(&root.join(".claude/added"));
        let restored = snapshot.restore("run");

        assert!(!root.join(".claude/added").exists());
        assert!(root.join(".claude/before").exists());
        assert_eq!(restored.put_back, [".claude/added"]);
    }

    #[test]
    fn a_name_that_is_not_utf_8_is_put_back_too() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        let snapshot = Snapshot::take(root, None, &dir.path().join("kept"));

        let odd = root
            .join(".claude")
            .join(std::ffi::OsStr::from_bytes(b"hook-\xff.sh"));
        if std::fs::write(&odd, "true\n").is_err() {
            // A file system that refuses such a name cannot hold one.
            return;
        }
        let restored = snapshot.restore("run");

        assert!(!odd.exists());
        assert_eq!(restored.put_back.len(), 1);
    }

    #[test]
    fn an_agent_folder_named_in_another_case_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let snapshot = Snapshot::take(root, None, &dir.path().join("kept"));

        made(&root.join(".CLAUDE/settings.json"), "{\"hooks\": {}}\n");
        made(&root.join("sub/.Codex/config.toml"), "\n");
        snapshot.restore("run");

        assert!(!root.join(".CLAUDE/settings.json").exists());
        assert!(!root.join("sub/.Codex/config.toml").exists());
    }

    #[test]
    fn the_home_s_agent_folders_are_left_out_when_the_place_is_reached_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        made(&home.join(".claude/settings.json"), "{}\n");
        std::os::unix::fs::symlink(&home, dir.path().join("link")).unwrap();
        let snapshot = Snapshot::take(
            &dir.path().join("link"),
            Some(&home),
            &dir.path().join("kept"),
        );
        assert!(snapshot.contents.is_empty());

        // The user's own agents write their settings while the run goes on.
        made(
            &home.join(".claude/settings.json"),
            "{\"theme\": \"dark\"}\n",
        );
        made(&home.join(".claude/projects/a.jsonl"), "{}\n");
        snapshot.restore("run");

        assert!(home.join(".claude/projects/a.jsonl").exists());
        assert!(
            std::fs::read_to_string(home.join(".claude/settings.json"))
                .unwrap()
                .contains("dark")
        );
    }

    #[test]
    fn what_the_put_back_removes_or_overwrites_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        let kept = dir.path().join("state/agent-folders");
        made(&root.join(".claude/settings.json"), "{}\n");
        let snapshot = Snapshot::take(&root, None, &kept);

        made(
            &root.join(".claude/settings.json"),
            "{\"theme\": \"mine\"}\n",
        );
        made(&root.join(".claude/commands/review.md"), "my command\n");
        let restored = snapshot.restore("run-1");
        let kept = kept.join("run-1");

        assert_eq!(restored.kept.as_deref(), Some(kept.as_path()));
        assert_eq!(
            std::fs::read_to_string(kept.join(".claude/settings.json")).unwrap(),
            "{\"theme\": \"mine\"}\n"
        );
        assert_eq!(
            std::fs::read_to_string(kept.join(".claude/commands/review.md")).unwrap(),
            "my command\n"
        );
        let text = said(&restored).unwrap();
        assert!(text.contains(&kept.display().to_string()), "{text}");
    }

    #[test]
    fn a_place_past_the_bound_says_its_agent_folders_could_not_all_be_checked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        for n in 0..ENTRIES_MAX {
            made(&root.join(format!("src/{n}.rs")), "\n");
        }
        let snapshot = Snapshot::take(&root, None, &dir.path().join("kept"));
        made(&root.join(".claude/settings.json"), "{\"hooks\": {}}\n");
        let restored = snapshot.restore("run");

        assert!(restored.unchecked);
        let text = said(&restored).unwrap();
        assert!(text.starts_with("\nflottille: refused: "), "{text}");
        assert!(text.contains("could not all be checked"), "{text}");
        // A place within it says nothing when nothing changed.
        let small = dir.path().join("small");
        made(&small.join("README.md"), "\n");
        let snapshot = Snapshot::take(&small, None, &dir.path().join("kept"));
        assert_eq!(said(&snapshot.restore("run")), None);
    }

    #[tokio::test]
    async fn overlapping_runs_do_not_put_back_a_planted_hook() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        made(&root.join("README.md"), "\n");
        let guards = Guards::default();
        let kept = dir.path().join("kept");

        // P starts and plants a hook; L starts while P goes on.
        let planting = guards.enter(&root, None, &kept, "p").await;
        made(&root.join(".claude/settings.json"), "{\"hooks\": {}}\n");
        let later = guards.enter(&root, None, &kept, "l").await;

        // P ends first: L still goes on, nothing is put back yet.
        assert_eq!(planting.leave().await, None);
        assert!(root.join(".claude/settings.json").exists());
        // L ends last: the place is put back as it was before P.
        let restored = later.leave().await.unwrap();
        assert!(!root.join(".claude/settings.json").exists());
        assert!(
            restored
                .put_back
                .contains(&".claude/settings.json".to_owned())
        );

        // A run that starts after takes a snapshot of its own.
        let next = guards.enter(&root, None, &kept, "n").await;
        made(&root.join(".codex/config.toml"), "\n");
        next.leave().await.unwrap();
        assert!(!root.join(".codex/config.toml").exists());
    }
}
