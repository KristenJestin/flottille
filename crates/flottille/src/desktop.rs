//! The launcher's files (`docs/releases.md`, "The launcher entry"): the desktop entry and the
//! icon, in the user's XDG data folder. `install.sh` writes them from the package; `flottille
//! update` writes them again when they changed, and follows the binary when it moved.

use std::io;
use std::path::{Path, PathBuf};

use flottille_core::paths::Paths;

/// The package's own entry says `Exec=flottille window`; the written one names the binary.
const EXEC: &str = "Exec=";

/// The XDG data folder: `Paths` keeps Flottille's own folder in it.
fn data_home(paths: &Paths) -> PathBuf {
    paths
        .data
        .parent()
        .map_or_else(|| paths.data.clone(), Path::to_owned)
}

fn entry_path(paths: &Paths) -> PathBuf {
    data_home(paths).join("applications/flottille.desktop")
}

/// The `Exec` of the entry for `binary`: its path as it is, or in double quotes when it holds a
/// character the format reserves (a space). None when it holds one that no quoting can say (a
/// quote, a backtick, `$`, a backslash, `%`).
fn exec_line(binary: &Path) -> Option<String> {
    let path = binary.to_str()?;
    if path.contains(['"', '`', '$', '\\', '%', '\n']) {
        return None;
    }
    let plain = path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._+-".contains(c));
    Some(if plain {
        format!("{EXEC}{path} window")
    } else {
        format!("{EXEC}\"{path}\" window")
    })
}

/// `entry` with its `Exec` line replaced by `exec`.
fn with_exec(entry: &str, exec: &str) -> String {
    let mut text: String = entry
        .lines()
        .map(|line| if line.starts_with(EXEC) { exec } else { line })
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    text
}

/// Writes `bytes` to `path` unless it already holds them: a file that is right keeps its date.
/// Written beside it then renamed over it, as the binary is.
fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if std::fs::read(path).is_ok_and(|now| now == bytes) {
        return Ok(());
    }
    let folder = path.parent().ok_or_else(|| io::Error::other("no folder"))?;
    std::fs::create_dir_all(folder)?;
    let beside = folder.join(format!(
        ".{}.new",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&beside, bytes)?;
    std::fs::rename(beside, path)
}

/// The entry and the icon of the package unpacked at `share`, written for `binary`. A package
/// that holds none (one older than this) writes nothing.
pub fn write(paths: &Paths, share: &Path, binary: &Path) -> io::Result<()> {
    let Ok(entry) = std::fs::read_to_string(share.join("applications/flottille.desktop")) else {
        return Ok(());
    };
    let Some(exec) = exec_line(binary) else {
        return Err(io::Error::other(format!(
            "{} holds a character a desktop entry cannot name, so none was written",
            binary.display()
        )));
    };
    write_if_changed(&entry_path(paths), with_exec(&entry, &exec).as_bytes())?;
    let mut icons = vec![share.join("icons/hicolor")];
    while let Some(folder) = icons.pop() {
        for item in std::fs::read_dir(&folder)? {
            let item = item?.path();
            if item.is_dir() {
                icons.push(item);
            } else if let Ok(inside) = item.strip_prefix(share) {
                write_if_changed(&data_home(paths).join(inside), &std::fs::read(&item)?)?;
            }
        }
    }
    Ok(())
}

/// Points the entry that is there at `binary`, when it points elsewhere: the binary moved, and
/// nothing new was unpacked to write it from.
pub fn follow(paths: &Paths, binary: &Path) -> io::Result<()> {
    let path = entry_path(paths);
    let Ok(entry) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let Some(exec) = exec_line(binary) else {
        return Err(io::Error::other(format!(
            "{} holds a character a desktop entry cannot name, so the entry was left as it is",
            binary.display()
        )));
    };
    write_if_changed(&path, with_exec(&entry, &exec).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_path_is_written_as_it_is_and_one_with_a_space_in_quotes() {
        assert_eq!(
            exec_line(Path::new("/home/a/.local/bin/flottille")).unwrap(),
            "Exec=/home/a/.local/bin/flottille window"
        );
        assert_eq!(
            exec_line(Path::new("/home/my home/bin/flottille")).unwrap(),
            "Exec=\"/home/my home/bin/flottille\" window"
        );
    }

    #[test]
    fn a_path_no_quoting_can_say_gets_no_entry() {
        for path in [
            "/a/$HOME/flottille",
            "/a/b\"c/flottille",
            "/a/50%/flottille",
        ] {
            assert!(exec_line(Path::new(path)).is_none(), "{path}");
        }
    }

    #[test]
    fn only_the_exec_line_changes() {
        let entry = "[Desktop Entry]\nName=Flottille\nExec=flottille window\nIcon=flottille\n";
        assert_eq!(
            with_exec(entry, "Exec=/x/flottille window"),
            "[Desktop Entry]\nName=Flottille\nExec=/x/flottille window\nIcon=flottille\n"
        );
    }
}
