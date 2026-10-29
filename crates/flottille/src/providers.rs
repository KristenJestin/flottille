//! The providers agents run on (`docs/agents.md`, "Providers"). Each is an entry: how its ACP agent
//! starts in bare mode, how it is checked ready, how it names Flottille's tools. The tree, the
//! deliveries, the gate and the store know a provider by its name only.
//!
//! Which providers are enabled is the user's `providers.toml`; the models each offers are learned
//! from its agents' sessions and kept by the daemon, per version of the provider.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use flottille_core::agent::{Agent, Choices};
use flottille_core::paths::Paths;
use flottille_core::project::Project;
use flottille_core::provider::{self, Judge, Settings};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::{self, Failure};
use crate::{claude, codex, opencode};

/// The name of Flottille's MCP server, the only one an agent has.
pub const MCP_SERVER: &str = "flottille";

/// One provider.
pub struct Provider {
    /// Its name in the protocol, the configuration and the store.
    pub name: &'static str,
    pub title: &'static str,
    /// The command line the user installed and signed in to.
    pub program: &'static str,
    /// The version of `program` this entry was checked against.
    pub checked: &'static str,
    /// Other versions whose bare mode was qualified as well.
    pub also_qualified: &'static [&'static str],
    /// What another version may have changed, for the warning that names both.
    pub may_change: &'static str,
    /// What to do when `program` is missing.
    pub install: &'static str,
    /// Its login file, checked for presence only: never opened.
    pub login_file: fn() -> Option<PathBuf>,
    /// The variables taken for a sign-in when one is set: never read, never shown.
    pub api_keys: &'static [&'static str],
    /// What to do to sign in.
    pub sign_in: &'static str,
    /// Whether a variable of the daemon's environment stays out of the agent's.
    pub withheld: fn(&str) -> bool,
    /// Flottille's tool `fs_read` is `<prefix>fs_read` for the agent.
    pub tool_prefix: &'static str,
    /// The tool a permission request of the agent names, when the agent states one; a title is
    /// text the model can shape, never taken for a name.
    pub stated_tool: fn(&Value) -> Option<&str>,
    /// The program and the arguments that start its ACP agent.
    pub command: fn() -> Result<Program, String>,
    /// The variables Flottille sets on the agent's process for its bare mode.
    pub env: fn(&Paths, &Agent) -> Result<Env, String>,
    /// `_meta` of `session/new`, `session/load` and `session/resume`.
    pub session_meta: fn(&str, &[String]) -> Value,
    /// Why its agents cannot run on this machine whatever the user does in Flottille, as a
    /// clause after the provider's name; `None` when nothing stops them.
    pub unready: fn() -> Option<String>,
    /// The process of that pid is gone: what Flottille kept for it goes.
    pub forget_process: fn(&Paths, u32),
    /// A session of the agent is over (replaced, or the agent ended).
    pub forget_session: fn(&Paths, &str),
    /// The agent ended: what Flottille kept for it goes.
    pub forget_agent: fn(&Paths, &str),
}

/// Every provider, in the order of `flottille_core::provider::NAMES`.
/// A program and its arguments.
pub type Program = (PathBuf, Vec<OsString>);

/// Variables set for a process.
pub type Env = Vec<(OsString, OsString)>;

pub static PROVIDERS: [&Provider; 3] = [&claude::PROVIDER, &opencode::PROVIDER, &codex::PROVIDER];

/// The provider of that name.
pub fn get(name: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().copied().find(|p| p.name == name)
}

/// The provider an agent runs on: it was checked when the agent was made.
pub fn of(agent: &Agent) -> Result<&'static Provider, String> {
    get(&agent.provider).ok_or_else(|| format!("there is no provider {}", agent.provider))
}

/// The names of the tools every agent has, in their fixed order (`docs/design.md` §4.3). Their
/// names are frozen in each agent, because the prompt cache depends on them.
const FILE_AND_COMMAND_TOOLS: [&str; 8] = [
    "fs_read",
    "fs_list",
    "search",
    "fs_write",
    "fs_edit",
    "commands_list",
    "commands_run",
    "commands_status",
];
const AGENT_TOOLS: [&str; 4] = ["agent_launch", "agent_send", "agent_status", "agent_stop"];
const ACTION_TOOLS: [&str; 3] = ["skills", "commands", "project"];

