//! The `workspace.*` methods: workspaces made of one worktree per repository, prepared in the
//! background by the project's recipe, and removed safely (`docs/workspaces.md`,
//! `docs/protocol.md`).
//!
//! Git and the steps never run inside a store transaction nor on the daemon's thread: git and the
//! run steps are child processes awaited by tokio, copies and links run on the blocking pool.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use flottille_core::command::split_line;
use flottille_core::glob;
use flottille_core::project::{Project, RecipeStep, RepositorySet, StepKind, valid_name};
use flottille_core::protocol::{WorkspaceCreate, WorkspaceList, WorkspaceName};
use flottille_core::secrets;
use flottille_core::store::now_ms;
use flottille_core::variables::{self, Names};
use flottille_core::workspace::{
    Action, Base, Failure as StepFailure, State, Step, StepState, Workspace, Worktree,
};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::daemon::Daemon;
use crate::git::{self, Class};
use crate::projects::{self, Failure};

type Answer = Result<Value, Failure>;

/// The lines of a failure's output that are kept, and at most how many characters of them.
const FAILURE_LINES: usize = 40;
const FAILURE_CHARS: usize = 8000;
/// What is read of a run step's output, from its end.
const RUN_OUTPUT: usize = 64 * 1024;

/// Answers a `workspace.*` request.
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    match method {
        "workspace.create" => create(daemon, serde_json::from_value(params)?).await,
        "workspace.list" => {
            let WorkspaceList { project } = match params {
                Value::Null => WorkspaceList::default(),
                params => serde_json::from_value(params)?,
            };
            let workspaces = daemon.read(|store| store.workspaces(project.as_deref()))?;
            Ok(json!({"workspaces": workspaces.iter().map(Workspace::to_json).collect::<Vec<_>>()}))
        }
        "workspace.show" => Ok(find(&daemon, &serde_json::from_value(params)?)?.to_json()),
        "workspace.prepare" => resume(daemon, serde_json::from_value(params)?).await,
        "workspace.remove" => remove(daemon, serde_json::from_value(params)?).await,
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

fn find(daemon: &Daemon, asked: &WorkspaceName) -> Result<Workspace, Failure> {
    daemon
        .read(|store| store.workspace(&asked.project, &asked.name))?
        .ok_or_else(|| {
            Failure::new(
                "not-found",
                format!(
                    "project {} has no workspace named {}",
                    asked.project, asked.name
                ),
            )
        })
}

/// What every event of a workspace starts with.
fn about(workspace: &Workspace) -> serde_json::Map<String, Value> {
    let mut data = serde_json::Map::new();
    data.insert("project".into(), workspace.project.clone().into());
    data.insert("workspace".into(), workspace.name.clone().into());
    data
}

fn with(mut data: serde_json::Map<String, Value>, more: Value) -> Value {
    if let Value::Object(more) = more {
        data.extend(more);
    }
    Value::Object(data)
}

// Creation.

async fn create(daemon: Arc<Daemon>, asked: WorkspaceCreate) -> Answer {
    let project = projects::load_project(&daemon, &asked.project)?;
    if !valid_name(&asked.name) {
        return Err(Failure::refused(format!(
            "{} is not a name for a workspace: use 1 to 64 letters, digits, dots, dashes or \
             underscores, starting with a letter or a digit",
            asked.name
        )));
    }
    let taken = taken_repositories(&project, asked.repositories.as_deref())?;
    let branch = match asked.branch {
        Some(branch) => {
            check_branch(&branch)?;
            branch
        }
        None => format!("{}/{}", project.branch_prefix, asked.name),
    };
    let folder = project.workspaces_root.join(&asked.name);
    if daemon
        .read(|store| store.workspace(&project.name, &asked.name))?
        .is_some()
    {
        return Err(Failure::refused(format!(
            "project {} already has a workspace named {}",
            project.name, asked.name
        )));
    }
    if std::fs::symlink_metadata(&folder).is_ok() {
        return Err(Failure::refused(format!(
            "{} already exists: remove it or choose another name",
            folder.display()
        )));
    }

    let mut repositories = Vec::new();
    for path in &taken {
        let repository = project
            .repositories
            .iter()
            .find(|r| &r.path == path)
            .expect("a repository taken is one of the project's");
        let main = main_checkout(&project.folder, path);
        let base = up_to_date_base(
            &main,
            path,
            &repository.remote,
            &repository.base,
            asked.offline,
        )
        .await?;
        let exists = git::run(
            &main,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("refs/heads/{branch}"),
            ],
            Class::Read,
        )
        .await
        .map_err(Failure::refused)?;
        if exists.success {
            return Err(Failure::refused(format!(
                "the branch {branch} already exists in {path}: choose another name or branch"
            )));
        }
        repositories.push(Worktree {
            path: path.clone(),
            base,
        });
    }

    let workspace = Workspace {
        id: 0,
        project: project.name.clone(),
        name: asked.name.clone(),
        branch,
        folder,
        project_folder: project.folder.clone(),
        created_at: now_ms(),
        variables: project.variables.clone(),
        steps: snapshot(&project, &taken),
        repositories,
    };
    let event = with(
        about(&workspace),
        json!({
            "branch": workspace.branch,
            "folder": workspace.folder,
            "repositories": workspace.repositories,
        }),
    );
    let id =
        daemon.write(|store| store.create_workspace(&workspace, ("workspace.created", event)))?;

    // The answer waits for the worktrees; the rest of the recipe goes on in the background.
    let (made, worktrees) = oneshot::channel();
    daemon.preparing.lock().unwrap().insert(id);
    tokio::spawn(prepare(daemon.clone(), id, Some(made)));
    let _ = worktrees.await;
    Ok(daemon.read(|store| store.workspace_by_id(id))?.to_json())
}

