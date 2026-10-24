//! The agents' MCP server (`docs/tools.md`): Streamable HTTP on `127.0.0.1`, an ephemeral port,
//! built on `rmcp`. Both protocol revisions are served (2025-11-25 with `initialize`, 2026-07-28
//! with `server/discover`), with no protocol session: the bearer token is the identity
//! (`docs/reference/mcp.md` §4).
//!
//! Each agent's process gets a token of its own when it starts; only its digest is kept, and
//! minting another revokes it. A request is checked for its host and origin, then for its token,
//! before anything else; the server that answers it holds the token's grant, so that it lists
//! and calls only the tools of that agent, in the order frozen in it. A call with a call key
//! (`claudecode/toolUseId`) is played once: a retry waits for the first and gets its answer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bytes::Bytes;
use flottille_core::agent::Agent;
use flottille_core::secrets;
use http::{Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::VERSION;
use crate::approvals;
use crate::claude;
use crate::daemon::Daemon;
use crate::files::{self, Context};
use crate::gate;
use crate::place::{Place, Resolved};
use crate::projects;
use crate::runs;

/// The path the server is reached at; any path is served.
pub const PATH: &str = "/mcp";
const BODY_LIMIT: usize = 16 * 1024 * 1024;
/// A normal result stays under this many characters, a refusal or an error under the other
/// (`docs/reference/mcp.md` §4 rule 5).
const RESULT_LIMIT: usize = 40_000;
const ERROR_LIMIT: usize = 2_000;
/// What a recorded call keeps of each text of its arguments.
const ARGUMENT_CHARS: usize = 500;
/// The call keys remembered, for retries, across agents.
const KEYS_KEPT: usize = 1024;
/// The revoked digests remembered, only to word the log line of a stale token.
const REVOKED_KEPT: usize = 256;
/// Log lines of refused tokens, at most, in a minute.
const REFUSALS_LOGGED: usize = 20;
/// How long a list of tools may be cached: as long as the token, which a new process replaces.
const LIST_TTL_MS: u64 = 24 * 3600 * 1000;

/// One served tool: its name, what the agent's client shows of it, how it is called.
struct Definition {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    read_only: bool,
    idempotent: bool,
    destructive: bool,
    schema: fn() -> Value,
}

/// The tools this server serves, in no order of their own: a session lists those of its grant in
/// the order frozen in the agent. The agent tools (`agent_*`, `report`, `ask_*`,
/// `approval_answer`) come with the tree.
const SERVED: [Definition; 9] = [
    Definition {
        name: "fs_read",
        title: "Read a file",
        description: "Reads a text file of the place, a page at a time (32 KiB at most), lines \
            numbered. The end of the page names the offset of the next one: pass it as \
            range.offset to read on. Reading a file is what lets fs_write and fs_edit change it \
            later. Refused outside the place and for a folder (use fs_list).",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Relative to the place, or to the repository when given."},
                "repository": {"type": "string", "description": "One of the project's repositories, by its path."},
                "range": {"type": "object", "properties": {
                    "offset": {"type": "integer", "minimum": 0, "description": "The byte to start at; a page names the next one."},
                    "length": {"type": "integer", "minimum": 1, "maximum": files::READ_PAGE_BYTES}
                }, "additionalProperties": false}
            }, "required": ["path"], "additionalProperties": false})
        },
    },
    Definition {
        name: "fs_list",
        title: "List a folder",
        description: "Lists the entries of a folder of the place, sorted, a folder's entries \
            after it down to depth levels (1 to 5, 1 by default): one line each with its kind \
            and size. .git is left out. A long listing stops and says so: list a folder below. \
            Refused outside the place.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Relative to the place, or to the repository when given; \".\" for its top."},
                "repository": {"type": "string"},
                "depth": {"type": "integer", "minimum": 1, "maximum": 5}
            }, "required": ["path"], "additionalProperties": false})
        },
    },
    Definition {
        name: "search",
        title: "Search the files",
        description: "Finds a text in the files of the place (case-insensitive, not a \
            pattern), path:line: text per match. It skips .git, node_modules, what the \
            .gitignore names and binary files, and can keep to a glob (\"**/*.rs\"). It stops at \
            100 matches or 1 MiB read and gives a cursor: pass it to go on.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "pattern": {"type": "string", "description": "The text to find."},
                "repository": {"type": "string"},
                "glob": {"type": "string", "description": "Only the files it matches: on the name alone without a /, ** for any folders."},
                "cursor": {"type": "string", "description": "The cursor a stopped search gave, path:line."}
            }, "required": ["pattern"], "additionalProperties": false})
        },
    },
    Definition {
        name: "fs_write",
        title: "Write a file",
        description: "Writes a whole file of the place, making it and its folders when it is \
            new. An existing file is written only when this session read it (fs_read) and it \
            has not changed since; otherwise the call is refused with what changed, and \
            nothing is written. Prefer fs_edit for a change in a long file.",
        read_only: false,
        idempotent: true,
        destructive: true,
        schema: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "repository": {"type": "string"},
                "content": {"type": "string", "description": "The whole new content."},
                "why": {"type": "string", "description": "One sentence: why this write."}
            }, "required": ["path", "content"], "additionalProperties": false})
        },
    },
    Definition {
        name: "fs_edit",
        title: "Edit a file",
        description: "Replaces texts in a file of the place that this session read: each \
            edit's old text must appear exactly once (overlaps counted) when its turn comes, \
            edits apply in order, and all apply or none does. Refused when the file changed \
            since it was read, with what changed. 1 to 50 edits.",
        read_only: false,
        idempotent: false,
        destructive: true,
        schema: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "repository": {"type": "string"},
                "edits": {"type": "array", "minItems": 1, "maxItems": 50, "items": {
                    "type": "object", "properties": {
                        "old": {"type": "string", "minLength": 1, "description": "The text to replace, appearing once."},
                        "new": {"type": "string"}
                    }, "required": ["old", "new"], "additionalProperties": false}},
                "why": {"type": "string"}
            }, "required": ["path", "edits"], "additionalProperties": false})
        },
    },
    Definition {
        name: "commands_list",
        title: "List the project's commands",
        description: "Lists the commands of the project's catalogue (tests, lint, servers…), \
            each with its line and where it runs. Run one with commands_run {command}; prefer \
            them to a line of your own, they are what the project means.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || json!({"type": "object", "properties": {}, "additionalProperties": false}),
    },
    Definition {
        name: "commands_run",
        title: "Run a command",
        description: "Runs a catalogue command or one line, without a shell: a pipe, a \
            redirection, a variable, a glob or && is refused, put them in a script of the \
            repository. Waits up to timeout seconds (30 by default, 600 at most) and answers \
            with the end of the output; a longer run keeps going under its run id.",
        read_only: false,
        idempotent: false,
        destructive: true,
        schema: || {
            json!({"type": "object", "properties": {
                "command": {"type": "string", "description": "A command of commands_list."},
                "line": {"type": "string", "description": "One command line, split on spaces, quotes kept whole."},
                "repository": {"type": "string", "description": "Where it runs; the place's top by default."},
                "timeout": {"type": "integer", "minimum": 1, "maximum": 600},
                "why": {"type": "string"}
            }, "additionalProperties": false})
        },
    },
    Definition {
        name: "commands_output",
        title: "Read a run's output",
        description: "The state of a run this agent started, and lines of its output: the last \
            tail lines (40 by default, 1000 at most), or those from start. The answer says how \
            to read the lines before or after. Only the last 64 KiB of an output are kept.",
        read_only: true,
        idempotent: true,
        destructive: false,
        schema: || {
            json!({"type": "object", "properties": {
                "run": {"type": "string", "description": "The run id commands_run gave."},
                "tail": {"type": "integer", "minimum": 1, "maximum": 1000},
                "start": {"type": "integer", "minimum": 1, "description": "The first line to show."}
            }, "required": ["run"], "additionalProperties": false})
        },
    },
    Definition {
        name: "commands_stop",
        title: "Stop a run",
        description: "Stops a run this agent started, with everything it started: asked to \
            end, then killed 5 seconds later. Answers once it ended, with the end of its \
            output. A run that already ended is left as it is.",
        read_only: false,
        idempotent: true,
        destructive: true,
        schema: || {
            json!({"type": "object", "properties": {
                "run": {"type": "string"}
            }, "required": ["run"], "additionalProperties": false})
        },
    },
];

