//! OpenCode through its own ACP agent, `opencode acp` (`docs/agents.md`, "OpenCode"): the bare
//! environment that leaves it nothing but Flottille's tools and instructions, and what
//! `flottille doctor` checks.
//!
//! OpenCode takes its configuration from files and variables, not from the session: everything
//! of its bare mode is in the variables the daemon sets on its process. What each one shuts, with
//! the place in OpenCode's source that shows it, is in `docs/agents.md`.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use flottille_core::agent::Agent;
use flottille_core::paths::{Paths, ensure_private_dir};
use serde_json::{Value, json};

use crate::claude::BOUNDARY;
use crate::providers::{self, Env, MCP_SERVER, Program, Provider};

/// The version of `opencode` whose bare mode was qualified (`docs/agents.md`).
pub const OPENCODE_VERSION: &str = "1.19.0";

/// The name of the agent Flottille defines in OpenCode's configuration, its only primary one.
const AGENT: &str = "flottille";

/// A tool call waiting on a run sends nothing for a while: OpenCode's MCP limit must outlast it.
const TOOL_WAIT_MS: u64 = 600_000;

/// The variables OpenCode's model providers take for a sign-in (`docs/agents.md`): passed to
/// OpenCode's process, and a sign-in for `flottille doctor`. A choice the maintainer can revise.
const API_KEYS: [&str; 10] = [
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GOOGLE_GENERATIVE_AI_API_KEY",
    "GEMINI_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "DEEPSEEK_API_KEY",
    "XAI_API_KEY",
    "OPENCODE_API_KEY",
];

pub static PROVIDER: Provider = Provider {
    name: "opencode",
    title: "OpenCode",
    program: "opencode",
    checked: OPENCODE_VERSION,
    may_change: "its bare mode was qualified on that version, and may not hold on another",
    install: "install OpenCode and sign in with it first",
    login_file,
    api_keys: &API_KEYS,
    sign_in: "run `opencode auth login` to sign in, or set the API key of a model provider \
              OpenCode takes, such as OPENAI_API_KEY",
    withheld,
    tool_prefix: "flottille_",
    stated_tool,
    command,
    env,
    session_meta,
    forget_process,
    forget_session,
    forget_agent,
};

/// Whether a variable of the daemon's environment stays out of OpenCode's (`docs/agents.md`).
/// The user's own OpenCode settings would undo the bare mode: Flottille sets the ones it needs.
pub fn withheld(name: &str) -> bool {
    if API_KEYS.contains(&name) {
        return false;
    }
    name.starts_with("OPENCODE_") || providers::secret_or_forge(name)
}

/// OpenCode names a permission's tool in the request's title only, which the model can shape:
/// no request is ever taken for one of Flottille's tools. Its bare agent allows those without
/// asking, so none should come.
fn stated_tool(_call: &Value) -> Option<&str> {
    None
}

fn command() -> Result<Program, String> {
    let program = providers::find_program("opencode")
        .ok_or("`opencode` is not on PATH: install OpenCode and sign in with it first")?;
    Ok((program, vec!["acp".into()]))
}

/// OpenCode's login file: `$XDG_DATA_HOME/opencode/auth.json`, or `~/.local/share/opencode`.
fn login_file() -> Option<PathBuf> {
    let data = match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(data) if data.is_absolute() => data,
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(data.join("opencode/auth.json"))
}

/// Everything Flottille keeps for OpenCode: its own configuration folder, an empty home and an
/// empty folder of managed settings, and the database of its sessions.
fn folder(paths: &Paths) -> PathBuf {
    paths.data.join("agents/opencode")
}

/// Where the agent's instructions are, for OpenCode to read when its process starts.
fn instructions_file(paths: &Paths, agent: &str) -> PathBuf {
    paths.runtime.join("opencode").join(format!("{agent}.md"))
}

/// The bare environment, the agent's instructions written where it names them.
fn env(paths: &Paths, agent: &Agent) -> Result<Env, String> {
    let root = folder(paths);
    for sub in ["config", "home", "managed"] {
        ensure_private_dir(&root.join(sub)).map_err(|e| e.to_string())?;
    }
    let file = instructions_file(paths, &agent.id);
    ensure_private_dir(file.parent().expect("a folder")).map_err(|e| e.to_string())?;
    let text = agent
        .instructions
        .replace(&format!("\n\n{BOUNDARY}\n\n"), "\n\n---\n\n");
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&file)
        .and_then(|mut out| out.write_all(text.as_bytes()))
        .map_err(|e| format!("cannot write the agent's instructions: {e}"))?;
    let password = password().map_err(|e| format!("cannot make OpenCode's password: {e}"))?;
    Ok(bare(paths, &file, &password)?
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect())
}

