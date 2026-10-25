//! Claude Code through Flottille's own bridge (`docs/agents.md`): how the daemon starts the
//! bridge, the bare options that leave the agent nothing but Flottille's tools, and what
//! `flottille doctor` checks.

use std::path::PathBuf;

use flottille_core::agent::Agent;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::providers::{self, Env, Program, Provider};

/// The version of `claude` whose stream-json mode the bridge was checked against
/// (`docs/reference/acp.md`).
pub const CLAUDE_VERSION: &str = "2.1.295";

/// Claude Code caches the system prompt before this marker across sessions.
pub const BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

/// A tool call waiting on the user sends nothing: both of Claude's limits must outlast the wait.
const TOOL_WAIT_MS: &str = "600000";

/// The agent's own sign-in, which counts as one for `flottille doctor` and passes to the agent,
/// where the secret-looking names `withheld` refuses would otherwise catch it.
const SIGN_IN: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
];

pub static PROVIDER: Provider = Provider {
    name: "claude",
    title: "Claude",
    program: "claude",
    checked: CLAUDE_VERSION,
    may_change: "its stream-json mode may have changed",
    install: "install Claude Code and sign in with it first",
    login_file,
    api_keys: &SIGN_IN,
    sign_in: "run `claude` and sign in, or set ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN; on \
              macOS the login is in the Keychain, where Flottille does not look",
    withheld,
    tool_prefix: "mcp__flottille__",
    stated_tool,
    command,
    env,
    session_meta,
    forget_process: forget_bridge,
    forget_session: forget_history,
    forget_agent,
};

/// Whether a variable of the daemon's environment stays out of the agent's (`docs/agents.md`).
/// The agent runs on the user's own sign-in and network, and on what Flottille sets: the daemon's
/// secrets and the user's Claude Code settings (a model, a cache lifetime) are not its business.
pub fn withheld(name: &str) -> bool {
    // The user's Claude Code settings.
    const SETTINGS: [&str; 4] = [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "MAX_THINKING_TOKENS",
        "FORCE_PROMPT_CACHING_5M",
    ];
    if SIGN_IN.contains(&name) {
        return false;
    }
    SETTINGS.contains(&name)
        || ["CLAUDE_CODE_", "DISABLE_"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        || (name.starts_with("ANTHROPIC_DEFAULT_") && name.ends_with("_MODEL"))
        || providers::secret_or_forge(name)
}

/// The tool a permission request names, as the bridge states it.
fn stated_tool(call: &Value) -> Option<&str> {
    call["_meta"]["claudeCode"]["toolName"].as_str()
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

/// Where each bridge writes the MCP configuration with its token, `<its pid>.mcp.json`.
pub fn bridge_files(paths: &Paths) -> PathBuf {
    paths.runtime.join("bridge")
}

/// The bridge of that pid is gone: its MCP configuration goes with it, even when it was killed
/// before it could remove it.
pub fn forget_bridge(paths: &Paths, pid: u32) {
    let _ = std::fs::remove_file(bridge_files(paths).join(format!("{pid}.mcp.json")));
}

/// At daemon start: no bridge of a previous daemon runs any more (its group was killed), and its
/// token was that daemon's. What they left goes.
pub fn forget_bridges(paths: &Paths) {
    let Ok(entries) = std::fs::read_dir(bridge_files(paths)) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with(".mcp.json") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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

/// Flottille's own bridge, `flottille bridge claude`, which drives the user's `claude`.
fn command() -> Result<Program, String> {
    if providers::find_program("claude").is_none() {
        return Err(
            "`claude` is not on PATH: install Claude Code and sign in with it first".into(),
        );
    }
    let program =
        std::env::current_exe().map_err(|e| format!("cannot find Flottille itself: {e}"))?;
    Ok((program, vec!["bridge".into(), "claude".into()]))
}

/// The bare mode is in the session's options: nothing to set on the process.
fn env(_paths: &Paths, _agent: &Agent) -> Result<Env, String> {
    Ok(Vec::new())
}

/// The bridge keeps nothing of an agent but its sessions' histories.
fn forget_agent(_paths: &Paths, _agent: &str) {}

/// Claude's login file: in `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
fn login_file() -> Option<PathBuf> {
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".claude"),
    };
    Some(dir.join(".credentials.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_system_prompt_keeps_the_cache_boundary_apart() {
        let text = format!("shared\n\n{BOUNDARY}\n\nthe project zebra");
        let prompt = system_prompt(&text);
        assert_eq!(prompt[1], BOUNDARY);
        assert!(prompt[2].as_str().unwrap().contains("zebra"));
        assert!(!prompt[0].as_str().unwrap().contains("zebra"));
        assert_eq!(system_prompt("plain"), json!("plain"));
    }
}
