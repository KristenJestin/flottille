//! Where a project's workspaces are made (`docs/projects.md`, "Where the workspaces go"): the
//! user's default for every project, kept in `workspaces.toml` of Flottille's configuration
//! folder, and a project's own `workspaces_root`.

use std::path::{Path, PathBuf};

/// Where workspaces go, as the user wrote it: `~` and `<project>` are filled in when the folder
/// is resolved, never in what is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Place {
    /// `$XDG_DATA_HOME/flottille/workspaces/<project>`.
    #[default]
    Data,
    /// `<parent of the project's folder>/<project>-work`.
    Beside,
    /// A folder of the user's, as typed.
    Own(String),
}

impl Place {
    /// The word that names it in the file and in the protocol.
    pub fn word(&self) -> &'static str {
        match self {
            Place::Data => "data",
            Place::Beside => "beside",
            Place::Own(_) => "own",
        }
    }

    /// The absolute folder of `project`'s workspaces. `field` names what is refused: `path` for
    /// the default, `workspaces_root` for a project's file.
    pub fn resolve(
        &self,
        project: &str,
        folder: &Path,
        data: &Path,
        home: Option<&Path>,
        field: &str,
    ) -> Result<PathBuf, String> {
        match self {
            Place::Data => Ok(data.join("workspaces").join(project)),
            Place::Beside => {
                let parent = folder.parent().ok_or_else(|| {
                    format!("{field}: {} has no folder to be beside", folder.display())
                })?;
                Ok(parent.join(format!("{project}-work")))
            }
            Place::Own(text) => typed(text, project, home, field),
        }
    }
}

/// The user's home, where a `~` leads: `$HOME` when it is absolute.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// The folder a typed text names: `<project>` replaced, a leading `~` expanded, absolute.
pub fn typed(
    text: &str,
    project: &str,
    home: Option<&Path>,
    field: &str,
) -> Result<PathBuf, String> {
    if text.trim().is_empty() {
        return Err(format!("{field}: must not be empty"));
    }
    let filled = text.replace("<project>", project);
    let folder = match filled.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = home
                .ok_or_else(|| format!("{field}: cannot expand ~, the home folder is unknown"))?;
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(&filled),
    };
    if !folder.is_absolute() {
        return Err(format!("{field}: must be absolute, not \"{text}\""));
    }
    Ok(folder)
}

/// What `workspaces.toml` says: the data folder when there is no file.
pub fn read(file: &Path) -> Result<Place, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Place::Data),
        Err(error) => return Err(format!("workspaces.toml: {error}")),
    };
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| format!("workspaces.toml: {}", e.message()))?;
    if let Some(key) = table
        .keys()
        .find(|key| !matches!(key.as_str(), "place" | "path"))
    {
        return Err(format!("workspaces.toml: unknown key `{key}`"));
    }
    let word = |key: &str| match table.get(key) {
        None => Ok(None),
        Some(toml::Value::String(word)) => Ok(Some(word.as_str())),
        Some(_) => Err(format!("workspaces.toml: {key}: it is a word")),
    };
    match word("place")? {
        None | Some("data") => Ok(Place::Data),
        Some("beside") => Ok(Place::Beside),
        Some("own") => word("path")?
            .map(|path| Place::Own(path.to_owned()))
            .ok_or_else(|| "workspaces.toml: path: a folder of your own needs one".to_owned()),
        Some(other) => Err(format!(
            "workspaces.toml: place: \"{other}\" is not data, beside or own"
        )),
    }
}

/// The text of `workspaces.toml` for `place`.
pub fn to_toml(place: &Place) -> String {
    match place {
        Place::Own(path) => format!(
            "place = \"own\"\npath = {}\n",
            toml::Value::String(path.clone())
        ),
        place => format!("place = \"{}\"\n", place.word()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = "/data/flottille";
    const HOME: &str = "/home/someone";

    fn resolve(place: &Place) -> Result<PathBuf, String> {
        place.resolve(
            "app",
            Path::new("/work/app"),
            Path::new(DATA),
            Some(Path::new(HOME)),
            "path",
        )
    }

    #[test]
    fn the_default_workspaces_folder_is_flottille_s_data_folder() {
        assert_eq!(Place::default(), Place::Data);
        assert_eq!(
            resolve(&Place::Data).unwrap(),
            Path::new("/data/flottille/workspaces/app")
        );
    }

    #[test]
    fn beside_the_project_puts_workspaces_in_project_work() {
        assert_eq!(
            resolve(&Place::Beside).unwrap(),
            Path::new("/work/app-work")
        );
        // The project's name, not its folder's: the folder may be called anything.
        assert_eq!(
            Place::Beside
                .resolve(
                    "shop",
                    Path::new("/srv/clients/web"),
                    Path::new(DATA),
                    None,
                    "path"
                )
                .unwrap(),
            Path::new("/srv/clients/shop-work")
        );
    }

    #[test]
    fn a_folder_of_my_own_replaces_project_and_expands_home() {
        assert_eq!(
            resolve(&Place::Own("~/Worktrees/<project>".into())).unwrap(),
            Path::new("/home/someone/Worktrees/app")
        );
        assert_eq!(
            resolve(&Place::Own("/mnt/<project>/wt".into())).unwrap(),
            Path::new("/mnt/app/wt")
        );
        assert_eq!(
            resolve(&Place::Own("~".into())).unwrap(),
            Path::new("/home/someone")
        );
        // Only a leading `~` of its own part is the home: `~other` and `a~` are folders.
        assert_eq!(
            typed("~other/x", "app", Some(Path::new(HOME)), "path").unwrap_err(),
            "path: must be absolute, not \"~other/x\""
        );
    }

    #[test]
    fn a_relative_folder_is_refused_naming_the_field() {
        assert_eq!(
            resolve(&Place::Own("worktrees/<project>".into())).unwrap_err(),
            "path: must be absolute, not \"worktrees/<project>\""
        );
        assert_eq!(
            typed("rel", "app", None, "workspaces_root").unwrap_err(),
            "workspaces_root: must be absolute, not \"rel\""
        );
        assert_eq!(
            typed("", "app", None, "path").unwrap_err(),
            "path: must not be empty"
        );
        // `~` with no home to expand it to is a refusal, not a folder named `~`.
        assert_eq!(
            typed("~/x", "app", None, "path").unwrap_err(),
            "path: cannot expand ~, the home folder is unknown"
        );
    }

    #[test]
    fn the_file_keeps_what_the_user_typed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("workspaces.toml");
        assert_eq!(read(&file).unwrap(), Place::Data);
        for place in [
            Place::Data,
            Place::Beside,
            Place::Own("~/Worktrees/<project>".into()),
        ] {
            std::fs::write(&file, to_toml(&place)).unwrap();
            assert_eq!(read(&file).unwrap(), place);
        }
        std::fs::write(&file, "place = \"own\"\n").unwrap();
        assert_eq!(
            read(&file).unwrap_err(),
            "workspaces.toml: path: a folder of your own needs one"
        );
        std::fs::write(&file, "place = \"elsewhere\"\n").unwrap();
        assert!(
            read(&file)
                .unwrap_err()
                .starts_with("workspaces.toml: place:")
        );
        std::fs::write(&file, "plac = \"data\"\n").unwrap();
        assert_eq!(
            read(&file).unwrap_err(),
            "workspaces.toml: unknown key `plac`"
        );
    }
}
