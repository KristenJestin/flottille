//! The agent folders of a place (`.claude`, `.claude.json`, `.codex`, `.mcp.json`, wherever they
//! are in it) as they were before an agent's run, put back as they were after it. A line may write there
//! without naming a path in one (a patch applied, a branch checked out or merged, a stash popped,
//! a folder copied, an archive unpacked): the gate, which refuses every write of such a folder it
//! sees, cannot see those, so what the run changed there is undone once it ends and the run's
//! result says so (`docs/design.md` §7.1).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

use crate::gate::{AGENT_FOLDERS, AGENT_LOGINS};

/// The most entries of the place looked at: past it, the rest of the place is not, and the
/// result says so.
#[cfg(not(test))]
const ENTRIES_MAX: usize = 500_000;
#[cfg(test)]
const ENTRIES_MAX: usize = 200;

/// `ENTRIES_MAX`, or a daemon's tests' own (`FLOTTILLE_TEST_AGENT_FOLDER_ENTRIES`): a place past
/// the real one would take them minutes to make. A release build reads no variable: 0 there
/// would turn the guard off.
fn entries_max() -> usize {
    static MAX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| {
        #[cfg(debug_assertions)]
        if let Some(max) = std::env::var("FLOTTILLE_TEST_AGENT_FOLDER_ENTRIES")
            .ok()
            .and_then(|max| max.parse().ok())
        {
            return max;
        }
        ENTRIES_MAX
    })
}

/// The largest file of an agent folder kept to be put back: a larger one changed is said, not
/// put back.
const KEPT_MAX: u64 = 1 << 20;

// Called with an entry's path, relative to the place, between the check of what is on the way
// to it and what is done there: a test changes the place in between.
#[cfg(test)]
type Race = Option<Box<dyn FnMut(&Path)>>;
#[cfg(test)]
thread_local! {
    static RACE: std::cell::RefCell<Race> = std::cell::RefCell::new(None);
}