/// The variables of OpenCode's bare mode. `instructions` is the file of the agent's instructions;
/// `password` closes the HTTP server `opencode acp` runs for itself to anyone without it.
fn bare(
    paths: &Paths,
    instructions: &Path,
    password: &str,
) -> Result<Vec<(String, String)>, String> {
    let root = folder(paths);
    let path = |sub: &str| root.join(sub).to_string_lossy().into_owned();
    Ok(vec![
        // Its configuration folder: the user's `opencode.json`, agents, commands, plugins and
        // `AGENTS.md` stay in theirs.
        ("XDG_CONFIG_HOME".into(), path("config")),
        // The home it looks in for `~/.opencode`, `~/.claude/CLAUDE.md` and skills.
        ("OPENCODE_TEST_HOME".into(), path("home")),
        // The machine's managed settings (`/etc/opencode`), which would come after Flottille's.
        ("OPENCODE_TEST_MANAGED_CONFIG_DIR".into(), path("managed")),
        // Its sessions, and no signed-in OpenCode account to bring an organisation's settings.
        ("OPENCODE_DB".into(), path("opencode.db")),
        ("OPENCODE_DISABLE_PROJECT_CONFIG".into(), "1".into()),
        ("OPENCODE_DISABLE_CLAUDE_CODE".into(), "1".into()),
        ("OPENCODE_DISABLE_EXTERNAL_SKILLS".into(), "1".into()),
        ("OPENCODE_PURE".into(), "1".into()),
        ("OPENCODE_DISABLE_AUTOUPDATE".into(), "1".into()),
        ("OPENCODE_DISABLE_LSP_DOWNLOAD".into(), "1".into()),
        ("OPENCODE_SERVER_PASSWORD".into(), password.into()),
        (
            "OPENCODE_CONFIG_CONTENT".into(),
            config(instructions)?.to_string(),
        ),
    ])
}

/// The configuration Flottille gives OpenCode: one primary agent, its own, denied every tool but
/// Flottille's, its prompt the agent's instructions; nothing shared, nothing updated.
///
/// OpenCode replaces `{env:…}` and `{file:…}` in this text before it reads it: no text of the
/// agent's goes in it but the path of its instructions, whose file is read as it is.
fn config(instructions: &Path) -> Result<Value, String> {
    let shown = instructions.to_string_lossy();
    if shown.contains(['{', '}', '"', '\\']) || shown.chars().any(char::is_control) {
        return Err(format!(
            "Flottille's runtime folder, {shown}, holds a character OpenCode's configuration \
             cannot carry"
        ));
    }
    Ok(json!({
        "default_agent": AGENT,
        "agent": {
            AGENT: {
                "mode": "primary",
                "prompt": format!("{{file:{shown}}}"),
                // A tool whose last matching rule denies it for every pattern is not sent to the
                // model at all; MCP tools are named `<server>_<tool>` and go through the same rules.
                "permission": {"*": "deny", format!("{MCP_SERVER}_*"): "allow"},
            },
            "build": {"disable": true},
            "plan": {"disable": true},
            "general": {"disable": true},
            "explore": {"disable": true},
        },
        "share": "disabled",
        "autoupdate": false,
        "snapshot": false,
        "experimental": {"mcp_timeout": TOOL_WAIT_MS},
    }))
}

/// 32 hexadecimal digits from the system's random source.
fn password() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// OpenCode takes nothing of its bare mode from the session.
fn session_meta(_instructions: &str, _tools: &[String]) -> Value {
    json!({})
}

fn forget_process(_paths: &Paths, _pid: u32) {}

/// Its sessions are in OpenCode's database in Flottille's folder: they stay, as history.
fn forget_session(_paths: &Paths, _session: &str) {}

