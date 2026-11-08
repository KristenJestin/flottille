//! The tree (`docs/design.md` §5, `docs/agents.md` "The tree"): agents launched by agents, the
//! tools that launch, read, write to and stop them, and what their parent is told.
//!
//! A child belongs to its parent: only its parent writes to it and answers what it asks, and the
//! parent is told everything that matters as deliveries (`deliveries`). Stopping an agent stops
//! its tree, deepest first.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::Arc;

use flottille_core::agent::{Agent, AgentState};
use flottille_core::approval::{self, Decision, Request};
use flottille_core::protocol::AgentTrees;
use flottille_core::secrets;
use flottille_core::store::{AgentChange, Store, now_ms};
use flottille_core::subscription::Subscription;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agents::{self, DEPTH_LIMIT, New, Order};
use crate::approvals;
use crate::daemon::Daemon;
use crate::deliveries;
use crate::git::{self, Class};
use crate::mcp::{Grant, Outcome, arguments};
use crate::projects::{self, Failure};
use crate::providers;
use crate::roles;
use crate::workspaces;

/// The tree's tools, served with the others.
pub const TOOLS: [&str; 11] = [
    "agent_launch",
    "agent_send",
    "agent_status",
    "agent_stop",
    "report",
    "ask",
    "ask_user",
    "ask_parent",
    "approval_answer",
    "session_send",
    "session_read",
];

/// What `agent_status` shows of a conversation at once, at most, in characters: under the result
/// limit, with room for the notes pinned on it.
const READ_PAGE: usize = 30_000;
/// What a line of a conversation keeps of a text.
pub(crate) const LINE_CHARS: usize = 2_000;
/// The last messages `agent_status` and a parent's deliveries show of a child.
const LAST_MESSAGES: usize = 3;
const MESSAGE_CHARS: usize = 600;
/// What a report holds at most, in characters: it rides whole as a note on its parent's results.
const REPORT_CHARS: usize = 10_000;
/// A reported child still in its turn this long after is stopped.
const REPORT_GRACE_MS: u64 = 2 * 60 * 1000;

pub async fn call(daemon: &Arc<Daemon>, grant: &Arc<Grant>, tool: &str, args: Value) -> Outcome {
    let caller = match daemon.read(|store| store.agent(&grant.agent)) {
        Ok(Some(caller)) => caller,
        Ok(None) => return Outcome::Failed(format!("there is no agent {}", grant.agent)),
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let done = match tool {
        "agent_launch" => launch(daemon, &caller, args).await,
        "agent_send" => send(daemon, &caller, args),
        "agent_status" => status(daemon, &caller, args),
        "agent_stop" => stop(daemon, &caller, args),
        "report" => report(daemon, grant, &caller, args).await,
        // The two names `ask` replaced stay answered for one release, routed as `ask` is.
        "ask" | "ask_user" | "ask_parent" => ask(daemon, &caller, tool, args),
        "approval_answer" => answer(daemon, &caller, args),
        "session_send" => session_send(daemon, &caller, args),
        "session_read" => Ok(crate::session_read::read(daemon, grant, args)),
        other => Err(Outcome::Failed(format!("{other} is not served"))),
    };
    done.unwrap_or_else(|outcome| outcome)
}

type Done = Result<Outcome, Outcome>;

fn refused(text: impl Into<String>) -> Outcome {
    Outcome::Refused(text.into())
}

fn failed(error: impl std::fmt::Display) -> Outcome {
    Outcome::Failed(error.to_string())
}

/// How an agent is named to another: its title and its id.
pub fn named(agent: &Agent) -> String {
    match &agent.title {
        Some(title) => format!("\"{title}\" (agent {})", agent.id),
        None => format!("agent {}", agent.id),
    }
}

/// One of the caller's children, or the refusal.
fn child_of(daemon: &Daemon, caller: &Agent, id: &str) -> Result<Agent, Outcome> {
    match daemon.read(|store| store.agent(id)) {
        Ok(Some(agent)) if agent.parent.as_deref() == Some(caller.id.as_str()) => Ok(agent),
        Ok(_) => Err(refused(format!(
            "refused: {id} is not one of your children: agent_status lists them"
        ))),
        Err(error) => Err(failed(error)),
    }
}

// Launching.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    title: String,
    brief: String,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    role: Option<String>,
    workspace: Option<String>,
    new_workspace: Option<String>,
    repositories: Option<Vec<String>>,
    /// With `new_workspace`: its branch, `<branch prefix>/<name>` when left out; an existing one
    /// is resumed, as `workspace create` does.
    branch: Option<String>,
    /// With `new_workspace`: the base branch of a repository, by path, in place of its rule.
    base: Option<BTreeMap<String, String>>,
    /// Which agent the launch starts (`docs/tools.md`, "The tree's tools"): never guessed.
    kind: Option<LaunchKind>,
}

/// What `agent_launch` starts: a child in the caller's place, a session the caller leads, or a
/// session it hands to the user (`docs/tools.md`, "Sessions an agent starts").
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    Child,
    Led,
    Handed,
}

/// What each kind does, as a refusal names them.
const KINDS: &str = "child: a part of your own task, in your place, that you direct and that \
    reports to you; led: an autonomous job of its own (its own workspace or main) that you pilot, \
    a real session the user sees under you and may talk to, which asks you its questions and wakes \
    you when it asks, reports or ends; handed: a session you start for the user and let go, \
    which is theirs and of which you hear nothing more unless it writes to you";

/// What a launch without `kind` is told.
fn kind_required() -> String {
    format!("refused: kind is required: say which agent you start. {KINDS}")
}

/// What a launch still using `root` or `follow` is told: there is no silent mapping.
fn root_and_follow_gone(field: &str) -> String {
    format!(
        "refused: {field} is no longer taken: kind says which agent you start, instead of root \
         and follow. {KINDS}"
    )
}

/// Whether the arguments of `agent_launch` start a session (`led` or `handed`) rather than a
/// child.
pub fn starts_session(args: &Value) -> bool {
    matches!(args["kind"].as_str(), Some("led" | "handed"))
}

/// Where a child works: an existing workspace, a new one, or its parent's place. The workspace
/// fields are plain and optional, and a mix of them is refused with the field to use.
enum WorkspaceChoice {
    Own,
    Existing(String),
    New {
        name: String,
        repositories: Option<Vec<String>>,
        branch: Option<String>,
        base: Option<BTreeMap<String, String>>,
    },
}

/// What a child is told when it would work in another place than its parent's: a workspace is a
/// session's (`docs/tools.md`, "Sessions an agent starts").
pub const CHILD_PLACES: &str = "refused: a child works where you work: workspace and \
    new_workspace are for a session for the user, which works there on its own, \
    agent_launch {\"kind\": \"led\", \"title\": \"Fix login\", \"brief\": \"...\", \
    \"workspace\": \"login\"} (or \"new_workspace\": \"login\"), judged; leave them out for a child in your own \
    place";

/// Whether `name`, given as the workspace of a session for the user, is the project folder:
/// `main`, unless the project holds a workspace of that name, made before it was reserved, which
/// keeps it (`docs/workspaces.md`, "Main").
pub fn names_main(daemon: &Daemon, project: &str, name: &str) -> bool {
    name == flottille_core::workspace::MAIN
        && daemon
            .read(|store| store.workspace(project, name))
            .is_ok_and(|workspace| workspace.is_none())
}

/// The refusals of a launch that need nothing looked up but the caller's place: checked before
/// the gate is asked, and again when the launch is performed.
pub fn launch_refusal(workspace: Option<&str>, args: &Value) -> Result<(), Outcome> {
    let asked = Launch::of(args.clone())?;
    asked.place(workspace).map(|_| ())
}

impl Launch {
    /// The arguments of a launch, its kind named: `root` and `follow`, which it replaced, are
    /// refused before the arguments are read, so that the refusal names `kind`.
    fn of(args: Value) -> Result<Launch, Outcome> {
        if let Some(field) = ["root", "follow"].into_iter().find(|f| !args[*f].is_null()) {
            return Err(refused(root_and_follow_gone(field)));
        }
        let asked: Launch = arguments("agent_launch", args)?;
        if asked.kind.is_none() {
            return Err(refused(kind_required()));
        }
        Ok(asked)
    }

    fn kind(&self) -> LaunchKind {
        self.kind.unwrap_or(LaunchKind::Child)
    }

    /// Whether it starts a session rather than a child.
    fn session(&self) -> bool {
        self.kind() != LaunchKind::Child
    }

    /// Where the agent launched works: a session anywhere, a child only in its parent's place,
    /// which it may name.
    fn place(&self, own: Option<&str>) -> Result<WorkspaceChoice, Outcome> {
        match self.workspace_choice()? {
            WorkspaceChoice::Existing(name) if !self.session() && own == Some(name.as_str()) => {
                Ok(WorkspaceChoice::Own)
            }
            WorkspaceChoice::Existing(_) | WorkspaceChoice::New { .. } if !self.session() => {
                Err(refused(CHILD_PLACES))
            }
            choice => Ok(choice),
        }
    }

    fn workspace_choice(&self) -> Result<WorkspaceChoice, Outcome> {
        if self.new_workspace.is_none()
            && let Some(field) = [
                ("branch", self.branch.is_some()),
                ("base", self.base.is_some()),
            ]
            .iter()
            .find_map(|(field, given)| given.then_some(field))
        {
            return Err(refused(format!(
                "refused: {field} goes with new_workspace, the name of the workspace to make on \
                 it: add it, or drop {field}"
            )));
        }
        match (&self.workspace, &self.new_workspace, &self.repositories) {
            (Some(_), Some(_), _) => Err(refused(
                "refused: workspace and new_workspace are exclusive: workspace names an existing \
                 workspace, new_workspace the name of one to make; leave both out for your own \
                 place",
            )),
            (Some(_), None, Some(_)) => Err(refused(
                "refused: repositories goes with new_workspace, which makes a workspace; \
                 workspace names one that exists: drop repositories, or use new_workspace",
            )),
            (None, None, Some(_)) => Err(refused(
                "refused: repositories goes with new_workspace, the name of the workspace to \
                 make: add it, or drop repositories to work in your own place",
            )),
            (Some(name), None, None) => Ok(WorkspaceChoice::Existing(name.clone())),
            (None, Some(name), repositories) => Ok(WorkspaceChoice::New {
                name: name.clone(),
                repositories: repositories.clone(),
                branch: self.branch.clone(),
                base: self.base.clone(),
            }),
            (None, None, None) => Ok(WorkspaceChoice::Own),
        }
    }
}