fn race(path: &Path) {
    #[cfg(test)]
    RACE.with(|race| {
        if let Some(race) = race.borrow_mut().as_mut() {
            race(path);
        }
    });
    let _ = path;
}

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
    /// The place and the folders of its repositories, relative to it: their agent folders are
    /// looked at first, however large the place.
    tops: Vec<PathBuf>,
    /// The tops whose agent folders were all looked at: what is found added in them after the
    /// run was added, even when the rest of the place could not all be looked at.
    whole: BTreeSet<PathBuf>,
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
    #[cfg(test)]
    pub fn take(root: &Path, home: Option<&Path>, kept: &Path) -> Snapshot {
        Snapshot::take_around(root, home, kept, &[], &[])
    }

    /// `take`, the agent folders right in each of `tops` (the place's repositories) looked at
    /// first, and for each folder of `inside` (relative to `root`), its snapshot as it was
    /// taken, not the disk: a run there may have changed it since.
    fn take_around(
        root: &Path,
        home: Option<&Path>,
        kept: &Path,
        tops: &[PathBuf],
        inside: &[(PathBuf, Arc<Snapshot>)],
    ) -> Snapshot {
        let root = real(root);
        let home = home.map(real);
        let kept = match (kept.parent(), kept.file_name()) {
            (Some(parent), Some(name)) => real(parent).join(name),
            _ => kept.to_owned(),
        };
        let mut contents = BTreeMap::new();
        let skipped: Vec<&Path> = inside.iter().map(|(at, _)| at.as_path()).collect();
        let mut own_tops = vec![PathBuf::new()];
        for top in tops {
            if let Ok(top) = real(top).strip_prefix(&root)
                && !own_tops.iter().any(|known| known == top)
                && !skipped.iter().any(|inner| top.starts_with(inner))
            {
                own_tops.push(top.to_owned());
            }
        }
        let walked = walk(
            &root,
            home.as_deref(),
            &kept,
            &own_tops,
            &skipped,
            Some(&mut contents),
        );
        let (mut entries, mut complete, mut whole) =
            (walked.entries, walked.complete, walked.whole);
        let mut tops = own_tops;
        for (at, snapshot) in inside {
            tops.extend(snapshot.tops.iter().map(|top| at.join(top)));
            whole.extend(snapshot.whole.iter().map(|top| at.join(top)));
            entries.extend(
                snapshot
                    .entries
                    .iter()
                    .map(|(path, kind)| (at.join(path), kind.clone())),
            );
            contents.extend(
                snapshot
                    .contents
                    .iter()
                    .map(|(path, content)| (at.join(path), content.clone())),
            );
            complete &= snapshot.complete;
        }
        Snapshot {
            root,
            home,
            kept,
            entries,
            complete,
            tops,
            whole,
            contents,
        }
    }

    /// Puts back what changed in the agent folders since the snapshot: what was added is
    /// removed, what was changed or removed is written again. What is removed or overwritten is
    /// moved first to the folder `run` of the snapshot's `kept`, never lost.
    pub fn restore(&self, run: &str) -> Restored {
        let kept = &self.kept.join(run);
        let walked = walk(
            &self.root,
            self.home.as_deref(),
            &self.kept,
            &self.tops,
            &[],
            None,
        );
        let (after, complete) = (walked.entries, walked.complete);
        let mut restored = Restored {
            unchecked: !self.complete || !complete,
            ..Restored::default()
        };
        let Ok(top) = open_root(&self.root) else {
            return restored;
        };
        let top = top.as_fd();
        let shown = |path: &Path| path.display().to_string();
        // What was added, the deepest first; a folder is said only when nothing below it is.
        // Past the bound, what the snapshot did not see may have been there before, but in the
        // agent folders of a top it looked at whole.
        let seen_before = |path: &Path| {
            self.complete || top_of(path).is_some_and(|top| self.whole.contains(&top))
        };
        for (path, kind) in after.iter().rev().filter(|(path, _)| seen_before(path)) {
            if self.entries.contains_key(path) {
                continue;
            }
            let removed = match kind {
                Kind::Folder => remove_folder(top, path),
                _ => keep(top, path, kept, &mut restored.kept),
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
                    looked = parent(top, path)
                        .ok()
                        .and_then(|(folder, name)| kind_of(folder.as_fd(), name));
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
                        parent(top, path)
                            .and_then(|(folder, name)| read_file(folder.as_fd(), name))
                            .is_ok_and(|content| content == *before)
                    }) =>
                {
                    continue;
                }
                _ => {}
            }
            let put = match now {
                Some(Kind::Folder) => remove_folder(top, path),
                Some(_) => keep(top, path, kept, &mut restored.kept),
                None => Ok(()),
            }
            .and_then(|()| self.write(top, path, kind));
            match put {
                Ok(()) => restored.put_back.push(shown(path)),
                Err(_) => restored.left.push(shown(path)),
            }
        }
        restored
    }

    /// Writes an entry as it was, making the folders above it that are gone, each in the one
    /// opened before it: a folder made a link since is never gone through (the home).
    fn write(&self, top: BorrowedFd, path: &Path, kind: &Kind) -> std::io::Result<()> {
        let mut folder = rustix::fs::openat(top, ".", FOLDER, Mode::empty())?;
        for part in path.parent().into_iter().flat_map(Path::components) {
            let part = part.as_os_str();
            match rustix::fs::mkdirat(&folder, part, Mode::from_raw_mode(0o777)) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
            folder = rustix::fs::openat(&folder, part, FOLDER, Mode::empty())?;
        }
        let name = path.file_name().ok_or(Errno::INVAL)?;
        race(path);
        if rustix::fs::statat(&folder, name, AtFlags::SYMLINK_NOFOLLOW)
            .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) != FileType::Directory)
        {
            return Err(std::io::Error::other("something else is there"));
        }
        match kind {
            Kind::Folder => match rustix::fs::mkdirat(&folder, name, Mode::from_raw_mode(0o777)) {
                Ok(()) | Err(Errno::EXIST) => Ok(()),
                Err(error) => Err(error.into()),
            },
            Kind::Link(target) => Ok(rustix::fs::symlinkat(target, &folder, name)?),
            Kind::File { mode, .. } => {
                let before = self
                    .contents
                    .get(path)
                    .ok_or_else(|| std::io::Error::other("too large to have been kept"))?;
                write_new(folder.as_fd(), name, before, *mode)
            }
            Kind::Special => Err(std::io::Error::other("never made again")),
        }
    }
}

