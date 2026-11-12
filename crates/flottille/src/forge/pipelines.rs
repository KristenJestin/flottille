//! The CI family of `forge` (`docs/forges.md`, "The CI family"): what can be run, the recent runs,
//! a run's steps and a step's log, and the writes that run a pipeline, stop a run and act on one of
//! its steps, on GitHub (`gh`) and Bitbucket Cloud (`bkt`). Any pipeline of any branch of the
//! place's repositories, with any variables: the gate decides, as for every forge write. What can
//! be run is read from the CI's own files on the branch, through git; the rest through the user's
//! own command line, its output masked.

use serde::Deserialize;
use serde_json::{Value, json};
use serde_norway::Value as Yaml;

use super::followed::{self, Started};
use super::{
    Forge, ForgeCall, NO_FORGE, bitbucket, chosen, ci, declared_forge, events, gh_json, gh_text,
    logs, place_checkouts, signed_in, yaml,
};
use crate::daemon::Daemon;
use crate::git::{self, Class};
use crate::mcp::Outcome;
use crate::place::Place;
use crate::workspaces::{self, Head};

/// The actions of the family.
pub(super) const ACTIONS: [&str; 7] = [
    "pipelines",
    "runs",
    "run_view",
    "run_logs",
    "run",
    "stop",
    "step",
];
/// How many runs `runs` lists.
const RUNS: usize = 20;
/// Bitbucket's file of pipelines, at the root of the repository.
const BITBUCKET_FILE: &str = "bitbucket-pipelines.yml";
/// GitHub's folder of workflows.
const WORKFLOWS: &str = ".github/workflows";

/// A variable a run is given: a Bitbucket pipeline's variable, a GitHub workflow's input.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Variable {
    pub key: String,
    pub value: String,
    /// Bitbucket keeps it out of its logs and API; Flottille out of every result and record.
    #[serde(default)]
    pub secured: bool,
}

/// The shortest secured value that is hidden wherever it appears in an answer or a record. A
/// shorter one (`4`, `ab`) would garble them (`#43`, `table`): it is hidden only where it is
/// echoed as a variable's value.
pub(crate) const HIDDEN_EVERYWHERE: usize = 3;
/// The largest file of the CI `pipelines` reads, in bytes: a real one is a few kilobytes.
const FILE_LIMIT: u64 = 1024 * 1024;
/// `FILE_LIMIT`, as the refusal says it.
const LIMIT_SAID: &str = "1 MiB";

/// A text with the secured variables' values hidden: each one everywhere from
/// `HIDDEN_EVERYWHERE` characters, a shorter one where the text echoes it as `KEY=value` (a
/// command line's, the forge's) or as `"value": "…"` (a JSON body's).
fn hidden(text: String, variables: &[Variable]) -> String {
    variables
        .iter()
        .filter(|v| v.secured && !v.value.is_empty())
        .fold(text, |text, v| {
            if v.value.chars().count() >= HIDDEN_EVERYWHERE {
                return text.replace(v.value.as_str(), "[masked]");
            }
            let json = Value::from(v.value.as_str()).to_string();
            text.replace(
                &format!("{}={}", v.key, v.value),
                &format!("{}=[masked]", v.key),
            )
            .replace(&format!("\"value\":{json}"), "\"value\":\"[masked]\"")
            .replace(&format!("\"value\": {json}"), "\"value\": \"[masked]\"")
        })
}

/// One action of the family, once the gate let it through. A secured variable's value is said in
/// no answer, whatever the forge echoed of it.
pub(super) async fn call(daemon: &Daemon, agent: &str, place: &Place, asked: ForgeCall) -> Outcome {
    let variables = asked.variables.clone().unwrap_or_default();
    let hide = |text: String| hidden(text, &variables);
    match acted(daemon, agent, place, asked).await {
        Outcome::Done(text) => Outcome::Done(hide(text)),
        Outcome::Refused(text) => Outcome::Refused(hide(text)),
        Outcome::Invalid(text) => Outcome::Invalid(hide(text)),
        Outcome::Failed(text) => Outcome::Failed(hide(text)),
        Outcome::Held(text) => Outcome::Held(hide(text)),
    }
}

/// Why a variable's name is refused for a Bitbucket pipeline, which takes letters, digits and
/// underscores and none starting with a digit; GitHub's inputs may also hold a `-`.
pub(crate) fn bitbucket_key(key: &str) -> Option<String> {
    let fine = !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (!fine).then(|| {
        format!(
            "refused: {key:?} is no name of a Bitbucket variable: letters, digits and underscores, \
             not starting with a digit"
        )
    })
}

/// The branch a `run` that names none will run on, put in the call before the gate sees it, so
/// that the card and the judge say where a pipeline runs. A branch that cannot be told is left
/// out: the call refuses it itself.
pub(crate) async fn with_branch(daemon: &Daemon, place: &Place, mut args: Value) -> Value {
    if args["action"] != "run" || args.get("branch").is_some_and(|b| !b.is_null()) {
        return args;
    }
    let checkout = place_checkouts(daemon, place)
        .ok()
        .and_then(|all| chosen(all, args["repository"].as_str(), "run", true).ok())
        .and_then(|mut checkouts| checkouts.pop());
    if let Some(checkout) = checkout
        && let Ok(branch) = branch_of(daemon, place, &checkout, None).await
    {
        args["branch"] = branch.into();
    }
    args
}

