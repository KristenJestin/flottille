//! Where an agent works, and how the paths it names are resolved and contained
//! (`docs/tools.md`, `docs/reference/hemera.md` §3 "Path resolution").
//!
//! "Outside" is decided on the text first: a path that is not under the place as written is never
//! looked up. A path under it is then resolved on the disk, links followed (at most 32), and
//! contained by its segments: `..notes` is a name inside, `../x` a step out.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use flottille_core::secrets;

/// Links followed at most, resolving one path.
const LINKS_MAX: usize = 32;

/// The place of an agent: its folder, the project's repositories in it, and what a command run
/// there is given.
#[derive(Clone, Debug)]
pub struct Place {
    pub project: String,
    /// The workspace's name; `None` for the project folder itself (`main`).
    pub workspace: Option<String>,
    /// The agent's folder, absolute.
    pub root: PathBuf,
    /// Each repository: its path in the project, its folder here.
    pub repositories: Vec<(String, PathBuf)>,
    /// The workspace's branch, empty in the project folder.
    pub branch: String,
    /// The variables of a command run here, templates: the workspace's, frozen when it was made,
    /// or the project's in its folder.
    pub variables: BTreeMap<String, String>,
    pub home: Option<PathBuf>,
}

/// A path an agent named, resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    /// As the place shows it: relative to its folder when inside, `~/…` or absolute when not.
    pub shown: String,
    /// Absolute: the real path when inside, the path as written when outside.
    pub path: PathBuf,
    pub inside: bool,
}

impl Place {
    /// The project folder itself (`main`), whose folder is `root`: its repositories where they
    /// are, the project's variables.
    pub fn main(project: &flottille_core::project::Project, root: PathBuf) -> Place {
        Place {
            project: project.name.clone(),
            workspace: None,
            root,
            repositories: project
                .repositories
                .iter()
                .map(|r| {
                    let folder = match r.path.as_str() {
                        "." => project.folder.clone(),
                        path => project.folder.join(path),
                    };
                    (r.path.clone(), folder)
                })
                .collect(),
            branch: String::new(),
            variables: project.variables.clone(),
            home: std::env::var_os("HOME").map(Into::into),
        }
    }

    /// Where relative paths start: the repository's folder, or the place's.
    pub fn base(&self, repository: Option<&str>) -> Result<(&str, &Path), String> {
        let Some(asked) = repository else {
            return Ok((".", &self.root));
        };
        self.repositories
            .iter()
            .find(|(path, _)| path == asked)
            .map(|(path, folder)| (path.as_str(), folder.as_path()))
            .ok_or_else(|| {
                let names: Vec<&str> = self.repositories.iter().map(|(p, _)| p.as_str()).collect();
                format!(
                    "refused: the project has no repository {asked}: it has {}",
                    names.join(", ")
                )
            })
    }

    /// `named`, relative to `base` unless absolute or starting with `~`.
    pub fn resolve(&self, base: &Path, named: &str) -> Resolved {
        let expanded = match (named.strip_prefix('~'), &self.home) {
            (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
                home.join(rest.trim_start_matches('/'))
            }
            _ => PathBuf::from(named),
        };
        let written = lexical(&base.join(expanded));
        let real_root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        if !written.starts_with(&self.root) && !written.starts_with(&real_root) {
            return self.outside(written);
        }
        match real_path(&written) {
            Ok(real) => match real.strip_prefix(&real_root) {
                Ok(relative) => Resolved {
                    shown: shown_inside(relative),
                    path: real,
                    inside: true,
                },
                // A link that leads out: outside, and shown where it leads.
                Err(_) => self.outside(real),
            },
            // Anything that cannot be resolved is not taken for inside.
            Err(_) => self.outside(written),
        }
    }

    fn outside(&self, path: PathBuf) -> Resolved {
        Resolved {
            shown: self.shown_outside(&path),
            path,
            inside: false,
        }
    }

    /// A path outside the place, with the home folder as `~`.
    fn shown_outside(&self, path: &Path) -> String {
        match self.home.as_deref().map(|home| path.strip_prefix(home)) {
            Some(Ok(rest)) if rest.as_os_str().is_empty() => "~".into(),
            Some(Ok(rest)) => format!("~/{}", rest.display()),
            _ => path.display().to_string(),
        }
    }

    /// The values of the files holding secrets (`.env`, `.env.*`) at the top of the place and of
    /// each of its repositories: masked out of what Flottille keeps or shows of a call.
    pub fn secret_values(&self) -> Vec<String> {
        let mut folders: Vec<&Path> = vec![&self.root];
        folders.extend(self.repositories.iter().map(|(_, folder)| folder.as_path()));
        folders.dedup();
        let mut values = Vec::new();
        for folder in folders {
            let Ok(entries) = std::fs::read_dir(folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_str().is_some_and(secrets::secret_file)
                    && let Ok(text) = std::fs::read_to_string(entry.path())
                {
                    values.extend(secrets::file_values(&text));
                }
            }
        }
        values
    }

