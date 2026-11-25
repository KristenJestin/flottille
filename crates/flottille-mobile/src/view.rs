//! The view state, computed in Rust from the member list and each runner's events
//! (`docs/remote.md`, "The Android app"): the machines, one list of sessions for every machine by
//! project (each session tagged with its machine, #1150), the conversation the app follows, the
//! requests and the questions that wait for the user, and this device's role. The screens draw it
//! as it is: they compute nothing of their own.

use std::collections::{BTreeMap, HashMap};

use std::time::{SystemTime, UNIX_EPOCH};

use flottille_link::{Members, PeerState, PublicKey, Role};
use flottille_protocol::words::{self, THE_CONVERSATION};

use crate::tunnel::{TunnelEnd, TunnelView};
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
    /// The members of the user's list, in the order they were added: this device among them.
    pub members: Vec<MemberView>,
    /// The sessions of every machine, by project: a project of the same name on two machines is
    /// one group. Projects by name, so that groups never move; sessions by last exchange.
    pub projects: Vec<ProjectView>,
    pub conversation: Option<ConversationView>,
    pub requests: Vec<RequestView>,
    pub asks: Vec<AskView>,
    /// The tunnels the phone holds to the runs of its machines, oldest first.
    pub tunnels: Vec<TunnelView>,
    /// The tunnels that ended, with why, the newest last: `TUNNELS_ENDED_KEPT` of them.
    pub tunnels_ended: Vec<TunnelEnd>,
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
    /// `online`, `offline` or `needs an update`: a machine that needs an update is never said
    /// offline (`docs/remote.md`, "The version").
    pub state: String,
    /// When it came to be in that state, in milliseconds since the Unix epoch.
    pub since: u64,
    /// Why it is not online, when it is not: the link's own sentence (the hub, which side to
    /// update).
    pub why: String,
    /// Whether its link goes through the hub (#1167): every link does since 2026-11-21, so it is
    /// whether it is online; kept for the app.
    pub through_hub: bool,
}

/// A member of the user's list.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MemberView {
    pub name: String,
    pub role: String,
    /// `desktop`, `laptop`, `server`, `phone`, `tablet`; empty for a member added before icons.
    pub icon: String,
    /// One of the eight colours; empty for a member added before colours.
    pub colour: String,
    /// Whether this member is this device.
    pub you: bool,
    /// Who added it: `the hub`, or a member's name; empty for the root.
    pub added_by: String,
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

/// The conversation of the session the app follows (`Core::follow`): its latest page of events,
/// live, with what the page needs to draw its controls.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ConversationView {
    pub machine: String,
    pub agent: String,
    pub title: String,
    /// The session's mode, `ask`, `auto` or `full`; empty before the daemon said it.
    pub mode: String,
    /// The agent's state as its events say it (`working`, `idle`, `ended`…).
    pub state: String,
    /// Whether a turn runs: what Stop interrupts.
    pub working: bool,
    /// Whether the agent is a child: it is written to by its parent only, its page reads.
    pub child: bool,
    pub ended: bool,
    /// Whether older events remain on the machine than the ones held (`Core::older`).
    pub more: bool,
    pub entries: Vec<Entry>,
    /// The agents of the session's tree under its root, read only.
    pub children: Vec<ChildView>,
    /// The commands of the followed session that run and have an address, oldest first: each opens
    /// through a tunnel.
    pub runs: Vec<RunView>,
}

/// A command of a session that runs and has an address (`run.started`, `run.ready`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RunView {
    pub run: String,
    /// The line it runs, as the daemon masks it.
    pub line: String,
    /// The address it reports.
    pub url: String,
}

/// One entry of a conversation: `user`, `agent`, `thought`, `tool` or `link`, and its text.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Entry {
    /// The number of the event the entry starts at: stable, and what orders the page.
    pub seq: u64,
    pub kind: String,
    pub text: String,
    /// The names of the pictures that went with a message of the user (`Core::image` reads each).
    pub images: Vec<String>,
    /// A `link` entry's address, the one an agent showed (`agent.link_shown`); empty for the rest.
    pub link: String,
}

/// An agent of the session's tree, as `agent.tree` gives it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ChildView {
    pub agent: String,
    pub title: String,
    /// What it does, in the daemon's words (`running \`cargo test\` for 12 min`).
    pub doing: String,
    pub role: String,
    pub depth: u32,
}

