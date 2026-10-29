//! An approval request: a tool call the gate held (`docs/design.md` §7). What the store keeps of
//! it, of the judge's verdict on it, and of the deliveries that carry an answer back.

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
    /// Its class is base: it always comes to the user, with a strong warning. Otherwise it is
    /// to judge.
    pub base: bool,
    /// What the card shows: a call, or an action said in words.
    pub kind: Kind,
    /// What it changes, in words, one sentence each: what the user allows (an action's).
    pub says: Vec<String>,
    /// The judge's verdict, when the judge saw it (Auto).
    pub judgement: Option<Judgement>,
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

/// The user's answer: there is no grant, for a tree or for always; each request is allowed or
/// denied once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Deny,
    /// Allowed, this once.
    Once,
}

/// What a request shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A tool call the gate held.
    #[default]
    Call,
    /// A write of the project an agent asked for (`docs/tools.md`, "The project"), said in words.
    Action,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Call => "call",
            Kind::Action => "action",
        }
    }

    /// A request stored before the classes (a `catalogue` one) reads as a call.
    pub fn parse(text: &str) -> Option<Kind> {
        match text {
            "call" | "catalogue" => Some(Kind::Call),
            "action" => Some(Kind::Action),
            _ => None,
        }
    }
}

/// What the judge said of a call to judge, in Auto (`docs/design.md` §7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Judgement {
    /// Who judged: `<provider> <model>`.
    pub by: String,
    /// `allow`, `ask`, or `failed` when the judge could not decide (which asks).
    pub verdict: String,
    /// Its one sentence, or why it could not decide.
    pub reason: String,
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
        }
    }

    /// A request allowed for a tree or always, before the classes, reads as allowed once.
    pub fn parse(text: &str) -> Option<Decision> {
        match text {
            "deny" => Some(Decision::Deny),
            "once" | "tree" | "always" => Some(Decision::Once),
            _ => None,
        }
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
            "class": if self.base { "base" } else { "judge" },
            "kind": self.kind,
            "says": self.says,
            "judgement": self.judgement,
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
        for decision in [Decision::Deny, Decision::Once] {
            assert_eq!(Decision::parse(decision.as_str()), Some(decision));
        }
        for kind in [Kind::Call, Kind::Action] {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(RequestState::parse("gone"), None);
        assert_eq!(Decision::parse("forever"), None);
    }

    #[test]
    fn what_was_stored_before_the_classes_reads_as_a_call_allowed_once() {
        assert_eq!(Kind::parse("catalogue"), Some(Kind::Call));
        assert_eq!(Decision::parse("tree"), Some(Decision::Once));
        assert_eq!(Decision::parse("always"), Some(Decision::Once));
    }
}