fn definition(name: &str) -> Option<&'static Definition> {
    SERVED.iter().find(|tool| tool.name == name)
}

fn tool_of(definition: &Definition) -> Tool {
    serde_json::from_value(json!({
        "name": definition.name,
        "title": definition.title,
        "description": definition.description,
        "inputSchema": (definition.schema)(),
        "annotations": {
            "title": definition.title,
            "readOnlyHint": definition.read_only,
            "idempotentHint": definition.idempotent,
            "destructiveHint": definition.destructive,
            "openWorldHint": false,
        },
        // In bare mode Flottille's tools are all the agent has: loaded upfront, never searched.
        "_meta": {"anthropic/alwaysLoad": true},
    }))
    .expect("a tool definition is a tool")
}

/// What a token lets its bearer do: the agent's tools that exist, in its order, in its place.
pub struct Grant {
    pub agent: String,
    pub tools: Vec<&'static str>,
    pub place: Place,
}

/// What a tool did.
pub enum Outcome {
    Done(String),
    /// Refused, with the sentence that says why and what to do.
    Refused(String),
    /// It could not be done.
    Failed(String),
    /// The gate held it for an answer: the waiting text.
    Held(String),
}

impl Outcome {
    fn word(&self) -> &'static str {
        match self {
            Outcome::Done(_) => "ok",
            Outcome::Refused(_) => "refused",
            Outcome::Failed(_) => "failed",
            Outcome::Held(_) => "held",
        }
    }
}

