//! The gate every tool call goes through, once, before it is performed (`docs/design.md` §7,
//! `docs/tools.md`, "The gate"). It does not grant rights: it puts each call in one of three
//! classes. **Harmless** calls pass; **to judge** calls make a card in Ask and go to the judge in
//! Auto (`judge.rs`); **base** calls always make a card, with a strong warning. What it refuses
//! is not a permission but a call that cannot be made as it is: shell syntax, a malformed call,
//! a login printed, the daemon driven from a command.

use std::path::{Path, PathBuf};

use flottille_core::approval::Kind;
use flottille_core::project::CatalogueCommand;
use serde_json::Value;

use crate::mcp::Grant;
use crate::place::Resolved;

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
    /// The catalogue command a run names, with its name.
    pub command: Option<(&'a str, &'a CatalogueCommand)>,
    /// The project's never list; `None` when the project's configuration cannot be read.
    pub never: Option<&'a [Vec<String>]>,
    /// Flottille's own folders (its data, configuration, state): sensitive places.
    pub own: &'a [PathBuf],
    /// The workspace's branch is on a remote: rewriting its history rewrites what others may
    /// have. Always false in the project folder, where every rewrite is base anyway.
    pub pushed: bool,
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

/// Programs that are shells: a catalogue command running one is judged, and the line one runs
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
const READS: [&str; 40] = [
    "ls", "pwd", "cat", "head", "tail", "wc", "grep", "rg", "egrep", "fgrep", "find", "fd", "stat",
    "file", "tree", "du", "df", "ps", "pgrep", "pidof", "which", "whoami", "uname", "date", "echo",
    "diff", "cmp", "basename", "dirname", "realpath", "readlink", "id", "hostname", "uptime",
    "free", "less", "more", "true", "test", "lsof",
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
        } else {
            found.judge(crate::actions::asked(family));
        }
        return found.verdict(Kind::Action);
    }
    match call.tool {
        "forge_write" => forge_write(call),
        "workspace_remove" => Found {
            base: Vec::new(),
            judge: vec![
                "it removes the agent's workspace and deletes its branches that hold nothing of \
                 their own"
                    .into(),
            ],
        }
        .verdict(Kind::Call),
        "commands_run" => run(call),
        tool if FILES.contains(&tool) => files(call),
        // Reads, the tree's tools, the outputs of the agent's own runs.
        _ => Verdict::Allow,
    }
}

