//! The `workspace.*` methods: workspaces made of one worktree per repository, prepared in the
//! background by the project's recipe, and removed safely (`docs/workspaces.md`,
//! `docs/protocol.md`).
//!
//! Git and the steps never run inside a store transaction nor on the daemon's thread: git and the
//! run steps are child processes awaited by tokio, copies and links run on the blocking pool.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use flottille_core::command::split_line;
use flottille_core::glob;
use flottille_core::project::{Project, RecipeStep, RepositorySet, StepKind, valid_name};
use flottille_core::protocol::{WorkspaceCreate, WorkspaceList, WorkspaceName};
use flottille_core::secrets;
use flottille_core::store::{Store, now_ms};
use flottille_core::variables::{self, Names};
use flottille_core::workspace::{
    Action, Base, Failure as StepFailure, State, Step, StepState, Workspace, Worktree,
};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;

use crate::daemon::Daemon;
use crate::forge;
use crate::git::{self, Class};
use crate::mcp::{Outcome, arguments};
use crate::place::Place;
use crate::projects::{self, Failure};
use crate::tree;

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
        "workspace.show" => show(&daemon, serde_json::from_value(params)?).await,
        "workspace.forge" => crate::forge::call(daemon, params).await,
        "workspace.ticket" => crate::tickets::call(daemon, params).await,
        "workspace.link_ticket" => crate::tickets::link(daemon, params).await,
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

/// `workspace.show`: the workspace, and the lines changed in each of its repositories.
async fn show(daemon: &Daemon, asked: WorkspaceName) -> Answer {
    let workspace = find(daemon, &asked)?;
    let mut shown = workspace.to_json();
    if let Some(repositories) = shown["repositories"].as_array_mut() {
        for (repository, worktree) in repositories.iter_mut().zip(&workspace.repositories) {
            repository["changes"] =
                changes(&workspace.folder_of(&worktree.path), &worktree.base.commit)
                    .await
                    .unwrap_or(Value::Null);
        }
    }
    shown["ready_at"] = match shown["state"] == "ready" {
        true => daemon
            .read(|store| store.workspace_ready_at(workspace.id))?
            .map_or(Value::Null, Value::from),
        false => Value::Null,
    };
    Ok(shown)
}

/// A new file larger than this is not read to count its lines.
const COUNTED_FILE: u64 = 8 * 1024 * 1024;
/// How many new files, and how many of their bytes, are read at most: past them the count stops
/// and says so (`capped`), rather than reading a dependency folder git does not ignore.
const COUNTED_FILES: usize = 1000;
const COUNTED_BYTES: u64 = 16 * 1024 * 1024;

/// The lines added and removed in a worktree since its base commit: what its branch committed,
/// what is changed and not committed, and the new files git does not ignore. A binary file counts
/// no line. `None` when git cannot read the worktree (it is not made yet, or gone).
async fn changes(folder: &Path, base: &str) -> Option<Value> {
    // `diff-index`, not `diff`: `diff` refreshes the index and writes it, under the feet of an
    // agent's own `git add` or `commit` in the same worktree.
    let diff = git::run(
        folder,
        &["diff-index", "--numstat", base, "--"],
        Class::Read,
    )
    .await
    .ok()
    .filter(|output| output.success)?;
    let (mut added, mut removed) = (0u64, 0u64);
    for line in diff.stdout.lines() {
        let mut counts = line.split('\t');
        // A binary file is `-\t-\t<path>`: no line to count.
        if let (Some(Ok(plus)), Some(Ok(minus))) = (
            counts.next().map(str::parse::<u64>),
            counts.next().map(str::parse::<u64>),
        ) {
            added += plus;
            removed += minus;
        }
    }
    let new = git::run(
        folder,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        Class::Read,
    )
    .await
    .ok()
    .filter(|output| output.success)?;
    let (mut files, mut bytes, mut capped) = (0usize, 0u64, false);
    for path in new.stdout.split('\0').filter(|path| !path.is_empty()) {
        if files == COUNTED_FILES {
            capped = true;
            break;
        }
        files += 1;
        let Some(text) = new_file(&folder.join(path)).await else {
            continue;
        };
        if bytes + text.len() as u64 > COUNTED_BYTES {
            capped = true;
            break;
        }
        bytes += text.len() as u64;
        if text.contains(&0) {
            continue;
        }
        let breaks = text.iter().filter(|&&byte| byte == b'\n').count() as u64;
        added += breaks + u64::from(text.last().is_some_and(|&last| last != b'\n'));
    }
    Some(json!({"added": added, "removed": removed, "capped": capped}))
}

