//! Codex through Flottille's own bridge, `flottille bridge codex` (`docs/agents.md`, "Bare mode
//! for Codex"): how the daemon starts the bridge, the bare command line, environment and thread
//! that leave Codex nothing but Flottille's tools, and what `flottille doctor` checks.
//!
//! Codex has no ACP mode of its own: the bridge drives `codex app-server` on stdio. Codex has no
//! allowlist of tools either: its bare mode is the switches below, each with the place in
//! Codex's source that shows what it shuts in `docs/agents.md`, qualified on one version only.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use flottille_core::agent::Agent;
use flottille_core::paths::{Paths, ensure_private_dir};
use serde_json::{Value, json};

use crate::claude::BOUNDARY;
use crate::providers::{self, Env, MCP_SERVER, Program, Provider};

/// The version of `codex` whose bare mode was qualified (`docs/agents.md`).
pub const CODEX_VERSION: &str = "0.162.0";

/// A tool call waiting on a run sends nothing for a while: Codex's MCP limit must outlast it.
const TOOL_WAIT_SEC: u64 = 600;

/// The variable `codex app-server` takes for a sign-in (`login/src/auth/manager.rs:1536`):
/// passed to Codex's process, and a sign-in for `flottille doctor`. `CODEX_API_KEY` is not one:
/// the app-server does not read it (`app-server/src/lib.rs:550`).
const SIGN_IN: [&str; 1] = ["CODEX_ACCESS_TOKEN"];

/// The machine's own Codex settings, which every `codex app-server` loads and nothing turns off.
const MACHINE_SETTINGS: &str = "/etc/codex";

pub static PROVIDER: Provider = Provider {
    name: "codex",
    title: "Codex",
    program: "codex",
    checked: CODEX_VERSION,
    may_change: "its bare mode was qualified on that version, and may not hold on another",
    install: "install Codex and sign in with it first",
    login_file,
    api_keys: &SIGN_IN,
    sign_in: "run `codex login` with its login kept in a file (Codex's default), or set \
              CODEX_ACCESS_TOKEN; a login kept in the system's keyring does not reach Flottille's \
              agents",
    withheld,
    tool_prefix: "mcp__flottille__",
    stated_tool,
    command,
    env,
    session_meta,
    unready,
    forget_process,
    forget_session,
    forget_agent,
};