/// A request of an agent's that waits for the user's Allow or Deny. What the card says is the
/// window's, word for word (`flottille_protocol::words`): the daemon's reasons and the sentences of
/// the agents that forwarded it, who asked, what the judge said, and why a base request always asks.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RequestView {
    pub machine: String,
    pub number: u64,
    pub agent: String,
    pub tool: String,
    /// `call`, or `action`: a change of the project the agent asks for. An `action` of the tool
    /// `connections` is a proposal to write a connection, which a card never allows: the window
    /// opens a dialog of what it would write.
    pub kind: String,
    /// The call on one line as the window shows it: a command's line, else the tool and its
    /// arguments, the daemon's own text.
    pub call: String,
    /// A base request: it always asks, in every mode.
    pub base: bool,
    /// What the user allows, in the daemon's words, for a request that changes a project.
    pub says: Vec<String>,
    /// What the Auto judge said, when it was judged; else empty.
    pub judgement: String,
    /// Every reason it was held and every sentence of the agents that forwarded it, each a
    /// sentence of its own.
    pub why: String,
    /// Why a base request always asks (the window's sentence, which names a first message to
    /// another session apart); empty when it is not a base request.
    pub always: String,
    /// Who asked and who forwarded it, with its number: `asked by the conversation · #4`.
    pub source: String,
}

