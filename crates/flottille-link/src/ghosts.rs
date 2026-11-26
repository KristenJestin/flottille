//! The local ghost of each session of another machine (`docs/remote.md`, "Machines are one",
//! point 4): a machine asks each runner it links to for the sessions it lists, once a link, with
//! where each of its ghosts stopped (`ghost`); the runner answers `{}`, then sends on the same
//! stream what each ghost missed and, as the session runs, each of its events, its tree and its
//! waits as that machine shows them ([`GhostNews`]). The ghost only reflects: nothing of it is
//! sent back.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sessions::{MOST_SESSIONS, an_id};

/// The most events of one session a ghost keeps, the last ones: earlier pages are read from the
/// session's machine, as the window reads them when it scrolls up.
pub const GHOST_EVENTS: usize = 2000;

/// What a machine asks a runner it links to: the ghosts of the sessions that runner lists, each
/// from after the last event this machine holds of it (none for a session it holds no ghost of).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GhostAsk {
    pub after: BTreeMap<String, u64>,
}

impl GhostAsk {
    /// Whether it names sessions of a list, within its bounds.
    pub fn check(&self) -> Result<(), String> {
        if self.after.len() > MOST_SESSIONS {
            return Err(format!(
                "a ghost is asked of {MOST_SESSIONS} sessions at most"
            ));
        }
        if !self.after.keys().all(|id| an_id(id)) {
            return Err(
                "an id of a session is 1 to 64 letters, digits, dashes or underscores".to_owned(),
            );
        }
        Ok(())
    }
}

/// One frame of a runner's answer to [`GhostAsk`], of one of its sessions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GhostNews {
    pub session: String,
    /// Its events, oldest first, each as that machine's `agent.show` gives it (`seq`, `at`,
    /// `name`, `data`), after those sent before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Value>,
    /// What was kept of it is dropped first: what it missed is more than a ghost keeps, and these
    /// are its last events.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
    /// With a reset, or the first events of a session: whether it has earlier events than these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earlier: Option<bool>,
    /// Its tree and its waits as that machine shows them: `{"tree": agent.tree's agents,
    /// "requests": approval.list's requests, "asks": the vault asks of its tree}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<Value>,
    /// It left that machine's list: its ghost goes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
}

impl GhostNews {
    /// Whether a ghost may keep it: a session's id, events that each say their number, in order.
    pub fn check(&self) -> Result<(), String> {
        if !an_id(&self.session) {
            return Err(
                "an id of a session is 1 to 64 letters, digits, dashes or underscores".to_owned(),
            );
        }
        let mut last = 0;
        for event in &self.events {
            let seq = event["seq"]
                .as_u64()
                .filter(|_| event["name"].is_string() && event["at"].is_u64())
                .ok_or_else(|| "an event of a ghost says its seq, at and name".to_owned())?;
            if seq <= last {
                return Err("the events of a ghost come in order, each once".to_owned());
            }
            last = seq;
        }
        if self.events.len() > GHOST_EVENTS {
            return Err(format!("a ghost keeps {GHOST_EVENTS} events at most"));
        }
        if self.view.as_ref().is_some_and(|view| !view.is_object()) {
            return Err("the view of a ghost is an object".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_ghost_asked_names_ids_of_a_list() {
        let ask = GhostAsk {
            after: [("a1".to_owned(), 3)].into(),
        };
        assert!(ask.check().is_ok());
        let ask = GhostAsk {
            after: [("../a1".to_owned(), 3)].into(),
        };
        assert!(ask.check().is_err());
    }

    #[test]
    fn news_of_a_ghost_come_in_order() {
        let event = |seq: u64| json!({"seq": seq, "at": 1, "name": "agent.message", "data": {}});
        let news = GhostNews {
            session: "a1".to_owned(),
            events: vec![event(1), event(2)],
            ..GhostNews::default()
        };
        assert!(news.check().is_ok());
        let news = GhostNews {
            events: vec![event(2), event(2)],
            ..news
        };
        assert!(news.check().is_err());
        let news = GhostNews {
            session: "a1".to_owned(),
            events: vec![json!({"seq": 1})],
            ..GhostNews::default()
        };
        assert!(news.check().is_err());
    }
}