/// A file tool: reading inside the place, and writing in a workspace, are harmless.
fn files(call: &Call) -> Verdict {
    let write = WRITES.contains(&call.tool);
    let mut found = Found::default();
    for path in call.paths {
        if write && in_git(path) {
            found.base(format!(
                "{} is in a .git folder, which git keeps: writing there can rewrite the \
                 repository or run code at its next command",
                path.shown
            ));
        }
        if let Some(place) = sensitive(call.own, path) {
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

/// `forge_write`: a malformed call is refused; merging what the checks or a review block is
/// base, since it is the user's own decision; the rest is to judge.
fn forge_write(call: &Call) -> Verdict {
    let action = call.args["action"].as_str().unwrap_or_default();
    if !["pr-create", "pr-merge-squash", "pipeline-rerun"].contains(&action) {
        return Verdict::Deny(format!(
            "refused: forge_write has no action {action:?}: its actions are pr-create, \
             pr-merge-squash and pipeline-rerun"
        ));
    }
    let mut found = Found::default();
    found.judge(match action {
        "pr-create" => "it opens a pull request on the forge".into(),
        "pr-merge-squash" => "it merges a pull request on the forge, by squash".into(),
        _ => "it re-runs the failed jobs of a pipeline on the forge".into(),
    });
    if let Some(waived) = call.args.get("waive").filter(|w| !w.is_null()) {
        if action != "pr-merge-squash" {
            return Verdict::Deny(
                "refused: waive is for pr-merge-squash only: it merges a pull request whose \
                 checks or review block it, once the user allows it"
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

/// `commands_run`: a catalogue command in a workspace, and a known read-only line inside the
/// place, are harmless.
fn run(call: &Call) -> Verdict {
    if call.words.is_empty() {
        // A line that cannot be split is shell syntax; a command the catalogue does not hold is
        // refused by the tool itself.
        if call.command.is_none()
            && let Some(line) = call.args["line"].as_str()
            && let Err(why) = flottille_core::command::split_line(line)
        {
            return Verdict::Deny(why);
        }
        return Verdict::Allow;
    }
    let layers = match layers(call.words) {
        Ok(layers) => layers,
        Err(why) => return Verdict::Deny(why),
    };
    if let Some(why) = layers.iter().find_map(|layer| refusal(layer)) {
        return Verdict::Deny(why);
    }
    let main = call.grant.place.workspace.is_none();
    let mut found = Found::default();
    match call.never {
        Some(never) => {
            let catalogue = call.command.map(|(name, _)| name);
            if let Some(entry) = never.iter().find(|entry| {
                (entry.len() == 1 && catalogue == Some(entry[0].as_str()))
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
        .filter_map(|word| named_paths(word))
        .find(|named| {
            Path::new(named)
                .components()
                .any(|p| p.as_os_str() == ".git")
        });
    if let Some(word) = git_word {
        found.base(format!("{word} reaches a .git folder, which git keeps"));
    }
    let reached = reached(call, &layers);
    for path in &reached {
        if in_git(path) {
            found.base(format!(
                "{} is in a .git folder, which git keeps",
                path.shown
            ));
        }
        if let Some(place) = sensitive(call.own, path) {
            found.base(format!("{} is a sensitive place ({place})", path.shown));
        } else if !path.inside {
            found.judge(format!("{} is outside this agent's place", path.shown));
        }
    }
    if let Some(why) = deletion(call, &layers) {
        found.base(why);
    }
    let mut reads = layers.len() == 1;
    for layer in &layers {
        match program(layer, main, call.pushed) {
            Line::Reads | Line::Files => {}
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
    if let Some((name, command)) = call.command {
        if command.ask_before_running {
            found.judge(format!(
                "the catalogue command {name} is marked to ask before it runs"
            ));
        }
        if let Some(shell) = call.words.first().filter(|w| SHELLS.contains(&base(w))) {
            found.judge(format!(
                "the catalogue command {name} runs a shell ({}), whose line the gate does not read",
                base(shell)
            ));
        }
        if main {
            found.judge(format!(
                "the catalogue command {name} runs in the project folder (main), the user's own \
                 checkout, not in a workspace"
            ));
        }
    } else if !reads {
        found.judge(format!(
            "{} is not a command the rules know to only read",
            shown_words(call.words)
        ));
    }
    found.verdict(Kind::Call)
}

/// What a layer of a line does, by its program.
enum Line {
    /// It only reads.
    Reads,
    /// It writes the files of the workspace, and nothing else: harmless.
    Files,
    /// It writes, in a way the rules have nothing to say about: harmless only as a catalogue
    /// command in a workspace.
    Writes,
    Judge(String),
    Base(String),
}

/// What is refused, whatever the user would say: a login printed, the daemon driven from a
/// command. These are not permissions; such a call cannot be made at all.
fn refusal(layer: &[String]) -> Option<String> {
    let program = base(&layer[0]);
    let args = &layer[1..];
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
        _ => None,
    }
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

/// What a layer's program does.
fn program(layer: &[String], main: bool, pushed: bool) -> Line {
    let program = base(&layer[0]);
    let args = &layer[1..];
    let any = |names: &[&str]| args.iter().any(|w| names.contains(&w.as_str()));
    match program {
        "git" => git(layer, main, pushed),
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
        "find" if find_writes(args) => Line::Writes,
        _ if READS.contains(&program) => Line::Reads,
        _ => Line::Writes,
    }
}

/// Whether a `find` does more than list: deletes, runs a command or writes a file.
fn find_writes(args: &[String]) -> bool {
    args.iter().any(|word| {
        [
            "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf",
            "-fls",
        ]
        .contains(&word.as_str())
    })
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

/// What a git line does: its reads are harmless, adding or moving files in a workspace too;
/// forcing a push, rewriting a pushed branch and discarding the work of the user's checkout are
/// base; the rest is to judge.
fn git(layer: &[String], main: bool, pushed: bool) -> Line {
    let args = &layer[1..];
    let mut at = 0;
    let mut options = Vec::new();
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        if !word.starts_with('-') {
            break;
        }
        if (word.starts_with("-c") && !word.starts_with("--")) || word.starts_with("--config-env") {
            return Line::Judge(
                "git -c and --config-env change git's configuration for this one command".into(),
            );
        }
        if [
            "-C",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "--super-prefix",
            "--attr-source",
            "--list-cmds",
        ]
        .contains(&word)
        {
            at += 2;
        } else {
            at += 1;
        }
        if !["-C", "--no-pager", "-P", "--paginate"].contains(&word) {
            options.push(word);
        }
    }
    let Some(command) = args.get(at).map(String::as_str) else {
        return Line::Reads;
    };
    let rest = &args[at + 1..];
    if let Some(option) = options.first() {
        return Line::Judge(format!(
            "git {option} points git at another repository or changes how it runs"
        ));
    }
    if main && let Some(why) = discards_work(layer) {
        return Line::Base(why);
    }
    let any = |names: &[&str]| rest.iter().any(|w| names.contains(&w.as_str()));
    // Rewriting a branch others may have, or the user's own, is base.
    let rewrite = |what: String| {
        if main || pushed {
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
    match command {
        _ if GIT_READS.contains(&command) => Line::Reads,
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
        "remote" if remote_reads(rest) => Line::Reads,
        "symbolic-ref" if !symbolic_ref_writes(rest) => Line::Reads,
        "replace" if replace_reads(rest) => Line::Reads,
        "notes" if notes_reads(rest) => Line::Reads,
        "stash" if matches!(rest.first().map(String::as_str), Some("list" | "show")) => Line::Reads,
        "worktree" if rest.first().is_some_and(|w| w == "list") => Line::Reads,
        "reflog" if matches!(rest.first().map(String::as_str), None | Some("show")) => Line::Reads,
        // Writing the files of a workspace, through git.
        "add" | "mv" | "rm" if !main => Line::Files,
        _ => Line::Judge(format!(
            "git {command} changes the repository, its branches or a remote"
        )),
    }
}

/// Whether a line pushes with git, at any of its layers: the run checks its remote first.
pub fn pushes(words: &[String]) -> bool {
    layers(words).is_ok_and(|layers| {
        layers
            .iter()
            .any(|layer| base(&layer[0]) == "git" && subcommand(&layer[1..]) == Some("push"))
    })
}

/// git's command, past its global options.
fn subcommand(args: &[String]) -> Option<&str> {
    let mut at = 0;
    while let Some(word) = args.get(at) {
        if !word.starts_with('-') {
            return Some(word);
        }
        at += match word.as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix"
            | "--attr-source" | "--config-env" => 2,
            _ => 1,
        };
    }
    None
}

/// A push: forcing it or deleting on the remote is base, anything else to judge.
fn push(rest: &[String]) -> Line {
    let mut options = true;
    for word in rest {
        let word = word.as_str();
        if options && word == "--" {
            options = false;
            continue;
        }
        let forced = if options && word.starts_with("--") {
            word.starts_with("--force") || ["--mirror", "--delete", "--prune"].contains(&word)
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
        "nohup" | "command" => from(skip(args, &[], false)),
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

/// The paths a call reaches: those its tool resolved and, for a run, every word of every layer
/// that can name one, resolved from the folder it runs in (its program is wherever it is
/// installed).
fn reached(call: &Call, layers: &[Vec<String>]) -> Vec<Resolved> {
    let mut reached = call.paths.to_vec();
    let Some(folder) = call.paths.first().filter(|_| call.tool == "commands_run") else {
        return reached;
    };
    for word in layers.iter().flat_map(|layer| &layer[1..]) {
        if let Some(named) = named_paths(word) {
            let path = call.grant.place.resolve(&folder.path, named);
            if !reached.contains(&path) {
                reached.push(path);
            }
        }
    }
    reached
}

/// What a word of a line can name as a path: itself, an option's value after `=`, an
/// assignment's value.
fn named_paths(word: &str) -> Option<&str> {
    if word.starts_with('-') {
        return word
            .split_once('=')
            .map(|(_, value)| value)
            .filter(|v| !v.is_empty());
    }
    if assignment(word) {
        return word
            .split_once('=')
            .map(|(_, value)| value)
            .filter(|v| !v.is_empty());
    }
    Some(word).filter(|w| !w.is_empty())
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
    for layer in layers {
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
/// is taken for a write: a push and `forge_write` trust where the remote leads.
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

/// Why a git line discards or rewrites the work of the user's own checkout, when it does: in the
/// project folder, that is base.
fn discards_work(layer: &[String]) -> Option<String> {
    let args = &layer[1..];
    let (at, _) = git_subcommand(args);
    let subcommand = args.get(at)?.as_str();
    let rest = &args[at + 1..];
    let any = |names: &[&str]| rest.iter().any(|word| names.contains(&word.as_str()));
    let discards = match subcommand {
        "checkout" => any(&["--", ".", "-f", "--force"]),
        "restore" | "clean" | "reset" => true,
        "stash" => !matches!(rest.first().map(String::as_str), Some("list" | "show")),
        "worktree" => rest.first().is_some_and(|word| word == "remove"),
        "branch" => rest.iter().any(|word| {
            word == "--delete"
                || (!word.starts_with("--") && word.starts_with('-') && word.contains(['d', 'D']))
        }),
        _ => false,
    };
    discards.then(|| {
        format!(
            "{} discards or rewrites the work in the user's own checkout (main)",
            shown_words(layer)
        )
    })
}

/// Where git's subcommand is among its arguments, and whether nothing but `--no-pager` comes
/// before it.
fn git_subcommand(args: &[String]) -> (usize, bool) {
    let mut at = 0;
    let mut plain = true;
    while let Some(word) = args.get(at) {
        if !word.starts_with('-') {
            break;
        }
        if word != "--no-pager" {
            plain = false;
        }
        at += if ["-C", "-c", "--git-dir", "--work-tree", "--namespace"].contains(&word.as_str()) {
            2
        } else {
            1
        };
    }
    (at, plain)
}

/// The sensitive place a path is in, if it is in one: inside the place, only what sits below it
/// counts.
pub fn sensitive(own: &[PathBuf], path: &Resolved) -> Option<String> {
    let parts: Vec<String> = if path.inside {
        Path::new(&path.shown)
    } else {
        if own.iter().any(|own| path.path.starts_with(own)) {
            return Some("Flottille's own folder".into());
        }
        if let Some(system) = SYSTEM_SENSITIVE.iter().find(|s| path.path.starts_with(s)) {
            return Some((*system).into());
        }
        path.path.as_path()
    }
    .components()
    .map(|part| part.as_os_str().to_string_lossy().into_owned())
    .collect();
    for (at, part) in parts.iter().enumerate() {
        if part == ".env"
            || (part.starts_with(".env.") && !TEMPLATES.iter().any(|t| part.ends_with(t)))
        {
            return Some(".env".into());
        }
        for place in SENSITIVE {
            let names: Vec<&str> = place.split('/').collect();
            if parts.len() - at >= names.len()
                && names.iter().zip(&parts[at..]).all(|(n, p)| n == p)
            {
                return Some(format!("~/{place}"));
            }
        }
    }
    None
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
        commands: BTreeMap<String, CatalogueCommand>,
        pushed: bool,
    }

    impl Fixture {
        fn new(workspace: Option<&str>) -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let base = std::fs::canonicalize(dir.path()).unwrap();
            let root = base.join("work/app");
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::create_dir_all(base.join("home")).unwrap();
            let grant = Grant {
                agent: "a1".into(),
                epoch: 0,
                tools: vec![
                    "fs_read",
                    "fs_list",
                    "search",
                    "fs_write",
                    "fs_edit",
                    "commands_list",
                    "commands_run",
                    "commands_status",
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

        fn command(mut self, name: &str, line: &str, set: impl Fn(&mut CatalogueCommand)) -> Self {
            let mut command = CatalogueCommand {
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
                "commands_run" => {
                    let (line, command) = match args["command"].as_str() {
                        Some(name) => {
                            let (name, command) = self.commands.get_key_value(name).unwrap();
                            (command.line.clone(), Some((name.as_str(), command)))
                        }
                        None => (args["line"].as_str().unwrap().to_owned(), None),
                    };
                    let words = flottille_core::command::split_line(&line).unwrap_or_default();
                    (words, vec![place.resolve(&self.root, ".")], command)
                }
                _ => match args["path"].as_str() {
                    Some(path) => (Vec::new(), vec![place.resolve(&self.root, path)], None),
                    None => (Vec::new(), Vec::new(), None),
                },
            };
            decide(&Call {
                grant: &self.grant,
                tool,
                args: &args,
                words: &words,
                paths: &paths,
                command,
                never,
                own: &self.own,
                pushed: self.pushed,
            })
        }

        fn run(&self, line: &str) -> Verdict {
            self.judge("commands_run", json!({"line": line}), Some(&self.never))
        }

        fn catalogue(&self, name: &str) -> Verdict {
            self.judge("commands_run", json!({"command": name}), Some(&self.never))
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
            for tool in [
                "commands_list",
                "commands_status",
                "forge_read",
                "agent_status",
            ] {
                assert_eq!(
                    fixture.judge(tool, json!({}), Some(&fixture.never)),
                    Verdict::Allow
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
            ] {
                assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
            }
        }
    }

    #[test]
    fn harmless_a_catalogue_command_in_a_workspace_passes() {
        let fixture = Fixture::workspace().command("test", "cargo test", |_| {});
        assert_eq!(fixture.catalogue("test"), Verdict::Allow);
    }

    #[test]
    fn to_judge_writing_in_main_is_judged() {
        let fixture = Fixture::main();
        assert!(to_judge(&fixture.file("fs_write", "src/a.rs")));
        assert!(to_judge(&fixture.file("fs_edit", "src/a.rs")));
        let fixture = fixture.command("test", "cargo test", |_| {});
        assert!(to_judge(&fixture.catalogue("test")));
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
                "git -c core.pager=less log",
                "git --git-dir=other log",
                "cargo build",
                "npm run dev",
                "curl https://example.com",
                "./script.sh",
                "sed -i s/a/b/ src/a.rs",
                "gh pr create --fill",
                "env ls",
                "sudo ls",
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
    fn to_judge_a_marked_or_shell_catalogue_command_is_judged_even_in_a_workspace() {
        let fixture = Fixture::workspace()
            .command("deploy-check", "cargo check", |c| {
                c.ask_before_running = true
            })
            .command("script", "sh run.sh", |_| {});
        assert!(to_judge(&fixture.catalogue("deploy-check")));
        assert!(to_judge(&fixture.catalogue("script")));
    }

    #[test]
    fn to_judge_forge_writes_and_workspace_removal_are_judged() {
        let fixture = Fixture::workspace();
        for action in ["pr-create", "pr-merge-squash", "pipeline-rerun"] {
            let verdict = fixture.judge(
                "forge_write",
                json!({"action": action, "repository": "."}),
                Some(&fixture.never),
            );
            assert!(to_judge(&verdict), "{action}");
        }
        assert!(to_judge(&fixture.judge(
            "workspace_remove",
            json!({}),
            Some(&fixture.never)
        )));
    }

    #[test]
    fn to_judge_the_window_actions_of_an_agent_are_judged_and_a_tracker_change_is_base() {
        let fixture = Fixture::main();
        let verdict = fixture.judge(
            "procedure",
            json!({"action": "start", "name": "ship", "workspace": "w"}),
            Some(&fixture.never),
        );
        assert!(to_judge(&verdict), "{verdict:?}");
        assert!(matches!(verdict, Verdict::Ask(ask) if ask.kind == Kind::Action));
        let tracker = fixture.judge(
            "project",
            json!({"action": "set_settings", "tracker": null}),
            Some(&fixture.never),
        );
        assert!(base_class(&tracker), "{tracker:?}");
        for reading in [
            json!({"action": "list"}),
            json!({"action": "show", "name": "ship"}),
        ] {
            assert_eq!(
                fixture.judge("procedure", reading, Some(&fixture.never)),
                Verdict::Allow
            );
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
            fixture.file("fs_read", "../../home/.ssh/id_ed25519"),
            fixture.file("fs_read", "../../xdg/data/flottille/store.db"),
            fixture.run("rm -rf .git"),
            fixture.run("rm -r ."),
            fixture.run("cat .git/config"),
            fixture.run("cat ../../home/.aws/credentials"),
            fixture.run("cargo publish"),
            fixture.run("npm publish"),
            fixture.run("gem push x.gem"),
            fixture.run("docker compose down"),
            fixture.run("nice deploy now"),
            fixture.run("bkt pr list"),
            fixture.run("acli jira workitem view ABC-1"),
            fixture.judge(
                "forge_write",
                json!({"action": "pr-merge-squash", "repository": ".", "waive": "ship it"}),
                Some(&fixture.never),
            ),
        ] {
            assert!(base_class(&verdict), "{verdict:?}");
        }
        assert_eq!(fixture.file("fs_read", ".env.example"), Verdict::Allow);
    }

    #[test]
    fn base_a_project_whose_never_list_cannot_be_read_makes_a_run_base() {
        let fixture = Fixture::workspace();
        assert!(base_class(&fixture.judge(
            "commands_run",
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
                "forge_write",
                json!({"action": "pr-delete", "repository": "."}),
                Some(&fixture.never),
            ),
            fixture.judge(
                "forge_write",
                json!({"action": "pr-create", "repository": ".", "waive": "x"}),
                Some(&fixture.never),
            ),
        ] {
            assert!(denied(&verdict), "{verdict:?}");
        }
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
}
