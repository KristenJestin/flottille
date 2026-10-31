//! The gate every tool call goes through, once, before it is performed (`docs/design.md` §7,
//! `docs/tools.md`, "The gate"). It does not grant rights: it puts each call in one of three
//! classes. **Harmless** calls pass; **to judge** calls make a card in Ask and go to the judge in
//! Auto (`judge.rs`); **base** calls always make a card, with a strong warning. What it refuses
//! is not a permission but a call that cannot be made as it is: shell syntax, a malformed call,
//! a login printed, the daemon driven from a command.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flottille_core::approval::Kind;
use flottille_core::project::Task;
use serde_json::Value;

use crate::mcp::Grant;
use crate::place::{self, Resolved, Unfollowed};

/// A call, as the gate sees it.
pub struct Call<'a> {
    /// The agent that makes it, its place and its tools.
    pub grant: &'a Grant,
    pub tool: &'a str,
    pub args: &'a Value,
    /// The words of the line a run would start, split without a shell; empty for any other call,
    /// and for a line that cannot be split (refused here with its sentence).
    pub words: &'a [String],
    /// The paths the call reaches, resolved: for a run, the folder it runs in first. Empty for a
    /// call that names none, or names one that cannot be read (the tool refuses it).
    pub paths: &'a [Resolved],
    /// The task a run names, with its name. For a write of a task or a setup step, the task it
    /// leaves to run (as the write leaves it), and `words`, `paths` (each folder it would start
    /// in) and `pushed` are those of that run.
    pub command: Option<(&'a str, &'a Task)>,
    /// The project's tasks: a line one of them holds keeps its rules under any other name.
    pub tasks: &'a BTreeMap<String, Task>,
    /// The project's never list; `None` when the project's configuration cannot be read.
    pub never: Option<&'a [Vec<String>]>,
    /// Flottille's own folders (its data, configuration, state): sensitive places.
    pub own: &'a [PathBuf],
    /// The workspace's branch is on a remote: rewriting its history rewrites what others may
    /// have. Always false in the project folder, where every rewrite is base anyway.
    pub pushed: bool,
    /// Where a run looks for a program named bare: its `PATH`. `None` for any other call.
    pub search: Option<&'a std::ffi::OsStr>,
    /// The folders the agent can write: the project folder, its workspaces, the place. A program
    /// found there is the agent's own file, whatever its name.
    pub writable: &'a [PathBuf],
    /// The process groups of the agent's runs: reading their `/proc` is reading its own.
    pub groups: &'a [u32],
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Harmless: it passes in both modes.
    Allow,
    /// Refused, with the sentence that says why and what to do.
    Deny(String),
    /// To judge or base: a card, unless the judge allows a call to judge in Auto.
    Ask(Ask),
}

/// The class of a call that does not simply pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// What the rules do not class elsewhere: the user's card in Ask, the judge's in Auto.
    Judge,
    /// What always comes to the user, with a strong warning, whatever the mode.
    Base,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    /// Why, one sentence each.
    pub reasons: Vec<String>,
    pub class: Class,
    /// What the card shows: a call, or an action said in words.
    pub kind: Kind,
}

impl Verdict {
    /// How a recorded call names it.
    pub fn word(&self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Deny(_) => "deny",
            Verdict::Ask(_) => "ask",
        }
    }
}

/// Programs that are shells: a task running one is judged, and the line one runs
/// with `-c` is judged in its place.
const SHELLS: [&str; 11] = [
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "mksh",
    "ash",
    "fish",
    "cmd",
    "powershell",
    "pwsh",
];
/// Wrappers unwrapped at most, one inside the other.
const LAYERS_MAX: usize = 16;
const WRITES: [&str; 2] = ["fs_write", "fs_edit"];
const FILES: [&str; 5] = ["fs_read", "fs_list", "search", "fs_write", "fs_edit"];
/// Programs that only read, whatever their arguments but those `writes_anyway` names: a line of
/// one of them, alone, inside the place, is harmless.
const READS: [&str; 38] = [
    "ls", "pwd", "cat", "head", "tail", "wc", "grep", "rg", "egrep", "fgrep", "find", "fd", "stat",
    "file", "tree", "du", "df", "ps", "pgrep", "pidof", "which", "whoami", "uname", "date", "echo",
    "diff", "cmp", "basename", "dirname", "realpath", "readlink", "id", "hostname", "uptime",
    "free", "true", "test", "lsof",
];
/// Git's subcommands that only read.
const GIT_READS: [&str; 17] = [
    "status",
    "log",
    "diff",
    "show",
    "blame",
    "shortlog",
    "describe",
    "rev-parse",
    "rev-list",
    "ls-files",
    "ls-tree",
    "cat-file",
    "grep",
    "whatchanged",
    "merge-base",
    "name-rev",
    "help",
];
/// Sensitive wherever they are along a path (`docs/reference/hemera.md` §4), with the agents'
/// own logins.
const SENSITIVE: [&str; 18] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".docker/config.json",
    ".config/gh",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".local/share/keyrings",
    "Library/Keychains",
    ".claude",
    ".claude.json",
    ".codex",
    ".local/share/opencode",
];
/// Sensitive from the top of the file system only.
const SYSTEM_SENSITIVE: [&str; 3] = ["/etc/shadow", "/etc/sudoers", "/etc/sudoers.d"];
/// What runs at the user's next login or command, or starts a process later, from the home
/// folder (`docs/design.md` §7.1): an agent never writes it.
const STARTUP: [&str; 20] = [
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".profile",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".zlogin",
    ".config/fish",
    ".gitconfig",
    ".config/git",
    ".config/autostart",
    ".config/systemd/user",
    ".config/environment.d",
    ".config/hypr",
    ".config/sway",
    ".config/niri",
    ".local/share/systemd/user",
    ".local/bin",
    "bin",
];
/// What is read from the user's configuration folder, wherever it is: [`STARTUP`]'s.
const CONFIG_STARTUP: [&str; 8] = [
    "fish",
    "git",
    "autostart",
    "systemd/user",
    "environment.d",
    "hypr",
    "sway",
    "niri",
];
/// Programs that start a process outside the daemon's tree of processes, or later, or on another
/// host: what it runs escapes the gate and the daemon's ancestry check, as the user.
const LAUNCHERS: [&str; 20] = [
    "systemd-run",
    "tmux",
    "screen",
    "zellij",
    "dtach",
    "abduco",
    "setsid",
    "at",
    "batch",
    "crontab",
    "dbus-send",
    "gdbus",
    "busctl",
    "ssh",
    "scp",
    "sftp",
    "loginctl",
    "hyprctl",
    "swaymsg",
    "i3-msg",
];
/// Programs whose words are text, never a file they write (`tr / _`).
const TEXT: [&str; 4] = ["tr", "printf", "expr", "seq"];
/// Password managers and keyrings: whatever they do, they read or write the user's logins.
const KEYRINGS: [&str; 6] = ["pass", "gopass", "secret-tool", "op", "bw", "rbw"];
/// What makes a `.env.*` file a template, free to read and write.
const TEMPLATES: [&str; 4] = [".example", ".sample", ".template", ".dist"];

/// What the rules found in a call: why it is base, why it is to judge.
#[derive(Default)]
struct Found {
    base: Vec<String>,
    judge: Vec<String>,
}

impl Found {
    fn base(&mut self, reason: String) {
        if !self.base.contains(&reason) {
            self.base.push(reason);
        }
    }

    fn judge(&mut self, reason: String) {
        if !self.judge.contains(&reason) {
            self.judge.push(reason);
        }
    }

    /// The verdict: base wins over to judge, and nothing found is harmless.
    fn verdict(self, kind: Kind) -> Verdict {
        if !self.base.is_empty() {
            let mut reasons = self.base;
            reasons.extend(self.judge);
            return Verdict::Ask(Ask {
                reasons,
                class: Class::Base,
                kind,
            });
        }
        if !self.judge.is_empty() {
            return Verdict::Ask(Ask {
                reasons: self.judge,
                class: Class::Judge,
                kind,
            });
        }
        Verdict::Allow
    }
}

/// The verdict on a call: refused when it cannot be made as it is, else its class.
pub fn decide(call: &Call) -> Verdict {
    if let Some(family) = crate::actions::family(call.tool, call.args) {
        let mut found = Found::default();
        // Nothing checks a tracker against the project, as a forge is against its remote.
        if family == "settings" && call.args.get("tracker").is_some() {
            found.base(
                "it changes the project's tracker, which nothing checks against the project".into(),
            );
        } else if family == "rules" {
            found.base(
                "it replaces the project's rules, the instructions of every agent created after it"
                    .into(),
            );
        } else {
            found.judge(crate::actions::asked(family));
        }
        if let Some(line) = crate::actions::line_of(call.tool, call.args)
            && let Err(why) = flottille_core::command::split_line(line)
        {
            return Verdict::Deny(why);
        }
        // A line a task or a setup step holds runs later without this gate (a task in a
        // workspace is harmless, a workspace's setup runs on its own): setting it is judged as
        // running it, in each folder it would start in, base when that would be.
        if !call.words.is_empty() {
            let args = match call.command {
                Some((name, _)) => serde_json::json!({"task": name}),
                None => serde_json::json!({"line": shown_words(call.words)}),
            };
            let folders: Vec<&[Resolved]> = match call.paths {
                [] => vec![&[]],
                paths => paths.iter().map(std::slice::from_ref).collect(),
            };
            for paths in folders {
                match run(&Call {
                    tool: "run",
                    args: &args,
                    paths,
                    ..*call
                }) {
                    Verdict::Deny(why) => return Verdict::Deny(why),
                    Verdict::Ask(ask) => {
                        for reason in ask.reasons {
                            let reason =
                                format!("the line it sets, {}: {reason}", shown_words(call.words));
                            match ask.class {
                                Class::Base => found.base(reason),
                                Class::Judge => found.judge(reason),
                            }
                        }
                    }
                    Verdict::Allow => {}
                }
            }
        }
        for why in later(call) {
            found.base(why);
        }
        return found.verdict(Kind::Action);
    }
    match call.tool {
        "forge" => forge(call),
        "tickets" => tickets(call),
        "agent_launch" if call.args["root"] == true => root_launch(call.grant, call.args),
        "session_send" => session_send(call.grant, call.args),
        // Its own session is no other's conversation: the answer says the tree's mode.
        "session_read" if call.args["session"] == call.grant.agent.as_str() => Verdict::Allow,
        "session_read" => {
            let mut found = Found::default();
            found.judge(SESSION_READ.into());
            found.verdict(Kind::Action)
        }
        "agent_launch" => match too_long("brief", &call.args["brief"]) {
            Some(refused) => refused,
            None => Verdict::Allow,
        },
        "workspace" if call.args["action"] == "remove" => {
            let which = match call.args["name"].as_str() {
                Some(name) => format!("the workspace {name}"),
                None => "a workspace".to_owned(),
            };
            // What a removal loses is the user's to weigh, never a judge's, in every mode.
            Found {
                base: vec![format!(
                    "it removes {which} and deletes its branches that hold nothing of their own: \
                     only the user decides a workspace's removal"
                )],
                judge: Vec::new(),
            }
            .verdict(Kind::Action)
        }
        "run" => run(call),
        tool if FILES.contains(&tool) => files(call),
        // Reads, the tree's tools, the outputs of the agent's own runs.
        _ => Verdict::Allow,
    }
}

/// A file tool: reading inside the place, and writing in a workspace, are harmless.
fn files(call: &Call) -> Verdict {
    if let Some(why) = malformed(call.paths) {
        return Verdict::Deny(why);
    }
    let write = WRITES.contains(&call.tool);
    let home = call.grant.place.home.as_deref();
    if let Some(why) = call
        .paths
        .iter()
        .find_map(|path| forbidden(call.own, home, path, write))
    {
        return Verdict::Deny(why);
    }
    let mut found = Found::default();
    for path in call.paths {
        if write && in_git(path) {
            found.base(format!(
                "{} is in a .git folder, which git keeps: writing there can rewrite the \
                 repository or run code at its next command",
                path.shown
            ));
        }
        if let Some(why) = process(call.groups, path) {
            found.base(why);
        } else if let Some(place) = sensitive(call.own, path) {
            found.base(format!("{} is a sensitive place ({place})", path.shown));
        } else if !path.inside {
            found.judge(format!("{} is outside this agent's place", path.shown));
        }
    }
    if write && call.grant.place.workspace.is_none() {
        found.judge(
            "it writes in the project folder (main), the user's own checkout, not in a workspace"
                .into(),
        );
    }
    found.verdict(Kind::Call)
}

/// Why the gate holds `agent_launch` with `root`.
pub const ROOT_LAUNCH: &str = "it starts a new session for the user, a root agent with the \
    project's tools, outside this agent's tree";

/// Why a session started by a root another agent started comes to the user: a chain of them
/// needs the user at each step.
pub const LAUNCHED_LAUNCH: &str = "this agent is a session another agent started for the user, \
    and a session it starts comes to the user, since it was itself started by an agent";

/// `agent_launch` with `root`: a session for the user acts with a root's tools, outside the
/// caller's tree. Its first message is shown whole to the user and to the judge.
pub fn root_launch(grant: &Grant, args: &Value) -> Verdict {
    let mut found = Found::default();
    found.judge(ROOT_LAUNCH.into());
    if grant.launched {
        found.base(LAUNCHED_LAUNCH.into());
    }
    if let Err(refused) = texts(grant, args, &["brief"], &mut found) {
        return refused;
    }
    found.verdict(Kind::Action)
}

/// Why the gate holds `session_send`.
pub const SESSION_SEND: &str = "it writes to another session, the user's, as this agent: what it \
    sends steers that session's work";

/// Why the gate holds `session_read`: each read of another session is judged.
pub const SESSION_READ: &str = "it reads another session's conversation: what the user said to \
    that session's agent, and what that agent answered, which this agent was not told";

/// `session_send`: a message into a session of the user's, which acts on it with a root's tools.
/// Its text is shown whole to the user and to the judge.
fn session_send(grant: &Grant, args: &Value) -> Verdict {
    let mut found = Found::default();
    found.judge(SESSION_SEND.into());
    if let Err(refused) = texts(grant, args, &["text"], &mut found) {
        return refused;
    }
    found.verdict(Kind::Action)
}

/// The texts a write posts or sends, which the user's card and the judge show whole: one longer
/// than the judge is given whole is refused, and one that holds what the masking takes for a
/// secret is base, since what is shown is masked and what is sent is not.
fn texts(grant: &Grant, args: &Value, keys: &[&str], found: &mut Found) -> Result<(), Verdict> {
    for key in keys {
        if let Some(refused) = too_long(key, &args[*key]) {
            return Err(refused);
        }
        let Some(text) = args[*key].as_str() else {
            continue;
        };
        if crate::mcp::masked(text, &grant.place) != text {
            found.base(format!(
                "its {key} holds what looks like a secret: the card shows it masked, and it is \
                 sent as it is written"
            ));
        }
    }
    Ok(())
}

/// The refusal of a text longer than the judge is given whole. A brief is handed to the judge in
/// the agent block, so any launch's, child or root, is held to it.
pub fn too_long(key: &str, text: &Value) -> Option<Verdict> {
    let count = text.as_str()?.chars().count();
    (count > crate::judge::WHOLE_CHARS).then(|| {
        Verdict::Deny(format!(
            "refused: the {key} is longer than the {} characters the user and the judge are \
             shown whole ({count}): shorten it",
            crate::judge::WHOLE_CHARS
        ))
    })
}

/// A text given to a write that takes none (a title on a comment, a body on a close, a title or a
/// body on a merge): refused before anything is asked, since the card would show what is never
/// posted.
fn untexted(tool: &str, action: &str, args: &Value, keys: &[&str]) -> Result<(), Verdict> {
    for key in keys {
        if args.get(*key).is_some_and(|text| !text.is_null()) {
            let takers = match *key {
                "title" => "edit and create",
                _ => "create, comment and edit",
            };
            return Err(Verdict::Deny(format!(
                "refused: {tool} {action} takes no {key}: {key} is for {takers}"
            )));
        }
    }
    Ok(())
}

/// `forge`: its reads are harmless, and so is waiting for what happens to a pull request; a malformed call is refused; merging what the checks or a
/// review block is base, since it is the user's own decision; the other writes are to judge.
fn forge(call: &Call) -> Verdict {
    let action = call.args["action"].as_str().unwrap_or_default();
    let method = call.args.get("method").filter(|m| !m.is_null());
    let methods = crate::forge::MERGE_METHODS;
    let by = match method.map(Value::as_str) {
        None => methods[0],
        Some(_) if action != "merge" => {
            return Verdict::Deny(format!(
                "refused: forge {action} takes no method: method is for merge"
            ));
        }
        Some(Some(method)) if methods.contains(&method) => method,
        Some(_) => {
            return Verdict::Deny(format!(
                "refused: forge merge has no method {}: its methods are squash (the default), \
                 merge and rebase",
                method.unwrap()
            ));
        }
    };
    let merged = format!("it merges a pull request on the forge, by {by}");
    let said = match action {
        "list" | "show" | "subscribe" | "unsubscribe" => return Verdict::Allow,
        "create" => "it opens a pull request on the forge",
        "comment" => "it comments on a pull request on the forge",
        "edit" => "it changes the title or the description of a pull request on the forge",
        "merge" => merged.as_str(),
        "checks" => "it re-runs the failed jobs of a pipeline on the forge",
        _ => {
            return Verdict::Deny(format!(
                "refused: forge has no action {action:?}: its actions are {FORGE_ACTIONS}"
            ));
        }
    };
    let none: &[&str] = match action {
        "comment" => &["title"],
        "merge" | "checks" => &["title", "body"],
        _ => &[],
    };
    if let Err(refused) = untexted("forge", action, call.args, none) {
        return refused;
    }
    let mut found = Found::default();
    found.judge(said.into());
    if let Err(refused) = texts(call.grant, call.args, &["title", "body"], &mut found) {
        return refused;
    }
    if let Some(waived) = call.args.get("waive").filter(|w| !w.is_null()) {
        if action != "merge" {
            return Verdict::Deny(
                "refused: waive is for merge only: it merges a pull request whose checks or \
                 review block it, once the user allows it"
                    .into(),
            );
        }
        let Some(why) = waived.as_str().map(str::trim).filter(|w| !w.is_empty()) else {
            return Verdict::Deny(
                "refused: waive says why the user lets the pull request be merged as it is: give \
                 their words"
                    .into(),
            );
        };
        found.base(format!(
            "it merges a pull request whose checks or review block it, waived: {why}"
        ));
    }
    found.verdict(Kind::Call)
}

/// The actions of `forge`, as a refusal names them.
pub const FORGE_ACTIONS: &str =
    "list, show, subscribe, unsubscribe, create, comment, edit, merge and checks";
/// The actions of `tickets`, as a refusal names them.
pub const TICKET_ACTIONS: &str =
    "list, search, show, create, comment, edit, close, reopen and status";

/// `tickets`: its reads are harmless, its writes to the tracker to judge.
fn tickets(call: &Call) -> Verdict {
    let action = call.args["action"].as_str().unwrap_or_default();
    let said = match action {
        "list" | "search" | "show" => return Verdict::Allow,
        "create" => "it opens a ticket on the tracker",
        "comment" => "it comments on a ticket on the tracker",
        "edit" => "it changes the title or the description of a ticket on the tracker",
        "close" => "it closes a ticket on the tracker",
        "reopen" => "it reopens a ticket on the tracker",
        "status" => "it moves a ticket to another status on the tracker",
        _ => {
            return Verdict::Deny(format!(
                "refused: tickets has no action {action:?}: its actions are {TICKET_ACTIONS}"
            ));
        }
    };
    let none: &[&str] = match action {
        "comment" => &["title"],
        "close" | "reopen" | "status" => &["title", "body"],
        _ => &[],
    };
    if let Err(refused) = untexted("tickets", action, call.args, none) {
        return refused;
    }
    let mut found = Found::default();
    found.judge(said.into());
    if let Err(refused) = texts(call.grant, call.args, &["title", "body"], &mut found) {
        return refused;
    }
    found.verdict(Kind::Call)
}

/// The tasks whose line is `words`.
fn holding<'a>(
    tasks: &'a BTreeMap<String, Task>,
    words: &'a [String],
) -> impl Iterator<Item = (&'a str, &'a Task)> {
    tasks
        .iter()
        .filter(move |(_, task)| {
            flottille_core::command::split_line(&task.line).is_ok_and(|line| line == words)
        })
        .map(|(name, task)| (name.as_str(), task))
}

/// Why a write of a task or a setup step is base for what it leaves to happen later without
/// this gate, beside the line it sets: what only the user's word may say of a task, and a step
/// that reaches the user's checkout or copies a secret into every workspace.
fn later(call: &Call) -> Vec<String> {
    let args = call.args;
    let mut why = Vec::new();
    match (call.tool, args["action"].as_str()) {
        ("tasks", Some(action @ ("add" | "edit"))) => {
            if action == "edit" && args["ask_before_running"] == false {
                why.push(
                    "it turns off asking before the task runs: only the user lets a task run \
                     unasked"
                        .into(),
                );
            }
            for (flag, said) in [("check", "is a check"), ("read_only", "changes nothing")] {
                if args[flag] == true {
                    why.push(format!(
                        "it says the task {said}, which the window shows as the user's word"
                    ));
                }
            }
            if let Some((name, task)) = call.command
                && !task.ask_before_running
                && let Some((other, _)) = holding(call.tasks, call.words)
                    .find(|(other, task)| *other != name && task.ask_before_running)
            {
                why.push(format!(
                    "its line is that of the task {other}, marked to ask before it runs, which a \
                     copy under another name would not"
                ));
            }
        }
        ("setup", Some("add" | "edit")) => {
            let step = &args["step"];
            if step.get("link").is_some() || step.get("root_entries").is_some() {
                why.push(
                    "a link step makes what is written in each workspace reach the user's \
                     checkout"
                        .into(),
                );
            }
            let globs = step["copy"].as_array().into_iter().flatten();
            for glob in globs.filter_map(Value::as_str) {
                if let Some(name) = sensitive_glob(glob) {
                    why.push(format!(
                        "it copies {glob} into every workspace, which matches a sensitive name \
                         ({name}): only the user chooses to spread it"
                    ));
                }
            }
        }
        _ => {}
    }
    why
}

/// Names a `copy` glob is tried against for keys, beside the sensitive places.
const KEY_NAMES: [&str; 6] = [
    "server.pem",
    "server.key",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
];

/// The sensitive name a setup's `copy` glob may match (`glob.rs` matches it part by part): a
/// `.env` but a template, a key, a sensitive place or a folder holding one.
fn sensitive_glob(glob: &str) -> Option<String> {
    use flottille_core::glob::matches;
    let parts: Vec<&str> = glob.split('/').filter(|part| !part.is_empty()).collect();
    for (at, part) in parts.iter().enumerate() {
        let template = TEMPLATES.iter().any(|t| part.ends_with(t));
        if matches(part, ".env")
            || matches(part, ".env.local")
            || (part.starts_with(".env.") && !template)
        {
            return Some(".env".into());
        }
        if let Some(key) = KEY_NAMES.iter().find(|key| matches(part, key)) {
            return Some((*key).into());
        }
        for place in SENSITIVE {
            let names: Vec<&str> = place.split('/').collect();
            if names.iter().zip(&parts[at..]).all(|(n, p)| matches(p, n)) {
                return Some(place.into());
            }
        }
    }
    None
}

