//! Where a project's workspaces are made (`docs/projects.md`, "Where the workspaces go"): the
//! user's default for every project, kept in `workspaces.toml` of Flottille's configuration
//! folder, and a project's own `workspaces_root`.

use std::path::{Component, Path, PathBuf};

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

/// What a folder for workspaces must keep out of besides a project's repositories.
#[derive(Clone, Copy)]
pub struct Avoid<'a> {
    /// Flottille's configuration folder; none when it is not known.
    pub config: Option<&'a Path>,
    /// Flottille's data folder, where only its `workspaces/` is allowed.
    pub data: &'a Path,
    /// The user's home, which is not itself a place for workspaces.
    pub home: Option<&'a Path>,
}

/// `path` with its `.` and `..` folded, so that a folder is compared by where it leads.
fn lexical(path: &Path) -> PathBuf {
    let mut folded = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            part => folded.push(part),
        }
    }
    folded
}

/// Whether workspaces may be made in `folder`: not in one of the project's `repositories` (a
/// workspace is a checkout of it), not in Flottille's configuration or data folders (the data
/// folder's own `workspaces/` is the default) and not in the home folder itself. The error says
/// which, naming `field`.
pub fn allowed(
    folder: &Path,
    avoid: &Avoid,
    repositories: &[PathBuf],
    field: &str,
) -> Result<(), String> {
    let folder = lexical(folder);
    let shown = folder.display();
    if avoid.home.map(lexical).as_deref() == Some(folder.as_path()) {
        return Err(format!(
            "{field}: {shown} is your home folder itself: name a folder inside it"
        ));
    }
    if let Some(repository) = repositories
        .iter()
        .map(|repository| lexical(repository))
        .find(|repository| folder.starts_with(repository))
    {
        return Err(format!(
            "{field}: {shown} is inside the repository {}: a workspace is a checkout of it",
            repository.display()
        ));
    }
    if let Some(config) = avoid.config.map(lexical)
        && folder.starts_with(&config)
    {
        return Err(format!(
            "{field}: {shown} is inside Flottille's configuration folder {}",
            config.display()
        ));
    }
    let data = lexical(avoid.data);
    if folder.starts_with(&data) && !folder.starts_with(data.join("workspaces")) {
        return Err(format!(
            "{field}: {shown} is inside Flottille's data folder {}: only its workspaces folder \
             is allowed",
            data.display()
        ));
    }
    Ok(())
}

/// Whether `text` can be the default for every project: a folder (`typed`) that holds
/// `<project>`, or every project would share one root, and that is allowed (`allowed`).
pub fn checked_default(text: &str, avoid: &Avoid, field: &str) -> Result<(), String> {
    let folder = typed(text, "project", avoid.home, field)?;
    if !text.contains("<project>") {
        return Err(format!(
            "{field}: must hold <project>, or every project would share one folder"
        ));
    }
    allowed(&folder, avoid, &[], field)
}

