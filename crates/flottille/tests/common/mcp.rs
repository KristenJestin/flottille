//! A client of the agents' MCP server, as an agent's own client would be: rmcp's, over HTTP, with
//! the bearer token the daemon gave the agent's session; and raw JSON-RPC posts for what a client
//! library hides (statuses, headers, the protocol revisions).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rmcp::model::{CallToolRequestParams, ClientConfig, ProtocolVersion};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientLifecycleMode, ClientServiceExt};
use serde_json::{Value, json};

use super::User;

/// The MCP server the daemon gave the agent session opened after `opened` others: its URL and
/// its `Authorization` header, as `session/new` carried them to the fake agent.
pub fn server_of(user: &User, opened: usize) -> (String, String) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(opened) = user.received_requests("session/new").get(opened) {
            let server = &opened["params"]["mcpServers"][0];
            let authorization = server["headers"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|h| h["name"] == "Authorization")
                .and_then(|h| h["value"].as_str())
                .unwrap_or_default()
                .to_owned();
            return (server["url"].as_str().unwrap().to_owned(), authorization);
        }
        assert!(Instant::now() < deadline, "no session was opened");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// rmcp's client, connected to the server with one of the two lifecycles.
pub struct Mcp {
    runtime: tokio::runtime::Runtime,
    client: Option<RunningService<RoleClient, ClientConfig>>,
    /// `flottille mcp`, when the client talks to the pilot door over its standard streams.
    child: Option<tokio::process::Child>,
}

/// What a tool call answered: its text, and whether it is an error.
#[derive(Debug)]
pub struct Answer {
    pub text: String,
    pub error: bool,
}

impl Mcp {
    /// With `initialize`, protocol 2025-11-25.
    pub fn initialize(url: &str, authorization: &str) -> Mcp {
        Mcp::connect(url, authorization, ClientLifecycleMode::Initialize)
    }

    /// With `server/discover`, protocol 2026-07-28, stateless.
    pub fn discover(url: &str, authorization: &str) -> Mcp {
        Mcp::connect(
            url,
            authorization,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
    }

    fn connect(url: &str, authorization: &str, lifecycle: ClientLifecycleMode) -> Mcp {
        // reqwest wants one even for plain http; a second install is refused, and harmless.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let token = authorization
            .strip_prefix("Bearer ")
            .expect("a bearer token");
        let config = StreamableHttpClientTransportConfig::with_uri(url).auth_header(token);
        let client = runtime.block_on(async {
            let transport = StreamableHttpClientTransport::from_config(config);
            let mut info = ClientConfig::default();
            info.protocol_version = ProtocolVersion::V_2025_11_25;
            info.serve_with_lifecycle(transport, lifecycle)
                .await
                .expect("the client connects")
        });
        Mcp {
            runtime,
            client: Some(client),
            child: None,
        }
    }

    /// The pilot door (`flottille mcp`) of `user`, over its standard streams, with `initialize`:
    /// protocol 2025-11-25.
    pub fn pilot(user: &User) -> Mcp {
        Mcp::over_stdio(user, ClientLifecycleMode::Initialize)
    }

    /// The same with `server/discover`: protocol 2026-07-28, stateless.
    pub fn pilot_discover(user: &User) -> Mcp {
        Mcp::over_stdio(
            user,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
    }

    fn over_stdio(user: &User, lifecycle: ClientLifecycleMode) -> Mcp {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut command = tokio::process::Command::from(user.command(&["mcp"]));
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let (child, client) = runtime.block_on(async {
            let mut child = command.spawn().expect("flottille mcp starts");
            let transport = (child.stdout.take().unwrap(), child.stdin.take().unwrap());
            let mut info = ClientConfig::default();
            info.protocol_version = ProtocolVersion::V_2025_11_25;
            let client = info
                .serve_with_lifecycle(transport, lifecycle)
                .await
                .expect("the client connects");
            (child, client)
        });
        Mcp {
            runtime,
            client: Some(client),
            child: Some(child),
        }
    }

    /// `tools/list`, as JSON.
    pub fn tools(&self) -> Vec<Value> {
        let client = self.client.as_ref().unwrap();
        let tools = self
            .runtime
            .block_on(client.list_all_tools())
            .expect("tools/list answers");
        tools
            .into_iter()
            .map(|tool| serde_json::to_value(tool).unwrap())
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    }

    pub fn call(&self, tool: &str, arguments: Value) -> Answer {
        self.call_with_meta(tool, arguments, None)
    }

    /// A call carrying Claude's call key, as Claude Code sends it.
    pub fn call_keyed(&self, tool: &str, arguments: Value, key: &str) -> Answer {
        self.call_with_meta(tool, arguments, Some(json!({"claudecode/toolUseId": key})))
    }

    fn call_with_meta(&self, tool: &str, arguments: Value, meta: Option<Value>) -> Answer {
        let client = self.client.as_ref().unwrap();
        let mut params = CallToolRequestParams::new(tool.to_owned())
            .with_arguments(arguments.as_object().cloned().unwrap_or_default());
        if let Some(meta) = meta {
            params.meta = Some(serde_json::from_value(meta).unwrap());
        }
        let result = self
            .runtime
            .block_on(client.call_tool(params))
            .unwrap_or_else(|e| panic!("{tool} answers: {e}"));
        let result = serde_json::to_value(result).unwrap();
        let text = result["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("");
        Answer {
            text,
            error: result["isError"] == true,
        }
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            let _ = self.runtime.block_on(client.cancel());
        }
    }
}

/// One raw HTTP/1.1 POST of a JSON body: the status, the body (a JSON answer, or the last `data:`
/// line of an event stream), and the headers by lowercase name.
pub fn post(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> (u16, String, HashMap<String, String>) {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut stream = TcpStream::connect(host).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let body = body.to_string();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    // The host of the URL, unless the test names another.
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        request.push_str(&format!("Host: {host}\r\n"));
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    stream.write_all(request.as_bytes()).unwrap();
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    let status: u16 = status.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut found = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            found.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let mut raw = Vec::new();
    let _ = reader.read_to_end(&mut raw);
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    if found
        .get("transfer-encoding")
        .is_some_and(|v| v == "chunked")
    {
        text = unchunk(&text);
    }
    if found
        .get("content-type")
        .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        text = text
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or_default()
            .to_owned();
    }
    (status, text, found)
}

fn unchunk(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        out.push_str(&after[..size]);
        rest = after[size..].trim_start_matches("\r\n");
    }
    out
}

/// The pilot door (`flottille mcp`) spoken to in raw JSON-RPC lines, for what a client library
/// hides: the handshake of each protocol revision and the server's instructions.
pub struct RawPilot {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl RawPilot {
    pub fn start(user: &User) -> RawPilot {
        let mut child = user
            .command(&["mcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        RawPilot {
            child,
            stdin,
            lines,
        }
    }

    /// One request, and the answer carrying its id.
    pub fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{message}").unwrap();
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            let answer: Value = serde_json::from_str(&line).unwrap();
            if answer["id"] == id {
                return answer;
            }
        }
    }

    pub fn notify(&mut self, method: &str) {
        let message = json!({"jsonrpc": "2.0", "method": method});
        writeln!(self.stdin, "{message}").unwrap();
    }
}

impl Drop for RawPilot {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
