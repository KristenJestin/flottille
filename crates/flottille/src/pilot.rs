//! The pilot door, `flottille mcp` (`docs/cli.md`, `docs/reference/mcp.md` §4): an MCP server on
//! stdio through which an agent running outside Flottille drives it. It is a client of the
//! daemon's socket, like the command line: it starts the daemon when nobody listens, and each
//! call asks the daemon what it needs. It serves its own tools, never Flottille's file or command
//! tools, and none that answers an approval: those stay the user's.
//!
//! Both protocol revisions are served and no state lives in the protocol session: what a call
//! needs across calls is an agent id or an event number the caller holds.

use std::time::Duration;

use flottille_core::paths::Paths;
use flottille_core::protocol::{
    AgentName, AgentRun, AgentSend, AgentTrees, WatchParams, WorkspaceCreate,
};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::VERSION;
use crate::approval_command;
use crate::client::{Client, DaemonError};
use crate::deliveries;
use crate::mcp::{Definition, ERROR_LIMIT, Outcome, RESULT_LIMIT, arguments, cut, tool_of};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The longest a call waits for an event: under Claude Code's two minutes, after which a call
/// moves to a background task.
const WAIT_MAX: u64 = 90;
/// The events `events_since` gives at once, at most.
const EVENTS_PAGE: usize = 100;
/// The room the body of a result takes once shown (escaped), what is left of `RESULT_LIMIT` being
/// the line that says how to read on, which is never cut.
const ROOM: usize = 36_000;
/// What a line of an event keeps of each text of its data, and of its whole.
const EVENT_TEXT: usize = 500;
const EVENT_LINE: usize = 2_000;
/// How long a list of tools may be cached: it never changes.
const LIST_TTL_MS: u64 = 24 * 3600 * 1000;

const INSTRUCTIONS: &str = "\
Flottille runs coding agents on the user's own machine: a daemon owns their projects, their \
workspaces (git worktrees) and their agents. These tools drive it from outside, as the user's \
pilot. Reach for them to hand a piece of work to a Flottille agent and to follow it, rather \
than doing it in this session.

How the work goes:
- projects_list and workspaces_list say where work can happen. workspace_create makes a new \
workspace (its own branch in every repository it takes); its preparation goes on in the \
background, so wait until workspaces_list says it is ready.
- agent_start starts a root agent in a project (its folder, or a ready workspace) with its \
first message. It answers at once, with the agent's id: the agent works on its own, and may \
launch sub-agents of its own.
- agent_send writes to a root agent, which plays the message after the turn that runs. A \
sub-agent belongs to its parent: agent_send and agent_stop refuse it, and name its root.
- agent_status says what an agent does now; agent_read pages the conversation of any agent of \
a tree; agents_list shows the trees. Both list the live root agents, newest first, a page at a \
time: ended: true adds the ended ones, and a page says how to read on.
- agent_stop stops a root agent and its whole tree.

Following without polling: events_since {after, wait_seconds} returns the events after an \
event number, or waits up to wait_seconds (at most 90) for the next one, and always says the \
number to ask next. agent_start and agent_send answer with a number to begin from. A root \
agent's report and its questions to the user come as events (agent.reported, agent.asked), \
and so does a failure (agent.failed).

What stays the user's: an agent's request to do something the gate holds (reading a secret \
file, running a command that is not allowed) waits for the user's answer. approvals_list shows \
what waits; the user answers with `flottille approve` or `flottille deny`. This door has no \
way to answer one, and a refusal is not to be worked around: tell the user what waits.

Agents are named by agent_id, twelve hexadecimal digits. A refusal says what to do next.";