/// The repositories a new workspace takes: the root one first, then in dependency order.
fn taken_repositories(project: &Project, asked: Option<&[String]>) -> Result<Vec<String>, Failure> {
    let wanted: Vec<String> = match (asked, &project.default_repositories) {
        (Some(paths), _) => paths.to_vec(),
        (None, RepositorySet::Only(paths)) => paths.clone(),
        (None, RepositorySet::All) => project
            .repositories
            .iter()
            .map(|r| r.path.clone())
            .collect(),
    };
    if wanted.is_empty() {
        return Err(Failure::refused(
            "a workspace takes at least one repository",
        ));
    }
    if let Some(unknown) = wanted
        .iter()
        .find(|path| !project.repositories.iter().any(|r| &r.path == *path))
    {
        return Err(Failure::refused(format!(
            "project {} has no repository {unknown}",
            project.name
        )));
    }
    let mut taken: Vec<String> = project
        .in_dependency_order()
        .into_iter()
        .map(|r| r.path.clone())
        .filter(|path| wanted.contains(path))
        .collect();
    // The others are made inside its worktree.
    if let Some(root) = taken.iter().position(|path| path == ".") {
        let root = taken.remove(root);
        taken.insert(0, root);
    }
    Ok(taken)
}

fn check_branch(branch: &str) -> Result<(), Failure> {
    let bad = branch.is_empty()
        || branch.starts_with(['-', '/'])
        || branch.ends_with(['/', '.'])
        || branch.contains("..")
        || branch.contains("//")
        || branch.ends_with(".lock")
        || branch
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    if bad {
        return Err(Failure::refused(format!(
            "{branch} is not a branch name git accepts"
        )));
    }
    Ok(())
}

fn main_checkout(project_folder: &Path, repository: &str) -> PathBuf {
    match repository {
        "." => project_folder.to_owned(),
        path => project_folder.join(path),
    }
}

