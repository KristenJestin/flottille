//! Templates and the environment of the commands run in a workspace (`docs/workspaces.md`).
//!
//! A project's variables are templates (`STORE = "{workspace.path}/.store"`), filled for the
//! workspace a command runs in, then laid over the daemon's own environment. They replace the
//! shell hooks (`.envrc`) a project would otherwise need. The recipe's `run` steps use this, and so
//! will every command run in a workspace later.

use std::collections::BTreeMap;
use std::ffi::OsString;

/// The names a template may hold, for `{name}`.
pub const NAMES: [&str; 5] = [
    "workspace",
    "workspace.path",
    "project",
    "branch",
    "repository",
];

/// What the names of a template stand for, where a command runs.
#[derive(Clone, Debug)]
pub struct Names {
    /// The workspace's name.
    pub workspace: String,
    /// The workspace's folder, absolute.
    pub workspace_path: String,
    pub project: String,
    pub branch: String,
    /// The path of the repository the command runs in, `.` for the workspace folder.
    pub repository: String,
}

impl Names {
    fn get(&self, name: &str) -> Option<&str> {
        Some(match name {
            "workspace" => &self.workspace,
            "workspace.path" => &self.workspace_path,
            "project" => &self.project,
            "branch" => &self.branch,
            "repository" => &self.repository,
            _ => return None,
        })
    }
}

/// `template` with its names filled. `{{` and `}}` are a literal brace.
pub fn fill(template: &str, names: &Names) -> Result<String, String> {
    let mut filled = String::new();
    walk(template, |piece| match piece {
        Piece::Text(text) => {
            filled.push_str(text);
            Ok(())
        }
        Piece::Name(name) => {
            filled.push_str(names.get(name).ok_or_else(|| unknown(name))?);
            Ok(())
        }
    })?;
    Ok(filled)
}

/// Whether every name of `template` is one Flottille fills, without filling it.
pub fn check(template: &str) -> Result<(), String> {
    walk(template, |piece| match piece {
        Piece::Name(name) if !NAMES.contains(&name) => Err(unknown(name)),
        _ => Ok(()),
    })
}

fn unknown(name: &str) -> String {
    format!(
        "“{{{name}}}” is not a name Flottille fills: use {}, or {{{{ for a brace",
        NAMES.map(|name| format!("{{{name}}}")).join(", ")
    )
}

enum Piece<'a> {
    Text(&'a str),
    Name(&'a str),
}

fn walk<'a>(
    template: &'a str,
    mut each: impl FnMut(Piece<'a>) -> Result<(), String>,
) -> Result<(), String> {
    let mut rest = template;
    while let Some(at) = rest.find(['{', '}']) {
        each(Piece::Text(&rest[..at]))?;
        let brace = &rest[at..at + 1];
        if rest[at + 1..].starts_with(brace) {
            each(Piece::Text(brace))?;
            rest = &rest[at + 2..];
            continue;
        }
        if brace == "}" {
            return Err("a “}” is never opened: write }} for a brace".into());
        }
        let close = rest[at..]
            .find('}')
            .ok_or("a “{” is never closed: write {{ for a brace")?;
        each(Piece::Name(&rest[at + 1..at + close]))?;
        rest = &rest[at + close + 1..];
    }
    each(Piece::Text(rest))
}

/// The environment of a command: `process` (the daemon's own), then each layer in turn, a later
/// one overriding an earlier one: the project's variables filled for the workspace, then the
/// workspace's own. A `PATH` of a layer is put in front of the `PATH` before it, never in its
/// place, so that the programs of the system are still found.
pub fn environment(
    process: impl IntoIterator<Item = (OsString, OsString)>,
    layers: &[&BTreeMap<String, String>],
) -> BTreeMap<OsString, OsString> {
    let mut merged: BTreeMap<OsString, OsString> = process.into_iter().collect();
    for layer in layers {
        for (name, value) in *layer {
            let key = OsString::from(name);
            let value = match merged.get(&key) {
                Some(before) if name == "PATH" && !before.is_empty() => {
                    let mut path = OsString::from(value);
                    path.push(":");
                    path.push(before);
                    path
                }
                _ => OsString::from(value),
            };
            merged.insert(key, value);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            workspace: "login".into(),
            workspace_path: "/work/app-work/login".into(),
            project: "app".into(),
            branch: "app/login".into(),
            repository: "app-api".into(),
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn os(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }

    #[test]
    fn every_name_is_filled() {
        assert_eq!(
            fill(
                "{workspace.path}/.store {project}-{workspace} {branch} {repository}",
                &names()
            )
            .unwrap(),
            "/work/app-work/login/.store app-login app/login app-api"
        );
    }

    #[test]
    fn a_double_brace_is_a_literal_brace() {
        assert_eq!(
            fill("{{\"a\": {project}}}", &names()).unwrap(),
            "{\"a\": app}"
        );
        assert!(check("{{x}}").is_ok());
    }

    #[test]
    fn a_name_flottille_does_not_fill_is_refused_with_the_names_it_does() {
        let error = check("{home}/x").unwrap_err();
        assert!(
            error.starts_with("“{home}” is not a name Flottille fills"),
            "{error}"
        );
        assert!(error.contains("{workspace.path}"), "{error}");
        assert_eq!(fill("{home}", &names()).unwrap_err(), error);
    }

    #[test]
    fn a_brace_left_open_is_refused() {
        assert!(check("{workspace").unwrap_err().contains("never closed"));
        assert!(check("a}").unwrap_err().contains("never opened"));
    }

    #[test]
    fn the_layers_override_the_process_in_order() {
        let project = map(&[("A", "project"), ("B", "project")]);
        let workspace = map(&[("B", "workspace")]);

        let merged = environment(
            os(&[("A", "process"), ("C", "process")]),
            &[&project, &workspace],
        );

        assert_eq!(merged[&OsString::from("A")], "project");
        assert_eq!(merged[&OsString::from("B")], "workspace");
        assert_eq!(merged[&OsString::from("C")], "process");
    }

    #[test]
    fn a_path_is_a_prefix_not_a_replacement() {
        let project = map(&[("PATH", "/ws/bin")]);
        let workspace = map(&[("PATH", "/ws/tools")]);

        let merged = environment(os(&[("PATH", "/usr/bin:/bin")]), &[&project, &workspace]);

        assert_eq!(
            merged[&OsString::from("PATH")],
            "/ws/tools:/ws/bin:/usr/bin:/bin"
        );
        let alone = environment(Vec::new(), &[&project]);
        assert_eq!(alone[&OsString::from("PATH")], "/ws/bin");
    }
}
