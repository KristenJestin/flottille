//! What the window shows, made from the daemon's answers: projects, their places, the sessions in
//! each place, and what waits for the user. Nothing here talks to the socket.

use std::collections::{HashMap, HashSet};

use flottille_core::workspace::{StepState, Workspace as Record};
use flottille_ui::AgentState;
use serde_json::Value;

/// Everything the shell draws, as one fresh read of the daemon gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// By name.
    pub projects: Vec<Project>,
    pub sessions: Vec<Session>,
    /// By project, then name.
    pub workspaces: Vec<Workspace>,
    /// Oldest first.
    pub bell: Vec<BellItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub name: String,
    pub folder: String,
    /// Its repositories' paths, `.` for the folder itself.
    pub repositories: Vec<String>,
    /// The repositories a workspace takes by default; all of them when none.
    pub default_repositories: Option<Vec<String>>,
    /// Its workspaces by name; `main`, the project folder, is always there and not listed.
    pub workspaces: Vec<String>,
    /// The provider and the model its file gives its agents, when it gives them.
    pub provider: Option<String>,
    pub model: Option<String>,
    /// How many commands its agents run now: the tree shows an icon while there is one.
    pub commands: usize,
}

/// A workspace as its card shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub project: String,
    pub name: String,
    pub branch: String,
    /// Its folder, opened in the user's editor.
    pub folder: String,
    /// The base branches its repositories started from, each once, in order.
    pub bases: Vec<String>,
    pub repositories: Vec<String>,
    pub preparation: Preparation,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Preparation {
    /// `done` of `total` steps are over, and what the next one does.
    Preparing {
        done: usize,
        total: usize,
        doing: String,
    },
    /// What the failed step was doing, and its last lines, masked by the daemon.
    Failed {
        doing: String,
        output: String,
    },
    Ready,
}