/// The base of one repository's branch: the first branch of the rule that is on the remote,
/// fetched into its tracking ref. Offline, the first one with a tracking ref, as it is.
async fn up_to_date_base(
    main: &Path,
    path: &str,
    remote: &str,
    rule: &[String],
    offline: bool,
) -> Result<Base, Failure> {
    let tracking = |branch: &str| format!("refs/remotes/{remote}/{branch}");
    let resolve = async |branch: &str| {
        git::run(
            main,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{}^{{commit}}", tracking(branch)),
            ],
            Class::Read,
        )
        .await
        .map(|output| output.success.then(|| output.stdout.trim().to_owned()))
        .map_err(Failure::refused)
    };
    if offline {
        for branch in rule {
            if let Some(commit) = resolve(branch).await? {
                let updated = git::run(
                    main,
                    &[
                        "log",
                        "-g",
                        "-n1",
                        "--format=%ct",
                        "--end-of-options",
                        &tracking(branch),
                    ],
                    Class::Read,
                )
                .await
                .map_err(Failure::refused)?;
                let fetched_at = updated
                    .success
                    .then(|| updated.stdout.trim().parse::<u64>().ok())
                    .flatten()
                    .map(|seconds| seconds * 1000);
                return Ok(Base {
                    branch: branch.clone(),
                    remote: remote.to_owned(),
                    reference: tracking(branch),
                    commit,
                    fetched_now: false,
                    fetched_at,
                });
            }
        }
        return Err(Failure::refused(format!(
            "{path} has no tracking ref of {remote} for {}: create the workspace online once",
            rule.join(" nor ")
        )));
    }

    let cannot_fetch = |reason: String| {
        Failure::refused(format!(
            "cannot fetch the base of {path} from {remote}: {}; nothing was created (with \
             --offline, the tracking refs are used as they are)",
            // Git's few lines, as one sentence.
            reason.split_whitespace().collect::<Vec<_>>().join(" ")
        ))
    };
    let heads = git::output(
        main,
        &["ls-remote", "--heads", "--end-of-options", remote],
        Class::Read,
    )
    .await
    .map_err(cannot_fetch)?;
    let on_remote: Vec<&String> = rule
        .iter()
        .filter(|branch| {
            heads
                .lines()
                .any(|line| line.split('\t').nth(1) == Some(&format!("refs/heads/{branch}")))
        })
        .collect();
    let Some(branch) = on_remote.first() else {
        return Err(Failure::refused(format!(
            "none of the base branches of {path} ({}) is on {remote}",
            rule.join(", ")
        )));
    };
    // Every candidate there is, so that the tracking refs are fresh for a later offline creation.
    let mut fetch: Vec<String> = ["fetch", "--quiet", "--no-tags", "--end-of-options", remote]
        .map(String::from)
        .into();
    fetch.extend(
        on_remote
            .iter()
            .map(|branch| format!("+refs/heads/{branch}:{}", tracking(branch))),
    );
    git::output(main, &fetch, Class::Read)
        .await
        .map_err(cannot_fetch)?;
    let commit = resolve(branch).await?.ok_or_else(|| {
        Failure::refused(format!(
            "the fetch of {path} left no tracking ref {}",
            tracking(branch)
        ))
    })?;
    Ok(Base {
        branch: (*branch).clone(),
        remote: remote.to_owned(),
        reference: tracking(branch),
        commit,
        fetched_now: true,
        fetched_at: Some(now_ms()),
    })
}

/// The steps of a new workspace: a worktree per repository, then the recipe as it is now, a step
/// for each repository expanded into one per repository.
fn snapshot(project: &Project, taken: &[String]) -> Vec<Step> {
    let mut steps: Vec<(Option<String>, Action)> = taken
        .iter()
        .map(|path| (Some(path.clone()), Action::Worktree))
        .collect();
    let in_order: Vec<String> = project
        .in_dependency_order()
        .into_iter()
        .map(|r| r.path.clone())
        .filter(|path| taken.contains(path))
        .collect();
    for step in &project.recipe {
        let action = action_of(project, step);
        let places: Vec<Option<String>> = if step.for_each_repository {
            in_order.iter().cloned().map(Some).collect()
        } else {
            match (step.kind(), &step.repository) {
                (_, Some(path)) => vec![Some(path.clone())],
                (StepKind::Command(name), None) => vec![project.commands[name].repository.clone()],
                _ => vec![None],
            }
        };
        steps.extend(places.into_iter().map(|place| (place, action.clone())));
    }
    steps
        .into_iter()
        .enumerate()
        .map(|(position, (repository, action))| Step {
            position: position as u32,
            repository,
            action,
            state: StepState::Pending,
            failure: None,
        })
        .collect()
}

fn action_of(project: &Project, step: &RecipeStep) -> Action {
    match step.kind() {
        StepKind::Copy(globs) => Action::Copy {
            globs: globs.to_vec(),
        },
        StepKind::Link(path) => Action::Link { path: path.into() },
        StepKind::RootEntries => Action::RootEntries {
            except: project
                .repositories
                .iter()
                .filter_map(|r| r.path.split('/').next().filter(|top| *top != "."))
                .map(String::from)
                .collect(),
        },
        StepKind::Run(line) => Action::Run {
            line: line.into(),
            command: None,
        },
        StepKind::Command(name) => Action::Run {
            line: project.commands[name].line.clone(),
            command: Some(name.into()),
        },
    }
}

// Preparation.