async fn acted(daemon: &Daemon, agent: &str, place: &Place, asked: ForgeCall) -> Outcome {
    let action = asked.action.as_str();
    if let Some(refusal) = malformed(&asked) {
        return Outcome::Refused(refusal);
    }
    let checkout = match place_checkouts(daemon, place)
        .and_then(|all| chosen(all, asked.repository.as_deref(), action, true))
    {
        Ok(mut checkouts) => checkouts.remove(0),
        Err(refusal) => return refusal,
    };
    let forge = match declared_forge(&checkout).await {
        Ok(forge) => forge,
        Err(None) => {
            return Outcome::Refused(format!(
                "refused: {}: {NO_FORGE}; nothing was asked of the forge: ask the user to set it",
                checkout.path
            ));
        }
        Err(Some(why)) => return Outcome::Refused(why),
    };
    let slug = forge.slug();
    if matches!(forge, Forge::Bitbucket { .. }) && !bitbucket::cloud(&forge) {
        return Outcome::Refused(format!(
            "refused: Bitbucket Data Center has no pipelines bkt can read or run, and {slug} is \
             on a Data Center host: its builds are another tool's; nothing was asked of the forge"
        ));
    }
    if matches!(forge, Forge::Bitbucket { .. })
        && let Some(refusal) = asked
            .variables
            .iter()
            .flatten()
            .find_map(|v| bitbucket_key(&v.key))
    {
        return Outcome::Refused(refusal);
    }
    if let ("step", Forge::Bitbucket { owner, name, .. }) = (action, &forge) {
        return Outcome::Refused(format!(
            "refused: Bitbucket Cloud's API has no call that starts a manual step or runs one step \
             again (it reads steps, and stops or runs a whole pipeline): start it on the run's \
             page, https://bitbucket.org/{owner}/{name}/pipelines/results/{}, or run the \
             pipeline again with forge run",
            asked.run.unwrap_or_default()
        ));
    }
    if let (Some("start"), Forge::GitHub { .. }) = (asked.verb.as_deref(), &forge) {
        return Outcome::Refused(
            "refused: GitHub has no manual step: a job that waits is held by its environment's \
             reviewers, a protection the user set for people, which an agent never answers for \
             them; rerun runs a job again"
                .into(),
        );
    }
    if matches!(forge, Forge::GitHub { .. })
        && let Err(problem) = signed_in().await
    {
        return Outcome::Failed(problem);
    }
    let path = checkout.path.clone();
    // The branch: the one named, else the workspace's; `runs` in main reads the repository's.
    let branch = match (action, &asked.branch, &place.workspace) {
        ("runs", None, None) | ("run_view" | "run_logs" | "stop" | "step", _, _) => None,
        _ => match branch_of(daemon, place, &checkout, asked.branch.as_deref()).await {
            Ok(branch) => Some(branch),
            Err(refusal) => return Outcome::Refused(refusal),
        },
    };
    let run = asked.run.unwrap_or_default();
    let step = asked.step.as_deref().unwrap_or_default();
    // A run started is followed unless the agent said no.
    let follow = |(outcome, started): (Outcome, Option<Started>)| match outcome {
        Outcome::Done(text) if asked.follow != Some(false) => {
            Outcome::Done(text + &followed::follow(daemon, agent, place, &path, &forge, started))
        }
        outcome => outcome,
    };
    match (&forge, action) {
        (_, "pipelines") => listed(&checkout, &forge, &path, &branch.unwrap_or_default()).await,
        (Forge::Bitbucket { .. }, action) => {
            let context = match bitbucket::Contexts::default().of(&forge).await {
                Ok(context) => context,
                Err(problem) => return Outcome::Failed(problem),
            };
            let context = context.as_str();
            match action {
                "runs" => bitbucket_runs(&forge, context, branch.as_deref()).await,
                "run_view" => bitbucket_view(&forge, context, run, &path).await,
                "run_logs" => bitbucket_log(&forge, context, run, step).await,
                "run" => {
                    let pipeline = asked.pipeline.as_deref().unwrap_or_default();
                    let variables = asked.variables.unwrap_or_default();
                    let branch = branch.unwrap_or_default();
                    follow(bitbucket_run(&forge, context, pipeline, &branch, &variables).await)
                }
                _ => bitbucket_stop(&forge, context, run).await,
            }
        }
        (Forge::GitHub { .. }, "runs") => github_runs(&forge, branch.as_deref()).await,
        (Forge::GitHub { .. }, "run_view") => github_view(&forge, run, &path).await,
        (Forge::GitHub { .. }, "run_logs") => github_log(&forge, run, step).await,
        (Forge::GitHub { .. }, "run") => {
            let pipeline = asked.pipeline.as_deref().unwrap_or_default();
            let variables = asked.variables.unwrap_or_default();
            follow(github_run(&forge, pipeline, &branch.unwrap_or_default(), &variables).await)
        }
        (Forge::GitHub { .. }, "stop") => {
            let id = run.to_string();
            match gh_text(&["run", "cancel", &id, "-R", &forge.gh_repository()]).await {
                Ok(_) => Outcome::Done(format!("The run {id} of {slug} was asked to stop.")),
                Err(why) => Outcome::Failed(why),
            }
        }
        (Forge::GitHub { .. }, _) => github_rerun(&forge, run, step).await,
    }
}

/// Why a call of the family cannot be made as it is, before anything is read.
fn malformed(asked: &ForgeCall) -> Option<String> {
    let action = asked.action.as_str();
    let takes = |what: &str, takers: &[&str]| {
        (!takers.contains(&action)).then(|| {
            format!(
                "refused: forge {action} takes no {what}: {what} is for {}",
                takers.join(" and ")
            )
        })
    };
    if asked.variables.is_some()
        && let Some(refusal) = takes("variables", &["run"])
    {
        return Some(refusal);
    }
    if asked.pipeline.is_some()
        && let Some(refusal) = takes("pipeline", &["run"])
    {
        return Some(refusal);
    }
    if asked.verb.is_some()
        && let Some(refusal) = takes("do", &["step"])
    {
        return Some(refusal);
    }
    if asked.step.is_some()
        && let Some(refusal) = takes("step", &["run_logs", "step"])
    {
        return Some(refusal);
    }
    let needs_run = ["run_view", "run_logs", "stop", "step"].contains(&action);
    if needs_run && asked.run.is_none() {
        return Some(format!(
            "refused: forge {action} needs run, the run's number: forge runs lists them"
        ));
    }
    if ["run_logs", "step"].contains(&action) && asked.step.as_deref().is_none_or(str::is_empty) {
        return Some(format!(
            "refused: forge {action} needs step, a step's name (or id): forge run_view lists them"
        ));
    }
    if action == "step" && !matches!(asked.verb.as_deref(), Some("start" | "rerun")) {
        return Some(
            "refused: forge step needs do: start (a manual step the run waits on) or rerun (the \
             step again)"
                .into(),
        );
    }
    if action == "run" {
        match asked.pipeline.as_deref().map(str::trim) {
            None | Some("") => {
                return Some(
                    "refused: forge run needs pipeline: what forge pipelines lists (Bitbucket: \
                     default, custom: <name>, branches: <pattern>…; GitHub: a workflow's file)"
                        .into(),
                );
            }
            Some(pipeline) if pipeline.starts_with('-') => {
                return Some(format!("refused: {pipeline:?} is no pipeline"));
            }
            _ => {}
        }
        for variable in asked.variables.iter().flatten() {
            let named = !variable.key.is_empty()
                && !variable.key.starts_with('-')
                && variable
                    .key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c));
            if !named {
                return Some(format!(
                    "refused: {:?} is no variable's name: letters, digits, _ and -",
                    variable.key
                ));
            }
        }
    }
    None
}

