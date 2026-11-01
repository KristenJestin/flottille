//! What the window shows, made from the daemon's answers: projects, their places, the sessions in
//! each place, and what waits for the user. Nothing here talks to the socket.

use std::collections::{HashMap, HashSet};

use flottille_core::workspace::{Action, Step, StepState, Workspace as Record};
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
    /// How many `project.changed` events the link has seen: a project's file was edited when it
    /// grows.
    pub project_changes: u64,
    /// The root of each agent's tree, by the agent's id: the session it belongs to.
    pub roots: HashMap<String, String>,
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
    /// Each step of its preparation, in order.
    pub steps: Vec<SetupLine>,
}

/// One step of a workspace's preparation, as its card lists it: "Run `pnpm install`".
#[derive(Clone, Debug, PartialEq)]
pub struct SetupLine {
    /// Its place in the preparation: the same step keeps it while the list changes.
    pub position: u32,
    pub verb: String,
    /// Whether it runs a command: only such a step has an output to open.
    pub runs: bool,
    /// What it acts on, in the mono face.
    pub object: Option<String>,
    pub state: StepState,
    /// Milliseconds since the Unix epoch.
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
}

impl SetupLine {
    fn of(step: &Step) -> SetupLine {
        let place = step.repository.clone().unwrap_or_else(|| ".".into());
        let (verb, object) = match &step.action {
            Action::Worktree => ("Check out", Some(place)),
            Action::Copy { globs } => ("Copy", Some(globs.join(" "))),
            Action::Link { path } => ("Link", Some(path.clone())),
            Action::RootEntries { .. } => ("Link the project folder's entries", None),
            Action::Run {
                task: Some(task), ..
            } => ("Run task", Some(task.clone())),
            Action::Run { line, .. } => ("Run", Some(line.clone())),
        };
        SetupLine {
            position: step.position,
            verb: verb.to_owned(),
            runs: matches!(step.action, Action::Run { .. }),
            object,
            state: step.state,
            started_at: step.started_at,
            ended_at: step.ended_at,
        }
    }
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
            // A checkout done is not part of the setup the project declares: it leaves the list.
            steps: record
                .steps
                .iter()
                .filter(|step| {
                    !matches!(step.action, Action::Worktree)
                        || !matches!(step.state, StepState::Done | StepState::Skipped)
                })
                .map(SetupLine::of)
                .collect(),
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
    /// `Running`, `NeedsYou`, or `Waiting` (between turns, for what happens outside); `None` for
    /// a session at rest.
    pub mark: Option<AgentState>,
    /// What its mark means, in words, for its tooltip: `Needs you: a permission`, `Running`,
    /// `Waiting: checks of #1`; empty without a mark.
    pub mark_said: String,
    /// How many agents its tree holds, the root included.
    pub agents: usize,
    /// How many of them have not ended: what ending the session ends.
    pub live: usize,
    /// Its root can be started again on its own session (`agent.resume`).
    pub resumable: bool,
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

/// A ticket or a pull request an agent created or changed: what `agent.tool_served` says of a
/// write of `forge` or `tickets` that was done. The session's row and the bell say it until the
/// session is opened.
#[derive(Clone, Debug, PartialEq)]
pub struct Forged {
    /// The event's number.
    pub seq: u64,
    pub agent: String,
    /// A ticket's, rather than a pull request's.
    pub ticket: bool,
    /// What was done, in words: `Opened a pull request`.
    pub said: String,
    /// A short excerpt of what it posted: the title, the start of the comment.
    pub excerpt: String,
    pub at: u64,
}

/// The longest an excerpt of a ticket's or a pull request's text is kept, on one line.
const EXCERPT: usize = 120;

impl Forged {
    /// What an event says was done to a ticket or a pull request, when it says it: a read, a
    /// call refused, failed or held, and the re-run of a pull request's checks are none.
    pub fn of(event: &flottille_core::protocol::Event) -> Option<Forged> {
        let data = &event.data;
        if event.name != "agent.tool_served" || data["outcome"] != "ok" {
            return None;
        }
        let arguments = &data["arguments"];
        let field = |name: &str| arguments[name].as_str().unwrap_or_default();
        let number = arguments["number"]
            .as_u64()
            .map(|n| format!("#{n}"))
            .unwrap_or_default();
        let ticket = match field("ticket") {
            "" => "its ticket",
            key => key,
        };
        // What it acted on, then the words it posted when it did.
        let on = |what: &str, words: &[&str]| {
            std::iter::once(what)
                .chain(words.iter().copied().find(|w| !w.trim().is_empty()))
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(": ")
        };
        let tool = data["tool"].as_str()?;
        let (said, excerpt) = match (tool, field("action")) {
            ("forge", "create") => ("Opened a pull request", field("title").to_owned()),
            ("forge", "merge") => ("Merged a pull request", field("repository").to_owned()),
            ("forge", "comment") => ("Commented on a pull request", on(&number, &[field("body")])),
            ("forge", "edit") => (
                "Edited a pull request",
                on(&number, &[field("title"), field("body")]),
            ),
            ("tickets", "create") => ("Created a ticket", field("title").to_owned()),
            ("tickets", "comment") => ("Commented on a ticket", on(ticket, &[field("body")])),
            ("tickets", "edit") => (
                "Edited a ticket",
                on(ticket, &[field("title"), field("body")]),
            ),
            ("tickets", "close") => ("Closed a ticket", ticket.to_owned()),
            ("tickets", "reopen") => ("Reopened a ticket", ticket.to_owned()),
            ("tickets", "status") => ("Moved a ticket", on(ticket, &[field("status")])),
            _ => return None,
        };
        let excerpt = excerpt.split_whitespace().collect::<Vec<_>>().join(" ");
        let excerpt = match excerpt.char_indices().nth(EXCERPT) {
            Some((cut, _)) => format!("{}…", &excerpt[..cut]),
            None => excerpt,
        };
        Some(Forged {
            seq: event.seq,
            agent: data["agent"].as_str()?.to_owned(),
            ticket: tool == "tickets",
            said: said.to_owned(),
            excerpt,
            at: event.at,
        })
    }
}

/// Something new in a session that is no ticket nor pull request: an agent said something,
/// reported, or was woken by what it waited for. The session's row says it until it is opened.
#[derive(Clone, Debug, PartialEq)]
pub struct News {
    pub agent: String,
    /// In words, as the row's tooltip finishes `New since you looked:`.
    pub said: &'static str,
}

impl News {
    /// An agent said something: the weakest news.
    pub const MESSAGE: &'static str = "a new message";