/// Runs the pending steps in order until one fails. `made` is told once the worktrees are made,
/// or when it stops before.
async fn prepare(daemon: Arc<Daemon>, id: i64, mut made: Option<oneshot::Sender<()>>) {
    if let Err(error) = prepare_steps(&daemon, id, &mut made).await {
        eprintln!(
            "flottille daemon: preparing workspace {id}: {}",
            error.message
        );
    }
    daemon.preparing.lock().unwrap().remove(&id);
    if let Some(made) = made {
        let _ = made.send(());
    }
}

async fn prepare_steps(
    daemon: &Daemon,
    id: i64,
    made: &mut Option<oneshot::Sender<()>>,
) -> Result<(), Failure> {
    loop {
        let workspace = daemon.read(|store| store.workspace_by_id(id))?;
        if workspace
            .steps
            .iter()
            .any(|step| matches!(step.state, StepState::Failed | StepState::Running))
        {
            return Ok(());
        }
        let Some(step) = workspace
            .steps
            .iter()
            .find(|step| step.state == StepState::Pending)
            .cloned()
        else {
            daemon.write(|store| {
                store
                    .append_event("workspace.ready", Value::Object(about(&workspace)))
                    .map(|event| ((), event))
            })?;
            return Ok(());
        };
        if step.action != Action::Worktree
            && let Some(made) = made.take()
        {
            let _ = made.send(());
        }
        let described = with(
            about(&workspace),
            json!({
                "position": step.position,
                "kind": kind_name(&step.action),
                "repository": step.repository,
                "doing": step.doing(),
            }),
        );
        daemon.write(|store| {
            store
                .set_step(
                    id,
                    step.position,
                    StepState::Running,
                    None,
                    ("workspace.step_started", described.clone()),
                )
                .map(|event| ((), event))
        })?;

        let outcome = run_step(&workspace, &step).await;

        let (state, failure, name) = match outcome {
            Ok(StepState::Skipped) => (StepState::Skipped, None, "workspace.step_skipped"),
            Ok(_) => (StepState::Done, None, "workspace.step_done"),
            Err(failure) => (StepState::Failed, Some(failure), "workspace.step_failed"),
        };
        let mut data = described.clone();
        if let Some(failure) = &failure {
            data["output"] = failure.output.clone().into();
        }
        daemon.write(|store| {
            store
                .set_step(id, step.position, state, failure.as_ref(), (name, data))
                .map(|event| ((), event))
        })?;
        if failure.is_some() {
            return Ok(());
        }
    }
}

fn kind_name(action: &Action) -> &'static str {
    match action {
        Action::Worktree => "worktree",
        Action::Copy { .. } => "copy",
        Action::Link { .. } => "link",
        Action::RootEntries { .. } => "root_entries",
        Action::Run { .. } => "run",
    }
}

/// Does one step: done, skipped, or its failure.
async fn run_step(workspace: &Workspace, step: &Step) -> Result<StepState, StepFailure> {
    let fail = |output: String| StepFailure {
        doing: step.doing(),
        output,
    };
    if let Some(path) = &step.repository
        && !workspace.repositories.iter().any(|r| &r.path == path)
    {
        return Ok(StepState::Skipped);
    }
    let place = step.repository.clone().unwrap_or_else(|| ".".into());
    let source = main_checkout(&workspace.project_folder, &place);
    let target = workspace.folder_of(&place);
    match &step.action {
        Action::Worktree => make_worktree(workspace, &place).await.map_err(fail)?,
        Action::Copy { globs } => {
            let globs = globs.clone();
            blocking(move || copy(&source, &target, &globs))
                .await
                .map_err(fail)?
        }
        Action::Link { path } => {
            let (from, to) = (source.join(path), target.join(path));
            blocking(move || link(&from, &to)).await.map_err(fail)?
        }
        Action::RootEntries { except } => {
            let (folder, root, except) = (
                workspace.project_folder.clone(),
                workspace.folder.clone(),
                except.clone(),
            );
            blocking(move || {
                for entry in root_entries(&folder, &root, &except)? {
                    link(&folder.join(&entry), &root.join(&entry))?;
                }
                Ok(())
            })
            .await
            .map_err(fail)?
        }
        Action::Run { line, .. } => run_line(workspace, &place, line, &target)
            .await
            .map_err(fail)?,
    }
    Ok(StepState::Done)
}

async fn blocking(
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| Err(format!("the step stopped: {error}")))
}

