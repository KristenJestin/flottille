//! What the window shows, made from the daemon's answers: projects, their places, the sessions in
//! each place, and what waits for the user. Nothing here talks to the socket.

use std::collections::{HashMap, HashSet};

use flottille_ui::AgentState;
use serde_json::Value;

/// Everything the shell draws, as one fresh read of the daemon gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// By name.
    pub projects: Vec<Project>,
    pub sessions: Vec<Session>,
    /// Oldest first.
    pub bell: Vec<BellItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub name: String,
    pub folder: String,
    /// Its workspaces by name; `main`, the project folder, is always there and not listed.
    pub workspaces: Vec<String>,
}

/// Where a session works: `None` for the project folder (`main`), else a workspace.
pub type Place = Option<String>;

/// A root agent and every agent it launched.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// The root agent's id.
    pub id: String,
    pub project: String,
    pub place: Place,
    pub title: String,
    /// `Running` or `NeedsYou`; `None` for a session at rest.
    pub mark: Option<AgentState>,
    /// The last sign of activity of its tree, in milliseconds since the Unix epoch.
    pub active_at: u64,
    pub created_at: u64,
}

/// One thing that waits for the user. Choosing it opens its session.
#[derive(Clone, Debug, PartialEq)]
pub struct BellItem {
    /// `request-<n>`.
    pub key: String,
    /// The root agent of the session it belongs to.
    pub session: String,
    pub label: String,
    pub detail: String,
    pub at: u64,
}

/// What the daemon answered, the raw results of each method.
pub struct Answers<'a> {
    /// `project.list`
    pub projects: &'a Value,
    /// `workspace.list`
    pub workspaces: &'a Value,
    /// `agent.list`
    pub agents: &'a Value,
    /// `approval.list`
    pub approvals: &'a Value,
    /// The roots whose `needs_user` the daemon says is true (`agent.trees`).
    pub needs_user: &'a HashSet<String>,
    /// Each root's first message, by id.
    pub briefs: &'a HashMap<String, String>,
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

fn number(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The title a session is shown with: the first line of its first message.
pub fn title_of(brief: &str) -> String {
    brief
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// The root of `id` in a tree of agents given by their parents.
fn root_of<'a>(parents: &HashMap<&'a str, Option<&'a str>>, mut id: &'a str) -> &'a str {
    // A tree is at most a few levels deep; the bound only guards against a loop in bad data.
    for _ in 0..64 {
        match parents.get(id) {
            Some(Some(parent)) => id = parent,
            _ => break,
        }
    }
    id
}

impl Snapshot {
    /// What waits for the user is the daemon's to say, as `flottille status` shows it:
    ///
    /// - a session is a root agent and its tree, in its root's place;
    /// - the bell lists every request the user holds (`approval.list`), which opens the session of
    ///   the root of the agent that asked;
    /// - a session needs the user (`NEEDS YOU`) when the daemon says its root `needs_user`;
    ///   otherwise it is running when an agent of its tree starts or works; otherwise it shows no
    ///   mark.
    pub fn build(answers: Answers) -> Snapshot {
        let agents = list(answers.agents, "agents");
        let parents: HashMap<&str, Option<&str>> = agents
            .iter()
            .map(|agent| {
                (
                    agent["id"].as_str().unwrap_or_default(),
                    agent["parent"].as_str(),
                )
            })
            .collect();

        let mut bell = Vec::new();
        for request in list(answers.approvals, "requests") {
            let asker = request["agent"].as_str().unwrap_or_default();
            let n = number(request, "number");
            bell.push(BellItem {
                key: format!("request-{n}"),
                session: root_of(&parents, asker).to_owned(),
                label: format!("Request #{n} · {}", text(request, "tool")),
                detail: list(request, "reasons")
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                at: number(request, "created_at"),
            });
        }
        bell.sort_by_key(|item| item.at);

        let mut trees: HashMap<&str, (bool, u64)> = HashMap::new();
        for agent in agents {
            let root = root_of(&parents, agent["id"].as_str().unwrap_or_default());
            let busy = matches!(agent["state"].as_str(), Some("starting" | "working"));
            let active = agent["active_at"]
                .as_u64()
                .unwrap_or_else(|| number(agent, "created_at"));
            let entry = trees.entry(root).or_default();
            entry.0 |= busy;
            entry.1 = entry.1.max(active);
        }
        let mut sessions: Vec<Session> = agents
            .iter()
            .filter(|agent| agent["parent"].is_null())
            .map(|agent| {
                let id = text(agent, "id");
                let (busy, active_at) = trees.get(id.as_str()).copied().unwrap_or_default();
                let mark = if answers.needs_user.contains(&id) {
                    Some(AgentState::NeedsYou)
                } else if busy {
                    Some(AgentState::Running)
                } else {
                    None
                };
                Session {
                    title: answers
                        .briefs
                        .get(&id)
                        .map(|brief| title_of(brief))
                        .filter(|title| !title.is_empty())
                        .unwrap_or_else(|| format!("agent {id}")),
                    project: text(agent, "project"),
                    place: agent["workspace"].as_str().map(str::to_owned),
                    mark,
                    active_at,
                    created_at: number(agent, "created_at"),
                    id,
                }
            })
            .collect();
        // Newest first within a place.
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));

        let mut projects: Vec<Project> = list(answers.projects, "projects")
            .iter()
            .map(|project| {
                let name = text(project, "name");
                let mut workspaces: Vec<String> = list(answers.workspaces, "workspaces")
                    .iter()
                    .filter(|w| w["project"].as_str() == Some(name.as_str()))
                    .map(|w| text(w, "name"))
                    .collect();
                workspaces.sort();
                Project {
                    folder: text(project, "folder"),
                    name,
                    workspaces,
                }
            })
            .collect();
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        Snapshot {
            projects,
            sessions,
            bell,
        }
    }

    /// The sessions of one place, newest first.
    pub fn sessions_in<'a>(
        &'a self,
        project: &'a str,
        place: &'a Place,
    ) -> impl Iterator<Item = &'a Session> + 'a {
        self.sessions
            .iter()
            .filter(move |s| s.project == project && &s.place == place)
    }

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }
}