/// The door's tools, in the order the client lists them.
const TOOLS: [Definition; 11] = [
    Definition {
        name: "projects_list",
        title: "List the projects",
        description: "Lists the projects Flottille knows: name, folder and repositories. Use \
            it first, to know the name a project is started with. Projects are registered by \
            the user (flottille project add).",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || json!({"type": "object", "properties": {}, "additionalProperties": false}),
    },
    Definition {
        name: "workspaces_list",
        title: "List the workspaces",
        description: "Lists the workspaces (a worktree per repository, on one branch): name, \
            state (preparing, ready or failed), branch and folder. An agent can start in a \
            workspace only when it is ready. All of them, or one project's.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "project": {"type": "string", "description": "Only this project's."}
            }, "additionalProperties": false})
        },
    },
    Definition {
        name: "workspace_create",
        title: "Make a workspace",
        description: "Makes a workspace for a piece of work: fetches the base of each \
            repository and makes a worktree on a new branch; the project's setup then goes on \
            in the background (workspaces_list says when it is ready). Refused when the name \
            or the branch is taken, or a fetch fails: nothing is made then.",
        read_only: false,
        idempotent: false,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "project": {"type": "string"},
                "name": {"type": "string", "description": "The workspace's name: letters, digits, - and _."},
                "repositories": {"type": "array", "items": {"type": "string"}, "description": "The repositories it takes, by path; the project's default set when left out."},
                "branch": {"type": "string", "description": "<branch prefix>/<name> when left out."},
                "offline": {"type": "boolean", "description": "Fetch nothing: start from the tracking refs as they are."}
            }, "required": ["project", "name"], "additionalProperties": false})
        },
    },
    Definition {
        name: "agent_start",
        title: "Start a root agent",
        description: "Starts a root agent in a project (its folder, or a ready workspace) and \
            gives it its first message; answers at once with its id and an event number to \
            follow it from. The agent works on its own and may launch sub-agents. Write the \
            message as a brief: the goal, the place, what done means. Refused when the \
            workspace is not ready, the message is empty or the agent cannot run.",
        read_only: false,
        idempotent: false,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "project": {"type": "string"},
                "message": {"type": "string", "description": "Its first message."},
                "workspace": {"type": "string", "description": "A ready workspace of the project; the project folder itself when left out."},
                "provider": {"type": "string", "description": "The provider it runs on, for life; the project's when left out."},
                "model": {"type": "string", "description": "The project's, else the provider's default, when left out."},
                "effort": {"type": "string", "description": "The model's effort; its default when left out."},
                "role": {"type": "string", "enum": ["lead", "worker"], "description": "lead when left out."}
            }, "required": ["project", "message"], "additionalProperties": false})
        },
    },
    Definition {
        name: "agent_send",
        title: "Write to a root agent",
        description: "Writes a message to a root agent: it plays after the turn that runs, if \
            one does, and starts the agent's process again when it was let go. Only root \
            agents: a sub-agent belongs to its parent and this is refused, naming its root. \
            Refused too for an agent that ended or failed, and for an empty message.",
        read_only: false,
        idempotent: false,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "agent_id": {"type": "string"},
                "message": {"type": "string"}
            }, "required": ["agent_id", "message"], "additionalProperties": false})
        },
    },
    Definition {
        name: "agent_status",
        title: "Say what an agent does",
        description: "Says what one agent does now (in a turn, running a command, waiting for \
            an approval or for its parent, idle), how long since its last sign of activity, \
            its context use and its last messages. Without agent_id: the live root agents, \
            newest first, 20 at a time (ended adds the ended ones; the page says how to read \
            on with before). For the conversation itself use agent_read.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "agent_id": {"type": "string", "description": "The live root agents when left out."},
                "ended": {"type": "boolean", "description": "Also the root agents that ended or failed (without agent_id)."},
                "before": {"type": "string", "description": "Read on after this root agent, the last of the page before (without agent_id)."}
            }, "additionalProperties": false})
        },
    },
    Definition {
        name: "agent_read",
        title: "Read an agent's conversation",
        description: "Reads a page of an agent's conversation: what it was sent, what it said, \
            its tool calls, its reports. Any agent of a tree, a sub-agent too. A page ends \
            with how to read on (after). Use agent_status for the state alone.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "agent_id": {"type": "string"},
                "after": {"type": "integer", "minimum": 0, "description": "Read what follows this event number; the start when left out."}
            }, "required": ["agent_id"], "additionalProperties": false})
        },
    },
    Definition {
        name: "agent_stop",
        title: "Stop a root agent",
        description: "Stops a root agent and its whole tree, the deepest first; their \
            workspaces stay. An agent that ended cannot be started again: start a new one. \
            Only root agents: a sub-agent is stopped by its parent, or with its root, and \
            this is refused, naming the root.",
        read_only: false,
        idempotent: false,
        destructive: true,
        schema: || {
            json!({"type": "object", "properties": {
                "agent_id": {"type": "string"}
            }, "required": ["agent_id"], "additionalProperties": false})
        },
    },
    Definition {
        name: "agents_list",
        title: "List the agents as trees",
        description: "Lists the agents as trees, each root followed by the sub-agents it \
            launched: id, title, role, project, and what it does now. The live roots, newest \
            first, 20 at a time (ended adds the ended ones; the page says how to read on with \
            before), one project's, or the tree of one agent's root. Use agent_status for \
            detail.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "project": {"type": "string", "description": "Only this project's."},
                "agent_id": {"type": "string", "description": "Only the tree of this agent's root."},
                "ended": {"type": "boolean", "description": "Also the root agents that ended or failed."},
                "before": {"type": "string", "description": "Read on after this root agent, the last of the page before."}
            }, "additionalProperties": false})
        },
    },
    Definition {
        name: "approvals_list",
        title: "List the user's pending requests",
        description: "Lists the requests of agents that wait for the user's answer: which \
            agent, which call, why it was held. They are the user's alone: this door cannot \
            answer them, tell the user what waits (flottille approve or deny).",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || json!({"type": "object", "properties": {}, "additionalProperties": false}),
    },
    Definition {
        name: "events_since",
        title: "Read the events since a number",
        description: "Reads the daemon's events after an event number, oldest first, and says \
            the number to ask next. With wait_seconds (at most 90) and nothing new, it waits \
            for the next event instead of answering empty, and answers empty if none comes: \
            the way to follow agents without polling. after 0 reads from the start.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "after": {"type": "integer", "minimum": 0, "description": "The number of the last event already seen."},
                "wait_seconds": {"type": "integer", "minimum": 0, "maximum": WAIT_MAX, "description": "How long to wait for the next event when there is none yet."}
            }, "required": ["after"], "additionalProperties": false})
        },
    },
];

