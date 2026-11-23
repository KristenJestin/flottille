//! The `project.*` methods: registering folders and keeping each project's configuration
//! (`docs/protocol.md`, `docs/projects.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use flottille_core::agent::Agent;
use flottille_core::declared::RepositoryForge;
use flottille_core::detect;
use flottille_core::paths::Paths;
use flottille_core::project::{Project, slug, valid_name};
use flottille_core::protocol::{
    CommitChecks, Conventions, MessageChecks, ProjectFolder, ProjectName, ProjectRemove,
    ProjectRepository, ProjectSettings, ShownWorkspace,
};
use flottille_core::registry::Registry;
use flottille_core::run::Run;
use flottille_core::store::Store;
use flottille_core::workspace::Workspace;
use flottille_core::{Error, Result};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::git::{self, Class};
use crate::{tree, workspaces};

/// An error answer: a code of `docs/protocol.md` and a sentence.
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    /// What the message says, as data a client draws (`ErrorBody::data`).
    pub data: Option<Value>,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(self, data: Value) -> Failure {
        Failure {
            data: Some(data),
            ..self
        }
    }

    /// The daemon's answer to the request `id`.
    pub fn answer(self, id: Option<String>) -> flottille_protocol::Message {
        flottille_protocol::Message::error_with(id, self.code, self.message, self.data)
    }

    pub fn refused(message: impl Into<String>) -> Failure {
        Failure::new("refused", message)
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Failure {
        match error {
            Error::Refused(sentence) => Failure::refused(sentence),
            Error::Config(error) => Failure::new("invalid-config", error.to_string()),
            error => Failure::new("unavailable", error.to_string()),
        }
    }
}

impl From<serde_json::Error> for Failure {
    fn from(error: serde_json::Error) -> Failure {
        Failure::new("invalid-params", error.to_string())
    }
}

/// The projects' files, and the projects the daemon last saw in them: the difference is what it
/// announces, since the files are the user's to edit.
pub struct Projects {
    registry: Registry,
    data: PathBuf,
    known: BTreeMap<String, Project>,
}

impl Projects {
    /// The registry the projects are read from.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Takes the projects there are as the starting point: they are not news.
    pub fn open(paths: &Paths) -> Result<Projects> {
        let registry = Registry::new(paths);
        let known = registry
            .load_all()?
            .projects
            .into_iter()
            .map(|project| (project.name.clone(), project))
            .collect();
        Ok(Projects {
            registry,
            data: paths.data.clone(),
            known,
        })
    }
}

