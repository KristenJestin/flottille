//! The name the program was called by: `flottille`, or `flot` through a link to it. Its help,
//! its errors and the commands it tells the user to run say that name (`docs/cli.md`). And the
//! program itself, as the daemon starts it again.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const OWN_NAME: &str = "flottille";

/// The file name of `argv[0]`; `flottille` when the system gave none.
pub fn name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::args_os()
            .next()
            .as_deref()
            .and_then(|called| Path::new(called).file_name())
            .and_then(|file| file.to_str())
            .filter(|file| !file.is_empty())
            .map_or(OWN_NAME, |file| Box::leak(file.to_owned().into_boxed_str()))
    })
}

/// Who starts Flottille's own program.
#[derive(Clone, Copy)]
pub enum Started {
    /// This process, in a child of its own: its bridges, the browser's network holder.
    Here,
    /// Another program, from a file Flottille writes: the vault's plugin, which agent-browser
    /// starts in the browser's network.
    ByAnother,
}

/// Flottille's own program, as the daemon starts it again: the file it runs from, unless an
/// update replaced that file while it runs (`docs/releases.md`, "Updating"). Linux then names it
/// `<path> (deleted)`, which cannot be started, and the program is:
/// - started [`Started::Here`], the running image, `/proc/self/exe` in the child, which is the
///   daemon's own: what talks to the daemon stays on its version until it restarts;
/// - started [`Started::ByAnother`], the build now at the path: `/proc/<pid>/exe` of the daemon
///   is refused to a process in another user namespace, as the browser's network is.
pub fn own(started: Started) -> std::io::Result<PathBuf> {
    Ok(of(std::env::current_exe()?, started))
}

fn of(exe: PathBuf, started: Started) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    if !exe.as_os_str().as_bytes().ends_with(b" (deleted)") {
        return exe;
    }
    match started {
        Started::Here => PathBuf::from("/proc/self/exe"),
        Started::ByAnother => crate::client::program_to_start(exe),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_whose_file_is_there_is_started_from_it() {
        let exe = PathBuf::from("/opt/x/flottille");
        assert_eq!(of(exe.clone(), Started::Here), exe);
        assert_eq!(of(exe.clone(), Started::ByAnother), exe);
    }

    #[test]
    fn a_program_whose_file_was_replaced_is_started_from_its_image_here_and_its_path_by_another() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille");
        std::fs::write(&path, "the new build").unwrap();
        let exe = PathBuf::from(format!("{} (deleted)", path.display()));

        assert_eq!(of(exe.clone(), Started::Here), Path::new("/proc/self/exe"));
        assert_eq!(of(exe, Started::ByAnother), path);
    }

    /// The places of the crate that ask the standard library for this program without [`own`],
    /// and why each may: none starts a process that talks to this one from it.
    const NOT_STARTED: [(&str, usize, &str); 5] = [
        ("program.rs", 1, "`own` itself"),
        (
            "client.rs",
            1,
            "starts the daemon, a new one, from the file at the path (`program_to_start`)",
        ),
        (
            "main.rs",
            1,
            "the window becomes the build that replaced it (`program_to_start`)",
        ),
        (
            "update.rs",
            3,
            "the file an update replaces, started by no one",
        ),
        (
            "gate.rs",
            2,
            "the program an agent never writes, started by no one",
        ),
    ];

    #[test]
    fn nothing_starts_flottille_from_current_exe_directly() {
        fn read(dir: &Path, found: &mut Vec<(String, usize)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    read(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let count = std::fs::read_to_string(&path)
                        .unwrap()
                        .matches(concat!("current_exe", "()"))
                        .count();
                    if count > 0 {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        found.push((name, count));
                    }
                }
            }
        }
        let mut found = Vec::new();
        read(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        found.sort();
        let mut allowed: Vec<(String, usize)> = NOT_STARTED
            .iter()
            .map(|(file, count, _)| ((*file).to_owned(), *count))
            .collect();
        allowed.sort();
        assert_eq!(
            found, allowed,
            "a program Flottille starts from itself is `program::own()`"
        );
    }
}
