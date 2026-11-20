//! The view state, computed in Rust from the member list and each runner's events
//! (`docs/remote.md`, "The Android app"): the machines, one list of sessions for every machine by
//! project (each session tagged with its machine, #1150), the conversation the app follows, the
//! requests and the questions that wait for the user, and this device's role. The screens draw it
//! as it is: they compute nothing of their own.

use std::collections::{BTreeMap, HashMap};

use flottille_link::{Members, PublicKey, Role};
use serde_json::Value;

/// What the app draws, whole, with every batch.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct ViewState {
    /// This device's role in the member list (`controller`, `viewer`…), empty before it is in one.
    pub role: String,
    /// This device's name in the list.
    pub name: String,
    /// The hub's state: empty while the session is open, else what happened (`the hub is
    /// unreachable since 22:14`).
    pub hub: String,
    pub machines: Vec<MachineView>,
    /// The sessions of every machine, by project: a project of the same name on two machines is
    /// one group. Projects by name, so that groups never move; sessions by last exchange.
    pub projects: Vec<ProjectView>,
    pub conversation: Option<ConversationView>,
    pub requests: Vec<RequestView>,
    pub asks: Vec<AskView>,
    /// What the core refused and the user should know, newest last (a member list that does not
    /// check).
    pub said: Vec<String>,
}

/// A runner of the user's.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MachineView {
    pub name: String,
    pub role: String,
    pub online: bool,
    /// Why it is not, when it is not: the link's own sentence.
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ProjectView {
    pub name: String,
    pub sessions: Vec<SessionView>,
}

/// A root session of a runner, tagged with that machine.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SessionView {
    pub machine: String,
    pub agent: String,
    pub title: String,
    /// `working`, `idle`, `ended`… as its last event says.
    pub state: String,
    /// When it last exchanged, in milliseconds since the Unix epoch.
    pub last: u64,
}

/// The conversation of the session the app follows (`Core::follow`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ConversationView {
    pub machine: String,
    pub agent: String,
    pub entries: Vec<Entry>,
}

/// One entry of a conversation: `user`, `agent`, `thought` or `tool`, and its text.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Entry {
    pub kind: String,
    pub text: String,
}

/// A request of an agent's that waits for the user's Allow or Deny.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RequestView {
    pub machine: String,
    pub number: u64,
    pub agent: String,
    pub tool: String,
    pub reasons: Vec<String>,
}

/// A question of an agent's that waits for the user's answer.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AskView {
    pub machine: String,
    pub agent: String,
    /// The number of its `agent.asked` event, which `agent.answer` names.
    pub asked: u64,
    pub question: String,
}

/// What the view is computed from.
#[derive(Default)]
pub(crate) struct View {
    role: String,
    name: String,
    hub: String,
    /// The runners, in the list's order, with whether each is linked.
    machines: Vec<MachineView>,
    /// The sessions by machine and agent.
    sessions: BTreeMap<(String, String), Session>,
    requests: BTreeMap<(String, u64), RequestView>,
    asks: BTreeMap<(String, u64), AskView>,
    /// The followed session and its events by number: an event seen twice (loaded, then live) is
    /// one.
    followed: Option<(String, String, BTreeMap<u64, Value>)>,
    said: Vec<String>,
}

struct Session {
    project: String,
    title: String,
    state: String,
    last: u64,
}

/// The sentences kept in `said`, the newest.
const SAID_KEPT: usize = 8;

impl View {
    /// The list as this device checked it: its own entry, and the runners.
    pub(crate) fn members(&mut self, members: &Members, own: &PublicKey) {
        if let Some(me) = members.get(own) {
            self.role = me.role.word().to_owned();
            self.name = me.name.clone();
        }
        let before: HashMap<String, MachineView> = self
            .machines
            .drain(..)
            .map(|machine| (machine.name.clone(), machine))
            .collect();
        self.machines = members
            .members()
            .iter()
            .filter(|member| member.role == Role::Runner && member.key != *own)
            .map(|member| match before.get(&member.name) {
                Some(kept) => kept.clone(),
                None => MachineView {
                    name: member.name.clone(),
                    role: member.role.word().to_owned(),
                    online: false,
                    why: "not linked yet".to_owned(),
                },
            })
            .collect();
    }