/// Serves the door on this process's standard input and output until the client leaves.
pub fn run(paths: &Paths) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let served: Result<()> = runtime.block_on(async {
        let running = Pilot {
            paths: paths.clone(),
        }
        .serve(rmcp::transport::io::stdio_pipes())
        .await?;
        running.waiting().await?;
        Ok(())
    });
    // A call still waiting on the daemon must not keep the process once its client is gone.
    runtime.shutdown_background();
    served
}

#[derive(Clone)]
struct Pilot {
    paths: Paths,
}

impl ServerHandler for Pilot {
    fn get_info(&self) -> ServerConfig {
        let capabilities: ServerCapabilities =
            serde_json::from_value(json!({"tools": {"listChanged": false}})).expect("capabilities");
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("flottille", VERSION))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        // Found through tool search by the outside agent: none is loaded upfront.
        let tools = TOOLS
            .iter()
            .map(|definition| tool_of(definition, false))
            .collect();
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(LIST_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let paths = self.paths.clone();
        let name = request.name.to_string();
        // The socket is the command line's blocking client: off the reactor.
        let outcome = tokio::task::spawn_blocking(move || play(&paths, &name, args))
            .await
            .unwrap_or_else(|error| Outcome::Failed(error.to_string()));
        let (text, error) = shown(outcome);
        let content = vec![ContentBlock::text(text)];
        Ok(if error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        }
        .into())
    }
}

/// The characters a text takes once `shown`: escaped.
fn weight(text: &str) -> usize {
    deliveries::escape(text).chars().count()
}

/// What the caller reads of an outcome, and whether it is an error: escaped (what an agent said
/// or a file held never forges a marker) and cut under its limit.
///
/// Nothing is masked of secrets again here: the daemon masks the texts it stores (an agent's
/// messages, reports and questions, the arguments and reasons of a request, a failure's output;
/// `docs/events.md`), and the door only shows what the daemon answers, so there is nothing
/// unmasked left to leak.
fn shown(outcome: Outcome) -> (String, bool) {
    let (text, limit, error) = match outcome {
        Outcome::Done(text) | Outcome::Held(text) => (text, RESULT_LIMIT, false),
        Outcome::Refused(text) => (text, ERROR_LIMIT, true),
        Outcome::Failed(text) => (format!("the call failed: {text}"), ERROR_LIMIT, true),
    };
    (cut(&deliveries::escape(&text), limit), error)
}

fn refused(text: impl Into<String>) -> Outcome {
    Outcome::Refused(text.into())
}

/// A daemon's answer that is an error: a refusal, with its sentence to act on, when the request
/// was wrong or cannot be done as asked; a failure when the daemon could not be reached or could
/// not do it.
fn failure(error: Box<dyn std::error::Error>) -> Outcome {
    match error.downcast_ref::<DaemonError>() {
        Some(DaemonError { code, message })
            if matches!(
                code.as_str(),
                "refused"
                    | "not-found"
                    | "invalid-params"
                    | "invalid-request"
                    | "invalid-config"
                    | "unknown-method"
            ) =>
        {
            Outcome::Refused(format!("refused: {message}"))
        }
        _ => Outcome::Failed(error.to_string()),
    }
}

type Done = std::result::Result<String, Outcome>;

fn play(paths: &Paths, tool: &str, args: Value) -> Outcome {
    let done = match tool {
        "projects_list" => projects_list(paths),
        "workspaces_list" => workspaces_list(paths, args),
        "workspace_create" => workspace_create(paths, args),
        "agent_start" => agent_start(paths, args),
        "agent_send" => agent_send(paths, args),
        "agent_status" => agent_status(paths, args),
        "agent_read" => agent_read(paths, args),
        "agent_stop" => agent_stop(paths, args),
        "agents_list" => agents_list(paths, args),
        "approvals_list" => approvals_list(paths),
        "events_since" => events_since(paths, args),
        other => Err(refused(format!(
            "refused: this door has no tool {other}: its tools are {}",
            TOOLS.map(|tool| tool.name).join(", ")
        ))),
    };
    match done {
        Ok(text) => Outcome::Done(text),
        Err(outcome) => outcome,
    }
}

/// A request to the daemon, which is started when nobody listens.
fn ask(
    paths: &Paths,
    method: &str,
    params: impl serde::Serialize,
) -> std::result::Result<Value, Outcome> {
    let params =
        serde_json::to_value(params).map_err(|error| Outcome::Failed(error.to_string()))?;
    let mut client = Client::connect_or_start(paths).map_err(failure)?;
    client.request(method, params).map_err(failure)
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn place(value: &Value) -> String {
    match value["workspace"].as_str() {
        Some(workspace) => format!("{}/{workspace}", text(value, "project")),
        None => format!("{} (the project folder)", text(value, "project")),
    }
}

fn projects_list(paths: &Paths) -> Done {
    let listed = ask(paths, "project.list", Value::Null)?;
    let mut out = String::new();
    for project in listed["projects"].as_array().into_iter().flatten() {
        let repositories: Vec<&str> = project["repositories"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["path"].as_str())
            .collect();
        out.push_str(&format!(
            "- {}: {} (repositories: {})\n",
            text(project, "name"),
            text(project, "folder"),
            repositories.join(", ")
        ));
    }
    if out.is_empty() {
        out.push_str(
            "No project is registered yet: the user registers one with `flottille project add \
             <folder>`.\n",
        );
    }
    for error in listed["errors"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "Unusable: {} ({}): {}\n",
            text(error, "name"),
            text(error, "file"),
            text(error, "message")
        ));
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    project: Option<String>,
}