/// The arguments of a call, decoded, or the refusal saying what does not fit.
pub fn arguments<T: DeserializeOwned>(tool: &str, args: Value) -> Result<T, Outcome> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|error| {
        Outcome::Refused(format!(
            "refused: the arguments do not fit {tool}: {error}; see its input schema"
        ))
    })
}

/// A text masked of the daemon's secrets and of those of the place's secret files.
pub fn masked(text: &str, place: &Place) -> String {
    secrets::mask_also(text, &std::env::vars_os().collect(), &place.secret_values())
}

/// The server's state, held by the daemon.
pub struct Server {
    /// Where agents reach it.
    pub url: String,
    port: u16,
    tokens: Mutex<Tokens>,
    keyed: Mutex<Keyed>,
    pub notes: Notes,
    pub kept: files::Kept,
    pub runs: runs::Live,
    refusals: Mutex<VecDeque<Instant>>,
    /// The agents whose calls came without a call key, said once in the log.
    keyless: Mutex<HashSet<String>>,
}

#[derive(Default)]
struct Tokens {
    /// By the SHA-256 digest of the token, never the token.
    grants: HashMap<String, Arc<Grant>>,
    by_agent: HashMap<String, String>,
    /// Digests revoked, with the agent each served.
    revoked: VecDeque<(String, String)>,
}

/// A call's answer: its text, and whether it is an error.
type Answer = (String, bool);

/// The answers of the calls made with a call key, by agent and key, kept for their retries.
#[derive(Default)]
struct Keyed {
    answers: HashMap<(String, String), watch::Receiver<Option<Answer>>>,
    order: VecDeque<(String, String)>,
}

/// Notes pinned for an agent, appended to its next tool result (the deliveries of `#7` use them
/// to reach an agent in a turn).
#[derive(Default)]
pub struct Notes {
    pinned: Mutex<HashMap<String, Vec<String>>>,
}

impl Notes {
    #[allow(dead_code)]
    pub fn pin(&self, agent: &str, note: String) {
        self.pinned
            .lock()
            .unwrap()
            .entry(agent.to_owned())
            .or_default()
            .push(note);
    }

    fn take(&self, agent: &str) -> Vec<String> {
        self.pinned
            .lock()
            .unwrap()
            .remove(agent)
            .unwrap_or_default()
    }
}

impl Server {
    pub fn new(port: u16) -> Server {
        Server {
            url: format!("http://127.0.0.1:{port}{PATH}"),
            port,
            tokens: Mutex::default(),
            keyed: Mutex::default(),
            notes: Notes::default(),
            kept: files::Kept::default(),
            runs: runs::Live::default(),
            refusals: Mutex::default(),
            keyless: Mutex::default(),
        }
    }

    /// A new token for the agent, revoking the one it had.
    pub fn mint(&self, grant: Grant) -> std::io::Result<String> {
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let token = base64url(&bytes);
        let digest = files::sha256(token.as_bytes());
        let mut tokens = self.tokens.lock().unwrap();
        let agent = grant.agent.clone();
        if let Some(old) = tokens.by_agent.insert(agent.clone(), digest.clone()) {
            revoke_digest(&mut tokens, old, &agent);
        }
        tokens.grants.insert(digest, Arc::new(grant));
        Ok(token)
    }

