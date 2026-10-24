//! The gate every tool call goes through, once, before it is performed (`docs/design.md` §7,
//! `docs/reference/hemera.md` §4, without the remote judge).

use std::path::{Path, PathBuf};

use flottille_core::project::CatalogueCommand;
use serde_json::{Value, json};

use crate::mcp::Grant;
use crate::place::Resolved;

/// A call, as the gate sees it.
pub struct Call<'a> {
    /// The agent that makes it, its place and its tools.
    pub grant: &'a Grant,
    pub tool: &'a str,
    pub args: &'a Value,
    /// The words of the line a run would start, split without a shell; empty for any other call,
    /// and for a line that cannot be split (the tool refuses it with its own sentence).
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Allow,
    /// Refused, with the sentence that says why and what to do.
    Deny(String),
    /// Held for an answer: a request is made.
    Ask(Ask),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    /// Why, one sentence each.
    pub reasons: Vec<String>,
    /// It reaches a sensitive place: it is never allowed for a tree.
    pub sensitive: bool,
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

/// Programs that are shells: a catalogue command running one is never allowed without asking,
/// and the line one runs with `-c` is judged in its place.
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

/// The verdict on a call, in the order of `docs/reference/hemera.md` §4: the role guards, the
/// refusals, then what asks; anything else is allowed.
pub fn decide(call: &Call) -> Verdict {
    let run = call.tool == "commands_run";
    // The project folder is read-only for agents (`docs/design.md` §6.2): no project grants
    // writing there yet.
    if call.grant.place.workspace.is_none() {
        if WRITES.contains(&call.tool) {
            return Verdict::Deny(
                "refused: this agent works in the project folder, which is read-only for agents: \
                 it writes files only in a workspace"
                    .into(),
            );
        }
        if run && !call.command.is_some_and(|(_, c)| c.check && c.read_only) {
            let what = call
                .command
                .map_or("a line".to_owned(), |(name, _)| name.to_owned());
            return Verdict::Deny(format!(
                "refused: this agent works in the project folder, which is read-only for agents: \
                 it runs only the catalogue's read-only checks there, and {what} is not one"
            ));
        }
    }
    let layers = if run && !call.words.is_empty() {
        match layers(call.words) {
            Ok(layers) => layers,
            Err(why) => return Verdict::Deny(why),
        }
    } else {
        Vec::new()
    };
    let reached = reached(call, &layers);
    if let Some(why) = refusal(call, &layers, &reached) {
        return Verdict::Deny(why);
    }
    let mut ask = Ask {
        reasons: Vec::new(),
        sensitive: false,
    };
    for path in &reached {
        let reason = match sensitive(call.own, path) {
            Some(place) => {
                ask.sensitive = true;
                format!("{} is a sensitive place ({place})", path.shown)
            }
            None if !path.inside => format!("{} is outside this agent's place", path.shown),
            None => continue,
        };
        if !ask.reasons.contains(&reason) {
            ask.reasons.push(reason);
        }
    }
    if run {
        if call.never.is_none() {
            ask.reasons.push(
                "the project's configuration cannot be read, so what it never allows is not known"
                    .into(),
            );
        }
        if let Some((name, command)) = call.command {
            if command.ask_before_running {
                ask.reasons.push(format!(
                    "the catalogue command {name} is marked to ask before it runs"
                ));
            }
            if let Some(shell) = call.words.first().filter(|w| SHELLS.contains(&base(w))) {
                ask.reasons.push(format!(
                    "the catalogue command {name} runs a shell ({}), whose line the gate does not read",
                    base(shell)
                ));
            }
        }
    }
    if ask.reasons.is_empty() {
        Verdict::Allow
    } else {
        Verdict::Ask(ask)
    }
}

/// What a grant for the tree is given to: the exact call, without what changes each time it is
/// made (a write's content, a read's page, a run's timeout and every why).
pub fn identity(call: &Call) -> String {
    let paths: Vec<String> = call
        .paths
        .iter()
        .map(|path| path.path.display().to_string())
        .collect();
    let what = match call.tool {
        "commands_run" => match call.command {
            Some((name, command)) => json!({"command": name, "line": command.line}),
            None => json!({"words": call.words}),
        },
        "fs_read" | "fs_list" | "search" | "fs_write" | "fs_edit" => Value::Null,
        _ => call.args.clone(),
    };
    let identity = json!({"tool": call.tool, "paths": paths, "what": what});
    crate::files::sha256(identity.to_string().as_bytes())
}

