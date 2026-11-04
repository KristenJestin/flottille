//! The projects' files: `$XDG_CONFIG_HOME/flottille/projects/<name>.toml`, one per project.

use std::io::ErrorKind;
use std::path::PathBuf;

use crate::paths::Paths;
use crate::place::{self, Place};
use crate::project::{ConfigError, Places, Project, valid_name};
use crate::{Error, Result};

pub struct Registry {
    dir: PathBuf,
    config: PathBuf,
    data: PathBuf,
    /// The user's default place for workspaces, read at every load so that a change applies at
    /// once.
    default: PathBuf,
    home: Option<PathBuf>,
}

/// Every file of the folder, read.
#[derive(Debug, Default)]
pub struct Loaded {
    /// By name.
    pub projects: Vec<Project>,
    /// The files that cannot be used, by name.
    pub invalid: Vec<ConfigError>,
}

impl Registry {
    pub fn new(paths: &Paths) -> Registry {
        Registry {
            dir: paths.projects(),
            config: paths.config.clone(),
            data: paths.data.clone(),
            default: paths.workspaces_place(),
            home: place::home(),
        }
    }

    /// Where the workspaces of a project that names no folder go now. A default that cannot be
    /// read is Flottille's data folder, since one broken file must not hide every project: the
    /// window says why it is not followed (`workspaces_folder.show`).
    fn place(&self) -> Place {
        self.place_read().unwrap_or_default()
    }

    /// What `workspaces.toml` says, or why it cannot be followed.
    pub fn place_read(&self) -> std::result::Result<Place, String> {
        place::read(&self.default, &self.avoid())
    }

