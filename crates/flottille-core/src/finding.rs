//! The findings (`docs/developer.md`, "The findings"): what the agents Flottille runs report
//! about Flottille itself, one finding per bug with its occurrences, kept in the store while the
//! developer mode is on and exported for the lead agent.
//!
//! The shape is the export's contract (`flottille developer reports --json`): a reader ignores a
//! field it does not know, and a field is never renamed.

use serde::Serialize;

/// The longest a finding's title is kept, and any other text of it, in characters.
pub const TITLE: usize = 200;
pub const TEXT: usize = 2000;

/// How many findings are kept at most, and how many occurrences one holds: an agent that loops
/// never fills the store.
pub const FINDINGS: usize = 500;
pub const OCCURRENCES: usize = 200;

/// One time an agent met the bug.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Occurrence {
    pub id: u64,
    /// When, in milliseconds since the Unix epoch.
    pub at: u64,
    /// The agent that reported it.
    pub session: String,
    /// What it called.
    pub called: String,
    /// What happened this time.
    pub happened: String,
    /// What differs from the other occurrences, when it said.
    pub differs: Option<String>,
}

/// An occurrence about to be kept.
#[derive(Clone, Debug, PartialEq)]
pub struct NewOccurrence {
    pub at: u64,
    pub session: String,
    pub called: String,
    pub happened: String,
    pub differs: Option<String>,
}

/// A bug, with every time it was met.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub id: u64,
    pub title: String,
    /// What the agent expected.
    pub expected: String,
    /// How many times it was met.
    pub count: usize,
    pub first_at: u64,
    pub last_at: u64,
    /// The address of the issue it became, once the lead agent filed it.
    pub filed: Option<String>,
    pub occurrences: Vec<Occurrence>,
}

impl Finding {
    /// Whether every word of `query` (case apart) is in its title or in any text of it.
    pub fn matches(&self, query: &str) -> bool {
        let mut text = format!("{} {}", self.title, self.expected);
        for occurrence in &self.occurrences {
            text.push(' ');
            text.push_str(&occurrence.called);
            text.push(' ');
            text.push_str(&occurrence.happened);
            if let Some(differs) = &occurrence.differs {
                text.push(' ');
                text.push_str(differs);
            }
        }
        let text = text.to_lowercase();
        query
            .split_whitespace()
            .all(|word| text.contains(&word.to_lowercase()))
    }
}