/// `run`: a task in a workspace, and a known read-only line inside the
/// place, are harmless.
fn run(call: &Call) -> Verdict {
    if call.words.is_empty() {
        // A line that cannot be split is shell syntax; a task the project does not hold is
        // refused by the tool itself.
        if call.command.is_none()
            && let Some(line) = call.args["line"].as_str()
            && let Err(why) = flottille_core::command::split_line(line)
        {
            return Verdict::Deny(why);
        }
        return Verdict::Allow;
    }
    let layers = match lines(call.words) {
        Ok(layers) => layers,
        Err(why) => return Verdict::Deny(why),
    };
    if let Some(why) = layers.iter().find_map(|layer| refusal(layer)) {
        return Verdict::Deny(why);
    }
    // What a runner runs (`uv run`, `cargo run --`) is a program too: a launcher there is
    // refused. Anywhere else a launcher's name is only a word (`cargo test tmux`).
    if let Some(why) = layers
        .iter()
        .filter_map(|layer| runs(layer))
        .find_map(|program| launches(base(program)))
    {
        return Verdict::Deny(why);
    }
    // A container's bind mount gives it the folder of the host it names, to read and to write.
    if let Some(folder) = call.paths.first() {
        let home = call.grant.place.home.as_deref();
        if let Some(why) = layers
            .iter()
            .flat_map(|layer| mounts(layer))
            .map(|source| call.grant.place.resolve(&folder.path, source))
            .find_map(|path| forbidden(call.own, home, &path, true))
        {
            return Verdict::Deny(why);
        }
    }
    let main = call.grant.place.workspace.is_none();
    let mut found = Found::default();
    match call.never {
        Some(never) => {
            // A never-listed task's line is never allowed under another name either.
            let names: Vec<&str> = call
                .command
                .map(|(name, _)| name)
                .into_iter()
                .chain(holding(call.tasks, call.words).map(|(name, _)| name))
                .collect();
            if let Some(entry) = never.iter().find(|entry| {
                (entry.len() == 1 && names.contains(&entry[0].as_str()))
                    || layers.iter().any(|layer| starts_anywhere(layer, entry))
            }) {
                found.base(format!(
                    "the project's never list holds {}",
                    entry.join(" ")
                ));
            }
        }
        None => found.base(
            "the project's configuration cannot be read, so what it never allows is not known"
                .into(),
        ),
    }
    let git_word = layers
        .iter()
        .flat_map(|layer| &layer[1..])
        .flat_map(|word| named_paths(word))
        .find(|named| {
            Path::new(named)
                .components()
                .any(|p| p.as_os_str() == ".git")
        });
    if let Some(word) = git_word {
        found.base(format!("{word} reaches a .git folder, which git keeps"));
    }
    let named = named(call, &layers);
    let mut reached = call.paths.to_vec();
    reached.extend(
        named
            .iter()
            .map(|(_, path, _)| path)
            .filter(|path| !call.paths.contains(path))
            .cloned(),
    );
    for path in &reached {
        // A word that goes up out of a folder that is not there is often no path at all (a
        // `sed` expression, a message, a URL), and the judge reads it as written. A program may
        // make that folder first (`install -D`): it is checked where it then leads. One the
        // kernel could never make, a program that takes `..` on the text still reaches
        // (`git restore`, `python3 -m zipfile`): it is checked as written.
        let unmade = path.unfollowed == Some(Unfollowed::UpFromMissing);
        let written;
        let path = if unmade {
            written = as_written(path);
            &written
        } else {
            path
        };
        if unmade || path.up_from_missing {
            let word = named
                .iter()
                .find(|(_, named, _)| named == path)
                .map_or(path.shown.as_str(), |(word, _, _)| word);
            found.judge(format!(
                "{word} {}{}",
                Unfollowed::UpFromMissing,
                if unmade {
                    ": the gate cannot tell where it leads"
                } else {
                    ""
                }
            ));
        }
        if let Some(why) = credential(path) {
            return Verdict::Deny(why);
        }
        if in_git(path) {
            found.base(format!(
                "{} is in a .git folder, which git keeps",
                path.shown
            ));
        }
        if let Some(why) = process(call.groups, path) {
            found.base(why);
        } else if let Some(place) = sensitive(call.own, path) {
            found.base(format!("{} is a sensitive place ({place})", path.shown));
        } else if !path.inside && !unmade {
            found.judge(format!("{} is outside this agent's place", path.shown));
        }
    }
    if let Some(why) = deletion(call, &layers) {
        found.base(why);
    }
    if let Some(why) = layers.iter().find_map(|layer| unwalls(layer)) {
        found.base(why);
    }
    let mut reads = layers.len() == 1;
    for layer in &layers {
        for (at, suffix) in suffixes(layer) {
            let line = program(call, suffix);
            // A word inside a line may start a program the gate does not know runs it: only
            // what would be base there counts.
            if at > 0 {
                if let Line::Base(why) = line {
                    found.base(why);
                }
                continue;
            }
            match line {
                Line::Reads => {}
                Line::Files if main => found.judge(format!(
                    "{} writes a file in the project folder (main), the user's own checkout",
                    shown_words(layer)
                )),
                Line::Files => {}
                Line::Writes => reads = false,
                Line::Judge(why) => {
                    reads = false;
                    found.judge(why);
                }
                Line::Base(why) => {
                    reads = false;
                    found.base(why);
                }
            }
        }
    }
    if let Some((name, command)) = call.command {
        if command.ask_before_running {
            found.judge(format!("the task {name} is marked to ask before it runs"));
        }
        if let Some(shell) = call.words.first().filter(|w| SHELLS.contains(&base(w))) {
            found.judge(format!(
                "the task {name} runs a shell ({}), whose line the gate does not read",
                base(shell)
            ));
        }
        if main {
            found.judge(format!(
                "the task {name} runs in the project folder (main), the user's own \
                 checkout, not in a workspace"
            ));
        }
    } else if !reads {
        found.judge(format!(
            "{} is not a command the rules know to only read",
            shown_words(call.words)
        ));
    }
    // The words only: a run's own folder holding Flottille's (a project in the home folder) is
    // where it works, not what it takes away. A line that does not only read may write any path
    // it names.
    if !reads {
        for (_, path, soft) in &named {
            let written;
            let path = if path.unfollowed == Some(Unfollowed::UpFromMissing) {
                written = as_written(path);
                &written
            } else {
                path
            };
            // An option's value or a text's word (`awk -F/`, `tr / _`) is seldom a path the line
            // writes: naming a folder above Flottille's there is held, as any base call.
            if *soft
                && credential(path).is_none()
                && let Some(why) = above_own(call.own, path)
            {
                found.base(why);
                continue;
            }
            if let Some(why) = forbidden(call.own, call.grant.place.home.as_deref(), path, true) {
                return Verdict::Deny(why);
            }
        }
    }
    // What deletes untracked files names none: the folder it runs in, or its whole worktree, is
    // what it takes away.
    if let Some(folder) = call.paths.first() {
        let place = &call.grant.place;
        let removals = layers
            .iter()
            .flat_map(|layer| suffixes(layer))
            .filter_map(|(_, layer)| removes_untracked(layer));
        for removal in removals {
            let mut runs_in = folder.path.clone();
            for chdir in &removal.chdirs {
                runs_in = place.resolve(&runs_in, chdir).path;
            }
            let taken = match (removal.reach, removal.work_tree) {
                (Untracked::Folder, _) => runs_in,
                (Untracked::Worktree, Some(tree)) => place.resolve(&runs_in, tree).path,
                (Untracked::Worktree, None) => top(&runs_in),
            };
            if let Some(why) = above_own(call.own, &place.at(&taken)) {
                return Verdict::Deny(format!("refused: {why}; nothing was done"));
            }
        }
    }
    found.verdict(Kind::Call)
}

/// A layer that deletes untracked files: where, the folders its `-C`s go to from the run's, and
/// the worktree its `--work-tree` names.
struct Removal<'a> {
    reach: Untracked,
    chdirs: Vec<&'a str>,
    work_tree: Option<&'a str>,
}

/// Where a layer deletes untracked files.
#[derive(Debug, PartialEq)]
enum Untracked {
    /// In the folder it runs in: `git clean`.
    Folder,
    /// In the whole worktree: a `git stash` that takes them (`-u`, `--include-untracked`, `-a`,
    /// `--all`), a `git clean` of a pathspec from the top (`:/`, `:(top)`).
    Worktree,
}

/// Whether a layer deletes untracked files, and where.
fn removes_untracked(layer: &[String]) -> Option<Removal<'_>> {
    if base(&layer[0]) != "git" {
        return None;
    }
    let args = &layer[1..];
    let (at, globals) = git_globals(args);
    let rest = args.get(at + 1..).unwrap_or_default();
    let reach = match args.get(at).map(String::as_str) {
        Some("clean") if rest.iter().any(|word| from_top(word)) => Some(Untracked::Worktree),
        Some("clean") => Some(Untracked::Folder),
        Some("stash")
            if rest.iter().take_while(|word| *word != "--").any(|word| {
                abbreviates(word, "include-untracked", 1) || abbreviates(word, "all", 1)
            }) || shorts(rest).any(|short| short.contains(['u', 'a'])) =>
        {
            Some(Untracked::Worktree)
        }
        _ => None,
    }?;
    let valued = |name: &'static str| {
        globals
            .iter()
            .filter(move |(option, _)| *option == name)
            .filter_map(|(_, value)| *value)
    };
    Some(Removal {
        reach,
        chdirs: valued("-C").collect(),
        work_tree: valued("--work-tree").next_back(),
    })
}

/// Whether a pathspec is taken from the top of the worktree: the magic `/` in its short form
/// (`:/`, `:!/x`), `top` in its long one (`:(top,exclude)`).
fn from_top(word: &str) -> bool {
    let Some(magic) = word.strip_prefix(':') else {
        return false;
    };
    match magic.strip_prefix('(') {
        Some(long) => long
            .split(')')
            .next()
            .is_some_and(|list| list.split(',').any(|word| word.trim() == "top")),
        None => magic
            .chars()
            .take_while(|c| "/!^".contains(*c))
            .any(|c| c == '/'),
    }
}

/// The top of the worktree that holds `folder`, as git finds it on the disk: the deepest folder
/// from `/` to it that holds a `.git` (a folder, or a linked worktree's file), each looked at in
/// the folder opened before it; `folder` itself when none does, where git would find no
/// repository.
fn top(folder: &Path) -> PathBuf {
    use rustix::fs::{AtFlags, Mode};
    let Ok(root) = rustix::fs::open("/", crate::nofollow::THROUGH, Mode::empty()) else {
        return folder.to_owned();
    };
    let mut opened = vec![(PathBuf::from("/"), root)];
    for part in folder
        .components()
        .filter(|part| matches!(part, std::path::Component::Normal(_)))
    {
        let (path, at) = opened.last().expect("`/` is opened first");
        let Ok(next) = rustix::fs::openat(
            at,
            part.as_os_str(),
            crate::nofollow::THROUGH,
            Mode::empty(),
        ) else {
            break;
        };
        let path = path.join(part);
        opened.push((path, next));
    }
    opened
        .iter()
        .rev()
        .find(|(_, at)| rustix::fs::statat(at, ".git", AtFlags::SYMLINK_NOFOLLOW).is_ok())
        .map_or_else(|| folder.to_owned(), |(path, _)| path.clone())
}

/// What a layer of a line does, by its program.
enum Line {
    /// It only reads.
    Reads,
    /// It writes the files of the place, and nothing else: harmless in a workspace.
    Files,
    /// It writes, in a way the rules have nothing to say about: harmless only as a task in a
    /// workspace.
    Writes,
    Judge(String),
    Base(String),
}

impl Line {
    fn rank(&self) -> u8 {
        match self {
            Line::Reads => 0,
            Line::Files => 1,
            Line::Writes => 2,
            Line::Judge(_) => 3,
            Line::Base(_) => 4,
        }
    }

    /// The stronger of two, with the reasons of both when they are as strong.
    fn or(self, other: Line) -> Line {
        match (self, other) {
            (Line::Judge(a), Line::Judge(b)) => Line::Judge(format!("{a}; {b}")),
            (Line::Base(a), Line::Base(b)) => Line::Base(format!("{a}; {b}")),
            (a, b) if b.rank() > a.rank() => b,
            (a, _) => a,
        }
    }
}

/// What is refused, whatever the user would say: a login printed, the daemon driven from a
/// command, a process started outside the daemon's tree. These are not permissions; such a call
/// cannot be made at all.
fn refusal(layer: &[String]) -> Option<String> {
    let program = base(&layer[0]);
    let args = &layer[1..];
    if let Some(why) = launches(program) {
        return Some(why);
    }
    if KEYRINGS.contains(&program)
        || program.starts_with("docker-credential-")
        || program.starts_with("git-credential-")
        || prints_login(program, args)
    {
        return Some(format!(
            "refused: {} reads or prints a login, and agents never see one, in any mode",
            shown_words(layer)
        ));
    }
    if let Some(why) = drives_later(program, args) {
        return Some(why);
    }
    match program {
        "flottille" => Some(
            "refused: flottille drives the daemon, and agents act only through their tools: \
             ask the user to run it"
                .into(),
        ),
        "gh" | "glab" if prints_token(args) => Some(format!(
            "refused: {program} auth prints a login token, and agents never see one: auth status \
             says whether a login exists"
        )),
        "git" => git_refusal(args).or_else(|| {
            rebase_execs(args).into_iter().find_map(|line| {
                flottille_core::command::split_line(line).err().map(|why| {
                    format!(
                        "refused: git rebase --exec runs its line through a shell, and Flottille \
                         reads that line: it cannot be split without a shell: {why}"
                    )
                })
            })
        }),
        _ => None,
    }
}

/// The words of a line that are not options, in order: a command line's commands, an option's
/// value among them.
fn plain(args: &[String]) -> Vec<&str> {
    args.iter()
        .map(String::as_str)
        .filter(|word| !word.starts_with('-'))
        .collect()
}

/// Whether a cloud's or a cluster's command line prints a login it holds.
fn prints_login(program: &str, args: &[String]) -> bool {
    let words = plain(args);
    let pair = |a: &str, b: &[&str]| words.windows(2).any(|w| w[0] == a && b.contains(&w[1]));
    match program {
        "aws" => {
            pair("configure", &["get", "export-credentials"])
                || pair("ecr", &["get-login-password", "get-login"])
                || pair("ecr-public", &["get-login-password"])
                || pair("sts", &["get-session-token"])
                || pair("codeartifact", &["get-authorization-token"])
        }
        "kubectl" => {
            (pair("config", &["view"]) && args.iter().any(|w| w == "--raw"))
                || pair("create", &["token"])
        }
        "gcloud" => {
            (words.contains(&"auth")
                && words
                    .iter()
                    .any(|w| w.starts_with("print-") && w.ends_with("token")))
                || pair("config", &["config-helper"])
        }
        "az" => pair("account", &["get-access-token"]),
        _ => false,
    }
}

/// The refusal of what makes the user's own session start a program later or elsewhere: a
/// service manager's unit, a desktop's or a terminal's remote control.
fn drives_later(program: &str, args: &[String]) -> Option<String> {
    let driven = match program {
        "systemctl" => {
            const READS: [&str; 20] = [
                "status",
                "show",
                "cat",
                "help",
                "list-units",
                "list-unit-files",
                "list-sockets",
                "list-timers",
                "list-jobs",
                "list-dependencies",
                "list-automounts",
                "list-paths",
                "list-machines",
                "is-active",
                "is-enabled",
                "is-failed",
                "is-system-running",
                "show-environment",
                "get-default",
                "whoami",
            ];
            const VALUED: [&str; 12] = [
                "-t",
                "--type",
                "-p",
                "--property",
                "-H",
                "--host",
                "-M",
                "--machine",
                "-n",
                "--lines",
                "-o",
                "--output",
            ];
            let verb = args
                .iter()
                .enumerate()
                .find(|(at, word)| {
                    !word.starts_with('-') && (*at == 0 || !VALUED.contains(&args[at - 1].as_str()))
                })
                .map(|(_, word)| word.as_str());
            verb.is_some_and(|verb| !READS.contains(&verb))
        }
        "niri" => args.first().is_some_and(|w| w == "msg"),
        "kitty" | "kitten" => args.first().is_some_and(|w| w.starts_with('@')),
        "wezterm" => args.first().is_some_and(|w| w == "cli"),
        "rsync" => {
            // A host's path (`host:path`, `user@host:path`) goes over ssh, which runs a line as
            // the user there; `-e` and `--rsync-path` name what runs.
            args.iter().any(|w| {
                abbreviates(w, "rsh", 3) || abbreviates(w, "rsync-path", 3) || {
                    !w.starts_with('-')
                        && w.split_once(':')
                            .is_some_and(|(host, _)| !host.is_empty() && !host.contains('/'))
                }
            }) || shorts(args).any(|w| w.contains('e'))
        }
        _ => false,
    };
    driven.then(|| {
        format!(
            "refused: {} starts a program outside Flottille's tree of processes, later or on a \
             host, where the gate sees nothing of it: an agent runs its programs itself, in any \
             mode",
            shown_words(
                &std::iter::once(program.to_owned())
                    .chain(args.iter().cloned())
                    .collect::<Vec<_>>()
            )
        )
    })
}

/// What git does, refused in every mode: a configuration that runs a program later, as the user
/// (the user's own, the system's, or a key that runs one), a program named to run on a remote.
fn git_refusal(args: &[String]) -> Option<String> {
    let (at, globals) = git_globals(args);
    for (option, value) in &globals {
        let key = value.unwrap_or_default();
        let key = key.split_once('=').map_or(key, |(key, _)| key);
        if matches!(*option, "-c" | "--config-env") && key.eq_ignore_ascii_case("core.sshCommand") {
            return Some(format!(
                "refused: git {option} {key} names a program git runs for a remote, outside the \
                 gate: an agent never names one, in any mode"
            ));
        }
    }
    let command = args.get(at).map(String::as_str)?;
    let rest = args.get(at + 1..).unwrap_or_default();
    if command == "credential" || command.starts_with("credential-") {
        return Some(format!(
            "refused: git {command} reads or prints a login, and agents never see one, in any mode"
        ));
    }
    if command == "config" && !config_reads(rest) {
        if let Some(scope) = rest
            .iter()
            .find(|w| ["--global", "--system"].contains(&w.as_str()))
        {
            return Some(format!(
                "refused: git config {scope} writes the user's or the system's own \
                 configuration, which every git the user runs reads: an agent never writes it, \
                 in any mode"
            ));
        }
        let removes = rest
            .iter()
            .any(|w| ["--unset", "--unset-all", "--remove-section"].contains(&w.as_str()))
            || plain(rest)
                .first()
                .is_some_and(|w| ["unset", "remove-section"].contains(w));
        if let Some(key) = config_keys(rest)
            .1
            .into_iter()
            .find(|key| !removes && runs_later(key))
        {
            return Some(format!(
                "refused: git config writes {key}, which runs a program or brings in a file the \
                 next time the user's own git runs, outside the gate: an agent never writes it, \
                 in any mode"
            ));
        }
    }
    if [
        "clone",
        "fetch",
        "pull",
        "push",
        "send-pack",
        "ls-remote",
        "archive",
    ]
    .contains(&command)
    {
        let named = rest.iter().take_while(|w| *w != "--").find(|w| {
            abbreviates(w, "upload-pack", 3)
                || abbreviates(w, "receive-pack", 4)
                || (command != "fetch" && command != "pull" && abbreviates(w, "exec", 2))
                || (command == "clone" && (*w == "-u" || w.starts_with("-u")))
        });
        if let Some(word) = named {
            return Some(format!(
                "refused: git {command} {word} names the program it runs on the remote, which \
                 may be this machine, outside the gate: an agent never names one, in any mode"
            ));
        }
    }
    None
}

/// Whether git's configuration under `key` runs a program, or brings in a file, the next time git
/// runs: hooks, aliases, includes, drivers. Where pushes go is base, not refused.
fn runs_later(key: &str) -> bool {
    let section = key.split('.').next().unwrap_or(key).to_ascii_lowercase();
    let name = key
        .rsplit_once('.')
        .map(|(_, name)| name.to_ascii_lowercase());
    match section.as_str() {
        "remote" | "url" | "push" => false,
        "core" => name.is_some_and(|name| {
            [
                "hookspath",
                "fsmonitor",
                "sshcommand",
                "editor",
                "pager",
                "askpass",
                "gitproxy",
            ]
            .contains(&name.as_str())
        }),
        _ => risky_setting(key).is_some(),
    }
}

/// What a container's command line gives it of the host: each bind mount's source that names a
/// path (`-v /x:/y`, `--mount type=bind,source=/x`), not a named volume.
fn mounts(layer: &[String]) -> Vec<&str> {
    if !["docker", "podman", "nerdctl"].contains(&base(&layer[0])) {
        return Vec::new();
    }
    let args = &layer[1..];
    let Some(at) = args.iter().position(|w| w == "run" || w == "create") else {
        return Vec::new();
    };
    let mut sources = Vec::new();
    let mut words = args[at + 1..].iter().map(String::as_str);
    while let Some(word) = words.next() {
        let (volume, mount) = match word {
            "-v" | "--volume" => (words.next(), None),
            "--mount" => (None, words.next()),
            _ => (
                word.strip_prefix("--volume=")
                    .or_else(|| word.strip_prefix("-v").filter(|v| !v.is_empty())),
                word.strip_prefix("--mount="),
            ),
        };
        if let Some(volume) = volume {
            sources.push(volume.split(':').next().unwrap_or(volume));
        }
        if let Some(mount) = mount {
            sources.extend(mount.split(',').filter_map(|pair| {
                pair.strip_prefix("source=")
                    .or_else(|| pair.strip_prefix("src="))
            }));
        }
    }
    sources.retain(|source| source.starts_with('/') || source.starts_with('.'));
    sources
}

/// The program a runner runs: what follows `uv run`, `poetry run`, `npm exec`…, or what follows
/// `--` of `cargo run`.
fn runs(layer: &[String]) -> Option<&str> {
    let program = base(&layer[0]);
    let args = &layer[1..];
    let command = args.iter().position(|w| !w.starts_with('-'))?;
    let verb = args[command].as_str();
    let after = &args[command + 1..];
    let runner = match program {
        "cargo" => {
            return (verb == "run")
                .then(|| after.iter().skip_while(|w| *w != "--").nth(1))
                .flatten()
                .map(String::as_str);
        }
        "uv" | "poetry" | "pipenv" | "pdm" | "hatch" | "rye" | "pixi" | "conda" | "mamba" => {
            verb == "run"
        }
        "npm" | "pnpm" | "yarn" | "bundle" | "mise" => verb == "exec" || verb == "x",
        "bun" => verb == "x",
        _ => false,
    };
    if !runner {
        return None;
    }
    after
        .iter()
        .find(|w| !w.starts_with('-'))
        .map(String::as_str)
}

/// The refusal of a program that starts what it runs outside the daemon's tree of processes, or
/// later, or on a host (`ssh localhost` is this machine, as the user, through its shell).
fn launches(program: &str) -> Option<String> {
    LAUNCHERS.contains(&program).then(|| {
        format!(
            "refused: {program} starts what it runs outside Flottille's tree of processes, later \
             or on a host, where the gate sees nothing of it: an agent runs its programs itself, \
             in any mode"
        )
    })
}

/// Whether a forge's command line prints its login.
fn prints_token(args: &[String]) -> bool {
    let words = forge_words(args);
    words.first() == Some(&"auth")
        && (words.get(1) == Some(&"token")
            || args.iter().any(|w| {
                w == "--show-token"
                    || (w.starts_with('-') && !w.starts_with("--") && w.contains('t'))
            }))
}

/// What a layer's program does. A program counts as one the rules know only when it is the
/// system's: named bare and found through `PATH` outside the folders the agent writes.
fn program(call: &Call, layer: &[String]) -> Line {
    let program = base(&layer[0]);
    let args = &layer[1..];
    let any = |names: &[&str]| args.iter().any(|w| names.contains(&w.as_str()));
    let line = match program {
        "git" => git(call, layer),
        "gh" | "glab" if forge_reads(program, args) => Line::Reads,
        "gh" | "glab" => Line::Judge(format!(
            "{} writes to the forge, or is not one of its reads",
            shown_words(layer)
        )),
        "bkt" => Line::Base(
            "bkt is Bitbucket's command line, signed in as the user: whatever it runs, it runs \
             with the user's own token"
                .into(),
        ),
        "sudo" | "sudoedit" | "doas" | "pkexec" | "run0" | "su" => Line::Base(format!(
            "{program} runs a program as another user, root by default, with an environment of \
             its own, without the wall that keeps git from pushing behind the gate"
        )),
        "acli" => Line::Base(
            "acli is Atlassian's command line, signed in as the user to Jira: whatever it runs, \
             it runs with the user's own sign-in"
                .into(),
        ),
        "npm" | "pnpm" | "yarn" | "bun" | "cargo" | "poetry" | "flit" | "hatch" | "uv"
            if any(&["publish", "unpublish"]) =>
        {
            Line::Base(publishing(layer))
        }
        "twine" if any(&["upload"]) => Line::Base(publishing(layer)),
        "gem" | "dotnet" | "nuget" if any(&["push"]) => Line::Base(publishing(layer)),
        _ if READS.contains(&program) => reads(call, layer),
        _ => Line::Writes,
    };
    match line {
        Line::Reads | Line::Files => match installed(call, &layer[0]) {
            Ok(()) => line,
            Err(why) => Line::Judge(why),
        },
        line => line,
    }
}

