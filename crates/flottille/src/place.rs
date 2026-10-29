//! Where an agent works, and how the paths it names are resolved and contained
//! (`docs/tools.md`, `docs/reference/hemera.md` §3 "Path resolution").
//!
//! A path is resolved on the disk, links and `..` followed in the order the kernel follows them (at
//! most 40 links), and contained by its segments: `..notes` is a name inside, `../x` a step out.
//! `src/cfg/../x` is where `src/cfg` leads, then up: never `src/x` when `src/cfg` is a link.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};

use flottille_core::secrets;
use rustix::fs::{AtFlags, FileType, Mode};
use rustix::io::Errno;

use crate::nofollow;

/// Links followed at most, resolving one path: Linux follows 40.
const LINKS_MAX: usize = 40;

/// The longest name of one part of a path Linux takes; a longer one is not there.
const NAME_MAX: usize = 255;

/// What a process's folder in `/proc` holds that leads elsewhere: its root, its working folder,
/// its open files, its program, its mapped files.
pub const LEADS_ELSEWHERE: [&str; 5] = ["root", "cwd", "fd", "exe", "map_files"];

/// Why a path could not be followed to where it leads.
#[derive(Clone, Debug, PartialEq)]
pub enum Unfollowed {
    /// It goes through a process's folder in `/proc` that leads elsewhere (`LEADS_ELSEWHERE`),
    /// under whatever name it was reached.
    Through(PathBuf),
    /// The disk would not let it be followed: more than `LINKS_MAX` links, a folder it may not
    /// look into.
    Failed(String),
    /// It names a folder that does not exist, then goes up out of it: a malformed path, which
    /// the kernel refuses too, and whose later parts the gate cannot follow.
    UpFromMissing,
}

impl std::fmt::Display for Unfollowed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Unfollowed::Through(process) => write!(
                f,
                "goes through {}, which leads anywhere on the disk",
                process.display()
            ),
            Unfollowed::Failed(why) => write!(f, "could not be followed on the disk ({why})"),
            Unfollowed::UpFromMissing => {
                f.write_str("names a folder that does not exist and then goes up out of it")
            }
        }
    }
}

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
    /// Absolute: the real path, every link and `..` followed, which the tools open; the path as
    /// written when it cannot be followed (`unfollowed`).
    pub path: PathBuf,
    pub inside: bool,
    /// Absolute, as written: `.` and `..` taken on the text. Checked by its names too.
    pub written: PathBuf,
    /// Why where the path leads is not known.
    pub unfollowed: Option<Unfollowed>,
}

impl Resolved {
    /// A path known to be inside, already real.
    pub fn real_inside(shown: String, path: PathBuf) -> Resolved {
        Resolved {
            shown,
            written: path.clone(),
            path,
            inside: true,
            unfollowed: None,
        }
    }
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
        self.at(&base.join(expanded))
    }

    /// An absolute path, resolved.
    pub fn at(&self, joined: &Path) -> Resolved {
        let written = lexical(joined);
        let real_root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let unfollowed = |why: Unfollowed| Resolved {
            shown: self.shown_outside(&written),
            path: written.clone(),
            inside: false,
            written: written.clone(),
            unfollowed: Some(why),
        };
        match follow(joined) {
            Ok(Followed {
                through: Some(process),
                ..
            }) => unfollowed(Unfollowed::Through(process)),
            Ok(Followed { path: real, .. }) => match real.strip_prefix(&real_root) {
                Ok(relative) => Resolved {
                    shown: shown_inside(relative),
                    path: real.clone(),
                    inside: true,
                    written,
                    unfollowed: None,
                },
                // A link that leads out: outside, and shown where it leads.
                Err(_) => Resolved {
                    shown: self.shown_outside(&real),
                    path: real,
                    inside: false,
                    written,
                    unfollowed: None,
                },
            },
            // Anything that cannot be resolved is neither inside nor known.
            Err(why) => unfollowed(why),
        }
    }

    /// A path outside the place, with the home folder as `~`.
    fn shown_outside(&self, path: &Path) -> String {
        let homes = self
            .home
            .iter()
            .flat_map(|home| [Some(home.clone()), std::fs::canonicalize(home).ok()])
            .flatten();
        for home in homes {
            match path.strip_prefix(&home) {
                Ok(rest) if rest.as_os_str().is_empty() => return "~".into(),
                Ok(rest) => return format!("~/{}", rest.display()),
                Err(_) => {}
            }
        }
        path.display().to_string()
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

/// A path followed on the disk.
pub struct Followed {
    /// The real path; below a process's folder in `/proc`, as written from there.
    pub path: PathBuf,
    /// The first of a process's folders in `/proc` that leads elsewhere (`LEADS_ELSEWHERE`) the
    /// path went through, under whatever name it was reached (`/dev/fd`, a link).
    pub through: Option<PathBuf>,
}

/// The real path, every link followed (at most `LINKS_MAX`); a missing part and what follows it
/// are taken as written, so a file about to be made resolves to its real folder and its name. A
/// dangling link resolves to its target.
pub fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    follow(path)
        .map(|followed| followed.path)
        .map_err(|why| std::io::Error::other(why.to_string()))
}

