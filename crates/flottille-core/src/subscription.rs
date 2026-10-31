//! A subscription (`docs/design.md` §5.6): an agent waits, once, for the next event of a target
//! that a source watches. Stored, so that it survives a restart of the daemon.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    /// The agent it wakes: its lineage, which a replacement keeps.
    pub agent: String,
    /// What produces its events: `forge` (a workspace's pull request).
    pub source: String,
    /// What it watches, as its source names it.
    pub target: Value,
    /// The events it waits for, among those its source produces.
    pub events: Vec<String>,
    /// Whose events count: `anyone`, or `people` (no bot, not the agent's own).
    pub from: String,
    /// What its source last saw of the target, so that a read is compared with it, after a
    /// restart too; its meaning is the source's.
    pub seen: Value,
    pub created_at: u64,
    pub expires_at: u64,
}