/// The branch a call is for: the one named, once git takes it for a branch's name; else the one
/// the workspace's repository is on now; in main, the one the project's checkout is on.
async fn branch_of(
    daemon: &Daemon,
    place: &Place,
    checkout: &super::Checkout,
    named: Option<&str>,
) -> Result<String, String> {
    if let Some(branch) = named {
        let fine = !branch.starts_with('-')
            && git::run(
                &checkout.folder,
                &["check-ref-format", "--branch", branch],
                Class::Read,
            )
            .await
            .is_ok_and(|checked| checked.success);
        return match fine {
            true => Ok(branch.to_owned()),
            false => Err(format!("refused: {branch:?} is no branch's name")),
        };
    }
    if let Some(name) = &place.workspace {
        let workspace = match daemon.read(|store| store.workspace(&place.project, name)) {
            Ok(Some(workspace)) => workspace,
            _ => return Err(format!("the workspace {name} is gone")),
        };
        let (workspace, heads) = workspaces::follow(daemon, workspace).await;
        let head = workspace
            .repositories
            .iter()
            .zip(&heads)
            .find(|(repository, _)| repository.path == checkout.path)
            .map(|(_, head)| head.clone())
            .unwrap_or(Head::Unknown);
        return Ok(head.branch_or(&workspace.branch).to_owned());
    }
    match git::output(
        &checkout.folder,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        Class::Read,
    )
    .await
    {
        Ok(branch) if !branch.trim().is_empty() => Ok(branch.trim().to_owned()),
        _ => Err(format!(
            "refused: {} is on no branch: name the branch (branch)",
            checkout.path
        )),
    }
}

/// The commit a branch's CI files are read at: the remote's, as last fetched, which is what the
/// forge runs; else the local branch, said as such. The ref and how it is said.
async fn tree_of(checkout: &super::Checkout, branch: &str) -> Option<(String, String)> {
    let remote = &checkout.remote;
    let candidates = [
        (
            format!("refs/remotes/{remote}/{branch}"),
            format!("as {remote}/{branch} holds it"),
        ),
        (
            format!("refs/heads/{branch}"),
            format!(
                "as the local branch holds it: {branch} is not on {remote} as last fetched, and \
                 the forge runs what is pushed"
            ),
        ),
    ];
    for (reference, said) in candidates {
        let verified = git::run(
            &checkout.folder,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ],
            Class::Read,
        )
        .await;
        if verified.is_ok_and(|v| v.success) {
            return Some((reference, said));
        }
    }
    None
}

/// A file of the CI at a commit, parsed, with what could not be done of it (a note per problem).
struct Parsed {
    yaml: Yaml,
    notes: Vec<String>,
}

/// Why a file of the CI could not be read: it is not at that commit, git could not give it, or it
/// is there and the parser refuses it (its message says where).
enum Unread {
    Missing,
    Git(String),
    Yaml(String),
    /// Its size in bytes, past `FILE_LIMIT`.
    Large(u64),
}

impl Unread {
    /// A file of a workflow folder, said after its path.
    fn said(&self) -> String {
        match self {
            Unread::Missing => "is not in the commit".to_owned(),
            Unread::Git(why) => format!("cannot be read by git: {why}"),
            Unread::Yaml(why) => format!("cannot be parsed: {why}"),
            Unread::Large(size) => format!("is {size} bytes, past the limit of {LIMIT_SAID}"),
        }
    }

    /// The refusal of a call that needs the file `BITBUCKET_FILE`.
    fn refusal(&self, branch: &str, slug: &str, how: &str) -> String {
        match self {
            Unread::Missing => format!(
                "refused: {branch} of {slug} has no {BITBUCKET_FILE}: it is not in the commit, \
                 {how}"
            ),
            Unread::Git(why) => format!(
                "refused: {BITBUCKET_FILE} of {branch} of {slug} cannot be read by git, {how}: \
                 {why}"
            ),
            Unread::Yaml(why) => format!(
                "refused: {branch} of {slug} has a {BITBUCKET_FILE} that cannot be parsed, {how}: \
                 {why}; fix the file, or name the pipeline to forge run, which does not need this \
                 list"
            ),
            Unread::Large(size) => format!(
                "refused: {BITBUCKET_FILE} of {branch} of {slug} is {size} bytes, {how}: the \
                 limit is {LIMIT_SAID}; name the pipeline to forge run, which does not need this \
                 list"
            ),
        }
    }
}