/// How a folder of the place is opened, to read its entries or act in it: never a link.
const FOLDER: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// The place's folder, a real path.
fn open_root(root: &Path) -> std::io::Result<OwnedFd> {
    Ok(crate::nofollow::open(root, FOLDER)?.into())
}

/// The folder `relative` of the place, opened without going through a link: one made since it
/// was looked at refuses the open (`ELOOP`) instead of leading elsewhere.
fn folder_at(top: BorrowedFd, relative: &Path) -> std::io::Result<OwnedFd> {
    if relative.as_os_str().is_empty() {
        return Ok(rustix::fs::openat(top, ".", FOLDER, Mode::empty())?);
    }
    crate::nofollow::folder_beneath(top, relative)
}

/// The folder holding `path` and its name in it: what is done there is done by its handle.
fn parent<'a>(top: BorrowedFd, path: &'a Path) -> std::io::Result<(OwnedFd, &'a OsStr)> {
    let name = path.file_name().ok_or(Errno::INVAL)?;
    let folder = folder_at(top, path.parent().unwrap_or(Path::new("")))?;
    Ok((folder, name))
}

/// Removes the empty folder at `path`.
fn remove_folder(top: BorrowedFd, path: &Path) -> std::io::Result<()> {
    let (folder, name) = parent(top, path)?;
    race(path);
    Ok(rustix::fs::unlinkat(&folder, name, AtFlags::REMOVEDIR)?)
}

/// Moves the entry at `path` below `kept`, where the user finds what the put-back removed.
fn keep(
    top: BorrowedFd,
    path: &Path,
    kept: &Path,
    said: &mut Option<PathBuf>,
) -> std::io::Result<()> {
    let (folder, name) = parent(top, path)?;
    let to = crate::nofollow::folder_made(&kept.join(path.parent().unwrap_or(Path::new(""))))?;
    *said = Some(kept.to_owned());
    race(path);
    match rustix::fs::renameat(&folder, name, &to, name) {
        Err(Errno::XDEV) => {}
        moved => return Ok(moved?),
    }
    // On another disk, its copy is made first.
    let stat = rustix::fs::statat(&folder, name, AtFlags::SYMLINK_NOFOLLOW)?;
    match FileType::from_raw_mode(stat.st_mode) {
        FileType::RegularFile => {
            write_new(to.as_fd(), name, &read_file(folder.as_fd(), name)?, 0o600)?
        }
        FileType::Symlink => {
            let target = rustix::fs::readlinkat(&folder, name, Vec::new())?;
            rustix::fs::symlinkat(target.as_c_str(), &to, name)?;
        }
        _ => {}
    }
    Ok(rustix::fs::unlinkat(&folder, name, AtFlags::empty())?)
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

/// What the entry `name` of `folder` is, not followed.
fn kind_of(folder: BorrowedFd, name: &OsStr) -> Option<Kind> {
    let stat = rustix::fs::statat(folder, name, AtFlags::SYMLINK_NOFOLLOW).ok()?;
    Some(match FileType::from_raw_mode(stat.st_mode) {
        FileType::Directory => Kind::Folder,
        FileType::Symlink => {
            let target = rustix::fs::readlinkat(folder, name, Vec::new()).ok()?;
            Kind::Link(PathBuf::from(OsString::from_vec(target.into_bytes())))
        }
        FileType::RegularFile => Kind::File {
            len: stat.st_size as u64,
            modified: (stat.st_mtime as i64, stat.st_mtime_nsec as i64),
            changed: (stat.st_ctime as i64, stat.st_ctime_nsec as i64),
            inode: stat.st_ino as u64,
            mode: stat.st_mode as u32 & 0o7777,
        },
        _ => Kind::Special,
    })
}

/// The content of the file `name` of `folder`: never through a link, never waiting on a pipe
/// put there since it was looked at.
fn read_file(folder: BorrowedFd, name: &OsStr) -> std::io::Result<Vec<u8>> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | crate::nofollow::READ;
    let mut file = std::fs::File::from(rustix::fs::openat(folder, name, flags, Mode::empty())?);
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a file"));
    }
    let mut content = Vec::new();
    file.read_to_end(&mut content)?;
    Ok(content)
}

