//! A command run an agent started with `commands_run` (`docs/tools.md`): what it runs, where, and
//! how it ended. What the store keeps.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Opaque, given by the daemon.
    pub id: String,
    /// The agent that started it: only that agent reads and stops it.
    pub agent: String,
    /// The catalogue command it runs, `None` for a free line.
    pub name: Option<String>,
    /// The line, as it was split and run.
    pub line: String,
    /// Where it runs, absolute.
    pub folder: PathBuf,
    pub state: RunState,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    pub ended_at: Option<u64>,
    /// The end of its output once it ended, masked: at most what the daemon keeps of it.
    pub output: String,
    /// The user dismissed it from the list of commands once it ended.
    pub dismissed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    /// Its process ended by itself, with an exit code or by a signal.
    Exited,
    /// Ended by `commands_status` with `action: stop`, or by the call that started it being stopped.
    Stopped,
    /// The daemon that ran it stopped, or died.
    Interrupted,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Exited => "exited",
            RunState::Stopped => "stopped",
            RunState::Interrupted => "interrupted",
        }
    }

    pub fn parse(text: &str) -> Option<RunState> {
        [
            RunState::Running,
            RunState::Exited,
            RunState::Stopped,
            RunState::Interrupted,
        ]
        .into_iter()
        .find(|state| state.as_str() == text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_reads_back_from_its_name() {
        for state in [
            RunState::Running,
            RunState::Exited,
            RunState::Stopped,
            RunState::Interrupted,
        ] {
            assert_eq!(RunState::parse(state.as_str()), Some(state));
        }
        assert_eq!(RunState::parse("gone"), None);
    }
}
