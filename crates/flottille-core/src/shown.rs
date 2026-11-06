//! What a link an agent shows the user may be (`docs/tools.md`, "Showing a link"): the system's
//! default handler opens it on a click, and that handler runs a program when it is given one. An
//! agent writes files in its place, so a file shown is only ever a document. The daemon checks it
//! when the agent shows it, and the window again right before it opens it, since the file may
//! have changed in between.

use std::path::Path;

/// The extensions a desktop runs, or installs, rather than shows (compared without case).
pub const RUN_EXTENSIONS: &[&str] = &[
    "desktop",
    "sh",
    "bash",
    "zsh",
    "fish",
    "py",
    "pl",
    "rb",
    "js",
    "appimage",
    "run",
    "bin",
    "jar",
    "exe",
    "msi",
    "bat",
    "cmd",
    "ps1",
    "command",
    "app",
    "deb",
    "rpm",
    "flatpakref",
    "flatpak",
    "snap",
    "jnlp",
    "php",
    "lua",
    "vbs",
    "scr",
    "com",
    "lnk",
    "pkg",
    "dmg",
];

/// Whether `path` may be opened by the system's default handler, or why not. The path is taken as
/// it is: a link is not followed, so a file that became one is refused.
pub fn document(path: &Path) -> Result<(), String> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Err("it is not a file".into());
    };
    check(&meta, path)
}

/// The checks of [`document`], on what the file is.
fn check(meta: &std::fs::Metadata, path: &Path) -> Result<(), String> {
    if !meta.file_type().is_file() {
        return Err("it is not a file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return Err("it is executable, and opening it would run it".into());
        }
    }
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    if let Some(extension) = extension.filter(|e| RUN_EXTENSIONS.contains(&e.as_str())) {
        return Err(format!(
            "a .{extension} file is one a desktop runs, and opening it would run it"
        ));
    }
    Ok(())
}

/// The file at `path`, opened once to be read, on the checks of [`document`] made on the opened
/// file itself: a link is not followed (so one put in its place after a check is refused, not
/// read), and opening a fifo or a device never waits. The caller bounds what it reads.
pub fn open_document(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .map_err(|_| "it is not a file".to_owned())?;
    let meta = file.metadata().map_err(|_| "it is not a file".to_owned())?;
    check(&meta, path)?;
    Ok(file)
}

/// Whether what a shown link opens may be opened: an http(s) address, or a document.
pub fn openable(target: &str) -> Result<(), String> {
    if target.starts_with("https://") || target.starts_with("http://") {
        return Ok(());
    }
    document(Path::new(target))
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn a_document_is_opened_and_read_once() {
        let folder = folder();
        let file = folder.path().join("report.png");
        std::fs::write(&file, "picture").unwrap();
        let mut read = String::new();
        open_document(&file)
            .unwrap()
            .read_to_string(&mut read)
            .unwrap();
        assert_eq!(read, "picture");
    }

    #[test]
    fn a_file_swapped_for_a_link_after_the_check_is_refused() {
        let folder = folder();
        let file = folder.path().join("shot.png");
        std::fs::write(&file, "picture").unwrap();
        assert!(document(&file).is_ok());
        // Between the check and the read, the file became a link to something that never ends.
        std::fs::remove_file(&file).unwrap();
        symlink("/dev/zero", &file).unwrap();
        assert!(open_document(&file).is_err());
        let other = folder.path().join("other.png");
        std::fs::write(&other, "x").unwrap();
        let linked = folder.path().join("linked.png");
        symlink(&other, &linked).unwrap();
        assert!(open_document(&linked).is_err(), "no link is followed");
    }

    #[test]
    fn a_fifo_is_refused_without_waiting_for_it() {
        let folder = folder();
        let fifo = folder.path().join("pipe.png");
        let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o644) }, 0);
        assert_eq!(open_document(&fifo).unwrap_err(), "it is not a file");
    }

    #[test]
    fn an_executable_and_a_program_are_refused_on_the_opened_file() {
        let folder = folder();
        let shot = folder.path().join("shot.png");
        std::fs::write(&shot, "picture").unwrap();
        std::fs::set_permissions(&shot, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(open_document(&shot).unwrap_err().contains("executable"));
        let script = folder.path().join("run.sh");
        std::fs::write(&script, "x").unwrap();
        assert!(open_document(&script).unwrap_err().contains(".sh"));
    }
}