/// Whether a program named in a line is the system's: a bare name `PATH` leads to, outside the
/// folders the agent writes. Anything else may be a file the agent wrote, whatever its name.
fn installed(call: &Call, word: &str) -> Result<(), String> {
    let own = |what: String| {
        Err(format!(
            "{what}: the gate takes it for the agent's own file, which may do anything, not for \
             the program of that name it knows"
        ))
    };
    if word.contains('/') {
        return own(format!("{word} names its program by a path"));
    }
    let folder = call.paths.first().map(|folder| folder.path.as_path());
    for dir in std::env::split_paths(call.search.unwrap_or_default()) {
        let dir = match folder {
            Some(folder) if dir.is_relative() => folder.join(dir),
            _ if dir.is_relative() => continue,
            _ => dir,
        };
        let candidate = dir.join(word);
        let executable = std::fs::metadata(&candidate).is_ok_and(|meta| {
            use std::os::unix::fs::PermissionsExt;
            meta.is_file() && meta.permissions().mode() & 0o111 != 0
        });
        if !executable {
            continue;
        }
        let real = std::fs::canonicalize(&candidate).unwrap_or(candidate);
        if let Some(bin) = user_bins(call)
            .iter()
            .find(|bin| real.starts_with(bin) || dir.starts_with(bin))
        {
            return own(format!(
                "{word} is found in {}, the user's own programs, where an agent a judge allowed \
                 may have written it",
                bin.display()
            ));
        }
        if call
            .writable
            .iter()
            .any(|folder| real.starts_with(folder) || dir.starts_with(folder))
        {
            return own(format!(
                "{word} is found in {}, a folder the agent writes",
                dir.display()
            ));
        }
        return Ok(());
    }
    own(format!(
        "{word} is not found where PATH leads outside the agent's folders"
    ))
}

/// The folders of the user's own programs, `~/.local/bin` and `~/bin`, as written and as they
/// really are.
fn user_bins(call: &Call) -> Vec<PathBuf> {
    let Some(home) = &call.grant.place.home else {
        return Vec::new();
    };
    let bins = [home.join(".local/bin"), home.join("bin")];
    let real: Vec<PathBuf> = bins
        .iter()
        .filter_map(|bin| std::fs::canonicalize(bin).ok())
        .collect();
    bins.into_iter().chain(real).collect()
}

/// What a program that only reads does with these arguments: an option that runs a program is
/// to judge, one that writes a file writes, a recursive read over a folder that holds a
/// sensitive file is to judge.
fn reads(call: &Call, layer: &[String]) -> Line {
    let program = base(&layer[0]);
    let args = &layer[1..];
    let short = |letters: &[char]| shorts(args).any(|word| word[1..].contains(letters));
    if let Some((why, _)) = executed(layer) {
        return Line::Judge(why);
    }
    match program {
        "find" if args.iter().any(|w| w == "-delete") => return Line::Writes,
        // `-s`, alone or after flags that take no value (`-us`); not `-Iseconds`.
        "date"
            if shorts(args).any(|w| {
                w[1..]
                    .split_once('s')
                    .is_some_and(|(before, _)| before.chars().all(|c| "uR".contains(c)))
            }) || args.iter().any(|w| abbreviates(w, "set", 2)) =>
        {
            return Line::Judge("date -s sets the system's clock".into());
        }
        _ => {}
    }
    let writes = match program {
        "find" => args
            .iter()
            .any(|w| ["-fprint", "-fprint0", "-fprintf", "-fls"].contains(&w.as_str())),
        "tree" => args.iter().any(|w| w.starts_with("-o")),
        "file" => short(&['C']) || args.iter().any(|w| abbreviates(w, "compile", 3)),
        _ => false,
    };
    if writes {
        return Line::Files;
    }
    match walks_secrets(call, layer) {
        Some(why) => Line::Judge(why),
        None => Line::Reads,
    }
}

/// The short options of a line's arguments (`-rn`), before any `--`.
fn shorts(args: &[String]) -> impl Iterator<Item = &str> {
    args.iter()
        .map(String::as_str)
        .take_while(|word| *word != "--")
        .filter(|word| word.len() > 1 && word.starts_with('-') && !word.starts_with("--"))
}

/// Whether a word is a long option that names `option`, whole or by a start of at least `min`
/// letters, as getopt and git take it.
fn abbreviates(word: &str, option: &str, min: usize) -> bool {
    word.strip_prefix("--")
        .map(|long| long.split_once('=').map_or(long, |(name, _)| name))
        .is_some_and(|name| name.len() >= min.min(option.len()) && option.starts_with(name))
}

/// Why a layer's options run a program, and the lines they run that the gate can read: `find
/// -exec`, `fd -x`, `rg --pre`, `git grep -O`, git's external diff and text conversion.
fn executed(layer: &[String]) -> Option<(String, Vec<Vec<String>>)> {
    let program = base(&layer[0]);
    let args = &layer[1..];
    let mut lines = Vec::new();
    let mut runs = None;
    let until = |from: &[String], end: &[&str]| -> Vec<String> {
        from.iter()
            .take_while(|w| !end.contains(&w.as_str()))
            .filter(|w| *w != "{}")
            .cloned()
            .collect()
    };
    match program {
        "find" => {
            for (at, word) in args.iter().enumerate() {
                if ["-exec", "-execdir", "-ok", "-okdir"].contains(&word.as_str()) {
                    runs = Some(format!("find {word} runs a program on what it finds"));
                    lines.push(until(&args[at + 1..], &[";", "+"]));
                }
            }
        }
        "fd" => {
            for (at, word) in args.iter().enumerate() {
                let rest = &args[at + 1..];
                let mut line = if ["-x", "-X", "--exec", "--exec-batch"].contains(&word.as_str()) {
                    until(rest, &[";"])
                } else if let Some(first) = word
                    .strip_prefix("--exec=")
                    .or_else(|| word.strip_prefix("--exec-batch="))
                {
                    std::iter::once(first.to_owned())
                        .chain(until(rest, &[";"]))
                        .collect()
                } else if !word.starts_with("--")
                    && word.starts_with('-')
                    && let Some(x) = word.find(['x', 'X'])
                {
                    let attached = &word[x + 1..];
                    let mut line = until(rest, &[";"]);
                    if !attached.is_empty() {
                        line.insert(0, attached.to_owned());
                    }
                    line
                } else {
                    continue;
                };
                line.retain(|w| !w.is_empty());
                runs = Some(format!("fd {word} runs a program on what it finds"));
                lines.push(line);
            }
        }
        "rg" => {
            for (at, word) in args.iter().enumerate() {
                for option in ["--pre", "--hostname-bin"] {
                    let value = if word == option {
                        args.get(at + 1).cloned()
                    } else {
                        word.strip_prefix(&format!("{option}=")).map(str::to_owned)
                    };
                    if let Some(value) = value {
                        runs = Some(format!("rg {option} runs a program"));
                        lines.push(vec![value]);
                    }
                }
            }
        }
        "git" => {
            let (at, _) = git_globals(args);
            let command = args.get(at).map(String::as_str).unwrap_or_default();
            let rest = args.get(at + 1..).unwrap_or_default();
            let options = rest.iter().take_while(|w| *w != "--");
            for word in options {
                let pager = word == "-O"
                    || (word.starts_with('-') && !word.starts_with("--") && word.contains('O'))
                    || abbreviates(word, "open-files-in-pager", 2);
                if command == "grep" && pager {
                    runs = Some(format!(
                        "git grep {word} opens what it finds in a program it runs"
                    ));
                    let value = word
                        .split_once('=')
                        .map(|(_, v)| v)
                        .or_else(|| word.strip_prefix("-O"))
                        .filter(|v| !v.is_empty());
                    if let Some(line) =
                        value.and_then(|v| flottille_core::command::split_line(v).ok())
                    {
                        lines.push(line);
                    }
                }
                let converts = abbreviates(word, "ext-diff", 3)
                    || abbreviates(word, "textconv", 5)
                    || (command == "cat-file" && abbreviates(word, "filters", 3));
                if converts && !word.starts_with("--no-") {
                    runs = Some(format!(
                        "git {command} {word} runs a program the configuration names on what it \
                         reads"
                    ));
                }
            }
            for line in rebase_execs(args) {
                runs = Some("git rebase --exec runs a line after each commit it makes".into());
                lines.extend(flottille_core::command::split_line(line).ok());
            }
        }
        _ => {}
    }
    lines.retain(|line| !line.is_empty());
    runs.map(|why| (why, lines))
}

/// The lines a `git rebase` runs after each commit: `-x`, `--exec` and their attached forms.
fn rebase_execs(args: &[String]) -> Vec<&str> {
    let at = git_globals(args).0;
    if args.get(at).is_none_or(|command| command != "rebase") {
        return Vec::new();
    }
    let mut execs = Vec::new();
    let mut words = args[at + 1..].iter().map(String::as_str);
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        let value = if abbreviates(word, "exec", 2) {
            match word.split_once('=') {
                Some((_, value)) => Some(value),
                None => words.next(),
            }
        } else if word.starts_with('-') && !word.starts_with("--") {
            // `-x`, `-xLINE`, or `-x` last of short options (`-ix`).
            match word[1..].split_once('x') {
                Some((_, "")) => words.next(),
                Some((_, value)) => Some(value),
                None => None,
            }
        } else {
            None
        };
        execs.extend(value);
    }
    execs
}

/// Why a recursive read would read a sensitive file: it walks hidden or ignored files (or every
/// file, for grep), over a folder that holds one.
fn walks_secrets(call: &Call, layer: &[String]) -> Option<String> {
    let program = base(&layer[0]);
    let args = &layer[1..];
    let short = |letters: &[char]| shorts(args).any(|word| word[1..].contains(letters));
    let long = |names: &[&str]| {
        args.iter()
            .any(|word| names.iter().any(|name| abbreviates(word, name, 4)))
    };
    let walks = match program {
        "grep" | "egrep" | "fgrep" => {
            short(&['r', 'R'])
                || long(&["recursive", "dereference-recursive"])
                || args.windows(2).any(|w| w[0] == "-d" && w[1] == "recurse")
                || args
                    .iter()
                    .any(|w| w == "-drecurse" || w == "--directories=recurse")
        }
        "rg" => {
            short(&['u', '.', 'L'])
                || long(&["unrestricted", "hidden", "follow"])
                || args.iter().any(|w| w.starts_with("--no-ignore"))
        }
        "fd" => {
            short(&['H', 'I', 'u', 'L'])
                || long(&["hidden", "unrestricted", "follow"])
                || args.iter().any(|w| w.starts_with("--no-ignore"))
        }
        "find" => args
            .iter()
            .any(|w| ["-H", "-L", "-follow"].contains(&w.as_str())),
        "git" => long(&["untracked", "no-index", "no-exclude-standard"]),
        _ => false,
    };
    if !walks {
        return None;
    }
    let from = call.paths.first()?;
    let place = &call.grant.place;
    let mut folders: Vec<Resolved> = args
        .iter()
        .filter(|word| !word.starts_with('-'))
        .map(|word| place.resolve(&from.path, word))
        .filter(|folder| folder.inside && folder.path.is_dir())
        .collect();
    if folders.is_empty() {
        folders.push(from.clone());
    }
    let real_root = std::fs::canonicalize(&place.root).unwrap_or_else(|_| place.root.clone());
    for folder in folders {
        if let Some(what) = holds_secret(call.own, &real_root, &folder.path) {
            return Some(format!(
                "{} reads hidden, ignored or every file below {}, {what}",
                shown_words(layer),
                if folder.shown.is_empty() {
                    "the place's folder"
                } else {
                    &folder.shown
                }
            ));
        }
    }
    None
}

/// Files looked through for a sensitive one, at most, before a folder is taken for holding one.
const WALKED_MAX: usize = 20_000;

/// Whether a folder of the place holds a sensitive file, said as a reason: the first found, or
/// that it holds more files than the gate looks through. Links are not followed; `.git` folders
/// are not entered.
fn holds_secret(own: &[PathBuf], root: &Path, folder: &Path) -> Option<String> {
    let mut walked = 0;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            walked += 1;
            if walked > WALKED_MAX {
                return Some(format!(
                    "which holds more than {WALKED_MAX} files: too many to tell that none is \
                     sensitive"
                ));
            }
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let resolved =
                Resolved::real_inside(relative.to_string_lossy().into_owned(), path.clone());
            if let Some(place) = sensitive(own, &resolved) {
                return Some(format!(
                    "which holds a sensitive file: {} ({place})",
                    resolved.shown
                ));
            }
            let directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if directory && entry.file_name() != ".git" {
                pending.push(path);
            }
        }
    }
    None
}

/// Why a path in `/proc` is base: what leads elsewhere (`place::LEADS_ELSEWHERE`) of any process, a
/// process's environment, with its secrets, or anything of a process that is not one of the
/// agent's runs. The reading program's own (`self`) is its own.
pub fn process(groups: &[u32], path: &Resolved) -> Option<String> {
    // Where it leads is not known: through a process's folder under another name, or a path the
    // disk would not let the gate follow.
    if let Some(why) = &path.unfollowed {
        return Some(format!("{} {why}", path.shown));
    }
    if path.inside {
        return None;
    }
    let rest = path.path.strip_prefix("/proc").ok()?;
    let parts: Vec<String> = rest
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let process = parts.first()?;
    if let Some(part) = parts[1..]
        .iter()
        .find(|part| place::LEADS_ELSEWHERE.contains(&part.as_str()))
    {
        return Some(format!(
            "{} goes through a process's {part}, which leads anywhere on the disk, Flottille's \
             own folders included",
            path.shown
        ));
    }
    let id = process.parse::<u32>().ok();
    if id.is_none() && process != "self" && process != "thread-self" {
        return None;
    }
    if parts.iter().any(|part| part == "environ") {
        return Some(format!(
            "{} is a process's environment, which holds its secrets",
            path.shown
        ));
    }
    let id = id?;
    if group_of(id).is_some_and(|group| groups.contains(&group)) {
        return None;
    }
    Some(format!(
        "{} belongs to a process that is not one of this agent's runs: its command line, its \
         memory and its files are not the agent's",
        path.shown
    ))
}

/// A process's group, from `/proc/<pid>/stat`.
fn group_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // After the name in parentheses: state, parent, group.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

/// The variables of the environment whose value is a program git runs.
const GIT_PROGRAMS: [&str; 9] = [
    "GIT_EXTERNAL_DIFF",
    "GIT_PAGER",
    "GIT_EDITOR",
    "GIT_SEQUENCE_EDITOR",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_ASKPASS",
    "GIT_PROXY_COMMAND",
    "GIT_EXEC_PATH",
];

/// Why a layer would lift what keeps an agent's git from pushing and running hooks, git's
/// configuration set or cleared through the environment, or name there a program git runs.
fn unwalls(layer: &[String]) -> Option<String> {
    let program = base(&layer[0]);
    if READS.contains(&program) {
        return None;
    }
    let args = &layer[1..];
    let git_config = |name: &str| name.starts_with("GIT_CONFIG");
    let named = |word: &String| word.split_once('=').map(|(name, _)| name.to_owned());
    if let Some(name) = args
        .iter()
        .filter(|word| assignment(word))
        .filter_map(named)
        .find(|name| GIT_PROGRAMS.contains(&name.as_str()))
    {
        return Some(format!(
            "{name} names a program git runs, set through the environment"
        ));
    }
    if let Some(word) = args
        .iter()
        .find(|word| assignment(word) && word.split_once('=').is_some_and(|(n, _)| git_config(n)))
    {
        return Some(format!(
            "{} sets git's configuration through the environment, which can lift Flottille's \
             wall on pushes and hooks",
            word.split_once('=').map_or(word.as_str(), |(name, _)| name)
        ));
    }
    if program != "env" {
        return None;
    }
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if !word.starts_with('-') {
            break;
        }
        let unset = match word {
            "-u" | "--unset" => words.next(),
            _ => word
                .strip_prefix("--unset=")
                .or_else(|| word.strip_prefix("-u").filter(|_| !word.starts_with("--"))),
        };
        let clears = word == "-"
            || abbreviates(word, "ignore-environment", 2)
            || (!word.starts_with("--") && word.contains('i') && unset.is_none());
        if clears || unset.is_some_and(git_config) {
            return Some(format!(
                "env {word} clears the environment that keeps git from pushing and running \
                 hooks behind the gate"
            ));
        }
    }
    None
}

fn publishing(layer: &[String]) -> String {
    format!("{} publishes a package or a release", shown_words(layer))
}

/// The first words of a line, as a reason names them.
fn shown_words(words: &[String]) -> String {
    let shown: Vec<&str> = words.iter().take(3).map(|word| base(word)).collect();
    let mut text = shown.join(" ");
    text.truncate(200);
    text
}

/// What git's configuration under `key` can do that a call must not do behind the user's back,
/// for the sections that run programs, lead pushes elsewhere or bring in other files.
fn risky_setting(key: &str) -> Option<&'static str> {
    let section = key.split('.').next().unwrap_or(key).to_ascii_lowercase();
    // The setting's own name is the last part; a subsection may sit between them.
    let name = match key.rsplit_once('.') {
        Some((_, name)) => name.to_ascii_lowercase(),
        None => String::new(),
    };
    Some(match (section.as_str(), name.as_str()) {
        ("core", _) => "can run a program or move git's hooks",
        ("alias", _) => "can make a git command run any program",
        ("remote" | "url", _) => "changes where git fetches and pushes",
        ("push", _) => "changes what a push sends",
        ("credential", _) => "changes how git finds the user's credentials",
        _ if section.starts_with("include") => "brings in another configuration file",
        ("diff", "external" | "command" | "textconv")
        | ("difftool" | "mergetool" | "man" | "browser", "cmd" | "path")
        | ("merge", "driver")
        | ("filter" | "pager" | "hook" | "guitool" | "sendemail" | "instaweb", _)
        | ("gpg", "program" | "defaultkeycommand")
        | ("sequence", "editor")
        | ("interactive", "difffilter")
        | ("submodule", "update")
        | ("uploadpack", "packobjectshook")
        | ("imap", "tunnel")
        | ("tar", "command")
        | ("web", "browser")
        | ("trailer", "command" | "cmd") => "names a program git runs",
        ("protocol", "allow") => "can let a remote's URL run a program (ext::)",
        _ => return None,
    })
}

/// What a git line does: its reads are harmless, adding or moving files in a workspace too;
/// forcing a push, rewriting a pushed branch, discarding the work of the user's checkout and
/// setting what runs programs or leads pushes are base; the rest is to judge. Global options are
/// skipped to class the command, and count too: the stronger wins.
fn git(call: &Call, layer: &[String]) -> Line {
    let args = &layer[1..];
    let (at, globals) = git_globals(args);
    let mut options: Option<Line> = None;
    for (option, value) in globals {
        let line = match option {
            "-C"
            | "--no-pager"
            | "-P"
            | "-p"
            | "--paginate"
            | "--no-replace-objects"
            | "--literal-pathspecs"
            | "--glob-pathspecs"
            | "--noglob-pathspecs"
            | "--icase-pathspecs"
            | "--no-optional-locks"
            | "--no-advice"
            | "--no-lazy-fetch" => {
                continue;
            }
            "-c" | "--config-env" => {
                let key = value.unwrap_or_default();
                let key = key.split_once('=').map_or(key, |(key, _)| key);
                match risky_setting(key) {
                    Some(why) => Line::Base(format!("git {option} {key}: it {why}")),
                    None => Line::Judge(format!(
                        "git {option} {key} changes git's configuration for this one command"
                    )),
                }
            }
            _ => Line::Judge(format!(
                "git {option} points git at another repository or changes how it runs"
            )),
        };
        options = Some(match options {
            Some(before) => before.or(line),
            None => line,
        });
    }
    let line = match args.get(at) {
        Some(command) => command_line(call, layer, command, &args[at + 1..]),
        None => Line::Reads,
    };
    match options {
        Some(options) => line.or(options),
        None => line,
    }
}

/// What git's command does, with its arguments.
fn command_line(call: &Call, layer: &[String], command: &str, rest: &[String]) -> Line {
    let main = call.grant.place.workspace.is_none();
    if main && let Some(why) = discards_work(call, layer, command, rest) {
        return Line::Base(why);
    }
    let any = |names: &[&str]| rest.iter().any(|w| names.contains(&w.as_str()));
    // Rewriting a branch others may have, or the user's own, is base.
    let rewrite = |what: String| {
        if main || call.pushed {
            Line::Base(format!(
                "{what}, on a branch that is {}",
                if main {
                    "the user's own checkout's"
                } else {
                    "already on a remote"
                }
            ))
        } else {
            Line::Judge(what)
        }
    };
    if GIT_READS.contains(&command) || command == "format-patch" {
        if let Some((why, _)) = executed(layer) {
            return Line::Judge(why);
        }
        if command == "help" && rest.iter().any(|w| w.starts_with('-')) {
            return Line::Judge(format!(
                "git help {} may open a browser or a viewer",
                rest.join(" ")
            ));
        }
        let writes = match command {
            "format-patch" => !any(&["--stdout"]),
            "diff" | "log" | "show" | "whatchanged" => {
                rest.iter().any(|w| abbreviates(w, "output", 3))
            }
            _ => false,
        };
        if writes {
            return Line::Files;
        }
        if command == "grep"
            && let Some(why) = walks_secrets(call, layer)
        {
            return Line::Judge(why);
        }
        return Line::Reads;
    }
    match command {
        "push" | "send-pack" | "http-push" => push(rest),
        "subtree" | "lfs" if any(&["push"]) => push(rest),
        "filter-branch" | "filter-repo" => Line::Base(format!(
            "git {command} rewrites the whole history of the repository"
        )),
        "commit" if amends(rest) => rewrite("git commit --amend rewrites the last commit".into()),
        "commit" => Line::Judge("git commit makes a commit".into()),
        "rebase" => rewrite("git rebase rewrites the branch's history".into()),
        "reset" if any(&["--hard"]) => {
            Line::Judge("git reset --hard throws away the uncommitted work of the workspace".into())
        }
        "reset" if rest.iter().any(|w| !w.starts_with('-')) && !any(&["--"]) => {
            rewrite("git reset moves the branch".into())
        }
        "update-ref" => rewrite("git update-ref moves a ref".into()),
        "tag"
            if rest.is_empty()
                || any(&["-l", "--list"]) && !any(&["-d", "--delete", "-f", "--force"]) =>
        {
            Line::Reads
        }
        "branch" if branch_reads(rest) => Line::Reads,
        "config" if config_reads(rest) => Line::Reads,
        "config" => config_writes(rest),
        "remote" if remote_reads(rest) => Line::Reads,
        "remote" => remote_writes(rest),
        "symbolic-ref" if !symbolic_ref_writes(rest) => Line::Reads,
        "replace" if replace_reads(rest) => Line::Reads,
        "notes" if notes_reads(rest) => Line::Reads,
        "stash" if matches!(rest.first().map(String::as_str), Some("list" | "show")) => Line::Reads,
        "worktree" if rest.first().is_some_and(|w| w == "list") => Line::Reads,
        "reflog" if matches!(rest.first().map(String::as_str), Some("expire" | "delete")) => {
            Line::Base(safety_net(layer))
        }
        "gc" if rest
            .iter()
            .any(|w| w.contains('=') && abbreviates(w, "prune", 2)) =>
        {
            Line::Base(safety_net(layer))
        }
        "prune" if !any(&["-n", "--dry-run"]) => Line::Base(safety_net(layer)),
        "reflog" if matches!(rest.first().map(String::as_str), None | Some("show")) => Line::Reads,
        // Writing the files of a workspace, through git.
        "add" | "mv" | "rm" if !main => Line::Files,
        _ => Line::Judge(format!(
            "git {command} changes the repository, its branches or a remote"
        )),
    }
}