/// Writes the file `name` of `folder`, which is not there yet, never through a link.
fn write_new(folder: BorrowedFd, name: &OsStr, content: &[u8], mode: u32) -> std::io::Result<()> {
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut file = std::fs::File::from(rustix::fs::openat(
        folder,
        name,
        flags,
        Mode::from_raw_mode(0o600),
    )?);
    file.write_all(content)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
}

/// What a walk of the place found.
struct Walked {
    /// Every entry of an agent folder, relative to the place.
    entries: BTreeMap<PathBuf, Kind>,
    /// Whether every entry of the place was looked at.
    complete: bool,
    /// The tops whose agent folders were all looked at, whatever the rest of the place.
    whole: BTreeSet<PathBuf>,
}

/// A walk of the place, its folders opened below it without going through a link, and what is
/// in each looked at by its handle.
struct Walk<'a> {
    top: OwnedFd,
    root: &'a Path,
    home: Option<&'a Path>,
    kept: &'a Path,
    tops: &'a [PathBuf],
    skipped: &'a [&'a Path],
    contents: Option<&'a mut BTreeMap<PathBuf, Vec<u8>>>,
    entries: BTreeMap<PathBuf, Kind>,
}

/// Every entry of an agent folder below `root`, relative to it: `.git`, the home's, `kept` and
/// the folders `skipped` left out, links not followed, and no login file of an agent folder,
/// which is never read; with `contents`, those of the files not too large to keep. The agent
/// folders right in each of `tops` (the place, its repositories) first, with a bound of their
/// own: those the agent's command line reads when the user opens a repository are looked at
/// however large the rest of the place.
fn walk(
    root: &Path,
    home: Option<&Path>,
    kept: &Path,
    tops: &[PathBuf],
    skipped: &[&Path],
    contents: Option<&mut BTreeMap<PathBuf, Vec<u8>>>,
) -> Walked {
    let mut walked = Walked {
        entries: BTreeMap::new(),
        complete: true,
        whole: BTreeSet::new(),
    };
    let Ok(top) = open_root(root) else {
        return walked;
    };
    let mut walk = Walk {
        top,
        root,
        home,
        kept,
        tops,
        skipped,
        contents,
        entries: BTreeMap::new(),
    };
    let mut seen = 0;
    for top in tops {
        if walk.visit(vec![(top.clone(), false)], &mut seen, true) {
            walked.whole.insert(top.clone());
        }
    }
    let mut seen = 0;
    walked.complete = walk.visit(vec![(PathBuf::new(), false)], &mut seen, false)
        && walked.whole.len() == tops.len();
    walked.entries = walk.entries;
    walked
}

impl Walk<'_> {
    /// The folders `pending` and what is below them, until `seen` passes the bound: whether it
    /// did not. With `tops`, only the agent folders right in them; without, the agent folders
    /// right in a top are left to that walk.
    fn visit(&mut self, mut pending: Vec<(PathBuf, bool)>, seen: &mut usize, tops: bool) -> bool {
        while let Some((folder, agent)) = pending.pop() {
            race(&folder);
            let Ok(opened) = folder_at(self.top.as_fd(), &folder) else {
                continue;
            };
            let Ok(read) = rustix::fs::Dir::read_from(&opened) else {
                continue;
            };
            let in_top = !agent && self.tops.contains(&folder);
            for entry in read.flatten() {
                let name = OsStr::from_bytes(entry.file_name().to_bytes());
                if name == "." || name == ".." || (!agent && tops && !agent_folder(name)) {
                    continue;
                }
                if !tops && in_top && agent_folder(name) {
                    continue;
                }
                *seen += 1;
                if *seen > entries_max() {
                    return false;
                }
                let path = folder.join(name);
                let Some(kind) = kind_of(opened.as_fd(), name) else {
                    continue;
                };
                let home_s = agent_folder(name)
                    && self
                        .home
                        .is_some_and(|home| self.root.join(&folder) == home);
                if (!agent && name.as_encoded_bytes().eq_ignore_ascii_case(b".git"))
                    || home_s
                    || self.root.join(&path) == self.kept
                    || self.skipped.contains(&path.as_path())
                {
                    continue;
                }
                let agent = agent || agent_folder(name);
                // Only a file is a login: a folder of that name is walked, or it would hide
                // what is planted below it.
                if agent && login(name) && kind != Kind::Folder {
                    continue;
                }
                if kind == Kind::Folder {
                    pending.push((path.clone(), agent));
                }
                if !agent {
                    continue;
                }
                if let (Some(contents), Kind::File { len, .. }) =
                    (self.contents.as_deref_mut(), &kind)
                    && *len <= KEPT_MAX
                    && let Ok(content) = read_file(opened.as_fd(), name)
                {
                    contents.insert(path.clone(), content);
                }
                self.entries.insert(path, kind);
            }
        }
        true
    }
}

