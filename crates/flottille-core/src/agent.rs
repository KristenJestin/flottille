//! An agent: one agent session run over ACP, in a place, with what it was started with frozen
//! (`docs/agents.md`). What the store keeps and the protocol shows.

use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// An agent as the store keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    /// Opaque, given by the daemon.
    pub id: String,
    /// The agent CLI behind the adapter: `claude`.
    pub provider: String,
    /// `lead` for an agent the user starts.
    pub role: String,
    pub project: String,
    /// The workspace it works in; the project folder itself (`main`) when none.
    pub workspace: Option<String>,
    /// Where it works, absolute: the session's `cwd`.
    pub folder: PathBuf,
    pub state: AgentState,
    /// Why it is in its state, when that needs saying (a failure), masked.
    pub state_reason: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub created_at: u64,
    pub updated_at: u64,
    /// Its last sign of activity: an update from the agent outside a replay.
    pub active_at: Option<u64>,
    pub ended_at: Option<u64>,
    /// What the user chose; nothing means the agent's default.
    pub chosen: Choices,
    /// What the agent reported it took, at the last start of its process.
    pub taken: Choices,
    /// The ACP session id, once a session is open: what a restarted process resumes or loads.
    pub native_id: Option<String>,
    /// The adapter's name and version, from its `initialize` answer.
    pub agent_version: Option<String>,
    /// The system prompt, frozen at creation, with its cache boundary.
    pub instructions: String,
    /// The exact names of its tools, frozen at creation (the prompt cache depends on it).
    pub tools: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Choices {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Its process is starting and its session opening.
    Starting,
    /// A turn runs.
    Working,
    /// Between turns, its process running or released.
    Idle,
    /// Final: stopped on purpose.
    Ended,
    /// Final: it cannot go on; `state_reason` says why.
    Failed,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Starting => "starting",
            AgentState::Working => "working",
            AgentState::Idle => "idle",
            AgentState::Ended => "ended",
            AgentState::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<AgentState> {
        [
            AgentState::Starting,
            AgentState::Working,
            AgentState::Idle,
            AgentState::Ended,
            AgentState::Failed,
        ]
        .into_iter()
        .find(|state| state.as_str() == text)
    }

    /// A final state is never left.
    pub fn is_final(self) -> bool {
        matches!(self, AgentState::Ended | AgentState::Failed)
    }
}

impl Agent {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("an agent is always JSON")
    }
}

/// A new agent id: 12 hexadecimal digits from the system's random source.
pub fn new_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 6];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_twelve_hexadecimal_digits_and_differ() {
        let (a, b) = (new_id().unwrap(), new_id().unwrap());
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn a_state_reads_back_from_its_name() {
        for state in [
            AgentState::Starting,
            AgentState::Working,
            AgentState::Idle,
            AgentState::Ended,
            AgentState::Failed,
        ] {
            assert_eq!(AgentState::parse(state.as_str()), Some(state));
        }
        assert!(AgentState::Failed.is_final() && !AgentState::Idle.is_final());
    }
}