/// A new file's bytes, when it is a regular file of at most [`COUNTED_FILE`]. Opened once,
/// never through a link (it may lead out of the worktree) and without waiting on a pipe; what is
/// read is what was opened, checked after opening, so a file swapped for a link meanwhile is not
/// followed.
async fn new_file(path: &Path) -> Option<Vec<u8>> {
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .await
        .ok()?;
    let metadata = file.metadata().await.ok()?;
    if !metadata.is_file() || metadata.len() > COUNTED_FILE {
        return None;
    }
    let mut text = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(COUNTED_FILE + 1)
        .read_to_end(&mut text)
        .await
        .ok()?;
    (text.len() as u64 <= COUNTED_FILE).then_some(text)
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
    let ticket = asked
        .ticket
        .as_deref()
        .map(|text| crate::tickets::accepted(&project, text).map(|key| key.text()))
        .transpose()
        .map_err(Failure::refused)?;
    let branch = asked
        .branch
        .unwrap_or_else(|| format!("{}/{}", project.branch_prefix, asked.name));
    check_branch(&main_checkout(&project.folder, &taken[0]), &branch).await?;
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
        ticket,
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

/// Asks git itself whether `branch` is a branch name, before anything is written.
async fn check_branch(dir: &Path, branch: &str) -> Result<(), Failure> {
    let checked = git::run(dir, &["check-ref-format", "--branch", branch], Class::Read)
        .await
        .map_err(Failure::refused)?;
    // Git expands a shorthand like `@{-1}` into another name: not the one asked for.
    if !checked.success || checked.stdout.trim_end() != branch {
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
                    urls: forge::urls_of(main, remote).await,
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
        urls: forge::urls_of(main, remote).await,
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
    let time_limit = step.time_limit.unwrap_or(project.run_time_limit);
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
            time_limit,
        },
        StepKind::Command(name) => Action::Run {
            line: project.commands[name].line.clone(),
            command: Some(name.into()),
            time_limit,
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
                    .workspace_ready(id, Value::Object(about(&workspace)))
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
        Action::Run {
            line, time_limit, ..
        } => run_line(workspace, &place, line, &target, *time_limit)
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
        write_new(&to, &mut reader, metadata.permissions()).map_err(failed)?;
    }
    Ok(())
}

/// Writes what `reader` gives into `to`, unless something is there already (`create_new`: a file
/// that appeared meanwhile is kept, never written over). Failing halfway, it removes what it
/// wrote, so that resuming copies the file again instead of taking it as done.
fn write_new(
    to: &Path,
    reader: &mut impl std::io::Read,
    permissions: std::fs::Permissions,
) -> std::io::Result<()> {
    let mut writer = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
    {
        Ok(writer) => writer,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error),
    };
    let written =
        std::io::copy(reader, &mut writer).and_then(|_| writer.set_permissions(permissions));
    if written.is_err() {
        let _ = std::fs::remove_file(to);
    }
    written
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

/// Runs one line in `dir`, with the workspace's variables, for at most `time_limit` seconds. The
/// error is the last lines of its output, masked.
async fn run_line(
    workspace: &Workspace,
    repository: &str,
    line: &str,
    dir: &Path,
    time_limit: u64,
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
    // The command holds the pipe's writing end: dropped, only the processes hold it.
    drop(command);
    let mut child = spawned.map_err(|error| format!("cannot run {}: {error}", words[0]))?;
    let group = child.id();
    let mut reader = tokio::net::unix::pipe::Receiver::from_owned_fd(reader.into())
        .map_err(|error| format!("cannot read the output of {}: {error}", words[0]))?;
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    let mut open = true;
    let deadline = tokio::time::sleep(Duration::from_secs(time_limit));
    tokio::pin!(deadline);
    let status = loop {
        tokio::select! {
            status = child.wait() => break Some(status),
            read = reader.read(&mut buffer), if open => match read {
                Ok(0) | Err(_) => open = false,
                Ok(read) => keep_tail(&mut output, &buffer[..read]),
            },
            () = &mut deadline => break None,
        }
    };
    // What it left in the background holds the pipe and would keep the step running: it ends
    // with the step, and what is already in the pipe is read without waiting for it.
    if let Some(group) = group {
        // SAFETY: kill has no precondition; the group is the step's own (process_group(0)).
        unsafe {
            libc::kill(-(group as i32), libc::SIGKILL);
        }
    }
    while let Ok(read) = reader.try_read(&mut buffer) {
        if read == 0 {
            break;
        }
        keep_tail(&mut output, &buffer[..read]);
    }
    let output = String::from_utf8_lossy(&output);
    let kept = last_lines(&output);
    let Some(status) = status else {
        // Killed with its group: waited for until it has exited.
        let _ = child.wait().await;
        let stopped = format!("it was stopped after {time_limit} s");
        return Err(if kept.is_empty() {
            stopped
        } else {
            format!("{}\n{stopped}", secrets::mask(&kept, &env))
        });
    };
    let status = status.map_err(|error| format!("cannot wait for {}: {error}", words[0]))?;
    if status.success() {
        return Ok(());
    }
    Err(if kept.is_empty() {
        match status.code() {
            Some(code) => format!("it ended with exit {code}"),
            None => "it ended with no exit code".into(),
        }
    } else {
        secrets::mask(&kept, &env)
    })
}

/// Keeps the last `RUN_OUTPUT` bytes of a run's output.
fn keep_tail(kept: &mut Vec<u8>, read: &[u8]) {
    kept.extend_from_slice(read);
    if kept.len() > RUN_OUTPUT {
        kept.drain(..kept.len() - RUN_OUTPUT);
    }
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
    // Held from the start, as a removal does, so that none starts while the steps are checked.
    if !daemon.preparing.lock().unwrap().insert(workspace.id) {
        return Err(being_prepared(&workspace));
    }
    let resumed = resume_held(&daemon, &workspace).await;
    let preparing = resumed
        .as_ref()
        .is_ok_and(|(workspace, _)| workspace.state() == State::Preparing);
    if preparing {
        tokio::spawn(prepare(daemon.clone(), workspace.id, None));
    } else {
        daemon.preparing.lock().unwrap().remove(&workspace.id);
    }
    let (workspace, resumed) = resumed?;
    let mut answer = workspace.to_json();
    answer["resumed"] = resumed;
    Ok(answer)
}

/// The steps to do again put back to pending: the workspace as it is then, and `{redone, retried}`.
async fn resume_held(
    daemon: &Daemon,
    workspace: &Workspace,
) -> Result<(Workspace, Value), Failure> {
    let checked = workspace.clone();
    let redone = tokio::task::spawn_blocking(move || done_but_missing(&checked))
        .await
        .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    let resumed = reset(daemon, workspace, redone)?;
    Ok((
        daemon.read(|store| store.workspace_by_id(workspace.id))?,
        resumed,
    ))
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

/// What works in the workspace outside the tree whose root is `ours`: its agents not ended and
/// its command runs still running (`docs/design.md` §5.1), each as a sentence.
fn in_use(daemon: &Daemon, workspace: &Workspace, ours: &str) -> Result<Vec<String>, Failure> {
    let theirs = |agent: &str| tree::root_of(daemon, agent) != ours;
    let mut used = Vec::new();
    for agent in daemon.read(Store::agents)? {
        if agent.project == workspace.project
            && agent.workspace.as_deref() == Some(workspace.name.as_str())
            && !agent.state.is_final()
            && theirs(&agent.id)
        {
            used.push(format!(
                "the agent {}, outside this run, works in it",
                agent.id
            ));
        }
    }
    for run in daemon.read(Store::running_runs)? {
        if run.folder.starts_with(&workspace.folder) && theirs(&run.agent) {
            used.push(format!(
                "the command run {} ({}) runs in it",
                run.id, run.line
            ));
        }
    }
    Ok(used)
}

async fn remove_held(daemon: &Daemon, workspace: &Workspace) -> Answer {
    let made: Vec<&Worktree> = workspace
        .repositories
        .iter()
        .filter(|r| std::fs::symlink_metadata(workspace.folder_of(&r.path).join(".git")).is_ok())
        .collect();
    let mut problems = {
        let made: Vec<PathBuf> = made.iter().map(|r| workspace.folder_of(&r.path)).collect();
        let workspace = workspace.clone();
        tokio::task::spawn_blocking(move || outside_worktrees(&workspace, &made))
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?
            .map_err(Failure::refused)?
    };
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
            let file = dir.join(path).components().collect::<PathBuf>();
            if code == "?? " {
                // Another worktree standing in this one: checked, then removed, by itself.
                if made.iter().any(|r| workspace.folder_of(&r.path) == file) {
                    continue;
                }
                if made_link(workspace, &file) {
                    ours.push(file);
                    continue;
                }
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRemove {}

/// `workspace_remove`, once the gate let it through by a procedure's grant: the agent's own
/// workspace removed as `workspace.remove` removes it, then the branches of it that hold no commit
/// of their own deleted from their repositories (`docs/procedures.md`).
pub async fn workspace_remove(
    daemon: &Arc<Daemon>,
    agent: &str,
    place: &Place,
    args: Value,
) -> Outcome {
    if let Err(refusal) = arguments::<WorkspaceRemove>("workspace_remove", args) {
        return refusal;
    }
    let Some(name) = &place.workspace else {
        return Outcome::Refused(
            "refused: this agent works in the project folder, not in a workspace: there is no \
             workspace to remove"
                .into(),
        );
    };
    let asked = WorkspaceName {
        project: place.project.clone(),
        name: name.clone(),
    };
    let workspace = match find(daemon, &asked) {
        Ok(workspace) => workspace,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    let used = match in_use(daemon, &workspace, &tree::root_of(daemon, agent)) {
        Ok(used) => used,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    if !used.is_empty() {
        return Outcome::Refused(format!(
            "refused: workspace {} of project {} is not removed, nothing was touched: {}; they \
             end first",
            workspace.name,
            workspace.project,
            used.join("; ")
        ));
    }
    if let Err(failure) = remove(daemon.clone(), asked).await {
        return match failure.code {
            "refused" => Outcome::Refused(format!("refused: {}", failure.message)),
            _ => Outcome::Failed(failure.message),
        };
    }
    let (mut deleted, mut kept, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    for worktree in &workspace.repositories {
        let main = main_checkout(&workspace.project_folder, &worktree.path);
        match delete_if_empty(&main, &workspace.branch, &worktree.base.reference).await {
            Ok(true) => deleted.push(worktree.path.as_str()),
            Ok(false) => kept.push(worktree.path.as_str()),
            Err(error) => failed.push(format!("{}: {error}", worktree.path)),
        }
    }
    let mut said = format!(
        "Workspace {} of project {} is removed: its worktrees and its folder are gone.",
        workspace.name, workspace.project
    );
    if !deleted.is_empty() {
        said.push_str(&format!(
            " Its branch {}, empty, was deleted in {}.",
            workspace.branch,
            deleted.join(", ")
        ));
    }
    if !kept.is_empty() {
        said.push_str(&format!(
            " Its branch {}, which holds commits of its own, is kept in {}.",
            workspace.branch,
            kept.join(", ")
        ));
    }
    if !failed.is_empty() {
        said.push_str(&format!(
            " Its empty branch could not be deleted in {}.",
            failed.join("; ")
        ));
    }
    Outcome::Done(said)
}

/// Deletes the branch from the repository when every commit of it is in its base (`git branch
/// -D` once `merge-base --is-ancestor` said so, since a squash merge leaves `-d` unsure); true when
/// it did.
async fn delete_if_empty(main: &Path, branch: &str, base: &str) -> Result<bool, String> {
    let local = format!("refs/heads/{branch}");
    let empty = git::output(
        main,
        &[
            "merge-base",
            "--is-ancestor",
            "--end-of-options",
            &local,
            base,
        ],
        Class::Read,
    )
    .await
    .is_ok();
    if !empty {
        return Ok(false);
    }
    git::output(
        main,
        &["branch", "-D", "--end-of-options", branch],
        Class::Work,
    )
    .await?;
    Ok(true)
}

fn cannot_check(repository: &str, error: &str) -> Failure {
    Failure::refused(format!(
        "cannot check the worktree of {repository}, nothing was touched: {error}"
    ))
}

/// What the workspace folder holds outside the worktrees made, which no `git status` sees and the
/// removal of the folder would take: everything but the links Flottille made and the folders
/// holding nothing else (a parent of a worktree, an empty one), one problem each.
fn outside_worktrees(workspace: &Workspace, made: &[PathBuf]) -> Result<Vec<String>, String> {
    fn walk(
        dir: &Path,
        workspace: &Workspace,
        made: &[PathBuf],
        found: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        let cannot = |error: std::io::Error| format!("cannot read {}: {error}", dir.display());
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(cannot(error)),
        };
        for entry in entries {
            let path = entry.map_err(cannot)?.path();
            if made.contains(&path) || made_link(workspace, &path) {
                continue;
            }
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                walk(&path, workspace, made, found)?;
            } else {
                found.push(path);
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    if !made.contains(&workspace.folder) {
        walk(&workspace.folder, workspace, made, &mut found)?;
    }
    found.sort();
    Ok(found
        .iter()
        .map(|path| {
            let shown = path.strip_prefix(&workspace.folder).unwrap_or(path);
            format!("{} is in no worktree", shown.display())
        })
        .collect())
}

/// A link into the project folder, made by a `link` or `root_entries` step.
fn made_link(workspace: &Workspace, file: &Path) -> bool {
    std::fs::symlink_metadata(file).is_ok_and(|m| m.is_symlink())
        && std::fs::read_link(file).is_ok_and(|to| to.starts_with(&workspace.project_folder))
}

fn depth(path: &str) -> usize {
    match path {
        "." => 0,
        path => path.split('/').count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gives a few bytes, then fails.
    struct Failing(bool);

    impl std::io::Read for Failing {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if std::mem::replace(&mut self.0, true) {
                return Err(std::io::Error::other("the disk went away"));
            }
            buffer[..4].copy_from_slice(b"half");
            Ok(4)
        }
    }

    #[test]
    fn a_copy_failing_halfway_leaves_no_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join(".env");
        let permissions = std::fs::metadata(dir.path()).unwrap().permissions();

        let written = write_new(&to, &mut Failing(false), permissions);

        assert!(written.is_err());
        // Resuming must see it missing and copy it again, not take it as done.
        assert!(std::fs::symlink_metadata(&to).is_err());
    }
}