/// The top an entry's agent folder is right in, when it is right in one: where the agent folder
/// starts, less its last part.
fn top_of(path: &Path) -> Option<PathBuf> {
    let mut at = PathBuf::new();
    for part in path.components() {
        if agent_folder(part.as_os_str()) {
            return Some(at);
        }
        at.push(part);
    }
    None
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
             {} entries; those right in it and in its repositories were, a change in one further \
             down is neither seen nor put back, tell the user",
            if text.is_empty() {
                "\nflottille: refused: "
            } else {
                "; "
            },
            entries_max()
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
/// another run goes on would hold what that run wrote, and put it back. A place inside another
/// (a repository's folder in the project's, a project that is the home) is the same place: its
/// runs join the outer one's group, and a run in an outer place takes in the groups inside it,
/// their snapshots kept for their folders.
#[derive(Default)]
pub struct Guards {
    /// One lock for all of them: a group may take in others. Held while a snapshot is taken or
    /// put back, so that a run that starts meanwhile waits for it.
    groups: Arc<tokio::sync::Mutex<Vec<Group>>>,
}

/// The runs that go on in a place, or in places inside it.
struct Group {
    root: PathBuf,
    runs: Vec<String>,
    snapshot: Option<Arc<Snapshot>>,
}

/// One run's share of its place's guard: `leave` once it ends.
pub struct Guard {
    groups: Arc<tokio::sync::Mutex<Vec<Group>>>,
    /// The run's id: where its put-back keeps what it removes, if it is the last.
    run: String,
    left: bool,
}

impl Guards {
    /// Guards the agent folders of `root` while the run `run` goes on there. `kept`, where a
    /// put-back keeps what it removes, a folder per run.
    pub async fn enter(
        &self,
        root: &Path,
        tops: &[PathBuf],
        home: Option<&Path>,
        kept: &Path,
        run: &str,
    ) -> Guard {
        let root = real(root);
        let mut groups = self.groups.lock().await;
        let guard = Guard {
            groups: self.groups.clone(),
            run: run.to_owned(),
            left: false,
        };
        if let Some(group) = groups
            .iter_mut()
            .find(|group| root.starts_with(&group.root))
        {
            group.runs.push(run.to_owned());
            return guard;
        }
        let (inside, others): (Vec<Group>, Vec<Group>) = std::mem::take(&mut *groups)
            .into_iter()
            .partition(|group| group.root.starts_with(&root));
        *groups = others;
        let mut runs: Vec<String> = inside.iter().flat_map(|group| group.runs.clone()).collect();
        runs.push(run.to_owned());
        let taken_in: Vec<(PathBuf, Arc<Snapshot>)> = inside
            .into_iter()
            .filter_map(|group| {
                let relative = group.root.strip_prefix(&root).ok()?.to_owned();
                Some((relative, group.snapshot?))
            })
            .collect();
        let (home, kept, at) = (home.map(Path::to_owned), kept.to_owned(), root.clone());
        let tops = tops.to_vec();
        let snapshot = tokio::task::spawn_blocking(move || {
            Arc::new(Snapshot::take_around(
                &at,
                home.as_deref(),
                &kept,
                &tops,
                &taken_in,
            ))
        })
        .await
        .ok();
        groups.push(Group {
            root,
            runs,
            snapshot,
        });
        guard
    }
}

impl Guard {
    /// The run ended: the last one of its place puts the agent folders back.
    pub async fn leave(mut self) -> Option<Restored> {
        self.left = true;
        leave(self.groups.clone(), self.run.clone()).await
    }
}

impl Drop for Guard {
    /// A run that never started still leaves its place.
    fn drop(&mut self) {
        if self.left {
            return;
        }
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(leave(self.groups.clone(), self.run.clone()));
            }
            // Only when the daemon stops: nothing is left to put back for.
            Err(_) => eprintln!(
                "flottille: the run {} left its agent folders' guard without a runtime: they \
                 are not put back",
                self.run
            ),
        }
    }
}

