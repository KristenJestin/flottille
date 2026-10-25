//! Roles, and the instructions of an agent built from its role (`docs/design.md` §4.4, §8,
//! `docs/reference/hemera.md` §7): a shared base, the role's template, the cache boundary, the
//! part of this session, then the project's own instruction files. Built once, when the agent is
//! created, and frozen in it.
//!
//! A role is data: a template (a file of `roles/`) and a few flags. Another role is another file
//! and another entry of `ROLES`.

use std::path::{Path, PathBuf};

use flottille_core::secrets;

use crate::claude::BOUNDARY;

pub struct Role {
    pub name: &'static str,
    pub template: &'static str,
    /// Whether its instructions carry the project's own instruction files.
    pub project_layer: bool,
}

/// The role of an agent the user starts, and the default role of a child.
pub const LEAD: &str = "lead";
pub const WORKER: &str = "worker";

const BASE: &str = include_str!("../roles/base.md");

pub const ROLES: [Role; 2] = [
    Role {
        name: LEAD,
        template: include_str!("../roles/lead.md"),
        project_layer: true,
    },
    Role {
        name: WORKER,
        template: include_str!("../roles/worker.md"),
        project_layer: true,
    },
];

pub fn role(name: &str) -> Option<&'static Role> {
    ROLES.iter().find(|role| role.name == name)
}

/// What the part of a session says, after the boundary.
pub struct Session<'a> {
    pub project: &'a str,
    /// `main`, or `workspace <name>`.
    pub place: &'a str,
    pub folder: &'a Path,
    /// What its parent named its task; `None` for a root.
    pub title: Option<&'a str>,
    /// It cannot launch children of its own.
    pub at_depth_limit: bool,
}

/// One instruction file found for the project layer: its heading and its text.
pub struct File {
    pub heading: String,
    pub text: String,
}

/// The agent's instructions, frozen in it. Everything before the boundary names no project nor
/// session, so that the provider's cache serves it to the next agent of the same role.
pub fn instructions(role: &Role, session: &Session, files: &[File]) -> String {
    let shared = format!(
        "{}\n\n---\n\n{}",
        BASE.trim_end().replace("{role}", role.name),
        role.template.trim_end()
    );
    let mut part = vec![format!(
        "You work for the project {}, in {}: the folder {}.",
        session.project,
        session.place,
        session.folder.display()
    )];
    part.push(match session.title {
        None => "You are the root agent of this session: the user started you and writes to \
                 you, and what your children's requests need from the user reaches them through \
                 you."
            .to_owned(),
        Some(title) => format!(
            "Your task: {title}. Your parent launched you for it; you never talk to the user."
        ),
    });
    if session.at_depth_limit {
        part.push(
            "You are at the depth limit: you cannot launch children, so do all of your task \
             yourself."
                .to_owned(),
        );
    }
    let mut text = format!("{shared}\n\n{BOUNDARY}\n\n{}", part.join(" "));
    if role.project_layer && !files.is_empty() {
        text.push_str("\n\n# The project's own instructions\n");
        for file in files {
            text.push_str(&format!(
                "\n## {}\n\n{}\n",
                file.heading,
                file.text.trim_end()
            ));
        }
    }
    text
}