async fn make_worktree(workspace: &Workspace, repository: &str) -> Result<(), String> {
    let main = main_checkout(&workspace.project_folder, repository);
    let target = workspace.folder_of(repository);
    let parent = target.parent().unwrap_or(&target).to_owned();
    std::fs::create_dir_all(&parent)
        .map_err(|error| format!("cannot make {}: {error}", parent.display()))?;
    // A daemon that stopped may have made it without recording it.
    if std::fs::symlink_metadata(target.join(".git")).is_ok() {
        return Ok(());
    }
    // Clears what a folder removed by hand left registered.
    git::output(&main, &["worktree", "prune"], Class::Work).await?;
    let branch = &workspace.branch;
    let exists = git::run(
        &main,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("refs/heads/{branch}"),
        ],
        Class::Read,
    )
    .await?
    .success;
    let target = target.to_string_lossy().into_owned();
    if exists {
        // Made by an earlier attempt: taken again, never made twice.
        git::output(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "--end-of-options",
                &target,
                branch,
            ],
            Class::Work,
        )
        .await?;
    } else {
        let base = &workspace
            .repositories
            .iter()
            .find(|r| r.path == repository)
            .expect("a worktree step is for a repository of the workspace")
            .base
            .commit;
        git::output(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                branch,
                "--end-of-options",
                &target,
                base,
            ],
            Class::Work,
        )
        .await?;
    }
    Ok(())
}

/// What the globs match in `source`, file by file, with where each goes in `target`, leaving out
/// what is there already: what a copy step has left to do.
fn copy_plan(
    source: &Path,
    target: &Path,
    globs: &[String],
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let cannot =
        |path: &Path, error: std::io::Error| format!("cannot read {}: {error}", path.display());
    let mut plan = Vec::new();
    let mut pending: Vec<PathBuf> = Vec::new();
    for pattern in globs {
        pending.extend(glob::expand(source, pattern).map_err(|e| cannot(source, e))?);
    }
    while let Some(relative) = pending.pop() {
        let from = source.join(&relative);
        let metadata = std::fs::symlink_metadata(&from).map_err(|e| cannot(&from, e))?;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(&from).map_err(|e| cannot(&from, e))? {
                let entry = entry.map_err(|e| cannot(&from, e))?;
                pending.push(relative.join(entry.file_name()));
            }
        } else if std::fs::symlink_metadata(target.join(&relative)).is_err() {
            plan.push((from, target.join(&relative)));
        }
    }
    plan.sort();
    plan.dedup();
    Ok(plan)
}

/// Copies from the main checkout, never over anything already there.
fn copy(source: &Path, target: &Path, globs: &[String]) -> Result<(), String> {
    for (from, to) in copy_plan(source, target, globs)? {
        let failed = |error: std::io::Error| {
            format!(
                "cannot copy {} to {}: {error}",
                from.display(),
                to.display()
            )
        };
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        let metadata = std::fs::symlink_metadata(&from).map_err(failed)?;
        if metadata.is_symlink() {
            let pointed = std::fs::read_link(&from).map_err(failed)?;
            match std::os::unix::fs::symlink(pointed, &to) {
                Err(error) if error.kind() != ErrorKind::AlreadyExists => {
                    return Err(failed(error));
                }
                _ => continue,
            }
        }
        let mut reader = std::fs::File::open(&from).map_err(failed)?;
        // `create_new`: a file that appeared meanwhile is kept, never written over.
        let mut writer = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&to)
        {
            Ok(writer) => writer,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(failed(error)),
        };
        std::io::copy(&mut reader, &mut writer).map_err(failed)?;
        writer
            .set_permissions(metadata.permissions())
            .map_err(failed)?;
    }
    Ok(())
}

/// Links `from` at `to`, unless something is there already.
fn link(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Ok(());
    }
    if std::fs::symlink_metadata(from).is_err() {
        return Err(format!("{} is not in the main checkout", from.display()));
    }
    let failed = |error: std::io::Error| {
        format!(
            "cannot link {} at {}: {error}",
            from.display(),
            to.display()
        )
    };
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    match std::os::unix::fs::symlink(from, to) {
        Err(error) if error.kind() != ErrorKind::AlreadyExists => Err(failed(error)),
        _ => Ok(()),
    }
}