async fn leave(groups: Arc<tokio::sync::Mutex<Vec<Group>>>, run: String) -> Option<Restored> {
    let mut groups = groups.lock().await;
    let at = groups.iter().position(|group| group.runs.contains(&run))?;
    groups[at].runs.retain(|other| *other != run);
    if !groups[at].runs.is_empty() {
        return None;
    }
    let snapshot = groups.remove(at).snapshot?;
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
    fn a_repository_s_mcp_json_is_put_back_as_an_agent_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        made(&root.join(".mcp.json"), "{\"mcpServers\": {}}\n");
        let snapshot = Snapshot::take(&root, None, &dir.path().join("kept"));

        made(
            &root.join(".mcp.json"),
            "{\"mcpServers\": {\"x\": {\"command\": \"true\"}}}\n",
        );
        made(&root.join("app/.mcp.json"), "{\"mcpServers\": {}}\n");
        let restored = snapshot.restore("run");

        assert_eq!(
            std::fs::read_to_string(root.join(".mcp.json")).unwrap(),
            "{\"mcpServers\": {}}\n"
        );
        assert!(!root.join("app/.mcp.json").exists());
        let mut put_back = restored.put_back.clone();
        put_back.sort();
        assert_eq!(put_back, [".mcp.json", "app/.mcp.json"]);
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
    fn a_folder_named_as_a_login_file_is_walked_and_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        made(&root.join(".claude/skills/kept/SKILL.md"), "kept\n");
        let snapshot = Snapshot::take(root, None, &dir.path().join("kept"));

        // A login file's name on a folder hid all below it, a skill declaring hooks included.
        made(&root.join(".claude/skills/auth.json/SKILL.md"), "hooks\n");
        made(
            &root.join(".claude/agents/.credentials.json/x.md"),
            "hooks\n",
        );
        snapshot.restore("run");

        assert!(!root.join(".claude/skills/auth.json").exists());
        assert!(!root.join(".claude/agents/.credentials.json").exists());
        assert!(root.join(".claude/skills/kept/SKILL.md").exists());
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

    /// Runs `swap` once, the first time the walk or the put-back reaches `at`.
    fn racing(at: &'static str, swap: impl FnOnce() + 'static) {
        let mut swap = Some(swap);
        RACE.with(|race| {
            *race.borrow_mut() = Some(Box::new(move |path: &Path| {
                if path == Path::new(at)
                    && let Some(swap) = swap.take()
                {
                    swap();
                }
            }))
        });
    }

    /// `folder` moved aside and made a link to `to`.
    fn swapped(folder: PathBuf, to: PathBuf) -> impl FnOnce() + 'static {
        move || {
            std::fs::rename(&folder, folder.with_extension("aside")).unwrap();
            std::os::unix::fs::symlink(&to, &folder).unwrap();
        }
    }

    #[test]
    fn a_folder_made_a_link_while_the_walk_reaches_it_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        let home = dir.path().join("home");
        made(&root.join("docs/README.md"), "\n");
        made(
            &home.join(".claude/settings.json"),
            "{\"theme\": \"mine\"}\n",
        );
        racing("docs", swapped(root.join("docs"), home.clone()));

        let snapshot = Snapshot::take(&root, None, &dir.path().join("kept"));

        assert!(snapshot.entries.is_empty(), "{:?}", snapshot.entries.keys());
        assert!(snapshot.contents.is_empty());
    }

    #[test]
    fn a_folder_made_a_link_while_the_put_back_acts_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        let home = dir.path().join("home");
        made(&root.join("docs/.claude/settings.json"), "{}\n");
        made(
            &home.join(".claude/settings.json"),
            "{\"theme\": \"mine\"}\n",
        );
        let snapshot = Snapshot::take(&root, None, &dir.path().join("kept"));

        made(
            &root.join("docs/.claude/settings.json"),
            "{\"hooks\": {}}\n",
        );
        racing(
            "docs/.claude/settings.json",
            swapped(root.join("docs"), home.clone()),
        );
        let restored = snapshot.restore("run");

        assert_eq!(
            std::fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
            "{\"theme\": \"mine\"}\n"
        );
        assert_eq!(std::fs::read_dir(home.join(".claude")).unwrap().count(), 1);
        // What the run wrote was taken from the folder the check found, moved aside since; the
        // link is not gone through to write it back.
        assert!(!root.join("docs.aside/.claude/settings.json").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("kept/run/docs/.claude/settings.json"))
                .unwrap(),
            "{\"hooks\": {}}\n"
        );
        assert_eq!(restored.left, ["docs/.claude/settings.json"]);
    }

    #[test]
    fn a_place_past_the_bound_still_puts_back_its_top_agent_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        for n in 0..ENTRIES_MAX {
            made(&root.join(format!("src/{n}.rs")), "\n");
        }
        made(&root.join(".codex/config.toml"), "\n");
        made(&root.join("app/README.md"), "\n");
        let snapshot = Snapshot::take_around(
            &root,
            None,
            &dir.path().join("kept"),
            &[root.join("app")],
            &[],
        );

        made(&root.join(".claude/settings.json"), "{\"hooks\": {}}\n");
        made(&root.join(".codex/hooks/start.sh"), "true\n");
        made(&root.join("app/.CLAUDE/settings.json"), "{\"hooks\": {}}\n");
        let restored = snapshot.restore("run");

        assert!(!root.join(".claude").exists(), "{restored:?}");
        assert!(!root.join("app/.CLAUDE").exists(), "{restored:?}");
        assert!(!root.join(".codex/hooks").exists(), "{restored:?}");
        assert!(root.join(".codex/config.toml").exists());
        assert!(restored.unchecked);
    }

    #[tokio::test]
    async fn nested_places_share_one_guard_and_do_not_put_back_a_planted_hook() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("p");
        let inner = outer.join("svc");
        made(&inner.join("README.md"), "\n");
        let kept = dir.path().join("kept");
        let planted = inner.join(".claude/settings.json");

        // The outer run starts first and plants; the inner one starts while it goes on.
        let guards = Guards::default();
        let a = guards.enter(&outer, &[], None, &kept, "a").await;
        made(&planted, "{\"hooks\": {}}\n");
        let b = guards.enter(&inner, &[], None, &kept, "b").await;
        assert_eq!(a.leave().await, None);
        assert!(planted.exists());
        let restored = b.leave().await.unwrap();
        assert!(!planted.exists(), "{restored:?}");
        assert_eq!(restored.put_back, ["svc/.claude/settings.json"]);

        // The inner run starts first and plants; the outer one starts while it goes on.
        let guards = Guards::default();
        let b = guards.enter(&inner, &[], None, &kept, "b2").await;
        made(&planted, "{\"hooks\": {}}\n");
        let a = guards.enter(&outer, &[], None, &kept, "a2").await;
        assert_eq!(b.leave().await, None);
        assert!(planted.exists());
        a.leave().await.unwrap();
        assert!(!planted.exists());
        assert!(guards.groups.lock().await.is_empty());
    }

    #[test]
    fn a_guard_dropped_outside_a_runtime_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let guards = Guards::default();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let guard =
            runtime.block_on(guards.enter(dir.path(), &[], None, &dir.path().join("kept"), "r"));
        drop(runtime);
        drop(guard);
    }

    #[tokio::test]
    async fn overlapping_runs_do_not_put_back_a_planted_hook() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        made(&root.join("README.md"), "\n");
        let guards = Guards::default();
        let kept = dir.path().join("kept");

        // P starts and plants a hook; L starts while P goes on.
        let planting = guards.enter(&root, &[], None, &kept, "p").await;
        made(&root.join(".claude/settings.json"), "{\"hooks\": {}}\n");
        let later = guards.enter(&root, &[], None, &kept, "l").await;

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
        let next = guards.enter(&root, &[], None, &kept, "n").await;
        made(&root.join(".codex/config.toml"), "\n");
        next.leave().await.unwrap();
        assert!(!root.join(".codex/config.toml").exists());
        // The last run of a place to leave takes its entry with it.
        assert!(guards.groups.lock().await.is_empty());
    }
}