fn workspaces_list(paths: &Paths, args: Value) -> Done {
    let asked: Project = arguments("workspaces_list", args)?;
    let listed = ask(paths, "workspace.list", json!({"project": asked.project}))?;
    let mut out = String::new();
    for workspace in listed["workspaces"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "- {}/{}: {}, branch {}, folder {}\n",
            text(workspace, "project"),
            text(workspace, "name"),
            text(workspace, "state"),
            text(workspace, "branch"),
            text(workspace, "folder")
        ));
        for step in workspace["steps"].as_array().into_iter().flatten() {
            if step["state"] == "failed" {
                out.push_str(&format!(
                    "  failed: {}\n",
                    step["failure"]["doing"].as_str().unwrap_or("a step")
                ));
            }
        }
    }
    if out.is_empty() {
        out.push_str("No workspace yet: workspace_create makes one.\n");
    }
    Ok(out)
}

fn workspace_create(paths: &Paths, args: Value) -> Done {
    let asked: WorkspaceCreate = arguments("workspace_create", args)?;
    let made = ask(paths, "workspace.create", &asked)?;
    Ok(format!(
        "Made workspace {}/{} on branch {} in {}; its state is {}. Its preparation goes on in \
         the background: workspaces_list says when it is ready, and an agent can start in it \
         then.\n",
        text(&made, "project"),
        text(&made, "name"),
        text(&made, "branch"),
        text(&made, "folder"),
        text(&made, "state")
    ))
}

/// What `agent_start` takes, its schema's fields and no other: the daemon's own `AgentRun` also
/// has a mode, which an outside agent must not choose.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    project: String,
    message: String,
    workspace: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    role: Option<String>,
}

fn agent_start(paths: &Paths, args: Value) -> Done {
    let Start {
        project,
        message,
        workspace,
        provider,
        model,
        effort,
        role,
    } = arguments("agent_start", args)?;
    let asked = AgentRun {
        project,
        workspace,
        provider,
        model,
        effort,
        mode: None,
        role,
        message,
    };
    let started = ask(paths, "agent.start", &asked)?;
    let agent = &started["agent"];
    let id = text(agent, "id");
    let after = &started["after"];
    Ok(format!(
        "Started agent {id} ({}) in {}. It works on its own now. Follow it with agent_status \
         {{\"agent_id\": \"{id}\"}}, read it with agent_read {{\"agent_id\": \"{id}\", \"after\": \
         {after}}}, or wait for what happens with events_since {{\"after\": {after}, \
         \"wait_seconds\": 60}}.",
        text(agent, "role"),
        place(agent)
    ))
}

fn agent_send(paths: &Paths, args: Value) -> Done {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Send {
        agent_id: String,
        message: String,
    }
    let asked: Send = arguments("agent_send", args)?;
    let sent = ask(
        paths,
        "agent.send",
        AgentSend {
            agent: asked.agent_id.clone(),
            message: asked.message,
            images: Vec::new(),
        },
    )
    .map_err(|outcome| unknown_agent(outcome, &asked.agent_id))?;
    let after = &sent["after"];
    Ok(format!(
        "Sent to agent {}: it plays after the turn that runs, if one does. Read what follows \
         with agent_read {{\"agent_id\": \"{}\", \"after\": {after}}}.",
        asked.agent_id, asked.agent_id
    ))
}