/// A file at a commit, parsed, a repeated key read as the forge reads it (the last one kept, said
/// in the notes), merge keys (`<<: *anchor`) applied: why not when it cannot be read.
async fn yaml_at(
    checkout: &super::Checkout,
    reference: &str,
    path: &str,
) -> Result<Parsed, Unread> {
    // Its size first: `git show` reads the whole of it into memory.
    let size = git::output(
        &checkout.folder,
        &["cat-file", "-s", &format!("{reference}:{path}")],
        Class::Read,
    )
    .await
    .ok()
    .and_then(|size| size.trim().parse::<u64>().ok());
    if let Some(size) = size.filter(|size| *size > FILE_LIMIT) {
        return Err(Unread::Large(size));
    }
    let shown = git::run(
        &checkout.folder,
        &["show", &format!("{reference}:{path}")],
        Class::Read,
    )
    .await;
    let text = match shown {
        Ok(output) if output.success => output.stdout,
        Ok(output) => {
            // `git show` says the same for a missing path and for a path it cannot read: the tree
            // tells them apart.
            let listed = git::output(
                &checkout.folder,
                &["ls-tree", "--name-only", reference, "--", path],
                Class::Read,
            )
            .await;
            return Err(match listed {
                Ok(names) if names.trim().is_empty() => Unread::Missing,
                _ => Unread::Git(output.stderr.trim().to_owned()),
            });
        }
        Err(why) => return Err(Unread::Git(why)),
    };
    let (mut yaml, mut notes) =
        yaml::parse(&text, path).map_err(|e| Unread::Yaml(e.to_string()))?;
    // `Value` keeps `<<` as a key of its own: the anchors it names are merged in, or the file is
    // used as written and the note says so.
    let mut merged = yaml.clone();
    match merged.apply_merge() {
        Ok(()) => yaml = merged,
        Err(why) => notes.push(format!(
            "{path}: a merge key (<<) could not be applied ({why}): what it merges is missing \
             from this list"
        )),
    }
    Ok(Parsed { yaml, notes })
}

/// A YAML scalar as words.
pub(super) fn scalar(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(text) => Some(text.clone()),
        Yaml::Number(number) => Some(number.to_string()),
        Yaml::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The keys of a mapping, as words; a merge key that could not be applied is no name.
fn keys(value: &Yaml) -> Vec<String> {
    value
        .as_mapping()
        .into_iter()
        .flat_map(|map| map.keys())
        .filter_map(scalar)
        .filter(|key| key != "<<")
        .collect()
}

/// `pipelines`: what can be run on the branch, from the CI's own files.
async fn listed(checkout: &super::Checkout, forge: &Forge, path: &str, branch: &str) -> Outcome {
    let slug = forge.slug();
    let Some((reference, said)) = tree_of(checkout, branch).await else {
        return Outcome::Refused(format!(
            "refused: {branch} is neither on {} as last fetched nor a local branch of {path}",
            checkout.remote
        ));
    };
    let (file, lines, notes, example) = match forge {
        Forge::Bitbucket { .. } => {
            let parsed = match yaml_at(checkout, &reference, BITBUCKET_FILE).await {
                Ok(parsed) => parsed,
                Err(unread) => {
                    return Outcome::Refused(super::masked(&unread.refusal(branch, &slug, &said)));
                }
            };
            (
                BITBUCKET_FILE.to_owned(),
                bitbucket_pipelines(&parsed.yaml),
                parsed.notes,
                "\"pipeline\": \"custom: <name>\", \"variables\": [{\"key\": \"K\", \"value\": \
                 \"v\", \"secured\": false}]",
            )
        }
        Forge::GitHub { .. } => {
            let (lines, notes) = github_workflows(checkout, &reference).await;
            (
                format!("{WORKFLOWS}/"),
                lines,
                notes,
                "\"pipeline\": \"<its file>\", \"variables\": [{\"key\": \"input\", \"value\": \"v\"}]",
            )
        }
    };
    let unread = match notes.is_empty() {
        true => String::new(),
        false => {
            let notes: String = notes.iter().map(|note| format!("- {note}\n")).collect();
            format!("Not read:\n{notes}")
        }
    };
    if lines.is_empty() {
        return Outcome::Done(super::masked(&format!(
            "{slug} has nothing that can be run on {branch}: {file} holds none that could be \
             read, {said}.\n{unread}"
        )));
    }
    let lines: String = lines.iter().map(|line| format!("- {line}\n")).collect();
    Outcome::Done(super::masked(&format!(
        "What can be run in {slug} on {branch}, from {file} {said}:\n{lines}{unread}Run one with \
         forge {{\"action\": \"run\", \"repository\": {}, {example}}}.",
        Value::from(path)
    )))
}

/// The pipelines of a `bitbucket-pipelines.yml`: `default`, each of `branches`, `tags` and
/// `pull-requests` by its pattern, and each `custom` one by its name, with the variables its first
/// item declares.
fn bitbucket_pipelines(yaml: &Yaml) -> Vec<String> {
    let pipelines = &yaml["pipelines"];
    let mut lines = Vec::new();
    if !pipelines["default"].is_null() {
        lines.push("default".to_owned());
    }
    for kind in ["branches", "tags", "pull-requests"] {
        for pattern in keys(&pipelines[kind]) {
            lines.push(format!("{kind}: {pattern}"));
        }
    }
    for name in keys(&pipelines["custom"]) {
        let declared = pipelines["custom"][name.as_str()]
            .as_sequence()
            .and_then(|items| items.iter().find(|item| !item["variables"].is_null()))
            .and_then(|item| item["variables"].as_sequence().cloned())
            .unwrap_or_default();
        let variables: Vec<String> = declared.iter().filter_map(bitbucket_variable).collect();
        match variables.is_empty() {
            true => lines.push(format!("custom: {name}")),
            false => lines.push(format!(
                "custom: {name}, variables: {}",
                variables.join(", ")
            )),
        }
    }
    lines
}

/// A variable a custom pipeline declares: its name, its default and the values it allows.
fn bitbucket_variable(variable: &Yaml) -> Option<String> {
    let name = scalar(&variable["name"])?;
    let mut said = Vec::new();
    if let Some(default) = scalar(&variable["default"]) {
        said.push(format!("default {default}"));
    }
    let allowed: Vec<String> = variable["allowed-values"]
        .as_sequence()
        .into_iter()
        .flatten()
        .filter_map(scalar)
        .collect();
    if !allowed.is_empty() {
        said.push(format!("one of {}", allowed.join(", ")));
    }
    if let Some(description) = scalar(&variable["description"]) {
        said.push(description);
    }
    Some(match said.is_empty() {
        true => name,
        false => format!("{name} ({})", said.join(", ")),
    })
}

/// The workflows of the commit that can be dispatched (`on: workflow_dispatch`), with their
/// inputs; and a note per workflow that could not be read, which never hides the others.
async fn github_workflows(
    checkout: &super::Checkout,
    reference: &str,
) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    let names = match git::output(
        &checkout.folder,
        &[
            "ls-tree",
            "--name-only",
            reference,
            &format!("{WORKFLOWS}/"),
        ],
        Class::Read,
    )
    .await
    {
        Ok(names) => names,
        Err(why) => {
            notes.push(format!("{WORKFLOWS}/ cannot be listed by git: {why}"));
            String::new()
        }
    };
    let mut lines = Vec::new();
    for path in names
        .lines()
        .filter(|p| p.ends_with(".yml") || p.ends_with(".yaml"))
    {
        let parsed = match yaml_at(checkout, reference, path).await {
            Ok(parsed) => parsed,
            Err(unread) => {
                notes.push(format!("{path} {}", unread.said()));
                continue;
            }
        };
        notes.extend(parsed.notes);
        let yaml = parsed.yaml;
        let on = &yaml["on"];
        let dispatched = match on {
            Yaml::String(event) => event == "workflow_dispatch",
            Yaml::Sequence(events) => events
                .iter()
                .any(|e| e.as_str() == Some("workflow_dispatch")),
            Yaml::Mapping(map) => map.contains_key("workflow_dispatch"),
            _ => false,
        };
        if !dispatched {
            continue;
        }
        let file = path.rsplit('/').next().unwrap_or(path);
        let name = scalar(&yaml["name"]).unwrap_or_else(|| file.to_owned());
        let inputs = &on["workflow_dispatch"]["inputs"];
        let inputs: Vec<String> = keys(inputs)
            .into_iter()
            .map(|input| github_input(&input, &inputs[input.as_str()]))
            .collect();
        match inputs.is_empty() {
            true => lines.push(format!("{name} ({path})")),
            false => lines.push(format!("{name} ({path}), inputs: {}", inputs.join(", "))),
        }
    }
    (lines, notes)
}

