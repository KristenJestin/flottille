//! The socket protocol (`docs/protocol.md`): `flottille-protocol`'s, and the parameters that need
//! the domain's types.

pub use flottille_protocol::*;
use serde::{Deserialize, Serialize};

/// The parameters of `project.set_settings`: the forge declared for each repository named (none
/// to declare none; a repository left out keeps its own), its base branches and dependencies
/// when given, the project's tracker (none for none) and its branch prefix when given.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSettings {
    pub name: String,
    #[serde(default)]
    pub repositories: Vec<RepositorySetting>,
    #[serde(default)]
    pub tracker: Option<crate::declared::ProjectTracker>,
    /// The branch prefix of its workspaces, empty for none; the file's when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_prefix: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositorySetting {
    pub path: String,
    #[serde(default)]
    pub forge: Option<crate::declared::RepositoryForge>,
    /// The base branches in order of preference; those the file has when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Vec<String>>,
    /// The paths of the repositories it depends on, none for none; those the file has when left
    /// out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    /// "Delete the branch on merge"; what the file has when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_branch_on_merge: Option<bool>,
}

/// The parameters of `agent.answer`: the user's answers to an ask of the root, one per question
/// in order (`question` is the number of its `agent.asked` event).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAnswer {
    pub agent: String,
    pub question: u64,
    pub answers: Vec<crate::question::Answer>,
}
