//! An approval request: a tool call the gate held (`docs/design.md` §7). What the store keeps of
//! it, of the grants the user gave a tree, and of the deliveries that carry an answer back.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Who holds a request that is the user's.
pub const USER: &str = "user";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Given by the store, counted across every agent: what the user names it by.
    pub number: u64,
    /// The agent that asked.
    pub agent: String,
    /// The call's key (`claudecode/toolUseId`): the same key asked twice is the same request.
    pub call_key: Option<String>,
    pub tool: String,
    /// The call's arguments whole, frozen: what is performed once it is allowed.
    pub arguments: Value,
    /// The arguments as they are shown, every text cut and masked.
    pub shown: Value,
    /// Why the gate held it, masked.
    pub reasons: Vec<String>,
    /// It reaches a sensitive place: it is never allowed for a tree.
    pub sensitive: bool,
    /// What a grant for the tree is given to: the exact call, without what changes each time.
    pub identity: String,
    /// `USER`, or the id of the agent that holds it.
    pub held_by: String,
    /// The sentences of why given by each agent that forwarded it, in order.
    pub forwards: Vec<Forward>,
    pub state: RequestState,
    pub decision: Option<Decision>,
    pub created_at: u64,
    pub decided_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Forward {
    pub by: String,
    pub why: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Pending,
    Denied,
    /// Allowed: performed, or being performed.
    Allowed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Deny,
    /// Allowed this once.
    Once,
    /// Allowed, and the same call allowed for the root's tree from now on.
    Tree,
}

impl RequestState {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestState::Pending => "pending",
            RequestState::Denied => "denied",
            RequestState::Allowed => "allowed",
        }
    }

    pub fn parse(text: &str) -> Option<RequestState> {
        [
            RequestState::Pending,
            RequestState::Denied,
            RequestState::Allowed,
        ]
        .into_iter()
        .find(|state| state.as_str() == text)
    }
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Deny => "deny",
            Decision::Once => "once",
            Decision::Tree => "tree",
        }
    }

    pub fn parse(text: &str) -> Option<Decision> {
        [Decision::Deny, Decision::Once, Decision::Tree]
            .into_iter()
            .find(|decision| decision.as_str() == text)
    }
}

impl Request {
    /// As a result and an event show it: the frozen arguments are left out, they may hold what
    /// is masked in `shown`.
    pub fn to_json(&self) -> Value {
        json!({
            "number": self.number,
            "agent": self.agent,
            "call_key": self.call_key,
            "tool": self.tool,
            "call": self.shown,
            "reasons": self.reasons,
            "sensitive": self.sensitive,
            "held_by": self.held_by,
            "forwards": self.forwards,
            "state": self.state,
            "decision": self.decision,
            "created_at": self.created_at,
            "decided_at": self.decided_at,
        })
    }
}

/// A message Flottille hands to an agent, stored before it is sent (`docs/design.md` §5.2,
/// `docs/agents.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    /// The idempotency key: `approval-<number>` for a request's answer, so it is stored once.
    pub id: String,
    /// The agent it is for: its lineage, which a replacement keeps.
    pub agent: String,
    /// `^[a-z][a-z-]*$`: `user`, `brief`, `child-report`…
    pub kind: String,
    /// Masked; escaped only when it is sent.
    pub body: String,
    /// `between-turns`, `urgent` or `redirect`.
    pub urgency: String,
    /// `queued`; `sending` while the prompt that holds it is played; `sent`; or `superseded`.
    pub state: String,
    pub created_at: u64,
    pub sent_at: Option<u64>,
}

pub const BETWEEN_TURNS: &str = "between-turns";
pub const URGENT: &str = "urgent";
pub const REDIRECT: &str = "redirect";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_and_a_decision_read_back_from_their_names() {
        for state in [
            RequestState::Pending,
            RequestState::Denied,
            RequestState::Allowed,
        ] {
            assert_eq!(RequestState::parse(state.as_str()), Some(state));
        }
        for decision in [Decision::Deny, Decision::Once, Decision::Tree] {
            assert_eq!(Decision::parse(decision.as_str()), Some(decision));
        }
        assert_eq!(RequestState::parse("gone"), None);
        assert_eq!(Decision::parse("always"), None);
    }
}