/// The program a word names, without its folder.
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

/// The sentence refusing the call, if anything refuses it.
fn refusal(call: &Call, layers: &[Vec<String>], reached: &[Resolved]) -> Option<String> {
    if WRITES.contains(&call.tool) {
        return reached.iter().find(|path| in_git(path)).map(|path| {
            format!(
                "refused: {} is in a .git folder, which agents never write: git keeps it",
                path.shown
            )
        });
    }
    if call.tool != "commands_run" {
        return None;
    }
    if let Some(never) = call.never {
        let catalogue = call.command.map(|(name, _)| name);
        let entry = never.iter().find(|entry| {
            (entry.len() == 1 && catalogue == Some(entry[0].as_str()))
                || layers.iter().any(|layer| starts_anywhere(layer, entry))
        });
        if let Some(entry) = entry {
            return Some(format!(
                "refused: the project never allows {}: its configuration says so; do without it, \
                 or say why it is needed and the user runs it",
                entry.join(" ")
            ));
        }
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
        return Some(format!(
            "refused: {word} reaches a .git folder, and an agent's command never touches one: git \
             keeps it"
        ));
    }
    if let Some(path) = reached.iter().find(|path| in_git(path)) {
        return Some(format!(
            "refused: {} is in a .git folder, and an agent's command never touches one: git keeps \
             it",
            path.shown
        ));
    }
    layers.iter().find_map(|layer| program_refusal(layer))
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

/// What agents are never let do, by program: push, rewrite history, write to a forge, publish.
fn program_refusal(layer: &[String]) -> Option<String> {
    let program = base(&layer[0]);
    let args = &layer[1..];
    let any = |names: &[&str]| args.iter().any(|w| names.contains(&w.as_str()));
    match program {
        "git" => git_refusal(args),
        "gh" | "glab" | "bkt" => forge_refusal(program, args),
        "npm" | "pnpm" | "yarn" | "bun" | "cargo" | "poetry" | "flit" | "hatch" | "uv"
            if any(&["publish", "unpublish"]) =>
        {
            Some(publishing(layer))
        }
        "twine" if any(&["upload"]) => Some(publishing(layer)),
        "gem" | "dotnet" | "nuget" if any(&["push"]) => Some(publishing(layer)),
        _ => None,
    }
}

fn publishing(layer: &[String]) -> String {
    format!(
        "refused: {} publishes a package, and agents never publish: say what is ready, and the \
         user publishes it",
        shown_words(layer)
    )
}

/// The first words of a line, as a refusal names them.
fn shown_words(words: &[String]) -> String {
    let shown: Vec<&str> = words.iter().take(3).map(String::as_str).collect();
    let mut text = shown.join(" ");
    text.truncate(200);
    text
}

fn git_refusal(args: &[String]) -> Option<String> {
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        if (word.starts_with("-c") && !word.starts_with("--")) || word.starts_with("--config-env") {
            return Some(
                "refused: git -c and --config-env change git's configuration for one command, \
                 and agents never do: run git without them"
                    .into(),
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
        } else if word.starts_with('-') {
            at += 1;
        } else {
            break;
        }
    }
    let command = args.get(at)?.as_str();
    let rest = &args[at + 1..];
    let any = |names: &[&str]| rest.iter().any(|w| names.contains(&w.as_str()));
    let refused = match command {
        "push" | "send-pack" | "http-push" => "push",
        "subtree" | "lfs" if any(&["push"]) => "push",
        "merge" | "rebase" | "reset" | "switch" | "filter-branch" | "update-ref" => "history",
        "commit" | "cherry-pick" | "revert" | "am" => "commit",
        "tag"
            if !(rest.is_empty()
                || any(&["-l", "--list"]) && !any(&["-d", "--delete", "-f", "--force"])) =>
        {
            "tag"
        }
        "checkout" if !(rest.first().is_some_and(|w| w == "--") && rest.len() > 1) => "checkout",
        "branch"
            if rest.iter().any(|w| {
                w == "--delete"
                    || (w.starts_with('-') && !w.starts_with("--") && w.contains(['d', 'D']))
            }) =>
        {
            "branch"
        }
        "config" if !config_reads(rest) => "config",
        "symbolic-ref" if symbolic_ref_writes(rest) => "history",
        // Plumbing that writes the history or the index without `commit` or `reset`.
        "fast-import" | "update-index" => "history",
        "commit-tree" => "commit",
        "replace" if !replace_reads(rest) => "history",
        "notes" if !notes_reads(rest) => "history",
        _ => return None,
    };
    Some(match refused {
        "push" => "refused: git push is refused for agents: Flottille never lets an agent push; \
                   say what is ready, and the user pushes it"
            .into(),
        "history" => format!(
            "refused: git {command} moves a branch, rewrites its history or edits what git keeps, \
             and agents never do: say what should be done, and the user does it"
        ),
        "commit" => format!(
            "refused: git {command} makes a commit, and this project does not let agents commit: \
             leave the change in the working tree and say it is ready"
        ),
        "tag" => "refused: git tag makes or removes a tag, and agents never do: git tag -l lists \
                  them"
            .into(),
        "checkout" => "refused: git checkout changes the branch, and agents only restore files \
                       with git checkout -- <paths>"
            .into(),
        "branch" => "refused: git branch deletes a branch, and agents never do: say which branch \
                     should go, and the user removes it"
            .into(),
        _ => "refused: git config changes git's configuration, and agents only read it (--get, \
              --get-all, --get-regexp, --list)"
            .into(),
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

/// The forge command lines' reads: anything else writes to the forge.
fn forge_refusal(program: &str, args: &[String]) -> Option<String> {
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
    let read = match (program, words.as_slice()) {
        (_, ["api", ..]) => api_reads(args),
        ("gh", ["pr", "view" | "list" | "diff" | "checks" | "status"])
        | ("gh", ["issue", "view" | "list"])
        | ("gh", ["run", "view" | "list"])
        | ("glab", ["mr", "view" | "list" | "diff"])
        | ("glab", ["issue", "view" | "list"])
        | ("glab", ["ci" | "pipeline", "view" | "list" | "status"])
        | ("bkt", ["pr", "view" | "list" | "diff"])
        | ("bkt", ["issue", "view" | "list"])
        | ("bkt", ["pipeline", "view" | "list"])
        | (_, ["repo", "view"])
        | (_, ["auth", "status"]) => true,
        _ => false,
    };
    if read {
        return None;
    }
    let reads = match program {
        "gh" => "pr view, list, diff, checks and status; issue view and list; run view and list",
        "glab" => "mr view, list and diff; issue view and list; ci view, list and status",
        _ => "pr view, list and diff; issue view and list; pipeline view and list",
    };
    Some(format!(
        "refused: {program} {} writes to the forge, or is not one of its reads, and agents only \
         read it ({reads}; repo view; auth status; api with GET and no fields): say what should \
         be done, and the user does it",
        words.join(" ")
    ))
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

/// The sensitive place a path is in, if it is in one: inside the place, only what sits below it
/// counts.
fn sensitive(own: &[PathBuf], path: &Resolved) -> Option<String> {
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

    /// A workspace `w` of the project `app` in a temporary folder, with a home beside it and
    /// Flottille's own folders under `xdg`.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        own: Vec<PathBuf>,
        grant: Grant,
        never: Vec<Vec<String>>,
        commands: BTreeMap<String, CatalogueCommand>,
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
                tools: vec![
                    "fs_read",
                    "fs_list",
                    "search",
                    "fs_write",
                    "fs_edit",
                    "commands_list",
                    "commands_run",
                    "commands_output",
                    "commands_stop",
                ],
                place: Place {
                    project: "app".into(),
                    workspace: workspace.map(str::to_owned),
                    root: root.clone(),
                    repositories: vec![(".".into(), root.clone())],
                    branch: String::new(),
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
            }
        }

        fn workspace() -> Fixture {
            Fixture::new(Some("w"))
        }

        fn command(mut self, name: &str, line: &str, set: impl Fn(&mut CatalogueCommand)) -> Self {
            let mut command = CatalogueCommand {
                line: line.into(),
                repository: None,
                serve: false,
                check: false,
                read_only: false,
                ask_before_running: false,
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
                    let words = flottille_core::command::split_line(&line).unwrap();
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

        fn identity_of(&self, tool: &str, args: Value) -> String {
            let place = &self.grant.place;
            let (words, paths) = match args["line"].as_str() {
                Some(line) => (
                    flottille_core::command::split_line(line).unwrap(),
                    vec![place.resolve(&self.root, ".")],
                ),
                None => (
                    Vec::new(),
                    vec![place.resolve(&self.root, args["path"].as_str().unwrap())],
                ),
            };
            identity(&Call {
                grant: &self.grant,
                tool,
                args: &args,
                words: &words,
                paths: &paths,
                command: None,
                never: Some(&self.never),
                own: &self.own,
            })
        }
    }

    fn denied(verdict: &Verdict) -> bool {
        matches!(verdict, Verdict::Deny(why) if why.starts_with("refused: ") && why.chars().count() < 2_000)
    }

    fn asked(verdict: &Verdict, sensitive: bool) -> bool {
        matches!(verdict, Verdict::Ask(ask) if ask.sensitive == sensitive && !ask.reasons.is_empty())
    }

    #[test]
    fn each_refused_git_form_is_denied() {
        let fixture = Fixture::workspace();
        for line in [
            "git push",
            "git push origin main",
            "/usr/bin/git push origin main",
            "git -C src push",
            "git --git-dir=other push",
            "git --no-pager push --force",
            "git -c url.x.pushInsteadOf=y push",
            "git -c core.pager=less log",
            "git --config-env=core.pager=PAGER log",
            "git send-pack origin main",
            "git subtree push --prefix=lib origin main",
            "git merge topic",
            "git rebase main",
            "git reset --hard HEAD",
            "git switch main",
            "git tag v1",
            "git tag -d v1",
            "git tag -l -d v1",
            "git checkout main",
            "git checkout -b topic",
            "git branch -d topic",
            "git branch -D topic",
            "git branch --delete topic",
            "git branch -rd origin/topic",
            "git commit -m change",
            "git cherry-pick abc123",
            "git revert abc123",
            "git am change.patch",
            "git config user.name someone",
            "git config --unset user.name",
            "git config --add remote.origin.pushurl somewhere",
            "git config --get user.name --add x",
            "git config set user.name someone",
            "git symbolic-ref HEAD refs/heads/other",
            "git symbolic-ref -m reason HEAD refs/heads/other",
            "git symbolic-ref --short HEAD refs/heads/other",
            "git symbolic-ref -d HEAD",
            "git symbolic-ref --delete refs/remotes/origin/HEAD",
            "git symbolic-ref -qd HEAD",
            "env git symbolic-ref HEAD refs/heads/other",
            "git fast-import",
            "git fast-import --force",
            "git commit-tree HEAD^{tree} -m change",
            "git update-index --add --cacheinfo 100644,abc,a",
            "git update-index --refresh",
            "git update-ref refs/heads/main abc123",
            "git replace abc123 def456",
            "git replace -d abc123",
            "git replace --graft abc123",
            "git replace -l -d abc123",
            "git notes add -m note",
            "git notes append -m note",
            "git notes edit",
            "git notes remove HEAD",
            "git notes copy a b",
            "git notes merge topic",
            "git notes prune",
            "git notes --ref=review add -m note",
            "git notes --ref review add -m note",
            "git notes -m note",
            "sh -c 'git fast-import'",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
    }

    #[test]
    fn git_reads_and_local_work_are_allowed() {
        let fixture = Fixture::workspace();
        for line in [
            "git status",
            "git log --oneline",
            "git diff HEAD",
            "git add src",
            "git stash",
            "git tag",
            "git tag -l",
            "git tag --list 'v*'",
            "git checkout -- src/a.rs",
            "git branch",
            "git branch -a",
            "git config --get user.name",
            "git config --get-all remote.origin.url",
            "git config --get-regexp ^remote",
            "git config --list",
            "git config -l",
            "git config get user.name",
            "git config list",
            "git symbolic-ref HEAD",
            "git symbolic-ref --short HEAD",
            "git symbolic-ref -q HEAD",
            "git symbolic-ref -q --short HEAD",
            "git replace",
            "git replace -l",
            "git replace --list 'abc*'",
            "git replace -l --format=long",
            "git notes",
            "git notes list",
            "git notes list HEAD",
            "git notes show HEAD",
            "git notes --ref=review show HEAD",
            "git notes get-ref",
            "git -C src status",
            "git fetch origin",
        ] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn each_forge_write_is_denied_and_its_reads_are_allowed() {
        let fixture = Fixture::workspace();
        for line in [
            "gh pr create --fill",
            "gh pr merge 1",
            "gh pr comment 1 --body hi",
            "gh issue create",
            "gh repo delete o/r",
            "gh release create v1",
            "gh api -X POST repos/o/r/issues",
            "gh api --method=PATCH repos/o/r",
            "gh api repos/o/r/issues -f title=x",
            "gh api repos/o/r/issues --field title=x",
            "gh api repos/o/r/issues --input body.json",
            "gh api graphql",
            "gh",
            "glab mr create",
            "glab api -X DELETE projects/1",
            "bkt pr create",
            "env gh pr merge 1",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        for line in [
            "gh pr view 1",
            "gh pr list",
            "gh pr diff 1",
            "gh pr checks 1",
            "gh pr status",
            "gh issue view 2",
            "gh issue list --state open",
            "gh run view 3",
            "gh run list",
            "gh repo view",
            "gh auth status",
            "gh api repos/o/r/pulls",
            "gh api -X GET repos/o/r",
            "gh -R o/r pr view 1",
            "gh pr view 1 --repo o/r",
            "glab mr view 1",
            "glab mr list",
            "glab ci status",
            "bkt pr list",
        ] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn each_publishing_command_is_denied() {
        let fixture = Fixture::workspace();
        for line in [
            "npm publish",
            "npm publish --access public",
            "pnpm publish",
            "yarn npm publish",
            "bun publish",
            "cargo publish",
            "poetry publish",
            "flit publish",
            "hatch publish",
            "uv publish",
            "twine upload dist",
            "gem push x.gem",
            "dotnet nuget push x.nupkg",
            "nuget push x.nupkg",
            "npm unpublish x",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        for line in ["npm run build", "cargo test", "echo publish", "gem list"] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn the_wrappers_are_unwrapped() {
        let fixture = Fixture::workspace();
        for line in [
            "env git push",
            "env -u GIT_CONFIG_COUNT git push",
            "env --unset=GIT_CONFIG_COUNT git push",
            "env -i HOME=src git push",
            "env -C src git push",
            "env -S 'git push origin'",
            "env --split-string='git push'",
            "env -- git push",
            "sudo git push",
            "sudo -u root -E git push",
            "sudo --user=root git push",
            "doas -u root git push",
            "nice git push",
            "nice -n 5 git push",
            "nice -5 git push",
            "nohup git push",
            "timeout 10 git push",
            "timeout -s KILL -k 5 10 git push",
            "timeout --signal=TERM 10s git push",
            "time git push",
            "time -p git push",
            "stdbuf -oL git push",
            "stdbuf -o L git push",
            "npx git push",
            "npx -y --package=x git push",
            "npx -c 'git push'",
            "pnpm dlx git push",
            "pnpm exec git push",
            "yarn dlx git push",
            "yarn exec git push",
            "bunx git push",
            "uvx git push",
            "uvx --from x git push",
            "xargs git push",
            "xargs -n 1 -P 2 git push",
            "command git push",
            "exec git push",
            "sh -c 'git push'",
            "bash -c 'git push origin main'",
            "bash -lc 'git push'",
            "bash -e -o pipefail -c 'git push'",
            "zsh -c 'env git push'",
            "dash -c 'git push'",
            "fish -c 'git push'",
            "fish --command='git push'",
            "nice sudo env -u GIT_CONFIG_COUNT sh -c 'timeout 5 git push'",
            "sudo rm -rf src",
            "sh -c 'gh pr merge 1'",
            "env npm publish",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        for line in [
            "env A=b cargo test",
            "sh -c 'cargo test'",
            "nice -n 19 cargo build",
            "timeout 60 cargo test",
            "bash scripts/check.sh",
        ] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn a_line_run_by_a_shell_that_cannot_be_split_is_denied() {
        let fixture = Fixture::workspace();
        for line in [
            "sh -c 'git status; git push'",
            "bash -c 'echo a | cat'",
            "sh -c 'echo $HOME'",
            "sh -c",
            "env -S 'a | b'",
        ] {
            let verdict = fixture.run(line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
        }
        let Verdict::Deny(why) = fixture.run("sh -c 'a; b'") else {
            unreachable!()
        };
        assert!(why.contains("“;”"), "{why}");
    }

    #[test]
    fn a_never_list_entry_matches_at_any_word_position() {
        let fixture = Fixture::workspace().command("deploy", "make deploy", |_| {});
        for line in [
            "rm -rf build",
            "sudo rm -rf build",
            "xargs rm -rf",
            "env X=1 docker compose down",
            "echo then rm -rf build",
            "sh -c 'nice rm -rf build'",
            "/bin/rm -rf build",
        ] {
            let verdict = fixture.run(line);
            let Verdict::Deny(why) = &verdict else {
                panic!("{line}: {verdict:?}");
            };
            assert!(
                why.starts_with("refused: the project never allows "),
                "{why}"
            );
        }
        assert!(denied(&fixture.catalogue("deploy")));
        for line in ["rm -r build", "rm build -rf", "docker compose up", "rm"] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn a_project_whose_never_list_cannot_be_read_asks_before_a_run() {
        let fixture = Fixture::workspace();
        let verdict = fixture.judge("commands_run", json!({"line": "cargo test"}), None);
        assert!(asked(&verdict, false), "{verdict:?}");
        // A read does not depend on it.
        let verdict = fixture.judge("fs_read", json!({"path": "src/a.rs"}), None);
        assert_eq!(verdict, Verdict::Allow);
    }

    #[test]
    fn a_write_or_a_deletion_under_a_git_folder_is_denied() {
        let fixture = Fixture::workspace();
        std::fs::create_dir_all(fixture.root.join(".git/hooks")).unwrap();
        std::os::unix::fs::symlink(".git/hooks", fixture.root.join("hooks")).unwrap();
        for (tool, path) in [
            ("fs_write", ".git/hooks/pre-commit"),
            ("fs_write", "sub/.git"),
            ("fs_write", "hooks/pre-commit"),
            ("fs_edit", ".git/config"),
            ("fs_write", "src/../.git/HEAD"),
        ] {
            let verdict = fixture.file(tool, path);
            assert!(denied(&verdict), "{tool} {path}: {verdict:?}");
        }
        for line in [
            "rm -rf .git",
            "rm -r sub/.git",
            "unlink .git",
            "shred .git/config",
            "cp evil .git/hooks/pre-commit",
            "ln -s ../evil .git/hooks/pre-commit",
            "find . -name .git -delete",
            "git --git-dir=.git log",
            "env GIT_DIR=.git git log",
            "sh -c 'rm -rf .git'",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        assert_eq!(fixture.file("fs_read", ".git/config"), Verdict::Allow);
        assert_eq!(fixture.file("fs_write", ".gitignore"), Verdict::Allow);
        assert_eq!(fixture.run("rm -r target"), Verdict::Allow);
        assert_eq!(fixture.run("cat .gitignore"), Verdict::Allow);
    }

    #[test]
    fn dot_env_is_asked_and_its_templates_are_allowed() {
        let fixture = Fixture::workspace();
        for path in [".env", ".env.local", "api/.env", "api/.env.production"] {
            for tool in ["fs_read", "fs_write"] {
                let verdict = fixture.file(tool, path);
                assert!(asked(&verdict, true), "{tool} {path}: {verdict:?}");
            }
        }
        for path in [
            ".env.example",
            ".env.sample",
            ".env.template",
            ".env.dist",
            "api/.env.example",
            ".envrc.md",
        ] {
            assert_eq!(fixture.file("fs_read", path), Verdict::Allow, "{path}");
        }
        assert!(asked(&fixture.run("cat .env"), true));
        assert!(asked(&fixture.run("cp .env.example api/.env"), true));
        assert_eq!(fixture.run("cat .env.example"), Verdict::Allow);
    }

    #[test]
    fn credentials_folders_and_flottille_s_own_are_sensitive_places() {
        let fixture = Fixture::workspace();
        let own = fixture.own[0].join("projects/app.toml");
        for path in [
            "~/.ssh/id_ed25519",
            "~/.gnupg",
            "~/.aws/credentials",
            "~/.config/gcloud/x",
            "~/.kube/config",
            "~/.docker/config.json",
            "~/.config/gh/hosts.yml",
            "~/.netrc",
            "~/.npmrc",
            "~/.pypirc",
            "~/.git-credentials",
            "~/.local/share/keyrings/login.keyring",
            "~/.claude/.credentials.json",
            "~/.claude.json",
            "~/.codex/auth.json",
            "/etc/shadow",
            "/etc/sudoers",
            "/etc/sudoers.d/x",
            own.to_str().unwrap(),
        ] {
            let verdict = fixture.file("fs_read", path);
            assert!(asked(&verdict, true), "{path}: {verdict:?}");
        }
        // Inside the place, only what sits below it counts: its own path may run through any
        // folder.
        assert!(asked(&fixture.file("fs_read", ".ssh/known_hosts"), true));
        assert_eq!(fixture.file("fs_read", "src/ssh.rs"), Verdict::Allow);
    }

    #[test]
    fn anything_outside_the_place_is_asked() {
        let fixture = Fixture::workspace();
        for (tool, path) in [
            ("fs_read", "/etc/hostname"),
            ("fs_read", "../other/a.rs"),
            ("fs_list", "~/notes"),
            ("fs_write", "/tmp/x"),
        ] {
            let verdict = fixture.file(tool, path);
            assert!(asked(&verdict, false), "{tool} {path}: {verdict:?}");
        }
        for line in [
            "cat /etc/hostname",
            "ls ../other",
            "cp src/a.rs --target-directory=/tmp",
        ] {
            assert!(
                asked(&fixture.run(line), false),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        // The program itself is wherever it is installed.
        assert_eq!(fixture.run("/usr/bin/cargo test"), Verdict::Allow);
    }

    #[test]
    fn reads_listings_and_writes_inside_the_place_are_allowed() {
        let fixture = Fixture::workspace();
        for (tool, path) in [
            ("fs_read", "src/a.rs"),
            ("fs_list", "."),
            ("search", "."),
            ("fs_write", "src/new.rs"),
            ("fs_edit", "src/a.rs"),
        ] {
            assert_eq!(fixture.file(tool, path), Verdict::Allow, "{tool} {path}");
        }
        for tool in ["commands_list", "commands_output", "commands_stop"] {
            assert_eq!(
                fixture.judge(tool, json!({"run": "r1"}), Some(&fixture.never)),
                Verdict::Allow,
                "{tool}"
            );
        }
    }

    #[test]
    fn a_catalogue_command_is_allowed_unless_marked_or_a_shell() {
        let fixture = Fixture::workspace()
            .command("test", "cargo test", |_| {})
            .command("seed", "npm run seed", |c| c.ask_before_running = true)
            .command("script", "bash scripts/x.sh", |_| {})
            .command("publish", "cargo publish", |_| {});
        assert_eq!(fixture.catalogue("test"), Verdict::Allow);
        assert!(asked(&fixture.catalogue("seed"), false));
        assert!(asked(&fixture.catalogue("script"), false));
        assert!(denied(&fixture.catalogue("publish")));
    }

    #[test]
    fn the_project_folder_is_read_only_for_agents() {
        let fixture = Fixture::new(None)
            .command("check", "cargo clippy", |c| {
                c.check = true;
                c.read_only = true;
            })
            .command("fmt", "cargo fmt", |c| c.check = true);
        assert!(denied(&fixture.file("fs_write", "src/new.rs")));
        assert!(denied(&fixture.file("fs_edit", "src/a.rs")));
        assert!(denied(&fixture.run("cargo test")));
        assert!(denied(&fixture.catalogue("fmt")));
        assert_eq!(fixture.catalogue("check"), Verdict::Allow);
        assert_eq!(fixture.file("fs_read", "src/a.rs"), Verdict::Allow);
        let Verdict::Deny(why) = fixture.run("cargo test") else {
            unreachable!()
        };
        assert!(why.contains("read-only"), "{why}");
    }

    #[test]
    fn a_grant_s_identity_is_the_exact_call_without_a_write_s_content() {
        let fixture = Fixture::workspace();
        let write = |content: &str| {
            fixture.identity_of("fs_write", json!({"path": "/tmp/x", "content": content}))
        };
        assert_eq!(write("a"), write("b"));
        assert_ne!(
            write("a"),
            fixture.identity_of("fs_write", json!({"path": "/tmp/y", "content": "a"}))
        );
        assert_ne!(
            write("a"),
            fixture.identity_of("fs_read", json!({"path": "/tmp/x"}))
        );
        let run = |line: &str| fixture.identity_of("commands_run", json!({"line": line}));
        assert_eq!(run("cat /etc/hostname"), run("cat  /etc/hostname"));
        assert_ne!(run("cat /etc/hostname"), run("cat /etc/hosts"));
        assert!(!write("a").is_empty());
    }

    #[test]
    fn a_path_outside_is_shown_as_the_place_shows_it() {
        let fixture = Fixture::workspace();
        let Verdict::Ask(ask) = fixture.file("fs_read", "~/notes.txt") else {
            unreachable!()
        };
        assert_eq!(
            ask.reasons,
            ["~/notes.txt is outside this agent's place".to_owned()]
        );
    }
}
