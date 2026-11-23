//! What a runner offers its peers (`docs/remote.md`, "Roles"): the `capabilities` machine
//! message, its projects with their free slots and the agents signed in on it, sent over the
//! link when a link is made and again when any of them changes; and which runner takes a
//! session, said the same way wherever a launch on another machine is judged.

use serde::{Deserialize, Serialize};

/// The most projects a runner's offer may name.
pub const MOST_PROJECTS: usize = 1024;
/// The most agents it may name.
pub const MOST_AGENTS: usize = 64;
/// The longest name of a project or an agent it may hold, in characters.
const NAME_LIMIT: usize = 64;

/// What a runner offers: each project registered on it, with its free slots, and the agent CLIs
/// installed and signed in there (their login files checked for presence only, never read).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub projects: Vec<Offered>,
    /// The providers' names (`claude`, `codex`, …).
    pub agents: Vec<String>,
}

/// A project of a runner's offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offered {
    pub name: String,
    /// Its cap of children running at once, minus those that run.
    pub slots: u32,
}

impl Capabilities {
    /// Whether another machine's offer is one to keep: within the bounds, and every name one a
    /// client may show as it is.
    pub fn check(&self) -> Result<(), String> {
        if self.projects.len() > MOST_PROJECTS {
            return Err(format!("an offer names {MOST_PROJECTS} projects at most"));
        }
        if self.agents.len() > MOST_AGENTS {
            return Err(format!("an offer names {MOST_AGENTS} agents at most"));
        }
        let names = self
            .projects
            .iter()
            .map(|project| project.name.as_str())
            .chain(self.agents.iter().map(String::as_str));
        for name in names {
            if name.is_empty()
                || name.chars().count() > NAME_LIMIT
                || name
                    .chars()
                    .any(|c| c.is_control() || crate::serial::invisible(c))
            {
                return Err(format!(
                    "a name of an offer is 1 to {NAME_LIMIT} characters, none of them a control \
                     or an invisible one"
                ));
            }
        }
        Ok(())
    }

    /// Why the runner `machine` that offers this would not take a session of `project` on
    /// `agent` (any agent signed in, when none is named); `None` when it would. The slots do not
    /// bound it: a session for the user counts in no children cap; they bound the children
    /// launched from another machine (#816).
    pub fn refusal(&self, machine: &str, project: &str, agent: Option<&str>) -> Option<String> {
        if !self.projects.iter().any(|offered| offered.name == project) {
            return Some(format!("the project {project} is not on {machine}"));
        }
        self.agent_refusal(machine, agent)
    }

    /// Why the runner `machine` that offers this would not run `agent` (any agent signed in,
    /// when none is named), whatever the project: an agent's launch takes its project with it
    /// (`docs/remote.md`, "Which project").
    pub fn agent_refusal(&self, machine: &str, agent: Option<&str>) -> Option<String> {
        match agent {
            Some(agent) if !self.agents.iter().any(|signed_in| signed_in == agent) => {
                Some(format!("{agent} is not signed in on {machine}"))
            }
            None if self.agents.is_empty() => Some(format!("no agent is signed in on {machine}")),
            _ => None,
        }
    }
}
