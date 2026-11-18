//! What the daemon keeps of the window (`docs/design.md` §10, `docs/protocol.md`, "The draft and
//! window methods"): the text and the pictures a composer holds and did not send, and what the
//! sidebar has folded. They are the user's own: a draft is never logged, never in an event, never
//! given to an agent, and drawn nowhere but in its composer.

use std::collections::HashMap;
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

/// What a composer knows of its draft and how it keeps it: the debounce, and what the daemon holds
/// as far as it knows, so that nothing is written that the daemon has already.
pub struct Keeping {
    key: DraftKey,
    /// What the daemon holds: what was read from it, or what was last written.
    known: Option<Draft>,
    /// The daemon answered the read of the draft: until then nothing is written, which would
    /// replace what it holds with an empty field.
    read: bool,
    /// The wait that ends with the write.
    due: Option<Task<()>>,
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

    /// Whether a write waits for its 500 ms.
    pub fn is_due(&self) -> bool {
        self.due.is_some()
    }

    /// The daemon's answer to the read of the draft: what it holds, which the composer takes if
    /// it is still empty.
    pub fn answered(&mut self, found: Option<Draft>) {
        self.read = true;
        self.known = found.filter(|draft| !draft.is_empty());
    }

    pub fn was_read(&self) -> bool {
        self.read
    }

    /// The composer changed: the draft is written 500 ms after its last change, by `write`.
    pub fn changed<T: 'static>(&mut self, cx: &mut Context<T>, write: fn(&mut T, &mut Context<T>)) {
        self.due = Some(cx.spawn(async move |this: WeakEntity<T>, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            this.update(cx, write).ok();
        }));
    }

    /// What to ask of the daemon to keep `now`, when it does not hold it already: its pictures
    /// only when they changed. Ends the wait for the write.
    pub fn write(&mut self, now: Draft) -> Option<Command> {
        self.due = None;
        if !self.read {
            return None;
        }
        let kept = self.known.clone().unwrap_or_default();
        if now == kept {
            return None;
        }
        let pictures = (now.pictures != kept.pictures).then(|| now.pictures.clone());
        let command = Command::DraftWrite {
            key: self.key.clone(),
            text: now.text.clone(),
            pictures,
        };
        self.sent += 1;
        self.known = Some(now).filter(|draft| !draft.is_empty());
        Some(command)
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
        }
    }

    pub fn to_json(&self) -> Value {
        let mut json = json!({"projects": self.projects, "leads": self.leads});
        if let Some(ended) = self.ended {
            json["ended"] = json!(ended);
        }
        json
    }
}