/// Whether a variable of the daemon's environment stays out of Codex's (`docs/agents.md`). The
/// user's own Codex settings (`CODEX_HOME` among them) would undo the bare mode, and other
/// sign-ins than Codex's own would reach what bare mode turns off (the realtime voice, a cloud
/// provider's models, `OTEL_*` exports with their headers).
pub fn withheld(name: &str) -> bool {
    if SIGN_IN.contains(&name) {
        return false;
    }
    ["CODEX_", "OPENAI_", "AWS_", "OTEL_"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || providers::secret_or_forge(name)
}

/// The bridge never asks a permission: Codex's own requests are refused in the bridge.
fn stated_tool(_call: &Value) -> Option<&str> {
    None
}

/// Flottille's own bridge, `flottille bridge codex`, which drives the user's `codex`.
fn command() -> Result<Program, String> {
    if providers::find_program("codex").is_none() {
        return Err("`codex` is not on PATH: install Codex and sign in with it first".into());
    }
    let program =
        std::env::current_exe().map_err(|e| format!("cannot find Flottille itself: {e}"))?;
    Ok((program, vec!["bridge".into(), "codex".into()]))
}

/// Codex's login file: in `$CODEX_HOME`, or `~/.codex`.
fn login_file() -> Option<PathBuf> {
    let dir = match std::env::var_os("CODEX_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".codex"),
    };
    Some(dir.join("auth.json"))
}

/// Codex's folders, Flottille's: its own `CODEX_HOME`, and an empty home.
pub fn folder(paths: &Paths) -> PathBuf {
    paths.data.join("agents/codex")
}

/// The folders made, and the user's login linked in Codex's own: Codex reads and refreshes it
/// there itself, Flottille never opens it. The bare mode is the bridge's to set on `codex`.
fn env(paths: &Paths, _agent: &Agent) -> Result<Env, String> {
    let root = folder(paths);
    for sub in ["codex", "home"] {
        ensure_private_dir(&root.join(sub)).map_err(|e| e.to_string())?;
    }
    if let Some(login) = login_file() {
        link_login(&root.join("codex/auth.json"), &login)
            .map_err(|e| format!("cannot link Codex's login: {e}"))?;
    }
    Ok(Vec::new())
}

/// `link` names `login`. A file that is not a link is a login made there, and stays.
fn link_login(link: &Path, login: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(link) {
        Ok(metadata) if !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) if std::fs::read_link(link)? == login => return Ok(()),
        Ok(_) => std::fs::remove_file(link)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    symlink(login, link)
}

/// Not ready while the machine has Codex settings of its own.
fn unready() -> Option<String> {
    machine_settings(Path::new(MACHINE_SETTINGS))
}

fn machine_settings(folder: &Path) -> Option<String> {
    std::fs::symlink_metadata(folder).ok().map(|_| {
        format!(
            "is not ready: {} exists, and Codex loads the machine's settings from it (its \
             configuration, requirements, rules, skills and hooks) before Flottille's, which \
             nothing turns off",
            folder.display()
        )
    })
}

/// `_meta` of `session/new` and `session/resume`: the agent's instructions and tools, which the
/// bridge gives Codex (`codex_bridge.rs`).
fn session_meta(instructions: &str, tools: &[String]) -> Value {
    json!({"codex": {
        "instructions": instructions.replace(&format!("\n\n{BOUNDARY}\n\n"), "\n\n---\n\n"),
        "tools": tools,
    }})
}

fn forget_process(_paths: &Paths, _pid: u32) {}

/// Its threads are in Codex's folder of Flottille's: they stay, as history.
fn forget_session(_paths: &Paths, _session: &str) {}

fn forget_agent(_paths: &Paths, _agent: &str) {}

/// `codex`'s arguments: its app server on stdio, every switch of the bare mode before it, as
/// overrides of the configuration that come after the files Codex reads.
pub fn arguments() -> Vec<String> {
    const SWITCHES: [&str; 32] = [
        // Nothing runs without Flottille's gate: no approval to ask, nothing written by Codex.
        "approval_policy=\"never\"",
        "sandbox_mode=\"read-only\"",
        "web_search=\"disabled\"",
        // No `AGENTS.md` of the project's.
        "project_doc_max_bytes=0",
        "allow_login_shell=false",
        "include_permissions_instructions=false",
        "include_apps_instructions=false",
        "analytics.enabled=false",
        "feedback.enabled=false",
        // No sub-agent.
        "agents.enabled=false",
        "tools.experimental_request_user_input.enabled=false",
        "skills.include_instructions=false",
        "skills.bundled.enabled=false",
        "features.shell_tool=false",
        "features.shell_snapshot=false",
        "features.view_image=false",
        "features.image_generation=false",
        "features.goals=false",
        "features.apps=false",
        "features.plugins=false",
        "features.remote_plugin=false",
        "features.tool_suggest=false",
        "features.sleep_tool=false",
        "features.memories=false",
        "features.hooks=false",
        "features.skill_mcp_dependency_install=false",
        "features.skip_host_skill_discovery=true",
        "features.stable_environment_tools=false",
        "features.code_mode=false",
        "features.code_mode_only=false",
        "features.multi_agent=false",
        "features.multi_agent_v2=false",
    ];
    let mut args = Vec::new();
    for switch in SWITCHES {
        args.push("-c".to_owned());
        args.push(switch.to_owned());
    }
    args.extend(["app-server", "--listen", "stdio://"].map(str::to_owned));
    args
}

/// The variables set on `codex`: Flottille's own Codex folder and an empty home, so that none of
/// the user's Codex configuration, skills, plugins or shell files is read, no Git configuration
/// of theirs, and no remote control.
pub fn environment(paths: &Paths) -> Vec<(String, String)> {
    let root = folder(paths);
    let path = |sub: &str| root.join(sub).to_string_lossy().into_owned();
    vec![
        ("CODEX_HOME".into(), path("codex")),
        ("HOME".into(), path("home")),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        (
            "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED".into(),
            "1".into(),
        ),
    ]
}

/// Flottille's MCP server as the thread's configuration names it, with the agent's tools only
/// (`tools`, as Codex names them) and its header, which goes through Codex's input, never its
/// command line: every call approved by Codex, the gate being Flottille's; listed upfront, not
/// searched for nor called from code.
pub fn server(url: &str, headers: &Value, tools: &[String]) -> Value {
    let tools: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool.strip_prefix(PROVIDER.tool_prefix))
        .collect();
    json!({MCP_SERVER: {
        "url": url,
        "http_headers": headers,
        "enabled_tools": tools,
        "required": true,
        "default_tools_approval_mode": "approve",
        "omit_tools_from": ["deferred", "code_mode"],
        "tool_timeout_sec": TOOL_WAIT_SEC,
    }})
}