    /// The agent's token no longer works: its process is gone.
    pub fn revoke(&self, agent: &str) {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some(old) = tokens.by_agent.remove(agent) {
            revoke_digest(&mut tokens, old, agent);
        }
    }

    fn grant(&self, headers: &http::HeaderMap) -> Result<Arc<Grant>, Option<String>> {
        let token = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                let (scheme, token) = value.split_once(char::is_whitespace)?;
                let token = token.trim();
                (scheme.eq_ignore_ascii_case("bearer")
                    && !token.is_empty()
                    && !token.contains(char::is_whitespace))
                .then_some(token)
            })
            .ok_or(None)?;
        let digest = files::sha256(token.as_bytes());
        let tokens = self.tokens.lock().unwrap();
        if let Some(grant) = tokens.grants.get(&digest) {
            return Ok(grant.clone());
        }
        Err(tokens
            .revoked
            .iter()
            .find(|(old, _)| *old == digest)
            .map(|(_, agent)| agent.clone()))
    }

    /// A log line for a refused token, naming the agent it once served, at most so many a
    /// minute.
    fn log_refusal(&self, served: Option<String>) {
        let mut refusals = self.refusals.lock().unwrap();
        let now = Instant::now();
        while refusals
            .front()
            .is_some_and(|at| now.duration_since(*at).as_secs() >= 60)
        {
            refusals.pop_front();
        }
        if refusals.len() >= REFUSALS_LOGGED {
            return;
        }
        refusals.push_back(now);
        match served {
            Some(agent) => {
                eprintln!("flottille daemon: MCP: a revoked token of agent {agent} was refused")
            }
            None => eprintln!("flottille daemon: MCP: a request without a known token was refused"),
        }
    }
}

fn revoke_digest(tokens: &mut Tokens, digest: String, agent: &str) {
    tokens.grants.remove(&digest);
    tokens.revoked.push_back((digest, agent.to_owned()));
    if tokens.revoked.len() > REVOKED_KEPT {
        tokens.revoked.pop_front();
    }
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// The agent's tools that this server serves, in the agent's order.
fn served(tools: &[String]) -> Vec<&'static str> {
    let prefix = format!("mcp__{}__", claude::MCP_SERVER);
    tools
        .iter()
        .filter_map(|tool| definition(tool.strip_prefix(&prefix)?))
        .map(|definition| definition.name)
        .collect()
}

/// The grant of an agent's process: its tools that are served, in its order, and its place.
pub fn grant_of(daemon: &Daemon, agent: &Agent) -> Result<Grant, String> {
    let tools = served(&agent.tools);
    let project = projects::load_project(daemon, &agent.project).map_err(|f| f.message)?;
    let place = match &agent.workspace {
        Some(name) => {
            let workspace = daemon
                .read(|store| store.workspace(&project.name, name))
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("project {} has no workspace named {name}", project.name))?;
            Place {
                project: project.name.clone(),
                workspace: Some(name.clone()),
                root: agent.folder.clone(),
                repositories: workspace
                    .repositories
                    .iter()
                    .map(|r| (r.path.clone(), workspace.folder_of(&r.path)))
                    .collect(),
                branch: workspace.branch.clone(),
                variables: workspace.variables.clone(),
                home: std::env::var_os("HOME").map(Into::into),
            }
        }
        None => Place {
            project: project.name.clone(),
            workspace: None,
            root: agent.folder.clone(),
            repositories: project
                .repositories
                .iter()
                .map(|r| {
                    let folder = match r.path.as_str() {
                        "." => project.folder.clone(),
                        path => project.folder.join(path),
                    };
                    (r.path.clone(), folder)
                })
                .collect(),
            branch: String::new(),
            variables: project.variables.clone(),
            home: std::env::var_os("HOME").map(Into::into),
        },
    };
    Ok(Grant {
        agent: agent.id.clone(),
        tools,
        place,
    })
}

// HTTP.

type Body = BoxBody<Bytes, Infallible>;

