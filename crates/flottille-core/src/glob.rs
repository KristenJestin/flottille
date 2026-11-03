//! The globs of the setup's `copy` steps: `*` and `?` within one part of a path, and a hidden
//! name matched only by a part that starts with a dot, as a shell does. `.git` is never matched:
//! git's own folder is no file of the setup's.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The paths under `root` that `pattern` matches, relative to it, sorted, none in a `.git`. Links
/// are not followed into, and a part with no wildcard names an entry that exists.
pub fn expand(root: &Path, pattern: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut found = vec![PathBuf::new()];
    for part in pattern.split('/').filter(|part| !part.is_empty()) {
        let mut next = Vec::new();
        for path in found {
            let folder = root.join(&path);
            if !part.contains(['*', '?']) {
                if part != ".git" && std::fs::symlink_metadata(folder.join(part)).is_ok() {
                    next.push(path.join(part));
                }
                continue;
            }
            if !std::fs::symlink_metadata(&folder).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            let entries = match std::fs::read_dir(&folder) {
                Ok(entries) => entries,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                let name = entry?.file_name();
                if let Some(name) = name.to_str()
                    && name != ".git"
                    && matches(part, name)
                {
                    next.push(path.join(name));
                }
            }
        }
        found = next;
    }
    found.retain(|path| !path.as_os_str().is_empty());
    found.sort();
    Ok(found)
}

/// Whether one part of a pattern matches one name.
pub fn matches(part: &str, name: &str) -> bool {
    if name.starts_with('.') && !part.starts_with('.') {
        return false;
    }
    let (part, name): (Vec<char>, Vec<char>) = (part.chars().collect(), name.chars().collect());
    // The classic two-pointer walk: on a mismatch, let the last `*` take one more character.
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match part.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((star_p, star_n)) => {
                    p = star_p + 1;
                    n = star_n + 1;
                    star = Some((star_p, star_n + 1));
                }
                None => return false,
            },
        }
    }
    part[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_matches_with_stars_and_question_marks() {
        assert!(matches(".env.*", ".env.local"));
        assert!(matches(".env*", ".env"));
        assert!(!matches(".env.*", ".env"));
        assert!(matches("*.json", "config.json"));
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
        assert!(matches("*a*b*", "xxaxxbxx"));
        assert!(!matches("*.json", "config.jsonx"));
    }

    #[test]
    fn a_hidden_name_is_matched_only_by_a_part_starting_with_a_dot() {
        assert!(!matches("*", ".env"));
        assert!(matches(".*", ".env"));
    }

    #[test]
    fn a_pattern_never_matches_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".github")).unwrap();
        std::fs::create_dir_all(root.join("sub/.git")).unwrap();
        std::fs::write(root.join(".gitignore"), "").unwrap();

        assert_eq!(
            expand(root, ".gi*").unwrap(),
            [PathBuf::from(".github"), PathBuf::from(".gitignore")]
        );
        assert!(!expand(root, ".*").unwrap().contains(&PathBuf::from(".git")));
        assert!(expand(root, ".git").unwrap().is_empty());
        assert!(expand(root, "*/.git").unwrap().is_empty());
        assert!(expand(root, "sub/.g?t").unwrap().is_empty());
    }

    #[test]
    fn a_pattern_finds_the_entries_under_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config/env")).unwrap();
        for file in [
            ".env",
            ".env.local",
            "config/a.json",
            "config/b.json",
            "config/env/c.json",
        ] {
            std::fs::write(root.join(file), "").unwrap();
        }

        assert_eq!(
            expand(root, ".env*").unwrap(),
            [PathBuf::from(".env"), PathBuf::from(".env.local")]
        );
        assert_eq!(
            expand(root, "config/*.json").unwrap(),
            [
                PathBuf::from("config/a.json"),
                PathBuf::from("config/b.json")
            ]
        );
        assert_eq!(
            expand(root, "*/env/c.json").unwrap(),
            [PathBuf::from("config/env/c.json")]
        );
        assert_eq!(expand(root, ".env").unwrap(), [PathBuf::from(".env")]);
        assert!(expand(root, ".env.missing").unwrap().is_empty());
        assert!(expand(root, "nowhere/*").unwrap().is_empty());
    }
}