/// The parameters of `thread/start` (and of `thread/resume`, with the thread's id): no
/// environment, so no command, file or image tool; the agent's instructions in place of
/// Codex's own; Flottille's server the only one.
pub fn thread(cwd: &str, instructions: &str, server: Value) -> Value {
    json!({
        "cwd": cwd,
        "approvalPolicy": "never",
        "sandbox": "read-only",
        "baseInstructions": instructions,
        "environments": [],
        "config": {"mcp_servers": server},
    })
}

/// The sign-ins whose workspace settings Codex loads from the cloud before Flottille's, which
/// nothing turns off (`cloud-config/src/service.rs:52-60`): a business, education or enterprise
/// ChatGPT plan. Personal plans and API keys are taken; a plan Flottille does not know is not.
pub fn refused_account(account: &Value) -> Option<String> {
    const PERSONAL: [&str; 6] = ["free", "go", "plus", "pro", "prolite", "promax"];
    if account["type"] != "chatgpt" {
        return None;
    }
    let plan = account["planType"].as_str().unwrap_or("unknown");
    (!PERSONAL.contains(&plan)).then(|| {
        format!(
            "Codex is signed in to a ChatGPT {plan} plan, whose workspace settings Codex loads \
             before Flottille's and nothing turns off: sign Codex in with a personal plan or an \
             API key to run its agents under Flottille"
        )
    })
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
    fn codex_s_bare_command_line_environment_and_thread_are_exactly_these() {
        let args = arguments();
        let (switches, rest) = args.split_at(args.len() - 3);
        assert_eq!(rest, ["app-server", "--listen", "stdio://"]);
        let pairs: Vec<&str> = switches
            .chunks(2)
            .map(|pair| {
                assert_eq!(pair[0], "-c");
                pair[1].as_str()
            })
            .collect();
        assert_eq!(
            pairs,
            [
                "approval_policy=\"never\"",
                "sandbox_mode=\"read-only\"",
                "web_search=\"disabled\"",
                "project_doc_max_bytes=0",
                "allow_login_shell=false",
                "include_permissions_instructions=false",
                "include_apps_instructions=false",
                "analytics.enabled=false",
                "feedback.enabled=false",
                "agents.enabled=false",
                "tools.experimental_request_user_input.enabled=false",
                "skills.include_instructions=false",
                "skills.bundled.enabled=false",
                "features.shell_tool=false",
                "features.shell_snapshot=false",
                "features.view_image=false",
                "features.image_generation=false",
                "features.goals=false",
                "features.apps=false",
                "features.plugins=false",
                "features.remote_plugin=false",
                "features.tool_suggest=false",
                "features.sleep_tool=false",
                "features.memories=false",
                "features.hooks=false",
                "features.skill_mcp_dependency_install=false",
                "features.skip_host_skill_discovery=true",
                "features.stable_environment_tools=false",
                "features.code_mode=false",
                "features.code_mode_only=false",
                "features.multi_agent=false",
                "features.multi_agent_v2=false",
            ]
        );

        let data = "/home/u/.local/share/flottille/agents/codex";
        assert_eq!(
            environment(&paths()),
            [
                ("CODEX_HOME", format!("{data}/codex")),
                ("HOME", format!("{data}/home")),
                ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
                ("GIT_CONFIG_NOSYSTEM", "1".into()),
                (
                    "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED",
                    "1".into()
                ),
            ]
            .map(|(name, value)| (name.to_owned(), value))
        );

        let tools = [
            "mcp__flottille__fs_read".to_owned(),
            "mcp__flottille__report".to_owned(),
        ];
        let server = server(
            "http://127.0.0.1:4100/mcp",
            &json!({"Authorization": "Bearer t"}),
            &tools,
        );
        assert_eq!(
            thread("/w/app", "Be brief.", server),
            json!({
                "cwd": "/w/app",
                "approvalPolicy": "never",
                "sandbox": "read-only",
                "baseInstructions": "Be brief.",
                "environments": [],
                "config": {"mcp_servers": {"flottille": {
                    "url": "http://127.0.0.1:4100/mcp",
                    "http_headers": {"Authorization": "Bearer t"},
                    "enabled_tools": ["fs_read", "report"],
                    "required": true,
                    "default_tools_approval_mode": "approve",
                    "omit_tools_from": ["deferred", "code_mode"],
                    "tool_timeout_sec": 600,
                }}},
            })
        );
    }

    #[test]
    fn the_instructions_go_whole_with_the_tools() {
        let meta = session_meta(
            &format!("shared\n\n{BOUNDARY}\n\nthe project"),
            &["mcp__flottille__report".to_owned()],
        );
        assert_eq!(
            meta,
            json!({"codex": {"instructions": "shared\n\n---\n\nthe project",
                "tools": ["mcp__flottille__report"]}})
        );
    }

    #[test]
    fn codex_gets_its_sign_in_and_the_network_and_not_the_user_s_codex_settings_or_secrets() {
        for (name, gone) in [
            ("CODEX_ACCESS_TOKEN", false),
            ("CODEX_HOME", true),
            ("CODEX_API_KEY", true),
            ("CODEX_SQLITE_HOME", true),
            ("CODEX_CONNECTORS_TOKEN", true),
            ("OPENAI_API_KEY", true),
            ("OPENAI_BASE_URL", true),
            ("AWS_ACCESS_KEY_ID", true),
            ("AWS_BEARER_TOKEN_BEDROCK", true),
            ("OTEL_EXPORTER_OTLP_HEADERS", true),
            ("GH_TOKEN", true),
            ("ANTHROPIC_API_KEY", true),
            ("DB_PASSWORD", true),
            ("HTTPS_PROXY", false),
            ("SSL_CERT_FILE", false),
            ("PATH", false),
            ("HOME", false),
            ("LANG", false),
        ] {
            assert_eq!(withheld(name), gone, "{name}");
        }
    }

    #[test]
    fn the_login_is_linked_never_read_and_a_login_made_there_stays() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("auth.json");
        let login = dir.path().join("user/auth.json");

        link_login(&link, &login).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), login);
        link_login(&link, &login).unwrap();
        let other = dir.path().join("other/auth.json");
        link_login(&link, &other).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), other);

        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "made here").unwrap();
        link_login(&link, &login).unwrap();
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "made here");
    }

    #[test]
    fn machine_settings_of_codex_s_make_it_unready() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("codex");
        assert_eq!(machine_settings(&folder), None);
        std::fs::create_dir(&folder).unwrap();
        let why = machine_settings(&folder).unwrap();
        assert!(why.starts_with("is not ready: "), "{why}");
        assert!(why.contains("nothing turns off"), "{why}");
    }

    #[test]
    fn a_workspace_plan_s_sign_in_is_refused_and_a_personal_one_or_a_key_is_taken() {
        for plan in ["free", "go", "plus", "pro", "prolite", "promax"] {
            let account = json!({"type": "chatgpt", "email": null, "planType": plan});
            assert_eq!(refused_account(&account), None, "{plan}");
        }
        for plan in [
            "team",
            "business",
            "enterprise",
            "edu",
            "ent26",
            "unknown",
            "new_one",
        ] {
            let account = json!({"type": "chatgpt", "email": null, "planType": plan});
            let why = refused_account(&account).unwrap();
            assert!(why.contains(&format!("ChatGPT {plan} plan")), "{why}");
        }
        assert_eq!(refused_account(&json!({"type": "apiKey"})), None);
    }
}
