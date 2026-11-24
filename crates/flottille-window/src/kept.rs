//! What the daemon keeps of the window (`docs/design.md` §10, `docs/protocol.md`, "The draft and
//! window methods"): the text and the pictures a composer holds and did not send, and what the
//! sidebar has folded. They are the user's own: a draft is never logged, never in an event, never
//! given to an agent, and drawn nowhere but in its composer.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use gpui_kit::{Context, Task, WeakEntity};
use serde_json::{Value, json};

use crate::actions::{Command, Picture};

/// How long after the last change a draft is written.
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// The composer a draft belongs to, as the daemon names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DraftKey {
    /// A session's composer, by its root agent's id.
    Session(String),
    /// A project's first message of a new session.
    Project(String),
}

impl DraftKey {
    pub fn kind(&self) -> &'static str {
        match self {
            DraftKey::Session(_) => "session",
            DraftKey::Project(_) => "project",
        }
    }

    pub fn target(&self) -> &str {
        match self {
            DraftKey::Session(target) | DraftKey::Project(target) => target,
        }
    }
}

/// What a composer holds and did not send.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub pictures: Vec<Picture>,
}

impl Draft {
    /// Nothing worth keeping: no word and no picture.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.pictures.is_empty()
    }
}

/// How a write of a draft, or of the window's state, ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    /// The daemon keeps it.
    Kept,
    /// The session or the project it belongs to is gone: nothing is kept, and nothing is asked
    /// again. Said to nobody: the composer of a thing that is gone is closing.
    Gone,
    /// The daemon could not be reached, or could not write: it is asked again, later.
    Retry,
    /// The daemon refused it for good (a text too long): asking again would be refused again.
    Refused,
}

/// The longest wait before a failed read or write is asked again.
const LONGEST_WAIT: Duration = Duration::from_secs(30);

/// What a composer knows of its draft and how it keeps it: the debounce, and what the daemon holds
/// as far as it is known, so that nothing is written that the daemon has already.
pub struct Keeping {
    key: DraftKey,
    /// What the daemon holds: what was read from it, or the last write it answered it kept.
    known: Option<Draft>,
    /// The daemon answered the read of the draft: until then nothing is written, which would
    /// replace what it holds with an empty field.
    read: bool,
    /// The wait that ends with the write.
    due: Option<Task<()>>,
    /// The wait that ends with the next try of a read the daemon did not answer.
    rereading: Option<Task<()>>,
    /// The writes asked and not answered yet, oldest first (the keeper answers in order).
    writing: VecDeque<Draft>,
    /// How many reads or writes in a row the daemon did not answer.
    failures: u32,
    /// The composer's session or project is gone: nothing is written.
    gone: bool,
    /// How many writes were asked.
    sent: usize,
}

impl Keeping {
    pub fn new(key: DraftKey) -> Self {
        Self {
            key,
            known: None,
            read: false,
            due: None,
            rereading: None,
            writing: VecDeque::new(),
            failures: 0,
            gone: false,
            sent: 0,
        }
    }

    pub fn key(&self) -> &DraftKey {
        &self.key
    }

    /// How many times the draft was written to the daemon.
    pub fn sent(&self) -> usize {
        self.sent
    }

    /// How many writes were asked and not answered yet.
    pub fn unanswered(&self) -> usize {
        self.writing.len()
    }

    /// Whether a write waits for its 500 ms.
    pub fn is_due(&self) -> bool {
        self.due.is_some()
    }

    /// The daemon's answer to the read of the draft: what it holds, which the composer takes if
    /// it is still empty.
    pub fn answered(&mut self, found: Option<Draft>) {
        self.read = true;
        self.failures = 0;
        self.rereading = None;
        self.known = found.filter(|draft| !draft.is_empty());
    }

    /// Whether the daemon answered the read of the draft.
    pub fn was_read(&self) -> bool {
        self.read
    }

    /// How long the next try waits: the debounce, doubled by each failure in a row.
    fn wait(&self) -> Duration {
        (DEBOUNCE * 2u32.pow(self.failures.min(6))).min(LONGEST_WAIT)
    }

