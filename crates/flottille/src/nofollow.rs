//! Opening the real paths the gate decided on by descriptors, so that no link is followed after
//! the check (`docs/tools.md`, "What the gate guarantees to the file tools").
//!
//! A path the gate decided on is real: every link followed, no `..`. Opened with
//! `RESOLVE_NO_SYMLINKS`, a link met on the way means that a folder changed since the check, and
//! the open fails (`ELOOP`) instead of following it. Where `openat2` is missing (`ENOSYS`), each
//! folder is opened in turn with `O_NOFOLLOW`, from `/` or from the folder below which the path
//! must stay.

use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Component, Path};

use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

/// How a folder is opened to go through it or to read its entries.
const FOLDER: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// How a file is opened to read: a pipe is opened without waiting for a writer.
pub const READ: OFlags = OFlags::RDONLY.union(OFlags::NONBLOCK);

/// How anything is opened to be looked at, not read: a socket or a device too.
#[cfg(target_os = "linux")]
pub const LOOK: OFlags = OFlags::PATH;
#[cfg(not(target_os = "linux"))]
pub const LOOK: OFlags = READ;

/// Opens the real, absolute path `path`, refusing any link on the way (`ELOOP`).
pub fn open(path: &Path, flags: OFlags) -> std::io::Result<File> {
    let flags = flags | OFlags::CLOEXEC;
    // `RESOLVE_NO_SYMLINKS` refuses a last part that is a link too, with `ELOOP`, where
    // `O_NOFOLLOW` with `O_DIRECTORY` would say `ENOTDIR`.
    #[cfg(target_os = "linux")]
    match rustix::fs::openat2(
        rustix::fs::CWD,
        path,
        flags - OFlags::NOFOLLOW,
        Mode::empty(),
        rustix::fs::ResolveFlags::NO_SYMLINKS,
    ) {
        Err(Errno::NOSYS) => {}
        done => return Ok(done?.into()),
    }
    let root = rustix::fs::open("/", FOLDER, Mode::empty())?;
    let relative = path.strip_prefix("/").map_err(|_| Errno::INVAL)?;
    Ok(walk(root.as_fd(), relative, flags)?.into())
}

/// Opens `relative` below the folder `folder`, never out of it and never through a link.
pub fn open_beneath(folder: BorrowedFd, relative: &Path, flags: OFlags) -> std::io::Result<File> {
    let flags = flags | OFlags::CLOEXEC;
    #[cfg(target_os = "linux")]
    match rustix::fs::openat2(
        folder,
        relative,
        flags - OFlags::NOFOLLOW,
        Mode::empty(),
        rustix::fs::ResolveFlags::NO_SYMLINKS | rustix::fs::ResolveFlags::BENEATH,
    ) {
        Err(Errno::NOSYS) => {}
        done => return Ok(done?.into()),
    }
    Ok(walk(folder, relative, flags)?.into())
}

/// `relative` opened from `start` a folder at a time, none of them a link.
fn walk(start: BorrowedFd, relative: &Path, flags: OFlags) -> rustix::io::Result<OwnedFd> {
    let mut parts = Vec::new();
    for part in relative.components() {
        match part {
            Component::Normal(name) => parts.push(name),
            Component::CurDir => {}
            // A real path holds no `..`, and one below a folder never leaves it.
            _ => return Err(Errno::XDEV),
        }
    }
    let Some((last, folders)) = parts.split_last() else {
        return rustix::fs::openat(start, ".", flags, Mode::empty());
    };
    let mut folder: Option<OwnedFd> = None;
    for name in folders {
        let at = folder.as_ref().map_or(start, AsFd::as_fd);
        folder = Some(open_at(at, name, FOLDER)?);
    }
    let at = folder.as_ref().map_or(start, AsFd::as_fd);
    open_at(at, last, flags)
}

