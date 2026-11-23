//! The project's map an agent is given (`docs/design.md` §7.1, "In Full"): where its pull
//! requests and its tickets live, and the command line of each, read from the project's settings
//! and never guessed. Told in the session's part of its instructions, and again when its session
//! switches to Full, where its commands are the user's own and those command lines are signed in.

use std::path::Path;

use flottille_core::declared::{ProjectTracker, RepositoryForge};
use flottille_core::project::Project;

/// The command line of a forge.
fn forge_cli(forge: &RepositoryForge) -> &'static str {
    match forge {
        RepositoryForge::Github { .. } => "gh",
        RepositoryForge::BitbucketCloud { .. } | RepositoryForge::BitbucketDataCenter { .. } => {
            "bkt"
        }
    }
}

/// The map of `project` for an agent in `place` (`main`, or `workspace <name>`), in `folder`, on
/// `branch` (empty in `main`).
pub fn map(project: &Project, place: &str, folder: &Path, branch: &str) -> String {
    let mut text = String::from(
        "# Where this project's work lives\n\n\
         In Full mode, a command you run with `run` is the user's own, as in their terminal: their \
         git and ssh settings, their SSH agent and the forge and tracker command lines' sign-ins. \
         There, use the command line named below for a pull request or a ticket. In Ask and Auto \
         your commands run without them: use the tools `forge` and `tickets`. Following a pull \
         request's checks and reviews, which wakes you, is always `forge`'s.\n\n",
    );
    let at = match branch.is_empty() {
        true => format!("- You work in {place}: the folder {}.\n", folder.display()),
        false => format!(
            "- You work in {place}: the folder {}, on the branch {branch}.\n",
            folder.display()
        ),
    };
    text.push_str(&at);
    for repository in &project.repositories {
        let base = match repository.base.as_slice() {
            [one] => format!("base branch {one}"),
            bases => format!(
                "base branch the first of {} on the remote",
                bases.join(", ")
            ),
        };
        let forge = match &repository.forge {
            Some(forge) => format!("{}, with `{}`", forge.said(), forge_cli(forge)),
            None => "no forge set".to_owned(),
        };
        text.push_str(&format!(
            "- Repository `{}`: remote {}, {base}; {forge}.\n",
            repository.path, repository.remote
        ));
    }
    let tickets = match &project.tracker {
        Some(ProjectTracker::Github { repository }) => {
            format!("GitHub issues of `{repository}`, with `gh`")
        }
        Some(ProjectTracker::Jira { site, projects, .. }) => format!(
            "Jira on {site}, the projects {}, with `acli`",
            projects.join(", ")
        ),
        None => "no tracker set".to_owned(),
    };
    text.push_str(&format!("- Tickets: {tickets}.\n"));
    text
}

/// The map of the agent's project, for its place; none for an agent whose project or workspace
/// cannot be read (an agent of another machine has neither here).
pub fn of_agent(
    daemon: &crate::daemon::Daemon,
    agent: &flottille_core::agent::Agent,
) -> Option<String> {
    let project = crate::projects::load_project(daemon, &agent.project).ok()?;
    let (place, branch) = match &agent.workspace {
        Some(name) => {
            let workspace = daemon
                .read(|store| store.workspace(&project.name, name))
                .ok()??;
            (format!("workspace {name}"), workspace.branch)
        }
        None => ("main".to_owned(), String::new()),
    };
    Some(map(&project, &place, &agent.folder, &branch))
}

/// What the agents of a tree switched to Full are told, between turns: their commands are the
/// user's own from their next run, and where their project's work lives.
pub fn tell_switched_to_full(daemon: &crate::daemon::Daemon, root: &flottille_core::agent::Agent) {
    let agents = std::iter::once(root.clone())
        .filter(|root| !root.state.is_final())
        .chain(crate::tree::descendants(daemon, &root.id));
    for agent in agents {
        let Some(map) = of_agent(daemon, &agent) else {
            continue;
        };
        let body = format!(
            "The user switched this session to Full mode: every call the gate would hold passes, \
             and from your next `run` on, a command you run is the user's own, as in their \
             terminal (their git and ssh settings, their SSH agent, their sign-ins). Flottille's \
             own folders stay out of its reach, and shell syntax stays refused. A command \
             already running is not touched.\n\n{map}"
        );
        crate::deliveries::queue(daemon, &agent.id, "mode", None, &body);
    }
}
