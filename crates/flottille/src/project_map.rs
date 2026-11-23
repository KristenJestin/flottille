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
         In Full mode, a command you run with `run` is the user's own, as in their terminal, and \
         nothing is held back from it: their git and ssh settings, their SSH agent, their keyring \
         and sign-ins, Flottille's vault and the key that signs as this device. Nothing stops a \
         mistake there: take the care the user would. Use the command line named below for a \
         pull request or a ticket. In Ask and Auto your commands are confined and have none of \
         them: use the tools `forge` and `tickets`. Following a pull request's checks and \
         reviews, which wakes you, is always `forge`'s.\n\n",
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
        let at = match repository.path.as_str() {
            "." => folder.to_owned(),
            path => folder.join(path),
        };
        text.push_str(&format!(
            "- Repository `{}`, in the folder {}: remote {}, {base}; {forge}.\n",
            repository.path,
            at.display(),
            repository.remote
        ));
    }
    let tickets = match &project.tracker {
        Some(ProjectTracker::Github { repository }) => {
            format!("GitHub issues of `{repository}`, with `gh`")
        }
        // A write to Jira goes through the gate's `tickets` alone (`docs/tickets.md`).
        Some(ProjectTracker::Jira { site, projects, .. }) => format!(
            "Jira on {site}, the projects {}, read with `acli`; never write to Jira with `acli`: \
             a change to a ticket goes through the tool `tickets`",
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
/// user's own from their next run, held back from nothing, and where their project's work lives.
pub fn tell_switched_to_full(daemon: &crate::daemon::Daemon, root: &flottille_core::agent::Agent) {
    for agent in live(daemon, root) {
        let Some(map) = of_agent(daemon, &agent) else {
            continue;
        };
        let body = format!(
            "The user switched this session to Full mode: every call the gate would hold passes, \
             and from your next `run` on, a command you run is the user's own, as in their \
             terminal. Nothing is held back from it: their git and ssh settings, their SSH agent, \
             their keyring and sign-ins, Flottille's vault and the key that signs as this device. \
             Nothing stops a mistake, so take the care the user would. Shell syntax stays \
             refused, and `run` stays your only way. A command already running is not \
             touched.\n\n{map}"
        );
        queue_mode(daemon, &agent.id, &body);
    }
}

/// Queues a delivery of the mode: the newest replaces the older ones the agent has not read, so
/// that Full, Auto, Full, Auto before its next turn tells it once, the last.
fn queue_mode(daemon: &crate::daemon::Daemon, agent: &str, body: &str) {
    let _ = daemon.write(|store| Ok(((), store.supersede_like(agent, &["mode-"])?)));
    crate::deliveries::queue(daemon, agent, "mode", None, body);
}

/// What the agents of a tree switched out of Full are told, between turns: their commands are
/// confined again from their next run.
pub fn tell_left_full(
    daemon: &crate::daemon::Daemon,
    root: &flottille_core::agent::Agent,
    mode: &str,
) {
    for agent in live(daemon, root) {
        let body = format!(
            "The user switched this session out of Full mode, to {mode}: from your next `run` \
             on, a command you run is confined again, with none of the user's keys, sign-ins or \
             keyring. For a pull request or a ticket, use the tools `forge` and `tickets`. A \
             command started in Full and still running keeps what it started with."
        );
        queue_mode(daemon, &agent.id, &body);
    }
}

/// The live agents of the tree of `root`.
fn live(
    daemon: &crate::daemon::Daemon,
    root: &flottille_core::agent::Agent,
) -> Vec<flottille_core::agent::Agent> {
    std::iter::once(root.clone())
        .filter(|root| !root.state.is_final())
        .chain(crate::tree::descendants(daemon, &root.id))
        .collect()
}