    /// What a folder for workspaces keeps out of.
    pub fn avoid(&self) -> place::Avoid<'_> {
        place::Avoid {
            config: Some(&self.config),
            data: &self.data,
            home: self.home.as_deref(),
        }
    }

    pub fn load_all(&self) -> Result<Loaded> {
        let mut loaded = Loaded::default();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(loaded),
            Err(error) => return Err(error.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            // A hidden file is an editor's or ours, in the making.
            let file = entry.file_name();
            let Some(name) = file.to_str().and_then(|file| file.strip_suffix(".toml")) else {
                continue;
            };
            if !name.starts_with('.') && entry.path().is_file() {
                names.push(name.to_owned());
            }
        }
        names.sort();
        for name in names {
            match self.load(&name) {
                Ok(Some(project)) => loaded.projects.push(project),
                // Gone since the folder was listed.
                Ok(None) => {}
                Err(Error::Config(error)) => loaded.invalid.push(error),
                Err(Error::Io(error)) => loaded.invalid.push(ConfigError {
                    name,
                    message: format!("cannot be read: {error}"),
                }),
                Err(error) => return Err(error),
            }
        }
        Ok(loaded)
    }

    /// `None` when there is no such project. A file that cannot be used is `Error::Config`.
    pub fn load(&self, name: &str) -> Result<Option<Project>> {
        if !valid_name(name) {
            return Ok(None);
        }
        let text = match std::fs::read_to_string(self.file(name)) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        self.parse(name, &text).map(Some).map_err(Error::Config)
    }

    /// Checks the text of a project's file as `load` would read it.
    pub fn parse(&self, name: &str, text: &str) -> std::result::Result<Project, ConfigError> {
        let default = self.place();
        let places = Places {
            config: Some(&self.config),
            data: &self.data,
            home: self.home.as_deref(),
            default: &default,
        };
        Project::parse_in(name, text, &places)
    }

    /// Writes the project's file, which must not exist yet. A reader sees the whole file or none.
    pub fn create(&self, project: &Project) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let file = self.file(&project.name);
        let writing = self.dir.join(format!(".{}.toml.tmp", project.name));
        std::fs::write(&writing, project.to_toml()?)?;
        // A link, not a rename: it fails when the file exists instead of replacing it.
        let linked = std::fs::hard_link(&writing, &file);
        std::fs::remove_file(&writing)?;
        match linked {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(Error::Refused(format!(
                "a project named {} already exists",
                project.name
            ))),
            linked => Ok(linked?),
        }
    }

    /// Removes the project's file, and nothing else. Whether there was one.
    pub fn remove(&self, name: &str) -> Result<bool> {
        if !valid_name(name) {
            return Ok(false);
        }
        match std::fs::remove_file(self.file(name)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Where the project's file is, whether it exists or not.
    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.toml"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const TEXT: &str = "folder = \"/work/app\"\n[[repositories]]\npath = \".\"\n";

    fn registry() -> (tempfile::TempDir, Registry) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            runtime: dir.path().join("runtime"),
            state: dir.path().join("state"),
            config: dir.path().join("config"),
            data: dir.path().join("data"),
        };
        let registry = Registry::new(&paths);
        (dir, registry)
    }

    fn project(name: &str) -> Project {
        Project::parse(name, TEXT, std::path::Path::new("/data")).unwrap()
    }

    #[test]
    fn a_created_project_is_loaded_back() {
        let (dir, registry) = registry();
        // The file names no folder for the workspaces: they follow the data folder of the day.
        let app = Project::parse("app", TEXT, &dir.path().join("data")).unwrap();

        registry.create(&app).unwrap();

        assert_eq!(registry.load("app").unwrap(), Some(app));
    }

    #[test]
    fn the_default_in_workspaces_toml_is_followed_at_once_by_a_project_with_no_folder() {
        let (dir, registry) = registry();
        registry.create(&project("app")).unwrap();
        let root = |registry: &Registry| registry.load("app").unwrap().unwrap().workspaces_root;
        assert_eq!(root(&registry), dir.path().join("data/workspaces/app"));

        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        let file = dir.path().join("config/workspaces.toml");
        std::fs::write(&file, "place = \"beside\"\n").unwrap();
        assert_eq!(root(&registry), Path::new("/work/app-work"));

        // A file that cannot be read hides no project: the data folder is followed.
        std::fs::write(&file, "place = \"elsewhere\"\n").unwrap();
        assert_eq!(root(&registry), dir.path().join("data/workspaces/app"));
    }

    #[test]
    fn a_project_that_does_not_exist_is_none() {
        let (_dir, registry) = registry();

        assert_eq!(registry.load("app").unwrap(), None);
        assert!(registry.load_all().unwrap().projects.is_empty());
    }

    #[test]
    fn a_name_that_is_a_path_finds_nothing() {
        let (dir, registry) = registry();
        std::fs::create_dir(dir.path().join("config")).unwrap();
        std::fs::write(dir.path().join("config/secret.toml"), TEXT).unwrap();

        assert_eq!(registry.load("../secret").unwrap(), None);
        assert!(!registry.remove("../secret").unwrap());
    }

    #[test]
    fn a_project_is_never_overwritten() {
        let (_dir, registry) = registry();
        registry.create(&project("app")).unwrap();

        let error = registry.create(&project("app")).unwrap_err();

        assert!(error.to_string().contains("already exists"), "{error}");
    }

    #[test]
    fn every_file_is_read_and_the_ones_that_fail_are_named() {
        let (dir, registry) = registry();
        registry.create(&project("b")).unwrap();
        registry.create(&project("a")).unwrap();
        let folder = dir.path().join("config/projects");
        std::fs::write(folder.join("broken.toml"), "max_children = 13").unwrap();
        std::fs::write(folder.join("notes.txt"), "not a project").unwrap();
        std::fs::write(folder.join(".a.toml.swp"), "an editor's file").unwrap();

        let loaded = registry.load_all().unwrap();

        let names: Vec<_> = loaded.projects.iter().map(|p| &p.name[..]).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(loaded.invalid.len(), 1);
        assert_eq!(loaded.invalid[0].name, "broken");
        assert_eq!(
            registry.load("broken").unwrap_err().to_string(),
            loaded.invalid[0].to_string()
        );
    }

    #[test]
    fn removing_a_project_removes_its_file_only() {
        let (dir, registry) = registry();
        registry.create(&project("app")).unwrap();
        registry.create(&project("other")).unwrap();

        assert!(registry.remove("app").unwrap());

        assert_eq!(registry.load("app").unwrap(), None);
        assert!(registry.load("other").unwrap().is_some());
        assert!(!registry.remove("app").unwrap());
        let left: Vec<_> = std::fs::read_dir(dir.path().join("config/projects"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["other.toml"]);
    }
}
