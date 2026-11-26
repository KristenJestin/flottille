//! What a runner tells its peers of the sessions it holds (`docs/remote.md`, "One list of every
//! machine"): the `sessions` machine message, a light list (each root session's title, project,
//! state and last exchange, never its conversation), sent over the link when a link is made and
//! again when it changes. The other machine keeps it, so that its clients list the sessions of a
//! machine that is off.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::offers::MOST_PROJECTS;

/// The most sessions a list may hold: past it, the sender keeps those of the latest exchange.
pub const MOST_SESSIONS: usize = 500;
/// The longest title it may hold, in characters.
pub const TITLE_LIMIT: usize = 200;
/// The longest project name or id it may hold, in characters.
const NAME_LIMIT: usize = 64;
/// The states a session of the list may be in, as the window marks them.
pub const STATES: [&str; 4] = ["running", "needs you", "waiting", "idle"];

/// The live root sessions of a runner.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionList {
    pub sessions: Vec<LightSession>,
}

/// The projects a runner holds, each by its repositories' remotes: what lets another machine say
/// two projects are one (`docs/remote.md`, "One list of every machine"). A message of its own, not
/// a field of [`SessionList`]: a machine of before refuses the message alone (it does not know
/// it), where a field it does not know would make it refuse the whole list of sessions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectList {
    pub projects: Vec<LightProject>,
}

/// The pilots of another machine of a runner's led sessions: what lets every window draw such a
/// session under its pilot (`docs/remote.md`, "One list of every machine"). A message of its own,
/// as [`ProjectList`] is, for the same reason: `pilot` of a [`LightSession`] is a session of the
/// same list, and an id of another machine there, or a field a machine of before does not know,
/// would make it refuse the whole list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PilotList {
    pub pilots: Vec<LightPilot>,
}

/// A session of the sender's list led from another machine: that machine by its name among the
/// members, and the pilot by its id there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightPilot {
    /// The session's id, one of the sender's list.
    pub session: String,
    /// The machine of its pilot, by its name.
    pub machine: String,
    /// Its pilot's id on that machine.
    pub pilot: String,
}

/// A project as another machine's list shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightProject {
    pub name: String,
    /// The remotes of its repositories, normalised (`flottille_core::remote_key`), sorted, each
    /// once; empty when one of its repositories has no remote to say it by.
    pub remotes: Vec<String>,
}

/// The most remotes a project of a list may hold, and the longest one, in characters.
const MOST_REMOTES: usize = 64;
const REMOTE_LIMIT: usize = 256;

/// A session as another machine's list shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightSession {
    /// Its root agent's id on its machine.
    pub id: String,
    pub title: String,
    pub project: String,
    /// One of [`STATES`].
    pub state: String,
    /// Its last exchange, in milliseconds since the Unix epoch: the list is ordered by it.
    pub exchanged_at: u64,
    pub created_at: u64,
    /// The session of the agent that leads it, on the same machine, whose row it sits under.
    pub pilot: Option<String>,
}

/// One id of the sender's: 1 to 64 letters, digits, dashes or underscores.
pub(crate) fn an_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= NAME_LIMIT
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A text a client may show as it is: no control and no invisible character.
fn showable(text: &str, most: usize) -> bool {
    text.chars().count() <= most
        && !text
            .chars()
            .any(|c| c.is_control() || crate::serial::invisible(c))
}

impl SessionList {
    /// Whether another machine's list is one to keep: within the bounds, every id one of the
    /// sender's, every text one a client may show as it is.
    pub fn check(&self) -> Result<(), String> {
        if self.sessions.len() > MOST_SESSIONS {
            return Err(format!("a list names {MOST_SESSIONS} sessions at most"));
        }
        for session in &self.sessions {
            if !an_id(&session.id) || !session.pilot.as_deref().is_none_or(an_id) {
                return Err(
                    "an id of a list is 1 to 64 letters, digits, dashes or underscores".to_owned(),
                );
            }
            if session.project.is_empty() || !showable(&session.project, NAME_LIMIT) {
                return Err(format!(
                    "a project of a list is 1 to {NAME_LIMIT} characters, none of them a control \
                     or an invisible one"
                ));
            }
            if !showable(&session.title, TITLE_LIMIT) {
                return Err(format!(
                    "a title of a list is {TITLE_LIMIT} characters at most, none of them a \
                     control or an invisible one"
                ));
            }
            if !STATES.contains(&session.state.as_str()) {
                return Err(format!("a state of a list is one of {}", STATES.join(", ")));
            }
        }
        let mut pilots = HashMap::new();
        for session in &self.sessions {
            if pilots
                .insert(session.id.as_str(), session.pilot.as_deref())
                .is_some()
            {
                return Err(format!("the id {} is in a list twice", session.id));
            }
        }
        // A session under its own pilot, directly or through others, shows under no row.
        for session in &self.sessions {
            let mut at = session.pilot.as_deref();
            for _ in 0..pilots.len() {
                at = match at.and_then(|id| pilots.get(id)) {
                    Some(pilot) => *pilot,
                    None => break,
                };
            }
            if at.is_some_and(|id| pilots.contains_key(id)) {
                return Err(format!(
                    "the pilots of a list make a cycle at {}",
                    session.id
                ));
            }
        }
        Ok(())
    }
}

