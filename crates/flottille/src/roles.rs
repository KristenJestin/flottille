//! Roles, and the instructions of an agent built from its role (`docs/design.md` §4.4, §8,
//! `docs/reference/hemera.md` §7): a shared base (what Flottille enforces, then the rules the user
//! edits), the role's template, the cache boundary, the part of this session, then the project's
//! own instruction files. Built once, when the agent is created, and frozen in it.
//!
//! A role is data: a template (a file of `roles/`) and a few flags. Another role is another file
//! and another entry of `ROLES`.

use std::path::{Path, PathBuf};

use flottille_core::secrets;

use crate::claude::BOUNDARY;
use crate::rules::{Rules, Source};

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
    /// The project's skills: name and one line each, never the content (`skill_read`).
    pub skills: &'a [(String, String)],
    /// How the user wants it to work.
    pub rules: &'a Rules,
}

/// One instruction file found for the project layer: its heading and its text.
pub struct File {
    pub heading: String,
    pub text: String,
}

/// The agent's instructions, frozen in it. Everything before the boundary names no project nor
/// session, so that the provider's cache serves it to the next agent of the same role. The rules
/// are the same for every project until a project has its own: they stay before the boundary,
/// and a project's own go after it, since they would otherwise make the shared part differ
/// from one project to the next.
pub fn instructions(role: &Role, session: &Session, files: &[File]) -> String {
    let env = std::env::vars_os().collect();
    let rules = secrets::mask(session.rules.text.trim(), &env);
    let own = session.rules.source == Source::Project;
    let mut shared = BASE.trim_end().replace("{role}", role.name);
    if !own && !rules.is_empty() {
        shared.push_str(&format!("\n\n---\n\n# How to work\n\n{rules}"));
    }
    let shared = format!("{shared}\n\n---\n\n{}", role.template.trim_end());
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
    if own && !rules.is_empty() {
        text.push_str(&format!("\n\n# How to work, in this project\n\n{rules}"));
    }
    if !session.skills.is_empty() {
        text.push_str(
            "\n\n# The project's skills\n\nTexts of the project that are not loaded here: \
             read one with the tool skill_read when your task needs it.\n\n",
        );
        for (name, description) in session.skills {
            text.push_str(&format!("- {name}: {}\n", secrets::mask(description, &env)));
        }
    }
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
            skills: &[],
            rules: &DEFAULT_RULES,
        }
    }

    static DEFAULT_RULES: std::sync::LazyLock<Rules> = std::sync::LazyLock::new(|| Rules {
        text: crate::rules::DEFAULT.to_owned(),
        source: Source::Default,
    });

    #[test]
    fn the_enforced_text_comes_first_then_the_rules_then_the_role_and_all_before_the_boundary() {
        let folder = Path::new("/work/zebra");
        let text = instructions(role(WORKER).unwrap(), &session(folder), &[]);
        let (before, after) = text.split_once(BOUNDARY).unwrap();
        let at = |needle: &str| before.find(needle).unwrap_or_else(|| panic!("no {needle}"));
        assert!(at("# What Flottille enforces") < at("# How to work"));
        assert!(at("# How to work") < at("# Your role: worker"));
        assert!(before.contains("Commit only when your brief or the user asks for it"));
        assert!(!after.contains("How to work"));
    }

    #[test]
    fn the_user_s_global_rules_replace_the_default_before_the_boundary() {
        let rules = Rules {
            text: "Answer in French.\n".into(),
            source: Source::Global,
        };
        let folder = Path::new("/work/zebra");
        let text = instructions(
            role(WORKER).unwrap(),
            &Session {
                rules: &rules,
                ..session(folder)
            },
            &[],
        );

        let (before, _) = text.split_once(BOUNDARY).unwrap();
        assert!(before.contains("# How to work\n\nAnswer in French.\n\n---"));
        assert!(!text.contains("Commit only when"));
    }

    #[test]
    fn a_project_s_rules_replace_the_global_ones_and_go_after_the_boundary() {
        let global = Rules {
            text: "Answer in French.".into(),
            source: Source::Global,
        };
        let own = Rules {
            text: "Answer in Spanish.".into(),
            source: Source::Project,
        };
        let folder = Path::new("/work/zebra");
        let with = |rules: &Rules, project: &str| {
            instructions(
                role(WORKER).unwrap(),
                &Session {
                    rules,
                    project,
                    ..session(folder)
                },
                &[],
            )
        };

        let text = with(&own, "zebra");
        let (before, after) = text.split_once(BOUNDARY).unwrap();
        assert!(!before.contains("How to work") && !before.contains("Spanish"));
        assert!(after.contains("# How to work, in this project\n\nAnswer in Spanish."));
        assert!(!text.contains("French") && !text.contains("Commit only when"));
        // The shared part is the same for every project that has its own rules, and so is the
        // one of those that follow the global rules.
        assert_eq!(
            text.split_once(BOUNDARY).unwrap().0,
            with(&own, "other").split_once(BOUNDARY).unwrap().0
        );
        assert_eq!(
            with(&global, "zebra").split_once(BOUNDARY).unwrap().0,
            with(&global, "other").split_once(BOUNDARY).unwrap().0
        );
    }

    #[test]
    fn rules_left_empty_leave_no_section_and_a_token_in_them_is_masked() {
        let folder = Path::new("/work/zebra");
        for source in [Source::Global, Source::Project] {
            let rules = Rules {
                text: " \n".into(),
                source,
            };
            let text = instructions(
                role(WORKER).unwrap(),
                &Session {
                    rules: &rules,
                    ..session(folder)
                },
                &[],
            );
            assert!(!text.contains("How to work"), "{source:?}");
        }
        let rules = Rules {
            text: "Deploy with token=ghp_abcdefghijklmnopqrstuvwxyz0123".into(),
            source: Source::Global,
        };
        let text = instructions(
            role(WORKER).unwrap(),
            &Session {
                rules: &rules,
                ..session(folder)
            },
            &[],
        );
        assert!(text.contains("Deploy with token=[masked]"));
    }

    #[test]
    fn the_skills_are_listed_by_name_and_description_after_the_boundary() {
        let skills = [
            ("react".to_owned(), "How we write components".to_owned()),
            ("style".to_owned(), "The code style".to_owned()),
        ];
        let folder = Path::new("/work/zebra");
        let text = instructions(
            role(WORKER).unwrap(),
            &Session {
                skills: &skills,
                ..session(folder)
            },
            &[],
        );

        let (before, after) = text.split_once(BOUNDARY).unwrap();
        assert!(!before.contains("react"));
        assert!(after.contains(
            "# The project's skills\n\nTexts of the project that are not loaded here: \
             read one with the tool skill_read when your task needs it.\n\n\
             - react: How we write components\n- style: The code style\n"
        ));
        let none = instructions(role(WORKER).unwrap(), &session(folder), &[]);
        assert!(!none.contains("skills"));
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
