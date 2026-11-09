//! The agent folders of a place (`.claude`, `.claude.json`, `.codex`, wherever they are in it) as
//! they were before an agent's run, put back as they were after it. A line may write there
//! without naming a path in one (a patch applied, a branch checked out or merged, a stash popped,
//! a folder copied, an archive unpacked): the gate, which refuses every write of such a folder it
//! sees, cannot see those, so what the run changed there is undone once it ends and the run's
//! result says so (`docs/design.md` §7.1).

use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::gate::{AGENT_FOLDERS, AGENT_LOGINS};

/// The most entries of the place looked at: past it, the rest of the place is not.
const ENTRIES_MAX: usize = 500_000;

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
}

/// The agent folders of a place before a run.
pub struct Snapshot {
    root: PathBuf,
    /// The home's own agent folders, when the place holds the home: the user's, which their own
    /// agents write while the run goes on, and a login's place, never read.
    skipped: Vec<PathBuf>,
    /// Relative to `root`.
    entries: BTreeMap<PathBuf, Kind>,
    /// The content of each file, when it is not too large to keep.
    contents: BTreeMap<PathBuf, Vec<u8>>,
}

/// What `Snapshot::restore` found the run changed, relative to the place.
#[derive(Debug, Default, PartialEq)]
pub struct Restored {
    pub put_back: Vec<String>,
    /// Changed, but too large to have been kept, or the disk refused to put it back.
    pub left: Vec<String>,
}

impl Snapshot {
    /// The agent folders of `root`, the home's left out.
    pub fn take(root: &Path, home: Option<&Path>) -> Snapshot {
        let skipped: Vec<PathBuf> = home
            .map(|home| AGENT_FOLDERS.iter().map(|entry| home.join(entry)).collect())
            .unwrap_or_default();
        let entries = walk(root, &skipped);
        let contents = entries
            .iter()
            .filter_map(|(path, kind)| match kind {
                Kind::File { len, .. } if *len <= KEPT_MAX => {
                    Some((path.clone(), std::fs::read(root.join(path)).ok()?))
                }
                _ => None,
            })
            .collect();
        Snapshot {
            root: root.to_owned(),
            skipped,
            entries,
            contents,
        }
    }

    /// Puts back what changed in the agent folders since the snapshot: what was added is
    /// removed, what was changed or removed is written again.
    pub fn restore(&self) -> Restored {
        let after = walk(&self.root, &self.skipped);
        let mut restored = Restored::default();
        let shown = |path: &Path| path.display().to_string();
        // What was added, the deepest first; a folder is said only when nothing below it is.
        for (path, kind) in after.iter().rev() {
            if self.entries.contains_key(path) {
                continue;
            }
            let at = self.root.join(path);
            let removed = match kind {
                Kind::Folder => std::fs::remove_dir(&at),
                Kind::File { .. } | Kind::Link(_) => std::fs::remove_file(&at),
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
            let now = after.get(path);
            if now == Some(kind) {
                continue;
            }
            let at = self.root.join(path);
            match (kind, now) {
                (Kind::Folder, Some(Kind::Folder)) | (Kind::Folder, None) => continue,
                (Kind::File { .. }, Some(Kind::File { .. }))
                    if self.contents.get(path).is_some_and(|kept| {
                        std::fs::read(&at).is_ok_and(|content| content == *kept)
                    }) =>
                {
                    continue;
                }
                _ => {}
            }
            let put = match now {
                Some(Kind::Folder) => std::fs::remove_dir_all(&at),
                Some(_) => std::fs::remove_file(&at),
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

    /// Writes an entry as it was.
    fn write(&self, path: &Path, kind: &Kind) -> std::io::Result<()> {
        let at = self.root.join(path);
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match kind {
            Kind::Folder => std::fs::create_dir_all(&at),
            Kind::Link(target) => std::os::unix::fs::symlink(target, &at),
            Kind::File { mode, .. } => {
                let kept = self
                    .contents
                    .get(path)
                    .ok_or_else(|| std::io::Error::other("too large to have been kept"))?;
                std::fs::write(&at, kept)?;
                std::fs::set_permissions(&at, std::fs::Permissions::from_mode(*mode))
            }
        }
    }
}

/// Every entry of an agent folder below `root`, relative to it: `.git` and `skipped` left out,
/// links not followed, and no login file of an agent folder, which is never read.
fn walk(root: &Path, skipped: &[PathBuf]) -> BTreeMap<PathBuf, Kind> {
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
                return entries;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let path = folder.join(name);
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if (!agent && name == ".git") || skipped.contains(&root.join(&path)) {
                continue;
            }
            let agent = agent || AGENT_FOLDERS.contains(&name);
            if agent && AGENT_LOGINS.contains(&name) {
                continue;
            }
            if kind.is_dir() {
                pending.push((path.clone(), agent));
            }
            if !agent {
                continue;
            }
            let kind = if kind.is_dir() {
                Kind::Folder
            } else if kind.is_symlink() {
                match std::fs::read_link(entry.path()) {
                    Ok(target) => Kind::Link(target),
                    Err(_) => continue,
                }
            } else {
                match entry.metadata() {
                    Ok(meta) => Kind::File {
                        len: meta.len(),
                        modified: (meta.mtime(), meta.mtime_nsec()),
                        changed: (meta.ctime(), meta.ctime_nsec()),
                        inode: meta.ino(),
                        mode: meta.mode() & 0o7777,
                    },
                    Err(_) => continue,
                }
            };
            entries.insert(path, kind);
        }
    }
    entries
}

/// What the run's result says of the agent folders it changed, once put back.
pub fn said(restored: &Restored) -> Option<String> {
    if restored.put_back.is_empty() && restored.left.is_empty() {
        return None;
    }
    let mut text = format!(
        "\nflottille: refused: this run wrote in a repository's agent folder ({}): its settings \
         and hooks run programs the next time the user opens that repository with the agent's \
         own command line, outside the gate: an agent never writes there, in any mode",
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
    text.push_str("; ask the user to make the change\n");
    Some(text)
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
        let snapshot = Snapshot::take(root, None);

        made(&root.join(".claude/settings.json"), "{\"hooks\": {}}\n");
        made(&root.join(".claude/hooks/start.sh"), "true\n");
        std::fs::remove_file(root.join("sub/.codex/config.toml")).unwrap();
        made(&root.join("src/.claude.json"), "{}\n");
        made(&root.join("src/main.rs"), "fn main() { changed() }\n");
        let restored = snapshot.restore();

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
        assert_eq!(snapshot.restore(), Restored::default());
    }

    #[test]
    fn a_login_file_and_the_home_s_agent_folders_are_never_read_nor_touched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        made(&root.join(".claude/.credentials.json"), "before");
        let snapshot = Snapshot::take(root, Some(&root.join("home")));
        assert!(snapshot.contents.is_empty());

        made(&root.join(".claude/.credentials.json"), "after");
        made(&root.join("home/.claude/settings.json"), "{}\n");
        made(&root.join(".codex/auth.json"), "{}\n");

        assert_eq!(snapshot.restore(), Restored::default());
        assert!(root.join("home/.claude/settings.json").exists());
    }

    #[test]
    fn an_agent_folder_made_a_link_is_put_back_as_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("place");
        made(&root.join(".claude/settings.json"), "{}\n");
        made(&dir.path().join("elsewhere/settings.json"), "outside\n");
        let snapshot = Snapshot::take(&root, None);

        std::fs::remove_dir_all(root.join(".claude")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), root.join(".claude")).unwrap();
        snapshot.restore();

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
}
