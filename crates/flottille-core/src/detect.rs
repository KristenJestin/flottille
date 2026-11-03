//! Which folders of a project are git repositories.

use std::io;
use std::path::Path;

/// The repositories of `folder`, as paths relative to it: `.` when it is one itself (first), then
/// the folders one or two levels down that hold a `.git`, a folder or a file (a worktree or a
/// submodule has a file), in alphabetical order.
///
/// Hidden folders are not looked into (`.claude`, `.cache`, `.git` itself) and neither are links,
/// so that a link out of the project never brings in a repository that is not under it.
pub fn repositories(folder: &Path) -> io::Result<Vec<String>> {
    let mut found = Vec::new();
    for first in subfolders(folder)? {
        let first_path = folder.join(&first);
        if holds_git(&first_path) {
            found.push(first.clone());
        }
        // A folder that cannot be read has no repository to offer.
        for second in subfolders(&first_path).unwrap_or_default() {
            if holds_git(&first_path.join(&second)) {
                found.push(format!("{first}/{second}"));
            }
        }
    }
    found.sort();
    if holds_git(folder) {
        found.insert(0, ".".to_owned());
    }
    Ok(found)
}

fn holds_git(folder: &Path) -> bool {
    folder.join(".git").symlink_metadata().is_ok()
}

fn subfolders(folder: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str()
            && !name.starts_with('.')
        {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(layout: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for path in layout {
            std::fs::create_dir_all(dir.path().join(path)).unwrap();
        }
        dir
    }

    fn found(dir: &tempfile::TempDir) -> Vec<String> {
        repositories(dir.path()).unwrap()
    }

    #[test]
    fn a_folder_that_is_a_repository_is_one_repository() {
        let dir = folder(&[".git", "src"]);

        assert_eq!(found(&dir), ["."]);
    }

    #[test]
    fn a_folder_holding_three_repositories_and_being_one_finds_all_four() {
        let dir = folder(&[".git", "web/.git", "api/.git", "lib/.git", "docs"]);

        assert_eq!(found(&dir), [".", "api", "lib", "web"]);
    }

    #[test]
    fn a_repository_two_levels_down_is_found_and_one_three_levels_down_is_not() {
        let dir = folder(&["libs/core/.git", "a/b/c/.git"]);

        assert_eq!(found(&dir), ["libs/core"]);
    }

    #[test]
    fn a_git_file_counts_like_a_git_folder() {
        let dir = folder(&["linked", "module/inner"]);
        std::fs::write(dir.path().join("linked/.git"), "gitdir: elsewhere").unwrap();
        std::fs::write(dir.path().join("module/inner/.git"), "gitdir: elsewhere").unwrap();

        assert_eq!(found(&dir), ["linked", "module/inner"]);
    }

    #[test]
    fn a_folder_with_no_repository_finds_none() {
        let dir = folder(&["a/b", "c"]);

        assert!(found(&dir).is_empty());
    }

    #[test]
    fn hidden_folders_are_not_looked_into() {
        let dir = folder(&[".claude/tool/.git", ".hidden/.git", "shown/.git"]);

        assert_eq!(found(&dir), ["shown"]);
    }

    #[test]
    fn a_link_is_not_followed() {
        let outside = folder(&[".git"]);
        let dir = folder(&["real/.git"]);
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();

        assert_eq!(found(&dir), ["real"]);
    }

    #[test]
    fn a_folder_that_does_not_exist_is_an_error() {
        let dir = folder(&[]);

        assert!(repositories(&dir.path().join("nope")).is_err());
    }
}