    /// Whether `path` is inside the place on the disk now, every link followed: `resolve` decides
    /// at a moment, and a folder can be swapped for a link after it.
    pub fn holds(&self, path: &Path) -> bool {
        let real_root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        std::fs::canonicalize(path).is_ok_and(|real| real.starts_with(&real_root))
    }

    /// How a path inside the place is shown.
    pub fn shown(&self, path: &Path) -> String {
        let real_root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        match path.strip_prefix(&real_root) {
            Ok(relative) => shown_inside(relative),
            Err(_) => self.shown_outside(path),
        }
    }
}

fn shown_inside(relative: &Path) -> String {
    if relative.as_os_str().is_empty() {
        ".".into()
    } else {
        relative.display().to_string()
    }
}

/// The path with `.` dropped and each `..` taking the segment before it, without the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The real path, every link followed (at most `LINKS_MAX`); a missing part and what follows it
/// are taken as written, so a file about to be made resolves to its real folder and its name. A
/// dangling link resolves to its target.
fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    let mut pending: Vec<PathBuf> = path
        .components()
        .rev()
        .map(|c| PathBuf::from(c.as_os_str()))
        .collect();
    let mut real = PathBuf::from("/");
    let mut links = 0;
    let mut missing = false;
    while let Some(part) = pending.pop() {
        match part.components().next() {
            Some(Component::RootDir) => {
                real = PathBuf::from("/");
                continue;
            }
            Some(Component::CurDir) | None => continue,
            Some(Component::ParentDir) => {
                real.pop();
                continue;
            }
            _ => {}
        }
        let next = real.join(&part);
        if missing {
            real = next;
            continue;
        }
        match std::fs::symlink_metadata(&next) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                links += 1;
                if links > LINKS_MAX {
                    return Err(std::io::Error::other("too many links"));
                }
                let target = std::fs::read_link(&next)?;
                pending.extend(
                    target
                        .components()
                        .rev()
                        .map(|c| PathBuf::from(c.as_os_str())),
                );
            }
            Ok(_) => real = next,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                missing = true;
                real = next;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(root: &Path) -> Place {
        Place {
            project: "app".into(),
            workspace: None,
            root: root.to_owned(),
            repositories: vec![
                (".".into(), root.to_owned()),
                ("api".into(), root.join("api")),
            ],
            branch: String::new(),
            variables: BTreeMap::new(),
            home: root.parent().map(Path::to_owned),
        }
    }

    #[test]
    fn a_name_starting_with_two_dots_is_inside_and_a_step_out_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        let place = place(&root);

        let notes = place.resolve(&root, "..notes");
        assert!(notes.inside);
        assert_eq!(notes.shown, "..notes");
        for path in ["../x", "a/../../x", "/etc/passwd", "~/x"] {
            assert!(!place.resolve(&root, path).inside, "{path}");
        }
        assert_eq!(place.resolve(&root, "~/x").shown, "~/x");
        assert_eq!(place.resolve(&root, ".").shown, ".");
        assert!(place.resolve(&root, "a/../b").inside);
    }

    #[test]
    fn links_are_followed_and_one_leading_out_is_outside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(dir.path().join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), root.join("out")).unwrap();
        std::os::unix::fs::symlink("src", root.join("alias")).unwrap();
        std::os::unix::fs::symlink("../gone", root.join("src/dangling")).unwrap();
        let place = place(&root);

        assert!(!place.resolve(&root, "out").inside);
        let alias = place.resolve(&root, "alias/new.rs");
        assert!(alias.inside);
        assert_eq!(alias.shown, "src/new.rs");
        let dangling = place.resolve(&root, "src/dangling");
        assert!(dangling.inside);
        assert_eq!(dangling.shown, "gone");
    }

    #[test]
    fn a_loop_of_links_is_outside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink("b", root.join("a")).unwrap();
        std::os::unix::fs::symlink("a", root.join("b")).unwrap();

        assert!(!place(&root).resolve(&root, "a").inside);
    }

    #[test]
    fn a_repository_is_one_of_the_project_s() {
        let dir = tempfile::tempdir().unwrap();
        let place = place(dir.path());

        assert_eq!(place.base(Some("api")).unwrap().1, dir.path().join("api"));
        assert_eq!(
            place.base(Some("web")).unwrap_err(),
            "refused: the project has no repository web: it has ., api"
        );
    }
}