    pub(crate) fn online(&mut self, machine: &str, online: bool, why: &str) {
        if let Some(view) = self.machines.iter_mut().find(|view| view.name == machine) {
            view.online = online;
            why.clone_into(&mut view.why);
        }
    }

    pub(crate) fn hub(&mut self, hub: &str) {
        hub.clone_into(&mut self.hub);
    }

    pub(crate) fn say(&mut self, sentence: String) {
        self.said.push(sentence);
        if self.said.len() > SAID_KEPT {
            self.said.remove(0);
        }
    }

    /// What a runner listed when it linked: its agents (`agent.list`) and the requests that wait
    /// for the user (`approval.list`).
    pub(crate) fn load(&mut self, machine: &str, agents: &Value, requests: &Value) {
        for agent in agents["agents"].as_array().into_iter().flatten() {
            if !agent["parent"].is_null() {
                continue;
            }
            let Some(id) = agent["id"].as_str() else {
                continue;
            };
            let last = ["exchanged_at", "active_at", "created_at"]
                .iter()
                .find_map(|field| agent[*field].as_u64())
                .unwrap_or_default();
            self.sessions.insert(
                (machine.to_owned(), id.to_owned()),
                Session {
                    project: text(&agent["project"]),
                    title: text(&agent["title"]),
                    state: text(&agent["state"]),
                    last,
                },
            );
        }
        for request in requests["requests"].as_array().into_iter().flatten() {
            self.requested(machine, request);
        }
    }

    /// Follows the session `agent` of `machine`: its conversation, from what `agent.show` gives
    /// and the events that come.
    pub(crate) fn follow(&mut self, machine: &str, agent: &str) {
        self.followed = Some((machine.to_owned(), agent.to_owned(), BTreeMap::new()));
    }

    /// The events `agent.show` gave of the followed session.
    pub(crate) fn shown(&mut self, machine: &str, agent: &str, events: &Value) {
        for event in events["events"].as_array().into_iter().flatten() {
            self.keep_followed(machine, agent, event);
        }
    }

    fn keep_followed(&mut self, machine: &str, agent: &str, event: &Value) {
        if let Some((followed_machine, followed_agent, events)) = &mut self.followed
            && followed_machine == machine
            && followed_agent == agent
            && let Some(seq) = event["seq"].as_u64()
        {
            events.insert(seq, event.clone());
        }
    }

    /// An event of `machine`'s.
    pub(crate) fn apply(&mut self, machine: &str, event: &Value) {
        let name = event["name"].as_str().unwrap_or_default();
        let data = &event["data"];
        let at = event["at"].as_u64().unwrap_or_default();
        let agent = data["agent"].as_str().unwrap_or_default().to_owned();
        if !agent.is_empty() {
            self.keep_followed(machine, &agent, event);
        }
        let key = (machine.to_owned(), agent.clone());
        match name {
            "agent.created" if data["parent"].is_null() => {
                self.sessions.insert(
                    key.clone(),
                    Session {
                        project: text(&data["project"]),
                        title: text(&data["title"]),
                        state: "starting".to_owned(),
                        last: at,
                    },
                );
            }
            "agent.titled" => {
                if let Some(session) = self.sessions.get_mut(&key) {
                    session.title = text(&data["title"]);
                }
            }
            "approval.requested" if data["held_by"] == "user" => self.requested(machine, data),
            "approval.decided" | "approval.cancelled" | "approval.settled" => {
                if let Some(number) = data["number"].as_u64() {
                    self.requests.remove(&(machine.to_owned(), number));
                }
            }
            "agent.asked" if data["to"] == "user" => {
                if let Some(asked) = event["seq"].as_u64() {
                    self.asks.insert(
                        (machine.to_owned(), asked),
                        AskView {
                            machine: machine.to_owned(),
                            agent: agent.clone(),
                            asked,
                            question: text(&data["question"]),
                        },
                    );
                }
            }
            "agent.answered" => match data["asked"].as_u64() {
                Some(asked) => {
                    self.asks.remove(&(machine.to_owned(), asked));
                }
                None => self
                    .asks
                    .retain(|(at, _), ask| at != machine || ask.agent != agent),
            },
            _ => {}
        }
        let state = match name {
            "agent.started" | "agent.prompted" => Some("working"),
            "agent.turn_ended" | "agent.released" => Some("idle"),
            "agent.ended" | "agent.failed" => Some("ended"),
            _ => None,
        };
        if matches!(name, "agent.ended" | "agent.failed") {
            self.asks
                .retain(|(at, _), ask| at != machine || ask.agent != agent);
        }
        if let Some(session) = self.sessions.get_mut(&key) {
            session.last = session.last.max(at);
            if let Some(state) = state {
                state.clone_into(&mut session.state);
            }
        }
    }

