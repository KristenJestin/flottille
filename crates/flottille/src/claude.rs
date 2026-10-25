//! Claude Code through Flottille's own bridge (`docs/agents.md`): how the daemon starts the
//! bridge, the bare options that leave the agent nothing but Flottille's tools, and what
//! `flottille doctor` checks.

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flottille_core::paths::Paths;
use serde_json::{Value, json};

/// The version of `claude` whose stream-json mode the bridge was checked against
/// (`docs/reference/acp.md`).
pub const CLAUDE_VERSION: &str = "2.1.295";

/// Claude Code caches the system prompt before this marker across sessions.
pub const BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

/// The name of Flottille's MCP server, which prefixes its tools for Claude.
pub const MCP_SERVER: &str = "flottille";

/// A tool call waiting on the user sends nothing: both of Claude's limits must outlast the wait.
const TOOL_WAIT_MS: &str = "600000";

/// Whether a variable of the daemon's environment stays out of the agent's (`docs/agents.md`).
/// The agent runs on the user's own sign-in and network, and on what Flottille sets: the daemon's
/// secrets and the user's Claude Code settings (a model, a cache lifetime) are not its business.
pub fn withheld(name: &str) -> bool {
    // The agent's own sign-in, which the secret-looking names below would otherwise catch.
    const SIGN_IN: [&str; 3] = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ];
    const EXACT: [&str; 12] = [
        // Forges: tokens and the folders of their command lines' configuration.
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "GITLAB_TOKEN",
        "GL_TOKEN",
        "GH_CONFIG_DIR",
        "GLAB_CONFIG_DIR",
        // The user's Claude Code settings.
        "ANTHROPIC_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "MAX_THINKING_TOKENS",
        "FORCE_PROMPT_CACHING_5M",
    ];
    if SIGN_IN.contains(&name) {
        return false;
    }
    EXACT.contains(&name)
        || ["BITBUCKET_", "BKT_", "CLAUDE_CODE_", "DISABLE_"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        || (name.starts_with("ANTHROPIC_DEFAULT_") && name.ends_with("_MODEL"))
        || ["_TOKEN", "_SECRET", "_PASSWORD", "_API_KEY"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// The names of the tools every agent has, in their fixed order (`docs/design.md` §4.3). The MCP
/// server that serves them comes with the tools themselves; their names are frozen in each agent
/// already, because the prompt cache depends on them.
const FILE_AND_COMMAND_TOOLS: [&str; 9] = [
    "fs_read",
    "fs_list",
    "search",
    "fs_write",
    "fs_edit",
    "commands_list",
    "commands_run",
    "commands_output",
    "commands_stop",
];
const AGENT_TOOLS: [&str; 5] = [
    "agent_launch",
    "agent_send",
    "agent_status",
    "agent_read",
    "agent_stop",
];

/// The exact tool names an agent gets, as Claude names them: what it can do decides (`docs/design.md`
/// §4.3, `docs/reference/mcp.md` §4 rule 9). A root asks the user, a child its parent; an agent at
/// the depth limit cannot have children, so it has no `agent_*` tool and nobody's request to answer.
pub fn tools(root: bool, at_depth_limit: bool) -> Vec<String> {
    let mut names: Vec<&str> = FILE_AND_COMMAND_TOOLS.to_vec();
    if !at_depth_limit {
        names.extend(AGENT_TOOLS);
    }
    names.push("report");
    names.push(if root { "ask_user" } else { "ask_parent" });
    if !at_depth_limit {
        names.push("approval_answer");
    }
    names
        .into_iter()
        .map(|tool| format!("mcp__{MCP_SERVER}__{tool}"))
        .collect()
}

/// `_meta` of `session/new`, `session/load` and `session/resume`: the bare mode, which the bridge
/// sets on `claude` (`bridge.rs`).
pub fn session_meta(instructions: &str, tools: &[String]) -> Value {
    json!({"claudeCode": {"options": {
        "tools": [],
        "allowedTools": tools,
        // Claude Code's default loads the user's settings, hooks, plugins and skills.
        "settingSources": [],
        "strictMcpConfig": true,
        "systemPrompt": {"type": "custom", "prompt": system_prompt(instructions), "snapshot": true},
        "env": {
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
            "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
            // Flottille's tools are all the agent has: they load upfront, with no search step
            // (`docs/reference/mcp.md` §4 rule 7).
            "ENABLE_TOOL_SEARCH": "false",
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

/// Where the bridge keeps each session's conversation, which a load replays.
pub fn histories(paths: &Paths) -> PathBuf {
    paths.state.join("bridge/claude")
}

/// The bridge's history of a session, which a session id of the bridge names.
pub fn history(paths: &Paths, session: &str) -> Option<PathBuf> {
    is_session_id(session).then(|| histories(paths).join(format!("{session}.jsonl")))
}

/// A session id that names a file: letters, digits and dashes only.
pub fn is_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The session is over (replaced, or its agent ended): its history goes, nothing will load it.
pub fn forget_history(paths: &Paths, session: &str) {
    if let Some(path) = history(paths, session) {
        let _ = std::fs::remove_file(path);
    }
}

/// How the agent's process starts.
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Flottille's own bridge, `flottille bridge claude`, which drives the user's `claude`; or, for
/// tests and development, the program `FLOTTILLE_AGENT_COMMAND` names in its place. A sentence the
/// user can act on when something is missing.
pub fn launch() -> Result<Launch, String> {
    if let Some(program) = std::env::var_os("FLOTTILLE_AGENT_COMMAND") {
        return Ok(Launch {
            program: program.into(),
            args: Vec::new(),
        });
    }
    if find_program("claude").is_none() {
        return Err(
            "`claude` is not on PATH: install Claude Code and sign in with it first".into(),
        );
    }
    let program =
        std::env::current_exe().map_err(|e| format!("cannot find Flottille itself: {e}"))?;
    Ok(Launch {
        program,
        args: vec!["bridge".into(), "claude".into()],
    })
}

/// What `flottille doctor` reports: whether Claude agents can run on this machine.
pub fn doctor() -> Value {
    let mut warnings = Vec::new();
    let claude = match find_program("claude") {
        None => json!({"present": false, "checked": CLAUDE_VERSION}),
        Some(path) => {
            let version = version_of(&path);
            if let Some(version) = version.as_deref().filter(|v| *v != CLAUDE_VERSION) {
                warnings.push(format!(
                    "claude is {version}, and this Flottille was checked against {CLAUDE_VERSION}: \
                     its stream-json mode may have changed"
                ));
            }
            json!({"present": true, "path": path, "version": version, "checked": CLAUDE_VERSION})
        }
    };
    // Only its presence: the file is never opened, the key never read.
    let login = login_file();
    let api_key = std::env::var_os("ANTHROPIC_API_KEY").is_some_and(|key| !key.is_empty());
    let login = json!({
        "present": login.as_deref().is_some_and(|p| std::fs::symlink_metadata(p).is_ok()),
        "path": login,
        "api_key": api_key,
    });
    let ready = claude["present"] == true && (login["present"] == true || api_key);
    json!({"claude": claude, "login": login, "warnings": warnings, "ready": ready})
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

    fn short(tools: Vec<String>) -> Vec<String> {
        tools
            .into_iter()
            .map(|t| t.strip_prefix("mcp__flottille__").unwrap().to_owned())
            .collect()
    }

    #[test]
    fn the_agent_gets_its_sign_in_and_the_network_and_not_the_daemon_s_secrets_or_settings() {
        for (name, gone) in [
            // Forges.
            ("GH_TOKEN", true),
            ("GITHUB_TOKEN", true),
            ("GH_ENTERPRISE_TOKEN", true),
            ("GITHUB_ENTERPRISE_TOKEN", true),
            ("GITLAB_TOKEN", true),
            ("GL_TOKEN", true),
            ("BITBUCKET_APP_PASSWORD", true),
            ("BKT_USER", true),
            ("GH_CONFIG_DIR", true),
            ("GLAB_CONFIG_DIR", true),
            // Any other secret by its name.
            ("NPM_TOKEN", true),
            ("DEPLOY_SECRET", true),
            ("DB_PASSWORD", true),
            ("OPENAI_API_KEY", true),
            // The user's Claude Code settings.
            ("ANTHROPIC_MODEL", true),
            ("ANTHROPIC_SMALL_FAST_MODEL", true),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", true),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", true),
            ("CLAUDE_CODE_PROMPT_CACHE_TTL", true),
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", true),
            ("DISABLE_TELEMETRY", true),
            ("MAX_THINKING_TOKENS", true),
            ("FORCE_PROMPT_CACHING_5M", true),
            // The agent's own sign-in.
            ("ANTHROPIC_API_KEY", false),
            ("ANTHROPIC_AUTH_TOKEN", false),
            ("CLAUDE_CODE_OAUTH_TOKEN", false),
            // Sign-in and network.
            ("ANTHROPIC_BASE_URL", false),
            ("CLAUDE_CONFIG_DIR", false),
            ("HTTPS_PROXY", false),
            ("https_proxy", false),
            ("NO_PROXY", false),
            ("SSL_CERT_FILE", false),
            ("NODE_EXTRA_CA_CERTS", false),
            // The rest of the user's environment.
            ("PATH", false),
            ("HOME", false),
            ("LANG", false),
        ] {
            assert_eq!(withheld(name), gone, "{name}");
        }
    }

    #[test]
    fn a_root_agent_gets_ask_user_the_agent_tools_and_approval_answer() {
        let tools = short(tools(true, false));
        assert_eq!(
            tools,
            [
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
                "approval_answer",
            ]
        );
    }

    #[test]
    fn the_tools_follow_what_the_agent_can_do() {
        let leaf = short(tools(false, true));
        assert_eq!(leaf.len(), 11);
        assert!(leaf.contains(&"ask_parent".to_owned()));
        assert!(
            !leaf
                .iter()
                .any(|t| t.starts_with("agent_") || t == "ask_user")
        );
        assert!(!leaf.contains(&"approval_answer".to_owned()));
        let lead_child = short(tools(false, false));
        assert!(lead_child.contains(&"ask_parent".to_owned()));
        assert!(lead_child.contains(&"approval_answer".to_owned()));
        assert!(!lead_child.contains(&"ask_user".to_owned()));
    }

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
        let text = format!("shared\n\n{BOUNDARY}\n\nthe project zebra");
        let prompt = system_prompt(&text);
        assert_eq!(prompt[1], BOUNDARY);
        assert!(prompt[2].as_str().unwrap().contains("zebra"));
        assert!(!prompt[0].as_str().unwrap().contains("zebra"));
        assert_eq!(system_prompt("plain"), json!("plain"));
    }
}