/// What `workspaces.toml` says: the data folder when there is no file. A folder of the user's
/// that would be refused when set (`checked_default`) is an error here too, so that a hand edit
/// cannot do what the window would not.
pub fn read(file: &Path, avoid: &Avoid) -> Result<Place, String> {
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
        Some("own") => {
            let path = word("path")?.ok_or_else(|| {
                "workspaces.toml: path: a folder of your own needs one".to_owned()
            })?;
            checked_default(path, avoid, "path")
                .map_err(|why| format!("workspaces.toml: {why}"))?;
            Ok(Place::Own(path.to_owned()))
        }
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
    const CONFIG: &str = "/home/someone/.config/flottille";

    fn avoid() -> Avoid<'static> {
        Avoid {
            config: Some(Path::new(CONFIG)),
            data: Path::new(DATA),
            home: Some(Path::new(HOME)),
        }
    }

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
        assert_eq!(read(&file, &avoid()).unwrap(), Place::Data);
        for place in [
            Place::Data,
            Place::Beside,
            Place::Own("~/Worktrees/<project>".into()),
        ] {
            std::fs::write(&file, to_toml(&place)).unwrap();
            assert_eq!(read(&file, &avoid()).unwrap(), place);
        }
        std::fs::write(&file, "place = \"own\"\n").unwrap();
        assert_eq!(
            read(&file, &avoid()).unwrap_err(),
            "workspaces.toml: path: a folder of your own needs one"
        );
        std::fs::write(&file, "place = \"elsewhere\"\n").unwrap();
        assert!(
            read(&file, &avoid())
                .unwrap_err()
                .starts_with("workspaces.toml: place:")
        );
        std::fs::write(&file, "plac = \"data\"\n").unwrap();
        assert_eq!(
            read(&file, &avoid()).unwrap_err(),
            "workspaces.toml: unknown key `plac`"
        );
    }

    fn refusal_of(path: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("workspaces.toml");
        std::fs::write(&file, to_toml(&Place::Own(path.into()))).unwrap();
        read(&file, &avoid()).unwrap_err()
    }

    #[test]
    fn a_hand_edited_path_is_checked_as_the_window_checks_it() {
        assert_eq!(
            refusal_of("worktrees/<project>"),
            "workspaces.toml: path: must be absolute, not \"worktrees/<project>\""
        );
        assert_eq!(refusal_of("  "), "workspaces.toml: path: must not be empty");
        assert_eq!(
            refusal_of("~other/<project>"),
            "workspaces.toml: path: must be absolute, not \"~other/<project>\""
        );
        // `~` with no home to expand it to.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("workspaces.toml");
        std::fs::write(&file, to_toml(&Place::Own("~/<project>".into()))).unwrap();
        let no_home = Avoid {
            home: None,
            ..avoid()
        };
        assert_eq!(
            read(&file, &no_home).unwrap_err(),
            "workspaces.toml: path: cannot expand ~, the home folder is unknown"
        );
    }

    #[test]
    fn an_own_default_without_project_is_refused_as_projects_would_share_it() {
        assert_eq!(
            refusal_of("~/Worktrees"),
            "workspaces.toml: path: must hold <project>, or every project would share one folder"
        );
    }

    #[test]
    fn the_home_folder_itself_is_not_a_place_for_workspaces() {
        let why = checked_default("~", &avoid(), "path").unwrap_err();
        assert!(why.contains("<project>"), "{why}");
        let why = allowed(Path::new(HOME), &avoid(), &[], "path").unwrap_err();
        assert!(why.contains("is your home folder itself"), "{why}");
        // A folder inside it is the usual choice, and `..` cannot hide the home.
        assert!(allowed(Path::new("/home/someone/wt"), &avoid(), &[], "path").is_ok());
        assert!(allowed(Path::new("/home/someone/wt/.."), &avoid(), &[], "path").is_err());
    }

    #[test]
    fn a_folder_inside_a_repository_of_the_project_is_refused() {
        let repositories = [PathBuf::from("/work/app"), PathBuf::from("/work/app/api")];
        let why = allowed(
            Path::new("/work/app/wt"),
            &avoid(),
            &repositories,
            "workspaces_root",
        )
        .unwrap_err();
        assert!(
            why.starts_with("workspaces_root: /work/app/wt is inside the repository /work/app"),
            "{why}"
        );
        // Beside it is not inside it, nor is a folder whose name merely starts the same.
        for outside in ["/work/app-work", "/work/application", "/work"] {
            assert!(
                allowed(Path::new(outside), &avoid(), &repositories, "path").is_ok(),
                "{outside}"
            );
        }
    }

    #[test]
    fn flottille_s_own_folders_are_refused_but_its_workspaces_folder_is_not() {
        let why = allowed(
            Path::new("/home/someone/.config/flottille/projects/x"),
            &avoid(),
            &[],
            "path",
        )
        .unwrap_err();
        assert!(why.contains("Flottille's configuration folder"), "{why}");
        for refused in [
            "/data/flottille",
            "/data/flottille/state",
            "/data/flottille/x/y",
        ] {
            let why = allowed(Path::new(refused), &avoid(), &[], "path").unwrap_err();
            assert!(why.contains("Flottille's data folder"), "{refused}: {why}");
        }
        for fine in [
            "/data/flottille/workspaces",
            "/data/flottille/workspaces/app",
            "/data/flottille-other/app",
        ] {
            assert!(
                allowed(Path::new(fine), &avoid(), &[], "path").is_ok(),
                "{fine}"
            );
        }
        // The data folder's own default resolves to an allowed place.
        assert!(allowed(&resolve(&Place::Data).unwrap(), &avoid(), &[], "path").is_ok());
    }
}