impl Workspace {
    fn of(record: &Record) -> Workspace {
        let mut bases: Vec<String> = Vec::new();
        for worktree in &record.repositories {
            if !bases.contains(&worktree.base.branch) {
                bases.push(worktree.base.branch.clone());
            }
        }
        let failed = record
            .steps
            .iter()
            .find(|step| step.state == StepState::Failed);
        let next = record
            .steps
            .iter()
            .find(|step| matches!(step.state, StepState::Pending | StepState::Running));
        let preparation = match (failed, next) {
            (Some(step), _) => Preparation::Failed {
                doing: step
                    .failure
                    .as_ref()
                    .map_or_else(|| step.doing(), |failure| failure.doing.clone()),
                output: step
                    .failure
                    .as_ref()
                    .map(|failure| failure.output.clone())
                    .unwrap_or_default(),
            },
            (None, Some(step)) => Preparation::Preparing {
                done: record
                    .steps
                    .iter()
                    .filter(|step| matches!(step.state, StepState::Done | StepState::Skipped))
                    .count(),
                total: record.steps.len(),
                doing: step.doing(),
            },
            (None, None) => Preparation::Ready,
        };
        Workspace {
            project: record.project.clone(),
            name: record.name.clone(),
            branch: record.branch.clone(),
            folder: record.folder.display().to_string(),
            bases,
            repositories: record
                .repositories
                .iter()
                .map(|worktree| worktree.path.clone())
                .collect(),
            preparation,
        }
    }
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
    /// How many agents its tree holds, the root included.
    pub agents: usize,
    /// Its root ended or failed: the left tree lists it in its project's "Ended" group.
    pub ended: bool,
    /// The last sign of activity of its tree, in milliseconds since the Unix epoch.
    pub active_at: u64,
    pub created_at: u64,
    /// How many commands the agents of its tree run now.
    pub commands: usize,
    /// The agent that started it for the user, by the name it goes by: `None` for a session the
    /// user started. Its title stays the first line of its brief.
    pub started_by: Option<String>,
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
    /// The question each root asking the user asked last, by id: its text and when.
    pub questions: &'a HashMap<String, (String, u64)>,
    /// `run.list`: the commands running.
    pub commands: &'a Value,
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

/// A root of `agent.list` whose question waits for the user's answer, as the daemon keeps it.
pub fn asks_the_user(agent: &Value) -> bool {
    agent["parent"].is_null()
        && agent["waiting_on_answer"] == true
        && !matches!(agent["state"].as_str(), Some("ended" | "failed"))
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
    ///   the root of the agent that asked, and every root's question waiting for the user's answer
    ///   (`waiting_on_answer`), which opens its session;
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
        for root in agents.iter().filter(|agent| asks_the_user(agent)) {
            let id = text(root, "id");
            let (question, at) = answers.questions.get(&id).cloned().unwrap_or_default();
            let title = title_of(answers.briefs.get(&id).map_or("", String::as_str));
            bell.push(BellItem {
                key: format!("question-{id}"),
                label: format!("Question · {title}"),
                detail: question,
                at,
                session: id,
            });
        }
        bell.sort_by_key(|item| item.at);

        let mut trees: HashMap<&str, (bool, u64, usize)> = HashMap::new();
        for agent in agents {
            let root = root_of(&parents, agent["id"].as_str().unwrap_or_default());
            let busy = matches!(agent["state"].as_str(), Some("starting" | "working"));
            let active = agent["active_at"]
                .as_u64()
                .unwrap_or_else(|| number(agent, "created_at"));
            let entry = trees.entry(root).or_default();
            entry.0 |= busy;
            entry.1 = entry.1.max(active);
            entry.2 += 1;
        }
        let running = list(answers.commands, "commands");
        let commands_of = |key: &str, name: &str| {
            running
                .iter()
                .filter(|command| command[key].as_str() == Some(name))
                .count()
        };
        let name_of = |id: &str| {
            let agent = agents.iter().find(|a| a["id"].as_str() == Some(id));
            agent
                .and_then(|a| a["title"].as_str())
                .map(str::to_owned)
                .or_else(|| answers.briefs.get(id).map(|brief| title_of(brief)))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("agent {id}"))
        };
        let mut sessions: Vec<Session> = agents
            .iter()
            .filter(|agent| agent["parent"].is_null())
            .map(|agent| {
                let id = text(agent, "id");
                let (busy, active_at, size) = trees.get(id.as_str()).copied().unwrap_or_default();
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
                    agents: size.max(1),
                    ended: matches!(agent["state"].as_str(), Some("ended" | "failed")),
                    active_at,
                    created_at: number(agent, "created_at"),
                    commands: commands_of("root", &id),
                    started_by: agent["launched_by"].as_str().map(name_of),
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
                let commands = commands_of("project", &name);
                Project {
                    folder: text(project, "folder"),
                    repositories: list(project, "repositories")
                        .iter()
                        .map(|repository| text(repository, "path"))
                        .collect(),
                    default_repositories: project["default_repositories"].as_array().map(|set| {
                        set.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    }),
                    name,
                    workspaces,
                    provider: project["provider"].as_str().map(str::to_owned),
                    model: project["model"].as_str().map(str::to_owned),
                    commands,
                }
            })
            .collect();
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        let mut workspaces: Vec<Workspace> = list(answers.workspaces, "workspaces")
            .iter()
            .filter_map(|w| serde_json::from_value::<Record>(w.clone()).ok())
            .map(|record| Workspace::of(&record))
            .collect();
        workspaces.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
        Snapshot {
            projects,
            sessions,
            workspaces,
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

    pub fn workspace(&self, project: &str, name: &str) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|w| w.project == project && w.name == name)
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
            questions: &HashMap::new(),
            briefs: &HashMap::from([("r1".into(), "\n  Fix the login\nmore".into())]),
            commands: &json!({"commands": []}),
        })
    }

    #[test]
    fn a_session_whose_root_ended_or_failed_is_ended() {
        let snapshot = build(
            json!({"agents": [agent("r1", None, "ended", Some("login")),
                agent("r2", None, "failed", None), agent("r3", None, "idle", None),
                agent("c1", Some("r3"), "ended", None)]}),
            json!({"requests": []}),
            &[],
        );
        let ended: Vec<(&str, bool)> = snapshot
            .sessions
            .iter()
            .map(|s| (s.id.as_str(), s.ended))
            .collect();
        assert_eq!(ended, [("r1", true), ("r2", true), ("r3", false)]);
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
    fn a_session_an_agent_started_keeps_its_title_and_names_the_agent_that_started_it() {
        let mut started = agent("r2", None, "idle", None);
        started["launched_by"] = json!("r1");
        let mut child = agent("c1", Some("r1"), "idle", None);
        child["title"] = json!("planner");
        let mut by_child = agent("r3", None, "idle", None);
        by_child["launched_by"] = json!("c1");
        let snapshot = Snapshot::build(Answers {
            projects: &json!({"projects": []}),
            workspaces: &json!({"workspaces": []}),
            agents: &json!({"agents": [agent("r1", None, "idle", None), started, child, by_child]}),
            approvals: &json!({"requests": []}),
            needs_user: &HashSet::new(),
            questions: &HashMap::new(),
            briefs: &HashMap::from([
                ("r1".into(), "Lead the work".into()),
                ("r2".into(), "Write the docs\nwith care".into()),
            ]),
            commands: &json!({"commands": []}),
        });
        let by = |id: &str| snapshot.session(id).unwrap().started_by.clone();
        assert_eq!(snapshot.session("r2").unwrap().title, "Write the docs");
        assert_eq!(by("r2").as_deref(), Some("Lead the work"));
        assert_eq!(by("r3").as_deref(), Some("planner"));
        assert_eq!(by("r1"), None, "a session the user started names nobody");
    }

    fn workspace_json(steps: Value) -> Value {
        json!({"project": "app", "name": "login", "branch": "app/login",
            "folder": "/work/app-work/login", "project_folder": "/work/app", "created_at": 1,
            "variables": {}, "state": "preparing",
            "repositories": [
                {"path": "api", "base": {"branch": "develop", "remote": "origin",
                    "ref": "refs/remotes/origin/develop", "commit": "abc", "fetched_now": true,
                    "fetched_at": null}},
                {"path": "web", "base": {"branch": "develop", "remote": "origin",
                    "ref": "refs/remotes/origin/develop", "commit": "def", "fetched_now": true,
                    "fetched_at": null}}],
            "steps": steps})
    }

    fn preparation_of(steps: Value) -> Workspace {
        let snapshot = Snapshot::build(Answers {
            projects: &json!({"projects": []}),
            workspaces: &json!({"workspaces": [workspace_json(steps)]}),
            agents: &json!({"agents": []}),
            approvals: &json!({"requests": []}),
            needs_user: &HashSet::new(),
            questions: &HashMap::new(),
            briefs: &HashMap::new(),
            commands: &json!({"commands": []}),
        });
        snapshot.workspaces[0].clone()
    }

    #[test]
    fn a_workspace_says_how_far_its_preparation_is_or_what_failed() {
        let preparing = preparation_of(json!([
            {"position": 0, "repository": "api", "kind": "worktree", "state": "done"},
            {"position": 1, "repository": "api", "kind": "run", "line": "npm install",
                "time_limit": 60, "state": "running"}]));
        assert_eq!(preparing.bases, ["develop"], "each base once");
        assert_eq!(preparing.repositories, ["api", "web"]);
        assert_eq!(
            preparing.preparation,
            Preparation::Preparing {
                done: 1,
                total: 2,
                doing: "npm install (in api)".into()
            }
        );
        let failed = preparation_of(json!([
            {"position": 0, "repository": "api", "kind": "run", "line": "npm install",
                "time_limit": 60, "state": "failed",
                "failure": {"doing": "npm install (in api)", "output": "npm ERR! made up"}}]));
        assert_eq!(
            failed.preparation,
            Preparation::Failed {
                doing: "npm install (in api)".into(),
                output: "npm ERR! made up".into()
            }
        );
    }

    #[test]
    fn a_project_and_a_session_count_the_commands_running_in_them() {
        let agents = json!({"agents": [agent("r1", None, "idle", Some("login")),
            agent("c1", Some("r1"), "idle", Some("login")), agent("r2", None, "idle", None)]});
        let commands = json!({"commands": [
            {"run": "a", "agent": "c1", "root": "r1", "project": "app"},
            {"run": "b", "agent": "r1", "root": "r1", "project": "app"},
            {"run": "c", "agent": "r2", "root": "r2", "project": "app"}]});

        let snapshot = Snapshot::build(Answers {
            projects: &json!({"projects": [{"name": "app", "folder": "/work/app"},
                {"name": "other", "folder": "/work/other"}]}),
            workspaces: &json!({"workspaces": []}),
            agents: &agents,
            approvals: &json!({"requests": []}),
            needs_user: &HashSet::new(),
            questions: &HashMap::new(),
            briefs: &HashMap::new(),
            commands: &commands,
        });

        let of = |id: &str| snapshot.session(id).unwrap().commands;
        assert_eq!(
            (of("r1"), of("r2")),
            (2, 1),
            "a child's command is its root's"
        );
        let project = |name: &str| {
            snapshot
                .projects
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .commands
        };
        assert_eq!((project("app"), project("other")), (3, 0));
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