    fn requested(&mut self, machine: &str, request: &Value) {
        let Some(number) = request["number"].as_u64() else {
            return;
        };
        self.requests.insert(
            (machine.to_owned(), number),
            RequestView {
                machine: machine.to_owned(),
                number,
                agent: text(&request["agent"]),
                tool: text(&request["tool"]),
                reasons: request["reasons"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(text)
                    .collect(),
            },
        );
    }

    /// The state the app draws.
    pub(crate) fn state(&self) -> ViewState {
        let mut projects: BTreeMap<&str, Vec<SessionView>> = BTreeMap::new();
        for ((machine, agent), session) in &self.sessions {
            projects
                .entry(&session.project)
                .or_default()
                .push(SessionView {
                    machine: machine.clone(),
                    agent: agent.clone(),
                    title: session.title.clone(),
                    state: session.state.clone(),
                    last: session.last,
                });
        }
        let projects = projects
            .into_iter()
            .map(|(name, mut sessions)| {
                sessions.sort_by(|a, b| b.last.cmp(&a.last).then(a.agent.cmp(&b.agent)));
                ProjectView {
                    name: name.to_owned(),
                    sessions,
                }
            })
            .collect();
        ViewState {
            role: self.role.clone(),
            name: self.name.clone(),
            hub: self.hub.clone(),
            machines: self.machines.clone(),
            projects,
            conversation: self
                .followed
                .as_ref()
                .map(|(machine, agent, events)| ConversationView {
                    machine: machine.clone(),
                    agent: agent.clone(),
                    entries: entries(events.values()),
                }),
            requests: self.requests.values().cloned().collect(),
            asks: self.asks.values().cloned().collect(),
            said: self.said.clone(),
        }
    }
}

/// The entries of a conversation's events, oldest first: the pieces of one reply (or one thought)
/// are one entry.
fn entries<'a>(events: impl Iterator<Item = &'a Value>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut message: Option<String> = None;
    for event in events {
        let data = &event["data"];
        let (kind, piece) = match event["name"].as_str().unwrap_or_default() {
            "agent.prompted" | "agent.user_message" => ("user", false),
            "agent.message" => ("agent", true),
            "agent.thought" => ("thought", true),
            "agent.tool_call" if data["update"] != true => ("tool", false),
            _ => continue,
        };
        let text = if kind == "tool" {
            text(&data["title"])
        } else {
            text(&data["text"])
        };
        let id = data["message_id"].as_str().map(str::to_owned);
        let goes_on = piece
            && entries.last().is_some_and(|last| last.kind == kind)
            && (id.is_none() || id == message);
        match entries.last_mut() {
            Some(last) if goes_on => last.text.push_str(&text),
            _ => entries.push(Entry {
                kind: kind.to_owned(),
                text,
            }),
        }
        message = if piece { id } else { None };
    }
    entries
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}
