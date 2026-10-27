//! The gate every tool call goes through, once, before it is performed (`docs/design.md` §7,
//! `docs/reference/hemera.md` §4, without the remote judge).

use std::path::{Path, PathBuf};

use flottille_core::procedure::{self, Grants, Mode};
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
    /// The procedure run whose grants the agent has, when it has some (`docs/procedures.md`).
    pub run: Option<&'a Run>,
}

/// The grants of the procedure run an agent works in, as the gate applies them.
pub struct Run {
    pub procedure: String,
    pub grants: Grants,
    /// Each repository's folder in the workspace, with the remote its branch goes to.
    pub remotes: Vec<(PathBuf, String)>,
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
    if call.tool == "forge_write" {
        return forge_write(call);
    }
    let layers = if run && !call.words.is_empty() {
        match layers(call.words) {
            Ok(layers) => layers,
            Err(why) => return Verdict::Deny(why),
        }
    } else {
        Vec::new()
    };
    // Inside a procedure run, its grants let through what the rules below refuse to agents;
    // outside one, nothing here changes them.
    let granted = match call.run {
        Some(procedure) if run => match granted(call, procedure, &layers) {
            Ok(granted) => granted,
            Err(why) => return Verdict::Deny(why),
        },
        _ => None,
    };
    let reached = reached(call, &layers);
    if let Some(why) = refusal(call, &layers, &reached, granted.is_some()) {
        return Verdict::Deny(why);
    }
    let mut ask = Ask {
        reasons: Vec::new(),
        sensitive: false,
    };
    if let (Some((grant, Mode::Ask)), Some(procedure)) = (granted, call.run) {
        ask.reasons
            .push(asked_by_grant(&procedure.procedure, grant));
        ask.sensitive = true;
    }
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

/// The sentence refusing the call, if anything refuses it. `granted` says a procedure's grant
/// lets the line's program do what it does: the rules of programs are not applied to it.
fn refusal(
    call: &Call,
    layers: &[Vec<String>],
    reached: &[Resolved],
    granted: bool,
) -> Option<String> {
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
    if let Some(why) = deletion_refusal(call, layers) {
        return Some(why);
    }
    if granted {
        return None;
    }
    layers.iter().find_map(|layer| program_refusal(layer))
}

/// The refusal of a recursive deletion that would take a repository's `.git` with it: its target
/// is the place's folder, a repository's, or a folder above one. The words of a line that name a
/// `.git` are refused before; this is the deletion that names only the folder holding it.
fn deletion_refusal(call: &Call, layers: &[Vec<String>]) -> Option<String> {
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
                    "refused: {} would delete {}, which is the place's folder, a repository's or \
                     a folder above one, and with it a .git folder, which agents never delete: \
                     name what to delete inside it",
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

/// What agents are never let do, by program: push, rewrite history, write to a forge, publish.
fn program_refusal(layer: &[String]) -> Option<String> {
    let program = base(&layer[0]);
    let args = &layer[1..];
    let any = |names: &[&str]| args.iter().any(|w| names.contains(&w.as_str()));
    match program {
        "git" => git_refusal(args),
        "gh" | "glab" => forge_refusal(program, args),
        // Bitbucket's sign-in is the user's too: `bkt` keeps its token in the OS keychain, which
        // no environment variable of ours moves, and any configuration that names the host
        // reaches it (`BKT_CONFIG_DIR` moves the configuration only). A read, `bkt api`
        // included, would run with the user's own token, so no word of `bkt` is let through
        // (`forge_read` reads the pull requests).
        "bkt" => Some(
            "refused: bkt is Bitbucket's command line with the user's own sign-in, which agents \
             never use: read the pull requests with forge_read, and say what should be written \
             to Bitbucket, and the user does it"
                .into(),
        ),
        // Jira's sign-in is the user's, in a place no environment variable of ours moves: no word
        // of `acli` is let through, reads included (`ticket_read` reads a ticket).
        "acli" => Some(
            "refused: acli is Atlassian's command line with the user's own Jira sign-in, which \
             agents never use: read a ticket with ticket_read, and say what should be written \
             to it, and the user does it"
                .into(),
        ),
        "npm" | "pnpm" | "yarn" | "bun" | "cargo" | "poetry" | "flit" | "hatch" | "uv"
            if any(&["publish", "unpublish"]) =>
        {
            Some(publishing(layer))
        }
        "twine" if any(&["upload"]) => Some(publishing(layer)),
        "gem" | "dotnet" | "nuget" if any(&["push"]) => Some(publishing(layer)),
        "flottille" => Some(
            "refused: flottille drives the daemon, and agents act only through their tools: \
             ask the user to run it"
                .into(),
        ),
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
        "remote" if !remote_reads(rest) => "remote",
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
        "remote" => "refused: git remote changes where the repository's remotes lead, and agents \
                     never do: git remote -v, git remote show and git remote get-url read them"
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
    if words.first() == Some(&"auth")
        && (words.get(1) == Some(&"token")
            || args.iter().any(|w| {
                w == "--show-token"
                    || (w.starts_with('-') && !w.starts_with("--") && w.contains('t'))
            }))
    {
        return Some(format!(
            "refused: {program} auth prints a login token, and agents never see one: \
             auth status says whether a login exists"
        ));
    }
    let read = match (program, words.as_slice()) {
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
    };
    if read {
        return None;
    }
    let reads = match program {
        "gh" => "pr view, list, diff, checks and status; issue view and list; run view and list",
        _ => "mr view, list and diff; issue view and list; ci view, list and status",
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

/// The grant a procedure's run lets this `commands_run` line through by, when the gate lets it
/// through: its environment is then lifted for it alone (`runs::defend`), hooks off, and the push
/// wall down for a push.
pub fn lifted(call: &Call) -> Option<procedure::Grant> {
    let run = call.run?;
    if call.tool != "commands_run" || call.words.is_empty() {
        return None;
    }
    let layers = layers(call.words).ok()?;
    let (grant, _) = granted(call, run, &layers).ok()??;
    matches!(decide(call), Verdict::Allow | Verdict::Ask(_)).then_some(grant)
}

/// The reason a call is held when the procedure grants it with `ask`.
fn asked_by_grant(procedure: &str, grant: procedure::Grant) -> String {
    format!("the procedure {procedure} grants {grant} only with the user's approval of each one")
}

/// The verdict on `forge_write`: only what the agent's procedure run grants, by the action's
/// grant.
fn forge_write(call: &Call) -> Verdict {
    let action = call.args["action"].as_str().unwrap_or_default();
    let grant = match action {
        "pr-create" | "pr-merge-squash" | "pipeline-rerun" => {
            procedure::Grant::of(action).expect("an action is a grant")
        }
        _ => {
            return Verdict::Deny(format!(
                "refused: forge_write has no action {action:?}: its actions are pr-create, \
                 pr-merge-squash and pipeline-rerun"
            ));
        }
    };
    let Some(run) = call.run else {
        return Verdict::Deny(format!(
            "refused: forge_write writes to the forge only inside a procedure that grants it, and \
             this agent runs in none: say what should be done ({action}), and the user does it \
             or runs a procedure for it"
        ));
    };
    match run.grants.get(&grant) {
        None => Verdict::Deny(format!(
            "refused: the procedure {} does not grant {grant}: {}; say what should be done, and \
             the user does it",
            run.procedure,
            granted_words(run)
        )),
        Some(Mode::Allow) => Verdict::Allow,
        Some(Mode::Ask) => Verdict::Ask(Ask {
            reasons: vec![asked_by_grant(&run.procedure, grant)],
            sensitive: true,
        }),
    }
}

/// What a run grants, as a refusal says it.
fn granted_words(run: &Run) -> String {
    if run.grants.is_empty() {
        return "it grants nothing".into();
    }
    format!("it grants {}", procedure::shown(&run.grants))
}

/// What a procedure run's grants decide of a line: `Ok(Some)` when a grant lets it through (it
/// is then the line's only program, `git` itself, never wrapped), `Ok(None)` when no grant
/// concerns it, and the refusal of what stays refused even inside a run (`git stash`, `git reset
/// --hard`, a force push without a lease, a push of another branch or to another remote).
fn granted(
    call: &Call,
    run: &Run,
    layers: &[Vec<String>],
) -> Result<Option<(procedure::Grant, Mode)>, String> {
    use procedure::Grant;
    for layer in layers {
        if base(&layer[0]) != "git" {
            continue;
        }
        let (at, _) = git_subcommand(&layer[1..]);
        let rest = layer.get(at + 2..).unwrap_or_default();
        match layer.get(at + 1).map(String::as_str) {
            Some("stash") => {
                return Err(
                    "refused: git stash is refused even inside a procedure: it hides work where \
                     nobody looks; commit it if the procedure grants commit, or leave it in the \
                     working tree"
                        .into(),
                );
            }
            Some("reset") if rest.iter().any(|w| w == "--hard") => {
                return Err(
                    "refused: git reset --hard is refused even inside a procedure: it throws work \
                     away; say what should be undone, and the user does it"
                        .into(),
                );
            }
            _ => {}
        }
    }
    let [layer] = layers else {
        return Ok(None);
    };
    if base(&layer[0]) != "git" {
        return Ok(None);
    }
    let (at, plain) = git_subcommand(&layer[1..]);
    let Some(command) = layer.get(at + 1).map(String::as_str) else {
        return Ok(None);
    };
    let rest = &layer[at + 2..];
    let concerned = match command {
        "push" => [Grant::Push, Grant::PushForceWithLease]
            .iter()
            .any(|grant| run.grants.contains_key(grant)),
        "commit" => run.grants.contains_key(&Grant::Commit),
        "rebase" => run.grants.contains_key(&Grant::Rebase),
        _ => false,
    };
    if !concerned {
        return Ok(None);
    }
    if !plain {
        return Err(format!(
            "refused: inside a procedure, git {command} runs without git's options before it \
             (-C, --git-dir, -c…): run it in the repository's folder, naming the repository with \
             commands_run's repository"
        ));
    }
    let grant = match command {
        "push" => push_grant(call, run, rest)?,
        "commit" => commit_check(rest).map(|()| Grant::Commit)?,
        _ => rebase_check(rest).map(|()| Grant::Rebase)?,
    };
    match run.grants.get(&grant) {
        Some(mode) => Ok(Some((grant, *mode))),
        None => Err(format!(
            "refused: this git {command} needs the grant {grant}, and the procedure {} does not \
             give it: {}",
            run.procedure,
            granted_words(run)
        )),
    }
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

/// The grant a `git push` needs, once it is known to push the workspace's own branch to its
/// repository's remote: `push`, or `push-force-with-lease` with a lease. Anything else is
/// refused with why.
fn push_grant(call: &Call, run: &Run, args: &[String]) -> Result<procedure::Grant, String> {
    let place = &call.grant.place;
    let branch = place.branch.as_str();
    let ours = |name: &str| name == branch || name.strip_prefix("refs/heads/") == Some(branch);
    let refuse = |what: &str| {
        format!(
            "refused: {what}; inside a procedure, git push sends the workspace's own branch \
             ({branch}) to its repository's remote: git push <remote> {branch}, with \
             --force-with-lease when the procedure grants push-force-with-lease"
        )
    };
    let mut lease = false;
    let mut operands = Vec::new();
    let mut options = true;
    for word in args {
        let word = word.as_str();
        if !options || !word.starts_with('-') || word == "-" {
            operands.push(word);
            continue;
        }
        match word {
            "--" => options = false,
            "--force-with-lease" => lease = true,
            _ if word.starts_with("--force-with-lease=") => {
                let value = &word["--force-with-lease=".len()..];
                let named = value.split_once(':').map_or(value, |(name, _)| name);
                if !ours(named) {
                    return Err(refuse(&format!(
                        "{word} leases {named}, which is not the workspace's branch"
                    )));
                }
                lease = true;
            }
            "--force-if-includes"
            | "--set-upstream"
            | "--quiet"
            | "--verbose"
            | "--porcelain"
            | "--dry-run"
            | "--atomic"
            | "--progress"
            | "--no-progress" => {}
            "--force" => {
                return Err(refuse(
                    "git push --force is refused even inside a procedure",
                ));
            }
            "--delete" | "--prune" => {
                return Err(refuse(&format!(
                    "git push {word} deletes branches on the remote"
                )));
            }
            "--mirror" | "--all" | "--branches" | "--tags" | "--follow-tags" => {
                return Err(refuse(&format!(
                    "git push {word} pushes more than the workspace's branch"
                )));
            }
            _ if word.starts_with("--") => {
                return Err(refuse(&format!(
                    "git push {word} is not an option a procedure's push takes"
                )));
            }
            _ => {
                for letter in word[1..].chars() {
                    match letter {
                        'u' | 'q' | 'v' | 'n' => {}
                        'f' => {
                            return Err(refuse("git push -f is refused even inside a procedure"));
                        }
                        'd' => return Err(refuse("git push -d deletes branches on the remote")),
                        _ => {
                            return Err(refuse(&format!(
                                "git push -{letter} is not an option a procedure's push takes"
                            )));
                        }
                    }
                }
            }
        }
    }
    let Some((remote, refspecs)) = operands.split_first() else {
        return Err(refuse("this git push names no remote"));
    };
    let folder = call.paths.first();
    let expected = run.remotes.iter().find(|(repository, _)| {
        folder.is_some_and(|folder| place.resolve(repository, ".").path == folder.path)
    });
    let Some((_, expected)) = expected else {
        return Err(refuse(
            "this git push does not run in one of the workspace's repositories: name the \
             repository with commands_run's repository",
        ));
    };
    if remote != expected {
        return Err(refuse(&format!(
            "{remote} is not this repository's remote, {expected}"
        )));
    }
    if refspecs.is_empty() {
        return Err(refuse("this git push names no branch"));
    }
    for refspec in refspecs {
        if refspec.starts_with('+') {
            return Err(refuse(&format!(
                "{refspec} forces the push without a lease, which is refused even inside a \
                 procedure"
            )));
        }
        let (source, target) = refspec.split_once(':').unwrap_or((refspec, refspec));
        if source.is_empty() {
            return Err(refuse(&format!("{refspec} deletes a branch on the remote")));
        }
        if target == "HEAD" {
            return Err(refuse(&format!(
                "{refspec} pushes to the branch HEAD is on, which may not be the workspace's: \
                 name it, HEAD:{branch}"
            )));
        }
        let source_ours = source == "HEAD" || ours(source);
        if !source_ours || !ours(target) {
            return Err(refuse(&format!("{refspec} is not the workspace's branch")));
        }
    }
    Ok(if lease {
        procedure::Grant::PushForceWithLease
    } else {
        procedure::Grant::Push
    })
}

/// A `git commit` a procedure's `commit` grant lets through: never rewriting the last commit, never
/// with `--no-verify`. Git takes any unambiguous start of a long option and groups short ones, so
/// both are read as git reads them.
fn commit_check(args: &[String]) -> Result<(), String> {
    let amend =
        || {
            Err("refused: git commit --amend rewrites a commit, and a procedure's commit makes new \
             ones only: commit the change on its own"
            .into())
        };
    let no_verify = || {
        Err(
            "refused: git commit --no-verify is refused inside a procedure: commit without it"
                .into(),
        )
    };
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
            let abbreviates = |option: &str| !name.is_empty() && option.starts_with(name);
            if abbreviates("amend") {
                return amend();
            }
            if abbreviates("no-verify") {
                return no_verify();
            }
            if abbreviates("fixup") && value.or_else(|| words.next()).is_some_and(rewrites) {
                return amend();
            }
        } else if let Some(group) = word.strip_prefix('-') {
            for letter in group.chars() {
                if letter == 'n' {
                    return no_verify();
                }
                // What follows an option that takes a value is that value.
                if "mFcCtuS".contains(letter) {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// A `git rebase` a procedure's `rebase` grant lets through: of the branch checked out (the
/// workspace's), onto one upstream, never interactive nor running commands.
fn rebase_check(args: &[String]) -> Result<(), String> {
    let refuse = |what: String| {
        format!(
            "refused: {what}; inside a procedure, git rebase replays the workspace's branch onto \
             one upstream (git rebase [--onto <base>] <upstream>, --continue, --abort, --skip)"
        )
    };
    let mut operands = 0;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let word = word.as_str();
        at += 1;
        match word {
            "--onto" => at += 1,
            _ if word.starts_with("--onto=") => {}
            "--continue" | "--abort" | "--skip" | "--quit" | "--keep-base" | "--quiet" | "-q"
            | "--verbose" | "-v" => {}
            _ if word.starts_with('-') => {
                return Err(refuse(format!(
                    "git rebase {word} is not an option a procedure's rebase takes"
                )));
            }
            _ => operands += 1,
        }
    }
    if operands > 1 {
        return Err(refuse(
            "git rebase with a branch after the upstream checks that branch out first".into(),
        ));
    }
    Ok(())
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
        /// The procedure run the agent works in, if any.
        run: Option<Run>,
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
                run: None,
            }
        }

        fn workspace() -> Fixture {
            Fixture::new(Some("w"))
        }

        /// The workspace, on the branch `app/login` whose remote is `origin`, in a run of the
        /// procedure `ship` that grants these.
        fn in_run(mut self, grants: &[(procedure::Grant, Mode)]) -> Fixture {
            self.grant.place.branch = "app/login".into();
            self.run = Some(Run {
                procedure: "ship".into(),
                grants: grants.iter().copied().collect(),
                remotes: vec![(self.root.clone(), "origin".into())],
            });
            self
        }

        fn forge(&self, action: &str) -> Verdict {
            self.judge(
                "forge_write",
                json!({"action": action, "repository": "."}),
                Some(&self.never),
            )
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
            self.considered(tool, args, never, decide)
        }

        /// The grant a procedure lifts the line's environment for.
        fn lifted(&self, line: &str) -> Option<procedure::Grant> {
            self.considered(
                "commands_run",
                json!({"line": line}),
                Some(&self.never),
                super::lifted,
            )
        }

        fn considered<T>(
            &self,
            tool: &str,
            args: Value,
            never: Option<&[Vec<String>]>,
            consider: impl FnOnce(&Call) -> T,
        ) -> T {
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
            consider(&Call {
                grant: &self.grant,
                tool,
                args: &args,
                words: &words,
                paths: &paths,
                command,
                never,
                own: &self.own,
                run: self.run.as_ref(),
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
                run: self.run.as_ref(),
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
            "gh auth status --show-token",
            "gh auth status -t",
            "gh auth status -ht",
            "gh auth status --hostname github.com --show-token",
            "gh auth token",
            "gh auth token --hostname github.com",
            "gh auth login",
            "glab auth status --show-token",
            "env gh auth token",
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
            "gh auth status --hostname github.com",
            "gh api repos/o/r/pulls",
            "gh api -X GET repos/o/r",
            "gh -R o/r pr view 1",
            "gh pr view 1 --repo o/r",
            "glab mr view 1",
            "glab mr list",
            "glab ci status",
        ] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn bkt_is_denied_whatever_it_is_asked_even_to_read() {
        let fixture = Fixture::workspace();
        for line in [
            "bkt",
            "bkt pr list",
            "bkt pr view 1 --json",
            "bkt pr checks 1",
            "bkt pr create --title x --source a --target main",
            "bkt pr merge 1",
            "bkt pipeline list",
            "bkt pipeline run --ref main",
            "bkt api /repositories/acme/app/pullrequests",
            "bkt context list",
            "bkt auth status",
            "bkt auth token",
            "bkt --context work pr list",
            "env bkt pr list",
            "nice bkt pr list",
        ] {
            let verdict = fixture.run(line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
        }
    }

    #[test]
    fn acli_is_denied_whatever_it_is_asked_even_to_read() {
        let fixture = Fixture::workspace();
        for line in [
            "acli",
            "acli jira workitem view ABC-1 --json",
            "acli jira workitem create --summary x",
            "acli jira workitem comment create --key ABC-1 --body hi",
            "acli jira auth status",
            "env acli jira workitem view ABC-1",
            "nice acli jira workitem transition --key ABC-1",
        ] {
            let verdict = fixture.run(line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
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
    fn a_recursive_deletion_of_a_place_or_a_repository_is_denied() {
        // Without the never list, which already refuses `rm -rf`: this is the guard on its own.
        let mut fixture = Fixture::workspace();
        for folder in ["a/src", "b/src", "docs"] {
            std::fs::create_dir_all(fixture.root.join(folder)).unwrap();
        }
        fixture.grant.place.repositories = vec![
            ("a".into(), fixture.root.join("a")),
            ("b".into(), fixture.root.join("b")),
        ];
        let free = |line: &str| fixture.judge("commands_run", json!({"line": line}), Some(&[]));
        for line in [
            "rm -rf .",
            "rm -rf ./",
            "rm -r .",
            "rm -R .",
            "rm --recursive .",
            "rm --recursive --force .",
            "rm -fr .",
            "rm -rv ./",
            "rm -r src/..",
            "rm -r a",
            "rm -R a/",
            "rm -r docs b",
            "rm -r -- a",
            "rm -r ..",
            "rm -rf ../..",
            "rm -rf /",
            "rm -r src ./",
            "find . -delete",
            "find -delete",
            "find a -delete",
            "find . -name '*.o' -delete",
            "find -L . -type f -delete",
            "find . -exec rm -rf {} +",
            "find a -execdir rm -r {} ';'",
            "sudo rm -rf .",
            "sh -c 'rm -rf .'",
            "env -i rm -r a",
            "xargs rm -r .",
        ] {
            let verdict = free(line);
            assert!(denied(&verdict), "{line}: {verdict:?}");
        }
        let Verdict::Deny(why) = free("rm -rf .") else {
            unreachable!()
        };
        assert!(why.contains(".git"), "{why}");
        for line in [
            "rm -r target",
            "rm -rf docs",
            "rm -r a/src",
            "rm -r a/src b/src",
            "rm -rf -- docs",
            "rm src/a.rs",
            "rm .",
            "rm -f a",
            "rm -- -r",
            "find src -name '*.o' -delete",
            "find a/src -delete",
            "find . -name '*.o'",
            "find . -exec cat {} +",
            "find docs -exec rm -r {} ';'",
            "find a -exec rm {} ';'",
            "ls -R .",
        ] {
            assert_eq!(free(line), Verdict::Allow, "{line}");
        }
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
    use procedure::Grant::{Commit, PrCreate, PrMergeSquash, Push, PushForceWithLease, Rebase};

    /// Every grant, allowed.
    fn every_grant() -> Vec<(procedure::Grant, Mode)> {
        procedure::Grant::ALL
            .iter()
            .map(|grant| (*grant, Mode::Allow))
            .collect()
    }

    #[test]
    fn outside_a_run_what_a_procedure_could_grant_is_decided_as_before() {
        let fixture = Fixture::workspace();
        for line in [
            "git push origin app/login",
            "git push --force-with-lease origin app/login",
            "git commit -m change",
            "git rebase main",
            "gh pr create --fill",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        // Allowed before procedures, and still.
        assert_eq!(fixture.run("git stash"), Verdict::Allow);
        for action in ["pr-create", "pr-merge-squash", "pipeline-rerun"] {
            let verdict = fixture.forge(action);
            assert!(denied(&verdict), "{action}: {verdict:?}");
            assert!(
                matches!(&verdict, Verdict::Deny(why) if why.contains("only inside a procedure")),
                "{verdict:?}"
            );
        }
    }

    #[test]
    fn inside_a_run_a_granted_push_of_the_workspace_s_branch_goes_through_an_ask_one_is_held_and_an_ungranted_one_is_refused()
     {
        let allowed = Fixture::workspace().in_run(&[(Push, Mode::Allow)]);
        for line in [
            "git push origin app/login",
            "git push -u origin app/login",
            "git push --set-upstream origin HEAD:app/login",
            "git push origin refs/heads/app/login:refs/heads/app/login",
            "git --no-pager push -q origin app/login",
        ] {
            assert_eq!(allowed.run(line), Verdict::Allow, "{line}");
        }

        let asked = Fixture::workspace().in_run(&[(Push, Mode::Ask)]);
        let verdict = asked.run("git push origin app/login");
        // Asked every time: never allowed for the tree.
        assert!(
            asked_by(&verdict, "the procedure ship grants push"),
            "{verdict:?}"
        );

        let ungranted = Fixture::workspace().in_run(&[(Commit, Mode::Allow)]);
        assert!(denied(&ungranted.run("git push origin app/login")));
    }

    fn asked_by(verdict: &Verdict, reason: &str) -> bool {
        matches!(verdict, Verdict::Ask(ask) if ask.sensitive
            && ask.reasons.iter().any(|r| r.starts_with(reason)))
    }

    #[test]
    fn a_push_of_another_branch_to_another_remote_or_a_deletion_is_refused_even_with_push() {
        let fixture = Fixture::workspace().in_run(&every_grant());
        for line in [
            "git push",
            "git push origin",
            "git push origin main",
            "git push origin app/login:main",
            "git push origin HEAD:main",
            "git push origin app/login other",
            "git push upstream app/login",
            "git push https://example.com/acme/app.git app/login",
            "git push origin :app/login",
            "git push --delete origin app/login",
            "git push -d origin app/login",
            "git push --mirror origin",
            "git push --all origin",
            "git push --tags origin app/login",
            "git push --prune origin app/login",
            "git push -o ci.skip origin app/login",
            "git push --receive-pack=evil origin app/login",
            "git push --force-with-lease=main origin app/login",
            "git -C src push origin app/login",
            "git -c remote.origin.pushurl=elsewhere push origin app/login",
            "env git push origin app/login",
            "sh -c 'git push origin app/login'",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
    }

    #[test]
    fn reset_hard_stash_and_a_force_push_without_lease_are_refused_even_with_every_grant() {
        let fixture = Fixture::workspace().in_run(&every_grant());
        for line in [
            "git reset --hard HEAD~1",
            "git reset --hard",
            "git stash",
            "git stash pop",
            "env git stash",
            "git push --force origin app/login",
            "git push -f origin app/login",
            "git push -uf origin app/login",
            "git push origin +app/login",
            "git push origin +HEAD:app/login",
            "git push --force --force-with-lease origin app/login",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        // A run with no grant at all refuses them too.
        let none = Fixture::workspace().in_run(&[]);
        assert!(denied(&none.run("git stash")));
    }

    #[test]
    fn a_push_with_a_lease_needs_its_own_grant() {
        let push = Fixture::workspace().in_run(&[(Push, Mode::Allow)]);
        let verdict = push.run("git push --force-with-lease origin app/login");
        assert!(
            matches!(&verdict, Verdict::Deny(why) if why.contains("push-force-with-lease")),
            "{verdict:?}"
        );
        let lease = Fixture::workspace().in_run(&[(PushForceWithLease, Mode::Allow)]);
        for line in [
            "git push --force-with-lease origin app/login",
            "git push --force-with-lease=app/login origin app/login",
            "git push --force-with-lease=app/login:abc123 --force-if-includes origin app/login",
        ] {
            assert_eq!(lease.run(line), Verdict::Allow, "{line}");
        }
        // The lease's grant does not give a plain push.
        assert!(denied(&lease.run("git push origin app/login")));
    }

    #[test]
    fn commit_and_rebase_go_through_only_as_their_grants_say() {
        let fixture = Fixture::workspace().in_run(&[(Commit, Mode::Allow), (Rebase, Mode::Ask)]);
        assert_eq!(fixture.run("git commit -m change"), Verdict::Allow);
        assert_eq!(fixture.run("git commit -a -m change"), Verdict::Allow);
        for line in [
            "git commit --amend -m change",
            "git commit --no-verify -m change",
            "git commit -n -m change",
        ] {
            assert!(denied(&fixture.run(line)), "{line}");
        }
        for line in [
            "git rebase main",
            "git rebase --onto main old",
            "git rebase --continue",
            "git rebase --abort",
        ] {
            assert!(
                asked_by(&fixture.run(line), "the procedure ship grants rebase"),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        for line in [
            "git rebase -i main",
            "git rebase --interactive main",
            "git rebase --exec make main",
            "git rebase -x make main",
            "git rebase --autostash main",
            "git rebase main other-branch",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        // What no grant covers is decided as without a procedure.
        for line in [
            "git merge topic",
            "git cherry-pick abc123",
            "git branch -D topic",
            "git tag v1",
        ] {
            assert!(denied(&fixture.run(line)), "{line}");
        }
        assert_eq!(fixture.run("git status"), Verdict::Allow);
    }

    #[test]
    fn the_project_s_never_list_wins_over_a_grant() {
        let mut fixture = Fixture::workspace().in_run(&every_grant());
        fixture.never.push(vec!["git".into(), "push".into()]);
        let verdict = fixture.run("git push origin app/login");
        assert!(
            matches!(&verdict, Verdict::Deny(why) if why.contains("the project never allows git push")),
            "{verdict:?}"
        );
    }

    #[test]
    fn forge_write_follows_the_run_s_grants() {
        let fixture =
            Fixture::workspace().in_run(&[(PrCreate, Mode::Allow), (PrMergeSquash, Mode::Ask)]);
        assert_eq!(fixture.forge("pr-create"), Verdict::Allow);
        assert!(asked_by(
            &fixture.forge("pr-merge-squash"),
            "the procedure ship grants pr-merge-squash"
        ));
        let verdict = fixture.forge("pipeline-rerun");
        assert!(
            matches!(&verdict, Verdict::Deny(why)
                if why.contains("does not grant pipeline-rerun")
                    && why.contains("pr-create (allow), pr-merge-squash (ask)")),
            "{verdict:?}"
        );
        assert!(denied(&fixture.forge("pr-close")));
    }

    #[test]
    fn an_agent_never_changes_where_a_remote_leads_even_with_every_grant() {
        for fixture in [
            Fixture::workspace(),
            Fixture::workspace().in_run(&every_grant()),
        ] {
            for line in [
                "git remote add evil https://evil.example/x.git",
                "git remote set-url origin https://evil.example/x.git",
                "git remote set-url --push origin https://evil.example/x.git",
                "git remote rename origin old",
                "git remote remove origin",
                "git remote rm origin",
                "git remote set-branches origin main",
                "git remote set-head origin main",
                "git remote -v add evil https://evil.example/x.git",
                "git config remote.origin.url https://evil.example/x.git",
                "git config set remote.origin.pushurl https://evil.example/x.git",
                "git config --add remote.origin.pushurl https://evil.example/x.git",
                "git config url.https://evil.example/.insteadOf git@github.com:",
            ] {
                assert!(
                    denied(&fixture.run(line)),
                    "{line}: {:?}",
                    fixture.run(line)
                );
            }
            for line in [
                "git remote",
                "git remote -v",
                "git remote --verbose",
                "git remote show origin",
                "git remote get-url origin",
                "git remote get-url --push --all origin",
            ] {
                assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
            }
        }
    }

    #[test]
    fn a_push_of_a_bare_head_is_refused_since_head_may_be_another_branch() {
        let fixture = Fixture::workspace().in_run(&every_grant());
        for line in [
            "git push origin HEAD",
            "git push -u origin HEAD",
            "git push origin HEAD:HEAD",
            "git push --force-with-lease origin HEAD",
        ] {
            let verdict = fixture.run(line);
            assert!(
                matches!(&verdict, Verdict::Deny(why) if why.contains("HEAD:app/login")),
                "{line}: {verdict:?}"
            );
        }
        // Named, the branch it goes to is known whatever HEAD is.
        assert_eq!(
            fixture.run("git push origin HEAD:app/login"),
            Verdict::Allow
        );
    }

    #[test]
    fn abbreviated_or_grouped_options_that_amend_or_skip_hooks_are_refused() {
        let fixture = Fixture::workspace().in_run(&[(Commit, Mode::Allow)]);
        for line in [
            "git commit --amen -m change",
            "git commit --ame -m change",
            "git commit --am -m change",
            "git commit --no-verif -m change",
            "git commit --no-ver -m change",
            "git commit -anm change",
            "git commit -na -m change",
            "git commit -an -m change",
            "git commit --fixup=amend:HEAD",
            "git commit --fixu=amend:HEAD",
            "git commit --fix=reword:HEAD",
            "git commit --fixup amend:HEAD",
            "git commit --fixup reword:HEAD",
        ] {
            assert!(
                denied(&fixture.run(line)),
                "{line}: {:?}",
                fixture.run(line)
            );
        }
        for line in [
            "git commit -am change",
            "git commit -m nothing-to-see",
            "git commit -mnote",
            "git commit --fixup HEAD",
            "git commit --no-edit -m change",
            "git commit --verbose -m change",
        ] {
            assert_eq!(fixture.run(line), Verdict::Allow, "{line}");
        }
    }

    #[test]
    fn a_granted_commit_push_or_rebase_is_lifted_and_nothing_else_is() {
        let fixture = Fixture::workspace().in_run(&[
            (Commit, Mode::Allow),
            (Push, Mode::Ask),
            (Rebase, Mode::Allow),
        ]);
        assert_eq!(fixture.lifted("git commit -m change"), Some(Commit));
        assert_eq!(fixture.lifted("git push origin app/login"), Some(Push));
        assert_eq!(fixture.lifted("git rebase main"), Some(Rebase));
        for line in ["git status", "git push origin main", "git commit --amend"] {
            assert_eq!(fixture.lifted(line), None, "{line}");
        }
        assert_eq!(
            Fixture::workspace().lifted("git commit -m change"),
            None,
            "outside a run"
        );
    }
}