async fn launch(daemon: &Arc<Daemon>, caller: &Agent, args: Value) -> Done {
    let asked = Launch::of(args)?;
    let session = asked.session();
    // Checked again here, as the request was: only a root starts a root.
    if session && caller.parent.is_some() {
        return Err(refused(crate::actions::CHILD_ROOTS));
    }
    let led = asked.kind() == LaunchKind::Led;
    let choice = asked.place(caller.workspace.as_deref())?;
    if !session && caller.depth + 1 > DEPTH_LIMIT {
        return Err(refused(format!(
            "refused: this agent is at the depth limit ({DEPTH_LIMIT}): it cannot launch \
             children; do the work yourself"
        )));
    }
    if asked.title.trim().is_empty() || asked.brief.trim().is_empty() {
        return Err(refused(
            "refused: a child needs a title and a brief: the brief is all it will know",
        ));
    }
    // A session for the user is made as the window makes one: a lead by default.
    let role = match session {
        true => roles::LEAD,
        false => roles::WORKER,
    };
    let role = roles::role(asked.role.as_deref().unwrap_or(role)).ok_or_else(|| {
        refused(format!(
            "refused: there is no role {}: the roles are lead and worker",
            asked.role.as_deref().unwrap_or_default()
        ))
    })?;
    let project = projects::load_project(daemon, &caller.project).map_err(|f| failed(f.message))?;
    // A child runs on its parent's provider unless it names another.
    let provider = asked.provider.or_else(|| Some(caller.provider.clone()));
    let asked_provider = providers::Asked {
        provider: provider.clone(),
        model: asked.model.clone(),
        effort: asked.effort.clone(),
    };
    providers::choose(daemon, &project, asked_provider)
        .map_err(|f| refused(format!("refused: {}", f.message)))?;
    // The slot is taken before anything is made: a launch refused leaves nothing behind. A
    // session for the user is no child, and counts in no cap of children.
    let _slot = match session {
        true => None,
        false => Some(Slot::take(daemon, &project.name, project.max_children)?),
    };
    let in_main = matches!(&choice, WorkspaceChoice::Existing(name)
        if names_main(daemon, &project.name, name));
    let (workspace, preparing) = match choice {
        WorkspaceChoice::Own => (caller.workspace.clone(), false),
        WorkspaceChoice::Existing(_) if in_main => (None, false),
        WorkspaceChoice::Existing(name) => (Some(name), false),
        WorkspaceChoice::New {
            name,
            repositories,
            branch,
            base,
        } => {
            let created = workspaces::call(
                daemon.clone(),
                "workspace.create",
                json!({"project": project.name, "name": name,
                    "repositories": repositories, "branch": branch,
                    "bases": base.unwrap_or_default()}),
            )
            .await
            .map_err(|f| {
                refused(format!(
                    "refused: the workspace cannot be made: {}",
                    f.message
                ))
            })?;
            let name = created["name"].as_str().unwrap_or(&name).to_owned();
            (Some(name), true)
        }
    };
    let child = agents::create(
        daemon,
        &project,
        New {
            id: None,
            workspace: workspace.clone(),
            preparing,
            role,
            parent: (!session).then(|| (caller.id.clone(), caller.depth)),
            title: Some(asked.title.trim().to_owned()),
            brief: asked.brief.clone(),
            provider,
            model: asked.model,
            effort: asked.effort,
            // A child's is not read: a tree runs in its root's mode (`judge::mode`). A new root
            // takes the mode of the tree that started it, never a looser one: in Full, the caller
            // could already do all the new session would do.
            gate_mode: Some(crate::judge::mode(daemon, &caller.id)),
            launched_by: session.then(|| caller.id.clone()),
            led,
        },
    )
    .map_err(|f| refused(format!("refused: {}", f.message)))?;
    // Before its first message, so that its first turn wakes its pilot too.
    if led {
        crate::subscriptions::lead(daemon, &caller.id, &child).map_err(failed)?;
    }
    if preparing {
        tokio::spawn(brief_once_ready(daemon.clone(), child.clone()));
    } else {
        brief(daemon, &child);
    }
    let place = match (workspace, in_main) {
        (_, true) => "main, the project folder".to_owned(),
        (Some(w), _) => format!("workspace {w}"),
        (None, _) => "your own place".to_owned(),
    };
    if led {
        return Ok(Outcome::Done(format!(
            "Started a session led by you, {}, in {place}. It is a real session, not your child: \
             the user sees it under you and may talk to it, and the agent_* tools do not reach \
             it. It asks you its questions: a delivery [flottille:session-turn-ended] wakes you \
             when it asks, reports or ends, with its last message. Answer with session_send, \
             relay to the user with ask, or do nothing; forge unsubscribe with its session stops \
             the wakes.",
            named(&child)
        )));
    }
    if session {
        return Ok(Outcome::Done(format!(
            "Started a session for the user, {}, in {place}, handed over. It is the user's, not \
             your child: the user follows it in the window and the agent_* tools do not reach \
             it. You hear nothing more of it unless it writes to you; you can write to it with \
             session_send.",
            named(&child)
        )));
    }
    Ok(Outcome::Done(format!(
        "Launched {} in {place}. It works on its own: you are told when it reports, asks, \
         fails, is stuck or goes idle, so do not poll it. Go on with your own work, or end your \
         turn.",
        named(&child)
    )))
}

/// A slot of a project's children cap, taken by a launch until its child exists (and counts as a
/// live child), or the launch fails.
struct Slot<'a> {
    daemon: &'a Daemon,
    project: String,
}

