//! Claude Code through its ACP adapter (`docs/agents.md`): where Flottille installs the adapter,
//! how it starts it, the bare options that leave the agent nothing but Flottille's tools, and what
//! `flottille doctor` checks.

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flottille_core::paths::Paths;
use serde_json::{Value, json};

/// The adapter, at the version Flottille was checked against (`docs/reference/acp.md`).
pub const ADAPTER: &str = "@agentclientprotocol/claude-agent-acp";
pub const ADAPTER_VERSION: &str = "0.95.0";

/// Claude Code caches the system prompt before this marker across sessions.
pub const BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

/// The name of Flottille's MCP server, which prefixes its tools for Claude.
pub const MCP_SERVER: &str = "flottille";

/// A tool call waiting on the user sends nothing: both of Claude's limits must outlast the wait.
const TOOL_WAIT_MS: &str = "600000";

/// Variables of the daemon's own environment that would change the agent's prompt cache: never
/// passed on.
const SCRUBBED: [&str; 2] = ["CLAUDE_CODE_PROMPT_CACHE_TTL", "FORCE_PROMPT_CACHING_5M"];

/// The tools of an agent the user starts, in their fixed order (`docs/design.md` §4.3). The MCP
/// server that serves them comes with the tools themselves; their names are frozen in each agent
/// already, because the prompt cache depends on them.
const ROOT_TOOLS: [&str; 18] = [
    "fs_read",
    "fs_list",
    "search",
    "fs_write",
    "fs_edit",
    "commands_list",
    "commands_run",
    "commands_output",
    "commands_stop",
    "agent_launch",
    "agent_send",
    "agent_status",
    "agent_read",
    "agent_stop",
    "report",
    "ask_user",
    "approval_forward",
    "approval_deny",
];

/// The exact tool names a root agent is allowed, as Claude names them.
pub fn root_tools() -> Vec<String> {
    ROOT_TOOLS
        .iter()
        .map(|tool| format!("mcp__{MCP_SERVER}__{tool}"))
        .collect()
}

/// The system prompt of a new agent: a shared part, the boundary, the part of this session.
/// Frozen in the agent; the full layering (roles, the project's instruction files) comes with the
/// roles.
pub fn instructions(project: &str, place: &str, folder: &Path) -> String {
    let shared = "You are an agent run by Flottille. Flottille's tools are your only tools: a tool \
        you do not have is work your role does not do; say so, and do not look for a way around \
        it. Every call goes through one gate: a refusal is an answer, do not retry it nor reach \
        the same effect another way. A call held for approval has not happened: go on with what \
        does not depend on it, or end your turn saying what you wait for. Never invent: say what \
        you observed, and \"not verified\" when you did not check.";
    let session = format!(
        "You work for the project {project}, in {place}: the folder {}.",
        folder.display()
    );
    format!("{shared}\n\n{BOUNDARY}\n\n{session}")
}

/// `_meta` of `session/new`, `session/load` and `session/resume`: the bare mode.
pub fn session_meta(instructions: &str, tools: &[String]) -> Value {
    json!({"claudeCode": {"options": {
        "tools": [],
        "allowedTools": tools,
        // The adapter's default loads the user's settings, hooks, plugins and skills.
        "settingSources": [],
        "strictMcpConfig": true,
        "systemPrompt": {"type": "custom", "prompt": system_prompt(instructions), "snapshot": true},
        "env": {
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
            "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
            "MCP_TOOL_TIMEOUT": TOOL_WAIT_MS,
            "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT": TOOL_WAIT_MS,
        },
    }}})
}

/// The instructions as Claude takes them: the part before the boundary, the boundary, the part
/// after; a text without one as it is.
fn system_prompt(instructions: &str) -> Value {
    match instructions.split_once(&format!("\n\n{BOUNDARY}\n\n")) {
        Some((before, after)) => json!([before, BOUNDARY, after]),
        None => json!(instructions),
    }
}

/// Where Flottille installs the adapter: `npm install --prefix` of this folder.
pub fn adapter_prefix(paths: &Paths) -> PathBuf {
    paths.data.join("adapters/claude")
}

fn adapter_package(paths: &Paths) -> PathBuf {
    adapter_prefix(paths).join("node_modules").join(ADAPTER)
}

/// The installed adapter: its version and the script its package names as its program.
fn installed_adapter(paths: &Paths) -> Option<(String, PathBuf)> {
    let package = adapter_package(paths);
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(package.join("package.json")).ok()?).ok()?;
    let version = manifest["version"].as_str()?.to_owned();
    let bin = match &manifest["bin"] {
        Value::String(script) => Some(script.as_str()),
        Value::Object(scripts) => scripts.values().next().and_then(Value::as_str),
        _ => None,
    };
    Some((version, package.join(bin.unwrap_or("dist/index.js"))))
}

/// A program of that name in `PATH`, as a shell would find it, without one.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// How the adapter process starts.
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(&'static str, OsString)>,
    pub scrubbed: &'static [&'static str],
}