/// Serves the listener until the daemon stops.
pub async fn serve(
    daemon: Arc<Daemon>,
    listener: TcpListener,
    mut stopping: watch::Receiver<bool>,
) {
    let port = daemon.mcp.port;
    let config = StreamableHttpServerConfig::default()
        // No protocol session, for either revision: the token is the identity.
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_max_request_body_bytes(BODY_LIMIT)
        .with_allowed_hosts([format!("127.0.0.1:{port}"), format!("localhost:{port}")])
        .with_allowed_origins([
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ]);
    let handler = Handler {
        daemon: daemon.clone(),
    };
    let service = Arc::new(StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    ));
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(error) => {
                    eprintln!("flottille daemon: MCP: accept: {error}");
                    continue;
                }
            },
            _ = stopping.wait_for(|stopping| *stopping) => return,
        };
        let (daemon, service) = (daemon.clone(), service.clone());
        tokio::spawn(async move {
            let answer = hyper::service::service_fn(move |request| {
                let (daemon, service) = (daemon.clone(), service.clone());
                async move { Ok::<_, Infallible>(respond(&daemon, &service, request).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), answer)
                .await;
        });
    }
}

async fn respond(
    daemon: &Daemon,
    service: &StreamableHttpService<Handler, LocalSessionManager>,
    mut request: Request<Incoming>,
) -> Response<Body> {
    // Against DNS rebinding, before anything else: only this machine's names, only this port.
    let port = daemon.mcp.port;
    let local = |value: &str, prefix: &str| {
        [
            format!("{prefix}127.0.0.1:{port}"),
            format!("{prefix}localhost:{port}"),
        ]
        .iter()
        .any(|allowed| allowed == value)
    };
    let header = |name| request.headers().get(name).and_then(|v| v.to_str().ok());
    if !header(http::header::HOST).is_some_and(|host| local(host, "")) {
        return answer(StatusCode::FORBIDDEN, json!({"error": "forbidden host"}));
    }
    if header(http::header::ORIGIN).is_some_and(|origin| !local(origin, "http://")) {
        return answer(StatusCode::FORBIDDEN, json!({"error": "forbidden origin"}));
    }
    match daemon.mcp.grant(request.headers()) {
        Ok(grant) => {
            request.extensions_mut().insert(grant);
            service.handle(request).await
        }
        Err(served) => {
            daemon.mcp.log_refusal(served);
            answer(StatusCode::UNAUTHORIZED, json!({"error": "unauthorized"}))
        }
    }
}

fn answer(status: StatusCode, body: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body.to_string())).boxed())
        .expect("a valid answer")
}

// MCP.

#[derive(Clone)]
struct Handler {
    daemon: Arc<Daemon>,
}

fn grant_in(context: &RequestContext<RoleServer>) -> Result<Arc<Grant>, ErrorData> {
    context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Arc<Grant>>())
        .cloned()
        .ok_or_else(|| ErrorData::internal_error("the request has no grant", None))
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        let capabilities: ServerCapabilities =
            serde_json::from_value(json!({"tools": {"listChanged": false}})).expect("capabilities");
        ServerConfig::new(capabilities).with_server_info(Implementation::new("flottille", VERSION))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let grant = grant_in(&context)?;
        let tools = grant
            .tools
            .iter()
            .filter_map(|name| definition(name))
            .map(tool_of)
            .collect();
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(LIST_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let grant = grant_in(&context)?;
        let key = request
            .meta
            .as_ref()
            .and_then(|meta| meta.get("claudecode/toolUseId"))
            .or_else(|| context.meta.get("claudecode/toolUseId"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let (text, error) = call(
            &self.daemon,
            grant,
            &request.name,
            args,
            key,
            context.ct.clone(),
        )
        .await;
        let content = vec![ContentBlock::text(text)];
        Ok(if error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        }
        .into())
    }
}