/// Answers a `project.*` request.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> std::result::Result<Value, Failure> {
    let mut projects = daemon.projects.lock().unwrap();
    let projects = &mut *projects;
    let loaded = announce_edits(daemon, projects)?;
    match method {
        "project.list" => Ok(json!({
            "projects": loaded.projects,
            "errors": loaded.invalid.iter().map(|error| json!({
                "name": error.name,
                "file": error.file(),
                "message": error.message,
            })).collect::<Vec<_>>(),
        })),
        "project.show" => {
            let ProjectName { name } = serde_json::from_value(params)?;
            Ok(serde_json::to_value(load(projects, &name)?)?)
        }
        "project.folders" => {
            let asked: FoldersAsked = serde_json::from_value(params)?;
            folders(projects, asked.path.as_deref())
        }
        "project.detect" => {
            let plan = plan(projects, serde_json::from_value(params)?)?;
            Ok(json!({
                "name": plan.name,
                "folder": plan.folder,
                "repositories": plan.repositories,
            }))
        }
        "project.add" => {
            let asked: ProjectFolder = serde_json::from_value(params)?;
            let confirmed = asked.repositories.clone();
            let plan = plan(projects, asked)?;
            if let Some(confirmed) = confirmed
                && confirmed != plan.repositories
            {
                return Err(Failure::refused(format!(
                    "the repositories of {} changed since they were shown: run it again",
                    plan.folder.display()
                )));
            }
            let project = Project::new(&plan.name, plan.folder, plan.repositories, &projects.data);
            projects.registry.create(&project)?;
            // The file names no folder for the workspaces: the project is what reads back.
            let project = load(projects, &project.name)?;
            projects.known.insert(project.name.clone(), project.clone());
            emit(
                daemon,
                "project.added",
                json!({"name": project.name, "folder": project.folder}),
            )?;
            Ok(serde_json::to_value(project)?)
        }
        "project.check" => {
            let ProjectName { name } = serde_json::from_value(params)?;
            let project = load(projects, &name)?;
            let detected = detect::repositories(&project.folder).map_err(|error| {
                Failure::refused(format!(
                    "the folder {} of project {name} no longer exists or cannot be read: {error}",
                    project.folder.display()
                ))
            })?;
            let configured: Vec<_> = project.repositories.iter().map(|r| &r.path).collect();
            Ok(json!({
                "name": name,
                "folder": project.folder,
                "new": detected.iter().filter(|path| !configured.contains(path)).collect::<Vec<_>>(),
                "missing": configured.iter().filter(|path| !detected.contains(path)).collect::<Vec<_>>(),
            }))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

// Removal (`docs/projects.md`, "Removing a project").

/// How long a removal waits for the processes of the project's agents to be gone once they
/// were stopped: each has its own grace, and they stop together.
fn release_wait() -> Duration {
    crate::test_time::tuned("FLOTTILLE_TEST_RELEASE_WAIT_MS", Duration::from_secs(15))
}

/// The refusal of anything that would start in a project being removed (an agent, a workspace,
/// a command run) or edit its file.
pub fn refuse_removing(daemon: &Daemon, project: &str) -> std::result::Result<(), Failure> {
    match daemon.removing_projects.lock().unwrap().contains(project) {
        true => Err(Failure::refused(format!(
            "project {project} is being removed: nothing starts in it and its file is not edited"
        ))),
        false => Ok(()),
    }
}

/// The agents of the project that have not ended, the shallowest first.
fn live_agents(daemon: &Daemon, project: &str) -> std::result::Result<Vec<Agent>, Failure> {
    let mut agents: Vec<Agent> = daemon
        .read(Store::agents)?
        .into_iter()
        .filter(|agent| agent.project == project && !agent.state.is_final())
        .collect();
    agents.sort_by_key(|agent| agent.depth);
    Ok(agents)
}

fn running_runs(daemon: &Daemon, project: &str) -> std::result::Result<Vec<Run>, Failure> {
    Ok(daemon
        .read(Store::running_runs)?
        .into_iter()
        .filter(|run| run.project == project)
        .collect())
}

/// The project's workspaces, by name.
fn workspaces_of(daemon: &Daemon, project: &str) -> std::result::Result<Vec<Workspace>, Failure> {
    let mut workspaces = daemon.read(|store| store.workspaces(Some(project)))?;
    workspaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(workspaces)
}

fn agent_json(agent: &Agent) -> Value {
    json!({"id": agent.id, "title": agent.title, "parent": agent.parent})
}

fn run_json(run: &Run) -> Value {
    json!({"run": run.id, "name": run.name, "line": run.line, "agent": run.agent})
}

/// `project.removal`: what removing the project would do, asked before it is: its sessions and
/// command runs still running, and each workspace with what removing it would lose.
pub async fn removal(daemon: Arc<Daemon>, params: Value) -> std::result::Result<Value, Failure> {
    let ProjectName { name } = serde_json::from_value(params)?;
    load_project(&daemon, &name)?;
    let sessions: Vec<Value> = live_agents(&daemon, &name)?
        .iter()
        .filter(|agent| agent.parent.is_none())
        .map(agent_json)
        .collect();
    let runs: Vec<Value> = running_runs(&daemon, &name)?.iter().map(run_json).collect();
    let mut workspaces = Vec::new();
    for workspace in workspaces_of(&daemon, &name)? {
        let (state, losses) = match workspaces::loss_state(&daemon, &workspace).await {
            Ok(found) => found,
            Err(failure) => ("unknown", vec![failure.message]),
        };
        // A state not read matches none: the workspace is kept.
        let digest = workspaces::digest(&workspace).await.unwrap_or_default();
        workspaces.push(json!({
            "name": workspace.name,
            "branch": workspace.branch,
            "state": state,
            "losses": losses,
            "digest": digest,
        }));
    }
    Ok(json!({
        "name": name,
        "sessions": sessions,
        "runs": runs,
        "workspaces": workspaces,
    }))
}

/// `project.remove`: stops the project's agents, then its command runs, removes its workspaces
/// (those that would lose work only with `discard`), then forgets it. Its folder and the
/// checkouts of its repositories are never touched. Nothing starts in it meanwhile.
pub async fn remove(daemon: Arc<Daemon>, params: Value) -> std::result::Result<Value, Failure> {
    let ProjectRemove { name, discard } = serde_json::from_value(params)?;
    load_project(&daemon, &name)?;
    // Marked before anything is stopped: nothing may start between a stop and the mark.
    if !daemon
        .removing_projects
        .lock()
        .unwrap()
        .insert(name.clone())
    {
        return Err(Failure::refused(format!(
            "project {name} is being removed already"
        )));
    }
    let removed = remove_marked(&daemon, &name, discard.as_deref()).await;
    daemon.removing_projects.lock().unwrap().remove(&name);
    removed
}

/// What a removal did so far, for its result or for the failure that stopped it partway.
#[derive(Default)]
struct Done {
    agents: Vec<Value>,
    runs: Vec<Value>,
    removed: Vec<Value>,
    kept: Vec<Value>,
}

impl Done {
    /// The failure of a removal stopped partway: what failed, what was done, and that the
    /// project is still known.
    fn partway(&self, name: &str, failed: &str) -> Failure {
        let names = |list: &[Value], key: &str| {
            list.iter()
                .map(|entry| entry[key].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut done = Vec::new();
        if !self.agents.is_empty() {
            done.push(format!("stopped the agents {}", names(&self.agents, "id")));
        }
        if !self.runs.is_empty() {
            done.push(format!(
                "stopped the command runs {}",
                names(&self.runs, "run")
            ));
        }
        if !self.removed.is_empty() {
            done.push(format!(
                "removed the workspaces {}",
                names(&self.removed, "name")
            ));
        }
        let done = match done.is_empty() {
            true => "nothing was done".to_owned(),
            false => format!("done: {}", done.join("; ")),
        };
        Failure::refused(format!(
            "project {name} is not removed, it is still known: {failed}; {done}"
        ))
    }
}

async fn remove_marked(
    daemon: &Arc<Daemon>,
    name: &str,
    discard: Option<&[ShownWorkspace]>,
) -> std::result::Result<Value, Failure> {
    let mut done = Done::default();
    // Read before the agents stop, whose runs end with them: all are said stopped. None starts
    // after it, the project being marked.
    let runs = running_runs(daemon, name)?;

    // The roots first: each ends with its tree, as Stop does.
    let agents = live_agents(daemon, name)?;
    for agent in &agents {
        let live = daemon
            .read(|store| store.agent(&agent.id))?
            .is_some_and(|agent| !agent.state.is_final());
        if live {
            tree::stop_tree(
                daemon,
                &agent.id,
                "stopped by the user: its project was removed",
            );
        }
    }
    done.agents = agents.iter().map(agent_json).collect();
    // A workspace is removed once nothing of an agent's runs in it any more: every agent of the
    // project, ended ones too, whose process an earlier removal's wait may have outlived.
    let all: Vec<Agent> = daemon
        .read(Store::agents)?
        .into_iter()
        .filter(|agent| agent.project == name)
        .collect();
    let deadline = Instant::now() + release_wait();
    loop {
        let stopping: Vec<&str> = all
            .iter()
            .filter(|agent| crate::agents::stopping(daemon, &agent.id))
            .map(|agent| agent.id.as_str())
            .collect();
        if stopping.is_empty() {
            break;
        }
        if Instant::now() > deadline {
            return Err(done.partway(
                name,
                &format!("the agents {} are still stopping", stopping.join(", ")),
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let left = live_agents(daemon, name)?;
    if !left.is_empty() {
        let ids: Vec<&str> = left.iter().map(|agent| agent.id.as_str()).collect();
        return Err(done.partway(
            name,
            &format!("the agents {} are still running", ids.join(", ")),
        ));
    }

    let going = running_runs(daemon, name)?;
    futures::future::join_all(
        going
            .iter()
            .map(|run| crate::runs::stop_by_user(daemon, &run.id)),
    )
    .await;
    done.runs = runs.iter().map(run_json).collect();
    let left = running_runs(daemon, name)?;
    if !left.is_empty() {
        let ids: Vec<&str> = left.iter().map(|run| run.id.as_str()).collect();
        return Err(done.partway(
            name,
            &format!("the command runs {} are still running", ids.join(", ")),
        ));
    }

    let mut failed = Vec::new();
    for workspace in workspaces_of(daemon, name)? {
        // A workspace whose losses cannot be read is kept: nothing is lost on a guess.
        let read = match workspaces::loss_state(daemon, &workspace).await {
            Ok((_, losses)) => workspaces::digest(&workspace)
                .await
                .map(|digest| (losses, digest)),
            Err(failure) => Err(failure),
        };
        let (losses, digest) = match read {
            Ok(read) => read,
            Err(failure) => {
                done.kept
                    .push(json!({"name": workspace.name, "why": [failure.message]}));
                continue;
            }
        };
        if !losses.is_empty() {
            let Some(shown) = discard else {
                done.kept
                    .push(json!({"name": workspace.name, "why": losses}));
                continue;
            };
            // The user discards what they were shown, never what appeared since, nor what was
            // written since in a file they were shown.
            let seen = shown.iter().find(|shown| {
                shown.name == workspace.name
                    && shown.digest == digest
                    && losses.iter().all(|loss| shown.losses.contains(loss))
            });
            if seen.is_none() {
                done.kept.push(json!({
                    "name": workspace.name,
                    "why": losses,
                    "changed": true,
                }));
                continue;
            }
        }
        let approved = (!losses.is_empty()).then_some((losses.as_slice(), digest.as_str()));
        match workspaces::remove_with_project(daemon.clone(), &workspace, approved).await {
            Ok(removed) => done.removed.push(json!({
                "name": workspace.name,
                "branch": workspace.branch,
                "branches": removed["branches"],
            })),
            Err(failure) => failed.push(failure.message),
        }
    }
    if !failed.is_empty() {
        return Err(done.partway(name, &failed.join("; ")));
    }

    let mut projects = daemon.projects.lock().unwrap();
    if !projects.registry.remove(name)? {
        return Err(done.partway(name, "its file is gone already"));
    }
    projects.known.remove(name);
    drop(projects);
    let sessions: Vec<String> = all.iter().map(|agent| agent.id.clone()).collect();
    crate::drafts::forget_project(daemon, name, &sessions);
    emit(daemon, "project.removed", json!({"name": name}))?;
    Ok(json!({
        "removed": name,
        "stopped": {"agents": done.agents, "runs": done.runs},
        "workspaces": {"removed": done.removed, "kept": done.kept},
    }))
}

/// `project.show`: the project, and the branch each repository's checkout in the project folder
/// is on (`null` when it is on none), read by git outside the projects' lock.
pub async fn show(daemon: Arc<Daemon>, params: Value) -> std::result::Result<Value, Failure> {
    let mut project = call(&daemon, "project.show", params)?;
    // A project that follows a default which cannot be followed gets the data folder: it says so.
    if project.get("workspaces_root_own").is_none()
        && let Err(why) = daemon.projects.lock().unwrap().registry().place_read()
    {
        project["workspaces_default_error"] = why.into();
    }
    let folder = PathBuf::from(project["folder"].as_str().unwrap_or_default());
    for repository in project["repositories"].as_array_mut().into_iter().flatten() {
        let checkout = match repository["path"].as_str().unwrap_or(".") {
            "." => folder.clone(),
            path => folder.join(path),
        };
        // A folder that is not its own repository yet: git would answer for the one around it.
        if !checkout.join(".git").exists() {
            repository["branch"] = Value::Null;
            continue;
        }
        let head = git::run(
            &checkout,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            Class::Read,
        )
        .await;
        repository["branch"] = match head {
            Ok(head) if head.success => head.stdout.trim().into(),
            _ => Value::Null,
        };
    }
    Ok(project)
}

/// The folder of a repository of the project; none when it is not its own repository: git would
/// answer for the one around it.
fn checkout_of(project: &Project, path: &str) -> Option<PathBuf> {
    let folder = match path {
        "." => project.folder.clone(),
        path => project.folder.join(path),
    };
    folder.join(".git").exists().then_some(folder)
}

/// The address a repository's remote leads to, credentials taken out and masked; none when it
/// has no such remote. It is shown, never checked: the check reads the remote itself.
async fn remote_url(project: &Project, path: &str, remote: &str) -> Option<String> {
    let folder = checkout_of(project, path)?;
    git::output(
        &folder,
        &["remote", "get-url", "--end-of-options", remote],
        Class::Read,
    )
    .await
    .ok()
    .map(|url| crate::forge::masked(&crate::forge::without_credentials(url.trim())))
}

/// Why `forge` is not where the repository's remote leads (`forge::disagreement`), masked; none
/// when it is.
async fn disagreement(
    project: &Project,
    path: &str,
    remote: &str,
    forge: &RepositoryForge,
) -> Option<String> {
    match checkout_of(project, path) {
        None => Some(format!("it has no remote {remote}")),
        Some(folder) => crate::forge::disagreement(forge, &folder, remote).await,
    }
}

/// What the project's settings show (`project.settings`): for each repository its remote's
/// address, the forge declared and the one its address proposes, and the tracker. The proposal is
/// only that: nothing is read nor written for a repository until its forge is declared.
async fn settings_of(project: &Project) -> Value {
    let mut repositories = Vec::new();
    for repository in &project.repositories {
        let url = remote_url(project, &repository.path, &repository.remote).await;
        let proposal = url.as_deref().and_then(RepositoryForge::proposed);
        let disagrees = match &repository.forge {
            Some(forge) => disagreement(project, &repository.path, &repository.remote, forge).await,
            None => None,
        };
        repositories.push(json!({
            "path": repository.path,
            "remote": repository.remote,
            "base": repository.base,
            "depends_on": repository.depends_on,
            "delete_branch_on_merge": repository.delete_branch_on_merge,
            "url": url,
            "forge": repository.forge,
            "proposal": proposal,
            "disagrees": disagrees,
        }));
    }
    // Why the user's SSH agent is lent to no push, which the settings say with what to do: the
    // machine's, the same for every project.
    let pushes_unlent = tokio::task::spawn_blocking(crate::pushed::unlent)
        .await
        .unwrap_or_else(|_| Some("the judgement of what ssh reads failed".into()));
    json!({
        "name": project.name,
        "folder": project.folder,
        "pushes_unlifted": crate::pushed::unlifted(),
        "pushes_unlent": pushes_unlent,
        "repositories": repositories,
        "tracker": project.tracker,
        "message_checks": project.message_checks,
        "branch_prefix": project.branch_prefix,
        "branch_format": project.branch_format,
        "pull_request_template": project.pull_requests.template,
        "windows": project.windows,
        "obsolete": project.obsolete,
    })
}

/// `project.settings`.
pub async fn settings(daemon: Arc<Daemon>, params: Value) -> std::result::Result<Value, Failure> {
    let ProjectName { name } = serde_json::from_value(params)?;
    let project = load_project(&daemon, &name)?;
    Ok(settings_of(&project).await)
}

/// `project.programs`: the command lines the project's forges and tracker need, checked.
pub async fn programs(daemon: Arc<Daemon>, params: Value) -> std::result::Result<Value, Failure> {
    let ProjectName { name } = serde_json::from_value(params)?;
    let project = load_project(&daemon, &name)?;
    Ok(crate::programs::of_project(&project).await)
}

/// Why `branch` cannot be the base of the repository at `path`, none when it can: it is a branch
/// of its remote, read with `git ls-remote`; of the tracking refs last fetched when the remote
/// cannot be read; of the repository itself when it has no such remote.
async fn base_refusal(
    project: &Project,
    path: &str,
    remote: &str,
    branches: &[String],
) -> Option<String> {
    let Some(folder) = checkout_of(project, path) else {
        return Some(format!("{path}: it is not in the folder of the project"));
    };
    let has_remote = git::run(
        &folder,
        &["remote", "get-url", "--end-of-options", remote],
        Class::Read,
    )
    .await
    .is_ok_and(|output| output.success);
    if !has_remote {
        let local = git::output(
            &folder,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
            Class::Read,
        )
        .await
        .unwrap_or_default();
        return branches
            .iter()
            .find(|branch| !local.lines().any(|line| line == branch.as_str()))
            .map(|branch| {
                format!("{path}: {branch} is not a branch of {path}, which has no remote {remote}")
            });
    }
    match git::output_remote(
        &folder,
        remote,
        "ls-remote",
        &["--heads", "--end-of-options", remote],
        Class::Read,
    )
    .await
    {
        Ok(heads) => branches
            .iter()
            .find(|branch| {
                let wanted = format!("refs/heads/{branch}");
                !heads
                    .lines()
                    .any(|line| line.split('\t').nth(1) == Some(wanted.as_str()))
            })
            .map(|branch| format!("{path}: {branch} is not a branch of the remote {remote}")),
        Err(reason) => {
            for branch in branches {
                let tracking = format!("refs/remotes/{remote}/{branch}^{{commit}}");
                let fetched = git::run(
                    &folder,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        "--end-of-options",
                        &tracking,
                    ],
                    Class::Read,
                )
                .await
                .is_ok_and(|output| output.success);
                if !fetched {
                    return Some(format!(
                        "{path}: the remote {remote} could not be read ({}) and {branch} was \
                         never fetched from it",
                        reason.split_whitespace().collect::<Vec<_>>().join(" ")
                    ));
                }
            }
            None
        }
    }
}

/// `project.set_settings`: writes the declared forges and tracker, and the base branches and
/// dependencies given, in the project's file, what is around them kept, once each forge is
/// checked to be the one its remote leads to and each base to be on its remote.
pub async fn set_settings(
    daemon: Arc<Daemon>,
    params: Value,
) -> std::result::Result<Value, Failure> {
    let asked: ProjectSettings = serde_json::from_value(params)?;
    let project = load_project(&daemon, &asked.name)?;
    // Each forge as checked (its host in lowercase), which is what the file gets.
    let mut checked_forges = BTreeMap::new();
    for setting in &asked.repositories {
        let Some(repository) = project.repositories.iter().find(|r| r.path == setting.path) else {
            return Err(Failure::refused(format!(
                "project {} has no repository {}",
                project.name, setting.path
            )));
        };
        let Some(forge) = &setting.forge else {
            continue;
        };
        let mut checked = forge.clone();
        checked
            .check()
            .map_err(|error| Failure::refused(format!("the forge of {}{error}", setting.path)))?;
        if let Some(why) = disagreement(&project, &setting.path, &repository.remote, &checked).await
        {
            return Err(Failure::refused(format!(
                "{} cannot be declared on {}: {why}; nothing was saved",
                setting.path,
                checked.said(),
            )));
        }
        checked_forges.insert(setting.path.clone(), checked);
    }
    for setting in &asked.repositories {
        let (Some(base), Some(repository)) = (
            &setting.base,
            project.repositories.iter().find(|r| r.path == setting.path),
        ) else {
            continue;
        };
        // Its shape (one at least, no space) is the file's to judge, with its key.
        let shaped = !base.is_empty()
            && base
                .iter()
                .all(|branch| !branch.is_empty() && !branch.contains(char::is_whitespace));
        if shaped
            && let Some(why) = base_refusal(&project, &setting.path, &repository.remote, base).await
        {
            return Err(Failure::refused(format!("{why}; nothing was saved")));
        }
    }
    let saved = rewrite(&daemon, &asked.name, |_, document| {
        let not_an_array = || {
            Failure::new(
                "invalid-config",
                format!(
                    "projects/{}.toml: repositories: must be an array of tables",
                    asked.name
                ),
            )
        };
        let entries = document
            .get_mut("repositories")
            .and_then(toml_edit::Item::as_array_of_tables_mut)
            .ok_or_else(not_an_array)?;
        for setting in &asked.repositories {
            for entry in entries.iter_mut() {
                if entry.get("path").and_then(|p| p.as_str()) != Some(setting.path.as_str()) {
                    continue;
                }
                match checked_forges.get(&setting.path) {
                    None => {
                        entry.remove("forge");
                    }
                    Some(forge) => {
                        entry.insert("forge", toml_edit::value(inline(forge)?));
                    }
                }
                if let Some(base) = &setting.base {
                    entry.insert(
                        "base",
                        toml_edit::value(base.iter().collect::<toml_edit::Array>()),
                    );
                }
                match setting.delete_branch_on_merge {
                    Some(true) => {
                        entry.insert("delete_branch_on_merge", toml_edit::value(true));
                    }
                    Some(false) => {
                        entry.remove("delete_branch_on_merge");
                    }
                    None => {}
                }
                match &setting.depends_on {
                    Some(paths) if !paths.is_empty() => {
                        entry.insert(
                            "depends_on",
                            toml_edit::value(paths.iter().collect::<toml_edit::Array>()),
                        );
                    }
                    Some(_) => {
                        entry.remove("depends_on");
                    }
                    None => {}
                }
            }
        }
        if let Some(prefix) = &asked.branch_prefix {
            document.insert("branch_prefix", toml_edit::value(prefix.as_str()));
        }
        match asked.windows {
            Some(true) => {
                document.insert("windows", toml_edit::value(true));
            }
            Some(false) => {
                document.remove("windows");
            }
            None => {}
        }
        match &asked.tracker {
            None => {
                document.remove("tracker");
            }
            Some(tracker) => {
                let mut table = toml_edit::Table::new();
                for (key, value) in inline(tracker)?.iter() {
                    table.insert(key, toml_edit::value(value.clone()));
                }
                document.insert("tracker", toml_edit::Item::Table(table));
            }
        }
        Ok(())
    })
    .map_err(|failure| match failure.code {
        // A base rule or a dependency the file would not accept (a cycle, itself, a stranger)
        // is the user's to fix, with the key at fault.
        "invalid-config"
            if asked
                .repositories
                .iter()
                .any(|s| s.base.is_some() || s.depends_on.is_some()) =>
        {
            Failure::refused(format!("{}; nothing was saved", failure.message))
        }
        _ => failure,
    })?;
    Ok(settings_of(&saved).await)
}

/// `project.set_message_checks`: the project's message checks, the user's alone (an agent's
/// `project` tool never writes them), each line refused with its reason when it cannot be run.
pub fn set_message_checks(daemon: &Daemon, params: Value) -> std::result::Result<Value, Failure> {
    let asked: MessageChecks = serde_json::from_value(params)?;
    let checks: Vec<String> = asked
        .checks
        .iter()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect();
    for line in &checks {
        flottille_core::project::check_message_check(line).map_err(|why| {
            Failure::refused(format!(
                "the check `{line}` cannot be used: {why}; nothing was saved"
            ))
        })?;
    }
    let saved = rewrite(daemon, &asked.name, |_, document| {
        match checks.is_empty() {
            true => {
                document.remove("message_checks");
            }
            false => {
                document["message_checks"] =
                    toml_edit::value(checks.iter().collect::<toml_edit::Array>());
            }
        }
        Ok(())
    })?;
    Ok(json!({"name": saved.name, "message_checks": saved.message_checks}))
}

/// `project.commit_checks`: each repository's commit checks, as the file says them, and what its
/// hooks in the project's folder propose besides, read and never run (`commit_checks::proposals`).
pub fn commit_checks(daemon: &Daemon, params: Value) -> std::result::Result<Value, Failure> {
    let asked: ProjectName = serde_json::from_value(params)?;
    let project = load_project(daemon, &asked.name)?;
    Ok(commit_checks_of(&project))
}

fn commit_checks_of(project: &Project) -> Value {
    let repositories: Vec<Value> = project
        .repositories
        .iter()
        .map(|repository| {
            let saved = &repository.commit_checks;
            let (mut proposed, left_out) =
                crate::commit_checks::proposals(&project.folder.join(&repository.path));
            proposed.retain(|proposal| {
                let lines = match proposal.hook {
                    "pre-commit" => &saved.pre_commit,
                    _ => &saved.commit_msg,
                };
                !lines.contains(&proposal.line)
            });
            json!({
                "path": repository.path,
                "pre_commit": saved.pre_commit,
                "commit_msg": saved.commit_msg,
                "proposed": proposed,
                "left_out": left_out,
            })
        })
        .collect();
    json!({"name": project.name, "repositories": repositories})
}

/// `project.set_commit_checks`: one repository's commit checks, the user's alone (an agent's
/// `project` tool never writes them), each line refused with its reason when it cannot be run;
/// then what `project.commit_checks` says.
pub fn set_commit_checks(daemon: &Daemon, params: Value) -> std::result::Result<Value, Failure> {
    let asked: CommitChecks = serde_json::from_value(params)?;
    let kept = |lines: &[String]| -> Vec<String> {
        lines
            .iter()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect()
    };
    let (pre_commit, commit_msg) = (kept(&asked.pre_commit), kept(&asked.commit_msg));
    for (lines, hook) in [(&pre_commit, "pre-commit"), (&commit_msg, "commit-msg")] {
        for line in lines {
            let message = hook == "commit-msg";
            flottille_core::project::check_commit_check(line, message).map_err(|why| {
                Failure::refused(format!(
                    "the {hook} check `{line}` cannot be used: {why}; nothing was saved"
                ))
            })?;
        }
    }
    let saved = rewrite(daemon, &asked.name, |project, document| {
        if !project
            .repositories
            .iter()
            .any(|repository| repository.path == asked.repository)
        {
            return Err(Failure::refused(format!(
                "the project {} has no repository {}; nothing was saved",
                asked.name, asked.repository
            )));
        }
        let entries = document
            .get_mut("repositories")
            .and_then(toml_edit::Item::as_array_of_tables_mut)
            .ok_or_else(|| {
                Failure::new(
                    "invalid-config",
                    format!(
                        "projects/{}.toml: repositories: must be an array of tables",
                        asked.name
                    ),
                )
            })?;
        for entry in entries.iter_mut() {
            if entry.get("path").and_then(|p| p.as_str()) != Some(asked.repository.as_str()) {
                continue;
            }
            let mut checks = toml_edit::InlineTable::new();
            for (key, lines) in [("pre_commit", &pre_commit), ("commit_msg", &commit_msg)] {
                if !lines.is_empty() {
                    checks.insert(key, lines.iter().collect::<toml_edit::Array>().into());
                }
            }
            match checks.is_empty() {
                true => {
                    entry.remove("commit_checks");
                }
                false => {
                    entry.insert("commit_checks", toml_edit::value(checks));
                }
            }
        }
        Ok(())
    })?;
    Ok(commit_checks_of(&saved))
}

/// `project.set_conventions`: the project's branch format and pull request template, the user's
/// alone (an agent's `project` tool never writes them); a pattern that is no regular expression
/// is refused with its reason, nothing written.
pub fn set_conventions(daemon: &Daemon, params: Value) -> std::result::Result<Value, Failure> {
    let asked: Conventions = serde_json::from_value(params)?;
    let format = asked.branch_format.as_deref().map(str::trim);
    if let Some(format) = format.filter(|format| !format.is_empty()) {
        flottille_core::project::check_branch_format(format).map_err(|why| {
            Failure::refused(format!("the branch format {why}; nothing was saved"))
        })?;
    }
    let saved = rewrite(daemon, &asked.name, |_, document| {
        match format {
            Some("") => {
                document.remove("branch_format");
            }
            Some(format) => {
                document.insert("branch_format", toml_edit::value(format));
            }
            None => {}
        }
        if let Some(template) = &asked.pull_request_template {
            let table = document
                .entry("pull_requests")
                .or_insert_with(toml_edit::table)
                .as_table_like_mut()
                .ok_or_else(|| {
                    Failure::new(
                        "invalid-config",
                        format!(
                            "projects/{}.toml: pull_requests: must be a table",
                            asked.name
                        ),
                    )
                })?;
            match template.trim().is_empty() {
                true => {
                    table.remove("template");
                }
                false => {
                    table.insert("template", toml_edit::value(template.as_str()));
                }
            }
            if table.is_empty() {
                document.remove("pull_requests");
            }
        }
        Ok(())
    })?;
    Ok(json!({
        "name": saved.name,
        "branch_format": saved.branch_format,
        "pull_request_template": saved.pull_requests.template,
    }))
}

/// `project.repositories`: each repository of the project as its checkout stands (found or not,
/// the branch it is on, where its remote leads, none when it is local only, its forge, its base
/// rule and its local branches), and `new`, the repositories the folder holds that the file does
/// not name, detected again.
pub async fn repositories(
    daemon: Arc<Daemon>,
    params: Value,
) -> std::result::Result<Value, Failure> {
    let ProjectName { name } = serde_json::from_value(params)?;
    let project = load_project(&daemon, &name)?;
    Ok(repositories_of(&project).await)
}

async fn repositories_of(project: &Project) -> Value {
    let mut listed = Vec::new();
    for repository in &project.repositories {
        let checkout = checkout_of(project, &repository.path);
        let (branch, branches) = match &checkout {
            None => (None, Vec::new()),
            Some(folder) => {
                let branch = git::output(
                    folder,
                    &["symbolic-ref", "--quiet", "--short", "HEAD"],
                    Class::Read,
                )
                .await
                .ok()
                .map(|branch| branch.trim().to_owned());
                let branches: Vec<String> = git::output(
                    folder,
                    &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
                    Class::Read,
                )
                .await
                .map(|listed| listed.lines().map(str::to_owned).collect())
                .unwrap_or_default();
                (branch, branches)
            }
        };
        listed.push(json!({
            "path": repository.path,
            "found": checkout.is_some(),
            "branch": branch,
            "remote": repository.remote,
            "url": remote_url(project, &repository.path, &repository.remote).await,
            "forge": repository.forge,
            "base": repository.base,
            "branches": branches,
        }));
    }
    // A folder that cannot be read offers nothing new; its repositories say they are not found.
    let new: Vec<String> = detect::repositories(&project.folder)
        .unwrap_or_default()
        .into_iter()
        .filter(|path| !project.repositories.iter().any(|r| &r.path == path))
        .collect();
    json!({
        "name": project.name,
        "folder": project.folder,
        "repositories": listed,
        "new": new,
    })
}

/// A path typed for a repository, as the file writes it: relative, `/`-separated, `.` for the
/// folder itself; none when it leads out of the folder.
fn repository_path(typed: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in Path::new(typed.trim()).components() {
        match part {
            std::path::Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(match parts.is_empty() {
        true => ".".to_owned(),
        false => parts.join("/"),
    })
}

/// `project.add_repository`: a repository of the project's folder, written in the file where it
/// stands, its comments kept, with the defaults (`origin`, base `main`).
pub async fn add_repository(
    daemon: Arc<Daemon>,
    params: Value,
) -> std::result::Result<Value, Failure> {
    let asked: ProjectRepository = serde_json::from_value(params)?;
    let project = load_project(&daemon, &asked.name)?;
    let Some(path) = repository_path(&asked.path) else {
        return Err(Failure::refused(format!(
            "{} is not a folder of {}",
            asked.path, asked.name
        )));
    };
    if project.repositories.iter().any(|r| r.path == path) {
        return Err(Failure::refused(format!(
            "{path} is already a repository of {}",
            asked.name
        )));
    }
    if checkout_of(&project, &path).is_none() {
        return Err(Failure::refused(format!(
            "{path} is not a Git repository: {} holds no .git",
            project.folder.join(&path).display()
        )));
    }
    // A link leading elsewhere would make a repository of the project out of someone else's.
    let stays = std::fs::canonicalize(project.folder.join(&path))
        .and_then(|folder| Ok(folder.starts_with(std::fs::canonicalize(&project.folder)?)))
        .unwrap_or(false);
    if !stays {
        return Err(Failure::refused(format!(
            "{path} leads outside the project folder"
        )));
    }
    let saved = rewrite(&daemon, &asked.name, |_, document| {
        let entries = document
            .entry("repositories")
            .or_insert(toml_edit::Item::ArrayOfTables(Default::default()))
            .as_array_of_tables_mut()
            .ok_or_else(|| {
                Failure::new(
                    "invalid-config",
                    format!(
                        "projects/{}.toml: repositories: must be an array of tables",
                        asked.name
                    ),
                )
            })?;
        let mut table = toml_edit::Table::new();
        table.insert("path", toml_edit::value(path.clone()));
        table.decor_mut().set_prefix("\n");
        entries.push(table);
        Ok(())
    })?;
    Ok(repositories_of(&saved).await)
}

/// `project.remove_repository`: the project forgets one of its repositories, written in the file
/// where it stands, its comments kept. Its folder, its branches and the worktrees of the
/// workspaces that took it are never touched. Refused when the file would no longer be one the
/// daemon can use (a repository depending on it, a task running in it…), with the reason.
pub async fn remove_repository(
    daemon: Arc<Daemon>,
    params: Value,
) -> std::result::Result<Value, Failure> {
    let asked: ProjectRepository = serde_json::from_value(params)?;
    let project = load_project(&daemon, &asked.name)?;
    let path = repository_path(&asked.path).unwrap_or_else(|| asked.path.clone());
    if !project.repositories.iter().any(|r| r.path == path) {
        return Err(Failure::refused(format!(
            "{} has no repository {path}",
            asked.name
        )));
    }
    if project.repositories.len() == 1 {
        return Err(Failure::refused(format!(
            "{path} is the last repository of {}, and a project keeps at least one repository",
            asked.name
        )));
    }
    let saved = rewrite(&daemon, &asked.name, |_, document| {
        let entries = document
            .get_mut("repositories")
            .and_then(toml_edit::Item::as_array_of_tables_mut)
            .ok_or_else(|| {
                Failure::new(
                    "invalid-config",
                    format!(
                        "projects/{}.toml: repositories: must be an array of tables",
                        asked.name
                    ),
                )
            })?;
        let at = entries
            .iter()
            .position(|entry| entry.get("path").and_then(|p| p.as_str()) == Some(path.as_str()));
        if let Some(at) = at {
            entries.remove(at);
        }
        Ok(())
    })
    .map_err(|failure| match failure.code {
        "invalid-config" => Failure::refused(format!(
            "{path} is not removed, nothing was written: {}",
            failure.message
        )),
        _ => failure,
    })?;
    Ok(repositories_of(&saved).await)
}

/// A declaration as the TOML inline table it is written as: its keys hold strings and lists of
/// strings.
fn inline(value: &impl serde::Serialize) -> std::result::Result<toml_edit::InlineTable, Failure> {
    let mut table = toml_edit::InlineTable::new();
    let Value::Object(fields) = serde_json::to_value(value)? else {
        return Err(Failure::refused("a declaration is a table"));
    };
    for (key, field) in fields {
        let value: toml_edit::Value = match field {
            Value::String(text) => text.into(),
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .collect::<toml_edit::Array>()
                .into(),
            _ => continue,
        };
        table.insert(&key, value);
    }
    Ok(table)
}

fn emit(daemon: &Daemon, name: &str, data: Value) -> std::result::Result<(), Failure> {
    daemon
        .emit(name, data)
        .map(|_| ())
        .map_err(|error| Failure::new("unavailable", error.to_string()))
}

fn not_found(name: &str) -> Failure {
    Failure::new("not-found", format!("no project named {name}"))
}

pub fn load(projects: &Projects, name: &str) -> std::result::Result<Project, Failure> {
    projects.registry.load(name)?.ok_or_else(|| not_found(name))
}

/// Edits the project's file where it stands, keeping what the user wrote around the edit, and
/// writes it only when the result is a configuration the daemon can use. The edit is given the
/// project as the file says now.
pub fn rewrite(
    daemon: &Daemon,
    name: &str,
    edit: impl FnOnce(&Project, &mut toml_edit::DocumentMut) -> std::result::Result<(), Failure>,
) -> std::result::Result<Project, Failure> {
    rewrite_checked(daemon, name, edit, |_| Ok(()))
}

/// As [`rewrite`], with `check` given the edited project before anything is written: a refusal
/// from it leaves the file as it was.
pub fn rewrite_checked(
    daemon: &Daemon,
    name: &str,
    edit: impl FnOnce(&Project, &mut toml_edit::DocumentMut) -> std::result::Result<(), Failure>,
    check: impl FnOnce(&Project) -> std::result::Result<(), Failure>,
) -> std::result::Result<Project, Failure> {
    let mut projects = daemon.projects.lock().unwrap();
    let projects = &mut *projects;
    let project = load(projects, name)?;
    refuse_removing(daemon, name)?;
    let file = projects.registry.file(name);
    let unavailable = |error: std::io::Error| {
        Failure::new("unavailable", format!("projects/{name}.toml: {error}"))
    };
    let text = std::fs::read_to_string(&file).map_err(unavailable)?;
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            Failure::new(
                "invalid-config",
                format!("projects/{name}.toml: {}", error.message()),
            )
        })?;
    edit(&project, &mut document)?;
    let text = document.to_string();
    let edited = projects
        .registry
        .parse(name, &text)
        .map_err(|error| Failure::new("invalid-config", error.to_string()))?;
    check(&edited)?;
    crate::providers::write_in_place(&file, &text).map_err(unavailable)?;
    announce_edits(daemon, projects)?;
    Ok(edited)
}

/// At start, every project's file is brought to the keys of this version (`former_keys_renamed`),
/// where it stands: what the user wrote around them, comments included, stays as it is, and the
/// file is replaced at once, never left half written. A file that the daemon could use is
/// written only when it still can afterwards.
pub fn migrate_files(daemon: &Daemon) {
    let names: Vec<String> = {
        let projects = daemon.projects.lock().unwrap();
        let Ok(loaded) = projects.registry.load_all() else {
            return;
        };
        loaded
            .projects
            .iter()
            .map(|project| project.name.clone())
            .chain(loaded.invalid.iter().map(|error| error.name.clone()))
            .collect()
    };
    for name in names {
        let mut projects = daemon.projects.lock().unwrap();
        let file = projects.registry.file(&name);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for task in tasks_twice(&text) {
            eprintln!(
                "flottille daemon: projects/{name}.toml: commands.{task} and tasks.{task} are \
                 both kept: remove the one you do not want"
            );
        }
        let Some(migrated) = migrated_for(&text, Some((&name, &projects.data))) else {
            continue;
        };
        // The file is written only when it reads afterwards: one that worked still does, one that
        // did not now does, or else it stays as the user left it.
        let usable = |text: &str| projects.registry.parse(&name, text).is_ok();
        if !usable(&migrated) {
            if usable(&text) {
                eprintln!(
                    "flottille daemon: projects/{name}.toml: its former keys are kept: the file \
                     would not read the same with their new names"
                );
            }
            continue;
        }
        if let Err(error) = crate::providers::write_in_place(&file, &migrated) {
            eprintln!("flottille daemon: projects/{name}.toml: its former keys are kept: {error}");
            continue;
        }
        let _ = announce_edits(daemon, &mut projects);
    }
}

/// The text of a project's file with its keys brought to this version, `None` when it holds
/// none of the former ones (or is no TOML at all).
#[cfg(test)]
fn migrated(text: &str) -> Option<String> {
    migrated_for(text, None)
}

/// As [`migrated`], and for the project `(name, data folder)` when given, its `workspaces_root`
/// dropped if it only pins the data folder: until the default could be chosen, `project add`
/// wrote it, and pinned it would never follow the user's default. It resolves the same under the
/// data default, and the workspaces that exist keep their folder from their record.
fn migrated_for(text: &str, project: Option<(&str, &Path)>) -> Option<String> {
    let mut document: toml_edit::DocumentMut = text.parse().ok()?;
    let mut changed = former_keys_renamed(&mut document);
    if let Some((name, data)) = project {
        changed |= pinned_default_dropped(&mut document, name, data);
    }
    changed.then(|| document.to_string())
}

/// Removes `workspaces_root` when it is `<data>/workspaces/<name>`, the folder a project
/// without one gets under the data default. Whether it did.
fn pinned_default_dropped(document: &mut toml_edit::DocumentMut, name: &str, data: &Path) -> bool {
    let pinned = document
        .get("workspaces_root")
        .and_then(toml_edit::Item::as_str)
        .is_some_and(|root| Path::new(root) == data.join("workspaces").join(name));
    if pinned {
        document.remove("workspaces_root");
    }
    pinned
}

/// The tasks a file holds twice, under their former key `[commands.<name>]` and under
/// `[tasks.<name>]`: neither is moved, the user settles which one stays.
pub fn tasks_twice(text: &str) -> Vec<String> {
    let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    former_tasks(document.as_table())
        .into_iter()
        .filter(|name| {
            document
                .get("tasks")
                .and_then(toml_edit::Item::as_table_like)
                .is_some_and(|tasks| tasks.contains_key(name))
        })
        .collect()
}

/// The entries of `[commands]` that are tasks, by their `line`.
fn former_tasks(root: &toml_edit::Table) -> Vec<String> {
    root.get("commands")
        .and_then(toml_edit::Item::as_table_like)
        .map(|commands| {
            commands
                .iter()
                .filter(|(_, entry)| {
                    entry
                        .as_table_like()
                        .is_some_and(|e| e.contains_key("line"))
                })
                .map(|(name, _)| name.to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Renames the keys of earlier versions in the document, keeping everything else, the comments
/// above and beside each entry included, and the order of the file:
///
/// - a task, `[commands.<name>]` with a `line`, goes under `[tasks.<name>]`, unless the file has
///   that task there already (`tasks_twice`);
/// - `[procedures.<name>]` becomes `[commands.<name>]`, once the tasks have left `commands`;
/// - `[references.<name>]` becomes `[skills.<name>]`;
/// - `[[recipe]]` becomes `[[setup]]`, and a step's `command` its `task`;
/// - the rights the gate no longer hands out go (`docs/design.md` §7): `[agent_actions]`, a
///   task's `added_by`, a command's `grants` and `children`.
///
/// Whether anything changed.
pub fn former_keys_renamed(document: &mut toml_edit::DocumentMut) -> bool {
    let mut changed = false;
    let root = document.as_table_mut();
    for name in former_tasks(root) {
        let twice = root
            .get("tasks")
            .and_then(toml_edit::Item::as_table_like)
            .is_some_and(|tasks| tasks.contains_key(&name));
        if !twice {
            changed |= moved(root, "commands", &name, "tasks");
        }
    }
    changed |= merged(root, "procedures", "commands");
    changed |= merged(root, "references", "skills");
    if !root.contains_key("setup")
        && let Some((key, recipe)) = root.remove_entry("recipe")
    {
        root.insert_formatted(&renamed(&key, "setup"), recipe);
        changed = true;
    }
    if let Some(steps) = root
        .get_mut("setup")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
    {
        for step in steps.iter_mut() {
            changed |= key_renamed(step, "command", "task");
        }
    }
    changed |= root.remove("agent_actions").is_some();
    for (table, keys) in [
        ("tasks", &["added_by"][..]),
        ("commands", &["grants", "children"][..]),
    ] {
        let Some(entries) = root
            .get_mut(table)
            .and_then(toml_edit::Item::as_table_like_mut)
        else {
            continue;
        };
        for (_, entry) in entries.iter_mut() {
            if let Some(entry) = entry.as_table_like_mut() {
                for key in keys {
                    changed |= entry.remove(key).is_some();
                }
            }
        }
    }
    changed
}

/// The key, named `name`, with the comments and spaces around it kept.
fn renamed(key: &toml_edit::Key, name: &str) -> toml_edit::Key {
    toml_edit::Key::new(name)
        .with_leaf_decor(key.leaf_decor().clone())
        .with_dotted_decor(key.dotted_decor().clone())
}

/// Moves the entry `name` of the table `from` into the table `to`, made when it is missing; `from`
/// goes once it is empty, unless its header holds a comment. Nothing moves when `to` is no table.
fn moved(root: &mut toml_edit::Table, from: &str, name: &str, to: &str) -> bool {
    if root
        .get(to)
        .is_some_and(|target| !target.is_table() && !target.is_inline_table())
    {
        return false;
    }
    let Some(table) = root
        .get_mut(from)
        .and_then(toml_edit::Item::as_table_like_mut)
    else {
        return false;
    };
    let Some(key) = table.key(name).cloned() else {
        return false;
    };
    let Some(entry) = table.remove(name) else {
        return false;
    };
    let commented = |decor: &toml_edit::Decor| {
        [decor.prefix(), decor.suffix()]
            .into_iter()
            .flatten()
            .any(|text| text.as_str().is_some_and(|text| text.contains('#')))
    };
    if root.get(from).is_some_and(|table| match table {
        toml_edit::Item::Table(table) => table.is_empty() && !commented(table.decor()),
        toml_edit::Item::Value(toml_edit::Value::InlineTable(table)) => {
            table.is_empty() && !commented(table.decor())
        }
        _ => false,
    }) {
        root.remove(from);
    }
    let target = root.entry(to).or_insert_with(|| {
        let mut table = toml_edit::Table::new();
        // Only the entries show: `[tasks.name]`, never an empty `[tasks]`.
        table.set_implicit(true);
        toml_edit::Item::Table(table)
    });
    match target {
        toml_edit::Item::Table(table) => {
            table.insert_formatted(&key, entry);
        }
        toml_edit::Item::Value(toml_edit::Value::InlineTable(table)) => {
            if let Ok(value) = entry.into_value() {
                table.insert_formatted(&key, value);
            }
        }
        _ => return false,
    }
    true
}

/// The table `from` renamed `to`, or its entries moved into `to` when the file has both.
fn merged(root: &mut toml_edit::Table, from: &str, to: &str) -> bool {
    if !root.contains_key(to) {
        let Some((key, table)) = root.remove_entry(from) else {
            return false;
        };
        root.insert_formatted(&renamed(&key, to), table);
        return true;
    }
    let names: Vec<String> = root
        .get(from)
        .and_then(toml_edit::Item::as_table_like)
        .map(|table| table.iter().map(|(name, _)| name.to_owned()).collect())
        .unwrap_or_default();
    let mut changed = false;
    for name in names {
        // A name in both is the user's to settle: the file says so when it is read.
        let twice = root
            .get(to)
            .and_then(toml_edit::Item::as_table_like)
            .is_some_and(|table| table.contains_key(&name));
        if !twice {
            changed |= moved(root, from, &name, to);
        }
    }
    changed
}

/// The key `from` of a table renamed `to`, in its place among the others.
fn key_renamed(table: &mut toml_edit::Table, from: &str, to: &str) -> bool {
    if !table.contains_key(from) || table.contains_key(to) {
        return false;
    }
    let entries: Vec<(toml_edit::Key, toml_edit::Item)> = table
        .iter()
        .map(|(name, _)| name.to_owned())
        .collect::<Vec<_>>()
        .into_iter()
        .filter_map(|name| table.remove_entry(&name))
        .collect();
    for (key, item) in entries {
        let key = if key.get() == from {
            renamed(&key, to)
        } else {
            key
        };
        table.insert_formatted(&key, item);
    }
    true
}

/// The file of the project `name`, as it is written.
pub fn text(daemon: &Daemon, name: &str) -> std::result::Result<String, Failure> {
    let file = daemon.projects.lock().unwrap().registry.file(name);
    std::fs::read_to_string(&file)
        .map_err(|error| Failure::new("unavailable", format!("projects/{name}.toml: {error}")))
}

/// The project `name` as its file `text` says it.
pub fn parsed(daemon: &Daemon, name: &str, text: &str) -> std::result::Result<Project, Failure> {
    let projects = daemon.projects.lock().unwrap();
    projects
        .registry
        .parse(name, text)
        .map_err(|error| Failure::new("invalid-config", error.to_string()))
}

/// The settings of the project `name` that travel with a launch to another machine, as its file
/// writes them (`docs/remote.md`, "What travels").
pub fn travelling(daemon: &Daemon, name: &str) -> std::result::Result<Value, Failure> {
    let text = text(daemon, name)?;
    flottille_core::project::travelling(&text)
        .map_err(|why| Failure::new("invalid-config", format!("projects/{name}.toml: {why}")))
}

/// Makes the project `name` for a launch of the machine `from` (its key, which its member's name
/// is not: a name another member can take), in `folder`, with the repositories at `paths` (none
/// of them there yet: they come from that machine), marked as made by Flottille, with none of
/// that machine's settings yet: they are laid once the launch is allowed (`take_travelled`).
pub fn make_travelled(
    daemon: &Daemon,
    name: &str,
    folder: &Path,
    paths: &[String],
    from: &str,
) -> std::result::Result<Project, Failure> {
    let mut projects = daemon.projects.lock().unwrap();
    let projects = &mut *projects;
    let mut project = Project::new(name, folder.to_owned(), paths.to_vec(), &projects.data);
    project.from_machine = Some(from.to_owned());
    projects.registry.create(&project)?;
    let project = load(projects, name)?;
    announce_edits(daemon, projects)?;
    eprintln!(
        "flottille daemon: project {name} made in {} for a launch of {from}",
        folder.display()
    );
    Ok(project)
}

/// The project `name` as its file `text` would be with `settings`, which came with a launch of
/// the machine `from`, laid on it (`lay_travelled`), nothing written: what the gate classes
/// before the launch is allowed. Refused when the result is not a configuration this machine can
/// use.
pub fn as_travelled(
    daemon: &Daemon,
    name: &str,
    text: &str,
    settings: &Value,
    from: &str,
) -> std::result::Result<Project, Failure> {
    let came = came(name, settings, from)?;
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            Failure::new(
                "invalid-config",
                format!("projects/{name}.toml: {}", error.message()),
            )
        })?;
    lay_travelled(name, &mut document, &came)?;
    let projects = daemon.projects.lock().unwrap();
    projects
        .registry
        .parse(name, &document.to_string())
        .map_err(|error| Failure::refused(format!("the settings of {name} from {from}: {error}")))
}

/// The TOML of `settings` that came from `from` for the project `name`, refused when it holds a
/// key that does not travel.
fn came(
    name: &str,
    settings: &Value,
    from: &str,
) -> std::result::Result<toml_edit::DocumentMut, Failure> {
    let refused =
        |why: String| Failure::refused(format!("the settings of {name} from {from}: {why}"));
    let text = flottille_core::project::travelled_toml(settings).map_err(refused)?;
    text.parse()
        .map_err(|error: toml_edit::TomlError| refused(error.message().to_owned()))
}

/// Lays `settings`, which came with a launch of the machine `from` (its key), in the file of the
/// project `name`, which Flottille made for a launch of that very machine: each key that travels
/// is replaced by the one that came (removed when none came), each repository's own keys too, a
/// repository that came and the file lacks is added; every other key stays the machine's own.
/// Laid only on the file `classed` was, which what the user or the judge allowed was said of, as
/// a held write is done only while its words still hold; or when laying changes nothing. Refused,
/// nothing written, for a project the user registered or one made for another machine, when an
/// added repository leads where it may not (`refused_repository`), and when the result is not a
/// configuration this machine can use.
pub fn take_travelled(
    daemon: &Daemon,
    name: &str,
    settings: &Value,
    from: &str,
    classed: Option<&str>,
) -> std::result::Result<Project, Failure> {
    let came = came(name, settings, from)?;
    // Read before the file is: `rewrite` holds the projects.
    let others = all(daemon);
    rewrite(daemon, name, |project, document| {
        match &project.from_machine {
            None => {
                return Err(Failure::refused(format!(
                    "{name} was registered by the user here: it keeps its own settings"
                )));
            }
            Some(owner) if owner != from => {
                return Err(Failure::refused(format!(
                    "{name} was made here for another machine's launches: it keeps that \
                     machine's settings"
                )));
            }
            Some(_) => {}
        }
        let was = document.to_string();
        lay_travelled(name, document, &came)?;
        if document.to_string() != was && classed != Some(was.as_str()) {
            return Err(Failure::refused(format!(
                "{name} changed here since the launch was allowed: what it allowed is no longer \
                 what its settings would do; launch it again"
            )));
        }
        let added = document
            .get("repositories")
            .and_then(toml_edit::Item::as_array_of_tables)
            .into_iter()
            .flat_map(toml_edit::ArrayOfTables::iter)
            .filter_map(|table| table.get("path").and_then(toml_edit::Item::as_str))
            .filter(|path| !project.repositories.iter().any(|r| r.path == *path));
        for path in added {
            if let Some(why) = refused_repository(&others, project, path) {
                return Err(Failure::refused(why));
            }
        }
        Ok(())
    })
}

/// Why a repository a launch adds at `path` to `project` is refused, as one the user adds is
/// (`add_repository`): a path out of its folder, a link leading out of it, or a repository of
/// another project of `others`. Nothing there yet is not refused, unless a folder on its way
/// leads out (`stays_inside`): its code comes from the launch's machine.
pub fn refused_repository(others: &[Project], project: &Project, path: &str) -> Option<String> {
    if repository_path(path).as_deref() != Some(path) {
        return Some(format!("{path} is not a folder of {}", project.name));
    }
    let folder = project.folder.join(path);
    if std::fs::symlink_metadata(&folder).is_err() {
        return (!stays_inside(&project.folder, path))
            .then(|| format!("{path} leads outside the project folder"));
    }
    let Ok(inside) = leads_inside(&project.folder, path) else {
        return Some(format!("{path} leads outside the project folder"));
    };
    let elsewhere = others
        .iter()
        .flat_map(|other| {
            other
                .repositories
                .iter()
                .filter(move |r| other.name != project.name || r.path != path)
                .map(move |r| (other, checkout_folder(other, &r.path)))
        })
        .find(|(_, folder)| std::fs::canonicalize(folder).ok().as_ref() == Some(&inside));
    elsewhere.map(|(other, _)| {
        format!(
            "{path} is a repository of the project {} already",
            other.name
        )
    })
}

/// Whether what is made at `path` of the project folder `folder` lands inside it: the deepest
/// part of the path that exists, links followed, is inside the project's folder. A path that is
/// not there yet can lead out through a link in a middle folder.
pub fn stays_inside(folder: &Path, path: &str) -> bool {
    let Ok(root) = std::fs::canonicalize(folder) else {
        // No project folder: nothing in it leads anywhere yet; a link that leads nowhere does not
        // lead inside.
        return std::fs::symlink_metadata(folder).is_err();
    };
    let mut deepest = folder.join(path);
    while std::fs::symlink_metadata(&deepest).is_err() {
        if !deepest.pop() {
            return false;
        }
    }
    std::fs::canonicalize(&deepest).is_ok_and(|at| at.starts_with(&root))
}

/// The folder of the repository at `path` of `project`, canonical, when it is inside the
/// project's folder: an error when it leads out of it, through a link or otherwise.
pub fn leads_inside(folder: &Path, path: &str) -> std::result::Result<PathBuf, ()> {
    let inside = std::fs::canonicalize(folder.join(path)).map_err(|_| ())?;
    let root = std::fs::canonicalize(folder).map_err(|_| ())?;
    match inside.starts_with(&root) {
        true => Ok(inside),
        false => Err(()),
    }
}

fn checkout_folder(project: &Project, path: &str) -> PathBuf {
    match path {
        "." => project.folder.clone(),
        path => project.folder.join(path),
    }
}

/// Each key that travels of `document` replaced by `came`'s, as `take_travelled` says.
fn lay_travelled(
    name: &str,
    document: &mut toml_edit::DocumentMut,
    came: &toml_edit::DocumentMut,
) -> std::result::Result<(), Failure> {
    use flottille_core::project::{TRAVELLING, TRAVELLING_REPOSITORY};
    for key in TRAVELLING {
        document.remove(key);
        if let Some(item) = came.get(key) {
            document.insert(key, item.clone());
        }
    }
    let invalid = || {
        Failure::new(
            "invalid-config",
            format!("projects/{name}.toml: repositories: must be a list of tables"),
        )
    };
    let mut here = document
        .remove("repositories")
        .and_then(|item| item.into_array_of_tables().ok())
        .ok_or_else(invalid)?;
    let path_of = |table: &toml_edit::Table| {
        table
            .get("path")
            .and_then(toml_edit::Item::as_str)
            .map(str::to_owned)
    };
    for repository in came
        .get("repositories")
        .and_then(toml_edit::Item::as_array_of_tables)
        .into_iter()
        .flat_map(toml_edit::ArrayOfTables::iter)
    {
        let Some(path) = path_of(repository) else {
            continue;
        };
        let at = here
            .iter()
            .position(|table| path_of(table).as_ref() == Some(&path));
        let table = match at {
            Some(at) => here.get_mut(at).expect("found"),
            None => {
                let mut added = toml_edit::Table::new();
                added.insert("path", toml_edit::value(path.as_str()));
                here.push(added);
                here.get_mut(here.len() - 1).expect("added")
            }
        };
        for key in TRAVELLING_REPOSITORY {
            table.remove(key);
            if let Some(item) = repository.get(key) {
                table.insert(key, item.clone());
            }
        }
    }
    document.insert("repositories", toml_edit::Item::ArrayOfTables(here));
    Ok(())
}

/// Removes the project `name` Flottille made in `folder` for a launch of `from` that did not
/// start: its file and its folder, which hold nothing but what that launch sent. Only while it is
/// still that machine's, and nothing else of this machine uses it: no agent, no workspace.
pub fn unmake_travelled(
    daemon: &Daemon,
    name: &str,
    folder: &Path,
    from: &str,
) -> std::result::Result<(), Failure> {
    let mut projects = daemon.projects.lock().unwrap();
    let projects = &mut *projects;
    let project = load(projects, name)?;
    if project.from_machine.as_deref() != Some(from) || project.folder != folder {
        return Err(Failure::refused(format!(
            "{name} is no longer the project made for {from}'s launch"
        )));
    }
    if !live_agents(daemon, name)?.is_empty() || !workspaces_of(daemon, name)?.is_empty() {
        return Err(Failure::refused(format!("{name} is in use")));
    }
    projects.registry.remove(name)?;
    let removed = std::fs::remove_dir_all(folder);
    announce_edits(daemon, projects)?;
    if let Err(error) = removed
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(Failure::new(
            "unavailable",
            format!("{}: {error}", folder.display()),
        ));
    }
    eprintln!(
        "flottille daemon: project {name}, made for a launch of {from} that did not start, removed"
    );
    Ok(())
}

/// Every project whose file can be used, as the files say now.
pub fn all(daemon: &Daemon) -> Vec<Project> {
    let projects = daemon.projects.lock().unwrap();
    match projects.registry.load_all() {
        Ok(loaded) => loaded.projects,
        Err(error) => {
            eprintln!("flottille daemon: the projects: {error}");
            Vec::new()
        }
    }
}

/// Every project's running sessions get the tools their roles have now: a switch outside the
/// projects' files changed them (`docs/tools.md`, "When the tools change").
pub fn tools_changed(daemon: &Daemon) {
    let projects = daemon.projects.lock().unwrap();
    match projects.registry.load_all() {
        Ok(loaded) => {
            for project in &loaded.projects {
                crate::mcp::project_changed(daemon, project);
            }
        }
        Err(error) => eprintln!("flottille daemon: the projects: {error}"),
    }
}

/// Whether the project's file is still there.
pub fn exists(daemon: &Daemon, name: &str) -> bool {
    let projects = daemon.projects.lock().unwrap();
    matches!(projects.registry.load(name), Ok(Some(_)))
}

/// The project as its file says now.
pub fn load_project(daemon: &Daemon, name: &str) -> std::result::Result<Project, Failure> {
    load(&daemon.projects.lock().unwrap(), name)
}

/// Reads the files and announces what changed in them since the last request: a project that
/// appeared, one that was edited, one that is gone. A file that cannot be used is neither: it is
/// reported by the requests that read it, and the project it was stays known until it is fixed.
fn announce_edits(
    daemon: &Daemon,
    projects: &mut Projects,
) -> std::result::Result<flottille_core::registry::Loaded, Failure> {
    let loaded = projects.registry.load_all()?;
    for project in &loaded.projects {
        let event = match projects.known.get(&project.name) {
            None => "project.added",
            Some(known) if known != project => "project.changed",
            Some(_) => continue,
        };
        emit(
            daemon,
            event,
            json!({"name": project.name, "folder": project.folder}),
        )?;
        if event == "project.changed" {
            // Before the answer: the next call of a session is served the tools it has now.
            crate::mcp::project_changed(daemon, project);
        }
        projects.known.insert(project.name.clone(), project.clone());
    }
    let gone: Vec<String> = projects
        .known
        .keys()
        .filter(|name| {
            !loaded.projects.iter().any(|project| &project.name == *name)
                && !loaded.invalid.iter().any(|error| &error.name == *name)
        })
        .cloned()
        .collect();
    for name in gone {
        projects.known.remove(&name);
        emit(daemon, "project.removed", json!({"name": name}))?;
    }
    Ok(loaded)
}

/// What registering a folder would register.
struct Plan {
    name: String,
    folder: PathBuf,
    repositories: Vec<String>,
}

/// A folder typed as `~` or `~/…` is the home of the user the daemon runs for. `~other` is not
/// expanded: it stays relative and is refused like any relative path.
pub(crate) fn home_expanded(typed: &str) -> Option<PathBuf> {
    let rest = typed.strip_prefix('~')?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(home.join(rest.trim_start_matches('/')))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FoldersAsked {
    path: Option<String>,
}

/// The folders inside `path` (the home when none), for a client that picks where a project is
/// added rather than typing it (`docs/remote.md`, "Paths are picked, not typed"): each with how
/// many repositories it holds as `project.detect` finds them, and the project already registered
/// there.
fn folders(projects: &Projects, path: Option<&str>) -> std::result::Result<Value, Failure> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let folder = match path {
        None => home
            .clone()
            .ok_or_else(|| Failure::refused("the daemon has no HOME to start from: give a path"))?,
        Some(typed) => home_expanded(typed).unwrap_or_else(|| PathBuf::from(typed)),
    };
    if !folder.is_absolute() {
        return Err(Failure::refused(format!(
            "the folder must be an absolute path, not {}",
            folder.display()
        )));
    }
    let names = detect::folders(&folder).map_err(|error| {
        Failure::refused(format!("{} cannot be read: {error}", folder.display()))
    })?;
    let project_of = |path: &Path| {
        projects
            .known
            .values()
            .find(|project| project.folder == path)
            .map(|project| project.name.clone())
    };
    let shown = |path: &Path| match home.as_deref().map(|home| path.strip_prefix(home)) {
        Some(Ok(rest)) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(Ok(rest)) => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    };
    let listed: Vec<Value> = names
        .iter()
        .map(|name| {
            let path = folder.join(name);
            let found = detect::repositories(&path).unwrap_or_default();
            json!({
                "name": name,
                "path": path,
                "repository": found.first().is_some_and(|first| first == "."),
                "repositories": found.len(),
                "project": project_of(&path),
            })
        })
        .collect();
    Ok(json!({
        "path": folder,
        "shown": shown(&folder),
        "parent": folder.parent(),
        "folders": listed,
    }))
}

fn plan(projects: &Projects, asked: ProjectFolder) -> std::result::Result<Plan, Failure> {
    let expanded = home_expanded(&asked.folder);
    let typed = expanded.as_deref().unwrap_or(Path::new(&asked.folder));
    if !typed.is_absolute() {
        return Err(Failure::refused(format!(
            "the folder must be an absolute path, not {}",
            asked.folder
        )));
    }
    let folder = typed.canonicalize().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            Failure::refused(format!("{} does not exist", asked.folder))
        }
        _ => Failure::refused(format!("cannot read {}: {error}", asked.folder)),
    })?;
    if !folder.is_dir() {
        return Err(Failure::refused(format!(
            "{} is not a folder",
            folder.display()
        )));
    }

    let name = match asked.name {
        Some(name) if valid_name(&name) => name,
        Some(name) => {
            return Err(Failure::refused(format!(
                "{name} is not a name: use 1 to 64 letters, digits, dots, dashes or underscores, \
                 starting with a letter or a digit"
            )));
        }
        None => {
            let base = folder.file_name().and_then(|name| name.to_str());
            Some(slug(base.unwrap_or_default()))
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    Failure::refused(format!(
                        "no name can be made from {}: give one with --name",
                        folder.display()
                    ))
                })?
        }
    };

    let repositories = detect::repositories(&folder)
        .map_err(|error| Failure::refused(format!("cannot read {}: {error}", folder.display())))?;
    if repositories.is_empty() {
        return Err(Failure::refused(format!(
            "no Git repository in {}: it is not one, and none of its folders one or two levels \
             down holds a .git",
            folder.display()
        )));
    }

    // A file that cannot be used is a project all the same: its name is taken. A file that cannot
    // be read is another matter, said as it is.
    let taken = match projects.registry.load(&name) {
        Ok(project) => project.is_some(),
        Err(Error::Config(_)) => true,
        Err(error) => return Err(error.into()),
    };
    if taken || projects.known.contains_key(&name) {
        return Err(Failure::refused(format!(
            "a project named {name} already exists"
        )));
    }
    if let Some(project) = projects.known.values().find(|project| {
        project.folder == folder || project.folder.canonicalize().ok().as_ref() == Some(&folder)
    }) {
        return Err(Failure::refused(format!(
            "{} is already registered as {}",
            folder.display(),
            project.name
        )));
    }
    Ok(Plan {
        name,
        folder,
        repositories,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const FORMER: &str =
        include_str!("../../flottille-core/tests/fixtures/multi-repo-case-former-keys.toml");
    const CURRENT: &str = include_str!("../../flottille-core/tests/fixtures/multi-repo-case.toml");

    fn parse(text: &str) -> Project {
        Project::parse("app", text, Path::new("/data/flottille")).unwrap()
    }

    /// Each task's name and line.
    fn lines(project: &Project) -> Vec<(&str, &str)> {
        project
            .tasks
            .iter()
            .map(|(name, task)| (name.as_str(), task.line.as_str()))
            .collect()
    }

    #[test]
    fn the_former_keys_of_a_file_get_their_new_names_and_its_comments_stay() {
        let migrated = migrated(FORMER).expect("the file holds former keys");

        for comment in FORMER.lines().filter(|line| line.starts_with('#')) {
            assert!(migrated.contains(comment), "{comment}\n---\n{migrated}");
        }
        for former in [
            "[commands.\"api:dev\"]",
            "[procedures",
            "[references",
            "[[recipe]]",
            "[agent_actions]",
            "grants",
            "added_by",
            "command = ",
        ] {
            assert!(!migrated.contains(former), "{former}\n---\n{migrated}");
        }
        // Each entry stays where it was, its comment above it.
        for kept in [
            "# What we do before every review.\n[commands.open-prs]\n",
            "[skills.style] # the team's own words\n",
            "# The test data, once.\n[[setup]]\ntask = \"generate-data\"\n",
            "PATH = \"{workspace.path}/.bin\"\n\n[tasks.\"api:dev\"]\n",
        ] {
            assert!(migrated.contains(kept), "{kept}\n---\n{migrated}");
        }
        let (before, after) = (parse(FORMER), parse(&migrated));
        assert_eq!(lines(&after), lines(&before));
        assert_eq!(format!("{:?}", after.setup), format!("{:?}", before.setup));
        assert_eq!(after.commands.keys().collect::<Vec<_>>(), ["open-prs"]);
        assert_eq!(after.skills.keys().collect::<Vec<_>>(), ["style"]);
        assert_eq!(after.tasks.len(), 5);
        assert_eq!(after.setup.len(), 4);
    }

    #[test]
    fn a_task_under_both_its_former_and_its_new_key_stays_under_both() {
        let text = "folder = \"/work/app\"\n\n\
            # The build we had.\n[commands.build]\nline = \"make\"\n\n\
            # The build we have.\n[tasks.build]\nline = \"make all\"\n\n\
            [references.style]\npath = \"docs/style.md\"\n";

        assert_eq!(tasks_twice(text), ["build"]);
        let migrated = migrated(text).expect("the reference is renamed");
        for kept in [
            "# The build we had.\n[commands.build]\nline = \"make\"\n",
            "# The build we have.\n[tasks.build]\nline = \"make all\"\n",
            "[skills.style]",
        ] {
            assert!(migrated.contains(kept), "{kept}\n---\n{migrated}");
        }
    }

    #[test]
    fn a_tasks_that_is_not_a_table_loses_nothing() {
        let text = "tasks = \"oops\"\n\n# Ours.\n[commands.build]\nline = \"make\"\n";

        assert_eq!(migrated(text), None);
        let mut document: toml_edit::DocumentMut = text.parse().unwrap();
        assert!(!former_keys_renamed(&mut document));
        assert_eq!(document.to_string(), text);
    }

    #[test]
    fn the_comment_of_a_commands_header_stays_when_its_tasks_leave() {
        let text = "# What we run.\n[commands]\n\n[commands.build]\nline = \"make\"\n";

        let migrated = migrated(text).unwrap();
        assert!(
            migrated.contains("# What we run.\n[commands]\n"),
            "{migrated}"
        );
        assert!(migrated.contains("[tasks.build]\n"), "{migrated}");
        assert!(!migrated.contains("[commands.build]"), "{migrated}");
    }

    #[test]
    fn a_file_with_the_new_keys_is_left_as_it_is() {
        assert_eq!(migrated(CURRENT), None);
        let once = migrated(FORMER).unwrap();
        assert_eq!(migrated(&once), None, "a second start changes nothing");
        assert_eq!(migrated("not = [toml"), None);
    }

    #[test]
    fn the_new_fixture_reads_as_the_former_one_brought_to_the_new_keys() {
        let (current, former) = (parse(CURRENT), parse(FORMER));
        assert_eq!(lines(&current), lines(&former));
        assert_eq!(current.setup.len() + 1, former.setup.len());
    }
}