    pub fn of(event: &flottille_core::protocol::Event) -> Option<News> {
        let data = &event.data;
        let said = match event.name.as_str() {
            // The history a load replays was said before.
            "agent.message" if data["replay"] != true => Self::MESSAGE,
            "agent.reported" => "a report",
            "delivery.queued" if data["kind"] == "subscription" => "a subscription woke the agent",
            _ => return None,
        };
        Some(News {
            agent: data["agent"].as_str()?.to_owned(),
            said,
        })
    }
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
    /// `subscription.list`: what the agents wait for outside Flottille.
    pub subscriptions: &'a Value,
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

/// What a line of greeting is made of: a line of these words alone says no work.
const GREETING: [&str; 17] = [
    "hello",
    "hi",
    "hey",
    "hiya",
    "howdy",
    "yo",
    "good",
    "morning",
    "afternoon",
    "evening",
    "there",
    "everyone",
    "all",
    "bonjour",
    "salut",
    "hola",
    "greetings",
];

/// Whether a line says what the work is: not a greeting, not a single word.
fn names_work(line: &str) -> bool {
    let words: Vec<String> = line
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();
    words.len() > 1 && !words.iter().all(|word| GREETING.contains(&word.as_str()))
}

/// The title a session is shown with: the first line of its first message that says what the work
/// is. A greeting or a very short message gives none (`""`).
pub fn title_of(brief: &str) -> String {
    brief
        .lines()
        .map(str::trim)
        .find(|line| names_work(line))
        .unwrap_or_default()
        .to_owned()
}

/// The title of the session of root `id`, from its first message; the neutral `agent <id>` while
/// none says what the work is.
fn session_title(id: &str, brief: Option<&String>) -> String {
    brief
        .map(|brief| title_of(brief))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| format!("agent {id}"))
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

/// Whether an agent or one of its parents ended.
fn under_an_ended<'a>(
    parents: &HashMap<&'a str, Option<&'a str>>,
    ended: &HashSet<&str>,
    mut id: &'a str,
) -> bool {
    for _ in 0..64 {
        if ended.contains(id) {
            return true;
        }
        match parents.get(id) {
            Some(Some(parent)) => id = parent,
            _ => break,
        }
    }
    false
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
                label: format!("Request #{n} waits for you"),
                detail: crate::session_link::Request::of(request).asked(),
                at: number(request, "created_at"),
            });
        }
        for root in agents.iter().filter(|agent| asks_the_user(agent)) {
            let id = text(root, "id");
            let (question, at) = answers.questions.get(&id).cloned().unwrap_or_default();
            bell.push(BellItem {
                key: format!("question-{id}"),
                label: "Asks you a question".into(),
                detail: question,
                at,
                session: id,
            });
        }
        bell.sort_by_key(|item| item.at);

        let ended = |agent: &Value| matches!(agent["state"].as_str(), Some("ended" | "failed"));
        let final_ids: HashSet<&str> = agents
            .iter()
            .filter(|agent| ended(agent))
            .filter_map(|agent| agent["id"].as_str())
            .collect();
        let mut trees: HashMap<&str, (bool, u64, usize, usize)> = HashMap::new();
        for agent in agents {
            let id = agent["id"].as_str().unwrap_or_default();
            let root = root_of(&parents, id);
            // Nothing runs under an agent that ended: whatever its record still says.
            let busy = matches!(agent["state"].as_str(), Some("starting" | "working"))
                && !under_an_ended(&parents, &final_ids, id);
            let active = agent["active_at"]
                .as_u64()
                .unwrap_or_else(|| number(agent, "created_at"));
            let entry = trees.entry(root).or_default();
            entry.0 |= busy;
            entry.1 = entry.1.max(active);
            entry.2 += 1;
            entry.3 += usize::from(!ended(agent));
        }
        let running = list(answers.commands, "commands");
        let commands_of = |key: &str, name: &str| {
            running
                .iter()
                .filter(|command| command[key].as_str() == Some(name))
                .count()
        };
        // The roots whose agents wait, between turns, for what happens outside.
        let waiting: HashSet<&str> = list(answers.subscriptions, "subscriptions")
            .iter()
            .filter_map(|subscription| subscription["agent"].as_str())
            .map(|agent| root_of(&parents, agent))
            .collect();
        // What each of those roots waits for, in a few words: the first of its subscriptions.
        let mut waiting_for: HashMap<&str, String> = HashMap::new();
        for subscription in list(answers.subscriptions, "subscriptions") {
            let Some(agent) = subscription["agent"].as_str() else {
                continue;
            };
            let events: Vec<&str> = list(subscription, "events")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let number = subscription["target"]["number"].as_u64();
            waiting_for.entry(root_of(&parents, agent)).or_insert(
                match (events.is_empty(), number) {
                    (false, Some(n)) => format!("Waiting: {} of #{n}", events.join(", ")),
                    (true, Some(n)) => format!("Waiting: #{n}"),
                    _ => "Waiting".to_owned(),
                },
            );
        }
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
                let (busy, active_at, size, live) =
                    trees.get(id.as_str()).copied().unwrap_or_default();
                let mark = if ended(agent) {
                    None
                } else if answers.needs_user.contains(&id) {
                    Some(AgentState::NeedsYou)
                } else if busy {
                    Some(AgentState::Running)
                } else if waiting.contains(id.as_str()) {
                    Some(AgentState::Waiting)
                } else {
                    None
                };
                let mark_said = match mark {
                    None => String::new(),
                    Some(AgentState::Running) => "Running".to_owned(),
                    Some(AgentState::Waiting) => waiting_for
                        .get(id.as_str())
                        .cloned()
                        .unwrap_or_else(|| "Waiting".to_owned()),
                    Some(_) => match bell.iter().find(|item| item.session == id) {
                        Some(item) if item.key.starts_with("request-") => {
                            "Needs you: a permission".to_owned()
                        }
                        Some(_) => "Needs you: a question".to_owned(),
                        None => "Needs you".to_owned(),
                    },
                };
                Session {
                    title: session_title(&id, answers.briefs.get(&id)),
                    project: text(agent, "project"),
                    place: agent["workspace"].as_str().map(str::to_owned),
                    mark,
                    mark_said,
                    agents: size.max(1),
                    live,
                    resumable: agent["resumable"] == true,
                    ended: ended(agent),
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
            project_changes: 0,
            roots: parents
                .keys()
                .map(|id| ((*id).to_owned(), root_of(&parents, id).to_owned()))
                .collect(),
        }
    }

    /// The session an agent belongs to: its tree's root, itself when it is not known.
    pub fn root_of(&self, agent: &str) -> String {
        self.roots
            .get(agent)
            .cloned()
            .unwrap_or_else(|| agent.to_owned())
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
            subscriptions: &json!({"subscriptions": []}),
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
                "call": {"path": ".env"},
                "reasons": [".env is a sensitive place (.env)"], "created_at": 30}]}),
            &["r1"],
        );
        assert_eq!(snapshot.sessions.len(), 1, "a child is not a session");
        assert_eq!(snapshot.sessions[0].mark, Some(AgentState::NeedsYou));
        assert_eq!(snapshot.sessions[0].title, "Fix the login");
        assert_eq!(snapshot.bell.len(), 1);
        assert_eq!(snapshot.bell[0].session, "r1");
        assert_eq!(snapshot.bell[0].label, "Request #3 waits for you");
        assert_eq!(snapshot.bell[0].detail, "Read .env");
        assert_eq!(snapshot.root_of("c1"), "r1");
    }

    #[test]
    fn a_held_web_call_says_the_url_it_reads_or_the_words_it_searches() {
        let asked = |call: Value| {
            crate::session_link::Request::of(&json!({"number": 1, "agent": "r1", "tool": "web",
                "call": call, "reasons": [], "created_at": 1}))
            .asked()
        };
        assert_eq!(
            asked(json!({"action": "read", "url": "https://docs.example/a?d=1"})),
            "Read https://docs.example/a?d=1"
        );
        assert_eq!(
            asked(json!({"action": "search", "query": "tokio select"})),
            "Search the web for tokio select"
        );
    }

    fn served(tool: &str, arguments: Value, outcome: &str) -> flottille_core::protocol::Event {
        flottille_core::protocol::Event {
            seq: 9,
            at: 90,
            name: "agent.tool_served".into(),
            data: json!({"agent": "c1", "tool": tool, "arguments": arguments,
                "outcome": outcome}),
        }
    }

    #[test]
    fn a_ticket_or_a_pull_request_written_is_said_with_an_excerpt() {
        let said = |tool: &str, arguments: Value| {
            Forged::of(&served(tool, arguments, "ok")).map(|f| (f.said, f.excerpt, f.ticket))
        };
        assert_eq!(
            said(
                "forge",
                json!({"action": "create", "title": "Add the login page"})
            ),
            Some((
                "Opened a pull request".into(),
                "Add the login page".into(),
                false
            ))
        );
        assert_eq!(
            said(
                "forge",
                json!({"action": "comment", "number": 12, "body": "Fixed\n  the  test"})
            ),
            Some((
                "Commented on a pull request".into(),
                "#12: Fixed the test".into(),
                false
            ))
        );
        assert_eq!(
            said("tickets", json!({"action": "close"})),
            Some(("Closed a ticket".into(), "its ticket".into(), true))
        );
        assert_eq!(
            said(
                "tickets",
                json!({"action": "status", "ticket": "ABC-1", "status": "In Progress"})
            ),
            Some(("Moved a ticket".into(), "ABC-1: In Progress".into(), true))
        );
        assert_eq!(
            said(
                "tickets",
                json!({"action": "comment", "ticket": "#4", "body": ""})
            ),
            Some(("Commented on a ticket".into(), "#4".into(), true))
        );
        let long = "word ".repeat(60);
        let (_, excerpt, _) = said("tickets", json!({"action": "create", "title": long})).unwrap();
        assert!(
            excerpt.ends_with('…') && excerpt.chars().count() == 121,
            "{excerpt}"
        );
    }

    #[test]
    fn a_read_a_held_call_and_a_re_run_of_checks_are_no_change() {
        let none = |tool: &str, action: &str, outcome: &str| {
            Forged::of(&served(tool, json!({"action": action}), outcome)).is_none()
        };
        assert!(none("forge", "show", "ok"));
        assert!(none("tickets", "list", "ok"));
        assert!(none("forge", "create", "held"));
        assert!(none("forge", "create", "refused"));
        assert!(none("forge", "checks", "ok"));
        assert!(none("fs_write", "", "ok"));
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
    fn a_child_that_ended_after_reporting_does_not_make_its_session_run() {
        let mut child = agent("c1", Some("r1"), "ended", None);
        child["state_reason"] = json!("it reported");
        let snapshot = build(
            json!({"agents": [agent("r1", None, "idle", None), child]}),
            json!({"requests": []}),
            &[],
        );
        assert_eq!(snapshot.sessions[0].mark, None);
    }

    #[test]
    fn an_ended_session_has_no_mark_whatever_its_tree_still_says() {
        let snapshot = build(
            json!({"agents": [agent("r1", None, "ended", None),
                agent("c1", Some("r1"), "working", None)]}),
            json!({"requests": []}),
            &["r1"],
        );
        assert!(snapshot.sessions[0].ended);
        assert_eq!(snapshot.sessions[0].mark, None);
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
    fn a_greeting_or_a_very_short_message_is_no_title() {
        for brief in [
            "Hello",
            "hello!",
            "Hi there,",
            "Hey",
            "Good morning",
            "ok",
            "  \n",
            "",
        ] {
            assert_eq!(title_of(brief), "", "{brief:?}");
        }
        assert_eq!(
            title_of("Hello\n\nFix the login form"),
            "Fix the login form"
        );
        assert_eq!(title_of("Fix the login form"), "Fix the login form");
        assert_eq!(title_of("Hello world app"), "Hello world app");
    }

    #[test]
    fn a_root_started_with_a_greeting_is_named_by_its_id_as_one_without_a_brief_is() {
        let snapshot = Snapshot::build(Answers {
            projects: &json!({"projects": []}),
            workspaces: &json!({"workspaces": []}),
            agents: &json!({"agents": [agent("r2", None, "idle", None)]}),
            approvals: &json!({"requests": []}),
            needs_user: &HashSet::new(),
            questions: &HashMap::new(),
            briefs: &HashMap::from([("r2".into(), "Hello".into())]),
            commands: &json!({"commands": []}),
            subscriptions: &json!({"subscriptions": []}),
        });
        assert_eq!(snapshot.sessions[0].title, "agent r2");
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
            subscriptions: &json!({"subscriptions": []}),
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
            subscriptions: &json!({"subscriptions": []}),
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
    fn a_workspace_lists_its_setup_steps_with_their_states_and_times() {
        let workspace = preparation_of(json!([
            {"position": 0, "repository": "api", "kind": "worktree", "state": "done",
                "started_at": 1_000, "ended_at": 1_400},
            {"position": 1, "kind": "copy", "globs": [".env", ".env.*"], "state": "done",
                "started_at": 1_400, "ended_at": 1_500},
            {"position": 2, "repository": "api", "kind": "run", "line": "pnpm install",
                "time_limit": 60, "state": "running", "started_at": 1_500},
            {"position": 3, "repository": "api", "kind": "run", "line": "pnpm build",
                "task": "build", "time_limit": 60, "state": "pending"}]));
        let said: Vec<_> = workspace
            .steps
            .iter()
            .map(|step| (step.verb.as_str(), step.object.as_deref(), step.state))
            .collect();
        assert_eq!(
            said,
            // The checkouts, once done, are not the setup the project declares.
            [
                ("Copy", Some(".env .env.*"), StepState::Done),
                ("Run", Some("pnpm install"), StepState::Running),
                ("Run task", Some("build"), StepState::Pending),
            ]
        );
        assert_eq!(
            (workspace.steps[0].started_at, workspace.steps[0].ended_at),
            (Some(1_400), Some(1_500))
        );
        assert_eq!(workspace.steps[1].ended_at, None);
        assert_eq!(workspace.steps[2].started_at, None);
    }

    #[test]
    fn a_session_whose_agents_wait_on_a_subscription_is_waiting_unless_it_runs_or_needs_the_user() {
        let agents = json!({"agents": [agent("r1", None, "idle", None),
            agent("c1", Some("r1"), "idle", None), agent("r2", None, "working", None),
            agent("r3", None, "idle", None)]});
        let subscriptions = json!({"subscriptions": [{"agent": "c1"}, {"agent": "r2"}]});
        let build = |needs_user: &HashSet<String>| {
            Snapshot::build(Answers {
                projects: &json!({"projects": [{"name": "app", "folder": "/work/app"}]}),
                workspaces: &json!({"workspaces": []}),
                agents: &agents,
                approvals: &json!({"requests": []}),
                needs_user,
                questions: &HashMap::new(),
                briefs: &HashMap::new(),
                commands: &json!({"commands": []}),
                subscriptions: &subscriptions,
            })
        };

        let snapshot = build(&HashSet::new());
        let mark = |id: &str| snapshot.session(id).unwrap().mark;
        assert_eq!(mark("r1"), Some(AgentState::Waiting), "a child's counts");
        assert_eq!(
            mark("r2"),
            Some(AgentState::Running),
            "a turn that runs wins"
        );
        assert_eq!(mark("r3"), None, "nothing is awaited");
        let asking = build(&HashSet::from(["r1".to_owned()]));
        assert_eq!(
            asking.session("r1").unwrap().mark,
            Some(AgentState::NeedsYou)
        );
    }

    #[test]
    fn a_session_s_mark_says_in_words_why_it_is_there() {
        let agents = json!({"agents": [agent("r1", None, "idle", None),
            json!({"id": "r2", "parent": null, "state": "idle", "project": "app",
                "waiting_on_answer": true, "created_at": 10, "active_at": 20}),
            agent("r3", None, "working", None),
            agent("r4", None, "idle", None), agent("c4", Some("r4"), "idle", None),
            agent("r5", None, "idle", None)]});
        let approvals = json!({"requests": [{"number": 3, "agent": "r1", "tool": "fs_read",
            "call": {"path": ".env"}, "created_at": 30}]});
        let subscriptions = json!({"subscriptions": [{"agent": "c4", "events": ["checks"],
            "target": {"project": "app", "repository": ".", "number": 1}}]});
        let asking = HashSet::from(["r1".to_owned(), "r2".to_owned()]);
        let snapshot = Snapshot::build(Answers {
            projects: &json!({"projects": [{"name": "app", "folder": "/work/app"}]}),
            workspaces: &json!({"workspaces": []}),
            agents: &agents,
            approvals: &approvals,
            needs_user: &asking,
            questions: &HashMap::from([("r2".to_owned(), ("Which one?".to_owned(), 40))]),
            briefs: &HashMap::new(),
            commands: &json!({"commands": []}),
            subscriptions: &subscriptions,
        });
        let said = |id: &str| snapshot.session(id).unwrap().mark_said.as_str();
        assert_eq!(said("r1"), "Needs you: a permission");
        assert_eq!(said("r2"), "Needs you: a question");
        assert_eq!(said("r3"), "Running");
        assert_eq!(said("r4"), "Waiting: checks of #1");
        assert_eq!(said("r5"), "", "no mark, no words");
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
            subscriptions: &json!({"subscriptions": []}),
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