/// A call, played once for its call key, recorded, its pinned notes appended: its text, and
/// whether it is an error.
async fn call(
    daemon: &Arc<Daemon>,
    grant: Arc<Grant>,
    name: &str,
    args: Value,
    key: Option<String>,
    cancelled: CancellationToken,
) -> (String, bool) {
    let tool = name
        .strip_prefix(&format!("mcp__{}__", claude::MCP_SERVER))
        .unwrap_or(name)
        .to_owned();
    let Some(key) = key else {
        if daemon
            .mcp
            .keyless
            .lock()
            .unwrap()
            .insert(grant.agent.clone())
        {
            eprintln!(
                "flottille daemon: agent {}: its MCP calls carry no call key \
                 (claudecode/toolUseId): a retried call is played again",
                grant.agent
            );
        }
        return played(daemon, &grant, &tool, args, None, cancelled).await;
    };
    let slot = (grant.agent.clone(), key.clone());
    let first = {
        let mut keyed = daemon.mcp.keyed.lock().unwrap();
        match keyed.answers.get(&slot) {
            Some(answer) => Err(answer.clone()),
            None => {
                let (sender, receiver) = watch::channel(None);
                keyed.answers.insert(slot.clone(), receiver);
                keyed.order.push_back(slot.clone());
                if keyed.order.len() > KEYS_KEPT
                    && let Some(old) = keyed.order.pop_front()
                {
                    keyed.answers.remove(&old);
                }
                Ok(sender)
            }
        }
    };
    match first {
        Ok(sender) => {
            let answer = played(daemon, &grant, &tool, args, Some(&key), cancelled).await;
            sender.send_replace(Some(answer.clone()));
            answer
        }
        // A retry: the first call's answer, once it has one.
        Err(mut answer) => match answer.wait_for(Option::is_some).await {
            Ok(answer) => answer.clone().expect("waited for"),
            Err(_) => (
                "the call failed: its first attempt was lost".to_owned(),
                true,
            ),
        },
    }
}

async fn played(
    daemon: &Arc<Daemon>,
    grant: &Arc<Grant>,
    tool: &str,
    args: Value,
    key: Option<&str>,
    cancelled: CancellationToken,
) -> (String, bool) {
    let started = Instant::now();
    let recorded = record_of(&args, &grant.place);
    let mut verdict = None;
    let outcome = if grant.tools.contains(&tool) {
        let (decided, identity) = judge(daemon, grant, tool, &args);
        // A call the user allowed for the tree goes through, never one reaching a sensitive place.
        let decided = match decided {
            gate::Verdict::Ask(ask)
                if !ask.sensitive && approvals::granted(daemon, &grant.agent, &identity) =>
            {
                gate::Verdict::Allow
            }
            decided => decided,
        };
        verdict = Some(decided.word());
        match decided {
            gate::Verdict::Allow => perform(daemon, grant, tool, args, cancelled, false).await,
            gate::Verdict::Deny(why) => Outcome::Refused(why),
            gate::Verdict::Ask(ask) => {
                approvals::ask(daemon, grant, tool, &args, key, ask, identity)
            }
        }
    } else {
        Outcome::Refused(format!(
            "refused: this agent has no tool {tool}: its tools are {}",
            grant.tools.join(", ")
        ))
    };
    record(
        daemon, grant, tool, key, recorded, verdict, &outcome, started, None,
    );
    let error = matches!(outcome, Outcome::Refused(_) | Outcome::Failed(_));
    let notes = daemon.mcp.notes.take(&grant.agent);
    (shown(outcome, &notes, &grant.place), error)
}

/// The gate's verdict on a call of one of the agent's tools, and the call's identity for a grant.
pub fn judge(daemon: &Daemon, grant: &Grant, tool: &str, args: &Value) -> (gate::Verdict, String) {
    let project = projects::load_project(daemon, &grant.place.project).ok();
    let (paths, words, command) = reached(daemon, &grant.place, tool, args);
    let command = command.as_deref().and_then(|name| {
        let (name, command) = project.as_ref()?.commands.get_key_value(name)?;
        Some((name.as_str(), command))
    });
    let paths_of = &daemon.paths;
    let own = [
        paths_of.data.clone(),
        paths_of.config.clone(),
        paths_of.state.clone(),
        paths_of.runtime.clone(),
    ];
    let call = gate::Call {
        grant,
        tool,
        args,
        words: &words,
        paths: &paths,
        command,
        never: project.as_ref().map(|project| project.never.as_slice()),
        own: &own,
    };
    (gate::decide(&call), gate::identity(&call))
}

/// Performs a call the user allowed, recorded as the agent's: the request's answer.
pub async fn performed(
    daemon: &Arc<Daemon>,
    grant: &Arc<Grant>,
    tool: &str,
    args: Value,
    request: u64,
) -> Outcome {
    let started = Instant::now();
    let recorded = record_of(&args, &grant.place);
    let outcome = perform(daemon, grant, tool, args, CancellationToken::new(), true).await;
    let verdict = Some("allow");
    record(
        daemon,
        grant,
        tool,
        None,
        recorded,
        verdict,
        &outcome,
        started,
        Some(request),
    );
    outcome
}

