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
];

/// Whether `path` may be opened by the system's default handler, or why not. The path is taken as
/// it is: a link is not followed, so a file that became one is refused.
pub fn document(path: &Path) -> Result<(), String> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Err("it is not a file".into());
    };
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

/// Whether what a shown link opens may be opened: an http(s) address, or a document.
pub fn openable(target: &str) -> Result<(), String> {
    if target.starts_with("https://") || target.starts_with("http://") {
        return Ok(());
    }
    document(Path::new(target))
}