impl Provider {
    /// The exact tool names an agent gets, as the provider names them: what it can do decides
    /// (`docs/design.md` §4.3, `docs/reference/mcp.md` §4 rule 9). A root asks the user, a child
    /// its parent; an agent at the depth limit cannot have children, so it has no `agent_*` tool
    /// and nobody's request to answer. The same tools in `main` as in a workspace: `main` names
    /// the workspace they act on. A tool that can only refuse is not listed (`docs/reference/mcp.md`
    /// §4 rule 9): `forge_read` and `forge_write` need a repository with a declared forge,
    /// `ticket_read` a declared tracker; `workspace_remove` acts in any project. What each may do
    /// is the gate's business, not the list's.
    pub fn tools(
        &self,
        root: bool,
        at_depth_limit: bool,
        forge: bool,
        tracker: bool,
    ) -> Vec<String> {
        let mut names: Vec<&str> = FILE_AND_COMMAND_TOOLS.to_vec();
        if forge {
            names.push("forge_read");
        }
        names.push("skill_read");
        if tracker {
            names.push("ticket_read");
        }
        if forge {
            names.push("forge_write");
        }
        names.push("workspace_remove");
        if !at_depth_limit {
            names.extend(AGENT_TOOLS);
        }
        // What the user does in the window, the agent they talk to does too, asking first
        // (`docs/tools.md`, "The project"); a child does its brief and nothing of the project's.
        if root {
            names.extend(ACTION_TOOLS);
        }
        names.push("report");
        names.push(if root { "ask_user" } else { "ask_parent" });
        if !at_depth_limit {
            names.push("approval_answer");
        }
        names
            .into_iter()
            .map(|tool| format!("{}{tool}", self.tool_prefix))
            .collect()
    }

    /// Flottille's name of a tool the provider names `tool`.
    pub fn tool_of<'a>(&self, tool: &'a str) -> Option<&'a str> {
        tool.strip_prefix(self.tool_prefix)
    }
}

/// How an agent's process starts.
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Set on the process, after the daemon's environment less what is withheld.
    pub env: Vec<(OsString, OsString)>,
}

/// The provider's agent for that agent: its program, or, for tests and development, the one
/// `FLOTTILLE_AGENT_COMMAND_<NAME>` or `FLOTTILLE_AGENT_COMMAND` names in its place (run with no
/// argument), and its bare environment either way. A sentence the user can act on when
/// something is missing.
pub fn launch(provider: &Provider, paths: &Paths, agent: &Agent) -> Result<Launch, String> {
    let (program, args) = match overridden(provider) {
        Some(program) => (program, Vec::new()),
        None => (provider.command)()?,
    };
    Ok(Launch {
        program,
        args,
        env: (provider.env)(paths, agent)?,
    })
}

/// Whether an agent's process may start on the provider now: a sentence saying why not.
pub fn startable(paths: &Paths, provider: &'static Provider) -> Result<(), String> {
    let settings = Settings::load(paths)?;
    match Check::of(provider, &settings, Version::Ask).why_not() {
        Some(why) => Err(format!("provider {} {why}", provider.name)),
        None => Ok(()),
    }
}

/// The test override of the provider's agent, when one is set.
pub fn overridden(provider: &Provider) -> Option<PathBuf> {
    let own = format!("FLOTTILLE_AGENT_COMMAND_{}", provider.name.to_uppercase());
    std::env::var_os(own)
        .or_else(|| std::env::var_os("FLOTTILLE_AGENT_COMMAND"))
        .filter(|program| !program.is_empty())
        .map(PathBuf::from)
}

/// Nothing on the machine stops the provider's agents.
pub fn ready_here() -> Option<String> {
    None
}