/// The top-level entries of the project folder a `root_entries` step has left to link: neither a
/// repository nor what holds one, nor `.git` nor `.gitignore` (linking those would make the
/// workspace folder look like the root repository's checkout with every file deleted), nor what
/// holds the workspace itself, nor anything already in the workspace folder.
fn root_entries(folder: &Path, root: &Path, except: &[String]) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(folder)
        .map_err(|error| format!("cannot read {}: {error}", folder.display()))?;
    let mut left = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read {}: {error}", folder.display()))?;
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        if matches!(name.as_str(), ".git" | ".gitignore")
            || except.contains(&name)
            || root.starts_with(folder.join(&name))
            || std::fs::symlink_metadata(root.join(&name)).is_ok()
        {
            continue;
        }
        left.push(name);
    }
    left.sort();
    Ok(left)
}

/// Runs one line in `dir`, with the workspace's variables. The error is the last lines of its
/// output, masked.
async fn run_line(
    workspace: &Workspace,
    repository: &str,
    line: &str,
    dir: &Path,
) -> Result<(), String> {
    let names = Names {
        workspace: workspace.name.clone(),
        workspace_path: workspace.folder.to_string_lossy().into_owned(),
        project: workspace.project.clone(),
        branch: workspace.branch.clone(),
        repository: repository.to_owned(),
    };
    let mut words = Vec::new();
    for word in split_line(line)? {
        words.push(variables::fill(&word, &names)?);
    }
    let mut filled = BTreeMap::new();
    for (name, template) in &workspace.variables {
        filled.insert(name.clone(), variables::fill(template, &names)?);
    }
    let env = variables::environment(std::env::vars_os(), &[&filled]);

    let (reader, writer) = std::io::pipe().map_err(|error| format!("cannot run it: {error}"))?;
    let mut command = tokio::process::Command::new(&words[0]);
    command
        .args(&words[1..])
        .env_clear()
        .envs(&env)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(
            writer
                .try_clone()
                .map_err(|error| format!("cannot run it: {error}"))?,
        )
        .stderr(writer)
        .process_group(0)
        .kill_on_drop(true);
    let spawned = command.spawn();
    // The command holds the pipe's writing end: dropped, the output ends with the process.
    drop(command);
    let mut child = spawned.map_err(|error| format!("cannot run {}: {error}", words[0]))?;
    let tail = tokio::task::spawn_blocking(move || read_tail(reader));
    let status = child
        .wait()
        .await
        .map_err(|error| format!("cannot wait for {}: {error}", words[0]))?;
    let output = tail.await.unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let kept = last_lines(&output);
    Err(if kept.is_empty() {
        match status.code() {
            Some(code) => format!("it ended with exit {code}"),
            None => "it ended with no exit code".into(),
        }
    } else {
        secrets::mask(&kept, &env)
    })
}