/// The entry `name` of the folder `at`, opened without following it: a link is `ELOOP`, even as
/// a folder (where the kernel says `ENOTDIR`).
fn open_at(at: BorrowedFd, name: &std::ffi::OsStr, flags: OFlags) -> rustix::io::Result<OwnedFd> {
    match rustix::fs::openat(at, name, flags | OFlags::NOFOLLOW, Mode::empty()) {
        Err(Errno::NOTDIR)
            if rustix::fs::statat(at, name, AtFlags::SYMLINK_NOFOLLOW)
                .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink) =>
        {
            Err(Errno::LOOP)
        }
        opened => opened,
    }
}

/// Opens the real folder `path`, making those of its folders that are missing a folder at a
/// time, each in the one opened before it: none of them is ever a link.
pub fn folder_made(path: &Path) -> std::io::Result<OwnedFd> {
    match open(path, FOLDER) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        done => return Ok(done?.into()),
    }
    let mut folder = rustix::fs::open("/", FOLDER, Mode::empty())?;
    for part in path.components() {
        let name = match part {
            Component::Normal(name) => name,
            Component::RootDir | Component::CurDir => continue,
            _ => return Err(Errno::XDEV.into()),
        };
        folder = match open_at(folder.as_fd(), name, FOLDER) {
            Err(Errno::NOENT) => {
                match rustix::fs::mkdirat(&folder, name, Mode::from_raw_mode(0o777)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
                open_at(folder.as_fd(), name, FOLDER)?
            }
            opened => opened?,
        };
    }
    Ok(folder)
}

/// Opens the folder below `folder` named `name`, to read its entries.
pub fn folder_beneath(folder: BorrowedFd, name: &Path) -> std::io::Result<OwnedFd> {
    Ok(open_beneath(folder, name, FOLDER)?.into())
}

/// An entry of a folder, as it is: a link is not followed.
pub struct Entry {
    pub name: String,
    pub kind: FileType,
    pub size: u64,
    /// Where a link leads, as written in it.
    pub target: Option<String>,
}

/// The entries of an open folder, sorted by name; those whose name is not UTF-8 are left out.
pub fn entries(folder: BorrowedFd) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in rustix::fs::Dir::read_from(folder)? {
        let Ok(name) = entry?.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name == "." || name == ".." {
            continue;
        }
        let Ok(stat) = rustix::fs::statat(folder, name.as_str(), AtFlags::SYMLINK_NOFOLLOW) else {
            continue;
        };
        let kind = FileType::from_raw_mode(stat.st_mode);
        let target = (kind == FileType::Symlink)
            .then(|| rustix::fs::readlinkat(folder, name.as_str(), Vec::new()).ok())
            .flatten()
            .map(|target| target.to_string_lossy().into_owned());
        entries.push(Entry {
            name,
            kind,
            size: stat.st_size as u64,
            target,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_openat2_a_link_anywhere_on_the_way_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(base.join("real/sub")).unwrap();
        std::fs::write(base.join("real/sub/f.txt"), "x").unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
        std::os::unix::fs::symlink(base.join("real/sub"), base.join("real/sublink")).unwrap();
        let root = rustix::fs::open("/", FOLDER, Mode::empty()).unwrap();
        let walked = |path: &Path, flags| {
            walk(
                root.as_fd(),
                path.strip_prefix("/").unwrap(),
                flags | OFlags::CLOEXEC,
            )
        };

        assert!(walked(&base.join("real/sub/f.txt"), READ).is_ok());
        for (path, flags) in [
            (base.join("link/sub/f.txt"), READ),
            (base.join("real/sublink/f.txt"), READ),
            (base.join("real/sublink"), READ),
            (base.join("real/sublink"), FOLDER),
        ] {
            assert_eq!(walked(&path, flags).unwrap_err(), Errno::LOOP, "{path:?}");
        }
        // Making folders never goes through a link either.
        assert_eq!(
            folder_made(&base.join("link/new"))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ELOOP)
        );
        assert!(!base.join("real/new").exists());
        assert!(folder_made(&base.join("real/new/deeper")).is_ok());
    }
}