/// A question of an agent's that waits for the user's answer.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AskView {
    pub machine: String,
    pub agent: String,
    /// The number of its `agent.asked` event, which `agent.answer` names.
    pub asked: u64,
    /// The ask as one text.
    pub question: String,
    /// Each question with its options, as the daemon sent them: one question without options when
    /// the ask came without a list.
    pub questions: Vec<AskQuestion>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AskQuestion {
    pub question: String,
    pub options: Vec<AskOption>,
    /// Whether several options may be chosen.
    pub multiple: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AskOption {
    pub label: String,
    pub description: String,
}

/// What the view is computed from.
#[derive(Default)]
pub(crate) struct View {
    role: String,
    name: String,
    hub: String,
    /// The runners, in the list's order, with whether each is linked.
    machines: Vec<MachineView>,
    members: Vec<MemberView>,
    /// The sessions by machine and agent.
    sessions: BTreeMap<(String, String), Session>,
    requests: BTreeMap<(String, u64), Held>,
    asks: BTreeMap<(String, u64), AskView>,
    /// The followed session and its events by number: an event seen twice (loaded, then live) is
    /// one.
    followed: Option<Followed>,
    tunnels: Vec<TunnelView>,
    tunnels_ended: Vec<TunnelEnd>,
    said: Vec<String>,
}

/// The session whose page is on screen.
struct Followed {
    machine: String,
    agent: String,
    events: BTreeMap<u64, Value>,
    /// The agent as `agent.show` gave it: its title, its mode, whether it has a parent.
    info: Value,
    state: String,
    mode: String,
    more: bool,
    children: Vec<ChildView>,
}

/// A request as the daemon gave it. Who asked and who forwarded it are named when the view is
/// computed: the names are the followed page's tree.
struct Held {
    number: u64,
    agent: String,
    tool: String,
    kind: String,
    call: String,
    base: bool,
    says: Vec<String>,
    judgement: Option<(String, String, String)>,
    reasons: Vec<String>,
    forwards: Vec<(String, String)>,
}

struct Session {
    project: String,
    title: String,
    state: String,
    last: u64,
}

/// The tunnels that ended kept in `tunnels_ended`, the newest.
const TUNNELS_ENDED_KEPT: usize = 16;

/// The sentences kept in `said`, the newest.
const SAID_KEPT: usize = 8;

impl View {
    /// The list as this device checked it: its own entry, and the runners.
    pub(crate) fn members(&mut self, members: &Members, own: &PublicKey) {
        match members.get(own) {
            Some(me) => {
                self.role = me.role.word().to_owned();
                self.name = me.name.clone();
            }
            // Removed: no role it could claim, and the user told why.
            None => {
                if !self.role.is_empty() {
                    self.say(
                        "this phone was removed from the user's hub: pair it again".to_owned(),
                    );
                }
                self.role.clear();
                self.name.clear();
            }
        }
        self.members = members
            .members()
            .iter()
            .map(|member| MemberView {
                name: member.name.clone(),
                role: member.role.word().to_owned(),
                icon: member
                    .icon
                    .map(|icon| icon.word().to_owned())
                    .unwrap_or_default(),
                colour: member
                    .colour
                    .map(|colour| colour.word().to_owned())
                    .unwrap_or_default(),
                you: member.key == *own,
                added_by: if member.added_by == members.hub() {
                    "the hub".to_owned()
                } else if member.added_by == member.key {
                    String::new()
                } else {
                    members
                        .get(&member.added_by)
                        .map(|adder| adder.name.clone())
                        .unwrap_or_default()
                },
            })
            .collect();
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
                    state: PeerState::Offline.to_string(),
                    since: now_millis(),
                    why: "not linked yet".to_owned(),
                    through_hub: false,
                },
            })
            .collect();
        // What a runner that left the list said goes with it.
        for name in before.keys() {
            if !self.machines.iter().any(|machine| machine.name == *name) {
                self.forget(name);
            }
        }
    }

    /// Forgets all of the runner `machine`: it left the list, or its name is another's now.
    pub(crate) fn forget(&mut self, machine: &str) {
        self.machines.retain(|view| view.name != machine);
        self.sessions.retain(|(of, _), _| of != machine);
        self.requests.retain(|(of, _), _| of != machine);
        self.asks.retain(|(of, _), _| of != machine);
        if self.followed.as_ref().is_some_and(|f| f.machine == machine) {
            self.followed = None;
        }
    }

    /// The link to `machine` is in `state`, for `why`. `since` moves only when the state does: a
    /// machine that fails again is offline since the first time.
    pub(crate) fn online(&mut self, machine: &str, state: PeerState, through_hub: bool, why: &str) {
        if let Some(view) = self.machines.iter_mut().find(|view| view.name == machine) {
            let state = state.to_string();
            if view.state != state {
                view.since = now_millis();
                view.state = state;
            }
            view.online = view.state == PeerState::Online.to_string();
            view.through_hub = view.online && through_hub;
            why.clone_into(&mut view.why);
        }
    }

    /// This phone removed itself from the list: the hub does not send the list to a device it just
    /// removed, so nothing of the user's is left to show, and the user is told why.
    pub(crate) fn removed(&mut self) {
        *self = View::default();
        self.say("this phone was removed from the user's hub: pair it again".to_owned());
    }

    pub(crate) fn hub(&mut self, hub: &str) {
        hub.clone_into(&mut self.hub);
    }

    /// A tunnel the phone holds.
    pub(crate) fn tunnel_opened(&mut self, tunnel: TunnelView) {
        self.tunnels.push(tunnel);
    }

    /// A tunnel ended: it is no longer held, and `reason` says why. One the view never held (a
    /// join again cleared it) says nothing.
    pub(crate) fn tunnel_ended(&mut self, tunnel: &TunnelView, reason: String) {
        let before = self.tunnels.len();
        self.tunnels.retain(|held| held.tunnel != tunnel.tunnel);
        if self.tunnels.len() == before {
            return;
        }
        self.tunnels_ended.push(TunnelEnd {
            tunnel: tunnel.tunnel.clone(),
            machine: tunnel.machine.clone(),
            run: tunnel.run.clone(),
            reason,
        });
        if self.tunnels_ended.len() > TUNNELS_ENDED_KEPT {
            self.tunnels_ended.remove(0);
        }
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
            if !is_session(agent) {
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
        self.followed = Some(Followed {
            machine: machine.to_owned(),
            agent: agent.to_owned(),
            events: BTreeMap::new(),
            info: Value::Null,
            state: String::new(),
            mode: String::new(),
            more: false,
            children: Vec::new(),
        });
    }

    /// The page is left: nothing of it is kept.
    pub(crate) fn unfollow(&mut self) {
        self.followed = None;
    }

    /// The number of the first event the followed page holds: where older ones end.
    pub(crate) fn first_followed(&self, machine: &str, agent: &str) -> Option<u64> {
        self.followed
            .as_ref()
            .filter(|f| f.machine == machine && f.agent == agent)
            .and_then(|f| f.events.keys().next().copied())
    }

    /// A page of events `agent.show` gave of the followed session (the latest, or older ones).
    pub(crate) fn shown(&mut self, machine: &str, agent: &str, shown: &Value) {
        let Some(followed) = self
            .followed
            .as_mut()
            .filter(|f| f.machine == machine && f.agent == agent)
        else {
            return;
        };
        for event in shown["events"].as_array().into_iter().flatten() {
            if let Some(seq) = event["seq"].as_u64() {
                followed.events.insert(seq, event.clone());
            }
        }
        followed.more = shown["more"] == true;
        if shown["agent"].is_object() {
            followed.info = shown["agent"].clone();
            followed.state = text(&followed.info["state"]);
            followed.mode = text(&followed.info["gate_mode"]);
        }
    }

    /// The tree of the followed session's root: its agents under it, read only, and the questions
    /// its root asks the user and still waits for.
    pub(crate) fn tree(&mut self, machine: &str, agent: &str, tree: &Value) {
        let Some(followed) = self
            .followed
            .as_mut()
            .filter(|f| f.machine == machine && f.agent == agent)
        else {
            return;
        };
        followed.children = tree["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| !entry["agent"]["parent"].is_null())
            .map(|entry| ChildView {
                agent: text(&entry["agent"]["id"]),
                title: text(&entry["agent"]["title"]),
                doing: text(&entry["doing"]),
                role: text(&entry["agent"]["role"]),
                depth: entry["agent"]["depth"].as_u64().unwrap_or(1) as u32,
            })
            .collect();
        for entry in tree["agents"].as_array().into_iter().flatten() {
            if !entry["agent"]["parent"].is_null() {
                continue;
            }
            let root = text(&entry["agent"]["id"]);
            self.asks
                .retain(|(at, _), ask| at != machine || ask.agent != root);
            for ask in entry["questions"].as_array().into_iter().flatten() {
                if let Some(asked) = ask["seq"].as_u64() {
                    self.asks.insert(
                        (machine.to_owned(), asked),
                        ask_view(machine, &root, asked, ask),
                    );
                }
            }
        }
    }

    fn keep_followed(&mut self, machine: &str, agent: &str, event: &Value) {
        if let Some(followed) = &mut self.followed
            && followed.machine == machine
            && followed.agent == agent
            && let Some(seq) = event["seq"].as_u64()
        {
            followed.events.insert(seq, event.clone());
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
            "agent.created" if is_session(data) => {
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
                        ask_view(machine, &agent, asked, data),
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
        if let Some(followed) = &mut self.followed
            && followed.machine == machine
            && followed.agent == agent
        {
            if let Some(state) = state {
                state.clone_into(&mut followed.state);
            }
            if name == "agent.gate_mode_changed" {
                followed.mode = text(&data["mode"]);
            }
        }
    }

    fn requested(&mut self, machine: &str, request: &Value) {
        let Some(number) = request["number"].as_u64() else {
            return;
        };
        let strings = |value: &Value| -> Vec<String> {
            value.as_array().into_iter().flatten().map(text).collect()
        };
        let tool = text(&request["tool"]);
        self.requests.insert(
            (machine.to_owned(), number),
            Held {
                number,
                agent: text(&request["agent"]),
                call: call_line(&tool, &request["call"]),
                tool,
                kind: match request["kind"].as_str() {
                    Some(kind) if !kind.is_empty() => kind.to_owned(),
                    _ => "call".to_owned(),
                },
                base: request["class"] == "base",
                says: strings(&request["says"]),
                judgement: request["judgement"].is_object().then(|| {
                    let judgement = &request["judgement"];
                    (
                        text(&judgement["by"]),
                        text(&judgement["verdict"]),
                        text(&judgement["reason"]),
                    )
                }),
                reasons: strings(&request["reasons"]),
                forwards: request["forwards"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|forward| (text(&forward["by"]), text(&forward["why"])))
                    .collect(),
            },
        );
    }

    /// How a request's card names an agent, as the window does: the followed session is the
    /// conversation, a child of its tree is its title, any other `agent <id>`.
    fn said_name(&self, machine: &str, id: &str) -> String {
        match &self.followed {
            Some(followed) if followed.machine == machine => {
                if followed.agent == id {
                    return THE_CONVERSATION.to_owned();
                }
                if let Some(child) = followed.children.iter().find(|child| child.agent == id)
                    && !child.title.trim().is_empty()
                {
                    return child.title.clone();
                }
            }
            _ => {}
        }
        format!("agent {id}")
    }

    /// The request as the app draws it.
    fn request_view(&self, machine: &str, held: &Held) -> RequestView {
        let forwards: Vec<String> = held
            .forwards
            .iter()
            .map(|(by, said)| words::forwarded(&self.said_name(machine, by), said))
            .collect();
        let why = words::sentences(
            held.reasons
                .iter()
                .map(String::as_str)
                .chain(forwards.iter().map(String::as_str)),
        );
        let forwarders: Vec<String> = held
            .forwards
            .iter()
            .map(|(by, _)| self.said_name(machine, by))
            .collect();
        RequestView {
            machine: machine.to_owned(),
            number: held.number,
            agent: held.agent.clone(),
            tool: held.tool.clone(),
            kind: held.kind.clone(),
            call: held.call.clone(),
            base: held.base,
            says: held.says.clone(),
            judgement: held
                .judgement
                .as_ref()
                .map(|(by, verdict, reason)| words::judged(by, verdict, reason))
                .unwrap_or_default(),
            always: match held.base {
                true => words::always_says(&why).to_owned(),
                false => String::new(),
            },
            source: words::source(
                held.number,
                &self.said_name(machine, &held.agent),
                forwarders.iter().map(String::as_str),
            ),
            why,
        }
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
            members: self.members.clone(),
            projects,
            conversation: self.followed.as_ref().map(|followed| {
                let session = self
                    .sessions
                    .get(&(followed.machine.clone(), followed.agent.clone()));
                let title = session
                    .map(|session| session.title.clone())
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| text(&followed.info["title"]));
                ConversationView {
                    machine: followed.machine.clone(),
                    agent: followed.agent.clone(),
                    title,
                    mode: followed.mode.clone(),
                    working: followed.state == "working",
                    child: !followed.info["parent"].is_null(),
                    ended: matches!(followed.state.as_str(), "ended" | "failed"),
                    state: followed.state.clone(),
                    more: followed.more,
                    entries: entries(followed.events.values()),
                    children: followed.children.clone(),
                    runs: runs(followed.events.values()),
                }
            }),
            requests: self
                .requests
                .iter()
                .map(|((machine, _), held)| self.request_view(machine, held))
                .collect(),
            asks: self.asks.values().cloned().collect(),
            tunnels: self.tunnels.clone(),
            tunnels_ended: self.tunnels_ended.clone(),
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
            // The card of the link is what the user sees of a `web` `show_link`, not the step.
            "agent.tool_call" if data["raw_input"]["action"] == "show_link" => continue,
            "agent.tool_call" if data["update"] != true => ("tool", false),
            // Only the links of the web and of a dev server open on the phone; a file of the
            // machine is not on it.
            "agent.link_shown" if data["kind"] == "url" => ("link", false),
            _ => continue,
        };
        let text = match kind {
            "tool" => text(&data["title"]),
            "link" => Some(text(&data["title"]))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| text(&data["host"])),
            _ => text(&data["text"]),
        };
        let id = data["message_id"].as_str().map(str::to_owned);
        let goes_on = piece
            && entries.last().is_some_and(|last| last.kind == kind)
            && (id.is_none() || id == message);
        match entries.last_mut() {
            Some(last) if goes_on => last.text.push_str(&text),
            _ => entries.push(Entry {
                seq: event["seq"].as_u64().unwrap_or_default(),
                kind: kind.to_owned(),
                text,
                images: data["images"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(self::text)
                    .collect(),
                link: if kind == "link" {
                    self::text(&data["target"])
                } else {
                    String::new()
                },
            }),
        }
        message = if piece { id } else { None };
    }
    entries
}

/// The commands of a session that run and have an address, oldest first: from `run.started`,
/// `run.ready` and `run.ended` among the events the page holds.
fn runs<'a>(events: impl Iterator<Item = &'a Value>) -> Vec<RunView> {
    let mut runs: Vec<RunView> = Vec::new();
    for event in events {
        let data = &event["data"];
        let run = text(&data["run"]);
        match event["name"].as_str().unwrap_or_default() {
            "run.started" | "run.ready" => {
                let url = text(&data["url"]);
                let line = text(&data["line"]);
                match runs.iter_mut().find(|known| known.run == run) {
                    Some(known) => {
                        if !url.is_empty() {
                            known.url = url;
                        }
                        if !line.is_empty() {
                            known.line = line;
                        }
                    }
                    None => runs.push(RunView { run, line, url }),
                }
            }
            "run.ended" => runs.retain(|known| known.run != run),
            _ => {}
        }
    }
    runs.retain(|run| !run.url.is_empty());
    runs
}