/// The call's `agent.tool_served` event.
#[allow(clippy::too_many_arguments)]
fn record(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    key: Option<&str>,
    recorded: Value,
    verdict: Option<&str>,
    outcome: &Outcome,
    started: Instant,
    request: Option<u64>,
) {
    let duration = started.elapsed().as_millis() as u64;
    let data = json!({"agent": grant.agent, "tool": tool, "call_key": key,
        "arguments": recorded, "gate": verdict, "outcome": outcome.word(),
        "duration_ms": duration, "request": request});
    if let Err(error) = daemon.write(|store| {
        let event = store.append_agent_event(&grant.agent, "agent.tool_served", data, true)?;
        Ok(((), Some(event)))
    }) {
        eprintln!("flottille daemon: agent {}: {error}", grant.agent);
    }
}

/// What the agent is shown of an outcome: its text under its limit, then the notes pinned for the
/// agent, whole and counted in that limit. A result and a failure are masked of the place's
/// secrets too.
fn shown(outcome: Outcome, notes: &[String], place: &Place) -> String {
    let (text, limit) = match outcome {
        Outcome::Done(text) => (masked(&text, place), RESULT_LIMIT),
        Outcome::Held(text) => (text, RESULT_LIMIT),
        Outcome::Refused(text) => (text, ERROR_LIMIT),
        Outcome::Failed(text) => (
            masked(&format!("the call failed: {text}"), place),
            ERROR_LIMIT,
        ),
    };
    if notes.is_empty() {
        return cut(&text, limit);
    }
    let notes = notes.join("\n\n");
    let room = limit.saturating_sub(notes.chars().count() + 2);
    format!("{}\n\n{notes}", cut(&text, room))
}

/// The paths a call reaches and, for a run, the words it would start and the catalogue command
/// it names, read from its arguments the way its tool will; what does not fit is left to the
/// tool, which refuses it with its own sentence.
fn reached(
    daemon: &Daemon,
    place: &Place,
    tool: &str,
    args: &Value,
) -> (Vec<Resolved>, Vec<String>, Option<String>) {
    match tool {
        "fs_read" | "fs_list" | "search" | "fs_write" | "fs_edit" => {
            (files::reaches(place, tool, args), Vec::new(), None)
        }
        _ => runs::reaches(daemon, place, tool, args),
    }
}

/// Performs a call the gate let through; `approved` when the user allowed it, which lets its
/// paths be outside the place.
async fn perform(
    daemon: &Arc<Daemon>,
    grant: &Arc<Grant>,
    tool: &str,
    args: Value,
    cancelled: CancellationToken,
    approved: bool,
) -> Outcome {
    match tool {
        "commands_list" => runs::commands_list(daemon, &grant.place, args),
        "commands_run" => {
            runs::commands_run(daemon, &grant.agent, &grant.place, args, cancelled).await
        }
        "commands_output" => runs::commands_output(daemon, &grant.agent, args),
        "commands_stop" => runs::commands_stop(daemon, &grant.agent, args).await,
        _ => {
            // The file tools read and write the disk: off the daemon's thread.
            let (daemon, grant, tool) = (daemon.clone(), grant.clone(), tool.to_owned());
            let done = tokio::task::spawn_blocking(move || {
                let cx = Context {
                    daemon: &daemon,
                    agent: &grant.agent,
                    place: &grant.place,
                    approved,
                };
                match tool.as_str() {
                    "fs_read" => files::fs_read(&cx, args),
                    "fs_list" => files::fs_list(&cx, args),
                    "search" => files::search(&cx, args),
                    "fs_write" => files::fs_write(&cx, args),
                    "fs_edit" => files::fs_edit(&cx, args),
                    other => Outcome::Failed(format!("{other} is not served")),
                }
            })
            .await;
            done.unwrap_or_else(|error| Outcome::Failed(error.to_string()))
        }
    }
}

/// The arguments as a call is recorded: every text cut and masked.
pub fn record_of(args: &Value, place: &Place) -> Value {
    fn walk(value: &Value, mask: &dyn Fn(&str) -> String) -> Value {
        match value {
            Value::String(text) => Value::String(mask(&cut(text, ARGUMENT_CHARS))),
            Value::Array(items) => Value::Array(items.iter().map(|v| walk(v, mask)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), walk(v, mask)))
                    .collect::<Map<_, _>>(),
            ),
            other => other.clone(),
        }
    }
    let env = std::env::vars_os().collect();
    let values = place.secret_values();
    walk(args, &|text| secrets::mask_also(text, &env, &values))
}