/// An input of a workflow: whether it is required, its type when it is not a text, the values it
/// allows, its default, then its description.
fn github_input(name: &str, input: &Yaml) -> String {
    let mut said = Vec::new();
    if input["required"].as_bool() == Some(true) {
        said.push("required".to_owned());
    }
    if let Some(kind) =
        scalar(&input["type"]).filter(|k| !["string", "choice"].contains(&k.as_str()))
    {
        said.push(kind);
    }
    let options: Vec<String> = input["options"]
        .as_sequence()
        .into_iter()
        .flatten()
        .filter_map(scalar)
        .collect();
    if !options.is_empty() {
        said.push(format!("one of {}", options.join(", ")));
    }
    if let Some(default) = scalar(&input["default"]) {
        said.push(format!("default {default}"));
    }
    let mut text = said.join(", ");
    if let Some(description) = scalar(&input["description"]) {
        text = match text.is_empty() {
            true => description,
            false => format!("{text}; {description}"),
        };
    }
    match text.is_empty() {
        true => name.to_owned(),
        false => format!("{name} ({text})"),
    }
}

/// A state as words: `IN_PROGRESS` is `in progress`.
fn worded(state: &str) -> String {
    state.to_lowercase().replace('_', " ")
}

/// Which pipeline of the file a Bitbucket run ran: `default`, `custom: <name>`…
fn pipeline_named(pipeline: &Value) -> String {
    let selector = &pipeline["target"]["selector"];
    match (selector["type"].as_str(), selector["pattern"].as_str()) {
        (None | Some("default"), _) => "default".to_owned(),
        (Some(kind), Some(pattern)) => format!("{kind}: {pattern}"),
        (Some(kind), None) => kind.to_owned(),
    }
}

/// `runs` on Bitbucket: the newest pipelines of the repository, of the branch when there is one.
async fn bitbucket_runs(forge: &Forge, context: &str, branch: Option<&str>) -> Outcome {
    let slug = forge.slug();
    let path = format!("{}/pipelines/", bitbucket::cloud_repository(forge));
    // The branch is asked of the API: the repository's newest 50 hold none of a quiet branch's
    // older runs.
    let named = branch.map(|branch| format!("target.ref_name={branch}"));
    let mut params = vec!["sort=-created_on"];
    params.extend(named.as_deref());
    let pipelines =
        match bitbucket::cloud_values_with(context, &path, bitbucket::PAGE, &params).await {
            Ok((pipelines, _)) => pipelines,
            Err(why) => return Outcome::Failed(why),
        };
    let lines: Vec<String> = pipelines
        .iter()
        .filter(|p| branch.is_none() || bitbucket::pipeline_branch(p) == branch)
        .take(RUNS)
        .map(|p| {
            let state = &p["state"];
            let state = match state["result"]["name"].as_str().filter(|r| !r.is_empty()) {
                Some(result) => worded(result),
                None => worded(state["name"].as_str().unwrap_or("unknown")),
            };
            let mut line = format!(
                "- #{} {}, {state}, {}, started {}",
                p["build_number"].as_u64().unwrap_or(0),
                pipeline_named(p),
                worded(p["trigger"]["name"].as_str().unwrap_or("unknown trigger")),
                p["created_on"].as_str().unwrap_or("at an unknown time")
            );
            if let Some(seconds) = p["duration_in_seconds"].as_u64().filter(|s| *s > 0) {
                line.push_str(&format!(", {}", events::lasted(seconds)));
            }
            if branch.is_none()
                && let Some(on) = bitbucket::pipeline_branch(p)
            {
                line.push_str(&format!(", on {on}"));
            }
            line
        })
        .collect();
    runs_text(
        &slug,
        branch,
        lines,
        match branch {
            Some(_) => "the newest 50 pipelines of the branch",
            None => "the newest 50 pipelines of the repository",
        },
    )
}

/// What `runs` answers.
fn runs_text(slug: &str, branch: Option<&str>, lines: Vec<String>, among: &str) -> Outcome {
    let of = match branch {
        Some(branch) => format!("{branch} in {slug}"),
        None => slug.to_owned(),
    };
    if lines.is_empty() {
        return Outcome::Done(format!("No run of {of} among {among}."));
    }
    Outcome::Done(super::masked(&format!(
        "The recent runs of {of}, newest first:\n{}\nRead one with forge {{\"action\": \
         \"run_view\", \"run\": <its number>}}.",
        lines.join("\n")
    )))
}