/// The instruction files of the project layer, in order: for the project folder when it is not
/// itself one of the repositories, then for each repository, its `CLAUDE.md` (Claude's own name
/// first) or else `AGENTS.md`, and its `CLAUDE.local.md` or else `AGENTS.local.md`. Each is read
/// in the agent's place first; a local file, which the user keeps out of the repository and a
/// workspace does not have, is read in the project's own checkout after.
///
/// `repositories` are each `(path in the project, folder in the place, folder in the project)`.
pub fn project_files(
    project_folder: &Path,
    repositories: &[(String, PathBuf, PathBuf)],
) -> Vec<File> {
    let mut files = Vec::new();
    // Frozen in the agent's instructions, which its clients show: masked like anything else.
    let env = std::env::vars_os().collect();
    let mut add = |prefix: &str, folders: &[&Path]| {
        for names in [
            ["CLAUDE.md", "AGENTS.md"],
            ["CLAUDE.local.md", "AGENTS.local.md"],
        ] {
            let found = names.iter().find_map(|name| {
                folders.iter().find_map(|folder| {
                    let text = std::fs::read_to_string(folder.join(name)).ok()?;
                    Some((*name, text))
                })
            });
            if let Some((name, text)) = found
                && !text.trim().is_empty()
            {
                files.push(File {
                    heading: format!("{prefix}{name}"),
                    text: secrets::mask(&text, &env),
                });
            }
        }
    };
    if !repositories.iter().any(|(path, _, _)| path == ".") {
        add("", &[project_folder]);
    }
    for (path, here, original) in repositories {
        let prefix = if path == "." {
            String::new()
        } else {
            format!("{path}/")
        };
        add(&prefix, &[here.as_path(), original.as_path()]);
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(folder: &Path) -> Session<'_> {
        Session {
            project: "zebra",
            place: "workspace login",
            folder,
            title: None,
            at_depth_limit: false,
        }
    }

    #[test]
    fn the_part_before_the_boundary_names_the_role_and_nothing_of_the_session() {
        let folder = Path::new("/work/zebra-login");
        let lead = instructions(role(LEAD).unwrap(), &session(folder), &[]);
        let (before, after) = lead.split_once(BOUNDARY).unwrap();
        assert!(before.contains("the role lead"));
        assert!(before.contains("# Your role: lead"));
        assert!(before.contains("approval_answer"));
        assert!(!before.contains("zebra"));
        assert!(after.contains("zebra") && after.contains("/work/zebra-login"));
        assert!(after.contains("You are the root"));

        let worker = instructions(
            role(WORKER).unwrap(),
            &Session {
                title: Some("fix the login"),
                at_depth_limit: true,
                ..session(folder)
            },
            &[],
        );
        assert!(worker.contains("# Your role: worker"));
        assert!(worker.contains("Your task: fix the login."));
        assert!(worker.contains("depth limit"));
        // Same role, other session: the same text before the boundary.
        let other = instructions(
            role(WORKER).unwrap(),
            &Session {
                project: "other",
                title: Some("x"),
                ..session(Path::new("/elsewhere"))
            },
            &[],
        );
        assert_eq!(
            worker.split_once(BOUNDARY).unwrap().0,
            other.split_once(BOUNDARY).unwrap().0
        );
    }

    #[test]
    fn the_project_layer_takes_claude_s_file_first_and_the_local_ones_from_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let place = dir.path().join("place");
        for folder in [
            project.join("api"),
            project.join("web"),
            place.join("api"),
            place.join("web"),
        ] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(project.join("AGENTS.md"), "root rules").unwrap();
        std::fs::write(place.join("api/CLAUDE.md"), "api for claude").unwrap();
        std::fs::write(place.join("api/AGENTS.md"), "api for any agent").unwrap();
        std::fs::write(place.join("web/AGENTS.md"), "web rules").unwrap();
        // Kept out of the repository: only in the project's own checkout.
        std::fs::write(project.join("web/AGENTS.local.md"), "my web rules").unwrap();
        let repositories = vec![
            ("api".to_owned(), place.join("api"), project.join("api")),
            ("web".to_owned(), place.join("web"), project.join("web")),
        ];

        let files = project_files(&project, &repositories);

        let headings: Vec<&str> = files.iter().map(|f| f.heading.as_str()).collect();
        assert_eq!(
            headings,
            [
                "AGENTS.md",
                "api/CLAUDE.md",
                "web/AGENTS.md",
                "web/AGENTS.local.md"
            ]
        );
        assert_eq!(files[3].text, "my web rules");
        let text = instructions(role(WORKER).unwrap(), &session(&place), &files);
        let after = text.split_once(BOUNDARY).unwrap().1;
        assert!(after.contains("# The project's own instructions\n\n## AGENTS.md\n\nroot rules"));
        assert!(after.contains("## api/CLAUDE.md\n\napi for claude"));
        assert!(!text.contains("api for any agent"));
    }

    #[test]
    fn a_project_that_is_one_repository_reads_its_files_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "rules").unwrap();
        std::fs::write(dir.path().join("CLAUDE.local.md"), "mine").unwrap();
        let repositories = vec![(".".to_owned(), dir.path().to_owned(), dir.path().to_owned())];

        let files = project_files(dir.path(), &repositories);

        let headings: Vec<&str> = files.iter().map(|f| f.heading.as_str()).collect();
        assert_eq!(headings, ["CLAUDE.md", "CLAUDE.local.md"]);
    }

    #[test]
    fn a_local_file_holding_a_token_is_masked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("CLAUDE.local.md"),
            "deploy with token=ghp_abcdefghijklmnopqrstuvwxyz0123 please",
        )
        .unwrap();
        let repositories = vec![(".".to_owned(), dir.path().to_owned(), dir.path().to_owned())];

        let files = project_files(dir.path(), &repositories);

        assert_eq!(files[0].text, "deploy with token=[masked] please");
    }
}