/// Why removing the reflog or unreachable objects is base: they are what recovers lost commits,
/// and the project folder and its workspaces share them.
fn safety_net(layer: &[String]) -> String {
    format!(
        "{} removes what git keeps to recover lost commits, the user's safety net, shared by the \
         project folder and its workspaces",
        shown_words(layer)
    )
}

/// `git config` that writes: base for a setting that runs programs or leads pushes, to judge
/// otherwise.
fn config_writes(rest: &[String]) -> Line {
    let (edit, keys) = config_keys(rest);
    if edit {
        return Line::Base(
            "git config --edit opens the whole configuration to change, whatever it sets".into(),
        );
    }
    for key in keys {
        if let Some(why) = risky_setting(key) {
            return Line::Base(format!("git config writes {key}: it {why}"));
        }
    }
    Line::Judge("git config changes git's configuration".into())
}

/// Whether a `git config` that writes opens the whole configuration, and the keys it writes.
fn config_keys(rest: &[String]) -> (bool, Vec<&str>) {
    let mut positional = Vec::new();
    let mut edit = false;
    let mut renames = false;
    let mut words = rest.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            "-f" | "--file" | "--blob" | "-t" | "--type" | "--default" | "--comment"
            | "--value" => {
                words.next();
            }
            "-e" | "--edit" => edit = true,
            "--rename-section" => renames = true,
            _ if word.starts_with('-') => {}
            _ => positional.push(word),
        }
    }
    let keys: Vec<&str> = match positional.first().copied() {
        Some("edit") => {
            edit = true;
            Vec::new()
        }
        Some("set" | "unset" | "remove-section") => {
            positional.get(1).copied().into_iter().collect()
        }
        Some("rename-section") => positional.iter().skip(1).take(2).copied().collect(),
        _ if renames => positional.iter().take(2).copied().collect(),
        _ => positional.first().copied().into_iter().collect(),
    };
    (edit, keys)
}

/// `git remote` that writes: changing a remote's URL or the remotes themselves is base, since a
/// push trusts where a remote leads.
fn remote_writes(rest: &[String]) -> Line {
    let command = rest
        .iter()
        .find(|w| !w.starts_with('-'))
        .map(String::as_str)
        .unwrap_or_default();
    if ["add", "rename", "remove", "rm", "set-url", "set-branches"].contains(&command) {
        Line::Base(format!(
            "git remote {command} changes where git fetches and pushes"
        ))
    } else {
        Line::Judge(format!("git remote {command} changes a remote"))
    }
}

/// Whether a line pushes with git, at any of its layers: the run checks its remote first.
pub fn pushes(words: &[String]) -> bool {
    lines(words).is_ok_and(|layers| {
        layers.iter().any(|layer| {
            suffixes(layer).iter().any(|(_, suffix)| {
                base(&suffix[0]) == "git" && {
                    let args = &suffix[1..];
                    args.get(git_globals(args).0).is_some_and(|w| w == "push")
                }
            })
        })
    })
}

/// Whether the line's own program is git pushing, with no wrapper around it: the push the gate
/// read, which may go out once it passed.
pub fn is_push(words: &[String]) -> bool {
    words.first().is_some_and(|program| program == "git") && {
        let args = &words[1..];
        args.get(git_globals(args).0).is_some_and(|w| w == "push")
    }
}

/// git's global options before its command: where the command is, and each option with its
/// value.
fn git_globals(args: &[String]) -> (usize, Vec<(&str, Option<&str>)>) {
    const VALUED: [&str; 8] = [
        "-C",
        "-c",
        "--git-dir",
        "--work-tree",
        "--namespace",
        "--super-prefix",
        "--config-env",
        "--attr-source",
    ];
    let mut at = 0;
    let mut options = Vec::new();
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        if !word.starts_with('-') {
            break;
        }
        at += 1;
        if VALUED.contains(&word) {
            options.push((word, args.get(at).map(String::as_str)));
            at += 1;
        } else if let Some(value) = word.strip_prefix("-c").filter(|_| !word.starts_with("--")) {
            options.push(("-c", Some(value)));
        } else if let Some((name, value)) = word.split_once('=') {
            options.push((name, Some(value)));
        } else {
            options.push((word, None));
        }
    }
    (at, options)
}

/// A push: forcing it, deleting on the remote or naming the program that receives it is base,
/// anything else to judge. Git takes any unambiguous start of a long option.
fn push(rest: &[String]) -> Line {
    const FORCING: [&str; 6] = [
        "force",
        "force-with-lease",
        "force-if-includes",
        "mirror",
        "delete",
        "prune",
    ];
    const RUNNING: [&str; 2] = ["receive-pack", "exec"];
    let mut options = true;
    for word in rest {
        let word = word.as_str();
        if options && word == "--" {
            options = false;
            continue;
        }
        if options
            && let Some(name) = word
                .strip_prefix("--")
                .and_then(|long| long.split('=').next())
            && !name.is_empty()
            && RUNNING.iter().any(|option| option.starts_with(name))
        {
            return Line::Base(format!(
                "git push {word} names the program it runs to receive the push, on this machine \
                 for a remote that is a path"
            ));
        }
        let forced = if options && word.starts_with("--") {
            let name = word[2..].split('=').next().unwrap_or_default();
            !name.is_empty()
                && !name.starts_with("no-")
                && FORCING.iter().any(|option| option.starts_with(name))
        } else if options && word.starts_with('-') && word.len() > 1 {
            word[1..].contains(['f', 'd'])
        } else {
            word.starts_with('+') || word.starts_with(':')
        };
        if forced {
            return Line::Base(format!(
                "git push {word} forces the push or deletes on the remote: it can rewrite or \
                 remove what others have"
            ));
        }
    }
    Line::Judge("git push sends commits to a remote".into())
}

/// Whether a `git commit` rewrites the last commit: `--amend`, or a fixup that amends or
/// rewords. Git takes any unambiguous start of a long option.
fn amends(args: &[String]) -> bool {
    let rewrites = |value: &str| value.starts_with("amend:") || value.starts_with("reword:");
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            let abbreviates = |option: &str| name.len() >= 3 && option.starts_with(name);
            if abbreviates("amend") {
                return true;
            }
            if abbreviates("fixup") && value.or_else(|| words.next()).is_some_and(rewrites) {
                return true;
            }
        }
    }
    false
}

/// Whether `git branch` with these words only lists.
fn branch_reads(rest: &[String]) -> bool {
    rest.iter().all(|word| {
        [
            "-a",
            "--all",
            "-r",
            "--remotes",
            "-v",
            "-vv",
            "--verbose",
            "-l",
            "--list",
            "--show-current",
            "--no-color",
            "--color",
        ]
        .contains(&word.as_str())
            || word.starts_with("--format=")
            || word.starts_with("--sort=")
    })
}

/// The first two words of a forge command line that are not options.
fn forge_words(args: &[String]) -> Vec<&str> {
    let mut words = Vec::new();
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        if ["-R", "--repo", "--hostname"].contains(&word) {
            at += 2;
            continue;
        }
        at += 1;
        if !word.starts_with('-') {
            words.push(word);
            if words.len() == 2 {
                break;
            }
        }
    }
    words
}

/// Whether a forge command line only reads.
fn forge_reads(program: &str, args: &[String]) -> bool {
    match (program, forge_words(args).as_slice()) {
        (_, ["api", ..]) => api_reads(args),
        ("gh", ["pr", "view" | "list" | "diff" | "checks" | "status"])
        | ("gh", ["issue", "view" | "list"])
        | ("gh", ["run", "view" | "list"])
        | ("glab", ["mr", "view" | "list" | "diff"])
        | ("glab", ["issue", "view" | "list"])
        | ("glab", ["ci" | "pipeline", "view" | "list" | "status"])
        | (_, ["repo", "view"])
        | (_, ["auth", "status"]) => true,
        _ => false,
    }
}

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// `NAME=value`, as `env` reads it.
fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The line, then each program a wrapper runs, unwrapped in turn.
fn layers(words: &[String]) -> Result<Vec<Vec<String>>, String> {
    let mut layers = vec![words.to_vec()];
    while let Some(inner) = inner(layers.last().expect("one layer at least"))? {
        if layers.len() == LAYERS_MAX {
            return Err(format!(
                "refused: the line wraps its program in more than {LAYERS_MAX} wrappers: run it \
                 directly"
            ));
        }
        layers.push(inner);
    }
    Ok(layers)
}

/// The layers of a line, then those of each line an option of theirs runs (`find -exec`, `fd
/// -x`…), unwrapped in turn.
fn lines(words: &[String]) -> Result<Vec<Vec<String>>, String> {
    let mut all = layers(words)?;
    let mut at = 0;
    while at < all.len() {
        for line in executed(&all[at])
            .map(|(_, lines)| lines)
            .unwrap_or_default()
        {
            for layer in layers(&line)? {
                if all.len() == LAYERS_MAX {
                    return Err(format!(
                        "refused: the line runs programs in more than {LAYERS_MAX} layers: run \
                         them directly"
                    ));
                }
                all.push(layer);
            }
        }
        at += 1;
    }
    Ok(all)
}

/// The lines a layer may run: itself, then, unless its program only reads, each of its ends that
/// starts with a word that is not an option, since a wrapper the gate does not know runs one of
/// them. A git layer's start only after its command (`git bisect run …`): `git rm` is not `rm`.
fn suffixes(layer: &[String]) -> Vec<(usize, &[String])> {
    let mut all = vec![(0, layer)];
    let program = base(&layer[0]);
    if READS.contains(&program) {
        return all;
    }
    let from = if program == "git" {
        git_globals(&layer[1..]).0 + 2
    } else {
        1
    };
    all.extend(
        (from..layer.len())
            .filter(|&at| !layer[at].starts_with('-'))
            .map(|at| (at, &layer[at..])),
    );
    all
}

/// The words a wrapper runs, `None` when the program is not one (or runs nothing), or the
/// refusal of a line it runs that cannot be split.
fn inner(words: &[String]) -> Result<Option<Vec<String>>, String> {
    let program = base(&words[0]);
    let args = &words[1..];
    let from = |at: usize| Some(args.get(at..)?.to_vec()).filter(|rest| !rest.is_empty());
    Ok(match program {
        "env" => return env(args),
        "sudo" => from(skip(
            args,
            &[
                "-u",
                "-g",
                "-C",
                "-D",
                "-p",
                "-r",
                "-t",
                "-U",
                "-T",
                "-R",
                "--user",
                "--group",
                "--chdir",
                "--prompt",
                "--role",
                "--type",
                "--other-user",
                "--close-from",
                "--command-timeout",
                "--chroot",
                "--host",
            ],
            true,
        )),
        "doas" => from(skip(args, &["-u", "-C"], false)),
        "nice" => from(skip(args, &["-n", "--adjustment"], false)),
        "ionice" => from(skip(args, &["-c", "--class", "-n", "--classdata"], false)),
        // The priority, or the CPUs' mask, comes before the program.
        "chrt" => from(
            skip(
                args,
                &[
                    "-T",
                    "--sched-runtime",
                    "-P",
                    "--sched-period",
                    "-D",
                    "--sched-deadline",
                ],
                false,
            ) + 1,
        ),
        "taskset" => from(skip(args, &[], false) + 1),
        "nohup" | "command" | "chronic" => from(skip(args, &[], false)),
        "exec" => from(skip(args, &["-a"], false)),
        // Its first word is the duration.
        "timeout" => from(skip(args, &["-s", "--signal", "-k", "--kill-after"], false) + 1),
        "time" => from(skip(args, &["-f", "--format", "-o", "--output"], false)),
        "stdbuf" => from(skip(
            args,
            &["-i", "-o", "-e", "--input", "--output", "--error"],
            false,
        )),
        "xargs" => from(skip(
            args,
            &[
                "-a",
                "--arg-file",
                "-d",
                "--delimiter",
                "-E",
                "-I",
                "-L",
                "--max-lines",
                "-n",
                "--max-args",
                "-P",
                "--max-procs",
                "-s",
                "--max-chars",
                "--process-slot-var",
            ],
            false,
        )),
        "npx" => {
            for (at, word) in args.iter().enumerate() {
                if word == "-c" || word == "--call" {
                    return split(program, args.get(at + 1).map(String::as_str), &[]);
                }
                if let Some(line) = word.strip_prefix("--call=") {
                    return split(program, Some(line), &[]);
                }
            }
            from(skip(args, &["-p", "--package"], false))
        }
        "pnpm" | "yarn" if args.first().is_some_and(|w| w == "dlx" || w == "exec") => {
            from(1 + skip(&args[1..], &["--package"], false))
        }
        "bunx" => from(skip(args, &["-p", "--package"], false)),
        "uvx" => from(skip(
            args,
            &[
                "--from",
                "--with",
                "--with-editable",
                "--with-requirements",
                "-p",
                "--python",
                "--index",
                "--index-url",
                "--extra-index-url",
            ],
            false,
        )),
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash" | "fish" => {
            return shell(program, args);
        }
        "script" => {
            for (at, word) in args.iter().enumerate() {
                if word == "-c" || word == "--command" {
                    return split(program, args.get(at + 1).map(String::as_str), &[]);
                }
                if let Some(line) = word.strip_prefix("--command=") {
                    return split(program, Some(line), &[]);
                }
            }
            None
        }
        // Without -x, watch runs its words joined, through a shell.
        "watch" => {
            let at = skip(args, &["-n", "--interval", "-q", "--equexit"], false);
            if args[..at].iter().any(|w| w == "-x" || w == "--exec") {
                from(at)
            } else if at < args.len() {
                return split(program, Some(&args[at..].join(" ")), &[]);
            } else {
                None
            }
        }
        _ => None,
    })
}

/// Where the program a wrapper runs starts: after its options (those in `valued` take the next
/// word) and, when `assignments`, its `NAME=value` words.
fn skip(args: &[String], valued: &[&str], assignments: bool) -> usize {
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        if word == "--" {
            return at + 1;
        }
        if valued.contains(&word) {
            at += 2;
        } else if (word.starts_with('-') && word.len() > 1) || (assignments && assignment(word)) {
            at += 1;
        } else {
            return at;
        }
    }
    at
}

fn env(args: &[String]) -> Result<Option<Vec<String>>, String> {
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        if word == "--" {
            at += 1;
            break;
        }
        if word == "-S" || word == "--split-string" {
            return split(
                "env",
                args.get(at + 1).map(String::as_str),
                &args[(at + 2).min(args.len())..],
            );
        }
        if let Some(line) = word
            .strip_prefix("--split-string=")
            .or_else(|| word.strip_prefix("-S").filter(|_| !word.starts_with("--")))
        {
            return split("env", Some(line), &args[at + 1..]);
        }
        if ["-u", "--unset", "-C", "--chdir"].contains(&word) {
            at += 2;
        } else if word.starts_with('-') || assignment(word) {
            at += 1;
        } else {
            break;
        }
    }
    while args.get(at).is_some_and(|word| assignment(word)) {
        at += 1;
    }
    Ok(Some(args.get(at..).unwrap_or_default().to_vec()).filter(|rest| !rest.is_empty()))
}

/// The line a shell runs with `-c` (`--command` for fish); `None` for a shell running a script.
fn shell(program: &str, args: &[String]) -> Result<Option<Vec<String>>, String> {
    let mut command = false;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        if let Some(line) = word.strip_prefix("--command=") {
            return split(program, Some(line), &[]);
        }
        if word == "--command" {
            return split(program, args.get(at + 1).map(String::as_str), &[]);
        }
        if ["-o", "+o", "-O", "+O", "--rcfile", "--init-file"].contains(&word) {
            at += 2;
            continue;
        }
        if word.len() > 1 && (word.starts_with('-') || word.starts_with('+')) {
            command |= !word.starts_with("--") && word[1..].contains('c');
            at += 1;
            continue;
        }
        return if command {
            split(program, Some(word), &[])
        } else {
            Ok(None)
        };
    }
    if command {
        return Err(format!("refused: {program} -c names no line to run"));
    }
    Ok(None)
}

/// The words of a line a wrapper runs, then `after`; a line that cannot be split without a shell
/// is refused, since the gate cannot tell what it would run.
fn split(
    program: &str,
    line: Option<&str>,
    after: &[String],
) -> Result<Option<Vec<String>>, String> {
    let Some(line) = line else {
        return Ok(None);
    };
    let mut words = flottille_core::command::split_line(line).map_err(|why| {
        format!(
            "refused: Flottille reads the line that {program} would run, and it cannot be split \
             without a shell: {why}"
        )
    })?;
    words.extend_from_slice(after);
    Ok(Some(words))
}

/// The paths a run's words name: every word of every layer that can name one, resolved from the
/// folder it runs in (its program is wherever it is installed), but the run's own streams; each
/// with the word that names it, and whether only an option's value or a text's word names it.
fn named<'a>(call: &Call, layers: &'a [Vec<String>]) -> Vec<(&'a str, Resolved, bool)> {
    let mut named: Vec<(&str, Resolved, bool)> = Vec::new();
    let Some(folder) = call.paths.first().filter(|_| call.tool == "run") else {
        return named;
    };
    for layer in layers {
        let text = TEXT.contains(&base(&layer[0]));
        for whole in &layer[1..] {
            let soft = text || whole.starts_with('-');
            for word in named_paths(whole)
                .into_iter()
                .filter(|word| !STREAMS.contains(word))
            {
                let path = call.grant.place.resolve(&folder.path, word);
                match named.iter_mut().find(|(_, named, _)| *named == path) {
                    Some((_, _, was)) => *was &= soft,
                    None => named.push((word, path, soft)),
                }
            }
        }
    }
    named
}

/// A path that could not be followed, taken as written: `..` on the text, as a program that
/// never asks the kernel reaches it.
fn as_written(path: &Resolved) -> Resolved {
    Resolved {
        path: path.written.clone(),
        unfollowed: None,
        ..path.clone()
    }
}

/// What names the run's own streams, or nothing: harmless words of a run. Another of its open
/// files (`/dev/fd/3`) may be anything.
const STREAMS: [&str; 7] = [
    "/dev/stdin",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/null",
    "/dev/fd/0",
    "/dev/fd/1",
    "/dev/fd/2",
];

/// The refusal of a call that names a folder that does not exist and then goes up out of it: the
/// kernel would fail it (`ENOENT`), and what follows cannot be followed.
pub fn malformed(paths: &[Resolved]) -> Option<String> {
    let path = paths
        .iter()
        .find(|path| path.up_from_missing || path.unfollowed == Some(Unfollowed::UpFromMissing))?;
    Some(format!(
        "refused: {} {}: name the path without going through a folder that is not there",
        path.shown,
        Unfollowed::UpFromMissing
    ))
}

/// What a word of a line can name as a path: itself, an option's value after `=`, an
/// assignment's value. Short options may hold their value attached (`-Dt<folder>`,
/// `-o<file>`): what follows each of their letters may be one.
fn named_paths(word: &str) -> Vec<&str> {
    let value = word.split_once('=').map(|(_, value)| value);
    if word.starts_with('-') {
        let mut named: Vec<&str> = value.into_iter().collect();
        if let Some(shorts) = word.strip_prefix('-').filter(|w| !w.starts_with('-')) {
            named.extend(
                shorts
                    .char_indices()
                    .take_while(|(_, c)| c.is_ascii_alphanumeric())
                    .map(|(at, c)| &shorts[at + c.len_utf8()..])
                    .filter(|rest| !rest.is_empty()),
            );
        }
        named.retain(|v| !v.is_empty());
        return named;
    }
    if assignment(word) {
        return value.into_iter().filter(|v| !v.is_empty()).collect();
    }
    Some(word).into_iter().filter(|w| !w.is_empty()).collect()
}

/// Whether a path is in a `.git` folder, or is one: what sits below the place when it is
/// inside, the whole path when it is not.
fn in_git(path: &Resolved) -> bool {
    let parts = if path.inside {
        Path::new(&path.shown)
    } else {
        path.path.as_path()
    };
    parts.components().any(|part| part.as_os_str() == ".git")
}

/// Why a recursive deletion would take a repository's `.git` with it: its target is the place's
/// folder, a repository's, or a folder above one. The words of a line that name a `.git` are
/// base already; this is the deletion that names only the folder holding it.
fn deletion(call: &Call, layers: &[Vec<String>]) -> Option<String> {
    let folder = call.paths.first()?;
    let place = &call.grant.place;
    let mut guarded = vec![place.root.clone()];
    guarded.extend(place.repositories.iter().map(|(_, folder)| folder.clone()));
    let real: Vec<PathBuf> = guarded
        .iter()
        .filter_map(|folder| std::fs::canonicalize(folder).ok())
        .collect();
    guarded.extend(real);
    for (_, layer) in layers.iter().flat_map(|layer| suffixes(layer)) {
        for named in deleted(layer) {
            let target = place.resolve(&folder.path, named);
            if guarded
                .iter()
                .any(|folder| folder.starts_with(&target.path))
            {
                return Some(format!(
                    "{} would delete {}, which is the place's folder, a repository's or a folder \
                     above one, and with it a .git folder",
                    shown_words(layer),
                    target.shown
                ));
            }
        }
    }
    None
}

/// What a recursive deletion of a layer names: the operands of `rm -r`, the folders `find` walks
/// when it deletes (`-delete`, or `-exec rm -r`). Nothing for any other program.
fn deleted(layer: &[String]) -> Vec<&str> {
    let args = &layer[1..];
    match base(&layer[0]) {
        "rm" if recursive(args) => {
            let mut options = true;
            let mut operands = Vec::new();
            for word in args {
                if options && word == "--" {
                    options = false;
                } else if !options || !word.starts_with('-') {
                    operands.push(word.as_str());
                }
            }
            operands
        }
        "find" if find_deletes(args) => {
            let mut points: Vec<&str> = args
                .iter()
                .map(String::as_str)
                .skip_while(|w| ["-H", "-L", "-P"].contains(w))
                .take_while(|w| !w.starts_with('-') && !["(", "!", ")"].contains(w))
                .collect();
            if points.is_empty() {
                points.push(".");
            }
            points
        }
        _ => Vec::new(),
    }
}

/// Whether the options of an `rm` ask for a recursive deletion.
fn recursive(args: &[String]) -> bool {
    args.iter()
        .take_while(|word| *word != "--")
        .any(|word| match word.strip_prefix("--") {
            Some(long) => long == "recursive",
            None => word.starts_with('-') && word.contains(['r', 'R']),
        })
}

/// Whether a `find` deletes what it walks: `-delete`, or an `-exec` of a recursive `rm`.
fn find_deletes(args: &[String]) -> bool {
    args.iter().any(|word| word == "-delete")
        || args.iter().enumerate().any(|(at, word)| {
            ["-exec", "-execdir", "-ok", "-okdir"].contains(&word.as_str()) && {
                let command: Vec<String> = args[at + 1..]
                    .iter()
                    .take_while(|w| *w != ";" && *w != "+")
                    .cloned()
                    .collect();
                command.first().is_some_and(|program| base(program) == "rm")
                    && recursive(&command[1..])
            }
        })
}

/// Whether `entry` is the start of `words` at some position; its first word is a program, which
/// matches whatever folder it is named in.
fn starts_anywhere(words: &[String], entry: &[String]) -> bool {
    (0..words.len()).any(|at| {
        words.len() - at >= entry.len()
            && entry
                .iter()
                .zip(&words[at..])
                .enumerate()
                .all(|(i, (e, w))| e == w || (i == 0 && base(w) == e))
    })
}