/// `real_path`, saying which process's folder in `/proc` it went through. Below `/proc`, links
/// are not followed: `/proc/self` is the reader's own, which is not Flottille's, and what a
/// process's folder leads to is its own; a `..` there is where such a link leads, so the path
/// goes through it. A part the kernel would not find (missing, too long a name, below a file) is
/// missing; a `..` after it is refused, since nothing after it can be followed.
///
/// Each part is looked at in the folder opened before it, as the kernel walks: the whole path,
/// links expanded, may be longer than it takes in one name.
pub fn follow(path: &Path) -> Result<Followed, Unfollowed> {
    let failed = |error: Errno| Unfollowed::Failed(std::io::Error::from(error).to_string());
    let mut pending: Vec<PathBuf> = path
        .components()
        .rev()
        .map(|c| PathBuf::from(c.as_os_str()))
        .collect();
    let mut real = PathBuf::from("/");
    // The open folders of `real`, `/` first; `file` when its last part is not a folder.
    let root = rustix::fs::open("/", nofollow::THROUGH, Mode::empty()).map_err(failed)?;
    let mut folders = vec![root];
    let mut file = false;
    let mut links = 0;
    let mut missing = false;
    let mut through = None;
    while let Some(part) = pending.pop() {
        match part.components().next() {
            Some(Component::RootDir) => {
                real = PathBuf::from("/");
                folders.truncate(1);
                file = false;
                continue;
            }
            Some(Component::CurDir) | None => continue,
            Some(Component::ParentDir) if missing => return Err(Unfollowed::UpFromMissing),
            Some(Component::ParentDir)
                if real.starts_with("/proc") && real != Path::new("/proc") =>
            {
                let process = through.unwrap_or_else(|| real.clone());
                return Ok(Followed {
                    path: real,
                    through: Some(process),
                });
            }
            Some(Component::ParentDir) => {
                if file {
                    file = false;
                } else if folders.len() > 1 {
                    folders.pop();
                }
                real.pop();
                continue;
            }
            _ => {}
        }
        let next = real.join(&part);
        if missing || real.starts_with("/proc") {
            real = next;
            if through.is_none() && leads_elsewhere(&real) {
                through = Some(real.clone());
            }
            continue;
        }
        let name = part.as_os_str();
        if file || name.len() > NAME_MAX {
            missing = true;
            real = next;
            continue;
        }
        let folder = folders.last().expect("`/` is never left");
        match rustix::fs::statat(folder, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => match FileType::from_raw_mode(stat.st_mode) {
                FileType::Symlink => {
                    links += 1;
                    if links > LINKS_MAX {
                        return Err(Unfollowed::Failed(format!("more than {LINKS_MAX} links")));
                    }
                    let target =
                        rustix::fs::readlinkat(folder, name, Vec::new()).map_err(failed)?;
                    let target = PathBuf::from(OsString::from_vec(target.into_bytes()));
                    pending.extend(
                        target
                            .components()
                            .rev()
                            .map(|c| PathBuf::from(c.as_os_str())),
                    );
                }
                FileType::Directory => {
                    let opened = rustix::fs::openat(folder, name, nofollow::THROUGH, Mode::empty())
                        .map_err(failed)?;
                    folders.push(opened);
                    real = next;
                }
                _ => {
                    file = true;
                    real = next;
                }
            },
            Err(Errno::NOENT) => {
                missing = true;
                real = next;
            }
            Err(error) => return Err(failed(error)),
        }
    }
    Ok(Followed {
        path: real,
        through,
    })
}

/// Whether a path is a process's folder in `/proc` that leads elsewhere, or below one.
fn leads_elsewhere(path: &Path) -> bool {
    let Ok(rest) = path.strip_prefix("/proc") else {
        return false;
    };
    rest.components()
        .skip(1)
        .any(|part| LEADS_ELSEWHERE.contains(&part.as_os_str().to_string_lossy().as_ref()))
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
        std::fs::create_dir(root.join("a")).unwrap();
        assert!(place.resolve(&root, "a/../b").inside);
        // Up out of a folder that is not there: the kernel would not find it.
        assert_eq!(
            place.resolve(&root, "m/../b").unfollowed,
            Some(Unfollowed::UpFromMissing)
        );
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