/// The output of a run, its last `RUN_OUTPUT` bytes.
fn read_tail(mut reader: std::io::PipeReader) -> String {
    let mut kept: Vec<u8> = Vec::new();
    let mut buffer = [0; 8192];
    while let Ok(read) = reader.read(&mut buffer) {
        if read == 0 {
            break;
        }
        kept.extend_from_slice(&buffer[..read]);
        if kept.len() > 2 * RUN_OUTPUT {
            kept.drain(..kept.len() - RUN_OUTPUT);
        }
    }
    if kept.len() > RUN_OUTPUT {
        kept.drain(..kept.len() - RUN_OUTPUT);
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// The last lines of an output, cut to its last characters.
fn last_lines(output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let kept = lines[lines.len().saturating_sub(FAILURE_LINES)..].join("\n");
    let count = kept.chars().count();
    kept.chars()
        .skip(count.saturating_sub(FAILURE_CHARS))
        .collect()
}

// Resume.

async fn resume(daemon: Arc<Daemon>, asked: WorkspaceName) -> Answer {
    let workspace = find(&daemon, &asked)?;
    if daemon.preparing.lock().unwrap().contains(&workspace.id) {
        return Err(being_prepared(&workspace));
    }
    let checked = workspace.clone();
    let redone = tokio::task::spawn_blocking(move || done_but_missing(&checked))
        .await
        .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    let resumed = reset(&daemon, &workspace, redone)?;
    let workspace = daemon.read(|store| store.workspace_by_id(workspace.id))?;
    if workspace.state() == State::Preparing
        && daemon.preparing.lock().unwrap().insert(workspace.id)
    {
        tokio::spawn(prepare(daemon.clone(), workspace.id, None));
    }
    let mut answer = workspace.to_json();
    answer["resumed"] = resumed;
    Ok(answer)
}

/// Puts back to pending the failed and running steps, and the `redone` ones: `{redone, retried}`.
fn reset(daemon: &Daemon, workspace: &Workspace, redone: Vec<u32>) -> Result<Value, Failure> {
    let retried: Vec<u32> = workspace
        .steps
        .iter()
        .filter(|step| matches!(step.state, StepState::Failed | StepState::Running))
        .map(|step| step.position)
        .collect();
    let counts = json!({"redone": redone.len(), "retried": retried.len()});
    if redone.is_empty() && retried.is_empty() {
        return Ok(counts);
    }
    let positions: Vec<u32> = retried.into_iter().chain(redone).collect();
    let event = with(about(workspace), counts.clone());
    daemon.write(|store| {
        store
            .reset_steps(workspace.id, &positions, ("workspace.resumed", event))
            .map(|event| ((), event))
    })?;
    Ok(counts)
}

/// The done steps, other than runs (never run twice), whose result is gone from the disk.
fn done_but_missing(workspace: &Workspace) -> Vec<u32> {
    workspace
        .steps
        .iter()
        .filter(|step| step.state == StepState::Done)
        .filter(|step| {
            let place = step.repository.as_deref().unwrap_or(".");
            let source = main_checkout(&workspace.project_folder, place);
            let target = workspace.folder_of(place);
            match &step.action {
                Action::Worktree => std::fs::symlink_metadata(target.join(".git")).is_err(),
                Action::Copy { globs } => {
                    copy_plan(&source, &target, globs).is_ok_and(|plan| !plan.is_empty())
                }
                Action::Link { path } => std::fs::symlink_metadata(target.join(path)).is_err(),
                Action::RootEntries { except } => {
                    root_entries(&workspace.project_folder, &workspace.folder, except)
                        .is_ok_and(|left| !left.is_empty())
                }
                Action::Run { .. } => false,
            }
        })
        .map(|step| step.position)
        .collect()
}

fn being_prepared(workspace: &Workspace) -> Failure {
    Failure::refused(format!(
        "workspace {} of project {} is being prepared: wait until it is done (`flottille \
         workspace show`)",
        workspace.name, workspace.project
    ))
}

/// At the daemon's start, the workspaces left preparing go on, in the background. A failed one
/// waits for the user.
pub fn resume_at_start(daemon: &Arc<Daemon>) {
    let workspaces = match daemon.read(|store| store.workspaces(None)) {
        Ok(workspaces) => workspaces,
        Err(error) => {
            eprintln!("flottille daemon: reading workspaces: {error}");
            return;
        }
    };
    for workspace in workspaces {
        if workspace.state() != State::Preparing {
            continue;
        }
        if let Err(error) = reset(daemon, &workspace, Vec::new()) {
            eprintln!(
                "flottille daemon: resuming {}: {}",
                workspace.name, error.message
            );
            continue;
        }
        daemon.preparing.lock().unwrap().insert(workspace.id);
        tokio::spawn(prepare(daemon.clone(), workspace.id, None));
    }
}

// Removal.

async fn remove(daemon: Arc<Daemon>, asked: WorkspaceName) -> Answer {
    let workspace = find(&daemon, &asked)?;
    // Held for the whole removal, so that no preparation starts meanwhile.
    if !daemon.preparing.lock().unwrap().insert(workspace.id) {
        return Err(being_prepared(&workspace));
    }
    let removed = remove_held(&daemon, &workspace).await;
    daemon.preparing.lock().unwrap().remove(&workspace.id);
    removed
}

async fn remove_held(daemon: &Daemon, workspace: &Workspace) -> Answer {
    let made: Vec<&Worktree> = workspace
        .repositories
        .iter()
        .filter(|r| std::fs::symlink_metadata(workspace.folder_of(&r.path).join(".git")).is_ok())
        .collect();
    let mut problems = Vec::new();
    let mut ours = Vec::new();
    for worktree in &made {
        let dir = workspace.folder_of(&worktree.path);
        let status = git::output(
            &dir,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            Class::Work,
        )
        .await
        .map_err(|error| cannot_check(&worktree.path, &error))?;
        let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
        while let Some(entry) = entries.next() {
            let (code, path) = entry.split_at(3.min(entry.len()));
            // A rename or a copy names where it came from next.
            if code.starts_with(['R', 'C']) {
                entries.next();
            }
            let file = dir.join(path);
            if code == "?? " && made_by_flottille(workspace, &file) {
                ours.push(file);
                continue;
            }
            let shown = match worktree.path.as_str() {
                "." => path.to_owned(),
                repository => format!("{repository}/{path}"),
            };
            problems.push(format!("{shown} holds work that is not committed"));
        }
        let commits = git::output(
            &dir,
            &["log", "--format=%h %s", "HEAD", "--not", "--remotes"],
            Class::Read,
        )
        .await
        .map_err(|error| cannot_check(&worktree.path, &error))?;
        let commits: Vec<&str> = commits.lines().filter(|line| !line.is_empty()).collect();
        if !commits.is_empty() {
            problems.push(format!(
                "{} has commits on no remote: {}",
                worktree.path,
                commits.join(", ")
            ));
        }
    }
    if !problems.is_empty() {
        return Err(Failure::refused(format!(
            "workspace {} of project {} is not removed, nothing was touched: {}",
            workspace.name,
            workspace.project,
            problems.join("; ")
        )));
    }

    for link in ours {
        std::fs::remove_file(&link).map_err(|error| {
            Failure::refused(format!("cannot remove {}: {error}", link.display()))
        })?;
    }
    // The deepest first: the others may stand inside the root one.
    let mut order = made.clone();
    order.sort_by_key(|r| std::cmp::Reverse(depth(&r.path)));
    for worktree in &order {
        let main = main_checkout(&workspace.project_folder, &worktree.path);
        let dir = workspace.folder_of(&worktree.path);
        git::output(
            &main,
            &[
                "worktree",
                "remove",
                "--end-of-options",
                &dir.to_string_lossy(),
            ],
            Class::Work,
        )
        .await
        .map_err(|error| {
            Failure::refused(format!(
                "cannot remove the worktree of {}: {error}",
                worktree.path
            ))
        })?;
    }
    for worktree in &workspace.repositories {
        let main = main_checkout(&workspace.project_folder, &worktree.path);
        git::output(&main, &["worktree", "prune"], Class::Work)
            .await
            .map_err(|error| {
                Failure::refused(format!("cannot prune {}: {error}", worktree.path))
            })?;
    }
    let folder = workspace.folder.clone();
    // Links are removed as links, never followed.
    blocking(move || match std::fs::remove_dir_all(&folder) {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            Err(format!("cannot remove {}: {error}", folder.display()))
        }
        _ => Ok(()),
    })
    .await
    .map_err(Failure::refused)?;
    if std::fs::symlink_metadata(&workspace.folder).is_ok() {
        return Err(Failure::refused(format!(
            "{} is still there after its removal",
            workspace.folder.display()
        )));
    }
    for worktree in &workspace.repositories {
        let main = main_checkout(&workspace.project_folder, &worktree.path);
        let list = git::output(&main, &["worktree", "list", "--porcelain"], Class::Read)
            .await
            .map_err(Failure::refused)?;
        let dir = workspace.folder_of(&worktree.path);
        if list
            .lines()
            .any(|line| line.strip_prefix("worktree ") == Some(&*dir.to_string_lossy()))
        {
            return Err(Failure::refused(format!(
                "git still lists the worktree {} of {}",
                dir.display(),
                worktree.path
            )));
        }
    }

    let event = Value::Object(about(workspace));
    daemon.write(|store| {
        store
            .remove_workspace(workspace.id, ("workspace.removed", event))
            .map(|event| ((), event))
    })?;
    Ok(json!({"removed": workspace.name, "project": workspace.project}))
}

fn cannot_check(repository: &str, error: &str) -> Failure {
    Failure::refused(format!(
        "cannot check the worktree of {repository}, nothing was touched: {error}"
    ))
}

/// What Flottille put in a worktree that git sees as untracked: a link into the project folder
/// (a `link` or `root_entries` step), or another worktree of the workspace standing in it.
fn made_by_flottille(workspace: &Workspace, file: &Path) -> bool {
    let file = file.components().collect::<PathBuf>();
    let link = std::fs::symlink_metadata(&file).is_ok_and(|m| m.is_symlink())
        && std::fs::read_link(&file).is_ok_and(|to| to.starts_with(&workspace.project_folder));
    link || workspace
        .repositories
        .iter()
        .any(|r| r.path != "." && workspace.folder_of(&r.path) == file)
}

fn depth(path: &str) -> usize {
    match path {
        "." => 0,
        path => path.split('/').count(),
    }
}