    /// The daemon did not answer the read: it is asked again, later, by `again`. Nothing is
    /// written meanwhile, a field read as empty would replace what the daemon holds.
    pub fn read_failed<T: 'static>(
        &mut self,
        cx: &mut Context<T>,
        again: fn(&mut T, &mut Context<T>),
    ) {
        let wait = self.wait();
        self.failures += 1;
        self.rereading = Some(cx.spawn(async move |this: WeakEntity<T>, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, again).ok();
        }));
    }

    /// The composer changed: the draft is written 500 ms after its last change, by `write`.
    pub fn changed<T: 'static>(&mut self, cx: &mut Context<T>, write: fn(&mut T, &mut Context<T>)) {
        let wait = self.wait();
        self.due = Some(cx.spawn(async move |this: WeakEntity<T>, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, write).ok();
        }));
    }

    /// What to ask of the daemon to keep `now`, when it does not hold it already: its pictures
    /// only when they are not the ones it holds. Ends the wait for the write.
    ///
    /// `closing`: the window is closing, nothing will answer, so what the daemon is not known to
    /// hold is asked whatever is in flight, and a field typed in before the read of the draft was
    /// answered is written once the keeper has read it.
    pub fn write(&mut self, now: Draft, closing: bool) -> Option<Command> {
        self.due = None;
        if self.gone {
            return None;
        }
        let known = self.known.clone().unwrap_or_default();
        let after_read = !self.read;
        if after_read {
            if !closing || now.is_empty() {
                return None;
            }
        } else {
            let expected = if closing {
                known.clone()
            } else {
                self.writing
                    .back()
                    .cloned()
                    .unwrap_or_else(|| known.clone())
            };
            if now == expected {
                return None;
            }
        }
        let pictures = (after_read || now.pictures != known.pictures).then(|| now.pictures.clone());
        let command = Command::DraftWrite {
            key: self.key.clone(),
            text: now.text.clone(),
            pictures,
            after_read,
        };
        self.sent += 1;
        self.writing.push_back(now);
        Some(command)
    }

    /// The daemon answered the oldest write asked. `true` when it must be asked again.
    pub fn wrote(&mut self, result: Written) -> bool {
        let sent = self.writing.pop_front();
        match result {
            Written::Kept => {
                self.failures = 0;
                self.known = sent.filter(|draft| !draft.is_empty());
                false
            }
            Written::Gone => {
                self.gone = true;
                self.known = None;
                false
            }
            Written::Refused => false,
            Written::Retry => {
                self.failures += 1;
                true
            }
        }
    }
}

/// What the sidebar has folded: each project, each lead, the "Ended" group, by what it is now. A
/// node that is not in it keeps its default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds {
    /// Each project, by name: whether its sessions are unfolded.
    pub projects: HashMap<String, bool>,
    /// Each session that leads others, by id: whether its led rows are unfolded.
    pub leads: HashMap<String, bool>,
    /// The "Ended" group: whether it is unfolded.
    pub ended: Option<bool>,
    /// The projects are shown by machine, one section each, instead of in one list.
    pub by_machine: bool,
    /// Each machine's section, by its name: whether it is unfolded.
    pub machines: HashMap<String, bool>,
    /// Each project of a machine's section, by [`machine_key`]: whether it is unfolded.
    pub on_machines: HashMap<String, bool>,
    /// Each shared project, by name: the machine the user chose its sessions start on (#1266).
    pub starts: HashMap<String, String>,
}

/// What the state keeps a project of a machine's section under.
pub fn machine_key(machine: &str, project: &str) -> String {
    format!("{machine}\u{1f}{project}")
}

impl Folds {
    /// The sidebar as the daemon keeps it (`window.get`'s `state`).
    pub fn of(state: &Value) -> Folds {
        let flags = |key: &str| -> HashMap<String, bool> {
            state["sidebar"][key]
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter_map(|(name, open)| Some((name.clone(), open.as_bool()?)))
                        .collect()
                })
                .unwrap_or_default()
        };
        Folds {
            projects: flags("projects"),
            leads: flags("leads"),
            ended: state["sidebar"]["ended"].as_bool(),
            by_machine: state["sidebar"]["by_machine"].as_bool().unwrap_or(false),
            machines: flags("machines"),
            on_machines: flags("on_machines"),
            starts: state["sidebar"]["starts"]
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter_map(|(name, on)| Some((name.clone(), on.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut json = json!({"projects": self.projects, "leads": self.leads});
        if let Some(ended) = self.ended {
            json["ended"] = json!(ended);
        }
        if self.by_machine {
            json["by_machine"] = json!(true);
        }
        if !self.machines.is_empty() {
            json["machines"] = json!(self.machines);
        }
        if !self.on_machines.is_empty() {
            json["on_machines"] = json!(self.on_machines);
        }
        if !self.starts.is_empty() {
            json["starts"] = json!(self.starts);
        }
        json
    }
}