/// A refusal for an agent that does not exist says where the agents are listed.
fn unknown_agent(outcome: Outcome, id: &str) -> Outcome {
    match outcome {
        Outcome::Refused(text) if text.contains(&format!("there is no agent {id}")) => refused(
            format!("{text}: agents_list shows the agents and their ids"),
        ),
        outcome => outcome,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Agent {
    agent_id: String,
}

fn agent_stop(paths: &Paths, args: Value) -> Done {
    let asked: Agent = arguments("agent_stop", args)?;
    let stopped = ask(
        paths,
        "agent.stop",
        AgentName {
            agent: asked.agent_id.clone(),
            after: None,
        },
    )
    .map_err(|outcome| unknown_agent(outcome, &asked.agent_id))?;
    Ok(format!(
        "Stopped agent {} and its tree. It is {} now; its workspace stays.",
        asked.agent_id,
        text(&stopped["agent"], "state")
    ))
}

fn agent_read(paths: &Paths, args: Value) -> Done {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Read {
        agent_id: String,
        after: Option<u64>,
    }
    let asked: Read = arguments("agent_read", args)?;
    let page = ask(
        paths,
        "agent.read",
        AgentName {
            agent: asked.agent_id.clone(),
            after: asked.after,
        },
    )
    .map_err(|outcome| unknown_agent(outcome, &asked.agent_id))?;
    let lines: Vec<&str> = page["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut out = format!("The conversation of {}:\n", text(&page, "name"));
    if lines.is_empty() {
        out.push_str("(nothing after this point)");
    } else {
        out.push_str(&lines.join("\n"));
    }
    let after = &page["after"];
    if page["more"] == true {
        out.push_str(&format!(
            "\n\nMore follows: agent_read {{\"agent_id\": \"{}\", \"after\": {after}}}",
            asked.agent_id
        ));
    } else {
        out.push_str(&format!(
            "\n\nThat is all for now; later, read on with agent_read {{\"agent_id\": \"{}\", \
             \"after\": {after}}}.",
            asked.agent_id
        ));
    }
    Ok(out)
}

/// An agent as `agent_status` shows it.
fn status_block(status: &Value) -> String {
    let name = text(status, "name");
    let line = text(status, "line");
    let mut out = format!(
        "- {name}, {} in {}: {}",
        text(&status["agent"], "role"),
        place(&status["agent"]),
        line.strip_prefix(&format!("{name}, ")).unwrap_or(line)
    );
    match status["waiting_on"].as_str() {
        Some("user") => out.push_str("\n  waits for the user"),
        Some(on) => out.push_str(&format!("\n  waits for its {on}")),
        None => {}
    }
    if status["needs_user"] == true {
        out.push_str("\n  a request of its tree waits for the user: approvals_list");
    }
    for message in status["messages"].as_array().into_iter().flatten() {
        out.push_str(&format!("\n  > {}", message.as_str().unwrap_or_default()));
    }
    out
}

/// What a listing of root agents asked, to say it again with a cursor.
struct Listing {
    tool: &'static str,
    project: Option<String>,
    ended: bool,
}

impl Listing {
    /// The call that reads on after a root, or from the newest.
    fn call(&self, before: Option<&str>, ended: bool) -> String {
        let mut parts = Vec::new();
        if let Some(before) = before {
            parts.push(format!("\"before\":{}", json!(before)));
        }
        if ended {
            parts.push("\"ended\":true".to_owned());
        }
        if let Some(project) = &self.project {
            parts.push(format!("\"project\":{}", json!(project)));
        }
        format!("{} {{{}}}", self.tool, parts.join(","))
    }
}

/// A page of root agents (the answer of `agent.trees`), each as `show` says, newest first, as
/// many as fit, and how to read on.
fn roots_text(answer: &Value, listing: &Listing, show: fn(&Value) -> String) -> String {
    let trees = answer["trees"].as_array().map_or(&[][..], Vec::as_slice);
    let hidden = answer["hidden"].as_u64().unwrap_or(0);
    let ended = if listing.ended || hidden == 0 {
        String::new()
    } else {
        let (agents, them) = match hidden {
            1 => ("agent is", "it"),
            _ => ("agents are", "them"),
        };
        format!(
            "{hidden} ended root {agents} not shown: {} lists {them}.",
            listing.call(None, true)
        )
    };
    if trees.is_empty() {
        return if hidden > 0 && !listing.ended {
            format!("No live root agent. {ended}")
        } else {
            "No agent yet: agent_start starts one.".into()
        };
    }
    let mut out = String::new();
    let mut size = 0;
    let mut kept = 0;
    for tree in trees {
        let mut block = show(tree);
        let mut weight = weight(&block);
        if kept > 0 && size + weight > ROOM {
            break;
        }
        if weight > ROOM {
            // A tree alone larger than the room: escaping grows a text by a fifth at most.
            block = cut(&block, ROOM / 6 * 5);
            weight = self::weight(&block);
        }
        size += weight + 1;
        out.push_str(&block);
        out.push('\n');
        kept += 1;
    }
    let last = trees[kept - 1]["status"]["agent"]["id"]
        .as_str()
        .unwrap_or_default();
    if kept < trees.len() || answer["more"] == true {
        if !ended.is_empty() {
            out.push_str(&format!("\n{ended}"));
        }
        out.push_str(&format!(
            "\nNewest first; more root agents follow: {}",
            listing.call(Some(last), listing.ended)
        ));
    } else if ended.is_empty() {
        out.push_str("\nThat is every root agent.");
    } else {
        out.push_str(&format!("\nThat is every live root agent. {ended}"));
    }
    out
}

/// A root's status, as `agent_status` shows it.
fn status_of_tree(tree: &Value) -> String {
    status_block(&tree["status"])
}

/// A tree of agents, a line each, deeper agents indented.
fn tree_block(tree: &Value) -> String {
    let mut lines = Vec::new();
    for entry in tree["agents"].as_array().into_iter().flatten() {
        let agent = &entry["agent"];
        let named = match agent["title"].as_str() {
            Some(title) => format!("\"{title}\" (agent {})", text(agent, "id")),
            None => format!("agent {}", text(agent, "id")),
        };
        let mut line = format!(
            "{}- {named}, {} in {}: {}",
            "  ".repeat(agent["depth"].as_u64().unwrap_or(0) as usize),
            text(agent, "role"),
            place(agent),
            text(entry, "doing")
        );
        if entry["needs_user"] == true {
            line.push_str("; a request waits for the user");
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn agent_status(paths: &Paths, args: Value) -> Done {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Status {
        agent_id: Option<String>,
        #[serde(default)]
        ended: bool,
        before: Option<String>,
    }
    let asked: Status = arguments("agent_status", args)?;
    if let Some(id) = &asked.agent_id {
        if asked.ended || asked.before.is_some() {
            return Err(refused(
                "refused: ended and before page through the root agents, and agent_id names one \
                 agent: leave agent_id out to list them, or leave ended and before out",
            ));
        }
        let status = ask(
            paths,
            "agent.status",
            AgentName {
                agent: id.clone(),
                after: None,
            },
        )
        .map_err(|outcome| unknown_agent(outcome, id))?;
        return Ok(status_block(&status));
    }
    let listing = Listing {
        tool: "agent_status",
        project: None,
        ended: asked.ended,
    };
    let answer = trees(paths, &listing, asked.before)?;
    Ok(roots_text(&answer, &listing, status_of_tree))
}

/// One round trip: a page of root agents with their trees.
fn trees(
    paths: &Paths,
    listing: &Listing,
    before: Option<String>,
) -> std::result::Result<Value, Outcome> {
    ask(
        paths,
        "agent.trees",
        AgentTrees {
            project: listing.project.clone(),
            ended: listing.ended,
            before: before.clone(),
        },
    )
    .map_err(|outcome| match &before {
        Some(id) => unknown_agent(outcome, id),
        None => outcome,
    })
}

fn agents_list(paths: &Paths, args: Value) -> Done {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct List {
        project: Option<String>,
        agent_id: Option<String>,
        #[serde(default)]
        ended: bool,
        before: Option<String>,
    }
    let asked: List = arguments("agents_list", args)?;
    if let Some(id) = &asked.agent_id {
        if asked.project.is_some() || asked.ended || asked.before.is_some() {
            return Err(refused(
                "refused: agent_id names the one tree to show: leave project, ended and before \
                 out, or leave agent_id out to list the trees",
            ));
        }
        let tree = ask(
            paths,
            "agent.tree",
            AgentName {
                agent: id.clone(),
                after: None,
            },
        )
        .map_err(|outcome| unknown_agent(outcome, id))?;
        return Ok(format!(
            "{}\n\nagent_status says more of one agent, agent_read reads its conversation.",
            tree_block(&tree)
        ));
    }
    let listing = Listing {
        tool: "agents_list",
        project: asked.project,
        ended: asked.ended,
    };
    let answer = trees(paths, &listing, asked.before)?;
    let mut out = roots_text(&answer, &listing, tree_block);
    out.push_str("\nagent_status says more of one agent, agent_read reads its conversation.");
    Ok(out)
}

fn approvals_list(paths: &Paths) -> Done {
    let listed = ask(paths, "approval.list", Value::Null)?;
    let requests = listed["requests"].as_array().map_or(&[][..], Vec::as_slice);
    if requests.is_empty() {
        return Ok("No request waits for the user.".into());
    }
    Ok(format!(
        "{}These requests are the user's alone to answer, with `flottille approve <n>` or \
         `flottille deny <n>`; this door has no way to. Tell the user what waits.",
        approval_command::request_lines(requests)
    ))
}

/// Every text of the data cut at `EVENT_TEXT` characters.
fn short(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(cut(text, EVENT_TEXT)),
        Value::Array(items) => Value::Array(items.iter().map(short).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), short(value)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

fn event_line(event: &flottille_core::protocol::Event) -> String {
    cut(
        &format!("#{} {} {}", event.seq, event.name, short(&event.data)),
        EVENT_LINE,
    )
}

fn events_since(paths: &Paths, args: Value) -> Done {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Since {
        after: u64,
        wait_seconds: Option<u64>,
    }
    let asked: Since = arguments("events_since", args)?;
    let wait = asked.wait_seconds.unwrap_or(0);
    if wait > WAIT_MAX {
        return Err(refused(format!(
            "refused: wait_seconds is at most {WAIT_MAX}, not {wait}: ask again with \
             wait_seconds {WAIT_MAX} or less, as often as needed"
        )));
    }
    let after = asked.after;
    // Subscribed first, so that an event stored while the other end is asked comes both ways and
    // none is lost; the second subscription, with no number, answers with the last event's.
    let mut watcher = Client::connect_or_start(paths).map_err(failure)?;
    watcher
        .request("events.watch", json!(WatchParams { after: Some(after) }))
        .map_err(failure)?;
    let last = Client::connect_or_start(paths)
        .and_then(|mut probe| probe.request("events.watch", Value::Null))
        .map_err(failure)?["after"]
        .as_u64()
        .unwrap_or(after);

    if after > last {
        return Err(refused(format!(
            "refused: there is no event {after} yet, the last is #{last}: ask again with \
             events_since {{\"after\": {last}, \"wait_seconds\": {WAIT_MAX}}} to wait for what \
             comes next"
        )));
    }

    let mut events = Vec::new();
    if last > after {
        // Stored events: those up to the last one, not further.
        while events.len() < EVENTS_PAGE {
            let Some(event) = watcher
                .next_event_within(Duration::from_secs(10))
                .map_err(|error| Outcome::Failed(error.to_string()))?
            else {
                break;
            };
            let seq = event.seq;
            events.push(event);
            if seq >= last {
                break;
            }
        }
    } else if wait > 0 {
        let event = watcher
            .next_event_within(Duration::from_secs(wait))
            .map_err(|error| Outcome::Failed(error.to_string()))?;
        events.extend(event);
    }

    Ok(events_text(&events, after, last))
}

/// The events as lines, as many as fit, and where to go on from.
fn events_text(events: &[flottille_core::protocol::Event], after: u64, last: u64) -> String {
    if events.is_empty() {
        return format!(
            "No event after {after} yet. Ask again with events_since {{\"after\": {after}, \
             \"wait_seconds\": {WAIT_MAX}}} to wait for the next one."
        );
    }
    let mut lines = Vec::new();
    let mut size = 0;
    let mut cursor = after;
    let mut more = false;
    for event in events {
        let line = event_line(event);
        // What the caller will read is this line escaped (`shown`): measured so, since escaping
        // makes a marker-dense text grow, and the cursor below must stay inside the limit.
        let weight = weight(&line);
        if size + weight > ROOM && !lines.is_empty() {
            more = true;
            break;
        }
        size += weight + 1;
        lines.push(line);
        cursor = event.seq;
    }
    more |= cursor < last && events.len() >= EVENTS_PAGE;
    let mut out = lines.join("\n");
    if more {
        out.push_str(&format!(
            "\n\nMore events follow: events_since {{\"after\": {cursor}}}"
        ));
    } else {
        out.push_str(&format!(
            "\n\nThat is all for now; later, ask events_since {{\"after\": {cursor}, \
             \"wait_seconds\": {WAIT_MAX}}}. agent_read reads a conversation better than its \
             events."
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instructions_fit_what_claude_code_reads_of_them() {
        assert!(INSTRUCTIONS.len() < 4_096, "{}", INSTRUCTIONS.len());
    }

    #[test]
    fn every_tool_is_described_for_a_new_colleague_and_none_answers_an_approval() {
        for definition in &TOOLS {
            let tool = serde_json::to_value(tool_of(definition, false)).unwrap();
            assert!(tool["description"].as_str().unwrap().len() < 1_000);
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(!definition.name.contains("answer"));
            assert!(!["approve", "deny"].contains(&definition.name));
            assert!(format!("mcp__flottille__{}", definition.name).len() < 128);
        }
    }

    #[test]
    fn a_page_of_events_stays_under_the_result_limit_and_says_where_to_go_on() {
        let events: Vec<_> = (1..=EVENTS_PAGE as u64)
            .map(|seq| flottille_core::protocol::Event {
                seq,
                at: 0,
                name: "agent.tool_call".into(),
                data: json!({"items": (0..8).map(|_| "é".repeat(400)).collect::<Vec<_>>()}),
            })
            .collect();
        let text = events_text(&events, 0, 500);
        assert!(
            text.chars().count() <= RESULT_LIMIT,
            "{}",
            text.chars().count()
        );
        let (_, rest) = text
            .rsplit_once("More events follow: events_since {\"after\": ")
            .unwrap();
        let cursor: u64 = rest.trim_end_matches('}').parse().unwrap();
        assert!(cursor > 0 && cursor < EVENTS_PAGE as u64);
        assert!(text.contains(&format!("#{cursor} ")));
        assert!(!text.contains(&format!("#{} ", cursor + 1)));
    }

    #[test]
    fn no_event_is_said_with_the_way_to_wait_for_one() {
        let text = events_text(&[], 12, 12);
        assert!(text.starts_with("No event after 12 yet."));
        assert!(text.contains("\"after\": 12,"));
    }

    #[test]
    fn a_long_text_of_an_event_is_cut() {
        let event = flottille_core::protocol::Event {
            seq: 7,
            at: 0,
            name: "agent.message".into(),
            data: json!({"agent": "a", "text": "é".repeat(5_000)}),
        };
        let line = event_line(&event);
        assert!(line.starts_with("#7 agent.message "));
        assert!(line.chars().count() < 700, "{}", line.chars().count());
        assert!(line.contains("‹+"));
    }

    #[test]
    fn a_page_of_events_dense_in_markers_still_ends_with_its_cursor_once_shown() {
        // What an agent said can be all markers: shown escaped, each grows, and the cursor line
        // after them must still be there.
        let dense = |seq: u64, marker: &str| flottille_core::protocol::Event {
            seq,
            at: 0,
            name: "agent.message".into(),
            data: json!({"text": marker.repeat(500 / marker.len())}),
        };
        for marker in ["<flottille-note", "[flottille:", "</flottille-note"] {
            let events: Vec<_> = (1..=EVENTS_PAGE as u64).map(|n| dense(n, marker)).collect();
            let (text, error) = shown(Outcome::Done(events_text(&events, 0, 500)));
            assert!(!error);
            assert!(
                text.chars().count() <= RESULT_LIMIT,
                "{marker}: {}",
                text.chars().count()
            );
            let (_, rest) = text
                .rsplit_once("More events follow: events_since {\"after\": ")
                .unwrap_or_else(|| panic!("{marker}: the cursor was cut"));
            let cursor: u64 = rest.trim_end_matches('}').parse().unwrap();
            assert!(text.contains(&format!("#{cursor} ")), "{marker}");
            assert!(!text.contains(&format!("#{} ", cursor + 1)), "{marker}");
            // Escaped once, not twice.
            assert!(!text.contains("\\\\[flottille:"), "{marker}");
        }
    }

    #[test]
    fn a_daemon_error_is_a_refusal_when_the_request_was_wrong_and_a_failure_when_it_could_not_be_done()
     {
        let error = |code: &str| -> Box<dyn std::error::Error> {
            Box::new(DaemonError {
                code: code.into(),
                message: "a sentence".into(),
            })
        };
        for code in [
            "refused",
            "not-found",
            "invalid-params",
            "invalid-request",
            "invalid-config",
            "unknown-method",
        ] {
            assert!(
                matches!(failure(error(code)), Outcome::Refused(text) if text == "refused: a sentence"),
                "{code}"
            );
        }
        for code in ["unavailable", "something-new"] {
            assert!(
                matches!(failure(error(code)), Outcome::Failed(text) if text == "a sentence"),
                "{code}"
            );
        }
    }

    /// What `agent.trees` answers for `count` roots, newest first, each with a tree of `size`.
    fn trees_answer(count: u64, size: u64, more: bool) -> Value {
        let trees: Vec<Value> = (0..count)
            .map(|n| {
                let agent = |k: u64| {
                    json!({"id": format!("{:06x}{k:06x}", count - n), "role": "lead",
                        "title": format!("title {n} {k} <flottille-note"), "project": "app",
                        "workspace": null, "parent": null, "depth": u64::from(k > 0),
                        "state": "idle"})
                };
                json!({
                    "status": {
                        "agent": agent(0), "name": format!("agent {:06x}000000", count - n),
                        "line": format!("agent {:06x}000000, idle for 3 min", count - n),
                        "doing": "idle",
                        "waiting_on": null, "needs_user": false,
                        "messages": ["m".repeat(600), "[flottille:x] ".repeat(40)],
                    },
                    "agents": (0..size).map(|k| json!({
                        "agent": agent(k), "doing": "idle for 3 min", "waiting_on": null,
                        "needs_user": false
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({"trees": trees, "more": more, "hidden": 0})
    }

    fn listing(tool: &'static str) -> Listing {
        Listing {
            tool,
            project: None,
            ended: false,
        }
    }

    #[test]
    fn many_roots_are_shown_newest_first_under_the_limit_with_the_cursor_to_the_rest() {
        // Trees of 40 agents do not fit 20 at a time; a status is short and the 20 do.
        for (tool, show, cut_short) in [
            ("agents_list", tree_block as fn(&Value) -> String, true),
            ("agent_status", status_of_tree, false),
        ] {
            // The daemon's page is 20 roots, each with 40 agents: more than fits.
            let answer = trees_answer(20, 40, true);
            let (text, error) = shown(Outcome::Done(roots_text(&answer, &listing(tool), show)));
            assert!(!error);
            assert!(
                text.chars().count() <= RESULT_LIMIT,
                "{tool}: {}",
                text.chars().count()
            );
            // The newest root is the first one, and the last one shown is the cursor's.
            let ids: Vec<&str> = answer["trees"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["status"]["agent"]["id"].as_str().unwrap())
                .collect();
            assert!(
                text.find(ids[0]).unwrap() < text.find(ids[1]).unwrap(),
                "{tool}"
            );
            let (_, rest) = text
                .rsplit_once(&format!("{tool} {{\"before\":\""))
                .unwrap_or_else(|| panic!("{tool}: no cursor in {}", &text[text.len() - 300..]));
            let before = &rest[..12];
            let at = ids
                .iter()
                .position(|id| *id == before)
                .expect("a root's id");
            assert!(text.contains(ids[at]));
            if cut_short {
                assert!(at > 0 && at < ids.len() - 1, "{tool}: {at}");
                assert!(
                    !text.contains(ids[at + 1]),
                    "{tool}: a root past the cursor"
                );
            } else {
                assert_eq!(at, ids.len() - 1, "{tool}");
            }
        }
    }

    #[test]
    fn the_cursor_keeps_what_the_listing_asked() {
        let answer = trees_answer(3, 1, true);
        let asked = Listing {
            tool: "agents_list",
            project: Some("app".into()),
            ended: true,
        };
        let text = roots_text(&answer, &asked, tree_block);
        let last = answer["trees"][2]["status"]["agent"]["id"]
            .as_str()
            .unwrap();
        assert!(
            text.ends_with(&format!(
                "agents_list {{\"before\":\"{last}\",\"ended\":true,\"project\":\"app\"}}"
            )),
            "{text}"
        );
    }

    #[test]
    fn live_roots_are_the_default_and_the_ended_ones_are_said() {
        let mut answer = trees_answer(2, 1, false);
        answer["hidden"] = json!(4);
        let text = roots_text(&answer, &listing("agents_list"), tree_block);
        assert!(text.contains("4 ended root agents are not shown"), "{text}");
        assert!(text.contains("agents_list {\"ended\":true}"), "{text}");
        let none = roots_text(
            &json!({"trees": [], "more": false, "hidden": 4}),
            &listing("agent_status"),
            status_of_tree,
        );
        assert!(none.starts_with("No live root agent"), "{none}");
        assert!(none.contains("agent_status {\"ended\":true}"), "{none}");
        let nothing = roots_text(
            &json!({"trees": [], "more": false, "hidden": 0}),
            &listing("agent_status"),
            status_of_tree,
        );
        assert!(nothing.starts_with("No agent yet"), "{nothing}");
    }
}
