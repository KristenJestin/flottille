//! The window's history, as a browser's: the places shown in it, in order, and where the window
//! stands among them. It lives as long as the window and is not stored.

use crate::shell::Selection;

/// A place of the history: a page, and for a session's page the agent shown in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visit {
    pub selection: Selection,
    /// The sub-agent whose conversation is shown, `None` for the session's own.
    pub agent: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Back,
    Forward,
}

#[derive(Debug, Default)]
pub struct History {
    visits: Vec<Visit>,
    at: usize,
}

impl History {
    pub fn current(&self) -> Option<&Visit> {
        self.visits.get(self.at)
    }

    /// A page was opened: what was ahead is gone. The page already shown adds nothing.
    pub fn open(&mut self, selection: Selection) {
        if self.current().is_some_and(|v| v.selection == selection) {
            return;
        }
        self.push(Visit {
            selection,
            agent: None,
        });
    }

    /// The session `session`'s page shows `agent` (the session itself for its own conversation).
    pub fn show_agent(&mut self, session: &str, agent: &str) {
        let agent = (agent != session).then(|| agent.to_owned());
        let Some(current) = self.current() else {
            return;
        };
        if current.selection != Selection::Session(session.to_owned()) || current.agent == agent {
            return;
        }
        let selection = current.selection.clone();
        self.push(Visit { selection, agent });
    }

    fn push(&mut self, visit: Visit) {
        self.visits
            .truncate(self.at + usize::from(!self.visits.is_empty()));
        self.visits.push(visit);
        self.at = self.visits.len() - 1;
    }

    /// The nearest place that way which `exists` (a forgotten session or a removed project is
    /// skipped), if any.
    pub fn find(&self, way: Way, exists: impl Fn(&Visit) -> bool) -> Option<usize> {
        match way {
            Way::Back => (0..self.at).rev().find(|&i| exists(&self.visits[i])),
            Way::Forward => (self.at + 1..self.visits.len()).find(|&i| exists(&self.visits[i])),
        }
    }

    /// Stands on the place `at` (found by [`History::find`]).
    pub fn move_to(&mut self, at: usize) -> Visit {
        self.at = at;
        self.visits[at].clone()
    }
}
