//! A subscription (`docs/design.md` §5.6): an agent is woken at the events of a target that a
//! source watches, until the target ends. Stored, so that it survives a restart of the daemon.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The time of a subscription with no deadline: far enough that the sweep never finds it due.
pub const NO_DEADLINE: u64 = i64::MAX as u64;

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
    /// What its source last saw of the target. No source writes it since the subscriptions
    /// filter the items' events (#257), which compare with the store: it is `{}`, its column
    /// kept, a migration being worth nothing here.
    pub seen: Value,
    pub created_at: u64,
    /// Its deadline; `NO_DEADLINE` when the agent gave none.
    pub expires_at: u64,
}