/// `bkt pipeline view <run> --json`: the run and its steps.
pub(super) async fn bitbucket_seen(
    forge: &Forge,
    context: &str,
    run: u64,
) -> Result<Value, String> {
    let run = run.to_string();
    bitbucket::json(&bitbucket::command(
        forge,
        context,
        &["pipeline", "view", &run],
        &["--json"],
    ))
    .await
}

/// A step's state: its own, then its result when it has one.
fn step_state(step: &Value) -> String {
    let state = &step["state"];
    let mut said = worded(state["name"].as_str().unwrap_or("unknown"));
    let result = state["result"]["name"]
        .as_str()
        .or(step["result"]["name"].as_str())
        .filter(|r| !r.is_empty());
    if let Some(result) = result {
        said.push_str(&format!(", {}", worded(result)));
    }
    said
}

/// `run_view` on Bitbucket.
async fn bitbucket_view(forge: &Forge, context: &str, run: u64, path: &str) -> Outcome {
    let seen = match bitbucket_seen(forge, context, run).await {
        Ok(seen) => seen,
        Err(why) => return Outcome::Failed(why),
    };
    let lines: Vec<String> = seen["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| {
            format!(
                "- {}: {}",
                step["name"].as_str().unwrap_or("a step with no name"),
                step_state(step)
            )
        })
        .collect();
    let Forge::Bitbucket { owner, name, .. } = forge else {
        unreachable!("a Bitbucket repository")
    };
    Outcome::Done(super::masked(&format!(
        "The steps of the run #{run} of {}, https://bitbucket.org/{owner}/{name}/pipelines/results/{run}:\n{}\n\
         Read a step's log with forge {{\"action\": \"run_logs\", \"repository\": {}, \"run\": \
         {run}, \"step\": <its name>}}.",
        forge.slug(),
        lines.join("\n"),
        Value::from(path)
    )))
}

/// The id of the step named `step` (or whose uuid it is) among those of a run; only what a uuid is
/// made of, since it goes to a command line.
fn step_uuid<'a>(seen: &'a Value, step: &str) -> Option<&'a str> {
    seen["steps"]
        .as_array()?
        .iter()
        .find(|s| s["name"].as_str() == Some(step) || s["uuid"].as_str() == Some(step))?["uuid"]
        .as_str()
        .filter(|uuid| {
            uuid.chars()
                .all(|c| c.is_ascii_alphanumeric() || "{}-".contains(c))
        })
}

/// A log as it is shown: its lines cleaned of what a terminal adds.
fn log_text(header: String, log: &str) -> Outcome {
    let lines: Vec<String> = log.lines().map(logs::clean).collect();
    Outcome::Done(format!("{header}\n{}", lines.join("\n")))
}

/// `run_logs` on Bitbucket.
async fn bitbucket_log(forge: &Forge, context: &str, run: u64, step: &str) -> Outcome {
    let seen = match bitbucket_seen(forge, context, run).await {
        Ok(seen) => seen,
        Err(why) => return Outcome::Failed(why),
    };
    let Some(uuid) = step_uuid(&seen, step) else {
        return Outcome::Refused(format!(
            "refused: the run #{run} of {} has no step {step:?}: forge run_view lists them",
            forge.slug()
        ));
    };
    let built = run.to_string();
    match bitbucket::text(&bitbucket::command(
        forge,
        context,
        &["pipeline", "logs", &built],
        &["--step", uuid],
    ))
    .await
    {
        Ok(log) => log_text(
            format!(
                "The log of the step {step} of the run #{run} of {}:",
                forge.slug()
            ),
            &log,
        ),
        Err(why) => Outcome::Failed(why),
    }
}

/// `run` on Bitbucket: `bkt pipeline run` when no variable is given; with variables, the API's own
/// call through `bkt api`, since `bkt`'s `--var` splits a value at its commas and has no secured
/// variable. The run started, when the forge said its number.
async fn bitbucket_run(
    forge: &Forge,
    context: &str,
    pipeline: &str,
    branch: &str,
    variables: &[Variable],
) -> (Outcome, Option<Started>) {
    let slug = forge.slug();
    let pipeline = pipeline.trim();
    let selector = match pipeline.split_once(':') {
        _ if pipeline == "default" => None,
        Some((kind, pattern)) => Some((kind.trim(), pattern.trim())),
        None => Some(("custom", pipeline)),
    };
    if let Some((kind, pattern)) = selector
        && (!["custom", "branches", "tags", "pull-requests"].contains(&kind)
            || pattern.is_empty()
            || pattern.starts_with('-'))
    {
        return (
            Outcome::Refused(format!(
                "refused: {pipeline:?} is no pipeline of Bitbucket: default, custom: <name>, \
                 branches: <pattern>, tags: <pattern> or pull-requests: <pattern>, as forge \
                 pipelines lists them"
            )),
            None,
        );
    }
    let named = match selector {
        None => "default".to_owned(),
        Some((kind, pattern)) => format!("{kind}: {pattern}"),
    };
    let number = if variables.is_empty() {
        let mut flags = vec!["--ref", branch];
        if let Some((kind, pattern)) = selector {
            flags.extend(["--selector-type", kind, "--selector-pattern", pattern]);
        }
        let args = bitbucket::command(forge, context, &["pipeline", "run"], &flags);
        match bitbucket::text(&args).await {
            // It may say the run's number (`#43`); the API's answer always does.
            Ok(said) => match said
                .split('#')
                .nth(1)
                .map(|rest| {
                    rest.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                })
                .and_then(|digits| digits.parse::<u64>().ok())
            {
                Some(number) => Some(number),
                None => number_of(forge, context, &said).await,
            },
            Err(why) => return (Outcome::Failed(why), None),
        }
    } else {
        let mut target = json!({"type": "pipeline_ref_target", "ref_type": "branch",
            "ref_name": branch});
        if let Some((kind, pattern)) = selector {
            target["selector"] = json!({"type": kind, "pattern": pattern});
        }
        let variables: Vec<Value> = variables
            .iter()
            .map(|v| json!({"key": v.key, "value": v.value, "secured": v.secured}))
            .collect();
        let body = json!({"target": target, "variables": variables}).to_string();
        let path = format!("{}/pipelines/", bitbucket::cloud_repository(forge));
        let args: [&str; 8] = [
            "api",
            &path,
            "--context",
            context,
            "--method",
            "POST",
            "--input",
            &body,
        ];
        match bitbucket::json(&args).await {
            Ok(created) => created["build_number"].as_u64(),
            Err(why) => return (Outcome::Failed(why), None),
        }
    };
    let Forge::Bitbucket { owner, name, .. } = forge else {
        unreachable!("a Bitbucket repository")
    };
    let link =
        |number: u64| format!("https://bitbucket.org/{owner}/{name}/pipelines/results/{number}");
    let said = match number {
        Some(number) => format!("#{number}, {}", link(number)),
        None => "forge runs lists it".to_owned(),
    };
    let started = number.map(|number| Started {
        number,
        pipeline: named.clone(),
        branch: branch.to_owned(),
        link: link(number),
    });
    (
        Outcome::Done(format!(
            "The pipeline {named} was started on {branch} in {slug}: {said}."
        )),
        started,
    )
}