/// Whether `git symbolic-ref` with these words points a ref somewhere (which switches the branch
/// when the ref is `HEAD`) or deletes one: it reads with a single ref.
fn symbolic_ref_writes(rest: &[String]) -> bool {
    let mut refs = 0;
    let mut options = true;
    for word in rest {
        let word = word.as_str();
        if options && word == "--" {
            options = false;
        } else if options && word.starts_with("--") {
            if word == "--delete" {
                return true;
            }
        } else if options && word.starts_with('-') {
            // `-m` only comes with a write.
            if word.contains(['d', 'm']) {
                return true;
            }
        } else {
            refs += 1;
        }
    }
    refs > 1
}

/// Whether `git remote` with these words only reads: it lists, shows or gives a URL. Anything else
/// is taken for a write: a push and `forge`'s writes trust where the remote leads.
fn remote_reads(rest: &[String]) -> bool {
    match rest
        .iter()
        .find(|w| !["-v", "--verbose"].contains(&w.as_str()))
    {
        None => true,
        Some(word) => ["show", "get-url"].contains(&word.as_str()),
    }
}

/// Whether `git replace` with these words only lists.
fn replace_reads(rest: &[String]) -> bool {
    let any = |names: &[&str]| rest.iter().any(|w| names.contains(&w.as_str()));
    rest.is_empty()
        || any(&["-l", "--list"])
            && !any(&[
                "-d",
                "--delete",
                "-f",
                "--force",
                "-e",
                "--edit",
                "--graft",
                "--convert-graft-file",
            ])
}

/// Whether `git notes` with these words only reads: no subcommand (it lists), or one that reads.
/// An option it does not know is taken for a write.
fn notes_reads(rest: &[String]) -> bool {
    let mut at = 0;
    while let Some(word) = rest.get(at) {
        let word = word.as_str();
        if word == "--ref" {
            at += 2;
        } else if word.starts_with("--ref=") {
            at += 1;
        } else if word.starts_with('-') {
            return false;
        } else {
            return ["list", "show", "get-ref"].contains(&word);
        }
    }
    true
}

/// Whether `git config` with these words only reads.
fn config_reads(rest: &[String]) -> bool {
    let any = |names: &[&str]| rest.iter().any(|w| names.contains(&w.as_str()));
    let reads = rest.first().is_some_and(|w| w == "get" || w == "list")
        || any(&["--get", "--get-all", "--get-regexp", "--list", "-l"]);
    let writes = any(&[
        "--add",
        "--unset",
        "--unset-all",
        "--replace-all",
        "--rename-section",
        "--remove-section",
        "-e",
        "--edit",
    ]);
    reads && !writes
}

/// Whether a forge's `api` call only reads: GET, no field, no input, not GraphQL.
fn api_reads(args: &[String]) -> bool {
    let get = |method: Option<&str>| method.is_some_and(|m| m.eq_ignore_ascii_case("GET"));
    let mut positional = Vec::new();
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        at += 1;
        match word {
            "-X" | "--method" => {
                if !get(args.get(at).map(String::as_str)) {
                    return false;
                }
                at += 1;
            }
            "-f" | "-F" | "--field" | "--raw-field" | "--input" => return false,
            "-H" | "--header" | "-R" | "--repo" | "--hostname" | "-q" | "--jq" | "-t"
            | "--template" | "--cache" | "-p" | "--preview" => at += 1,
            _ if word.starts_with("--method=") => {
                if !get(word.strip_prefix("--method=")) {
                    return false;
                }
            }
            _ if word.starts_with("-X") => {
                if !get(word.strip_prefix("-X")) {
                    return false;
                }
            }
            _ if word.starts_with("--field=")
                || word.starts_with("--raw-field=")
                || word.starts_with("--input=")
                || word.starts_with("-f")
                || word.starts_with("-F") =>
            {
                return false;
            }
            _ if word.starts_with('-') => {}
            _ => positional.push(word),
        }
    }
    // `api`, then its endpoint.
    positional
        .get(1)
        .is_some_and(|endpoint| *endpoint != "graphql")
}

/// Why a git command discards the work of the user's own checkout, or moves the branch it has
/// checked out, when it does: in the project folder, that is base.
fn discards_work(call: &Call, layer: &[String], command: &str, rest: &[String]) -> Option<String> {
    let any = |names: &[&str]| rest.iter().any(|word| names.contains(&word.as_str()));
    let short = |letters: &[char]| shorts(rest).any(|word| word[1..].contains(letters));
    let first = rest.first().map(String::as_str);
    let discards = match command {
        "checkout" => {
            any(&[
                "--",
                ".",
                "--force",
                "--ours",
                "--theirs",
                "--patch",
                "--pathspec-from-file",
            ]) || short(&['f', 'B', 'p'])
                || checks_out_paths(call, rest)
        }
        "switch" => any(&["--force", "--discard-changes", "--force-create"]) || short(&['f', 'C']),
        "checkout-index" => any(&["--force"]) || short(&['f']),
        "read-tree" => short(&['u']),
        "restore" | "clean" | "reset" | "update-ref" => true,
        "stash" => !matches!(first, Some("list" | "show")),
        "worktree" => first == Some("remove"),
        "branch" => {
            any(&["--delete", "--force", "--move"]) || short(&['d', 'D', 'f', 'M', 'm', 'C'])
        }
        "rm" => !any(&["--cached"]),
        "apply" | "am" if any(&["--reject"]) => !any(&["--cached", "--check"]),
        "merge" | "cherry-pick" | "revert" | "am" => any(&["--abort"]),
        "symbolic-ref" => symbolic_ref_writes(rest),
        "submodule" => {
            matches!(first, Some("update" | "deinit")) && (any(&["--force"]) || short(&['f']))
        }
        _ => false,
    };
    discards.then(|| {
        format!(
            "{} discards the work in the user's own checkout (main), or moves the branch it has \
             checked out",
            shown_words(layer)
        )
    })
}

/// Whether a `git checkout` without `--` names paths, which it then overwrites from the index or
/// a commit: a word that is a file or a folder of the place, a pattern or a pathspec magic. A new
/// branch (`-b`, `--orphan`) carries the work along.
fn checks_out_paths(call: &Call, rest: &[String]) -> bool {
    if rest
        .iter()
        .any(|word| word == "-b" || word == "--orphan" || word.starts_with("--orphan="))
    {
        return false;
    }
    let folder = call.paths.first().map(|folder| folder.path.as_path());
    rest.iter()
        .filter(|word| !word.starts_with('-'))
        .any(|word| {
            word.contains(['*', '?', '['])
                || word.starts_with(':')
                || folder.is_some_and(|folder| folder.join(word).exists())
        })
}

/// The sensitive place a path is in, if it is in one: inside the place, only what sits below it
/// counts.
pub fn sensitive(own: &[PathBuf], path: &Resolved) -> Option<String> {
    if path.inside {
        return sensitive_names(Path::new(&path.shown));
    }
    // A path outside is taken as written and where it leads (`path.path`, every link and `..`
    // followed): a link anyone made, `/proc/self/root`, a configuration folder that is a link.
    let own = own_folders(own);
    for path in [&path.written, &path.path] {
        if own.iter().any(|own| path.starts_with(own)) {
            return Some("Flottille's own folder".into());
        }
        if let Some(system) = SYSTEM_SENSITIVE.iter().find(|s| path.starts_with(s)) {
            return Some((*system).into());
        }
    }
    sensitive_names(&path.written).or_else(|| sensitive_names(&path.path))
}

/// Why a call that reaches a path is refused in every mode, Full included (`docs/design.md`
/// §7.1): a login read or written, Flottille's own folders or a startup file written. `own` names
/// the data folder first.
pub fn forbidden(
    own: &[PathBuf],
    home: Option<&Path>,
    path: &Resolved,
    write: bool,
) -> Option<String> {
    if let Some(why) = credential(path) {
        return Some(why);
    }
    if !write {
        return None;
    }
    // Where a process's folder in `/proc` leads is anywhere on the disk: the daemon's own runs in
    // `/`.
    if let Some(Unfollowed::Through(process)) = &path.unfollowed {
        return Some(format!(
            "refused: {} goes through {}, which leads anywhere on the disk, Flottille's own \
             folders and the user's startup files included: an agent never writes through it, in \
             any mode; nothing was done",
            path.shown,
            process.display()
        ));
    }
    if let Ok(program) = std::env::current_exe() {
        let program = [place::real_path(&program).ok(), Some(program)];
        if seen(path)
            .iter()
            .any(|path| program.iter().flatten().any(|program| program == path))
        {
            return Some(format!(
                "refused: {} is Flottille's own program: an agent never writes it, in any mode; \
                 nothing was done",
                path.shown
            ));
        }
    }
    // A project's workspaces may sit in Flottille's data folder, the first of its own: they are
    // the project's, not Flottille's.
    let folders = own_folders(own);
    let data = own_folders(&own[..own.len().min(1)]);
    let flottille_s = |path: &Path| {
        folders.iter().any(|own| path.starts_with(own))
            && !data.iter().any(|data| {
                path.strip_prefix(data.join("workspaces"))
                    .is_ok_and(|rest| rest.components().count() >= 2)
            })
    };
    if seen(path).iter().any(|path| flottille_s(path)) {
        return Some(format!(
            "refused: {} is in Flottille's own folders (its configuration, its data, its state, \
             its socket): an agent never writes there, in any mode; nothing was done",
            path.shown
        ));
    }
    if let Some(why) = above_own(own, path) {
        return Some(format!("refused: {why}; nothing was done"));
    }
    startup(home, path).map(|entry| {
        format!(
            "refused: {} is or holds a startup file ({entry}): what it holds runs at the user's \
             next login or command, or starts a process later, outside the gate: an agent never \
             writes there, in any mode; nothing was done",
            path.shown
        )
    })
}

/// A path as written and where it leads, each also without a leading `/proc/<process>/root`
/// (or `/proc/<process>/task/<thread>/root`), which is `/` again for a program the kernel never
/// asks (a path taken as written), or `cwd`, which is `/` for the daemon.
fn seen(path: &Resolved) -> Vec<PathBuf> {
    let mut seen = vec![path.written.clone(), path.path.clone()];
    for path in [&path.written, &path.path] {
        let parts: Vec<&std::ffi::OsStr> = path.iter().collect();
        if parts.get(1).is_none_or(|part| *part != "proc") {
            continue;
        }
        let root = if parts.get(3).is_some_and(|part| *part == "task") {
            5
        } else {
            3
        };
        if parts
            .get(root)
            .is_some_and(|part| *part == "root" || *part == "cwd")
        {
            seen.push(Path::new("/").join(parts[root + 1..].iter().collect::<PathBuf>()));
        }
    }
    seen
}

/// The refusal of a path in a login's place: an agent's, the user's, the system's secrets.
fn credential(path: &Resolved) -> Option<String> {
    let place = if path.inside {
        credential_names(Path::new(&path.shown))
    } else {
        let seen = seen(path);
        seen.iter()
            .find_map(|path| SYSTEM_SENSITIVE.iter().find(|s| path.starts_with(s)))
            .map(|system| (*system).to_owned())
            .or_else(|| seen.iter().find_map(|path| credential_names(path)))
    }?;
    Some(format!(
        "refused: {} is in a login's place ({place}): an agent never reads nor writes a login, \
         in any mode; nothing was done",
        path.shown
    ))
}

/// The startup file a path is, is in or holds, if any: from the home folder, and from the user's
/// configuration folder.
fn startup(home: Option<&Path>, path: &Resolved) -> Option<String> {
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    for home in home
        .into_iter()
        .flat_map(|home| [Some(home.to_owned()), place::real_path(home).ok()])
        .flatten()
    {
        files.extend(
            STARTUP
                .iter()
                .map(|entry| (home.join(entry), format!("~/{entry}"))),
        );
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME").filter(|c| !c.is_empty()) {
        let config = PathBuf::from(config);
        files.extend(
            CONFIG_STARTUP
                .iter()
                .map(|entry| (config.join(entry), config.join(entry).display().to_string())),
        );
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        let units = PathBuf::from(data).join("systemd/user");
        files.push((units.clone(), units.display().to_string()));
    }
    seen(path).iter().find_map(|path| {
        files
            .iter()
            .find(|(file, _)| path.starts_with(file) || file.starts_with(path))
            .map(|(_, entry)| entry.clone())
    })
}

/// Flottille's own folders, as named and as they are on the disk.
fn own_folders(own: &[PathBuf]) -> Vec<PathBuf> {
    own.iter()
        .flat_map(|own| [Some(own.clone()), place::real_path(own).ok()])
        .flatten()
        .collect()
}

/// Why writing over, moving or removing a path would take Flottille's own folders away: it is one
/// of them, or a folder above one.
fn above_own(own: &[PathBuf], path: &Resolved) -> Option<String> {
    let own = own_folders(own);
    [&path.written, &path.path]
        .iter()
        .any(|path| own.iter().any(|own| own.starts_with(path)))
        .then(|| {
            format!(
                "{} holds Flottille's own folders (its configuration, its data): writing over, \
                 moving or removing it takes them away",
                path.shown
            )
        })
}

/// The sensitive place a path names along its parts, if any.
fn sensitive_names(path: &Path) -> Option<String> {
    let parts = parts_of(path);
    (0..parts.len()).find_map(|at| {
        let part = &parts[at];
        if part == ".env"
            || (part.starts_with(".env.") && !TEMPLATES.iter().any(|t| part.ends_with(t)))
        {
            return Some(".env".into());
        }
        credential_at(&parts, at)
    })
}

/// The login's place a path names along its parts, if any.
fn credential_names(path: &Path) -> Option<String> {
    let parts = parts_of(path);
    (0..parts.len()).find_map(|at| credential_at(&parts, at))
}

fn parts_of(path: &Path) -> Vec<String> {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect()
}

/// The login's place the parts name from `at`, if any.
fn credential_at(parts: &[String], at: usize) -> Option<String> {
    SENSITIVE.iter().find_map(|place| {
        let names: Vec<&str> = place.split('/').collect();
        (parts.len() - at >= names.len() && names.iter().zip(&parts[at..]).all(|(n, p)| n == p))
            .then(|| format!("~/{place}"))
    })
}