/// How long ago, said short: `now`, `4m`, `1h`, `2d`, `3w`.
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    let s = now_ms.saturating_sub(then_ms) / 1000;
    match s {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 => format!("{}h", s / 3_600),
        86_400..1_209_600 => format!("{}d", s / 86_400),
        _ => format!("{}w", s / 604_800),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent(id: &str, parent: Option<&str>, state: &str, workspace: Option<&str>) -> Value {
        json!({"id": id, "parent": parent, "state": state, "project": "app",
            "workspace": workspace, "created_at": 10, "active_at": 20})
    }

    fn build(agents: Value, approvals: Value, needs_user: &[&str]) -> Snapshot {
        Snapshot::build(Answers {
            projects: &json!({"projects": [{"name": "app", "folder": "/work/app"}]}),
            workspaces: &json!({"workspaces": [{"project": "app", "name": "login"}]}),
            agents: &agents,
            approvals: &approvals,
            needs_user: &needs_user.iter().map(|id| id.to_string()).collect(),
            briefs: &HashMap::from([("r1".into(), "\n  Fix the login\nmore".into())]),
        })
    }

    #[test]
    fn a_request_of_a_child_marks_its_root_and_rings_the_bell() {
        let snapshot = build(
            json!({"agents": [agent("r1", None, "idle", Some("login")),
                agent("c1", Some("r1"), "idle", Some("login"))]}),
            json!({"requests": [{"number": 3, "agent": "c1", "tool": "fs_read",
                "reasons": [".env is a sensitive place (.env)"], "created_at": 30}]}),
            &["r1"],
        );
        assert_eq!(snapshot.sessions.len(), 1, "a child is not a session");
        assert_eq!(snapshot.sessions[0].mark, Some(AgentState::NeedsYou));
        assert_eq!(snapshot.sessions[0].title, "Fix the login");
        assert_eq!(snapshot.bell.len(), 1);
        assert_eq!(snapshot.bell[0].session, "r1");
        assert_eq!(snapshot.bell[0].label, "Request #3 · fs_read");
    }

    #[test]
    fn a_working_child_makes_its_session_run() {
        let snapshot = build(
            json!({"agents": [agent("r1", None, "idle", None),
                agent("c1", Some("r1"), "working", None)]}),
            json!({"requests": []}),
            &[],
        );
        assert_eq!(snapshot.sessions[0].mark, Some(AgentState::Running));
        assert_eq!(snapshot.sessions[0].place, None);
    }

    #[test]
    fn a_session_needs_the_user_when_the_daemon_says_its_root_does() {
        let agents = json!({"agents": [agent("r1", None, "idle", None)]});
        let said = build(agents.clone(), json!({"requests": []}), &["r1"]);
        assert_eq!(said.sessions[0].mark, Some(AgentState::NeedsYou));
        assert!(said.bell.is_empty(), "the bell lists the requests only");
        let not_said = build(agents, json!({"requests": []}), &[]);
        assert_eq!(not_said.sessions[0].mark, None);
    }

    #[test]
    fn a_root_without_a_brief_is_named_by_its_id() {
        let snapshot = build(
            json!({"agents": [agent("r2", None, "idle", None)]}),
            json!({"requests": []}),
            &[],
        );
        assert_eq!(snapshot.sessions[0].title, "agent r2");
        assert_eq!(snapshot.projects[0].workspaces, ["login"]);
    }

    #[test]
    fn a_time_ago_is_short() {
        let now = 100 * 86_400_000;
        assert_eq!(ago(now, now - 20_000), "now");
        assert_eq!(ago(now, now - 4 * 60_000), "4m");
        assert_eq!(ago(now, now - 3_600_000), "1h");
        assert_eq!(ago(now, now - 2 * 86_400_000), "2d");
        assert_eq!(ago(now, now - 21 * 86_400_000), "3w");
    }
}