/// Forge tokens and the folders of their command lines' configuration, and any variable named
/// like a secret: no agent's business, whatever its provider (`docs/agents.md`, "The agent's
/// environment"). Each provider lets its own sign-in through.
pub fn secret_or_forge(name: &str) -> bool {
    const FORGES: [&str; 8] = [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "GITLAB_TOKEN",
        "GL_TOKEN",
        "GH_CONFIG_DIR",
        "GLAB_CONFIG_DIR",
    ];
    FORGES.contains(&name)
        || ["BITBUCKET_", "BKT_"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        || ["_TOKEN", "_SECRET", "_PASSWORD", "_API_KEY"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
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

/// How much a check looks at the program's version.
#[derive(Clone, Copy, PartialEq)]
pub enum Version {
    /// Not at all.
    Skip,
    /// What an earlier check asked, when the program has not changed since: never waits.
    Known,
    /// `<program> --version`, given 5 s, unless an earlier check asked it of the same program.
    Ask,
}

/// What is known of a provider on this machine: whether it is enabled, its program, its login.
pub struct Check {
    pub provider: &'static Provider,
    pub enabled: bool,
    /// The test override that stands for its agent.
    pub overridden: Option<PathBuf>,
    pub program: Option<PathBuf>,
    /// `None` when not asked for, or not known.
    pub version: Option<String>,
    /// Whether `version` was read from the program: `None` then means it could not say.
    pub version_read: bool,
    /// The user allows a version other than the qualified one (`allow_unqualified_version`).
    pub unqualified_allowed: bool,
    pub login: Option<PathBuf>,
    pub login_present: bool,
    /// The sign-in variable found set, by name.
    pub api_key: Option<&'static str>,
    /// Every sign-in variable found set, by name, in the provider's order.
    pub api_keys: Vec<&'static str>,
}

impl Check {
    /// Checks the provider, looking at its program's version as `version` says.
    pub fn of(provider: &'static Provider, settings: &Settings, version: Version) -> Check {
        let overridden = overridden(provider);
        let program = find_program(provider.program);
        // A test override stands for the program: it is never asked its version.
        let read = match (&program, version != Version::Skip && overridden.is_none()) {
            (Some(program), true) => version_of(program, version == Version::Ask),
            _ => None,
        };
        let version_read = read.is_some();
        let version = read.flatten();
        let login = (provider.login_file)();
        // Only its presence: the file is never opened, the key never read.
        let login_present = login
            .as_deref()
            .is_some_and(|p| std::fs::symlink_metadata(p).is_ok());
        let api_keys: Vec<&'static str> = provider
            .api_keys
            .iter()
            .copied()
            .filter(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
            .collect();
        let api_key = api_keys.first().copied();
        Check {
            provider,
            enabled: settings.enabled(provider.name),
            overridden,
            program,
            version,
            version_read,
            unqualified_allowed: settings.allows_unqualified_version(provider.name),
            login,
            login_present,
            api_key,
            api_keys,
        }
    }

    /// Why its agents cannot run now, as a clause after the provider's name; `None` when they can.
    pub fn why_not(&self) -> Option<String> {
        let provider = self.provider;
        if !self.enabled {
            return Some(format!(
                "is not enabled (`enabled = true` under [{}] in providers.toml enables it)",
                provider.name
            ));
        }
        if self.overridden.is_some() {
            return None;
        }
        if self.program.is_none() {
            return Some(format!(
                "is not ready: `{}` is not on PATH ({})",
                provider.program, provider.install
            ));
        }
        if let Some(why) = (provider.unready)() {
            return Some(why);
        }
        if self.version_refused() {
            let allow = format!(
                "`allow_unqualified_version = true` under [{}] in providers.toml runs it anyway, \
                 at the user's own risk",
                provider.name
            );
            let qualified = match provider.also_qualified {
                [] => format!("{} was", provider.checked),
                more => format!("{} and {} were", provider.checked, more.join(" and ")),
            };
            return Some(match &self.version {
                Some(version) => format!(
                    "is not ready: {} is {version}, and only {qualified} qualified for its bare \
                     mode ({allow})",
                    provider.program
                ),
                None => format!(
                    "is not ready: `{} --version` did not say its version, and only {qualified} \
                     qualified for its bare mode ({allow})",
                    provider.program
                ),
            });
        }
        // A missing login is never a refusal: the provider may be signed in where the daemon
        // cannot look (the macOS Keychain, a cloud's own credentials), or need no sign-in.
        None
    }

    /// Whether the program's version, read, is one its bare mode does not hold on for sure and
    /// the user did not allow.
    fn version_refused(&self) -> bool {
        provider::QUALIFIED_ONLY.contains(&self.provider.name)
            && !self.unqualified_allowed
            && self.version_read
            && !self.version.as_deref().is_some_and(|v| self.qualified(v))
    }

    fn qualified(&self, version: &str) -> bool {
        version == self.provider.checked || self.provider.also_qualified.contains(&version)
    }

    pub fn ready(&self) -> bool {
        self.why_not().is_none()
    }

    /// What may stop its agents that the daemon cannot tell for sure: a version other than the
    /// one this Flottille was checked against, no sign-in it can see.
    pub fn warnings(&self) -> Vec<String> {
        let provider = self.provider;
        let mut warnings = Vec::new();
        if let Some(version) = self.version.as_deref()
            && !self.qualified(version)
            && !self.version_refused()
        {
            warnings.push(format!(
                "{} is {version}, and this Flottille was checked against {}: {}",
                provider.program, provider.checked, provider.may_change
            ));
        }
        if self.enabled
            && self.overridden.is_none()
            && self.program.is_some()
            && !self.login_present
            && self.api_key.is_none()
        {
            warnings.push(format!(
                "{} is not signed in where Flottille can see it, and its agents fail if it is not \
                 signed in at all: {}",
                provider.title, provider.sign_in
            ));
        }
        warnings
    }

    /// The provider as `flottille doctor` and `provider.list` report it.
    pub fn to_json(&self) -> Value {
        let provider = self.provider;
        json!({
            "name": provider.name,
            "title": provider.title,
            "enabled": self.enabled,
            "ready": self.ready(),
            "why": self.why_not(),
            "program": {
                "name": provider.program,
                "present": self.program.is_some(),
                "path": self.program,
                "version": self.version,
                "checked": provider.checked,
            },
            "login": {
                "present": self.login_present,
                "path": self.login,
                "api_key": self.api_key,
            },
            "override": self.overridden,
        })
    }
}

/// Every provider checked, with the user's settings.
pub fn checks(settings: &Settings, version: Version) -> Vec<Check> {
    PROVIDERS
        .iter()
        .map(|provider| Check::of(provider, settings, version))
        .collect()
}

/// What `flottille doctor` reports: every provider, enabled or not, ready or not and why. It is
/// ready when at least one provider is enabled and every enabled one is ready.
pub fn doctor(paths: &Paths) -> Value {
    let (settings, mut warnings) = match Settings::load(paths) {
        Ok(settings) => (settings, Vec::new()),
        Err(error) => (Settings::default(), vec![error]),
    };
    let checks = checks(&settings, Version::Ask);
    warnings.extend(checks.iter().flat_map(Check::warnings));
    let ready = checks.iter().any(|c| c.enabled) && checks.iter().all(|c| !c.enabled || c.ready());
    json!({
        "providers": checks.iter().map(Check::to_json).collect::<Vec<_>>(),
        "warnings": warnings,
        "ready": ready,
    })
}

/// `--version` answers, by program path and its modification time.
static VERSIONS: Mutex<Option<Versions>> = Mutex::new(None);

type Versions = HashMap<(PathBuf, SystemTime), Option<String>>;

/// `<program> --version`, given 5 s: the first version number it prints, `None` inside when it
/// printed none. Kept while the program is the same file: a new version of it is asked again.
/// Without `ask`, only what was kept; `None` when nothing was.
fn version_of(program: &Path, ask: bool) -> Option<Option<String>> {
    let modified = std::fs::metadata(program).and_then(|m| m.modified()).ok();
    let Some(modified) = modified else {
        return ask.then_some(None);
    };
    let key = (program.to_owned(), modified);
    if let Some(known) = VERSIONS.lock().unwrap().get_or_insert_default().get(&key) {
        return Some(known.clone());
    }
    if !ask {
        return None;
    }
    let version = ask_version(program);
    VERSIONS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(key, version.clone());
    Some(version)
}

fn ask_version(program: &Path) -> Option<String> {
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

// The models.

/// What a provider's models are kept under: its program file, by path and modification time,
/// which a new version replaces (a test override for a test's), and the names of its API keys
/// set, which decide some of the models it offers; never their values. Never `--version`: the
/// daemon runs on one thread, and a program slow to answer would hold every client.
fn models_version(check: &Check) -> Option<String> {
    let program = if check.overridden.is_some() {
        "override".to_owned()
    } else {
        let program = check.program.as_ref()?;
        let modified = std::fs::metadata(program).and_then(|m| m.modified()).ok()?;
        let since = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?;
        format!("{} {}", program.display(), since.as_nanos())
    };
    Some(format!("{program}; keys {}", check.api_keys.join(",")))
}

/// The models a provider's agents last offered, each `{"id", "name", "efforts"}` (`efforts`
/// `null` until the model was seen chosen) and `"default": true` on the one a new session takes,
/// and whether they were read from the version of the provider there is now.
fn last_models(daemon: &Daemon, check: &Check) -> Option<(Vec<Value>, bool)> {
    let (version, models) = daemon
        .read(|store| store.provider_models(check.provider.name))
        .ok()??;
    let current = version == models_version(check);
    Some((models.as_array().cloned().unwrap_or_default(), current))
}

/// The models a provider's agents offered, when they were read from the version of the provider
/// there is now: only these refuse a model or an effort.
fn known_models(daemon: &Daemon, check: &Check) -> Option<Vec<Value>> {
    last_models(daemon, check).and_then(|(models, current)| current.then_some(models))
}

/// What a session of the provider's agent offers in its config options, kept: its models, the
/// efforts of the model it has now, and, `fresh` (a new session none was set on), that this
/// model is the default. What another version offered goes.
pub fn learn(daemon: &Daemon, provider: &'static Provider, options: &Value, fresh: bool) {
    let find = |category: &str| {
        options
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| o["category"] == category)
    };
    let Some(model) = find("model") else { return };
    let current = model["currentValue"].as_str().unwrap_or_default();
    let efforts = find("thought_level").map(values).unwrap_or_default();
    let settings = Settings::load(&daemon.paths).unwrap_or_default();
    let check = Check::of(provider, &settings, Version::Skip);
    let before = known_models(daemon, &check).unwrap_or_default();
    let models: Vec<Value> = named_values(model)
        .into_iter()
        .map(|(id, name)| {
            let known = before.iter().find(|m| m["id"] == id.as_str());
            let efforts = if id == current {
                json!(efforts)
            } else {
                known.map_or(Value::Null, |m| m["efforts"].clone())
            };
            let default = if fresh {
                id == current
            } else {
                known.is_some_and(|m| m["default"] == true)
            };
            let mut model = json!({"id": id, "name": name, "efforts": efforts});
            if default {
                model["default"] = true.into();
            }
            model
        })
        .collect();
    let version = models_version(&check);
    let _ = daemon.read(|store| {
        store.set_provider_models(provider.name, version.as_deref(), &Value::Array(models))
    });
}

/// The values a select option offers, in order, groups flattened.
pub fn values(option: &Value) -> Vec<String> {
    named_values(option).into_iter().map(|(id, _)| id).collect()
}

/// The values a select option offers and the name each is shown by, its value when it has none.
fn named_values(option: &Value) -> Vec<(String, String)> {
    fn collect(values: &Value, out: &mut Vec<(String, String)>) {
        for value in values.as_array().into_iter().flatten() {
            if let Some(id) = value["value"].as_str() {
                let name = value["name"].as_str().unwrap_or(id);
                out.push((id.to_owned(), name.to_owned()));
            } else {
                collect(&value["options"], out);
            }
        }
    }
    let mut out = Vec::new();
    collect(&option["options"], &mut out);
    out
}

// Choosing.

/// What an agent is asked to run on.
pub struct Asked {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The project's provider: its own, else the first one enabled.
fn project_provider(project: &Project, settings: &Settings) -> Option<&'static Provider> {
    match &project.provider {
        Some(name) => get(name),
        None => PROVIDERS
            .iter()
            .copied()
            .find(|provider| settings.enabled(provider.name)),
    }
}

/// The provider and the choices of a new agent, checked against the providers enabled and ready
/// and the models they offer: a refusal names what is available.
pub fn choose(
    daemon: &Daemon,
    project: &Project,
    asked: Asked,
) -> Result<(&'static Provider, Choices), Failure> {
    let settings =
        Settings::load(&daemon.paths).map_err(|error| Failure::new("invalid-config", error))?;
    let provider = match &asked.provider {
        Some(name) => get(name).ok_or_else(|| {
            Failure::refused(format!(
                "there is no provider {name}: the providers are {}",
                provider::NAMES.join(", ")
            ))
        })?,
        None => project_provider(project, &settings).ok_or_else(|| {
            Failure::refused(
                "no provider is enabled: enable one in providers.toml (see `flottille doctor`)",
            )
        })?,
    };
    let checks = checks(&settings, Version::Known);
    let check = checks
        .iter()
        .find(|c| c.provider.name == provider.name)
        .expect("every provider is checked");
    if let Some(why) = check.why_not() {
        let ready: Vec<&str> = checks
            .iter()
            .filter(|c| c.ready())
            .map(|c| c.provider.name)
            .collect();
        let others = if ready.is_empty() {
            "no provider is ready: see `flottille doctor`".to_owned()
        } else {
            format!("the ready providers are {}", ready.join(", "))
        };
        return Err(Failure::refused(format!(
            "provider {} {why}; {others}",
            provider.name
        )));
    }
    let model = asked.model.or_else(|| {
        project.model.clone().filter(|_| {
            project_provider(project, &settings).is_some_and(|p| p.name == provider.name)
        })
    });
    // What the provider offers is known once one of its sessions said it; until then a model or
    // an effort it does not offer fails the agent when its session opens. Without a model, an
    // effort is the default model's, once a new session said which it is.
    let check = Check::of(provider, &settings, Version::Skip);
    if let Some(models) = known_models(daemon, &check) {
        let offered: Vec<&str> = models.iter().filter_map(|m| m["id"].as_str()).collect();
        let target = match &model {
            Some(model) => match models.iter().find(|m| m["id"] == model.as_str()) {
                Some(known) => Some((known, format!("model {model}"))),
                None => {
                    return Err(Failure::refused(format!(
                        "provider {} offers no model {model}: it offers {}",
                        provider.name,
                        offered.join(", ")
                    )));
                }
            },
            None => models
                .iter()
                .find(|m| m["default"] == true)
                .and_then(|known| {
                    Some((known, format!("default model {}", known["id"].as_str()?)))
                }),
        };
        if let (Some(effort), Some((known, named))) = (&asked.effort, target)
            && let Some(efforts) = known["efforts"].as_array()
            && !efforts.iter().any(|e| e == effort.as_str())
        {
            let efforts: Vec<&str> = efforts.iter().filter_map(Value::as_str).collect();
            return Err(Failure::refused(if efforts.is_empty() {
                format!(
                    "provider {}'s {named} has no effort to choose",
                    provider.name
                )
            } else {
                format!(
                    "provider {}'s {named} offers no effort {effort}: it offers {}",
                    provider.name,
                    efforts.join(", ")
                )
            }));
        }
    }
    Ok((
        provider,
        Choices {
            model,
            effort: asked.effort,
            mode: None,
        },
    ))
}

// The method.

/// Answers a `provider.*` request.
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "provider.list" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct List {
                #[serde(default)]
                project: Option<String>,
            }
            let List { project } = match params {
                Value::Null => List { project: None },
                params => serde_json::from_value(params)?,
            };
            let project = project
                .map(|name| projects::load_project(&daemon, &name))
                .transpose()?;
            // `--version` may take seconds: off the daemon's workers.
            tokio::task::spawn_blocking(move || list(&daemon, project.as_ref()))
                .await
                .map_err(|e| Failure::new("unavailable", e.to_string()))?
        }
        "provider.enable" | "provider.disable" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Named {
                name: String,
            }
            let Named { name } = serde_json::from_value(params)?;
            let enabled = method == "provider.enable";
            let provider = get(&name).ok_or_else(|| {
                Failure::new(
                    "not-found",
                    format!(
                        "there is no provider {name}: the providers are {}",
                        provider::NAMES.join(", ")
                    ),
                )
            })?;
            let announced = if set_enabled(&daemon.paths, provider.name, enabled)? {
                daemon
                    .emit(
                        "provider.changed",
                        json!({"name": provider.name, "enabled": enabled}),
                    )
                    .map(drop)
                    .map_err(|error| error.to_string())
            } else {
                Ok(())
            };
            Ok(switched(provider.name, enabled, announced))
        }
        "provider.set_judge" => {
            // The user's choice only: no tool of an agent reaches this method.
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Chosen {
                provider: Option<String>,
                #[serde(default)]
                model: Option<String>,
            }
            let Chosen { provider, model } = serde_json::from_value(params)?;
            let judge = match provider {
                None => None,
                Some(name) => {
                    let provider = get(&name).ok_or_else(|| {
                        Failure::new(
                            "not-found",
                            format!(
                                "there is no provider {name}: the providers are {}",
                                provider::NAMES.join(", ")
                            ),
                        )
                    })?;
                    Some(Judge {
                        provider: provider.name.to_owned(),
                        model: model.filter(|m| !m.trim().is_empty()),
                    })
                }
            };
            if set_judge(&daemon.paths, judge.as_ref())? {
                daemon
                    .emit("provider.judge_changed", json!({"judge": judge}))
                    .map_err(|error| Failure::new("unavailable", error.to_string()))?;
            }
            Ok(json!({"judge": judge}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// One change of `providers.toml` at a time: each reads what the one before wrote.
static WRITING: Mutex<()> = Mutex::new(());

/// Sets `enabled` under the provider's table of `providers.toml`, the rest of the file kept as
/// the user wrote it, comments included; the file is made when there is none. Whether it
/// changed. A file the daemon cannot read is refused and left as it is.
fn set_enabled(paths: &Paths, name: &str, enabled: bool) -> Result<bool, Failure> {
    let _writing = WRITING.lock().unwrap();
    let file = provider::file(paths);
    let unavailable =
        |error: std::io::Error| Failure::new("unavailable", format!("providers.toml: {error}"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(unavailable(error)),
    };
    let before = Settings::parse(&text).map_err(|error| Failure::new("invalid-config", error))?;
    if before.enabled(name) == enabled {
        return Ok(false);
    }
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            Failure::new(
                "invalid-config",
                format!("providers.toml: {}", error.message()),
            )
        })?;
    if !document.contains_key(name) {
        document.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let table = document[name].as_table_like_mut().ok_or_else(|| {
        Failure::new(
            "invalid-config",
            format!("providers.toml: {name}: not a table"),
        )
    })?;
    match table
        .get_mut("enabled")
        .and_then(toml_edit::Item::as_value_mut)
    {
        // The comment after the value stays with it.
        Some(value) => {
            let decor = value.decor().clone();
            *value = enabled.into();
            *value.decor_mut() = decor;
        }
        None => {
            table.insert("enabled", toml_edit::value(enabled));
        }
    }
    let text = document.to_string();
    Settings::parse(&text).map_err(|error| Failure::new("invalid-config", error))?;
    std::fs::create_dir_all(&paths.config).map_err(unavailable)?;
    write_in_place(&file, &text).map_err(unavailable)?;
    Ok(true)
}

/// Sets `[judge]` in `providers.toml`, or removes it when `judge` is none, the rest of the file
/// kept as the user wrote it. Whether it changed.
fn set_judge(paths: &Paths, judge: Option<&Judge>) -> Result<bool, Failure> {
    let _writing = WRITING.lock().unwrap();
    let file = provider::file(paths);
    let unavailable =
        |error: std::io::Error| Failure::new("unavailable", format!("providers.toml: {error}"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(unavailable(error)),
    };
    let before = Settings::parse(&text).map_err(|error| Failure::new("invalid-config", error))?;
    if before.judge.as_ref() == judge {
        return Ok(false);
    }
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            Failure::new(
                "invalid-config",
                format!("providers.toml: {}", error.message()),
            )
        })?;
    document.remove("judge");
    if let Some(judge) = judge {
        let mut table = toml_edit::Table::new();
        table.insert("provider", toml_edit::value(judge.provider.as_str()));
        if let Some(model) = &judge.model {
            table.insert("model", toml_edit::value(model.as_str()));
        }
        document.insert("judge", toml_edit::Item::Table(table));
    }
    let text = document.to_string();
    Settings::parse(&text).map_err(|error| Failure::new("invalid-config", error))?;
    std::fs::create_dir_all(&paths.config).map_err(unavailable)?;
    write_in_place(&file, &text).map_err(unavailable)?;
    Ok(true)
}

/// Writes `text` in place of `file`, through a temporary file beside it so that a reader sees
/// the old file or the new one, never half of one. A symbolic link stays one (its target is
/// written, the temporary file beside the target, made when the link dangles), the file's mode
/// is kept, and no temporary file stays when a step fails.
pub fn write_in_place(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let real = written_path(file)?;
    let mode = std::fs::metadata(&real).ok().map(|m| m.permissions());
    let name = real.file_name().unwrap_or_default().to_string_lossy();
    let writing = real.with_file_name(format!(".{name}.tmp"));
    let written = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode.as_ref().map_or(0o644, |m| m.mode() & 0o777))
            .open(&writing)?;
        out.write_all(text.as_bytes())?;
        // The mode asked at creation is the umask's to narrow: the file's own is set again.
        if let Some(mode) = mode {
            out.set_permissions(mode)?;
        }
        // On the disk before it takes the file's name: a crash never leaves the file empty.
        out.sync_all()?;
        drop(out);
        std::fs::rename(&writing, &real)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&writing);
    }
    written
}

/// Where `file` is written: what it resolves to, or, when it is a symbolic link whose target does
/// not exist yet, that target (followed link by link, a relative one from its link's folder). A
/// folder is never made for it: a target without one is refused.
fn written_path(file: &Path) -> std::io::Result<PathBuf> {
    if let Ok(real) = std::fs::canonicalize(file) {
        return Ok(real);
    }
    let mut path = file.to_owned();
    let mut followed = false;
    // The system's own bound on a chain of links.
    for _ in 0..40 {
        let Ok(target) = std::fs::read_link(&path) else {
            let folder = path
                .parent()
                .filter(|folder| !folder.as_os_str().is_empty());
            return match folder {
                Some(folder) if followed && !folder.is_dir() => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "the link points to {}, whose folder does not exist",
                        path.display()
                    ),
                )),
                _ => Ok(path),
            };
        };
        path = path.parent().unwrap_or(Path::new("")).join(target);
        followed = true;
    }
    Err(std::io::Error::other("too many levels of symbolic links"))
}