/// The number of the run `bkt pipeline run` named by its uuid (`{…}`) alone, read from the API;
/// none when it named neither, or the read failed.
async fn number_of(forge: &Forge, context: &str, said: &str) -> Option<u64> {
    let uuid = said
        .split_whitespace()
        .find(|word| word.starts_with('{') && word.ends_with('}') && word.len() > 2)
        .filter(|uuid| {
            uuid.chars()
                .all(|c| c.is_ascii_alphanumeric() || "{}-".contains(c))
        })?;
    let path = format!("{}/pipelines/{uuid}", bitbucket::cloud_repository(forge));
    bitbucket::json(&["api", &path, "--context", context])
        .await
        .ok()?["build_number"]
        .as_u64()
}

/// `stop` on Bitbucket: the run's uuid, from `bkt pipeline view`, then the API's `stopPipeline`.
async fn bitbucket_stop(forge: &Forge, context: &str, run: u64) -> Outcome {
    let slug = forge.slug();
    let seen = match bitbucket_seen(forge, context, run).await {
        Ok(seen) => seen,
        Err(why) => return Outcome::Failed(why),
    };
    let uuid = seen["pipeline"]["uuid"].as_str().filter(|uuid| {
        uuid.chars()
            .all(|c| c.is_ascii_alphanumeric() || "{}-".contains(c))
    });
    let Some(uuid) = uuid else {
        return Outcome::Failed(format!(
            "bkt pipeline view {run} says no uuid of the run, which stopping it needs"
        ));
    };
    let path = format!(
        "{}/pipelines/{uuid}/stopPipeline",
        bitbucket::cloud_repository(forge)
    );
    match bitbucket::text(&["api", &path, "--context", context, "--method", "POST"]).await {
        Ok(_) => Outcome::Done(format!("The run #{run} of {slug} was asked to stop.")),
        Err(why) => Outcome::Failed(why),
    }
}

/// `runs` on GitHub.
async fn github_runs(forge: &Forge, branch: Option<&str>) -> Outcome {
    let repo = forge.gh_repository();
    let limit = RUNS.to_string();
    let mut args = vec!["run", "list", "-R", &repo];
    if let Some(branch) = branch {
        args.extend(["--branch", branch]);
    }
    args.extend([
        "--limit",
        &limit,
        "--json",
        "databaseId,workflowName,status,conclusion,event,headBranch,createdAt,startedAt,\
         updatedAt,url",
    ]);
    let runs = match gh_json(&args).await {
        Ok(runs) => runs,
        Err(why) => return Outcome::Failed(why),
    };
    let lines: Vec<String> = runs
        .as_array()
        .into_iter()
        .flatten()
        .map(|run| {
            let state = match run["status"].as_str() {
                Some("completed") => run["conclusion"].as_str().unwrap_or("completed"),
                other => other.unwrap_or("unknown"),
            };
            let mut line = format!(
                "- {} {}, {}, {}, started {}",
                run["databaseId"].as_u64().unwrap_or(0),
                run["workflowName"].as_str().unwrap_or("a workflow"),
                worded(state),
                run["event"].as_str().unwrap_or("unknown event"),
                run["startedAt"]
                    .as_str()
                    .or(run["createdAt"].as_str())
                    .unwrap_or("at an unknown time")
            );
            if run["status"] == "completed"
                && let Some(seconds) = ci::between(&run["startedAt"], &run["updatedAt"])
            {
                line.push_str(&format!(", {}", events::lasted(seconds)));
            }
            if branch.is_none()
                && let Some(on) = run["headBranch"].as_str()
            {
                line.push_str(&format!(", on {on}"));
            }
            if let Some(url) = run["url"].as_str() {
                line.push_str(&format!("  {url}"));
            }
            line
        })
        .collect();
    runs_text(&forge.slug(), branch, lines, "the newest 20 runs")
}

/// `gh run view <run> --json …`: the run and its jobs.
async fn github_seen(forge: &Forge, run: u64) -> Result<Value, String> {
    let id = run.to_string();
    gh_json(&[
        "run",
        "view",
        &id,
        "-R",
        &forge.gh_repository(),
        "--json",
        "databaseId,workflowName,status,conclusion,url,jobs",
    ])
    .await
}