/// A question of the agent `agent`, from an `agent.asked` event or a question of `agent.tree`:
/// each question with its options, or the ask's own text as the one question when it came
/// without a list.
fn ask_view(machine: &str, agent: &str, asked: u64, ask: &Value) -> AskView {
    let question = text(&ask["question"]);
    let mut questions: Vec<AskQuestion> = ask["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|each| AskQuestion {
            question: text(&each["question"]),
            options: each["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|option| AskOption {
                    label: text(&option["label"]),
                    description: text(&option["description"]),
                })
                .collect(),
            multiple: each["multiple"] == true,
        })
        .collect();
    if questions.is_empty() {
        questions.push(AskQuestion {
            question: question.clone(),
            options: Vec::new(),
            multiple: false,
        });
    }
    AskView {
        machine: machine.to_owned(),
        agent: agent.to_owned(),
        asked,
        question,
        questions,
    }
}

/// The prefix of the tools a connection offers (`conn_<name>`).
const CONNECTION: &str = "conn_";

/// The call of a request on one line, as the window shows it: a command's line, else the tool and
/// its arguments. The daemon's own words, masked and cut by it: nothing is summarised here.
fn call_line(tool: &str, call: &Value) -> String {
    match (tool, call["line"].as_str()) {
        ("run", Some(line)) => line.to_owned(),
        (tool, _) if tool.starts_with(CONNECTION) => {
            let name = &tool[CONNECTION.len()..];
            let action = call["action"].as_str().unwrap_or("?");
            match &call["arguments"] {
                Value::Null => format!("{name} {action}"),
                arguments => format!("{name} {action} {arguments}"),
            }
        }
        (tool, _) => format!("{tool} {call}"),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

/// Whether an agent of `agent.list` or `agent.created` is a session, the root of a tree: never a
/// child, here or one an agent of another machine launched, which has no parent there and is at
/// depth 1 all the same. A machine of before says no depth.
fn is_session(agent: &Value) -> bool {
    agent["parent"].is_null() && agent["depth"].as_u64().unwrap_or(0) == 0
}

#[cfg(test)]
mod tests {
    use flottille_link::{Action, DeviceKey, MemberList};

    use super::*;

    #[test]
    fn a_child_launched_from_another_machine_is_not_a_session_on_the_phone() {
        let mut view = View::default();
        let agents = serde_json::json!({"agents": [
            {"id": "a1", "parent": null, "depth": 0, "title": "Export", "project": "app"},
            {"id": "a2", "parent": null, "depth": 1, "title": "Write the tests", "project": "app",
                "launched_from": {"machine": "pc", "agent": "b1", "kind": "child"}},
            {"id": "a3", "parent": "a1", "depth": 1, "title": "Read", "project": "app"},
        ]});
        view.load("server", &agents, &serde_json::json!({"requests": []}));
        view.apply(
            "server",
            &serde_json::json!({"name": "agent.created", "at": 1, "data": {"agent": "a4",
                "parent": null, "depth": 1, "title": "Another", "project": "app"}}),
        );
        let listed: Vec<&str> = view.sessions.keys().map(|(_, id)| id.as_str()).collect();
        assert_eq!(listed, vec!["a1"]);
    }

    #[test]
    fn a_request_and_a_question_say_when_they_came() {
        let mut view = View::default();
        let before = now_millis();
        view.load(
            "server",
            &serde_json::json!({"agents": []}),
            &serde_json::json!({"requests": [
                {"number": 4, "agent": "a1", "tool": "run", "call": {"line": "ls"},
                    "created_at": 1_000},
                {"number": 5, "agent": "a1", "tool": "run", "call": {"line": "ls"}},
            ]}),
        );
        let requested = serde_json::json!({"name": "approval.requested", "seq": 7, "at": 2,
            "data": {"number": 4, "agent": "b1", "tool": "run", "call": {"line": "pwd"},
                "held_by": "user"}});
        view.apply("laptop", &requested);
        view.apply(
            "laptop",
            &serde_json::json!({"name": "agent.asked", "seq": 9, "at": 3,
                "data": {"agent": "b1", "to": "user", "question": "Push?"}}),
        );
        view.follow("server", "a1");
        view.tree(
            "server",
            "a1",
            &serde_json::json!({"agents": [{"agent": {"id": "a1", "parent": null},
                "questions": [{"seq": 12, "at": 1_500, "question": "Which one?"}]}]}),
        );
        let after = now_millis();
        let state = view.state();
        let request = |machine: &str, number: u64| {
            state
                .requests
                .iter()
                .find(|r| r.machine == machine && r.number == number)
                .unwrap()
                .at
        };
        let ask = |machine: &str, asked: u64| {
            state
                .asks
                .iter()
                .find(|a| a.machine == machine && a.asked == asked)
                .unwrap()
                .at
        };
        // Already there at the link: its own time, else the link's.
        assert_eq!(request("server", 4), 1_000);
        assert!((before..=after).contains(&request("server", 5)));
        assert_eq!(ask("server", 12), 1_500);
        // Come since: when the phone saw it, the daemon's counters and clock aside.
        assert!((before..=after).contains(&request("laptop", 4)));
        assert!((before..=after).contains(&ask("laptop", 9)));

        // Seen again (a link again, the tree again): it keeps the time it came at.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let first = request("laptop", 4);
        view.apply("laptop", &requested);
        view.tree(
            "server",
            "a1",
            &serde_json::json!({"agents": [{"agent": {"id": "a1", "parent": null},
                "questions": [{"seq": 12, "question": "Which one?"}]}]}),
        );
        let again = view.state();
        assert_eq!(
            again
                .requests
                .iter()
                .find(|r| r.machine == "laptop")
                .unwrap()
                .at,
            first
        );
        assert_eq!(again.asks.iter().find(|a| a.asked == 12).unwrap().at, 1_500);
    }

    #[test]
    fn a_child_s_request_names_its_session_and_the_child() {
        let mut view = View::default();
        let request = |number: u64, agent: &str| {
            serde_json::json!({"number": number, "agent": agent, "tool": "run",
                "call": {"line": "ls"}})
        };
        view.load(
            "server",
            &serde_json::json!({"agents": [
                {"id": "a1", "parent": null, "depth": 0, "title": "Fix the login",
                    "project": "app"},
                {"id": "c1", "parent": "a1", "depth": 1, "title": "Check"},
                {"id": "c2", "parent": "c1", "depth": 2, "title": ""},
            ]}),
            &serde_json::json!({"requests": [request(4, "c2"), request(5, "a1"),
                request(6, "c1"), request(8, "x9")]}),
        );
        view.apply(
            "server",
            &serde_json::json!({"name": "agent.created", "at": 1, "data": {"agent": "c3",
                "parent": "a1", "depth": 1, "title": "Write"}}),
        );
        let mut held = request(7, "c3");
        held["held_by"] = "user".into();
        view.apply(
            "server",
            &serde_json::json!({"name": "approval.requested", "at": 2, "data": held}),
        );
        let state = view.state();
        let said: Vec<(u64, &str, &str)> = state
            .requests
            .iter()
            .map(|r| (r.number, r.session.as_str(), r.child.as_str()))
            .collect();
        assert_eq!(
            said,
            vec![
                (4, "a1", "agent c2"),
                (5, "a1", ""),
                (6, "a1", "Check"),
                (7, "a1", "Write"),
                // An agent the phone does not know is taken for a session.
                (8, "x9", ""),
            ]
        );
    }

    #[test]
    fn a_phone_removed_from_the_list_claims_no_role_and_says_so() {
        let (owner, phone) = (DeviceKey::make().unwrap(), DeviceKey::make().unwrap());
        let mut list = MemberList::default();
        for (key, name, role) in [
            (&owner, "server", Role::Runner),
            (&phone, "phone", Role::Controller),
        ] {
            list.sign(
                Action::Add {
                    key: key.public(),
                    name: name.to_owned(),
                    role,
                },
                &owner,
            );
        }
        let hub = DeviceKey::make().unwrap().public();
        let mut view = View::default();
        view.members(&list.check(&owner.public(), &hub).unwrap(), &phone.public());
        assert_eq!(
            (view.role.as_str(), view.name.as_str()),
            ("controller", "phone")
        );

        list.sign(
            Action::Remove {
                key: phone.public(),
            },
            &owner,
        );
        view.members(&list.check(&owner.public(), &hub).unwrap(), &phone.public());
        let state = view.state();
        assert_eq!((state.role.as_str(), state.name.as_str()), ("", ""));
        assert_eq!(
            state.said,
            ["this phone was removed from the user's hub: pair it again"]
        );
    }
}
