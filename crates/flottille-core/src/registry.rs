//! The projects' files: `$XDG_CONFIG_HOME/flottille/projects/<name>.toml`, one per project.

use std::io::ErrorKind;
use std::path::PathBuf;

use crate::paths::Paths;
use crate::project::{ConfigError, Project, valid_name};
use crate::{Error, Result};

pub struct Registry {
    dir: PathBuf,
    data: PathBuf,
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
            data: paths.data.clone(),
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
        Project::parse(name, &text, &self.data)
            .map(Some)
            .map_err(Error::Config)
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

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.toml"))
    }
}

#[cfg(test)]
mod tests {
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
        let (_dir, registry) = registry();
        let app = project("app");

        registry.create(&app).unwrap();

        assert_eq!(registry.load("app").unwrap(), Some(app));
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
        std::fs::write(folder.join("broken.toml"), "max_children = 9").unwrap();
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