impl ProjectList {
    /// Whether another machine's projects are ones to keep: within the bounds, every text one a
    /// client may show as it is.
    pub fn check(&self) -> Result<(), String> {
        if self.projects.len() > MOST_PROJECTS {
            return Err(format!("a list names {MOST_PROJECTS} projects at most"));
        }
        for project in &self.projects {
            if project.name.is_empty() || !showable(&project.name, NAME_LIMIT) {
                return Err(format!(
                    "a project of a list is 1 to {NAME_LIMIT} characters, none of them a control \
                     or an invisible one"
                ));
            }
            if project.remotes.len() > MOST_REMOTES
                || project
                    .remotes
                    .iter()
                    .any(|remote| remote.is_empty() || !showable(remote, REMOTE_LIMIT))
            {
                return Err(format!(
                    "a project names {MOST_REMOTES} remotes at most, of {REMOTE_LIMIT} characters \
                     at most"
                ));
            }
        }
        Ok(())
    }
}

impl PilotList {
    /// Whether another machine's pilots are ones to keep: within the bounds of a list, every id
    /// one of the sender's form, every name one a client may show as it is, each session once.
    pub fn check(&self) -> Result<(), String> {
        if self.pilots.len() > MOST_SESSIONS {
            return Err(format!("a list names {MOST_SESSIONS} pilots at most"));
        }
        let mut seen = std::collections::HashSet::new();
        for pilot in &self.pilots {
            if !an_id(&pilot.session) || !an_id(&pilot.pilot) {
                return Err(
                    "an id of a list is 1 to 64 letters, digits, dashes or underscores".to_owned(),
                );
            }
            if pilot.machine.is_empty() || !showable(&pilot.machine, NAME_LIMIT) {
                return Err(format!(
                    "a machine of a list is 1 to {NAME_LIMIT} characters, none of them a control \
                     or an invisible one"
                ));
            }
            if !seen.insert(pilot.session.as_str()) {
                return Err(format!("the session {} has two pilots", pilot.session));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str) -> LightSession {
        LightSession {
            id: id.to_owned(),
            title: "Ship the export".to_owned(),
            project: "app".to_owned(),
            state: "idle".to_owned(),
            exchanged_at: 2,
            created_at: 1,
            pilot: None,
        }
    }

    #[test]
    fn a_list_within_its_bounds_is_kept() {
        let list = SessionList {
            sessions: vec![session("a1"), session("b-2")],
        };
        assert_eq!(list.check(), Ok(()));
    }

    #[test]
    fn a_list_past_its_bounds_is_refused() {
        let many = SessionList {
            sessions: (0..=MOST_SESSIONS)
                .map(|at| session(&format!("s{at}")))
                .collect(),
        };
        assert!(many.check().is_err());
        let mut long = session("a1");
        long.title = "x".repeat(TITLE_LIMIT + 1);
        assert!(
            SessionList {
                sessions: vec![long],
            }
            .check()
            .is_err()
        );
        let mut hidden = session("a1");
        hidden.title = "a\u{202e}b".to_owned();
        assert!(
            SessionList {
                sessions: vec![hidden],
            }
            .check()
            .is_err()
        );
        let mut odd = session("a1");
        odd.state = "sleeping".to_owned();
        assert!(
            SessionList {
                sessions: vec![odd],
            }
            .check()
            .is_err()
        );
        assert!(
            SessionList {
                sessions: vec![session("../x")],
            }
            .check()
            .is_err()
        );
    }

    #[test]
    fn a_list_with_a_cycle_of_pilots_is_refused() {
        let mut a = session("a1");
        let mut b = session("b2");
        a.pilot = Some("b2".to_owned());
        b.pilot = Some("a1".to_owned());
        let cycle = SessionList {
            sessions: vec![a, b],
        };
        let why = cycle.check().unwrap_err();
        assert!(why.contains("cycle"), "{why}");
        let mut itself = session("a1");
        itself.pilot = Some("a1".to_owned());
        assert!(
            SessionList {
                sessions: vec![itself],
            }
            .check()
            .is_err()
        );
        // A chain is no cycle, and a pilot that is not listed is the sender's business.
        let mut under = session("b2");
        under.pilot = Some("a1".to_owned());
        let mut away = session("c3");
        away.pilot = Some("gone".to_owned());
        let chain = SessionList {
            sessions: vec![session("a1"), under, away],
        };
        assert_eq!(chain.check(), Ok(()));
    }

    #[test]
    fn a_list_with_the_same_id_twice_is_refused() {
        let twice = SessionList {
            sessions: vec![session("a1"), session("b2"), session("a1")],
        };
        let why = twice.check().unwrap_err();
        assert!(why.contains("twice"), "{why}");
    }

    #[test]
    fn the_projects_of_a_machine_are_bounded() {
        let project = |name: &str, remote: &str| LightProject {
            name: name.to_owned(),
            remotes: vec![remote.to_owned()],
        };
        let list = ProjectList {
            projects: vec![project("app", "github.com/acme/app")],
        };
        assert_eq!(list.check(), Ok(()));
        for odd in [
            project("", "x/y"),
            project("app", ""),
            project("app", "a\u{202e}b"),
        ] {
            assert!(
                ProjectList {
                    projects: vec![odd]
                }
                .check()
                .is_err()
            );
        }
        let many = ProjectList {
            projects: (0..=MOST_PROJECTS)
                .map(|at| project(&format!("p{at}"), "x/y"))
                .collect(),
        };
        assert!(many.check().is_err());
    }

    #[test]
    fn the_list_of_sessions_keeps_the_shape_a_machine_of_before_reads() {
        // A list of sessions carries no projects: a machine of before refuses a field it does not
        // know (`deny_unknown_fields`), and would lose the whole list.
        let list = SessionList {
            sessions: vec![session("a1")],
        };
        let wire = serde_json::to_value(&list).unwrap();
        assert_eq!(
            wire.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["sessions"]
        );
    }

    #[test]
    fn the_pilots_of_another_machine_are_bounded() {
        let pilot = |session: &str, machine: &str, id: &str| LightPilot {
            session: session.to_owned(),
            machine: machine.to_owned(),
            pilot: id.to_owned(),
        };
        let list = PilotList {
            pilots: vec![pilot("a1", "pc", "r1"), pilot("b2", "pc", "r1")],
        };
        assert_eq!(list.check(), Ok(()));
        for odd in [
            vec![pilot("a1", "pc", "r1@00ff")],
            vec![pilot("../a", "pc", "r1")],
            vec![pilot("a1", "", "r1")],
            vec![pilot("a1", "p\u{202e}c", "r1")],
            vec![pilot("a1", "pc", "r1"), pilot("a1", "lab", "r2")],
        ] {
            assert!(PilotList { pilots: odd }.check().is_err());
        }
        let many = PilotList {
            pilots: (0..=MOST_SESSIONS)
                .map(|at| pilot(&format!("s{at}"), "pc", "r1"))
                .collect(),
        };
        assert!(many.check().is_err());
    }

    #[test]
    fn a_machine_of_before_still_reads_the_sessions_and_refuses_the_pilots_alone() {
        use crate::link::{MachineMessage, Message};
        let sessions = Message::Machine(MachineMessage::Sessions(SessionList {
            sessions: vec![session("a1")],
        }));
        let pilots = Message::Machine(MachineMessage::Pilots(PilotList {
            pilots: vec![LightPilot {
                session: "a1".to_owned(),
                machine: "pc".to_owned(),
                pilot: "r1".to_owned(),
            }],
        }));
        let machine = |message: &Message| serde_json::to_value(message).unwrap()["machine"].clone();
        let Before::Sessions(older) = serde_json::from_value(machine(&sessions)).unwrap();
        assert_eq!(older.sessions.len(), 1);
        assert!(serde_json::from_value::<Before>(machine(&pilots)).is_err());
        for message in [sessions, pilots] {
            let back: Message =
                serde_json::from_value(serde_json::to_value(&message).unwrap()).unwrap();
            assert_eq!(back, message);
        }
    }

    /// What a machine of before knows of this link's messages: its list of sessions as this one
    /// writes it, and no `projects` message.
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Before {
        Sessions(Older),
    }
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Older {
        sessions: Vec<LightSession>,
    }

    #[test]
    fn a_machine_of_before_still_reads_the_sessions_and_refuses_the_projects_alone() {
        use crate::link::{MachineMessage, Message};
        let sessions = Message::Machine(MachineMessage::Sessions(SessionList {
            sessions: vec![session("a1")],
        }));
        let projects = Message::Machine(MachineMessage::Projects(ProjectList {
            projects: vec![LightProject {
                name: "app".to_owned(),
                remotes: vec!["github.com/acme/app".to_owned()],
            }],
        }));
        let machine = |message: &Message| serde_json::to_value(message).unwrap()["machine"].clone();
        let read: Before = serde_json::from_value(machine(&sessions)).unwrap();
        let Before::Sessions(older) = read;
        assert_eq!(older.sessions.len(), 1);
        // The message it does not know is refused alone: the link and the sessions go on.
        assert!(serde_json::from_value::<Before>(machine(&projects)).is_err());
        // And this version reads both.
        for message in [sessions, projects] {
            let back: Message =
                serde_json::from_value(serde_json::to_value(&message).unwrap()).unwrap();
            assert_eq!(back, message);
        }
    }
}