/// A text cut at `limit` characters, saying how much was left out.
fn cut(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    let marker = format!("‹+{} chars›", count - limit + 16);
    let keep = limit.saturating_sub(16);
    let at = text
        .char_indices()
        .nth(keep)
        .map_or(text.len(), |(at, _)| at);
    format!("{}{marker}", &text[..at])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_has_no_padding_and_no_plus_or_slash() {
        assert_eq!(base64url(b"hello"), "aGVsbG8");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64url(&[0u8; 32]).len(), 43);
    }

    #[test]
    fn every_tool_is_described_for_a_new_colleague() {
        for definition in &SERVED {
            let tool = serde_json::to_value(tool_of(definition)).unwrap();
            let description = tool["description"].as_str().unwrap();
            assert!(description.len() < 1000, "{}", definition.name);
            assert_eq!(tool["_meta"]["anthropic/alwaysLoad"], true);
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(
                claude::tools(true, false)
                    .contains(&format!("mcp__flottille__{}", definition.name)),
                "{} is a tool agents get",
                definition.name
            );
        }
    }

    #[test]
    fn a_grant_holds_the_agent_s_served_tools_in_its_own_order() {
        let mut leaf = claude::tools(false, true);
        assert_eq!(served(&leaf).len(), 9);
        leaf.reverse();
        leaf.retain(|tool| !tool.ends_with("__search"));
        leaf.push("fs_read".into());
        let served = served(&leaf);
        assert_eq!(served.first(), Some(&"commands_stop"));
        assert_eq!(served.last(), Some(&"fs_read"));
        assert!(!served.contains(&"search"));
        assert_eq!(served.len(), 8);
    }

    #[test]
    fn a_long_text_is_cut_under_its_limit() {
        let text = "é".repeat(50_000);
        let kept = cut(&text, RESULT_LIMIT);
        assert!(kept.chars().count() <= RESULT_LIMIT);
        assert!(kept.ends_with(&format!("‹+{} chars›", 50_000 - RESULT_LIMIT + 16)));
        assert_eq!(cut("short", 10), "short");
    }

    fn place_in(root: &std::path::Path) -> Place {
        Place {
            project: "app".into(),
            workspace: None,
            root: root.to_owned(),
            repositories: vec![(".".into(), root.to_owned())],
            branch: String::new(),
            variables: Default::default(),
            home: None,
        }
    }

    #[test]
    fn pinned_notes_are_counted_in_the_limit_and_kept_whole() {
        let dir = tempfile::tempdir().unwrap();
        let place = place_in(dir.path());
        let notes = vec!["note one ".repeat(40), "note two".to_owned()];

        let shown = shown(Outcome::Done("é".repeat(50_000)), &notes, &place);

        assert!(
            shown.chars().count() <= RESULT_LIMIT,
            "{}",
            shown.chars().count()
        );
        assert!(shown.ends_with(&format!("\n\n{}\n\n{}", notes[0], notes[1])));
        assert!(shown.contains("chars›"));
        // Without notes, and with a short text, nothing changes.
        assert_eq!(shown_of("short", &[], &place), "short");
        assert_eq!(shown_of("short", &["n".into()], &place), "short\n\nn");
    }

    fn shown_of(text: &str, notes: &[String], place: &Place) -> String {
        shown(Outcome::Done(text.into()), notes, place)
    }

    #[test]
    fn a_failure_is_masked_of_the_place_s_secrets_and_cut_under_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "DB_URL=hunter2-hunter2\n").unwrap();
        let place = place_in(dir.path());

        let shown = shown(
            Outcome::Failed(format!("cannot open hunter2-hunter2 {}", "x".repeat(5_000))),
            &[],
            &place,
        );

        assert!(
            shown.starts_with("the call failed: cannot open [masked] "),
            "{shown}"
        );
        assert!(!shown.contains("hunter2"));
        assert!(shown.chars().count() <= ERROR_LIMIT);
    }

    #[test]
    fn pinned_notes_are_taken_once() {
        let notes = Notes::default();
        notes.pin("a", "first".into());
        notes.pin("a", "second".into());
        assert_eq!(notes.take("a"), ["first", "second"]);
        assert!(notes.take("a").is_empty());
    }
}