/// The answer to a switch. When the file was written but its event could not be sent, the
/// change happened all the same: the answer says so rather than refusing.
fn switched(name: &str, enabled: bool, announced: Result<(), String>) -> Value {
    let mut answer = json!({"name": name, "enabled": enabled});
    if let Err(error) = announced {
        answer["warning"] = json!(format!(
            "providers.toml was written, but its event could not be sent: {error}"
        ));
    }
    answer
}

/// Every provider as `doctor` reports it, with the models it offers (`null` until a session of
/// its agent said them; those another version said, outdated, until a session says them again),
/// and the project's default provider and model when one is named.
fn list(daemon: &Daemon, project: Option<&Project>) -> Result<Value, Failure> {
    let settings =
        Settings::load(&daemon.paths).map_err(|error| Failure::new("invalid-config", error))?;
    let providers: Vec<Value> = checks(&settings, Version::Ask)
        .iter()
        .map(|check| {
            let mut entry = check.to_json();
            entry["warnings"] = json!(check.warnings());
            let last = last_models(daemon, check);
            entry["models_outdated"] = json!(last.as_ref().is_some_and(|(_, current)| !current));
            entry["models"] = json!(last.map(|(models, _)| models));
            entry
        })
        .collect();
    let mut answer = json!({"providers": providers, "judge": settings.judge});
    if let Some(project) = project {
        let provider = project_provider(project, &settings);
        answer["project"] = json!({
            "name": project.name,
            "provider": provider.map(|p| p.name),
            "model": project.model,
        });
    }
    Ok(answer)
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
    fn the_providers_are_in_the_order_of_their_names() {
        let names: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
        assert_eq!(names, provider::NAMES);
    }

    #[test]
    fn a_root_agent_gets_ask_user_the_agent_tools_and_approval_answer() {
        let tools = short(claude::PROVIDER.tools(true, false, false, false));
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
                "commands_status",
                "skill_read",
                "workspace_remove",
                "agent_launch",
                "agent_send",
                "agent_status",
                "agent_stop",
                "skills",
                "commands",
                "project",
                "report",
                "ask_user",
                "approval_answer",
            ]
        );
    }

    #[test]
    fn the_tools_follow_what_the_agent_can_do() {
        let leaf = short(claude::PROVIDER.tools(false, true, false, false));
        assert_eq!(leaf.len(), 12);
        assert!(leaf.contains(&"ask_parent".to_owned()));
        assert!(
            !leaf
                .iter()
                .any(|t| t.starts_with("agent_") || t == "ask_user")
        );
        assert!(!leaf.contains(&"approval_answer".to_owned()));
        let lead_child = short(claude::PROVIDER.tools(false, false, false, false));
        assert!(lead_child.contains(&"ask_parent".to_owned()));
        assert!(lead_child.contains(&"approval_answer".to_owned()));
        assert!(!lead_child.contains(&"ask_user".to_owned()));
        // A child does its brief: the project's skills, commands and settings are the root's.
        for child in [&leaf, &lead_child] {
            for family in ["skills", "commands", "project"] {
                assert!(!child.contains(&family.to_owned()), "{family}");
            }
        }
    }

    #[test]
    fn a_root_agent_in_main_with_a_forge_lists_the_forge_tools() {
        // The same tools in `main` and in a workspace: the place does not decide, the project's
        // declarations do, and what is allowed stays the gate's business.
        let bare = short(claude::PROVIDER.tools(true, false, false, false));
        for tool in ["forge_read", "ticket_read", "forge_write"] {
            assert!(!bare.contains(&tool.to_owned()), "{tool}");
        }
        let forge = short(claude::PROVIDER.tools(true, false, true, false));
        assert_eq!(forge[8], "forge_read", "after the command tools");
        assert_eq!(forge[9], "skill_read", "after forge_read");
        assert_eq!(forge[10], "forge_write", "after skill_read");
        assert!(!forge.contains(&"ticket_read".to_owned()));
        let both = short(claude::PROVIDER.tools(true, false, true, true));
        assert_eq!(both.len(), 23);
        assert_eq!(both[10], "ticket_read", "after skill_read");
        assert_eq!(both[11], "forge_write", "after ticket_read");
        assert_eq!(both[12], "workspace_remove", "after forge_write");
        assert_eq!(
            short(claude::PROVIDER.tools(false, true, true, true)).len(),
            15
        );
    }

    #[test]
    fn ticket_read_is_listed_only_when_a_tracker_is_declared() {
        // Without a tracker it could only refuse (`docs/reference/mcp.md` §4 rule 9).
        let tracker = short(claude::PROVIDER.tools(true, false, false, true));
        assert_eq!(tracker[9], "ticket_read", "after skill_read");
        assert!(!tracker.contains(&"forge_read".to_owned()));
        assert!(!tracker.contains(&"forge_write".to_owned()));
        let forge = short(claude::PROVIDER.tools(true, false, true, false));
        assert!(!forge.contains(&"ticket_read".to_owned()));
    }

    #[test]
    fn workspace_remove_is_listed_in_any_project() {
        // It acts in `main` too, naming the workspace: the close command closes one in any project.
        for (forge, tracker) in [(false, false), (true, false), (false, true), (true, true)] {
            let listed = short(claude::PROVIDER.tools(true, false, forge, tracker));
            assert!(listed.contains(&"workspace_remove".to_owned()));
        }
    }

    #[test]
    fn each_provider_names_flottille_s_tools_its_own_way() {
        assert_eq!(
            claude::PROVIDER.tools(true, false, false, false)[0],
            "mcp__flottille__fs_read"
        );
        assert_eq!(
            opencode::PROVIDER.tools(true, false, false, false)[0],
            "flottille_fs_read"
        );
        assert_eq!(
            opencode::PROVIDER.tool_of("flottille_report"),
            Some("report")
        );
        assert_eq!(opencode::PROVIDER.tool_of("bash"), None);
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
    fn a_failed_write_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        // A folder in the file's place: the temporary file is written, and the rename fails.
        let file = dir.path().join("providers.toml");
        std::fs::create_dir(&file).unwrap();
        std::fs::write(file.join("kept"), "").unwrap();

        assert!(write_in_place(&file, "[claude]\nenabled = true\n").is_err());

        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["providers.toml"]);
    }

    #[test]
    fn a_chain_of_dangling_links_is_written_at_its_last_target_and_the_links_stay() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("dotfiles")).unwrap();
        let (first, second) = (dir.path().join("providers.toml"), dir.path().join("second"));
        std::os::unix::fs::symlink("second", &first).unwrap();
        std::os::unix::fs::symlink("dotfiles/providers.toml", &second).unwrap();

        write_in_place(&first, "[claude]\nenabled = true\n").unwrap();

        assert_eq!(std::fs::read_link(&first).unwrap(), Path::new("second"));
        assert_eq!(
            std::fs::read_link(&second).unwrap(),
            Path::new("dotfiles/providers.toml")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dotfiles/providers.toml")).unwrap(),
            "[claude]\nenabled = true\n"
        );
    }

    #[test]
    fn a_loop_of_links_is_refused_with_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("providers.toml"), dir.path().join("second"));
        std::os::unix::fs::symlink("second", &first).unwrap();
        std::os::unix::fs::symlink("providers.toml", &second).unwrap();

        let error = write_in_place(&first, "").unwrap_err();

        assert!(error.to_string().contains("too many levels"), "{error}");
    }

    #[test]
    fn a_write_whose_event_could_not_be_sent_answers_that_the_change_happened() {
        assert_eq!(
            switched("opencode", false, Ok(())),
            json!({"name": "opencode", "enabled": false})
        );
        assert_eq!(
            switched("opencode", true, Err("disk I/O error".into())),
            json!({"name": "opencode", "enabled": true, "warning":
                "providers.toml was written, but its event could not be sent: disk I/O error"})
        );
    }

    #[test]
    fn grouped_option_values_are_flattened() {
        let option =
            json!({"options": [{"value": "a"}, {"group": "g", "options": [{"value": "b"}]}]});
        assert_eq!(values(&option), ["a", "b"]);
    }
}