fn forget_agent(paths: &Paths, agent: &str) {
    let _ = std::fs::remove_file(instructions_file(paths, agent));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths {
            runtime: "/run/user/1000/flottille".into(),
            state: "/home/u/.local/state/flottille".into(),
            config: "/home/u/.config/flottille".into(),
            data: "/home/u/.local/share/flottille".into(),
        }
    }

    #[test]
    fn opencode_s_bare_environment_and_config_content_are_exactly_these() {
        let file = Path::new("/run/user/1000/flottille/opencode/3f9a1c0b7d2e.md");

        let env = bare(&paths(), file, "0123456789abcdef0123456789abcdef").unwrap();

        let data = "/home/u/.local/share/flottille/agents/opencode";
        let expected: Vec<(String, String)> = [
            ("XDG_CONFIG_HOME", format!("{data}/config")),
            ("OPENCODE_TEST_HOME", format!("{data}/home")),
            (
                "OPENCODE_TEST_MANAGED_CONFIG_DIR",
                format!("{data}/managed"),
            ),
            ("OPENCODE_DB", format!("{data}/opencode.db")),
            ("OPENCODE_DISABLE_PROJECT_CONFIG", "1".into()),
            ("OPENCODE_DISABLE_CLAUDE_CODE", "1".into()),
            ("OPENCODE_DISABLE_EXTERNAL_SKILLS", "1".into()),
            ("OPENCODE_PURE", "1".into()),
            ("OPENCODE_DISABLE_AUTOUPDATE", "1".into()),
            ("OPENCODE_DISABLE_LSP_DOWNLOAD", "1".into()),
            (
                "OPENCODE_SERVER_PASSWORD",
                "0123456789abcdef0123456789abcdef".into(),
            ),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
        assert_eq!(env[..expected.len()], expected[..]);
        assert_eq!(env.len(), expected.len() + 1);
        let (name, content) = &env[expected.len()];
        assert_eq!(name, "OPENCODE_CONFIG_CONTENT");
        let content: Value = serde_json::from_str(content).unwrap();
        assert_eq!(
            content,
            json!({
                "default_agent": "flottille",
                "agent": {
                    "flottille": {
                        "mode": "primary",
                        "prompt": "{file:/run/user/1000/flottille/opencode/3f9a1c0b7d2e.md}",
                        "permission": {"*": "deny", "flottille_*": "allow"},
                    },
                    "build": {"disable": true},
                    "plan": {"disable": true},
                    "general": {"disable": true},
                    "explore": {"disable": true},
                },
                "share": "disabled",
                "autoupdate": false,
                "snapshot": false,
                "experimental": {"mcp_timeout": 600000},
            })
        );
        // The deny rule comes before the allow: OpenCode takes the last rule that matches.
        let text = env.last().unwrap().1.as_str();
        assert!(text.find("\"*\":\"deny\"").unwrap() < text.find("\"flottille_*\"").unwrap());
        assert!(!text.contains("{env:"), "{text}");
    }

    #[test]
    fn a_path_opencode_s_configuration_cannot_carry_is_refused() {
        for path in [
            "/run/{env:HOME}/a.md",
            "/run/a\"b.md",
            "/run/a\\b.md",
            "/run/a\nb.md",
        ] {
            assert!(config(Path::new(path)).is_err(), "{path}");
        }
    }

    #[test]
    fn opencode_gets_its_model_providers_keys_and_not_the_user_s_opencode_settings_or_secrets() {
        for (name, gone) in [
            ("OPENAI_API_KEY", false),
            ("ANTHROPIC_API_KEY", false),
            ("OPENROUTER_API_KEY", false),
            ("OPENCODE_API_KEY", false),
            ("OPENCODE_CONFIG", true),
            ("OPENCODE_CONFIG_DIR", true),
            ("OPENCODE_PERMISSION", true),
            ("OPENCODE_SERVER_PASSWORD", true),
            ("OPENCODE_EXPERIMENTAL", true),
            ("GH_TOKEN", true),
            ("NPM_TOKEN", true),
            ("DB_PASSWORD", true),
            ("ANTHROPIC_AUTH_TOKEN", true),
            ("CLAUDE_CODE_OAUTH_TOKEN", true),
            ("HTTPS_PROXY", false),
            ("PATH", false),
            ("HOME", false),
            ("XDG_DATA_HOME", false),
        ] {
            assert_eq!(withheld(name), gone, "{name}");
        }
    }

    #[test]
    fn a_permission_request_never_names_a_tool() {
        let call = json!({"title": "flottille_fs_read", "_meta": {"claudeCode": {"toolName": "flottille_fs_read"}}});
        assert_eq!(stated_tool(&call), None);
    }
}
