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
    /// What the user can answer besides denying it.
    pub kind: Kind,
    /// What it changes, in words, one sentence each: what the user allows (an action's).
    pub says: Vec<String>,
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
    /// Never answered: the agent that asked, or the one that held it, ended.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Deny,
    /// Allowed this once.
    Once,
    /// Allowed, and the same call allowed for the root's tree from now on.
    Tree,
    /// Allowed, and its line added to the project's catalogue: a `Kind::Catalogue` request only.
    Always,
}

/// What a request is, which says what the user may answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A tool call the gate held: allowed once or for the tree (never for the tree when it
    /// reaches a sensitive place).
    #[default]
    Call,
    /// A line outside the catalogue, in the project folder: allowed once, or always, which adds
    /// it to the catalogue.
    Catalogue,
    /// A write of the project an agent asked for (`docs/tools.md`, "The project"): allowed once,
    /// each time.
    Action,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Call => "call",
            Kind::Catalogue => "catalogue",
            Kind::Action => "action",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        [Kind::Call, Kind::Catalogue, Kind::Action]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }

    /// Whether the user may answer it with `decision`.
    pub fn takes(self, decision: Decision) -> bool {
        match decision {
            Decision::Deny | Decision::Once => true,
            Decision::Tree => self == Kind::Call,
            Decision::Always => self == Kind::Catalogue,
        }
    }
}

impl RequestState {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestState::Pending => "pending",
            RequestState::Denied => "denied",
            RequestState::Allowed => "allowed",
            RequestState::Cancelled => "cancelled",
        }
    }

    pub fn parse(text: &str) -> Option<RequestState> {
        [
            RequestState::Pending,
            RequestState::Denied,
            RequestState::Allowed,
            RequestState::Cancelled,
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
            Decision::Always => "always",
        }
    }

    pub fn parse(text: &str) -> Option<Decision> {
        [
            Decision::Deny,
            Decision::Once,
            Decision::Tree,
            Decision::Always,
        ]
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
            "kind": self.kind,
            "says": self.says,
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
    /// The images it carries: names of files in the agent's folder of the data folder.
    #[serde(default)]
    pub images: Vec<String>,
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
        for decision in [
            Decision::Deny,
            Decision::Once,
            Decision::Tree,
            Decision::Always,
        ] {
            assert_eq!(Decision::parse(decision.as_str()), Some(decision));
        }
        for kind in [Kind::Call, Kind::Catalogue, Kind::Action] {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(RequestState::parse("gone"), None);
        assert_eq!(Decision::parse("forever"), None);
    }

    #[test]
    fn only_a_call_is_allowed_for_the_tree_and_only_a_line_outside_the_catalogue_always() {
        assert!(Kind::Call.takes(Decision::Tree));
        assert!(!Kind::Call.takes(Decision::Always));
        assert!(Kind::Catalogue.takes(Decision::Always));
        assert!(!Kind::Catalogue.takes(Decision::Tree));
        assert!(!Kind::Action.takes(Decision::Tree));
        assert!(!Kind::Action.takes(Decision::Always));
        for kind in [Kind::Call, Kind::Catalogue, Kind::Action] {
            assert!(kind.takes(Decision::Once) && kind.takes(Decision::Deny));
        }
    }
}