/// `run_view` on GitHub.
async fn github_view(forge: &Forge, run: u64, path: &str) -> Outcome {
    let seen = match github_seen(forge, run).await {
        Ok(seen) => seen,
        Err(why) => return Outcome::Failed(why),
    };
    let lines: Vec<String> = seen["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|job| {
            let mut state = job["status"].as_str().unwrap_or("unknown").to_owned();
            if let Some(conclusion) = job["conclusion"].as_str().filter(|c| !c.is_empty()) {
                state.push_str(&format!(", {conclusion}"));
            }
            format!(
                "- {} ({}): {}",
                job["name"].as_str().unwrap_or("a job with no name"),
                job["databaseId"].as_u64().unwrap_or(0),
                worded(&state)
            )
        })
        .collect();
    let state = match seen["status"].as_str() {
        Some("completed") => seen["conclusion"].as_str().unwrap_or("completed"),
        other => other.unwrap_or("unknown"),
    };
    Outcome::Done(super::masked(&format!(
        "The run {run} of {}, {}: {}  {}\n{}\nRead a job's log with forge {{\"action\": \
         \"run_logs\", \"repository\": {}, \"run\": {run}, \"step\": <its name>}}.",
        forge.slug(),
        seen["workflowName"].as_str().unwrap_or("a workflow"),
        worded(state),
        seen["url"].as_str().unwrap_or_default(),
        lines.join("\n"),
        Value::from(path)
    )))
}

/// The id of the job named `step` (or whose id it is) of a run.
fn job_id(seen: &Value, step: &str) -> Option<u64> {
    seen["jobs"].as_array()?.iter().find(|job| {
        job["name"].as_str() == Some(step)
            || job["databaseId"]
                .as_u64()
                .map(|id| id.to_string())
                .as_deref()
                == Some(step)
    })?["databaseId"]
        .as_u64()
}

/// Whether the run `run` holds the job `job`, which is what `step` `rerun` needs on GitHub.
pub(super) async fn github_holds(forge: &Forge, run: u64, job: u64) -> bool {
    match github_seen(forge, run).await {
        Ok(seen) => job_id(&seen, &job.to_string()).is_some(),
        Err(_) => false,
    }
}

/// The job `step` of a run, or the refusal that says it has none.
async fn github_job(forge: &Forge, run: u64, step: &str) -> Result<u64, Outcome> {
    let seen = github_seen(forge, run).await.map_err(Outcome::Failed)?;
    job_id(&seen, step).ok_or_else(|| {
        Outcome::Refused(format!(
            "refused: the run {run} of {} has no job {step:?}: forge run_view lists them",
            forge.slug()
        ))
    })
}

/// `run_logs` on GitHub: a job's whole log.
async fn github_log(forge: &Forge, run: u64, step: &str) -> Outcome {
    let job = match github_job(forge, run, step).await {
        Ok(job) => job.to_string(),
        Err(refusal) => return refusal,
    };
    let repo = forge.gh_repository();
    match gh_text(&["run", "view", "--job", &job, "-R", &repo, "--log"]).await {
        Ok(log) => log_text(
            format!(
                "The log of the job {step} of the run {run} of {}:",
                forge.slug()
            ),
            &log,
        ),
        Err(why) => Outcome::Failed(why),
    }
}

/// `run` on GitHub: `gh workflow run`, each variable an input.
async fn github_run(
    forge: &Forge,
    workflow: &str,
    branch: &str,
    variables: &[Variable],
) -> (Outcome, Option<Started>) {
    let repo = forge.gh_repository();
    let workflow = workflow.trim();
    let fields: Vec<String> = variables
        .iter()
        .map(|v| format!("{}={}", v.key, v.value))
        .collect();
    let mut args = vec!["workflow", "run", workflow, "-R", &repo, "--ref", branch];
    for field in &fields {
        args.extend(["-f", field]);
    }
    match gh_text(&args).await {
        Ok(said) => {
            // It prints the run's link when GitHub gives it.
            let link = said
                .split_whitespace()
                .find(|word| word.starts_with("https://") && word.contains("/actions/runs/"));
            let started = link.and_then(|link| {
                Some(Started {
                    number: link.rsplit('/').next()?.parse().ok()?,
                    pipeline: workflow.to_owned(),
                    branch: branch.to_owned(),
                    link: link.to_owned(),
                })
            });
            (
                Outcome::Done(format!(
                    "The workflow {workflow} was started on {branch} in {}: {}.",
                    forge.slug(),
                    link.unwrap_or("forge runs lists it")
                )),
                started,
            )
        }
        Err(why) => (Outcome::Failed(why), None),
    }
}

/// `step` `rerun` on GitHub: `gh run rerun --job`.
async fn github_rerun(forge: &Forge, run: u64, step: &str) -> Outcome {
    let job = match github_job(forge, run, step).await {
        Ok(job) => job.to_string(),
        Err(refusal) => return refusal,
    };
    let id = run.to_string();
    let repo = forge.gh_repository();
    match gh_text(&["run", "rerun", &id, "-R", &repo, "--job", &job]).await {
        Ok(_) => Outcome::Done(format!(
            "The job {step} of the run {run} of {} was started again.",
            forge.slug()
        )),
        Err(why) => Outcome::Failed(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secured(key: &str, value: &str) -> Variable {
        Variable {
            key: key.into(),
            value: value.into(),
            secured: true,
        }
    }

    #[test]
    fn a_short_secured_value_is_masked_only_where_it_is_echoed() {
        let echoed = "PREVIEW_TOKEN=4 refused; {\"key\":\"PREVIEW_TOKEN\",\"value\":\"4\"} in #43";
        assert_eq!(
            hidden(echoed.into(), &[secured("PREVIEW_TOKEN", "4")]),
            "PREVIEW_TOKEN=[masked] refused; {\"key\":\"PREVIEW_TOKEN\",\"value\":\"[masked]\"} in #43"
        );
    }

    #[test]
    fn a_secured_value_of_three_characters_or_more_is_masked_everywhere() {
        let said = hidden("token abc, then abc".into(), &[secured("T", "abc")]);
        assert_eq!(said, "token [masked], then [masked]");
    }

    #[test]
    fn an_unsecured_value_is_left_alone() {
        let plain = Variable {
            key: "T".into(),
            value: "4".into(),
            secured: false,
        };
        assert_eq!(hidden("T=4 #43".into(), &[plain]), "T=4 #43");
    }

    #[test]
    fn bitbucket_takes_letters_digits_and_underscores_in_a_name() {
        assert!(bitbucket_key("PREVIEW_SEED_2").is_none());
        for key in ["PREVIEW-SEED", "1SEED", "", "A B"] {
            assert!(bitbucket_key(key).is_some(), "{key:?}");
        }
    }
}