/// The adapter run by Node, pointed at the user's `claude`; or, for tests and development, the
/// program `FLOTTILLE_AGENT_COMMAND` names in its place. A sentence the user can act on when
/// something is missing.
pub fn launch(paths: &Paths) -> Result<Launch, String> {
    let claude = find_program("claude");
    let env: Vec<(&'static str, OsString)> = claude
        .iter()
        .map(|claude| ("CLAUDE_CODE_EXECUTABLE", claude.clone().into_os_string()))
        .collect();
    if let Some(program) = std::env::var_os("FLOTTILLE_AGENT_COMMAND") {
        return Ok(Launch {
            program: program.into(),
            args: Vec::new(),
            env,
            scrubbed: &SCRUBBED,
        });
    }
    let Some((version, script)) = installed_adapter(paths) else {
        return Err(
            "the Claude adapter is not installed: run `flottille agent install` (it needs npm)"
                .into(),
        );
    };
    if version != ADAPTER_VERSION {
        return Err(format!(
            "the Claude adapter installed is {version}, this Flottille runs {ADAPTER_VERSION}: run \
             `flottille agent install`"
        ));
    }
    let node = find_program("node")
        .ok_or("Node is not installed, or not on PATH: the Claude adapter runs on it")?;
    if claude.is_none() {
        return Err(
            "`claude` is not on PATH: install Claude Code and sign in with it first".into(),
        );
    }
    Ok(Launch {
        program: node,
        args: vec![script.into_os_string()],
        env,
        scrubbed: &SCRUBBED,
    })
}

/// `npm install` of the pinned adapter into Flottille's data folder; never `npx`.
pub fn install(paths: &Paths) -> Result<(), String> {
    let npm = find_program("npm").ok_or(
        "npm is not installed, or not on PATH: it comes with Node, which the adapter needs",
    )?;
    let prefix = adapter_prefix(paths);
    std::fs::create_dir_all(&prefix).map_err(|e| format!("{}: {e}", prefix.display()))?;
    let status = Command::new(npm)
        .arg("install")
        .arg("--prefix")
        .arg(&prefix)
        // The adapter runs the user's own `claude`: the SDK's bundled one is not needed, nor any
        // install script.
        .args([
            "--omit=optional",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ])
        .arg(format!("{ADAPTER}@{ADAPTER_VERSION}"))
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("npm: {e}"))?;
    if !status.success() {
        return Err(format!("npm install failed ({status})"));
    }
    Ok(())
}

/// What `flottille doctor` reports: whether Claude agents can run on this machine.
pub fn doctor(paths: &Paths) -> Value {
    let program = |name: &str| match find_program(name) {
        None => json!({"present": false}),
        Some(path) => json!({"present": true, "path": path, "version": version_of(&path)}),
    };
    let node = program("node");
    let claude = program("claude");
    // Only its presence: the file is never opened.
    let login = login_file();
    let login = json!({
        "present": login.as_deref().is_some_and(|p| std::fs::symlink_metadata(p).is_ok()),
        "path": login,
    });
    let adapter = match installed_adapter(paths) {
        None => {
            json!({"present": false, "pinned": ADAPTER_VERSION, "path": adapter_package(paths)})
        }
        Some((version, _)) => json!({"present": true, "version": version,
            "pinned": ADAPTER_VERSION, "path": adapter_package(paths)}),
    };
    let ready = node["present"] == true
        && claude["present"] == true
        && login["present"] == true
        && adapter["version"] == ADAPTER_VERSION;
    json!({"node": node, "claude": claude, "login": login, "adapter": adapter, "ready": ready})
}

/// Claude's login file: in `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
fn login_file() -> Option<PathBuf> {
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".claude"),
    };
    Some(dir.join(".credentials.json"))
}

/// `<program> --version`, given 5 s: the first version number it prints.
fn version_of(program: &Path) -> Option<String> {
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().ok()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    first_version(&output)
}

/// The first `N.N.N` of the text, with its pre-release part (`-beta.1`) when it has one.
fn first_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    (0..bytes.len()).find_map(|start| {
        if start > 0 && bytes[start - 1].is_ascii_digit() {
            return None;
        }
        let mut end = start;
        for part in 0..3 {
            let digits = bytes[end..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits == 0 {
                return None;
            }
            end += digits;
            if part < 2 {
                if bytes.get(end) != Some(&b'.') {
                    return None;
                }
                end += 1;
            }
        }
        if bytes.get(end) == Some(&b'-') {
            let more = bytes[end + 1..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || **b == b'.' || **b == b'-')
                .count();
            if more > 0 {
                end += 1 + more;
            }
        }
        Some(text[start..end].to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_the_first_number_of_three_parts() {
        assert_eq!(first_version("v24.11.0\n").as_deref(), Some("24.11.0"));
        assert_eq!(
            first_version("2.1.300 (Claude Code)").as_deref(),
            Some("2.1.300")
        );
        assert_eq!(
            first_version("tool 1.2 then 3.4.5-beta.1 x").as_deref(),
            Some("3.4.5-beta.1")
        );
        assert_eq!(first_version("no version"), None);
    }

    #[test]
    fn the_system_prompt_keeps_the_cache_boundary_apart() {
        let text = instructions("zebra", "main", Path::new("/work/zebra"));
        let prompt = system_prompt(&text);
        assert_eq!(prompt[1], BOUNDARY);
        assert!(prompt[2].as_str().unwrap().contains("zebra"));
        assert!(!prompt[0].as_str().unwrap().contains("zebra"));
        assert_eq!(system_prompt("plain"), json!("plain"));
    }
}