impl<'a> Slot<'a> {
    fn take(daemon: &'a Daemon, project: &str, cap: u8) -> Result<Slot<'a>, Outcome> {
        let mut launching = daemon.launching.lock().unwrap();
        let taken = launching.get(project).copied().unwrap_or(0);
        let running = daemon
            .read(|store| store.live_children(project))
            .map_err(failed)?
            + taken;
        if running >= usize::from(cap) {
            return Err(refused(format!(
                "refused: {running} children already run in project {project}, its cap: do the \
                 work yourself, or wait for one of them to end (agent_stop ends one of yours)"
            )));
        }
        *launching.entry(project.to_owned()).or_default() += 1;
        Ok(Slot {
            daemon,
            project: project.to_owned(),
        })
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut launching = self.daemon.launching.lock().unwrap();
        if let Some(taken) = launching.get_mut(&self.project) {
            *taken -= 1;
            if *taken == 0 {
                launching.remove(&self.project);
            }
        }
    }
}

/// The first message of an agent a launch made: a child's brief, or the first message of a session
/// an agent started for the user.
fn brief(daemon: &Arc<Daemon>, child: &Agent) {
    let (kind, body) = first_message(daemon, child);
    deliveries::deliver(
        daemon,
        &child.id,
        kind,
        Some(format!("brief-{}-{}", child.id, child.epoch)),
        &body,
        approval::BETWEEN_TURNS,
    );
}

/// At start: an agent whose first message was never delivered (a daemon stopped while its new
/// workspace was preparing, and the wait for it lived in memory only) gets it, once its
/// workspace is ready. The delivery's id is the key, so one stored already is not stored twice.
pub fn brief_at_start(daemon: &Arc<Daemon>) {
    let agents = match daemon.read(|store| store.agents()) {
        Ok(agents) => agents,
        Err(error) => {
            eprintln!("flottille daemon: reading agents: {error}");
            return;
        }
    };
    for agent in agents {
        // A session the user started has its message delivered where it is made, as theirs.
        let launched =
            agent.parent.is_some() || agent.launched_by.is_some() || agent.detached_from.is_some();
        if agent.state.is_final() || agent.brief.is_none() || !launched {
            continue;
        }
        let key = format!("brief-{}-{}", agent.id, agent.epoch);
        if daemon
            .read(|store| store.delivery(&key))
            .is_ok_and(|d| d.is_some())
        {
            continue;
        }
        match agent.workspace {
            Some(_) => {
                tokio::spawn(brief_once_ready(daemon.clone(), agent));
            }
            None => brief(daemon, &agent),
        }
    }
}

/// What follows the launcher's name on the first line of a session an agent started for the user.
pub const LAUNCHED: &str = " started this session for the user with this message:";

/// The kind and the body of an agent's brief as it is delivered: `brief`, or for a session an
/// agent started for the user `launched`, under a line naming that agent: never the user's words,
/// for the window, the judge or the agent itself.
pub fn first_message(daemon: &Daemon, agent: &Agent) -> (&'static str, String) {
    let brief = agent.brief.clone().unwrap_or_default();
    // Detached, it is still that agent's words.
    let Some(launcher) = agent.launched_by.as_ref().or(agent.detached_from.as_ref()) else {
        return ("brief", brief);
    };
    let who = match daemon.read(|store| store.agent(launcher)) {
        Ok(Some(launcher)) => named(&launcher),
        _ => format!("agent {launcher}"),
    };
    let guide = where_questions_go(launcher, agent.led);
    ("launched", format!("{who}{LAUNCHED}\n\n{brief}\n\n{guide}"))
}

/// What opens the paragraph of a session's first message that says where a question goes: the
/// window shows the message without it.
pub const WHERE_QUESTIONS_GO: &str = "Where a question goes:";

/// Where a question of a session an agent started goes (`docs/tools.md`, "Sessions an agent
/// starts"): nothing reads the text of a message to find one, so the session is told.
fn where_questions_go(starter: &str, led: bool) -> String {
    let goes = match led {
        true => format!(
            "this session is led by agent {starter}: what you need answered goes through ask, \
             which reaches it, and its answer comes as [flottille:session]; the user may also \
             write to you directly."
        ),
        false => format!(
            "this session is the user's: what you need answered goes through ask, which reaches \
             the user's inbox. Agent {starter} started it and hears nothing more of it unless you \
             write to it with session_send {{\"session\": \"{starter}\", \"text\": \"…\"}}."
        ),
    };
    format!(
        "{WHERE_QUESTIONS_GO} {goes} A question left in a report or in your last message is a \
         follow-up, not a wait: nobody is told to answer it."
    )
}

/// The brief of a child whose new workspace is being prepared, once it is ready; a workspace
/// that fails fails the child.
async fn brief_once_ready(daemon: Arc<Daemon>, child: Agent) {
    let name = child.workspace.clone().unwrap_or_default();
    loop {
        let workspace = daemon.read(|store| store.workspace(&child.project, &name));
        match workspace.ok().flatten().map(|w| w.state()) {
            Some(flottille_core::workspace::State::Ready) => return brief(&daemon, &child),
            Some(flottille_core::workspace::State::Preparing) => {}
            _ => {
                return fail_tree(
                    &daemon,
                    &child.id,
                    &format!(
                        "its workspace {name} could not be prepared: see `flottille workspace \
                         show {} {name}`",
                        child.project
                    ),
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

// Writing, reading, stopping.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    agent_id: String,
    message: String,
    /// The child's provider never changes: these are its model and effort, set between its
    /// turns, as `agent.options` does for the user.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
}

fn send(daemon: &Arc<Daemon>, caller: &Agent, args: Value) -> Done {
    let asked: Send = arguments("agent_send", args)?;
    let child = child_of(daemon, caller, &asked.agent_id)?;
    if child.state.is_final() {
        return Err(refused(format!(
            "refused: {} has {}: launch a new child whose brief names what it left",
            named(&child),
            child.state.as_str()
        )));
    }
    if asked.message.trim().is_empty() {
        return Err(refused("refused: the message is empty"));
    }
    // `/name` sends the project's command of that name, as the user's composer does.
    let message = crate::commands::message_for(
        daemon,
        &child.project,
        child.workspace.as_deref(),
        &asked.message,
    )
    .map_err(|failure| refused(format!("refused: {}", failure.message)))?;
    // Checked before anything is sent: a refused change sends no message.
    let changed = asked.model.is_some() || asked.effort.is_some();
    let mut dropped = String::new();
    if changed {
        let answer = crate::agents::change_options(daemon, &child, asked.model, asked.effort)
            .map_err(|failure| refused(format!("refused: {}", failure.message)))?;
        if let Some(why) = answer["dropped"].as_str() {
            dropped = format!(" {why}.");
        }
    }
    deliveries::deliver_message(
        daemon,
        &child.id,
        "parent",
        None,
        &message,
        approval::URGENT,
        Vec::new(),
    );
    let options = if changed {
        format!(" Its model and effort change between its turns, before its next prompt.{dropped}")
    } else {
        String::new()
    };
    Ok(Outcome::Done(format!(
        "Sent to {}: it reaches it on its next tool result, or as a new turn.{options}",
        named(&child)
    )))
}

// Sessions that talk to each other.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSend {
    pub session: String,
    pub text: String,
}

/// How the caller of `session_send` stands to the session it writes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// The caller started it.
    Started,
    /// It started the caller.
    StartedYou,
    /// The user let the caller write to it.
    Leave,
    /// None of these: the first message asks the user's leave.
    NoLeave,
}

/// The session a `session_send` writes to, and how the caller stands to it, or the refusal: a
/// live root of the caller's project other than its own.
pub fn session_of(
    daemon: &Daemon,
    caller: &Agent,
    asked: &SessionSend,
) -> Result<(Agent, Relation), Outcome> {
    let found = daemon
        .read(|store| store.agent(&asked.session))
        .map_err(failed)?;
    let id = &asked.session;
    let session = match found {
        None => {
            return Err(refused(format!(
                "refused: {id} names no session: session_send writes to a session of your \
                 project, named by its agent id"
            )));
        }
        Some(agent) if agent.id == caller.id => {
            return Err(refused(format!(
                "refused: {id} is your own session: session_send writes to another"
            )));
        }
        Some(agent) if agent.parent.as_deref() == Some(caller.id.as_str()) => {
            return Err(refused(format!(
                "refused: {id} is one of your children: they are written to with agent_send"
            )));
        }
        Some(agent) if agent.parent.is_some() => {
            return Err(refused(format!(
                "refused: {id} is a child of another session: it is written to by its parent, \
                 never with session_send"
            )));
        }
        Some(agent) if agent.project != caller.project => {
            return Err(refused(format!(
                "refused: {id} is a session of another project, {}, not of {}: an agent writes \
                 only to the sessions of its own project",
                agent.project, caller.project
            )));
        }
        Some(agent) => agent,
    };
    let relation = if session.launched_by.as_deref() == Some(caller.id.as_str()) {
        Relation::Started
    } else if caller.launched_by.as_deref() == Some(session.id.as_str()) {
        Relation::StartedYou
    } else if daemon
        .read(|store| store.has_leave(&caller.id, &session.id))
        .map_err(failed)?
    {
        Relation::Leave
    } else {
        Relation::NoLeave
    };
    if session.state.is_final() {
        return Err(refused(format!(
            "refused: {} has {}: it reads nothing more",
            named(&session),
            session.state.as_str()
        )));
    }
    if asked.text.trim().is_empty() {
        return Err(refused("refused: the text is empty"));
    }
    Ok((session, relation))
}

/// Whether a `session_send` asks the user's leave: anything but a session the caller started,
/// the one that started it, or one the user let it write to. A call that names no session it may
/// write to is the refusal that says why (an ended session, a child), so that nothing passes on a
/// doubt and the agent reads what changed.
pub fn needs_leave(daemon: &Daemon, grant: &Grant, args: &Value) -> Result<bool, String> {
    let asked = serde_json::from_value::<SessionSend>(args.clone())
        .map_err(|error| format!("refused: session_send: {error}"))?;
    let caller = match daemon.read(|store| store.agent(&grant.agent)) {
        Ok(Some(caller)) => caller,
        Ok(None) => return Err("refused: the agent that writes no longer exists".into()),
        Err(error) => return Err(format!("failed: {error}")),
    };
    match session_of(daemon, &caller, &asked) {
        Ok((_, relation)) => Ok(relation == Relation::NoLeave),
        Err(
            Outcome::Refused(why) | Outcome::Failed(why) | Outcome::Held(why) | Outcome::Done(why),
        ) => Err(why),
    }
}

/// A session another agent acts on, as the user reads it on a request: its name, and who started
/// it, which is never the agent asking.
pub fn other_session(daemon: &Daemon, session: &Agent) -> String {
    let starter = match &session.launched_by {
        None => "you".to_owned(),
        Some(id) => match daemon.read(|store| store.agent(id)) {
            Ok(Some(agent)) => named(&agent),
            _ => format!("agent {id}"),
        },
    };
    format!(
        "the session {} (started by {starter}, not by this agent)",
        named(session)
    )
}

/// The user allowed the first message of `writer` to `session`: the pair writes without asking
/// the user again, until either session ends or the user takes it back.
pub fn grant_leave(daemon: &Daemon, writer: &str, session: &str, request: u64) {
    if let Err(error) = daemon.write(|store| store.grant_leave(writer, session, request)) {
        eprintln!("flottille daemon: the leave of {writer} to write to {session}: {error}");
    }
}

/// What follows the sender's name on the first line of a message `session_send` delivers.
pub const SESSION_WROTE: &str = " wrote to this session:";

/// `session_send`: a message to another session of the caller's project, once the gate let it
/// through (`Relation`), delivered as `session` under a line naming the sender: an agent's words, never the user's.
/// The delivery's id names the sender, for the judge (`judge::session_messages`).
fn session_send(daemon: &Arc<Daemon>, caller: &Agent, args: Value) -> Done {
    let asked: SessionSend = arguments("session_send", args)?;
    let (session, relation) = session_of(daemon, caller, &asked)?;
    // A following ties a session to the one that started it; two sessions a leave joins have none.
    let following = match relation {
        Relation::Started => Some((caller.id.as_str(), &session)),
        Relation::StartedYou => Some((session.id.as_str(), caller)),
        Relation::Leave | Relation::NoLeave => None,
    };
    if let Some((starter, followed)) = following
        && crate::subscriptions::paused(daemon, starter, &followed.id)
    {
        return Err(refused(crate::subscriptions::PAUSED));
    }
    let id = flottille_core::agent::new_id().map_err(failed)?;
    deliveries::deliver(
        daemon,
        &session.id,
        "session",
        Some(format!("session-{}-{id}", caller.id)),
        &format!("{}{SESSION_WROTE}\n\n{}", named(caller), asked.text),
        approval::URGENT,
    );
    if let Some((starter, followed)) = following {
        let answered = relation == Relation::StartedYou;
        crate::subscriptions::exchanged(daemon, starter, followed, answered);
    }
    Ok(Outcome::Done(format!(
        "Sent to {}: it reaches it on its next tool result, or as a new turn. An answer comes \
         as a [flottille:session] message.",
        named(&session)
    )))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    agent_id: Option<String>,
    action: Option<String>,
    detail: Option<String>,
    after: Option<u64>,
}

/// `agent_status`: how the children do, or the conversation of one of them.
fn status(daemon: &Daemon, caller: &Agent, args: Value) -> Done {
    let asked: Status = arguments("agent_status", args)?;
    match asked.action.as_deref().unwrap_or("status") {
        "status" => {
            if asked.after.is_some() {
                return Err(refused(
                    "refused: after is for action conversation: it pages a child's conversation",
                ));
            }
            // All the children at once are a line each; one child, which was asked for, is
            // shown whole.
            let full = match asked.detail.as_deref() {
                Some("full") => true,
                Some("short") => false,
                None => asked.agent_id.is_some(),
                Some(other) => {
                    return Err(refused(format!(
                        "refused: agent_status has no detail {other:?}: its details are short and \
                         full"
                    )));
                }
            };
            states(daemon, caller, asked.agent_id, full)
        }
        "conversation" => {
            if asked.detail.is_some() {
                return Err(refused(
                    "refused: detail is for action status; a conversation is paged with after",
                ));
            }
            let Some(id) = asked.agent_id else {
                return Err(refused(
                    "refused: action conversation needs agent_id, the child whose conversation \
                     to read: agent_status without it lists your children",
                ));
            };
            read(daemon, caller, &id, asked.after)
        }
        other => Err(refused(format!(
            "refused: agent_status has no action {other:?}: its actions are status (the default) \
             and conversation"
        ))),
    }
}

/// How one child, or each of them, does: a line each, with the child's last messages when `full`.
fn states(daemon: &Daemon, caller: &Agent, id: Option<String>, full: bool) -> Done {
    let children = match id {
        Some(id) => vec![child_of(daemon, caller, &id)?],
        None => daemon
            .read(|store| store.children(&caller.id))
            .map_err(failed)?,
    };
    if children.is_empty() {
        return Ok(Outcome::Done("You have no children.".into()));
    }
    let blocks: Vec<String> = children
        .iter()
        .map(|child| {
            let mut block = format!("- {}", line(daemon, child));
            if full {
                for message in last_messages(daemon, &child.id) {
                    block.push_str(&format!("\n  > {}", cut(&message, MESSAGE_CHARS)));
                }
            }
            block
        })
        .collect();
    let mut text = blocks.join("\n");
    if !full {
        text.push_str(
            "\n\nTheir last messages: agent_status {\"detail\": \"full\"}, or {\"agent_id\": \
             \"<id>\"} for one child; its conversation: {\"agent_id\": \"<id>\", \"action\": \
             \"conversation\"}.",
        );
    }
    Ok(Outcome::Done(text))
}

/// One line of what a child is and does, as `agent_status` and `children-status` give it.
pub fn line(daemon: &Daemon, agent: &Agent) -> String {
    let now = now_ms();
    let mut parts = vec![format!("{}, {}", named(agent), doing(daemon, agent))];
    if !agent.state.is_final() {
        let last = agent.active_at.unwrap_or(agent.created_at);
        parts.push(format!(
            "last sign of activity {} ago",
            ago(now.saturating_sub(last))
        ));
        if let Some(used) = context_used(daemon, agent) {
            parts.push(format!("context {used}% used"));
        }
    }
    parts.join("; ")
}

/// How much of its context the agent used at its last report, in percent; `None` before one,
/// and while the window's size is not known.
pub fn context_used(daemon: &Daemon, agent: &Agent) -> Option<u64> {
    let (used, size) = context_tokens(daemon, agent)?;
    let size = size.filter(|s| *s > 0)?;
    Some(used * 100 / size)
}

/// The tokens the agent's context holds at its last report, and the window's size when known.
pub fn context_tokens(daemon: &Daemon, agent: &Agent) -> Option<(u64, Option<u64>)> {
    let usage = daemon
        .read(|store| store.last_agent_events(&agent.id, "agent.usage", 1))
        .ok()?
        .into_iter()
        .next()?;
    Some((usage.data["used"].as_u64()?, usage.data["size"].as_u64()))
}

/// What the agent does now, in a few words.
pub fn doing(daemon: &Daemon, agent: &Agent) -> String {
    let now = now_ms();
    match agent.state {
        AgentState::Ended | AgentState::Failed => {
            let reason = agent
                .state_reason
                .as_deref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default();
            return format!("{}{reason}", agent.state.as_str());
        }
        AgentState::Starting => return "starting".into(),
        _ => {}
    }
    if let Some(request) = daemon
        .read(|store| store.pending_requests_of(&agent.id))
        .ok()
        .and_then(|requests| requests.into_iter().next())
    {
        return format!("waiting for approval #{}", request.number);
    }
    if let Some(run) = daemon.read(Store::running_runs).ok().and_then(|runs| {
        runs.into_iter()
            .find(|run| run.agent.as_deref() == Some(agent.id.as_str()))
    }) {
        let line = secrets::mask(&run.line, &std::env::vars_os().collect());
        return format!(
            "running `{}` for {}",
            cut(&line, 200),
            ago(now.saturating_sub(run.started_at))
        );
    }
    if daemon.waiting_provider.lock().unwrap().contains(&agent.id) {
        return "waiting on the provider".into();
    }
    if crate::compaction::running(daemon, &agent.id) {
        return "compacting the conversation".into();
    }
    match agent.state {
        AgentState::Working => "in a turn".into(),
        _ if agent.reported_at.is_some() => "reported".into(),
        _ if agent.waiting_on_answer && agent.parent.is_some() => {
            "waiting for its parent's answer".into()
        }
        _ if agent.waiting_on_answer && waits_on_pilot(daemon, agent) => {
            "waiting for its pilot's answer".into()
        }
        _ if agent.waiting_on_answer => "waiting for the user's answer".into(),
        _ => match crate::subscriptions::waiting_for(daemon, &agent.id) {
            Some(waiting) => waiting,
            None => format!("idle for {}", ago(now.saturating_sub(agent.updated_at))),
        },
    }
}

/// A duration as a person says it.
pub(crate) fn ago(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds < 90 {
        format!("{seconds} s")
    } else if seconds < 90 * 60 {
        format!("{} min", seconds / 60)
    } else {
        format!("{} h", seconds / 3600)
    }
}

fn cut(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}‹+{} chars›", count - limit)
}

/// The id a piece of a reply carries when its source marks which message it belongs to.
fn message_id(data: &Value) -> Option<&str> {
    data["message_id"].as_str()
}

/// Whether the daemon records the event itself, beside the agent's recorder (a tool it serves, a
/// delivery, an approval): it may come between two pieces of a message the agent is still saying.
fn beside_the_recorder(name: &str) -> bool {
    matches!(
        name,
        "agent.tool_served" | "agent.asked" | "agent.reported" | "agent.stuck"
    ) || name.starts_with("delivery.")
        || name.starts_with("approval.")
}

/// The agent's last messages, each whole (its pieces joined) and masked, the oldest first.
pub fn last_messages(daemon: &Daemon, agent: &str) -> Vec<String> {
    last_messages_after(daemon, agent, 0)
}

/// [`last_messages`], of those after the event `after` only.
pub fn last_messages_after(daemon: &Daemon, agent: &str, after: u64) -> Vec<String> {
    let mut events = daemon
        .read(|store| store.last_agent_events(agent, "agent.message", 200))
        .unwrap_or_default();
    events.retain(|event| event.seq > after);
    // Pieces of one message share its id: an event the daemon recorded between them does not
    // split it. Without an id, the pieces of one follow each other: a gap in the numbers starts
    // another message.
    let mut messages: Vec<(u64, Option<&str>, String)> = Vec::new();
    for event in events.iter().rev() {
        let text = event.data["text"].as_str().unwrap_or_default();
        let id = message_id(&event.data);
        match messages.last_mut() {
            Some((last, current, message))
                if id == *current && (id.is_some() || event.seq == *last + 1) =>
            {
                message.push_str(text);
                *last = event.seq;
            }
            _ => messages.push((event.seq, id, text.to_owned())),
        }
    }
    let skip = messages.len().saturating_sub(LAST_MESSAGES);
    // A reply is stored masked whole (`agents::Recorder`); masked again for one stored before.
    let env = std::env::vars_os().collect();
    messages
        .into_iter()
        .skip(skip)
        .map(|(_, _, message)| secrets::mask(&message, &env))
        .collect()
}

/// A page of an agent's conversation.
pub struct Page {
    /// Each line led by the number of its event.
    pub lines: Vec<String>,
    /// The number to read on after.
    pub after: u64,
    /// Whether the page stopped before the end of what is stored.
    pub more: bool,
}

/// The agent's conversation after the event `after`, at most `READ_PAGE` characters of it. A
/// message streamed in pieces is one line, led by the number of its first piece, and masked once
/// joined; a long one goes on, whole, on the next lines, each of `LINE_CHARS` at most. A piece
/// that carries another message id starts another line; one that carries the same id joins it
/// across the events the daemon recorded meanwhile, whose lines follow it.
pub fn conversation(daemon: &Daemon, agent: &Agent, after: u64) -> flottille_core::Result<Page> {
    let mut page = Page {
        lines: Vec::new(),
        after,
        more: false,
    };
    let mut size = 0;
    // Lines that go together, unless the page is full: they end after the event `last`.
    let mut add = |page: &mut Page, lines: Vec<(u64, String)>, last: u64| {
        let lines: Vec<String> = lines
            .into_iter()
            .map(|(first, line)| format!("#{first} {line}"))
            .collect();
        let length: usize = lines.iter().map(|line| line.len() + 1).sum();
        if size + length > READ_PAGE + 1 && !page.lines.is_empty() {
            page.more = true;
            return false;
        }
        size += length;
        page.lines.extend(lines);
        page.after = last;
        true
    };
    let mut said: Option<Said> = None;
    let mut between = Between::default();
    // A reply is stored masked whole (`agents::Recorder`); masked again for one stored before.
    let env = std::env::vars_os().collect();
    let mut read = after;
    'pages: loop {
        let events = daemon.read(|store| store.agent_events(&agent.id, read, 200))?;
        if events.is_empty() {
            break;
        }
        for event in events {
            read = event.seq;
            if event.name == "agent.message" {
                let piece = event.data["text"].as_str().unwrap_or_default();
                let id = message_id(&event.data);
                match &mut said {
                    Some((_, last, current, text))
                        if current.as_deref() == id
                            && text.chars().count() + piece.chars().count() <= LINE_CHARS =>
                    {
                        *last = event.seq;
                        text.push_str(piece);
                        continue;
                    }
                    _ => {}
                }
                if let Some((lines, last)) = joined(&mut said, &mut between, &env)
                    && !add(&mut page, lines, last)
                {
                    break 'pages;
                }
                said = Some((
                    event.seq,
                    event.seq,
                    id.map(str::to_owned),
                    piece.to_owned(),
                ));
                continue;
            }
            if said.as_ref().is_some_and(|said| said.2.is_some())
                && beside_the_recorder(&event.name)
            {
                between.0 = event.seq;
                between
                    .1
                    .extend(conversation_line(&event.name, &event.data).map(|l| (event.seq, l)));
                continue;
            }
            if let Some((lines, last)) = joined(&mut said, &mut between, &env)
                && !add(&mut page, lines, last)
            {
                break 'pages;
            }
            match conversation_line(&event.name, &event.data) {
                Some(line) => {
                    if !add(&mut page, vec![(event.seq, line)], event.seq) {
                        break 'pages;
                    }
                }
                None => page.after = event.seq,
            }
        }
    }
    if !page.more
        && let Some((lines, last)) = joined(&mut said, &mut between, &env)
    {
        add(&mut page, lines, last);
    }
    Ok(page)
}

/// The message being joined: the numbers of its first and last pieces, its id and its text.
type Said = (u64, u64, Option<String>, String);

/// The events the daemon recorded between the pieces of a message: the last one's number, and
/// their lines, each led by its number.
#[derive(Default)]
struct Between(u64, Vec<(u64, String)>);

/// The line of the message joined so far, masked, and the lines that came between its pieces:
/// they go together, after the last event they hold.
fn joined(
    said: &mut Option<Said>,
    between: &mut Between,
    env: &BTreeMap<OsString, OsString>,
) -> Option<(Vec<(u64, String)>, u64)> {
    let (first, last, _, text) = said.take()?;
    let Between(crossed, lines) = std::mem::take(between);
    let line = conversation_line("agent.message", &json!({"text": secrets::mask(&text, env)}));
    let lines = std::iter::once((first, line.unwrap_or_default()))
        .chain(lines)
        .collect();
    Some((lines, last.max(crossed)))
}

fn read(daemon: &Daemon, caller: &Agent, id: &str, after: Option<u64>) -> Done {
    let child = child_of(daemon, caller, id)?;
    let Page { lines, after, more } =
        conversation(daemon, &child, after.unwrap_or(0)).map_err(failed)?;
    let mut text = format!("The conversation of {}:\n", named(&child));
    if lines.is_empty() {
        text.push_str("(nothing after this point)");
    } else {
        text.push_str(&lines.join("\n"));
    }
    if more {
        text.push_str(&format!(
            "\n\nMore follows: agent_status {{\"agent_id\": \"{}\", \"action\": \"conversation\", \
             \"after\": {after}}}",
            child.id
        ));
    } else {
        text.push_str(&format!(
            "\n\nThat is all for now; later, read on with after {after}."
        ));
    }
    Ok(Outcome::Done(text))
}

/// A line of a conversation, for an event that is one.
fn conversation_line(name: &str, data: &Value) -> Option<String> {
    let text = |field: &str| cut(data[field].as_str().unwrap_or_default(), LINE_CHARS);
    Some(match name {
        "agent.prompted" => {
            // What a compaction sends the new session again was not said to it by anyone.
            let sent = flottille_core::prompt::without_replay(data);
            if sent.is_empty() {
                return None;
            }
            format!("sent: {}", cut(&sent, LINE_CHARS))
        }
        "agent.message" => format!("said: {}", text("text")),
        "agent.tool_call" if data["update"] != true => {
            format!("tool call: {}", text("title"))
        }
        "agent.tool_served" => format!(
            "{} ({})",
            data["tool"].as_str().unwrap_or_default(),
            data["outcome"].as_str().unwrap_or_default()
        ),
        "agent.turn_ended" => format!(
            "turn ended ({})",
            data["stop_reason"].as_str().unwrap_or_default()
        ),
        "agent.replaced" => format!("its session was replaced: {}", text("reason")),
        "agent.reported" => format!("reported: {}", text("text")),
        _ => return None,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopChild {
    agent_id: String,
}

/// What `agent_stop` names: one of the caller's children, stopped at once as ever, or a live
/// session of its project, ended only once the user allowed it, every time.
enum Stopped {
    Child(Agent),
    Session(Agent),
}

/// What `agent_stop` ends, or the refusal, before anything is asked: the caller itself, a child
/// of another session, a session of another project, an unknown id or a session that ended.
fn stopped(daemon: &Daemon, caller: &Agent, id: &str) -> Result<Stopped, Outcome> {
    let found = daemon.read(|store| store.agent(id)).map_err(failed)?;
    match found {
        None => Err(refused(format!(
            "refused: {id} names no agent: agent_stop ends one of your children or a session of \
             your project, named by its agent id"
        ))),
        Some(agent) if agent.parent.as_deref() == Some(caller.id.as_str()) => {
            Ok(Stopped::Child(agent))
        }
        Some(agent) if agent.id == caller.id => Err(refused(format!(
            "refused: {id} is your own session: agent_stop ends another; end your turn instead"
        ))),
        Some(agent) if agent.parent.is_some() => Err(refused(format!(
            "refused: {id} is a child of another session: it is stopped by its parent, never by \
             you"
        ))),
        Some(agent) if agent.project != caller.project => Err(refused(format!(
            "refused: {id} is a session of another project, {}, not of {}: an agent ends only \
             the sessions of its own project",
            agent.project, caller.project
        ))),
        Some(agent) if agent.state.is_final() => Err(refused(format!(
            "refused: {} has {} already",
            named(&agent),
            agent.state.as_str()
        ))),
        Some(agent) => Ok(Stopped::Session(agent)),
    }
}

fn caller_of(daemon: &Daemon, grant: &Grant) -> Result<Agent, Outcome> {
    daemon
        .read(|store| store.agent(&grant.agent))
        .map_err(failed)?
        .ok_or_else(|| failed(format!("there is no agent {}", grant.agent)))
}

/// What `agent_stop` does, in words, for a session the user is asked to end: the session, where
/// it works, who started it, and what ending does; nothing for a child, stopped as ever. A call
/// that names nothing it may end is the refusal, before anything is asked.
pub fn stop_says(daemon: &Daemon, grant: &Grant, args: &Value) -> Result<Vec<String>, Outcome> {
    let asked: StopChild = arguments("agent_stop", args.clone())?;
    let caller = caller_of(daemon, grant)?;
    let Stopped::Session(session) = stopped(daemon, &caller, &asked.agent_id)? else {
        return Ok(Vec::new());
    };
    let place = match &session.workspace {
        Some(name) => format!("the workspace {name}"),
        None => "main".to_owned(),
    };
    let starter = match session.launched_by.as_deref() {
        Some(id) if id == caller.id => "this agent".to_owned(),
        None => "you, not by this agent".to_owned(),
        Some(id) => match daemon.read(|store| store.agent(id)) {
            Ok(Some(agent)) => format!("{}, not by this agent", named(&agent)),
            _ => format!("agent {id}, not by this agent"),
        },
    };
    Ok(vec![format!(
        "End the session {}, in {place}, started by {starter}: its tree, its runs and what was \
         queued for it are stopped; its workspace stays",
        named(&session)
    )])
}

/// Whether an `agent_stop` ends a session rather than one of the caller's children: the user
/// is asked, every time, in every mode (`gate::end_session`).
pub fn ends_session(daemon: &Daemon, grant: &Grant, args: &Value) -> bool {
    let Ok(asked) = serde_json::from_value::<StopChild>(args.clone()) else {
        return false;
    };
    caller_of(daemon, grant)
        .and_then(|caller| stopped(daemon, &caller, &asked.agent_id))
        .is_ok_and(|stopped| matches!(stopped, Stopped::Session(_)))
}

/// `agent_stop`: one of the caller's children, or, once the user allowed it, a session of its
/// project, ended as the window's "End session" ends it.
fn stop(daemon: &Arc<Daemon>, caller: &Agent, args: Value) -> Done {
    let asked: StopChild = arguments("agent_stop", args)?;
    let child = match stopped(daemon, caller, &asked.agent_id)? {
        Stopped::Child(child) => child,
        Stopped::Session(session) => {
            let reason = format!(
                "ended at the request of the session {}, allowed by you",
                named(caller)
            );
            stop_tree(daemon, &session.id, &reason);
            return Ok(Outcome::Done(format!(
                "Ended the session {} with its tree. Its workspace stays.",
                named(&session)
            )));
        }
    };
    if child.state.is_final() {
        return Ok(Outcome::Done(format!(
            "{} has {} already.",
            named(&child),
            child.state.as_str()
        )));
    }
    stop_tree(daemon, &child.id, "stopped by its parent");
    Ok(Outcome::Done(format!(
        "Stopped {} and its own children. Its workspace stays.",
        named(&child)
    )))
}

// Reporting and asking.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    text: String,
}

async fn report(daemon: &Arc<Daemon>, grant: &Grant, caller: &Agent, args: Value) -> Done {
    let asked: Report = arguments("report", args)?;
    if asked.text.trim().is_empty() {
        return Err(refused("refused: the report is empty"));
    }
    let length = asked.text.chars().count();
    if length > REPORT_CHARS {
        return Err(refused(format!(
            "refused: the report is {length} characters, over {REPORT_CHARS}: say what was done \
             and what is left in fewer words, and put the details in a file of your place that \
             the report names"
        )));
    }
    if caller.reported_at.is_some() && caller.parent.is_some() {
        return Err(refused(
            "refused: you reported already: your session ends with this turn, end it now",
        ));
    }
    let env = std::env::vars_os().collect();
    let text = secrets::mask(&asked.text, &env);
    daemon
        .write(|store| {
            let (_, event) = store.change_agent(
                &caller.id,
                AgentChange {
                    reported_at: caller.parent.is_some().then_some(Some(now_ms())),
                    ..AgentChange::default()
                },
                Some(("agent.reported", json!({"agent": caller.id, "text": text}))),
            )?;
            Ok(((), event))
        })
        .map_err(failed)?;
    if caller.parent.is_none() {
        return Ok(Outcome::Done("Your report was shown to the user.".into()));
    }
    let mut body = format!("reported:\n\n{text}\n\n");
    body.push_str(&format!(
        "It worked in {}.",
        caller
            .workspace
            .as_ref()
            .map_or("the project folder".to_owned(), |w| format!(
                "workspace {w}, branch {}",
                grant.place.branch
            ))
    ));
    for (path, folder) in &grant.place.repositories {
        if let Ok(commit) = git::output(
            folder,
            &["log", "-1", "--format=%h %s", "--end-of-options", "HEAD"],
            Class::Read,
        )
        .await
        {
            body.push_str(&format!(
                "\nLast commit in {path}: {}",
                secrets::mask(commit.trim(), &env)
            ));
        }
    }
    tell_parent(daemon, caller, "child-report", &body, approval::URGENT);
    Ok(Outcome::Done(
        "Your report went to your parent. Your session ends with this turn: end it now.".into(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Question {
    question: String,
}

fn ask(daemon: &Arc<Daemon>, caller: &Agent, tool: &str, args: Value) -> Done {
    let asked: Question = arguments(tool, args)?;
    if asked.question.trim().is_empty() {
        return Err(refused("refused: the question is empty"));
    }
    let question = secrets::mask(&asked.question, &std::env::vars_os().collect());
    let pilot = pilot_of(daemon, caller);
    let to = match (&caller.parent, &pilot) {
        (Some(_), _) => "parent",
        (None, Some(_)) => "pilot",
        (None, None) => "user",
    };
    daemon
        .write(|store| {
            let (_, event) = store.change_agent(
                &caller.id,
                AgentChange {
                    waiting_on_answer: Some(true),
                    ..AgentChange::default()
                },
                Some((
                    "agent.asked",
                    json!({"agent": caller.id, "question": question, "to": to, "pilot": pilot}),
                )),
            )?;
            Ok(((), event))
        })
        .map_err(failed)?;
    if let Some(pilot) = pilot {
        return Ok(Outcome::Done(format!(
            "Your question went to agent {pilot}, which leads this session: it is told when your \
             turn ends, and its answer comes later as [flottille:session]. Go on with what does \
             not depend on it, or end your turn saying what you wait for."
        )));
    }
    if caller.parent.is_none() {
        return Ok(Outcome::Done(
            "Your question was given to the user; the answer comes later as a message. Go on \
             with what does not depend on it, or end your turn saying what you wait for."
                .into(),
        ));
    }
    tell_parent(
        daemon,
        caller,
        "child-question",
        &format!(
            "asks:\n\n{question}\n\nAnswer with agent_send {{\"agent_id\": \"{}\", \"message\": \
             \"…\"}}.",
            caller.id
        ),
        approval::BETWEEN_TURNS,
    );
    Ok(Outcome::Done(
        "Your question went to your parent; its answer comes later as [flottille:parent]. Go on \
         with what does not depend on it, or end your turn saying what you wait for."
            .into(),
    ))
}

/// The agent that leads `agent`'s session and still follows it: where its questions go
/// (`docs/tools.md`, "Where a question goes"). A pilot that ended its subscription, or whose
/// following is paused, is not woken, so the session's questions go to the user.
pub fn pilot_of(daemon: &Daemon, agent: &Agent) -> Option<String> {
    let pilot = agent.launched_by.as_ref().filter(|_| agent.led)?;
    crate::subscriptions::following(daemon, pilot, Some(&agent.id))
        .iter()
        .any(|following| following.seen["paused"] != true)
        .then(|| pilot.clone())
}

/// The user detaches a session from the agent that started it (`docs/tools.md`, "Detach"): the
/// agent's subscription ends and it is told once, the session becomes the user's alone and is
/// told so, at once. There is no undo.
pub fn detach(daemon: &Arc<Daemon>, id: &str) -> Result<Agent, Failure> {
    let session = daemon
        .read(|store| store.agent(id))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no agent {id}")))?;
    let Some(starter) = session.launched_by.clone() else {
        let why = match &session.detached_from {
            Some(from) => format!(
                "{} is already detached from agent {from}: it is the user's alone, and there is \
                 no undo",
                named(&session)
            ),
            None => format!(
                "{}: no agent started it, so there is nothing to detach it from",
                named(&session)
            ),
        };
        return Err(Failure::new("invalid-params", why));
    };
    let followings = crate::subscriptions::following(daemon, &starter, Some(&session.id));
    // One write: no reader sees the followings ended while the session is still led.
    daemon.write_all(|store| {
        let mut events = Vec::new();
        for following in &followings {
            if store.unsubscribe(&following.id)? {
                events.push(store.append_event(
                    "subscription.ended",
                    json!({"agent": following.agent, "subscription": following.id,
                        "why": "detached"}),
                )?);
            }
        }
        let (_, event) = store.change_agent(
            &session.id,
            AgentChange {
                detached_from: Some(starter.clone()),
                ..AgentChange::default()
            },
            Some((
                "agent.detached",
                json!({"agent": session.id, "from": starter}),
            )),
        )?;
        events.extend(event);
        Ok(((), events))
    })?;
    daemon.mcp.detached(&session.id);
    let moved = questions_to_user(daemon, &session, &starter);
    let title = session.title.clone().unwrap_or_else(|| session.id.clone());
    let starter_alive = daemon
        .read(|store| store.agent(&starter))
        .ok()
        .flatten()
        .is_some_and(|agent| !agent.state.is_final());
    if starter_alive {
        deliveries::deliver(
            daemon,
            &starter,
            "detached",
            Some(format!("detached-{starter}-from-{}", session.id)),
            &format!(
                "the user detached the session \"{title}\" (agent {}) from you. It is theirs \
                 alone: you are no longer woken by it, and writing to it now asks the user's leave.",
                session.id
            ),
            approval::BETWEEN_TURNS,
        );
    }
    if !session.state.is_final() {
        deliveries::deliver(
            daemon,
            &session.id,
            "detached",
            Some(format!("detached-{}", session.id)),
            &format!(
                "The user detached this session from agent {starter}: it is now the user's alone. \
                 What you need answered goes through ask to the user, and writing to agent \
                 {starter} now asks the user's leave.{}",
                moved_said(moved)
            ),
            approval::BETWEEN_TURNS,
        );
    }
    daemon
        .read(|store| store.agent(id))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no agent {id}")))
}

/// The pilot no longer reads `session` (it stopped following it, it ended, or the user detached
/// the session): the questions still open to it are asked again of the user (`agent.asked` to
/// the user, `moved_from` the pilot), which closes the pilot's, so that the session waits on the
/// user and the inbox shows them. How many went.
pub fn questions_to_user(daemon: &Daemon, session: &Agent, pilot: &str) -> usize {
    if session.parent.is_some() || !session.waiting_on_answer || session.state.is_final() {
        return 0;
    }
    let open = unanswered_to(daemon, &session.id, "pilot").unwrap_or_default();
    for question in &open {
        let _ = daemon.write(|store| {
            let (_, event) = store.change_agent(
                &session.id,
                AgentChange::default(),
                Some((
                    "agent.asked",
                    json!({"agent": session.id, "question": question["question"], "to": "user",
                        "pilot": null, "moved_from": pilot}),
                )),
            )?;
            Ok(((), event))
        });
    }
    open.len()
}

/// What a session its pilot no longer reads is told of its questions that went to the user.
pub fn moved_said(moved: usize) -> String {
    match moved {
        0 => String::new(),
        1 => " Your question still open went to the user.".into(),
        n => format!(" Your {n} questions still open went to the user."),
    }
}

/// The pilot stopped following the session it leads (`forge unsubscribe`, or the user's Cancel):
/// the session stays led, but its questions now go to the user, those still open to the pilot
/// too, and it is told once.
pub fn unfollowed(daemon: &Arc<Daemon>, following: &Subscription) {
    let Some(id) = following.target["session"].as_str() else {
        return;
    };
    let Ok(Some(session)) = daemon.read(|store| store.agent(id)) else {
        return;
    };
    if session.state.is_final()
        || !session.led
        || session.launched_by.as_deref() != Some(following.agent.as_str())
    {
        return;
    }
    let moved = questions_to_user(daemon, &session, &following.agent);
    let name = match daemon.read(|store| store.agent(&following.agent)) {
        Ok(Some(pilot)) => named(&pilot),
        _ => format!("agent {}", following.agent),
    };
    deliveries::deliver(
        daemon,
        &session.id,
        "unfollowed",
        Some(format!("unfollowed-{}-{}", session.id, following.id)),
        &format!(
            "{name}, which leads this session, no longer follows it: what you need answered now \
             goes through ask to the user.{}",
            moved_said(moved)
        ),
        approval::BETWEEN_TURNS,
    );
}

/// The pilot ended: the sessions it led go on, handed to the user, each told once.
fn hand_over(daemon: &Arc<Daemon>, pilot: &Agent) {
    let Ok(agents) = daemon.read(|store| store.agents()) else {
        return;
    };
    for session in agents
        .into_iter()
        .filter(|agent| agent.led && agent.launched_by.as_deref() == Some(pilot.id.as_str()))
    {
        let _ = daemon.write(|store| {
            let (_, event) = store.change_agent(
                &session.id,
                AgentChange {
                    led: Some(false),
                    ..AgentChange::default()
                },
                Some((
                    "agent.handed",
                    json!({"agent": session.id, "from": pilot.id}),
                )),
            )?;
            Ok(((), event))
        });
        if session.state.is_final() {
            continue;
        }
        let moved = questions_to_user(daemon, &session, &pilot.id);
        deliveries::deliver(
            daemon,
            &session.id,
            "handed",
            Some(format!("handed-{}-{}", session.id, pilot.id)),
            &format!(
                "{}, which led this session, ended. The session goes on, handed to the user: what \
                 you need answered now goes through ask to the user, and that agent no longer \
                 reads you.{}",
                named(pilot),
                moved_said(moved)
            ),
            approval::BETWEEN_TURNS,
        );
    }
}

// Requests.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    request_id: u64,
    decision: Choice,
    why: String,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Choice {
    Forward,
    Deny,
}

fn answer(daemon: &Arc<Daemon>, caller: &Agent, args: Value) -> Done {
    let asked: Answer = arguments("approval_answer", args)?;
    let number = asked.request_id;
    daemon
        .read(|store| store.request(number))
        .map_err(failed)?
        .filter(|r| r.held_by == caller.id && r.state == approval::RequestState::Pending)
        .ok_or_else(|| {
            refused(format!(
                "refused: request #{number} is not held by this agent: you answer only the \
                 pending requests that reach you as [flottille:child-asks]"
            ))
        })?;
    let why = asked.why.trim();
    if why.is_empty() {
        return Err(refused(
            "refused: say why in one sentence: it goes with your answer",
        ));
    }
    let why = secrets::mask(why, &std::env::vars_os().collect());
    if asked.decision == Choice::Deny {
        let decided = daemon
            .write(|store| {
                Ok(match store.decide(number, Decision::Deny, &caller.id)? {
                    Some((request, event)) => (Some(request), Some(event)),
                    None => (None, None),
                })
            })
            .map_err(failed)?
            .ok_or_else(|| refused(format!("refused: request #{number} was answered meanwhile")))?;
        approvals::store_answer(
            daemon,
            &decided,
            "refused",
            &format!(
                "Request #{number} ({}) was refused by {}: {why}. Nothing was done.",
                decided.tool,
                if decided.agent == caller.id {
                    "you".to_owned()
                } else {
                    named(caller)
                }
            ),
        );
        return Ok(Outcome::Done(format!(
            "Request #{number} is refused; the agent that asked is told why."
        )));
    }
    let to = caller
        .parent
        .clone()
        .unwrap_or_else(|| approval::USER.to_owned());
    let forwarded = daemon
        .write(|store| {
            Ok(match store.forward(number, &caller.id, &why, &to)? {
                Some((request, event)) => (Some(request), Some(event)),
                None => (None, None),
            })
        })
        .map_err(failed)?
        .ok_or_else(|| refused(format!("refused: request #{number} was answered meanwhile")))?;
    if caller.parent.is_none() {
        return Ok(Outcome::Done(format!(
            "Request #{number} went to the user, with your why. Its answer comes to the agent \
             that asked."
        )));
    }
    asks_parent(daemon, &forwarded);
    Ok(Outcome::Done(format!(
        "Request #{number} went to your parent, with your why. You cannot answer it any more."
    )))
}

/// Tells the agent that holds the request, a parent, that it is held for it (`child-asks`).
pub fn asks_parent(daemon: &Arc<Daemon>, request: &Request) {
    let Ok(Some(holder)) = daemon.read(|store| store.agent(&request.held_by)) else {
        return;
    };
    let Ok(Some(asker)) = daemon.read(|store| store.agent(&request.agent)) else {
        return;
    };
    let mut body = format!(
        "Request #{} of {}: a call held by the gate, which has not happened.\nThe call: {} {}\n\
         Held because: {}.",
        request.number,
        named(&asker),
        request.tool,
        request.shown,
        request.reasons.join("; ")
    );
    for forward in &request.forwards {
        let by = daemon
            .read(|store| store.agent(&forward.by))
            .ok()
            .flatten()
            .map_or_else(|| forward.by.clone(), |a| named(&a));
        body.push_str(&format!("\nForwarded by {by}: {}", forward.why));
    }
    body.push_str(&format!(
        "\n\nAnswer with approval_answer {{\"request_id\": {}, \"decision\": \"forward\" or \
         \"deny\", \"why\": \"…\"}}. You cannot allow it: forward it when {} should decide.",
        request.number,
        if holder.parent.is_some() {
            "your parent"
        } else {
            "the user"
        }
    ));
    deliveries::deliver(
        daemon,
        &holder.id,
        "child-asks",
        Some(format!("asks-{}-{}", request.number, holder.id)),
        &body,
        approval::URGENT,
    );
}

// What the parent is told, and ends.

/// A delivery to the agent's parent about it: `text` follows its name. Nothing for a root.
pub fn tell_parent(daemon: &Arc<Daemon>, child: &Agent, kind: &str, text: &str, urgency: &str) {
    let Some(parent) = &child.parent else {
        return;
    };
    deliveries::deliver(
        daemon,
        parent,
        kind,
        None,
        &format!("Your child {} {text}", named(child)),
        urgency,
    );
}

/// The agent and its tree end, deepest first: each stops, and what was queued for it is
/// superseded.
pub fn stop_tree(daemon: &Arc<Daemon>, id: &str, reason: &str) {
    let mut tree = descendants(daemon, id);
    tree.sort_by_key(|agent| std::cmp::Reverse(agent.depth));
    for agent in &tree {
        end(daemon, &agent.id, AgentState::Ended, "its parent stopped");
    }
    end(daemon, id, AgentState::Ended, reason);
}

/// The agent fails: its tree ends, and its parent is told, with its last messages.
pub fn fail_tree(daemon: &Arc<Daemon>, id: &str, reason: &str) {
    let mut tree = descendants(daemon, id);
    tree.sort_by_key(|agent| std::cmp::Reverse(agent.depth));
    for agent in &tree {
        end(daemon, &agent.id, AgentState::Ended, "its parent failed");
    }
    let reason = secrets::mask(reason, &std::env::vars_os().collect());
    if !end(daemon, id, AgentState::Failed, &reason) {
        return;
    }
    let Ok(Some(agent)) = daemon.read(|store| store.agent(id)) else {
        return;
    };
    let mut text = format!("failed: {reason}.");
    let messages = last_messages(daemon, id);
    if !messages.is_empty() {
        text.push_str("\nIts last messages:");
        for message in messages {
            text.push_str(&format!("\n> {}", cut(&message, MESSAGE_CHARS)));
        }
    }
    tell_parent(daemon, &agent, "child-failed", &text, approval::URGENT);
}

/// The ids of the agents of the tree under `root`, `root` first, ended ones too.
pub fn ids(daemon: &Daemon, root: &str) -> Result<Vec<String>, Failure> {
    let mut found = vec![root.to_owned()];
    let mut at = 0;
    while let Some(parent) = found.get(at).cloned() {
        let children = daemon.read(|store| store.children(&parent))?;
        found.extend(children.into_iter().map(|child| child.id));
        at += 1;
    }
    Ok(found)
}

/// Every agent below this one that has not ended.
fn descendants(daemon: &Daemon, id: &str) -> Vec<Agent> {
    let mut found = Vec::new();
    let mut parents = vec![id.to_owned()];
    while let Some(parent) = parents.pop() {
        for child in daemon
            .read(|store| store.children(&parent))
            .unwrap_or_default()
        {
            parents.push(child.id.clone());
            if !child.state.is_final() {
                found.push(child);
            }
        }
    }
    found
}

/// Ends one agent: its state, its queued deliveries, its process and its runs. Whether it was
/// still going.
fn end(daemon: &Arc<Daemon>, id: &str, state: AgentState, reason: &str) -> bool {
    let Ok(Some(agent)) = daemon.read(|store| store.agent(id)) else {
        return false;
    };
    if agent.state.is_final() {
        return false;
    }
    let event = if state == AgentState::Failed {
        "agent.failed"
    } else {
        "agent.ended"
    };
    agents::change(
        daemon,
        id,
        state,
        Some(reason.to_owned()),
        Some((event, json!({"reason": reason}))),
    );
    let _ = daemon.write(|store| Ok(((), store.supersede(id)?)));
    // Nobody is left to answer what it asked, nor to answer what it held: nobody sees them.
    let _ = daemon.write_all(|store| {
        let events = store.cancel_requests(id, &format!("agent {id} {}", state.as_str()))?;
        Ok(((), events))
    });
    deliveries::forget(daemon, id);
    crate::subscriptions::ended(daemon, id);
    let why = match state {
        AgentState::Failed => format!("failed: {reason}"),
        _ => format!("ended: {reason}"),
    };
    crate::subscriptions::followed_ended(daemon, &agent, &why);
    hand_over(daemon, &agent);
    agents::tell(daemon, id, Order::Stop);
    let (daemon, id) = (daemon.clone(), id.to_owned());
    tokio::spawn(async move { crate::runs::stop_runs_of(&daemon, &id).await });
    true
}

/// What follows the end of an agent's turn, for a child: a reported child ends; one that ended
/// its turn without reporting, with nothing to wait for, is reminded once, then its parent is
/// told it went idle.
pub fn after_turn(daemon: &Arc<Daemon>, id: &str, stop_reason: &str) {
    let Ok(Some(agent)) = daemon.read(|store| store.agent(id)) else {
        return;
    };
    if agent.state.is_final() {
        return;
    }
    if agent.parent.is_none() {
        crate::subscriptions::turn_ended(daemon, &agent);
        return;
    }
    if agent.reported_at.is_some() {
        stop_tree(daemon, id, "it reported");
        return;
    }
    // A cancelled turn is followed by what cancelled it.
    if stop_reason != "end_turn" {
        return;
    }
    let queued = daemon
        .read(|store| store.deliveries(id, "queued"))
        .unwrap_or_default();
    let asking = daemon
        .read(|store| store.pending_requests_of(id))
        .is_ok_and(|requests| !requests.is_empty());
    if !queued.is_empty() || asking || agent.waiting_on_answer {
        return;
    }
    let quiet = |turns| {
        let _ = daemon.write(|store| {
            let (_, event) = store.change_agent(
                id,
                AgentChange {
                    quiet_turns: Some(turns),
                    ..AgentChange::default()
                },
                None,
            )?;
            Ok(((), event))
        });
    };
    match agent.quiet_turns {
        0 => {
            quiet(1);
            deliveries::deliver(
                daemon,
                id,
                "reminder",
                Some(format!("reminder-{id}-{}", now_ms())),
                "End with report: your report is the only thing you return to your parent. If \
                 what you need is held for approval or asked of your parent and nothing else can \
                 be done, report what is missing.",
                approval::BETWEEN_TURNS,
            );
        }
        1 => {
            quiet(2);
            let last = last_messages(daemon, id)
                .pop()
                .map(|m| format!(" Its last message:\n\n{}", cut(&m, MESSAGE_CHARS)))
                .unwrap_or_default();
            tell_parent(
                daemon,
                &agent,
                "child-idle",
                &format!(
                    "ended two turns without reporting, even after a reminder, and waits.{last}"
                ),
                approval::BETWEEN_TURNS,
            );
        }
        _ => {}
    }
}

/// A child whose conversation was compacted gets its brief again: what it knew of its task was
/// in the part that went (`docs/reference/hemera.md` §5 "Activity signals").
pub fn compacted(daemon: &Arc<Daemon>, id: &str) {
    let Ok(Some(agent)) = daemon.read(|store| store.agent(id)) else {
        return;
    };
    if agent.parent.is_none() {
        return;
    }
    if let Some(brief) = &agent.brief {
        deliveries::deliver(
            daemon,
            id,
            "brief",
            Some(flottille_core::prompt::replay_id(&format!(
                "brief-{id}-{}-{}",
                agent.epoch,
                now_ms()
            ))),
            &format!("Your conversation was compacted. Your brief, again:\n\n{brief}"),
            approval::BETWEEN_TURNS,
        );
    }
}

/// A child reported, and its turn has not ended long after: it is stopped.
pub fn end_late_reporters(daemon: &Arc<Daemon>) {
    let late = now_ms().saturating_sub(REPORT_GRACE_MS);
    for agent in daemon.read(Store::live_agents).unwrap_or_default() {
        if agent.parent.is_some() && agent.reported_at.is_some_and(|at| at < late) {
            stop_tree(daemon, &agent.id, "it reported");
        }
    }
}

// The tree as the user's clients read it.

/// The root of the agent's tree.
pub fn root_of(daemon: &Daemon, id: &str) -> String {
    let mut id = id.to_owned();
    for _ in 0..=DEPTH_LIMIT + 8 {
        match daemon.read(|store| store.agent(&id)).ok().flatten() {
            Some(Agent {
                parent: Some(parent),
                ..
            }) => id = parent,
            _ => break,
        }
    }
    id
}

/// Whether the agent's session is kept for `agent.resume`: a root that ended (not one that failed:
/// its session could not go on) whose agent said it resumes or loads sessions, and that opened
/// one. Flottille forgets it `KEPT_MS` after the root ended (`forget_kept_sessions`).
pub fn keeps_session(agent: &Agent) -> bool {
    agent.state == AgentState::Ended
        && agent.parent.is_none()
        && agent.reopens
        && agent.native_id.is_some()
}

/// How long an ended root's session is kept to be resumed: 7 days.
pub const KEPT_MS: u64 = 7 * 24 * 3_600_000;

/// Why `agent.resume` could not start the ended agent again on its own session, `None` when it
/// could once its last process is gone (`agents::stopping`).
pub fn resume_refusal(daemon: &Daemon, agent: &Agent) -> Option<&'static str> {
    if agent.state == AgentState::Failed {
        return Some("it failed, and its session went with it");
    }
    if !keeps_session(agent) {
        return Some("its agent can neither resume nor load its session, or it was forgotten");
    }
    // Gone, or made again under the same name since: not the place the session worked in.
    let Ok(project) = projects::load_project(daemon, &agent.project) else {
        return Some("its project is gone");
    };
    match &agent.workspace {
        None if project.folder != agent.folder => Some("its project is gone"),
        None => None,
        Some(name) => match daemon.read(|store| store.workspace(&agent.project, name)) {
            Ok(Some(workspace)) if workspace.created_at <= agent.created_at => None,
            _ => Some("its workspace is gone"),
        },
    }
}

/// An agent as a list shows it: without its instructions and its brief.
pub fn summary(daemon: &Daemon, agent: &Agent) -> Value {
    json!({
        "id": agent.id,
        "provider": agent.provider,
        // What it runs on: what it took at its last start, else what was asked.
        "model": agent.taken.model.as_ref().or(agent.chosen.model.as_ref()),
        "effort": agent.taken.effort.as_ref().or(agent.chosen.effort.as_ref()),
        // What the user changed to, set before the next prompt: `null` when nothing waits.
        "pending": (agent.pending.model.is_some() || agent.pending.effort.is_some())
            .then(|| json!({"model": agent.pending.model, "effort": agent.pending.effort})),
        "role": agent.role,
        "title": agent.title,
        "project": agent.project,
        "workspace": agent.workspace,
        "parent": agent.parent,
        "launched_by": agent.launched_by,
        // The agent that leads it (`docs/tools.md`, "Sessions an agent starts"); `null` for a
        // session handed to the user.
        "led_by": agent.launched_by.as_ref().filter(|_| agent.led),
        // The agent the user detached it from, kept for the record (`docs/tools.md`, "Detach").
        "detached_from": agent.detached_from,
        "depth": agent.depth,
        "epoch": agent.epoch,
        "state": agent.state,
        "state_reason": agent.state_reason,
        "created_at": agent.created_at,
        "active_at": agent.active_at,
        "exchanged_at": agent.exchanged_at,
        "ended_at": agent.ended_at,
        "waiting_on_answer": agent.waiting_on_answer,
        "gate_mode": crate::judge::mode(daemon, &agent.id),
        "resumable": agent.state == AgentState::Ended && resume_refusal(daemon, agent).is_none(),
        "stopping": agent.state.is_final() && agents::stopping(daemon, &agent.id),
        "images": agent.images,
        // Its session runs an older list of tools than its role has: a restart gives the new one.
        "tools_outdated": daemon.mcp.outdated(&agent.id),
        // A reload of its tools waits for its next process (`agent.reload_tools`).
        "tools_reload": agents::reloading(daemon, &agent.id),
        // Flottille's compaction of it runs (`docs/design.md` §5.8): the compactor at work, or the
        // agent writing its handoff.
        "compacting": crate::compaction::running(daemon, &agent.id),
    })
}

/// What `agent.status` gives of one agent: what it is and does, what it waits on, its last
/// messages.
pub fn status_of(daemon: &Daemon, agent: &Agent) -> Result<Value, Failure> {
    let entry = tree(daemon, agent)?
        .into_iter()
        .find(|entry| entry["agent"]["id"] == agent.id.as_str())
        .unwrap_or_default();
    Ok(status_from(daemon, agent, &entry))
}

/// `status_of` for an agent whose entry of its tree is known.
fn status_from(daemon: &Daemon, agent: &Agent, entry: &Value) -> Value {
    json!({
        "agent": summary(daemon, agent),
        "name": named(agent),
        "line": line(daemon, agent),
        "doing": entry["doing"],
        "waiting_on": entry["waiting_on"],
        "needs_user": entry["needs_user"],
        "questions": entry["questions"],
        "messages": last_messages(daemon, &agent.id)
            .iter()
            .map(|message| cut(message, MESSAGE_CHARS))
            .collect::<Vec<_>>(),
    })
}

/// The root agents a page of `agent.trees` holds at most.
pub const TREES_PAGE: usize = 20;

/// A page of root agents, newest first, each with its status and its whole tree, in one answer:
/// the live ones, or all of them with `ended`; `before` reads on after the last root of the
/// page before. `more` says whether roots follow, `hidden` how many ended roots a live-only
/// listing leaves out.
pub fn trees(daemon: &Daemon, asked: &AgentTrees) -> Result<Value, Failure> {
    let mut roots: Vec<Agent> = daemon
        .read(Store::agents)?
        .into_iter()
        .filter(|agent| agent.parent.is_none())
        .filter(|agent| asked.project.as_ref().is_none_or(|p| &agent.project == p))
        .collect();
    // The store gives them oldest first.
    roots.reverse();
    if let Some(before) = &asked.before {
        // Found among all of them, not the listed ones: a root that ended since the page before
        // still marks the place.
        let at = roots
            .iter()
            .position(|root| &root.id == before)
            .ok_or_else(|| {
                Failure::refused(format!("there is no agent {before} among the root agents"))
            })?;
        roots.drain(..=at);
    }
    let hidden = if asked.ended {
        0
    } else {
        let all = roots.len();
        roots.retain(|root| !root.state.is_final());
        all - roots.len()
    };
    let more = roots.len() > TREES_PAGE;
    roots.truncate(TREES_PAGE);
    let mut trees = Vec::new();
    for root in &roots {
        let agents = tree(daemon, root)?;
        let entry = agents.first().cloned().unwrap_or_default();
        trees.push(json!({"status": status_from(daemon, root, &entry), "agents": agents}));
    }
    Ok(json!({"trees": trees, "more": more, "hidden": hidden}))
}

/// A root whose question waits for the user's answer; a root that ended waits for nothing.
pub fn asks_the_user(daemon: &Daemon, agent: &Agent) -> bool {
    agent.parent.is_none()
        && agent.waiting_on_answer
        && !agent.state.is_final()
        && !waits_on_pilot(daemon, agent)
}

/// The questions a led session asked its pilot that wait for its answer, the oldest first.
pub fn pilot_questions(daemon: &Daemon, agent: &Agent) -> Result<Vec<Value>, Failure> {
    if agent.parent.is_some() || !agent.waiting_on_answer || agent.state.is_final() {
        return Ok(Vec::new());
    }
    unanswered_to(daemon, &agent.id, "pilot")
}

/// Whether the root's last question went to its pilot rather than to the user.
fn waits_on_pilot(daemon: &Daemon, agent: &Agent) -> bool {
    agent.parent.is_none()
        && daemon
            .read(|store| store.last_agent_events(&agent.id, "agent.asked", 1))
            .is_ok_and(|asked| {
                asked
                    .first()
                    .is_some_and(|event| event.data["to"] == "pilot")
            })
}

/// The questions a root asked the user that wait for the answer, the oldest first, each with
/// when it was asked: none while it does not wait (`waiting_on_answer`), else [`unanswered`].
pub fn open_questions(daemon: &Daemon, agent: &Agent) -> Result<Vec<Value>, Failure> {
    if !asks_the_user(daemon, agent) {
        return Ok(Vec::new());
    }
    unanswered(daemon, &agent.id)
}

/// The questions the root asked the user (`agent.asked`) that nothing answered since: a question
/// is answered once the root took a message of the user written after it was asked, and none
/// asked before the root's session was replaced or resumed waits any more. The store alone says
/// it, whatever version wrote it: `agent.answered` only tells a client to read again.
pub fn unanswered(daemon: &Daemon, id: &str) -> Result<Vec<Value>, Failure> {
    unanswered_to(daemon, id, "user")
}

/// The same, of the questions the root asked `to` (`user`, or `pilot`, which answers with
/// `session_send`).
pub fn unanswered_to(daemon: &Daemon, id: &str, to: &str) -> Result<Vec<Value>, Failure> {
    let answers = match to {
        "pilot" => "session",
        _ => "user",
    };
    let last = |name: &str| {
        daemon.read(|store| {
            Ok(store
                .last_agent_events(id, name, 1)?
                .first()
                .map_or(0, |event| event.seq))
        })
    };
    let limit = last("agent.replaced")?.max(last("agent.resumed")?);
    let taken: Vec<_> = daemon
        .read(|store| store.last_deliveries_of(id, answers, 64))?
        .into_iter()
        .filter(|delivery| matches!(delivery.state.as_str(), "sending" | "sent"))
        .collect();
    // When the newest answer the root took was written: a message of the user's, or for a
    // question to its pilot, a `session_send` of that pilot, whose key names it.
    let answered = |pilot: Option<&str>| {
        taken
            .iter()
            .filter(|delivery| {
                to != "pilot"
                    || pilot
                        .is_some_and(|pilot| delivery.id.starts_with(&format!("session-{pilot}-")))
            })
            .map(|delivery| delivery.created_at)
            .max()
    };
    let mut asked = daemon.read(|store| store.last_agent_events(id, "agent.asked", 50))?;
    asked.reverse();
    // The questions moved to the user were asked of them again: the pilot's are closed.
    let moved = asked
        .iter()
        .filter(|event| !event.data["moved_from"].is_null())
        .map(|event| event.seq)
        .max()
        .unwrap_or(0);
    Ok(asked
        .into_iter()
        .filter(|event| event.seq > limit && event.data["to"] == to)
        .filter(|event| to != "pilot" || event.seq > moved)
        .filter(|event| {
            answered(event.data["pilot"].as_str()).is_none_or(|written| written <= event.at)
        })
        .map(|event| json!({"seq": event.seq, "at": event.at, "question": event.data["question"]}))
        .collect())
}

/// The root's questions no longer wait (`why`: its session was replaced or resumed): clients
/// read them again.
pub fn answered(daemon: &Daemon, id: &str, why: &str) {
    let _ = daemon.write(|store| {
        let (_, event) = store.change_agent(
            id,
            AgentChange::default(),
            Some(("agent.answered", json!({"agent": id, "why": why}))),
        )?;
        Ok(((), event))
    });
}

/// The agent's whole tree, from its root, each agent before its children: what it does, whom it
/// waits on (a child on its parent, never on the user), and for the root, whether a request of
/// its tree waits for the user.
pub fn tree(daemon: &Daemon, agent: &Agent) -> Result<Vec<Value>, Failure> {
    let root = root_of(daemon, &agent.id);
    let user_requests = daemon.read(|store| store.pending_requests(approval::USER))?;
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(agent) = daemon.read(|store| store.agent(&id))? else {
            continue;
        };
        let asking = !daemon
            .read(|store| store.pending_requests_of(&agent.id))?
            .is_empty();
        let waiting_on = match (&agent.parent, asking || agent.waiting_on_answer) {
            // Between turns with a subscription open: it waits for what happens outside.
            (_, false)
                if !matches!(agent.state, AgentState::Working | AgentState::Starting)
                    && !agent.state.is_final()
                    && crate::subscriptions::waiting_for(daemon, &agent.id).is_some() =>
            {
                "subscription".into()
            }
            (_, false) => Value::Null,
            (Some(_), true) => "parent".into(),
            (None, true) if !asking && waits_on_pilot(daemon, &agent) => "pilot".into(),
            (None, true) => "user".into(),
        };
        let needs_user = agent.parent.is_none()
            && (asks_the_user(daemon, &agent)
                || user_requests
                    .iter()
                    .any(|request| root_of(daemon, &request.agent) == agent.id));
        // What the user wrote to the root while it worked, not sent yet: the window shows it
        // waiting until the root reads it.
        let queued: Vec<Value> = match agent.parent {
            Some(_) => Vec::new(),
            None => daemon
                .read(|store| store.deliveries(&agent.id, "queued"))?
                .into_iter()
                .filter(|delivery| delivery.kind == "user")
                .map(|delivery| {
                    // What the user typed is shown; what the agent will receive unfolds.
                    let mut queued = json!({"delivery": delivery.id,
                        "text": delivery.typed.as_deref().unwrap_or(&delivery.body),
                        "images": delivery.images.len(), "pictures": delivery.images,
                        "at": delivery.created_at});
                    if delivery.typed.is_some() {
                        queued["commands"] = json!(delivery.commands);
                        queued["received"] = delivery.body.into();
                    }
                    queued
                })
                .collect(),
        };
        out.push(json!({
            "agent": summary(daemon, &agent),
            "doing": doing(daemon, &agent),
            "waiting_on": waiting_on,
            "needs_user": needs_user,
            "questions": open_questions(daemon, &agent)?,
            "context": context_used(daemon, &agent),
            "context_tokens": context_tokens(daemon, &agent).map(|(used, _)| used),
            "queued": queued,
        }));
        let children = daemon.read(|store| store.children(&agent.id))?;
        stack.extend(children.into_iter().rev().map(|child| child.id));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_said_in_its_unit() {
        assert_eq!(ago(40_000), "40 s");
        assert_eq!(ago(12 * 60_000), "12 min");
        assert_eq!(ago(3 * 3_600_000), "3 h");
    }

    #[test]
    fn a_conversation_line_keeps_what_a_parent_reads() {
        assert_eq!(
            conversation_line("agent.message", &json!({"text": "done"})).as_deref(),
            Some("said: done")
        );
        assert_eq!(
            conversation_line(
                "agent.tool_served",
                &json!({"tool": "fs_read", "outcome": "ok"})
            )
            .as_deref(),
            Some("fs_read (ok)")
        );
        assert!(conversation_line("agent.thought", &json!({"text": "hm"})).is_none());
        assert!(
            conversation_line("agent.tool_call", &json!({"update": true, "title": "x"})).is_none()
        );
    }
}