/// Whether the paths are inside the place. This is the place's own invariant, not a policy: the
/// tools check it again on the paths they resolve themselves, unless the user allowed the call.
pub fn contained(paths: &[Resolved]) -> Verdict {
    match paths.iter().find(|path| !path.inside) {
        Some(path) => Verdict::Deny(format!(
            "refused: {} is outside this agent's place: its tools reach only the files of the \
             folder it works in",
            path.shown
        )),
        None => Verdict::Allow,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::place::Place;

    /// A workspace `w` (or the project folder) of the project `app` in a temporary folder, with a
    /// home beside it and Flottille's own folders under `xdg`.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        own: Vec<PathBuf>,
        grant: Grant,
        never: Vec<Vec<String>>,
        commands: BTreeMap<String, Task>,
        pushed: bool,
        /// Where the system's programs are: a folder beside the project, every known program in it.
        search: std::ffi::OsString,
        groups: Vec<u32>,
        /// Where a run runs, relative to the place.
        folder: String,
    }

    impl Fixture {
        fn new(workspace: Option<&str>) -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let base = std::fs::canonicalize(dir.path()).unwrap();
            let root = base.join("work/app");
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::create_dir_all(base.join("home")).unwrap();
            let bin = base.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            for program in READS.iter().chain(&["git", "gh", "glab", "less", "more"]) {
                installed(&bin.join(program));
            }
            let grant = Grant {
                agent: "a1".into(),
                epoch: 0,
                tools: vec![
                    "fs_read",
                    "fs_list",
                    "search",
                    "fs_write",
                    "fs_edit",
                    "tasks",
                    "run",
                    "run_status",
                ],
                place: Place {
                    project: "app".into(),
                    workspace: workspace.map(str::to_owned),
                    root: root.clone(),
                    repositories: vec![(".".into(), root.clone())],
                    branch: workspace.map(|_| "app/login".into()).unwrap_or_default(),
                    variables: BTreeMap::new(),
                    home: Some(base.join("home")),
                },
                launched: false,
            };
            Fixture {
                own: vec![base.join("xdg/data/flottille")],
                _dir: dir,
                root,
                grant,
                never: vec![
                    vec!["rm".into(), "-rf".into()],
                    vec!["docker".into(), "compose".into(), "down".into()],
                    vec!["deploy".into()],
                ],
                commands: BTreeMap::new(),
                pushed: false,
                search: bin.into_os_string(),
                groups: Vec::new(),
                folder: ".".into(),
            }
        }

        fn workspace() -> Fixture {
            Fixture::new(Some("w"))
        }

        fn main() -> Fixture {
            Fixture::new(None)
        }

        fn pushed(mut self) -> Fixture {
            self.pushed = true;
            self
        }

        fn command(mut self, name: &str, line: &str, set: impl Fn(&mut Task)) -> Self {
            let mut command = Task {
                line: line.into(),
                repository: None,
                serve: false,
                check: false,
                read_only: false,
                ask_before_running: false,
                added_by: None,
            };
            set(&mut command);
            self.commands.insert(name.into(), command);
            self
        }

        fn judge(&self, tool: &str, args: Value, never: Option<&[Vec<String>]>) -> Verdict {
            let place = &self.grant.place;
            let (words, paths, command) = match tool {
                "run" => {
                    let (line, command) = match args["task"].as_str() {
                        Some(name) => {
                            let (name, command) = self.commands.get_key_value(name).unwrap();
                            (command.line.clone(), Some(Ok((name.as_str(), command))))
                        }
                        None => (args["line"].as_str().unwrap().to_owned(), None),
                    };
                    let words = flottille_core::command::split_line(&line).unwrap_or_default();
                    (
                        words,
                        vec![place.resolve(&self.root, &self.folder)],
                        command,
                    )
                }
                // As `mcp::classify` resolves what a write sets.
                _ => match crate::actions::sets(&self.commands, tool, &args) {
                    Some(sets) => (
                        flottille_core::command::split_line(&sets.task.line).unwrap_or_default(),
                        crate::actions::folders(place, &sets),
                        sets.name.map(|name| Err((name, sets.task))),
                    ),
                    None => match args["path"].as_str() {
                        Some(path) => (Vec::new(), vec![place.resolve(&self.root, path)], None),
                        None => (Vec::new(), Vec::new(), None),
                    },
                },
            };
            let owned;
            let command = match command {
                Some(Ok(command)) => Some(command),
                Some(Err((name, task))) => {
                    owned = (name, task);
                    Some((owned.0.as_str(), &owned.1))
                }
                None => None,
            };
            decide(&Call {
                grant: &self.grant,
                tool,
                args: &args,
                words: &words,
                paths: &paths,
                command,
                tasks: &self.commands,
                never,
                own: &self.own,
                pushed: self.pushed,
                search: Some(&self.search),
                writable: std::slice::from_ref(&self.root),
                groups: &self.groups,
            })
        }

        fn run(&self, line: &str) -> Verdict {
            self.judge("run", json!({"line": line}), Some(&self.never))
        }

        fn task(&self, name: &str) -> Verdict {
            self.judge("run", json!({"task": name}), Some(&self.never))
        }

        fn file(&self, tool: &str, path: &str) -> Verdict {
            let args = match tool {
                "fs_write" => json!({"path": path, "content": "x"}),
                "fs_edit" => json!({"path": path, "edits": [{"old": "a", "new": "b"}]}),
                _ => json!({"path": path}),
            };
            self.judge(tool, args, Some(&self.never))
        }
    }

    /// An executable file.
    fn installed(path: &Path) {
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }

    fn reasons(verdict: &Verdict) -> String {
        match verdict {
            Verdict::Ask(ask) => ask.reasons.join(" | "),
            other => format!("{other:?}"),
        }
    }

    fn denied(verdict: &Verdict) -> bool {
        matches!(verdict, Verdict::Deny(why) if !why.is_empty() && why.chars().count() < 2_000)
    }

    fn to_judge(verdict: &Verdict) -> bool {
        matches!(verdict, Verdict::Ask(ask) if ask.class == Class::Judge && !ask.reasons.is_empty())
    }

    fn base_class(verdict: &Verdict) -> bool {
        matches!(verdict, Verdict::Ask(ask) if ask.class == Class::Base && !ask.reasons.is_empty())
    }

    #[test]
    fn harmless_reading_inside_the_project_passes_in_main_and_in_a_workspace() {
        for fixture in [Fixture::main(), Fixture::workspace()] {
            for tool in ["fs_read", "fs_list", "search"] {
                assert_eq!(fixture.file(tool, "src/a.rs"), Verdict::Allow, "{tool}");
            }
            for tool in ["run_status", "agent_status"] {
                assert_eq!(
                    fixture.judge(tool, json!({}), Some(&fixture.never)),
                    Verdict::Allow
                );
            }
            for (tool, action) in [
                ("forge", "list"),
                ("forge", "show"),
                ("forge", "subscribe"),
                ("forge", "unsubscribe"),
                ("tickets", "list"),
                ("tickets", "search"),
                ("tickets", "show"),
                ("workspace", "list"),
            ] {
                assert_eq!(
                    fixture.judge(tool, json!({"action": action}), Some(&fixture.never)),
                    Verdict::Allow,
                    "{tool} {action}"
                );
            }
        }
    }

    #[test]
    fn a_push_is_found_past_git_s_options_and_its_wrappers() {
        let words = |line: &str| flottille_core::command::split_line(line).unwrap();
        for line in [
            "git push",
            "git -C app push origin x",
            "env A=b git push",
            "nice git push",
        ] {
            assert!(pushes(&words(line)), "{line}");
        }
        for line in ["git status", "git -C push status", "echo git push"] {
            assert!(!pushes(&words(line)), "{line}");
        }
    }

    #[test]
    fn harmless_writing_in_a_workspace_passes() {
        let fixture = Fixture::workspace();
        assert_eq!(fixture.file("fs_write", "src/a.rs"), Verdict::Allow);
        assert_eq!(fixture.file("fs_edit", "src/a.rs"), Verdict::Allow);
        for line in ["git add src", "git mv a b", "git rm src/a.rs"] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn harmless_known_read_only_commands_pass_in_main_and_in_a_workspace() {
        for fixture in [Fixture::main(), Fixture::workspace()] {
            for line in [
                "git status",
                "git log --oneline",
                "git diff HEAD",
                "git -C src status",
                "git --no-pager show HEAD",
                "git branch",
                "git branch -a",
                "git tag -l",
                "git config --get user.name",
                "git remote -v",
                "git stash list",
                "git symbolic-ref --short HEAD",
                "ls -la src",
                "pgrep -a cargo",
                "ps aux",
                "cat src/a.rs",
                "grep -rn needle src",
                "find src -name '*.rs'",
                "gh pr view 1",
                "wc -l src/a.rs",
                "date -Iseconds",
            ] {
                assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
            }
        }
    }

    #[test]
    fn harmless_a_task_in_a_workspace_passes() {
        let fixture = Fixture::workspace().command("test", "cargo test", |_| {});
        assert_eq!(fixture.task("test"), Verdict::Allow);
    }

    #[test]
    fn to_judge_writing_in_main_is_judged() {
        let fixture = Fixture::main();
        assert!(to_judge(&fixture.file("fs_write", "src/a.rs")));
        assert!(to_judge(&fixture.file("fs_edit", "src/a.rs")));
        let fixture = fixture.command("test", "cargo test", |_| {});
        assert!(to_judge(&fixture.task("test")));
    }

    #[test]
    fn to_judge_what_the_rules_do_not_know_is_judged() {
        for fixture in [Fixture::main(), Fixture::workspace()] {
            for line in [
                "git commit -m change",
                "git push origin app/login",
                "git push",
                "git merge topic",
                "git fetch origin",
                "git checkout main",
                "git config user.name someone",
                "git -c color.ui=never log",
                "git --git-dir=other log",
                "cargo build",
                "npm run dev",
                "curl https://example.com",
                "./script.sh",
                "sed -i s/a/b/ src/a.rs",
                "gh pr create --fill",
                "env ls",
            ] {
                assert!(
                    to_judge(&fixture.run(line)),
                    "{line}: {:?}",
                    fixture.run(line)
                );
            }
        }
    }

    #[test]
    fn to_judge_reading_outside_the_project_is_judged() {
        let fixture = Fixture::workspace();
        assert!(to_judge(&fixture.file("fs_read", "../../home/notes.txt")));
        assert!(to_judge(&fixture.run("cat ../../home/notes.txt")));
    }

    #[test]
    fn to_judge_a_marked_or_shell_task_is_judged_even_in_a_workspace() {
        let fixture = Fixture::workspace()
            .command("deploy-check", "cargo check", |c| {
                c.ask_before_running = true
            })
            .command("script", "sh run.sh", |_| {});
        assert!(to_judge(&fixture.task("deploy-check")));
        assert!(to_judge(&fixture.task("script")));
    }

    #[test]
    fn to_judge_forge_writes_are_judged() {
        let fixture = Fixture::workspace();
        for action in ["create", "comment", "edit", "merge", "checks"] {
            let verdict = fixture.judge(
                "forge",
                json!({"action": action, "repository": ".", "number": 1}),
                Some(&fixture.never),
            );
            assert!(to_judge(&verdict), "{action}");
        }
    }

    #[test]
    fn to_judge_ticket_writes_are_judged_and_an_unknown_action_refused() {
        let fixture = Fixture::main();
        for action in ["create", "comment", "edit", "close", "reopen"] {
            let verdict = fixture.judge(
                "tickets",
                json!({"action": action, "ticket": "#1"}),
                Some(&fixture.never),
            );
            assert!(to_judge(&verdict), "{action}");
        }
        assert!(denied(&fixture.judge(
            "tickets",
            json!({"action": "delete"}),
            Some(&fixture.never)
        )));
    }

    #[test]
    fn a_jira_write_is_classed_with_the_github_ticket_writes() {
        // Full and Auto act on the class alone: the same class, the same treatment.
        let fixture = Fixture::main();
        for (action, extra) in [
            ("create", json!({"title": "T"})),
            ("comment", json!({"body": "b"})),
            ("edit", json!({"title": "T"})),
            ("close", json!({})),
            ("reopen", json!({})),
            ("status", json!({"status": "In Progress"})),
        ] {
            let verdict = |ticket: &str| {
                let mut args = json!({"action": action, "ticket": ticket});
                args.as_object_mut()
                    .unwrap()
                    .extend(extra.as_object().unwrap().clone());
                fixture.judge("tickets", args, Some(&fixture.never))
            };
            let jira = verdict("ABC-1");
            assert!(to_judge(&jira), "{action}: {jira:?}");
            assert_eq!(jira, verdict("#1"), "{action}");
        }
        assert!(denied(&fixture.judge(
            "tickets",
            json!({"action": "status", "ticket": "ABC-1", "status": "Done", "body": "b"}),
            Some(&fixture.never)
        )));
    }

    #[test]
    fn to_judge_a_session_started_for_the_user_is_judged_as_an_action() {
        let fixture = Fixture::main();
        let verdict = fixture.judge(
            "agent_launch",
            json!({"title": "t", "brief": "b", "root": true}),
            Some(&fixture.never),
        );
        assert!(to_judge(&verdict), "{verdict:?}");
        let Verdict::Ask(ask) = verdict else {
            unreachable!()
        };
        assert_eq!(ask.kind, Kind::Action);
        assert_eq!(ask.reasons, [ROOT_LAUNCH]);
        // A child launched is the caller's own work, as before.
        let child = fixture.judge(
            "agent_launch",
            json!({"title": "t", "brief": "b"}),
            Some(&fixture.never),
        );
        assert_eq!(child, Verdict::Allow);
    }

    #[test]
    fn base_a_session_started_by_a_root_an_agent_started_comes_to_the_user() {
        let mut fixture = Fixture::main();
        fixture.grant.launched = true;
        let verdict = fixture.judge(
            "agent_launch",
            json!({"title": "t", "brief": "b", "root": true}),
            Some(&fixture.never),
        );
        assert!(base_class(&verdict), "{verdict:?}");
    }

    #[test]
    fn base_a_write_or_a_session_whose_text_holds_a_secret_and_refused_one_too_long() {
        let fixture = Fixture::main();
        let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let long = "x".repeat(crate::judge::WHOLE_CHARS + 1);
        for (tool, args) in [
            (
                "tickets",
                json!({"action": "comment", "ticket": "#1", "body": token}),
            ),
            ("forge", json!({"action": "create", "title": token})),
            (
                "agent_launch",
                json!({"title": "t", "brief": token, "root": true}),
            ),
        ] {
            let verdict = fixture.judge(tool, args, Some(&fixture.never));
            assert!(base_class(&verdict), "{tool}: {verdict:?}");
        }
        for (tool, args) in [
            (
                "tickets",
                json!({"action": "create", "title": "t", "body": long}),
            ),
            (
                "forge",
                json!({"action": "edit", "number": 1, "title": long}),
            ),
            (
                "agent_launch",
                json!({"title": "t", "brief": long, "root": true}),
            ),
        ] {
            assert!(
                denied(&fixture.judge(tool, args, Some(&fixture.never))),
                "{tool}"
            );
        }
    }

    #[test]
    fn workspace_remove_is_base_an_action_and_names_the_workspace_it_removes() {
        let fixture = Fixture::main();
        let verdict = fixture.judge(
            "workspace",
            json!({"action": "remove", "name": "login"}),
            Some(&fixture.never),
        );
        assert!(base_class(&verdict), "{verdict:?}");
        let Verdict::Ask(ask) = verdict else {
            unreachable!()
        };
        assert_eq!(ask.kind, Kind::Action);
        assert!(
            ask.reasons[0].starts_with("it removes the workspace login and deletes"),
            "{:?}",
            ask.reasons
        );
    }

    #[test]
    fn to_judge_the_window_actions_of_an_agent_are_judged_and_a_tracker_change_is_base() {
        let fixture = Fixture::main();
        for (tool, write) in [
            (
                "skills",
                json!({"action": "add", "name": "style", "description": "d", "text": "t"}),
            ),
            ("skills", json!({"action": "remove", "name": "style"})),
            (
                "commands",
                json!({"action": "edit", "name": "ship", "instructions": "Push."}),
            ),
        ] {
            let verdict = fixture.judge(tool, write, Some(&fixture.never));
            assert!(to_judge(&verdict), "{tool}: {verdict:?}");
            assert!(matches!(verdict, Verdict::Ask(ask) if ask.kind == Kind::Action));
        }
        let tracker = fixture.judge(
            "project",
            json!({"action": "set_settings", "tracker": null}),
            Some(&fixture.never),
        );
        assert!(base_class(&tracker), "{tracker:?}");
        for tool in ["tasks", "skills", "commands"] {
            for reading in [
                json!({"action": "list"}),
                json!({"action": "show", "name": "ship"}),
            ] {
                assert_eq!(
                    fixture.judge(tool, reading, Some(&fixture.never)),
                    Verdict::Allow
                );
            }
        }
    }

    #[test]
    fn to_judge_the_base_branches_and_dependencies_an_agent_sets_are_judged_not_base() {
        let fixture = Fixture::main();
        for repositories in [
            json!([{"path": ".", "base": ["develop", "main"]}]),
            json!([{"path": ".", "depends_on": []}]),
            json!([{"path": ".", "forge": null, "base": ["main"], "depends_on": ["lib"]}]),
        ] {
            let verdict = fixture.judge(
                "project",
                json!({"action": "set_settings", "repositories": repositories}),
                Some(&fixture.never),
            );
            assert!(to_judge(&verdict), "{repositories}: {verdict:?}");
            assert!(!base_class(&verdict), "{repositories}: {verdict:?}");
        }
    }

    #[test]
    fn a_line_a_task_or_a_setup_step_sets_is_judged_as_a_run_of_it() {
        let fixture = Fixture::workspace().command("build", "cargo build", |_| {});
        let task = |line: &str| json!({"action": "add", "name": "build", "line": line});
        let step = |line: &str| json!({"action": "add", "step": {"run": line}});
        // A harmless line is judged as the write it is.
        let verdict = fixture.judge("tasks", task("cargo build"), Some(&fixture.never));
        assert!(to_judge(&verdict), "{verdict:?}");
        // A line that would be judged to run as a task is judged to set, said as the line it
        // sets.
        let verdict = fixture.judge("tasks", task("sh run.sh"), Some(&fixture.never));
        assert!(to_judge(&verdict), "{verdict:?}");
        assert!(
            reasons(&verdict).contains("the line it sets"),
            "{verdict:?}"
        );
        // A line that would be base to run is base to set, said as the line it sets.
        for (tool, args) in [
            ("tasks", task("sudo make install")),
            ("setup", step("sudo apt-get install jq")),
            ("tasks", task("git push --force")),
            ("setup", step("deploy --prod")),
        ] {
            let verdict = fixture.judge(tool, args, Some(&fixture.never));
            assert!(base_class(&verdict), "{tool}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("the line it sets"),
                "{verdict:?}"
            );
        }
        // What a run of it refuses is refused to set.
        assert!(denied(&fixture.judge(
            "tasks",
            json!({"action": "edit", "name": "build", "line": "rm -rf /"}),
            Some(&fixture.never)
        )));
        // Shell syntax is refused, as a run of it is.
        assert!(denied(&fixture.judge(
            "setup",
            step("npm ci && npm test"),
            Some(&fixture.never)
        )));
        // A step that sets no line is judged for what it does.
        let copy = json!({"action": "add", "step": {"copy": ["dist"]}});
        assert!(to_judge(&fixture.judge(
            "setup",
            copy,
            Some(&fixture.never)
        )));
    }

    #[test]
    fn a_copy_glob_is_sensitive_when_it_matches_a_secret_s_name_but_a_template_is_not() {
        for glob in [
            ".env",
            ".env*",
            ".env.*",
            "config/.env.production",
            "*.pem",
            "certs/*.key",
            "id_*",
            "*",
            ".ssh",
            ".npmrc",
            "home/.netrc",
            ".docker/*.json",
            ".config",
        ] {
            assert!(sensitive_glob(glob).is_some(), "{glob}");
        }
        for glob in [
            ".env.example",
            ".env.sample",
            "README.md",
            "dist",
            "node_modules",
            "*.lock",
            ".config/app.toml",
        ] {
            assert_eq!(sensitive_glob(glob), None, "{glob}");
        }
    }

    #[test]
    fn base_a_forced_push_or_a_deletion_on_the_remote_is_base() {
        for fixture in [Fixture::main(), Fixture::workspace()] {
            for line in [
                "git push --force",
                "git push -f origin app/login",
                "git push --force-with-lease origin app/login",
                "git push origin +app/login",
                "git push origin :app/login",
                "git push --delete origin app/login",
                "git push --mirror",
                "env git push -uf origin x",
            ] {
                assert!(
                    base_class(&fixture.run(line)),
                    "{line}: {:?}",
                    fixture.run(line)
                );
            }
        }
    }

    #[test]
    fn base_a_history_rewrite_of_a_pushed_branch_is_base_and_of_an_unpushed_one_judged() {
        let unpushed = Fixture::workspace();
        let pushed = Fixture::workspace().pushed();
        for line in [
            "git rebase main",
            "git commit --amend -m x",
            "git commit --amen",
            "git commit --fixup=amend:HEAD",
            "git reset HEAD~1",
            "git update-ref refs/heads/app/login abc",
        ] {
            assert!(
                to_judge(&unpushed.run(line)),
                "{line}: {:?}",
                unpushed.run(line)
            );
            assert!(
                base_class(&pushed.run(line)),
                "{line}: {:?}",
                pushed.run(line)
            );
        }
        assert!(base_class(&unpushed.run("git filter-branch --all")));
    }

    #[test]
    fn base_discarding_the_user_s_work_in_main_is_base_and_judged_in_a_workspace() {
        let main = Fixture::main();
        let workspace = Fixture::workspace();
        for line in [
            "git reset --hard",
            "git clean -fd",
            "git checkout -- .",
            "git checkout -f",
            "git restore src",
            "git stash drop",
            "git stash",
            "git worktree remove x",
            "git branch -D topic",
        ] {
            assert!(base_class(&main.run(line)), "{line}: {:?}", main.run(line));
            assert!(
                to_judge(&workspace.run(line)),
                "{line}: {:?}",
                workspace.run(line)
            );
        }
    }

    #[test]
    fn base_the_git_folder_sensitive_places_publishing_and_the_never_list_are_base() {
        let fixture = Fixture::workspace();
        for verdict in [
            fixture.file("fs_write", ".git/hooks/pre-commit"),
            fixture.file("fs_read", ".env"),
            fixture.file("fs_write", ".env.local"),
            fixture.file("fs_read", "../../xdg/data/flottille/store.db"),
            fixture.run("rm -rf .git"),
            fixture.run("rm -r ."),
            fixture.run("cat .git/config"),
            fixture.run("cargo publish"),
            fixture.run("npm publish"),
            fixture.run("gem push x.gem"),
            fixture.run("docker compose down"),
            fixture.run("nice deploy now"),
            fixture.run("bkt pr list"),
            fixture.run("acli jira workitem view ABC-1"),
            fixture.judge(
                "forge",
                json!({"action": "merge", "repository": ".", "waive": "ship it"}),
                Some(&fixture.never),
            ),
        ] {
            assert!(base_class(&verdict), "{verdict:?}");
        }
        assert_eq!(fixture.file("fs_read", ".env.example"), Verdict::Allow);
        // A login's place is refused, in every mode.
        for verdict in [
            fixture.file("fs_read", "../../home/.ssh/id_ed25519"),
            fixture.run("cat ../../home/.aws/credentials"),
        ] {
            assert!(denied(&verdict), "{verdict:?}");
        }
    }

    #[test]
    fn base_a_project_whose_never_list_cannot_be_read_makes_a_run_base() {
        let fixture = Fixture::workspace();
        assert!(base_class(&fixture.judge(
            "run",
            json!({"line": "ls"}),
            None
        )));
    }

    #[test]
    fn a_base_reason_wins_over_a_reason_to_judge() {
        let fixture = Fixture::main();
        let verdict = fixture.run("git push --force origin main");
        assert!(base_class(&verdict));
    }

    #[test]
    fn shell_syntax_a_malformed_call_a_login_printed_and_the_daemon_stay_refused() {
        let fixture = Fixture::workspace();
        for verdict in [
            fixture.run("ls | wc"),
            fixture.run("echo $HOME"),
            fixture.run("sh -c 'ls | wc'"),
            fixture.run("gh auth token"),
            fixture.run("gh auth status --show-token"),
            fixture.run("flottille agent list"),
            fixture.judge(
                "forge",
                json!({"action": "pr-delete", "repository": "."}),
                Some(&fixture.never),
            ),
            fixture.judge(
                "forge",
                json!({"action": "create", "repository": ".", "waive": "x"}),
                Some(&fixture.never),
            ),
        ] {
            assert!(denied(&verdict), "{verdict:?}");
        }
    }

    #[test]
    fn a_shell_line_hiding_a_pipe_behind_an_escaped_quote_is_refused() {
        let fixture = Fixture::workspace();
        for line in [
            r#"sh -c 'echo \" | git push --force \"'"#,
            r#"sh -c 'echo \'"'"' | git push --force \'"'""#,
            r#"fish -c "'a\' '| git push --force '\' '""#,
            r#"sh -c 'echo "$(git push --force)"'"#,
        ] {
            let verdict = fixture.run(line);
            assert!(
                matches!(&verdict, Verdict::Deny(why) if why.contains("is shell syntax")),
                "{line}: {verdict:?}"
            );
        }
    }

    #[test]
    fn a_pipe_quoted_in_a_shell_line_is_its_text() {
        let fixture = Fixture::workspace();
        let verdict = fixture.run(r#"sh -c 'echo "a | b"'"#);
        assert!(!denied(&verdict), "{verdict:?}");
        assert!(base_class(
            &fixture.run(r#"bash -c 'git push --force "a | b"'"#)
        ));
    }

    #[test]
    fn the_wrappers_are_unwrapped_and_judged_as_what_they_run() {
        let fixture = Fixture::workspace();
        for line in [
            "env FOO=1 git push --force",
            "sudo cargo publish",
            "timeout 10 git push -f",
            "bash -c 'git push --force'",
            "xargs rm -rf",
        ] {
            assert!(
                base_class(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
    }

    #[test]
    fn a_path_outside_is_shown_as_the_place_shows_it() {
        let fixture = Fixture::workspace();
        let Verdict::Ask(ask) = fixture.file("fs_read", "/etc/hosts") else {
            panic!("outside is judged");
        };
        assert!(
            ask.reasons[0].starts_with("/etc/hosts is outside"),
            "{ask:?}"
        );
    }

    #[test]
    fn a_program_the_agent_wrote_is_judged_whatever_its_name() {
        let fixture = Fixture::workspace();
        let root = fixture.root.display().to_string();
        let bin = fixture.root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        installed(&bin.join("ls"));
        installed(&bin.join("git"));
        let mut inside = Fixture::workspace();
        let system = PathBuf::from(&inside.search);
        inside.search = std::env::join_paths([inside.root.join("bin"), system]).unwrap();
        std::fs::create_dir_all(inside.root.join("bin")).unwrap();
        installed(&inside.root.join("bin/cat"));
        for (fixture, line) in [
            (&fixture, "./ls".to_owned()),
            (&fixture, "./ls -la src".to_owned()),
            (&fixture, format!("{root}/bin/ls")),
            (&fixture, "./git status".to_owned()),
            (&fixture, "bin/git log".to_owned()),
            (&fixture, "/usr/bin/../../tmp/ls".to_owned()),
            (&inside, "cat src/a.rs".to_owned()),
        ] {
            let verdict = fixture.run(&line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("agent's own file"),
                "{line}: {verdict:?}"
            );
        }
    }

    #[test]
    fn a_read_only_program_with_an_option_that_runs_a_program_is_judged() {
        for fixture in [Fixture::main(), Fixture::workspace()] {
            for line in [
                "find . -exec cat {} ';'",
                "find . -execdir cat {} +",
                "find . -ok cat {} ';'",
                "fd -x cat",
                "fd -X cat",
                "fd --exec cat",
                "fd --exec-batch cat",
                "fd needle -x cat",
                "rg --pre cat needle",
                "rg --pre=cat needle",
                "rg --hostname-bin hostname needle",
                "git grep -O needle",
                "git grep --open-files-in-pager=cat needle",
                "git log -p --ext-diff",
                "git diff --ext-diff",
                "git diff --textconv",
                "git show --ext-diff HEAD",
                "git cat-file --textconv HEAD:a",
                "git cat-file --filters HEAD:a",
                "less src/a.rs",
                "more src/a.rs",
                "git help -w log",
                "date -s 2020-01-01",
                "date -us 2020-01-01",
                "date --set=2020-01-01",
            ] {
                let verdict = fixture.run(line);
                assert!(to_judge(&verdict), "{line}: {verdict:?}");
            }
        }
    }

    #[test]
    fn a_base_line_an_option_runs_is_base() {
        let fixture = Fixture::workspace();
        for line in [
            "fd -X git push -f",
            "fd -x cargo publish",
            "find . -exec git push --force ';'",
            "find . -execdir npm publish ';'",
            "git grep --open-files-in-pager='git push -f' needle",
            "rg --pre=deploy needle",
        ] {
            let verdict = fixture.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn a_global_git_option_does_not_hide_its_subcommand() {
        let main = Fixture::main();
        for line in [
            "git -c color.ui=never push --force",
            "git -ccolor.ui=never push -f",
            "git --no-pager -c color.ui=never push --mirror",
            "git --git-dir=other push --delete origin x",
            "git --work-tree other -c a.b=c push origin +x",
            "git -c color.ui=never reset --hard",
            "git --config-env=color.ui=X clean -fd",
        ] {
            let verdict = main.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
        for line in [
            "git -c color.ui=never status",
            "git -c color.ui=never commit -m x",
            "git --git-dir=other log",
        ] {
            let verdict = main.run(line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn a_global_git_setting_that_runs_or_redirects_is_base() {
        let fixture = Fixture::workspace();
        for line in [
            "git -c core.pager=cat log",
            "git -c alias.st=!x st",
            "git -c remote.origin.url=x fetch",
            "git -c url.x.insteadOf=y fetch",
            "git -c push.default=matching push",
            "git -c include.path=x status",
            "git -cinclude.path=x status",
            "git -c includeIf.onbranch:x.path=y status",
            "git -c credential.helper=x fetch",
            "git --config-env core.hooksPath=X commit -m x",
            "git --config-env=alias.x=X status",
        ] {
            let verdict = fixture.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
        // The program git runs for a remote, which may be this machine, is refused.
        assert!(denied(&fixture.run("git -c core.sshCommand=x fetch")));
    }

    #[test]
    fn a_push_option_shortened_to_a_unique_prefix_is_base() {
        let fixture = Fixture::workspace();
        for line in [
            "git push --mirr",
            "git push --dele origin x",
            "git push --prun origin",
            "git push --forc origin x",
            "git push --force-w origin x",
            "git push --force-with-lease=x:y origin x",
        ] {
            let verdict = fixture.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
        for line in [
            "git push --dry-run",
            "git push --set-upstream origin x",
            "git push --no-force origin x",
        ] {
            let verdict = fixture.run(line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn a_launcher_inside_a_line_is_refused_but_not_an_ordinary_word() {
        let workspace = Fixture::workspace();
        for line in [
            "cargo run -- systemd-run --user touch x",
            "chronic tmux new-session -d true",
            "uv run ssh localhost true",
            "nice at now",
        ] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
        for line in [
            "git commit -m at",
            "cargo test -p flottille --lib",
            "cargo run --example screen --release",
            "cargo test ssh",
        ] {
            assert!(!denied(&workspace.run(line)), "{line}");
        }
    }

    #[test]
    fn a_workspace_in_flottille_s_data_folder_is_the_project_s() {
        let mut fixture = Fixture::workspace();
        let data = fixture.root.parent().unwrap().to_owned();
        fixture.own = vec![data.clone()];
        let ws = data.join("workspaces/app/w");
        std::fs::create_dir_all(&ws).unwrap();
        fixture.root = ws.clone();
        fixture.grant.place.root = ws.clone();
        fixture.grant.place.repositories = vec![(".".into(), ws)];
        assert_eq!(fixture.file("fs_write", "notes.md"), Verdict::Allow);
        // The folder that holds every workspace is Flottille's.
        assert!(denied(&fixture.run("rm -r ../../../workspaces")));
    }

    #[test]
    fn a_workspaces_folder_in_flottille_s_configuration_or_state_is_flottille_s() {
        let mut fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        // The data folder first, as the daemon names them.
        fixture.own = vec![
            base.join("xdg/data/flottille"),
            base.join("xdg/config/flottille"),
        ];
        let path = base.join("xdg/config/flottille/workspaces/app/w/notes.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let verdict = fixture.file("fs_write", &path.display().to_string());
        assert!(denied(&verdict), "{verdict:?}");
    }

    #[test]
    fn an_unlisted_wrapper_cannot_turn_base_into_judge() {
        let workspace = Fixture::workspace();
        for line in [
            "unbuffer git push -f",
            "ionice -c3 git push --force",
            "taskset 1 git push -f",
            "flock /tmp/lock git push -f",
            "chrt 1 git push --mirror",
            "script -c 'git push -f' /dev/null",
            "script --command='git push -f'",
            "watch git push -f",
            "watch -n 5 'git push -f'",
            "strace -f git push -f",
            "unbuffer cargo publish",
            "unbuffer rm -rf .git",
            "unbuffer rm -rf .",
            "unbuffer deploy now",
            "unbuffer ./git push -f",
        ] {
            let verdict = workspace.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
        // A launcher out of the daemon's tree is refused outright.
        assert!(denied(&workspace.run("setsid git push -f")));
        let main = Fixture::main();
        for line in ["unbuffer git reset --hard", "flock x git checkout -- ."] {
            let verdict = main.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn clearing_or_setting_git_s_configuration_through_the_environment_is_base() {
        let fixture = Fixture::workspace();
        for line in [
            "env GIT_CONFIG_COUNT=0 cargo test",
            "env GIT_CONFIG_PARAMETERS=x git commit -m x",
            "env GIT_CONFIG_GLOBAL=x git status",
            "env -i cargo test",
            "env - cargo test",
            "env --ignore-environment cargo test",
            "env -u GIT_CONFIG_COUNT cargo test",
            "env --unset=GIT_CONFIG_KEY_0 cargo test",
        ] {
            let verdict = fixture.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn writing_a_sensitive_git_setting_is_base() {
        for fixture in [Fixture::main(), Fixture::workspace()] {
            // What runs later as the user, or the user's own configuration, is refused.
            for line in [
                "git config core.hooksPath x",
                "git config set alias.x y",
                "git config --global url.x.insteadOf y",
                "git config --replace-all credential.helper x",
                "git config include.path x",
            ] {
                let verdict = fixture.run(line);
                assert!(denied(&verdict), "{line}: {verdict:?}");
            }
            for line in [
                "git config --add remote.origin.pushurl x",
                "git config --unset push.default",
                "git config unset push.default",
                "git config --rename-section remote.a remote.b",
                "git config --remove-section credential",
                "git config --file .git/config core.x y",
                "git config -e",
                "git remote set-url origin x",
                "git remote set-url --push origin x",
                "git remote add other x",
                "git remote rename origin o",
                "git remote remove origin",
                "git remote set-branches origin x",
            ] {
                let verdict = fixture.run(line);
                assert!(base_class(&verdict), "{line}: {verdict:?}");
            }
            for line in ["git config user.name someone", "git remote prune origin"] {
                let verdict = fixture.run(line);
                assert!(to_judge(&verdict), "{line}: {verdict:?}");
            }
            assert_eq!(fixture.run("git config --get core.editor"), Verdict::Allow);
        }
    }

    #[test]
    fn discarding_work_in_main_is_base_in_all_its_forms() {
        let main = Fixture::main();
        std::fs::write(main.root.join("src/a.rs"), "a\n").unwrap();
        let workspace = Fixture::workspace();
        for line in [
            "git checkout src",
            "git checkout src/a.rs",
            "git checkout HEAD src/a.rs",
            "git checkout '*.rs'",
            "git checkout :/src",
            "git checkout --ours src",
            "git checkout -B topic",
            "git checkout --force main",
            "git switch -f main",
            "git switch --discard-changes main",
            "git switch -C topic",
            "git switch --force-create topic",
            "git checkout-index -f -a",
            "git checkout-index -af",
            "git read-tree --reset -u HEAD",
            "git read-tree -m -u HEAD",
            "git branch -f main HEAD~1",
            "git branch --force main HEAD~1",
            "git branch -M x",
            "git branch -m x",
            "git update-ref refs/heads/main HEAD~1",
            "git rm src/a.rs",
            "git apply --reject x.patch",
            "git am --reject x.mbox",
            "git merge --abort",
            "git cherry-pick --abort",
            "git revert --abort",
            "git symbolic-ref HEAD refs/heads/other",
            "git submodule update --force",
            "git submodule deinit -f x",
        ] {
            let verdict = main.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
            assert!(
                !matches!(workspace.run(line), Verdict::Deny(_)),
                "{line}: {:?}",
                workspace.run(line)
            );
        }
        for line in [
            "git checkout main",
            "git checkout -b topic",
            "git switch main",
            "git rm --cached src/a.rs",
            "git apply x.patch",
        ] {
            let verdict = main.run(line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn an_option_that_writes_a_file_is_judged_in_main_and_harmless_inside_a_workspace() {
        let main = Fixture::main();
        let workspace = Fixture::workspace();
        for line in [
            "git diff --output=out.patch",
            "git diff --output out.patch",
            "git log -p --outp=out.txt",
            "git show --output=out.txt HEAD",
            "git format-patch -o patches HEAD~1",
            "tree -o out.txt",
            "find . -fprint out.txt",
            "file -C -m magic",
        ] {
            let verdict = main.run(line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
            assert_eq!(workspace.run(line), Verdict::Allow, "{line}");
        }
        for line in [
            "git diff --output=../../home/out.patch",
            "tree -o ../../home/out.txt",
        ] {
            let verdict = workspace.run(line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn a_recursive_reader_over_a_folder_with_a_sensitive_file_is_judged() {
        let fixture = Fixture::workspace();
        std::fs::create_dir_all(fixture.root.join("config")).unwrap();
        std::fs::write(fixture.root.join("config/.env"), "A=b\n").unwrap();
        for line in [
            "grep -r needle .",
            "grep -rn needle",
            "grep -R needle config",
            "grep --recursive needle",
            "grep -d recurse needle .",
            "rg -u needle",
            "rg -uu needle",
            "rg --hidden needle",
            "rg --no-ignore needle",
            "git grep --untracked needle",
            "git grep --no-index needle",
            "fd -H env",
            "fd --hidden env",
            "fd -I env",
            "find -L .",
        ] {
            let verdict = fixture.run(line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
            assert!(reasons(&verdict).contains(".env"), "{line}: {verdict:?}");
        }
        for line in [
            "grep -rn needle src",
            "rg needle",
            "fd env",
            "git grep needle",
        ] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn reading_another_process_s_proc_is_base() {
        let mut fixture = Fixture::workspace();
        for verdict in [
            fixture.file("fs_read", "/proc/1/status"),
            fixture.file("fs_read", "/proc/self/environ"),
            fixture.run("cat /proc/1/cmdline"),
            fixture.run("cat /proc/self/environ"),
            fixture.run("cat /proc/thread-self/environ"),
            fixture.run("cat /proc/self/task/1/environ"),
        ] {
            assert!(base_class(&verdict), "{verdict:?}");
        }
        assert!(to_judge(&fixture.file("fs_read", "/proc/self/status")));
        // The agent's own runs are its own processes.
        let me = std::process::id();
        fixture.groups = vec![unsafe { libc::getpgid(0) } as u32];
        let verdict = fixture.file("fs_read", &format!("/proc/{me}/status"));
        assert!(to_judge(&verdict), "{verdict:?}");
        assert!(base_class(
            &fixture.file("fs_read", &format!("/proc/{me}/environ"))
        ));
    }

    #[test]
    fn a_path_through_a_process_s_root_cwd_or_files_in_proc_is_base_to_read_and_refused_to_write() {
        let mut fixture = Fixture::workspace();
        let own = fixture.own[0].clone();
        std::fs::create_dir_all(&own).unwrap();
        let me = std::process::id();
        // Even the agent's own runs: their root leads anywhere on the disk.
        fixture.groups = vec![unsafe { libc::getpgid(0) } as u32];
        let judge = own.join("judge.md");
        let judge = judge.display();
        for path in [
            format!("/proc/self/root{judge}"),
            format!("/proc/thread-self/root{judge}"),
            format!("/proc/{me}/root{judge}"),
            format!("/proc/1/root{judge}"),
            format!("/proc/{me}/cwd/judge.md"),
            "/proc/self/fd/3".into(),
            format!("/proc/self/task/{me}/root{judge}"),
            format!("/proc/{me}/exe"),
        ] {
            for tool in ["fs_write", "fs_edit", "fs_read"] {
                let verdict = fixture.file(tool, &path);
                // Writing through any of them is refused.
                if WRITES.contains(&tool) {
                    assert!(denied(&verdict), "{tool} {path}: {verdict:?}");
                } else {
                    assert!(base_class(&verdict), "{tool} {path}: {verdict:?}");
                }
            }
        }
        let verdict = fixture.run(&format!("cat /proc/self/root{judge}"));
        assert!(base_class(&verdict), "{verdict:?}");
    }

    #[test]
    fn flottille_s_own_folder_reached_through_a_link_is_base_to_read_and_refused_to_write() {
        let mut fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        // `XDG_CONFIG_HOME` (or `~/.config`) is a link: Flottille names its folder through it.
        std::fs::create_dir_all(base.join("real-config/flottille")).unwrap();
        std::fs::create_dir_all(base.join("xdg")).unwrap();
        std::os::unix::fs::symlink(base.join("real-config"), base.join("xdg/config")).unwrap();
        fixture.own = vec![base.join("xdg/config/flottille")];
        let real = base.join("real-config/flottille/judge.md");
        // A link the agent made outside its place, and one inside it, to Flottille's folder.
        std::fs::create_dir_all(base.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(base.join("xdg/config"), base.join("elsewhere/link")).unwrap();
        std::os::unix::fs::symlink(base.join("real-config"), fixture.root.join("src/link"))
            .unwrap();
        for path in [
            real.display().to_string(),
            base.join("elsewhere/link/flottille/judge.md")
                .display()
                .to_string(),
            base.join("elsewhere/link/flottille/new/judge.md")
                .display()
                .to_string(),
            "src/link/flottille/judge.md".into(),
        ] {
            for tool in ["fs_write", "fs_edit", "fs_read"] {
                let verdict = fixture.file(tool, &path);
                if WRITES.contains(&tool) {
                    assert!(denied(&verdict), "{tool} {path}: {verdict:?}");
                } else {
                    assert!(base_class(&verdict), "{tool} {path}: {verdict:?}");
                }
                assert!(
                    reasons(&verdict).contains("Flottille's own folder"),
                    "{tool} {path}: {verdict:?}"
                );
            }
        }
        // A sensitive place of the home through a link the agent made is still that place.
        std::fs::create_dir_all(base.join("home/.ssh")).unwrap();
        std::os::unix::fs::symlink(base.join("home/.ssh"), base.join("elsewhere/keys")).unwrap();
        let path = base.join("elsewhere/keys/id_ed25519").display().to_string();
        assert!(denied(&fixture.file("fs_read", &path)));
    }

    const FS_TOOLS: [&str; 4] = ["fs_read", "fs_list", "fs_write", "fs_edit"];

    #[test]
    fn dot_dot_after_a_link_is_taken_where_the_link_leads() {
        let fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".config/nvim")).unwrap();
        std::fs::create_dir_all(home.join(".config/gh")).unwrap();
        std::fs::write(home.join(".config/gh/hosts.yml"), "token").unwrap();
        std::fs::create_dir_all(&fixture.own[0]).unwrap();
        std::fs::create_dir_all(base.join("xdg/data/other")).unwrap();
        // Links inside the place, and one outside it.
        std::os::unix::fs::symlink(home.join(".config/nvim"), fixture.root.join("src/cfg"))
            .unwrap();
        std::os::unix::fs::symlink(base.join("xdg/data/other"), fixture.root.join("src/data"))
            .unwrap();
        std::fs::create_dir_all(base.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(home.join(".config/nvim"), base.join("elsewhere/cfg")).unwrap();
        std::os::unix::fs::symlink(base.join("xdg/data/other"), base.join("elsewhere/data"))
            .unwrap();
        let outside = |path: &str| base.join("elsewhere").join(path).display().to_string();

        for (path, place) in [
            ("src/cfg/../gh/hosts.yml".to_owned(), "~/.config/gh"),
            (outside("cfg/../gh/hosts.yml"), "~/.config/gh"),
            (
                "src/data/../flottille/judge.md".to_owned(),
                "Flottille's own folder",
            ),
            (
                outside("data/../flottille/judge.md"),
                "Flottille's own folder",
            ),
        ] {
            for tool in FS_TOOLS {
                let verdict = fixture.file(tool, &path);
                // A login's place is refused; Flottille's own folder is base to read only.
                if place.starts_with('~') || WRITES.contains(&tool) {
                    assert!(denied(&verdict), "{tool} {path}: {verdict:?}");
                } else {
                    assert!(base_class(&verdict), "{tool} {path}: {verdict:?}");
                }
                assert!(
                    reasons(&verdict).contains(place),
                    "{tool} {path}: {verdict:?}"
                );
            }
            let verdict = fixture.run(&format!("cat {path}"));
            if place.starts_with('~') {
                assert!(denied(&verdict), "cat {path}: {verdict:?}");
            } else {
                assert!(base_class(&verdict), "cat {path}: {verdict:?}");
            }
            assert!(reasons(&verdict).contains(place), "cat {path}: {verdict:?}");
        }
        // What the tools open is what the gate checked: where the link leads.
        let resolved = fixture
            .grant
            .place
            .resolve(&fixture.root, "src/cfg/../gh/hosts.yml");
        assert!(!resolved.inside);
        assert_eq!(resolved.path, home.join(".config/gh/hosts.yml"));
        // A `..` that stays inside is still inside.
        assert_eq!(fixture.file("fs_read", "src/../src/a.rs"), Verdict::Allow);
    }

    #[test]
    fn a_path_that_cannot_be_followed_is_base() {
        let fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        std::fs::create_dir_all(&fixture.own[0]).unwrap();
        let chain = base.join("chain");
        std::fs::create_dir_all(&chain).unwrap();
        // 33 links, then 41 (Linux follows 40), ending in Flottille's own folder.
        let link_chain = |name: &str, links: usize| {
            let mut target = fixture.own[0].clone();
            for n in (0..links).rev() {
                let link = chain.join(format!("{name}{n}"));
                std::os::unix::fs::symlink(&target, &link).unwrap();
                target = link;
            }
            target.join("judge.md").display().to_string()
        };
        let long = link_chain("a", 33);
        let longer = link_chain("b", 41);

        for tool in FS_TOOLS {
            let verdict = fixture.file(tool, &long);
            // Reading Flottille's own folder is base; writing there is refused.
            if WRITES.contains(&tool) {
                assert!(denied(&verdict), "{tool}: {verdict:?}");
            } else {
                assert!(base_class(&verdict), "{tool}: {verdict:?}");
            }
            assert!(
                reasons(&verdict).contains("Flottille's own folder"),
                "{tool}: {verdict:?}"
            );
            let verdict = fixture.file(tool, &longer);
            assert!(base_class(&verdict), "{tool}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("could not be followed"),
                "{tool}: {verdict:?}"
            );
        }
        for path in [&long, &longer] {
            let verdict = fixture.run(&format!("cat {path}"));
            assert!(base_class(&verdict), "{verdict:?}");
        }
    }

    #[test]
    fn a_link_into_proc_under_another_name_is_base_to_read_and_refused_to_write() {
        let fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        std::fs::create_dir_all(base.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink("/proc/self/cwd", base.join("elsewhere/here")).unwrap();
        std::os::unix::fs::symlink("/proc/self/cwd", fixture.root.join("src/here")).unwrap();
        let here = base.join("elsewhere/here/judge.md").display().to_string();
        for path in ["/dev/fd/3", "/dev/stdin", &here, "src/here/judge.md"] {
            for tool in FS_TOOLS {
                let verdict = fixture.file(tool, path);
                // Writing through it is refused.
                if WRITES.contains(&tool) {
                    assert!(denied(&verdict), "{tool} {path}: {verdict:?}");
                } else {
                    assert!(base_class(&verdict), "{tool} {path}: {verdict:?}");
                }
            }
            // A run's own streams are its own (`the_run_s_own_streams_are_harmless_words`).
            if path != "/dev/stdin" {
                let verdict = fixture.run(&format!("cat {path}"));
                assert!(base_class(&verdict), "cat {path}: {verdict:?}");
            }
        }
        // The reading program's own status is still its own.
        assert!(!base_class(&fixture.run("cat /proc/self/status")));
    }

    #[test]
    fn moving_or_removing_a_folder_above_flottille_s_own_is_refused() {
        let mut fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        let config = base.join("home/.config");
        fixture.own = vec![config.join("flottille")];
        std::fs::create_dir_all(config.join("flottille")).unwrap();
        let shown = config.display();

        for line in [
            format!("mv {shown} {shown}.bak"),
            format!("rm -r {}", base.join("home").display()),
            format!("rm -r {shown}/../.config"),
        ] {
            let verdict = fixture.run(&line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("Flottille's own folders"),
                "{line}: {verdict:?}"
            );
        }
        for path in [config.display().to_string(), "~/.config".to_owned()] {
            let verdict = fixture.file("fs_write", &path);
            assert!(denied(&verdict), "{path}: {verdict:?}");
        }
        // Reading it is not taking it away.
        assert!(to_judge(&fixture.run(&format!("ls {shown}"))));
        assert!(to_judge(&fixture.file("fs_list", "~/.config")));
    }

    #[test]
    fn a_missing_folder_followed_by_dot_dot_is_refused() {
        let fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".config/nvim")).unwrap();
        std::fs::create_dir_all(&fixture.own[0]).unwrap();
        std::os::unix::fs::symlink(home.join(".config/nvim"), fixture.root.join("src/cfg"))
            .unwrap();
        // `src/up` leads to the folder holding the work, the home and Flottille's folders.
        std::os::unix::fs::symlink("../../..", fixture.root.join("src/up")).unwrap();
        // A link whose own target goes up out of a folder that does not exist.
        std::os::unix::fs::symlink("zz/../cfg", fixture.root.join("src/via")).unwrap();

        for path in [
            "m/../src/cfg/init.lua",
            "m/../src/up/xdg/data/flottille/judge.md",
            "src/via/init.lua",
        ] {
            for tool in FS_TOOLS {
                let verdict = fixture.file(tool, path);
                assert!(denied(&verdict), "{tool} {path}: {verdict:?}");
                assert!(
                    reasons(&verdict).contains("does not exist and then goes up out of it"),
                    "{tool} {path}: {verdict:?}"
                );
            }
            // A run's word is checked where it leads once the folder is made instead
            // (`a_word_going_up_out_of_a_folder_a_program_can_make_…`): Flottille's own is base.
            let verdict = fixture.run(&format!("cat {path}"));
            if path.contains("flottille") {
                assert!(base_class(&verdict), "cat {path}: {verdict:?}");
            } else {
                assert!(to_judge(&verdict), "cat {path}: {verdict:?}");
            }
        }
        // Deciding made nothing on the disk.
        assert!(!fixture.root.join("m").exists());
        assert!(!fixture.root.join("src/zz").exists());
        // A `..` out of a folder that exists is still followed.
        assert_eq!(fixture.file("fs_read", "src/../src/a.rs"), Verdict::Allow);
    }

    #[test]
    fn a_name_too_long_or_below_a_file_is_taken_as_written_not_base() {
        let fixture = Fixture::workspace();
        std::fs::write(fixture.root.join("src/a.rs"), "fn main() {}\n").unwrap();
        let long = "word ".repeat(60);
        for line in [
            format!("git commit -m '{long}'"),
            format!("echo '{long}'"),
            "cat src/a.rs/x".to_owned(),
        ] {
            let verdict = fixture.run(&line);
            assert!(!base_class(&verdict), "{line}: {verdict:?}");
            assert!(!denied(&verdict), "{line}: {verdict:?}");
        }
        assert!(!base_class(&fixture.file("fs_read", "src/a.rs/x")));
    }

    #[test]
    fn a_run_s_own_folder_above_flottille_s_folders_is_not_base() {
        let mut fixture = Fixture::workspace();
        // A project whose folder holds Flottille's own folders: a dotfiles repository.
        fixture.own = vec![fixture.root.join("sub/flottille")];
        std::fs::create_dir_all(&fixture.own[0]).unwrap();

        for line in ["git commit -m x", "git add notes.md"] {
            let verdict = fixture.run(line);
            assert!(!base_class(&verdict), "{line}: {verdict:?}");
        }
        for line in [
            format!("rm -r {}", fixture.root.join("sub").display()),
            "rm -r sub".to_owned(),
            "rm -r .".to_owned(),
        ] {
            let verdict = fixture.run(&line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("Flottille's own folders"),
                "{line}: {verdict:?}"
            );
        }
    }

    #[test]
    fn the_run_s_own_streams_are_harmless_words() {
        let fixture = Fixture::workspace();
        for stream in [
            "/dev/stdin",
            "/dev/stdout",
            "/dev/stderr",
            "/dev/null",
            "/dev/fd/0",
            "/dev/fd/1",
            "/dev/fd/2",
        ] {
            let line = format!("tee {stream}");
            let verdict = fixture.run(&line);
            assert!(!base_class(&verdict), "{line}: {verdict:?}");
            assert!(!reasons(&verdict).contains(stream), "{line}: {verdict:?}");
        }
        // Another of its open files may be anything: read, held; written, refused.
        assert!(denied(&fixture.run("tee /dev/fd/3")));
        assert!(base_class(&fixture.run("cat /dev/fd/3")));
    }

    /// Makes `parts` as folders below the open folder `at`, one at a time, so that their whole
    /// path may be longer than the kernel takes at once; returns the deepest, open.
    fn folders_below(at: std::os::fd::BorrowedFd, parts: &[String]) -> std::os::fd::OwnedFd {
        use rustix::fs::{Mode, OFlags};
        let mut deepest = rustix::fs::openat(at, ".", OFlags::PATH, Mode::empty()).unwrap();
        for part in parts {
            rustix::fs::mkdirat(&deepest, part.as_str(), Mode::from_raw_mode(0o755)).unwrap();
            deepest = rustix::fs::openat(
                &deepest,
                part.as_str(),
                OFlags::PATH | OFlags::DIRECTORY,
                Mode::empty(),
            )
            .unwrap();
        }
        deepest
    }

    #[test]
    fn a_path_too_long_once_its_links_are_expanded_still_has_them_followed() {
        use std::os::fd::AsFd;
        let fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        std::fs::create_dir_all(base.join("home/.ssh")).unwrap();
        std::fs::write(base.join("home/.ssh/id"), "fake key").unwrap();
        // `src/a`, then `b` and `c` where it leads, each seven folders of 200 characters
        // deeper: their whole expanded path is longer than the kernel takes at once, a part at
        // a time it is not. `x` leads to the home's `.ssh`.
        let chain = |letter: char| -> Vec<String> {
            (1..=7)
                .map(|n| format!("{n}{}", letter.to_string().repeat(199)))
                .collect()
        };
        let root = std::fs::File::open(&fixture.root).unwrap();
        let mut at = folders_below(root.as_fd(), &chain('a'));
        std::os::unix::fs::symlink(
            format!("../{}", chain('a').join("/")),
            fixture.root.join("src/a"),
        )
        .unwrap();
        for (link, letter) in [("b", 'b'), ("c", 'c')] {
            let next = folders_below(at.as_fd(), &chain(letter));
            rustix::fs::symlinkat(chain(letter).join("/"), &at, link).unwrap();
            at = next;
        }
        rustix::fs::symlinkat(base.join("home/.ssh"), &at, "x").unwrap();

        let verdict = fixture.run("cat src/a/b/c/x/id");
        assert!(denied(&verdict), "{verdict:?}");
        assert!(reasons(&verdict).contains(".ssh"), "{verdict:?}");
        let verdict = fixture.file("fs_read", "src/a/b/c/x/id");
        assert!(denied(&verdict), "{verdict:?}");
        // One name too long is still only a word.
        let verdict = fixture.run(&format!("git commit -m '{}'", "w".repeat(300)));
        assert!(!base_class(&verdict), "{verdict:?}");
    }

    #[test]
    fn dot_dot_below_proc_is_base_to_read_and_refused_to_write() {
        let fixture = Fixture::workspace();
        let own = fixture.own[0].clone();
        std::fs::create_dir_all(&own).unwrap();
        let judge = own.join("judge.md");
        let judge = judge.display();
        // `net` leads two folders down, `thread-self` three: the kernel stays in /proc.
        for path in [
            format!("/proc/net/../../self/root{judge}"),
            format!("/proc/thread-self/../../../self/root{judge}"),
        ] {
            let verdict = fixture.run(&format!("cat {path}"));
            assert!(base_class(&verdict), "cat {path}: {verdict:?}");
            // Writing through it is refused.
            for line in [format!("tee {path}"), format!("cp notes.md {path}")] {
                let verdict = fixture.run(&line);
                assert!(denied(&verdict), "{line}: {verdict:?}");
            }
            for tool in FS_TOOLS {
                let verdict = fixture.file(tool, &path);
                if WRITES.contains(&tool) {
                    assert!(denied(&verdict), "{tool} {path}: {verdict:?}");
                } else {
                    assert!(base_class(&verdict), "{tool} {path}: {verdict:?}");
                }
            }
        }
    }

    #[test]
    fn a_run_s_word_going_up_out_of_a_missing_folder_is_judged_and_named_as_written() {
        let fixture = Fixture::workspace();
        for (line, word) in [
            ("sed -i 's/a/../' src/a.rs", "s/a/../"),
            ("echo 'x/../y'", "x/../y"),
            ("git commit -m 'move foo/../bar'", "move foo/../bar"),
            ("curl http://h/../x", "http://h/../x"),
        ] {
            let verdict = fixture.run(line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
            assert!(
                reasons(&verdict).contains(&format!("{word} names a folder")),
                "{line}: {verdict:?}"
            );
        }
        // The file tools still refuse it.
        assert!(denied(&fixture.file("fs_read", "m/../x")));
    }

    #[test]
    fn deleting_untracked_files_in_a_folder_above_flottille_s_is_refused() {
        let mut fixture = Fixture::workspace();
        fixture.own = vec![fixture.root.join("sub/flottille")];
        std::fs::create_dir_all(&fixture.own[0]).unwrap();
        for line in [
            "git clean -fdx",
            "git clean -fd",
            "git stash -u",
            "git stash push --include-untracked",
            "git stash -a",
            "git stash --all",
        ] {
            let verdict = fixture.run(line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("Flottille's own folders"),
                "{line}: {verdict:?}"
            );
        }
        // Stashing tracked changes only takes nothing of Flottille's.
        assert!(!base_class(&fixture.run("git stash")));
    }

    #[test]
    fn a_word_going_up_out_of_a_folder_a_program_can_make_is_checked_where_it_leads() {
        let fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        std::fs::create_dir_all(base.join("home/.ssh")).unwrap();
        let own = fixture.own[0].clone();
        std::fs::create_dir_all(&own).unwrap();
        std::os::unix::fs::symlink(&base, fixture.root.join("src/h")).unwrap();
        let judge = own.join("judge.md");
        let judge = judge.display();
        let own_word = format!(
            "m/../../../{}/judge.md",
            own.strip_prefix(&base).unwrap().display()
        );
        // `install -D` makes `m`, then writes through its `..`.
        for line in [
            "install -D notes.md m/../../../home/.ssh/authorized_keys".to_owned(),
            format!("install -D notes.md {own_word}"),
            "install -D notes.md src/h/m/../home/.ssh/authorized_keys".into(),
            "install -D notes.md m/../src/h/home/.ssh/authorized_keys".into(),
            format!("install -D notes.md /m/../proc/self/root{judge}"),
            "install -D -t m/../../../home/.ssh notes.md".into(),
            "install -D --target-directory=m/../../../home/.ssh notes.md".into(),
        ] {
            // Where it leads is a login's place or Flottille's own folder: refused.
            let verdict = fixture.run(&line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
            let why = reasons(&verdict);
            assert!(
                why.contains(".ssh") || why.contains("Flottille's own folders"),
                "{line}: {verdict:?}"
            );
        }
        for line in [
            "sed -i 's/a/../' src/a.rs",
            "git commit -m 'move foo/../bar'",
        ] {
            assert!(to_judge(&fixture.run(line)), "{line}");
        }
        // The file tools still refuse it.
        assert!(denied(&fixture.file("fs_write", "m/../notes.md")));
    }

    #[test]
    fn deleting_the_untracked_files_of_the_whole_worktree_checks_its_top() {
        let mut fixture = Fixture::workspace();
        fixture.own = vec![fixture.root.join("sub/flottille")];
        std::fs::create_dir_all(&fixture.own[0]).unwrap();
        std::fs::write(fixture.root.join(".git"), "gitdir: elsewhere\n").unwrap();
        fixture.folder = "src".into();
        for line in [
            "git stash -u",
            "git stash push --all",
            "git clean -fdx :/",
            "git clean -fdx :(top)",
            "git clean -fdx -- :(top,exclude)x",
        ] {
            let verdict = fixture.run(line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("Flottille's own folders"),
                "{line}: {verdict:?}"
            );
        }
        // Without a pathspec, `git clean` cleans the folder it runs in only.
        let verdict = fixture.run("git clean -fdx");
        assert!(!base_class(&verdict), "{verdict:?}");
    }

    #[test]
    fn a_word_through_a_name_the_kernel_could_never_make_is_checked_as_written() {
        let fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        std::fs::create_dir_all(base.join("home/.ssh")).unwrap();
        std::fs::write(fixture.root.join("notes.md"), "n").unwrap();
        let own = fixture.own[0].clone();
        std::fs::create_dir_all(&own).unwrap();
        let long = "n".repeat(300);
        let own_word = own.strip_prefix(&base).unwrap().display().to_string();
        // `git restore` and `python3 -m zipfile` take `..` on the text: they reach the path as
        // written.
        for line in [
            format!("git restore -- {long}/../.env"),
            format!("python3 -m zipfile -e a.zip {long}/../../../home/.ssh/z2"),
            "python3 -m zipfile -e a.zip notes.md/x/../../../../home/.ssh".into(),
            format!("python3 -m zipfile -e a.zip {long}/../../../{own_word}"),
        ] {
            let verdict = fixture.run(&line);
            // As written it is a `.env`, base; a login's place or Flottille's own folder,
            // refused.
            if line.contains(".env") {
                assert!(base_class(&verdict), "{line}: {verdict:?}");
                assert!(
                    reasons(&verdict).contains("names a folder that does not exist"),
                    "{line}: {verdict:?}"
                );
            } else {
                assert!(denied(&verdict), "{line}: {verdict:?}");
                let why = reasons(&verdict);
                assert!(
                    why.contains(".ssh") || why.contains("Flottille's own folders"),
                    "{line}: {verdict:?}"
                );
            }
        }
        let verdict = fixture.run("git commit -m 'fix foo/../bar'");
        assert!(to_judge(&verdict), "{verdict:?}");
        // The file tools still refuse it.
        assert!(denied(
            &fixture.file("fs_write", &format!("{long}/../.env"))
        ));
    }

    #[test]
    fn an_option_s_value_attached_without_equals_is_resolved() {
        let fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        std::fs::create_dir_all(base.join("home/.ssh")).unwrap();
        for line in [
            "install -Dtm/../../../home/.ssh notes.md",
            "install -Dt../../home/.ssh notes.md",
            "curl -om/../../../home/.ssh/authorized_keys https://x",
        ] {
            let verdict = fixture.run(line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
            assert!(reasons(&verdict).contains(".ssh"), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn the_top_of_a_worktree_is_found_on_the_disk() {
        let mut fixture = Fixture::workspace();
        let base = fixture.root.parent().unwrap().parent().unwrap().to_owned();
        let home = base.join("home");
        // The home itself is a repository, Flottille's data in it, a project folder below.
        std::fs::create_dir_all(home.join(".git")).unwrap();
        fixture.own = vec![home.join(".local/share/flottille")];
        std::fs::create_dir_all(&fixture.own[0]).unwrap();
        std::fs::create_dir_all(home.join("outer/sub")).unwrap();
        let sub = home.join("outer/sub");
        for line in [
            format!("git -C {} stash -u", sub.display()),
            format!(
                "git -C {} -C sub clean -fdx :/",
                home.join("outer").display()
            ),
            format!("git --work-tree={} stash --all", home.display()),
        ] {
            let verdict = fixture.run(&line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("Flottille's own folders"),
                "{line}: {verdict:?}"
            );
        }
    }

    #[test]
    fn a_git_setting_whose_value_git_runs_is_base_and_refused_to_write() {
        let keys = [
            "diff.external",
            "Diff.External",
            "diff.word.command",
            "diff.pdf.textconv",
            "difftool.mine.cmd",
            "difftool.mine.path",
            "mergetool.mine.cmd",
            "merge.ours.driver",
            "filter.lfs.smudge",
            "filter.x.process",
            "gpg.program",
            "gpg.ssh.program",
            "gpg.ssh.defaultKeyCommand",
            "sequence.editor",
            "pager.log",
            "interactive.diffFilter",
            "submodule.lib.update",
            "protocol.allow",
            "protocol.ext.allow",
            "hook.check.command",
            "uploadpack.packObjectsHook",
            "imap.tunnel",
            "guitool.x.cmd",
            "tar.tgz.command",
            "sendemail.sendmailCmd",
            "man.mine.cmd",
            "web.browser",
            "browser.mine.path",
            "instaweb.httpd",
            "trailer.sign.command",
            "trailer.sign.cmd",
        ];
        for fixture in [Fixture::main(), Fixture::workspace()] {
            for key in keys {
                for line in [
                    format!("git -c {key}=x status"),
                    format!("git -c{key}=x log"),
                    format!("git --config-env {key}=VALUE diff"),
                    format!("git --config-env={key}=VALUE diff"),
                ] {
                    let verdict = fixture.run(&line);
                    assert!(base_class(&verdict), "{line}: {verdict:?}");
                }
                // Written, git runs it later, as the user: refused.
                for line in [
                    format!("git config {key} x"),
                    format!("git config set {key} x"),
                ] {
                    let verdict = fixture.run(&line);
                    assert!(denied(&verdict), "{line}: {verdict:?}");
                }
            }
            for line in [
                "env GIT_EXTERNAL_DIFF=x git diff",
                "env GIT_PAGER=x git log",
                "env GIT_EDITOR=x git commit",
                "env GIT_SEQUENCE_EDITOR=x git rebase -i main",
                "env GIT_SSH_COMMAND=x git fetch",
                "env GIT_SSH=x git fetch",
                "env GIT_ASKPASS=x git fetch",
                "env GIT_PROXY_COMMAND=x git fetch",
                "env GIT_EXEC_PATH=x git status",
            ] {
                let verdict = fixture.run(line);
                assert!(base_class(&verdict), "{line}: {verdict:?}");
            }
            for line in [
                "git -c diff.renames=false diff",
                "git -c merge.ff=only merge x",
                "git config submodule.recurse true",
                "git config gpg.format ssh",
            ] {
                let verdict = fixture.run(line);
                assert!(!base_class(&verdict), "{line}: {verdict:?}");
            }
        }
    }

    #[test]
    fn a_push_with_a_receive_pack_or_exec_is_refused() {
        for fixture in [Fixture::main(), Fixture::workspace()] {
            for line in [
                "git push --receive-pack=x origin HEAD",
                "git push --receive-pack x origin HEAD",
                "git push --receive=x origin HEAD",
                "git push --exec=x origin HEAD",
                "git push --exec x origin HEAD",
                "git push --ex=x origin HEAD",
                "git send-pack --receive-pack=x ../r HEAD",
            ] {
                let verdict = fixture.run(line);
                assert!(denied(&verdict), "{line}: {verdict:?}");
            }
            let verdict = fixture.run("git push --repo=origin");
            assert!(to_judge(&verdict), "{verdict:?}");
        }
    }

    #[test]
    fn sudo_doas_and_pkexec_are_base_in_any_form() {
        let fixture = Fixture::workspace().command("deploy-local", "sudo make install", |_| {});
        for line in [
            "sudo ls",
            "sudo -E npm test",
            "sudo -u nobody id",
            "/usr/bin/sudo ls",
            "sudoedit src/a.rs",
            "doas ls",
            "doas -u nobody id",
            "pkexec ls",
            "pkexec --user nobody id",
            "run0 ls",
            "su -c ls",
            "env sudo ls",
            "nice -n 5 doas ls",
            "timeout 5 pkexec ls",
            "npx sudo ls",
        ] {
            let verdict = fixture.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
            assert!(reasons(&verdict).contains("another user"), "{line}");
        }
        assert!(base_class(&fixture.task("deploy-local")));
        assert_eq!(fixture.run("echo sudo"), Verdict::Allow);
    }

    #[test]
    fn removing_git_s_safety_net_is_base_in_main_and_in_a_workspace() {
        for fixture in [Fixture::main(), Fixture::workspace()] {
            for line in [
                "git reflog expire --expire=now --all",
                "git reflog delete HEAD@{1}",
                "git gc --prune=now",
                "git gc --prune=all --aggressive",
                "git prune",
                "git prune --expire=now",
                "git -C . prune",
            ] {
                let verdict = fixture.run(line);
                assert!(base_class(&verdict), "{line}: {verdict:?}");
                assert!(reasons(&verdict).contains("safety net"), "{line}");
            }
            for line in [
                "git gc",
                "git gc --no-prune",
                "git prune -n",
                "git prune --dry-run",
            ] {
                let verdict = fixture.run(line);
                assert!(to_judge(&verdict), "{line}: {verdict:?}");
            }
            assert_eq!(fixture.run("git reflog"), Verdict::Allow);
        }
    }

    #[test]
    fn a_rebase_exec_line_is_read_as_a_line_it_runs() {
        // Unpushed, in a workspace: a rebase alone is to judge.
        let fixture = Fixture::workspace();
        assert!(to_judge(&fixture.run("git rebase -x 'cargo test' main")));
        for line in [
            "git rebase -x 'git push -f' main",
            "git rebase -x'git push --force' main",
            "git rebase --exec 'git push -f' main",
            "git rebase --exec='git push -f' main",
            "git rebase --exe='git push -f' main",
            "git rebase -i -x 'cargo test' -x 'git push -f' main",
            "git rebase -x 'sudo ls' main",
            "git rebase -x 'env GIT_CONFIG_COUNT=0 cargo test' main",
        ] {
            let verdict = fixture.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
        for line in [
            "git rebase -x 'cargo test && git push -f' main",
            "git rebase --exec='make; git push -f' main",
        ] {
            let verdict = fixture.run(line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
            assert!(reasons(&verdict).contains("shell"), "{line}");
        }
    }

    #[test]
    fn a_program_in_the_user_s_bin_folders_is_not_taken_for_installed() {
        let mut fixture = Fixture::workspace();
        let home = fixture.grant.place.home.clone().unwrap();
        for bin in [".local/bin", "bin"] {
            std::fs::create_dir_all(home.join(bin)).unwrap();
        }
        installed(&home.join(".local/bin/ls"));
        installed(&home.join("bin/cat"));
        let system = PathBuf::from(&fixture.search);
        fixture.search =
            std::env::join_paths([home.join(".local/bin"), home.join("bin"), system]).unwrap();
        for line in ["ls src", "cat src/a.rs"] {
            let verdict = fixture.run(line);
            assert!(to_judge(&verdict), "{line}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("agent's own file"),
                "{line}: {verdict:?}"
            );
        }
        // The system's own, found after them, still is.
        assert_eq!(fixture.run("wc src/a.rs"), Verdict::Allow);
    }

    #[test]
    fn an_agent_setting_the_project_s_rules_is_base() {
        let fixture = Fixture::workspace();
        let verdict = fixture.judge(
            "project",
            json!({"action": "set_rules", "text": "Push when done."}),
            Some(&fixture.never),
        );
        assert!(base_class(&verdict), "{verdict:?}");
        assert!(matches!(&verdict, Verdict::Ask(ask) if ask.kind == Kind::Action));
        assert!(reasons(&verdict).contains("every agent created after"));
    }

    #[test]
    fn writing_through_a_process_s_cwd_fd_or_exe_in_proc_is_refused() {
        let mut fixture = Fixture::workspace();
        let me = std::process::id();
        fixture.groups = vec![unsafe { libc::getpgid(0) } as u32];
        let home = fixture.grant.place.home.clone().unwrap();
        // The daemon's folder is `/`: through it, any path of the disk.
        let through_cwd = format!("/proc/self/cwd{}", home.join(".bashrc").display());
        for path in [
            through_cwd.clone(),
            format!("/proc/1/cwd{}", home.join("notes").display()),
            format!("/proc/{me}/cwd/judge.md"),
            "/proc/self/fd/3".into(),
            format!("/proc/{me}/exe"),
            format!("/proc/{me}/map_files/x"),
        ] {
            for tool in WRITES {
                let verdict = fixture.file(tool, &path);
                assert!(denied(&verdict), "{tool} {path}: {verdict:?}");
            }
            // Reading there stays base.
            let verdict = fixture.file("fs_read", &path);
            assert!(base_class(&verdict), "{path}: {verdict:?}");
        }
        let verdict = fixture.run(&format!("cp README.md {through_cwd}"));
        assert!(denied(&verdict), "{verdict:?}");
        let verdict = fixture.run(&format!("cat {through_cwd}"));
        assert!(base_class(&verdict), "{verdict:?}");
    }

    #[test]
    fn git_config_global_or_a_setting_that_runs_a_program_later_is_refused() {
        let workspace = Fixture::workspace();
        for line in [
            "git config --global user.name x",
            "git config --system core.editor vi",
            "git config core.hooksPath /ws/h",
            "git config set core.hooksPath /ws/h",
            "git config alias.x '!touch y'",
            "git config include.path /ws/x",
            "git config includeIf.gitdir:/ws/.path /ws/x",
            "git config core.fsmonitor /ws/x",
            "git config core.sshCommand /ws/x",
            "git config credential.helper /ws/x",
            "git config diff.x.textconv /ws/x",
            "git config filter.x.clean /ws/x",
            "git --git-dir=.git config core.pager /ws/x",
        ] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
        // Reading it, and settings that run nothing, are what they were.
        assert!(!denied(
            &workspace.run("git config --global --get user.name")
        ));
        assert!(to_judge(
            &workspace.run("git config user.email a@example.invalid")
        ));
        assert!(base_class(
            &workspace.run("git config remote.origin.url /x")
        ));
    }

    #[test]
    fn systemctl_and_loginctl_that_install_or_start_a_unit_are_refused() {
        let workspace = Fixture::workspace();
        for line in [
            "systemctl --user link /ws/x.service",
            "systemctl --user start x",
            "systemctl --user enable --now x",
            "systemctl --user edit x",
            "systemctl --user set-environment A=b",
            "systemctl --user import-environment",
            "systemctl --user restart flottille",
            "loginctl enable-linger",
        ] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
        for line in [
            "systemctl --user status x",
            "systemctl is-active x",
            "systemctl --user list-units",
        ] {
            assert!(!denied(&workspace.run(line)), "{line}");
        }
    }

    #[test]
    fn a_container_s_bind_mount_of_a_login_or_flottille_s_folders_is_refused() {
        let workspace = Fixture::workspace();
        let home = workspace.grant.place.home.clone().unwrap();
        let ssh = home.join(".ssh").display().to_string();
        let own = workspace.own[0].display().to_string();
        let bashrc = home.join(".bashrc").display().to_string();
        for line in [
            format!("docker run -v {ssh}:/k alpine cat /k/id_ed25519"),
            format!("docker run --volume {ssh}:/k:ro alpine true"),
            format!("docker run --volume={ssh}:/k alpine true"),
            format!("docker run -v{ssh}:/k alpine true"),
            format!("docker run --mount type=bind,source={ssh},target=/k alpine true"),
            format!("docker run --mount=type=bind,src={ssh},dst=/k alpine true"),
            format!("docker container create -v {ssh}:/k alpine"),
            format!("podman run -v {own}:/x alpine true"),
            format!("nerdctl run -v {bashrc}:/x alpine true"),
            "docker run -v /:/host alpine true".into(),
        ] {
            assert!(denied(&workspace.run(&line)), "{line}");
        }
        // Containers themselves are not refused.
        for line in [
            "docker run -v data:/x alpine true",
            "docker run -v ./src:/src alpine ls /src",
            "docker ps",
        ] {
            assert!(!denied(&workspace.run(line)), "{line}");
        }
    }

    #[test]
    fn a_program_that_prints_a_login_is_refused() {
        let workspace = Fixture::workspace();
        for line in [
            "aws configure get aws_secret_access_key",
            "aws configure export-credentials",
            "kubectl config view --raw",
            "gcloud auth print-access-token",
            "gcloud auth application-default print-access-token",
            "gcloud config config-helper",
            "az account get-access-token",
            "secret-tool search --all service x",
            "secret-tool lookup service x",
            "pass show web/x",
            "pass web/x",
            "gopass show web/x",
            "op read op://vault/item/password",
            "bw get password x",
            "rbw get x",
            "git credential fill",
            "docker-credential-pass get",
        ] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
        for line in ["aws s3 ls", "kubectl config view", "gcloud auth list"] {
            assert!(!denied(&workspace.run(line)), "{line}");
        }
    }

    #[test]
    fn a_launcher_behind_any_known_wrapper_is_refused() {
        let workspace = Fixture::workspace();
        for line in [
            "ionice -c3 at -f job now",
            "ionice -c 3 batch",
            "chrt 10 at now",
            "chrt -f 10 at now",
            "taskset 1 at now",
            "taskset -c 0,1 at now",
            "chronic at now",
            "timeout 5 batch",
            "stdbuf -o0 at now",
            "nohup at now",
            "nice -n 5 at now",
            "env A=b at now",
        ] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
    }

    #[test]
    fn a_desktop_or_terminal_that_starts_a_program_is_refused() {
        let workspace = Fixture::workspace();
        for line in [
            "hyprctl dispatch exec touch x",
            "swaymsg exec touch x",
            "i3-msg exec touch x",
            "niri msg action spawn -- touch x",
            "kitty @ launch touch x",
            "kitten @ launch touch x",
            "wezterm cli spawn touch x",
        ] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
    }

    #[test]
    fn writing_the_user_s_programs_or_desktop_configuration_is_refused() {
        let workspace = Fixture::workspace();
        let home = workspace.grant.place.home.clone().unwrap();
        for entry in [
            ".local/bin/x",
            "bin/x",
            ".local/share/systemd/user/x.service",
            ".config/hypr/hyprland.conf",
            ".config/sway/config",
            ".config/niri/config.kdl",
        ] {
            let path = home.join(entry).display().to_string();
            let verdict = workspace.file("fs_write", &path);
            assert!(denied(&verdict), "{entry}: {verdict:?}");
            assert!(
                reasons(&verdict).contains("startup"),
                "{entry}: {verdict:?}"
            );
            assert!(
                denied(&workspace.run(&format!("cp README.md {path}"))),
                "{entry}"
            );
        }
        // The daemon's own executable, wherever it is.
        let exe = std::env::current_exe().unwrap().display().to_string();
        assert!(denied(&workspace.file("fs_write", &exe)));
        assert!(denied(&workspace.run(&format!("cp README.md {exe}"))));
    }

    #[test]
    fn copying_to_a_host_or_naming_git_s_remote_programs_is_refused() {
        let workspace = Fixture::workspace();
        for line in [
            "rsync -a src/ localhost:backup",
            "rsync -a user@host:/x src/",
            "rsync -e /ws/x -a src/ dst/",
            "rsync --rsh=/ws/x src/ dst/",
            "rsync --rsync-path=/ws/x src/ dst/",
            "git clone --upload-pack=/ws/x /r y",
            "git clone -u /ws/x /r y",
            "git fetch --upload-pack /ws/x origin",
            "git push --receive-pack=/ws/x origin",
            "git push --exec=/ws/x origin",
            "git archive --remote=/r --exec=/ws/x HEAD",
            "git -c core.sshCommand=/ws/x fetch",
        ] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
        assert!(!denied(&workspace.run("rsync -a src/ dst/")));
    }

    #[test]
    fn an_option_s_value_or_a_text_word_naming_the_root_is_held_not_refused() {
        let workspace = Fixture::workspace();
        for line in [
            "awk -F/ '{print $1}' src/a",
            "cut -d/ -f1 src/a",
            "tr / _",
            "tool --base=/",
        ] {
            let verdict = workspace.run(line);
            assert!(base_class(&verdict), "{line}: {verdict:?}");
        }
        // Naming the root to write there is still refused.
        for line in ["rm -rf /", "find / -delete", "chmod -R 000 /"] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
    }

    #[test]
    fn a_launcher_word_is_refused_only_where_it_would_run() {
        let workspace = Fixture::workspace();
        for line in [
            "cargo test tmux --lib",
            "git commit -m ssh -m body",
            "git commit -m at",
            "cargo run --example screen --release",
            "cargo test -- ssh",
            "grep -r systemd-run src",
        ] {
            assert!(!denied(&workspace.run(line)), "{line}");
        }
        for line in [
            "cargo run -- systemd-run --user true",
            "uv run ssh localhost true",
            "uv run at now",
            "poetry run tmux new-session -d true",
            "npm exec -- ssh localhost",
            "nice systemd-run --user true",
        ] {
            assert!(denied(&workspace.run(line)), "{line}");
        }
    }
}
