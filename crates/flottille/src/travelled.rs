//! The project a launch to another machine runs in (`docs/remote.md`, "Which project"): the
//! receiving machine recognises one it has by its repositories, whatever its name or folder, or
//! makes it, its repositories cloned from what the caller sends, never from their remotes; and
//! the settings of the caller's project travel with each launch to a project Flottille made.

use std::path::{Path, PathBuf};

use flottille_core::project::{Project, valid_name};
use flottille_link::{LaunchAsk, RepositoryIdentity};
use serde_json::Value;

use crate::daemon::Daemon;
use crate::git::{self, Class};
use crate::projects;

/// The most root commits a repository's identity may name.
const MOST_ROOTS: usize = 16;

/// The longest URL an identity may give, in bytes.
const LONGEST_URL: usize = 2048;

/// The longest settings a launch may carry, as JSON, in bytes.
pub(crate) const LONGEST_SETTINGS: usize = 256 * 1024;

// The caller's machine.

/// What each repository of `place` is: the root commits of its branch, and the URL of the
/// project's remote of it without its credentials, when it has one.
pub(crate) async fn identities(
    project: &Project,
    place: &[(String, PathBuf)],
) -> Vec<RepositoryIdentity> {
    let mut identities = Vec::new();
    for (path, folder) in place {
        let mut roots = roots(folder, "HEAD").await;
        roots.truncate(MOST_ROOTS);
        let remote = project
            .repositories
            .iter()
            .find(|repository| &repository.path == path)
            .map_or("origin", |repository| repository.remote.as_str());
        let url = git::output(
            folder,
            &["remote", "get-url", "--end-of-options", remote],
            Class::Read,
        )
        .await
        .ok()
        .map(|url| flottille_core::declared::without_credentials(url.trim()))
        .filter(|url| !url.is_empty() && url.len() <= LONGEST_URL);
        identities.push(RepositoryIdentity {
            path: path.clone(),
            roots,
            url,
        });
    }
    identities
}

/// The commits with no parent reachable from `from` in the repository at `folder`.
async fn roots(folder: &Path, from: &str) -> Vec<String> {
    git::output(
        folder,
        &["rev-list", "--max-parents=0", from, "--"],
        Class::Read,
    )
    .await
    .map(|listed| listed.lines().map(str::to_owned).collect())
    .unwrap_or_default()
}

// The machine it runs on.

/// The project of this machine a launch runs in.
pub(crate) enum Found {
    /// One this machine has.
    Here(Box<Project>),
    /// None: it is made, under this name, in this folder.
    Made { name: String, folder: PathBuf },
}

/// Refused when the identities a launch gives are not bounded, or name a repository it does not.
fn check(asked: &LaunchAsk) -> Result<(), String> {
    if asked.identities.len() > asked.repositories.len() {
        return Err("a launch names each of its repositories once".to_owned());
    }
    for identity in &asked.identities {
        if !asked.repositories.contains(&identity.path) {
            return Err(format!(
                "the launch has no repository {}",
                identity.path.chars().take(64).collect::<String>()
            ));
        }
        if identity.roots.len() > MOST_ROOTS
            || identity.roots.iter().any(|root| {
                !matches!(root.len(), 40 | 64) || !root.chars().all(|c| c.is_ascii_hexdigit())
            })
        {
            return Err(format!(
                "the root commits of {} are not commits: {MOST_ROOTS} at most",
                identity.path
            ));
        }
        if let Some(url) = &identity.url
            && (url.len() > LONGEST_URL || url.contains(|c: char| c.is_control()))
        {
            return Err(format!("the URL of {} is not one", identity.path));
        }
    }
    if let Some(settings) = &asked.settings {
        if !serde_json::to_string(settings).is_ok_and(|text| text.len() <= LONGEST_SETTINGS) {
            return Err(format!(
                "the settings of {} are more than {} KiB",
                asked.project,
                LONGEST_SETTINGS / 1024
            ));
        }
        // Only what travels is taken, nor kept in the launch's record: a variable never is.
        flottille_core::project::travelled_toml(settings)
            .map_err(|why| format!("the settings of {}: {why}", asked.project))?;
    }
    if asked.copied.len() > MOST_COPIED {
        return Err(format!(
            "a launch names {MOST_COPIED} files its setup copies at most"
        ));
    }
    if let Some(path) = asked.copied.iter().find(|path| !fit_path(path)) {
        return Err(format!(
            "{:?} is not a path inside the project",
            path.chars().take(64).collect::<String>()
        ));
    }
    Ok(())
}

// What the setup copies.

/// The most files a launch names that its setup copies.
const MOST_COPIED: usize = 32;

/// The largest file the setup copies that becomes a vault entry, in bytes.
pub(crate) const LARGEST_COPIED: u64 = 64 * 1024;

/// The files the setup of `project` copies (its `copy` steps) that its folder holds, in the
/// repositories at `paths`, by their paths in the project: what a launch from there names, never
/// what they hold. A link is not one: it would lead elsewhere.
pub(crate) fn copied(project: &Project, paths: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    for step in &project.setup {
        let Some(globs) = &step.copy else {
            continue;
        };
        let places: Vec<&str> = match (&step.repository, step.for_each_repository) {
            (Some(path), _) if paths.contains(path) => vec![path.as_str()],
            (Some(_), _) => Vec::new(),
            (None, true) => paths.iter().map(String::as_str).collect(),
            (None, false) => vec!["."],
        };
        for place in places {
            let source = match place {
                "." => project.folder.clone(),
                path => project.folder.join(path),
            };
            for relative in crate::workspaces::copy_sources(&source, globs).unwrap_or_default() {
                if std::fs::symlink_metadata(source.join(&relative))
                    .is_ok_and(|metadata| !metadata.is_file())
                {
                    continue;
                }
                let path = match place {
                    "." => relative.to_string_lossy().into_owned(),
                    place => format!("{place}/{}", relative.to_string_lossy()),
                };
                if fit_path(&path) && !found.contains(&path) {
                    found.push(path);
                }
            }
        }
    }
    found.truncate(MOST_COPIED);
    found
}

/// Whether `path` names a file inside a project: relative, of no `.`, `..` or `.git` part, no
/// control character, 255 bytes at most.
pub(crate) fn fit_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && !path.starts_with('/')
        && !path.contains(|c: char| c.is_control() || c == '\\')
        && path
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".." | ".git"))
}

/// The vault entry the file `path` of the project `project` is offered as: `<project>-<path>`,
/// each `/` a `-` and each character an entry's name cannot hold a `_`, 64 at most.
pub fn entry_for(project: &str, path: &str) -> String {
    format!("{project}-{path}")
        .chars()
        .map(|c| match c {
            '/' => '-',
            c if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') => c,
            _ => '_',
        })
        .take(64)
        .collect()
}

/// A file the setup of a launch's project copies, which this machine's project lacks: made, once
/// the launch is allowed, from the vault entry `entry`, shared from the caller's machine.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Need {
    pub path: String,
    pub entry: String,
}

/// What the launch `asked` needs here that the project in `folder` lacks: each file its setup
/// copies that the folder does not hold, with the vault entry it is made from.
pub(crate) fn needs(asked: &LaunchAsk, folder: &Path) -> Vec<Need> {
    asked
        .copied
        .iter()
        .filter(|path| std::fs::symlink_metadata(folder.join(path)).is_err())
        .map(|path| Need {
            path: path.clone(),
            entry: entry_for(&asked.project, path),
        })
        .collect()
}

/// Those of `needs` whose vault entry this machine lacks: what the user is asked to share.
pub(crate) fn missing(daemon: &Daemon, needs: &[Need]) -> Vec<Need> {
    let held = daemon
        .read(|store| store.vault_entries())
        .unwrap_or_default();
    needs
        .iter()
        .filter(|need| !held.iter().any(|entry| entry.name == need.entry))
        .cloned()
        .collect()
}

/// `a`, `a and b`, `a, b and c`.
pub(crate) fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the user is asked, on every device, for a launch that lacks `missing` here:
/// `app needs .env on server: share it?`.
pub(crate) fn asked_to_share(project: &str, here: &str, missing: &[Need]) -> String {
    let paths: Vec<String> = missing.iter().map(|need| need.path.clone()).collect();
    let them = if missing.len() == 1 { "it" } else { "them" };
    format!(
        "{project} needs {} on {here}: share {them}?",
        listed(&paths)
    )
}

/// Makes each file of `needs` the project `project` lacks from this machine's vault entry of it,
/// never over anything there, readable by the user alone: the setup's `copy` steps then copy it
/// as any file of the project's. What is still missing is refused, naming it.
pub(crate) async fn lay(daemon: &Daemon, project: &str, needs: &[Need]) -> Result<(), String> {
    if needs.is_empty() {
        return Ok(());
    }
    let project = projects::load_project(daemon, project).map_err(|failure| failure.message)?;
    let root = std::fs::canonicalize(&project.folder)
        .map_err(|error| format!("{}: {error}", project.folder.display()))?;
    let mut lacking = Vec::new();
    for need in needs {
        let target = root.join(&need.path);
        if !fit_path(&need.path) || std::fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        let Some(secret) = crate::vault::of_file(daemon, &need.entry).await else {
            lacking.push(need.path.clone());
            continue;
        };
        crate::vault::mask_lines(&secret);
        let written = write_new(&root, &target, &secret);
        if let Err(why) = written {
            return Err(format!("{} cannot be made: {why}", need.path));
        }
    }
    match lacking.is_empty() {
        true => Ok(()),
        false => Err(format!(
            "the project {} here still lacks {}: its vault entries were not shared with it",
            project.name,
            listed(&lacking)
        )),
    }
}

/// Writes `text` as the new file `target`, inside `root` (reached through no link), readable by
/// the user alone.
fn write_new(root: &Path, target: &Path, text: &str) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let parent = target.parent().unwrap_or(root);
    std::fs::create_dir_all(parent).map_err(|error| error.kind().to_string())?;
    let real = std::fs::canonicalize(parent).map_err(|error| error.kind().to_string())?;
    if !real.starts_with(root) {
        return Err("its folder leads out of the project".to_owned());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(real.join(target.file_name().unwrap_or_default()))
        .map_err(|error| error.kind().to_string())?;
    file.write_all(text.as_bytes())
        .map_err(|error| error.kind().to_string())
}

/// The project of this machine `asked` runs in: the one whose repositories are the launch's, by
/// their root commits and their remotes' URLs, whatever its name or folder (one of the launch's
/// name first); else one to make, under the launch's project name or the first of `-2`, `-3`…
/// that is free, in this machine's folder for projects from other machines. A launch of a
/// sender that gives no identities is taken by its project's name alone, as before.
pub(crate) async fn recognise(
    daemon: &Daemon,
    asked: &LaunchAsk,
    here: &str,
) -> Result<Found, String> {
    check(asked)?;
    if asked.identities.is_empty() {
        let project = projects::load_project(daemon, &asked.project)
            .map_err(|_| format!("the project {} is not on {here}", asked.project))?;
        if let Some(missing) = asked
            .repositories
            .iter()
            .find(|path| !project.repositories.iter().any(|r| &r.path == *path))
        {
            return Err(format!(
                "the project {} on {here} has no repository {missing}: add it there with the \
                 same path",
                project.name
            ));
        }
        return Ok(Found::Here(Box::new(project)));
    }
    let mut candidates = projects::all(daemon);
    // The launch's own name first: when two projects hold the same repositories, it is that one.
    candidates.sort_by_key(|project| project.name != asked.project);
    for project in candidates {
        if is_the_launch_s(&project, asked).await {
            return Ok(Found::Here(Box::new(project)));
        }
    }
    if !valid_name(&asked.project) {
        return Err(format!(
            "{:?} is not a project's name",
            asked.project.chars().take(64).collect::<String>()
        ));
    }
    let root = folder_for_others(daemon);
    for n in 1.. {
        let name = match n {
            1 => asked.project.clone(),
            n => format!("{}-{n}", asked.project),
        };
        if !valid_name(&name) {
            break;
        }
        let folder = root.join(&name);
        if !projects::exists(daemon, &name) && free(&folder) {
            return Ok(Found::Made { name, folder });
        }
    }
    Err(format!(
        "no name is free on {here} for the project {}",
        asked.project
    ))
}

/// Where this machine makes the projects that come from other machines.
pub(crate) fn folder_for_others(daemon: &Daemon) -> PathBuf {
    crate::travelled_folder::folder(daemon)
}

/// Whether nothing is at `folder`, or an empty folder.
fn free(folder: &Path) -> bool {
    match std::fs::read_dir(folder) {
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        Ok(mut entries) => entries.next().is_none(),
    }
}

/// Whether `project` holds each repository of the launch at its path, the same repository: a
/// root commit in common and, when both have a remote, the same URL. A repository that is not
/// there yet is the launch's only in a project of the launch's name: it is received there.
async fn is_the_launch_s(project: &Project, asked: &LaunchAsk) -> bool {
    for path in &asked.repositories {
        if !project.repositories.iter().any(|r| &r.path == path) {
            return false;
        }
        let folder = match path.as_str() {
            "." => project.folder.clone(),
            path => project.folder.join(path),
        };
        // A link in a project Flottille made, which came with a launch's code, leads nowhere
        // else: never to one of the user's own repositories.
        if project.from_machine.is_some()
            && std::fs::symlink_metadata(&folder).is_ok()
            && projects::leads_inside(&project.folder, path).is_err()
        {
            return false;
        }
        if !folder.join(".git").exists() {
            if project.name != asked.project {
                return false;
            }
            continue;
        }
        let Some(identity) = asked.identities.iter().find(|i| &i.path == path) else {
            return false;
        };
        let here = roots(&folder, "--all").await;
        if !identity.roots.iter().any(|root| here.contains(root)) {
            return false;
        }
        let remote = project
            .repositories
            .iter()
            .find(|r| &r.path == path)
            .map_or("origin", |r| r.remote.as_str());
        let url = git::output(
            &folder,
            &["remote", "get-url", "--end-of-options", remote],
            Class::Read,
        )
        .await
        .ok();
        if let (Some(theirs), Some(ours)) = (&identity.url, url)
            && same_place(theirs) != same_place(ours.trim())
        {
            return false;
        }
    }
    true
}

/// A remote's URL as the place it leads to, whatever the way it is written: its host, lower
/// case, and its path, with no scheme, user, port, `.git` or final slash.
fn same_place(url: &str) -> String {
    let url = flottille_core::declared::without_credentials(url);
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => {
            let rest = rest.rsplit_once('@').map_or(rest, |(_, rest)| rest);
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            (
                host.split(':').next().unwrap_or(host).to_owned(),
                path.to_owned(),
            )
        }
        // `git@host:path`, scp-like; or a folder.
        None => match url.split_once(':') {
            Some((host, path)) if !host.contains('/') => (
                host.rsplit_once('@')
                    .map_or(host, |(_, host)| host)
                    .to_owned(),
                path.to_owned(),
            ),
            _ => (String::new(), url.clone()),
        },
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    format!("{}/{path}", host.to_lowercase())
}

/// What laying a launch's settings would change in its project here, each line and key classed
/// as the matching write of an agent here is (`docs/tools.md`, "The project"): the strongest
/// class is the launch's.
#[derive(Default)]
pub(crate) struct Classed {
    /// One of them is base: only the user decides the launch, never the judge.
    pub base: bool,
    /// Why, as the gate says it of each write.
    pub reasons: Vec<String>,
    /// Each line and key that would change, in words.
    pub words: Vec<String>,
    /// The project's file they were classed against, as it is written: they are laid only on it
    /// (`projects::take_travelled`). None when nothing is laid.
    pub text: Option<String>,
}

/// Whether the settings of a launch of the machine `machine` (its key) are laid on `project`:
/// only on one Flottille made for that very machine's launches. Its key, never its member's name,
/// which another member may rename it from and take.
pub(crate) fn takes(project: &Project, machine: &str) -> bool {
    project.from_machine.as_deref() == Some(machine)
}

/// Classes what laying the settings of `asked`, from `from` (its machine's key `machine`), would
/// change in the project `found` names (nothing for a project they are not laid on), as the gate
/// classes the same writes of an agent here. Refused when the gate refuses one of them.
pub(crate) fn classed(
    daemon: &Daemon,
    found: &Found,
    asked: &LaunchAsk,
    from: &str,
    machine: &str,
) -> Result<Classed, String> {
    let Some(settings) = &asked.settings else {
        return Ok(Classed::default());
    };
    let (before, text) = match found {
        Found::Made { name, folder } => {
            let mut project = Project::new(
                name,
                folder.clone(),
                asked.repositories.clone(),
                &daemon.paths.data,
            );
            project.from_machine = Some(machine.to_owned());
            let text = project.to_toml().map_err(|error| error.to_string())?;
            (project, text)
        }
        Found::Here(project) if takes(project, machine) => {
            // The project as the very text classed says it, which is what it is laid on.
            let text = projects::text(daemon, &project.name).map_err(|f| f.message)?;
            let before = projects::parsed(daemon, &project.name, &text).map_err(|f| f.message)?;
            (before, text)
        }
        Found::Here(_) => return Ok(Classed::default()),
    };
    let after = projects::as_travelled(daemon, &before.name, &text, settings, from)
        .map_err(|failure| failure.message)?;
    let others = projects::all(daemon);
    let mut classing = Classing {
        daemon,
        grant: crate::mcp::Grant {
            agent: format!("launch-{}", asked.launch),
            epoch: 0,
            tools: Vec::new(),
            place: launch_place(&after, &asked.branch),
            launched: false,
        },
        after: &after,
        from,
        classed: Classed {
            text: Some(text),
            ..Classed::default()
        },
    };
    classing.tasks(&before)?;
    classing.setup(&before)?;
    classing.never(&before);
    classing.rules(&before)?;
    classing.checks(&before);
    classing.repositories(&before, &others)?;
    classing.conventions(&before);
    Ok(classing.classed)
}

/// Where the launch's agent will work, which a write of the settings is classed in, as a local
/// write is in its agent's place (`docs/tools.md`, "The project"): a new workspace of `project` on
/// `branch`, not made yet.
fn launch_place(project: &Project, branch: &str) -> crate::place::Place {
    let name = match flottille_core::project::slug(branch) {
        name if name.is_empty() => "launch".to_owned(),
        name => name,
    };
    let root = project.workspaces_root.join(&name);
    crate::place::Place {
        project: project.name.clone(),
        workspace: Some(name),
        repositories: project
            .repositories
            .iter()
            .map(|r| {
                let folder = match r.path.as_str() {
                    "." => root.clone(),
                    path => root.join(path),
                };
                (r.path.clone(), folder)
            })
            .collect(),
        root,
        branch: branch.to_owned(),
        variables: project.variables.clone(),
        home: std::env::var_os("HOME").map(Into::into),
    }
}

/// The settings a launch would lay, being classed.
struct Classing<'a> {
    daemon: &'a Daemon,
    /// An agent of the project in its folder, as the gate sees a write of its settings.
    grant: crate::mcp::Grant,
    after: &'a Project,
    from: &'a str,
    classed: Classed,
}

impl Classing<'_> {
    /// The gate's class of the write `args` of `tool` an agent here would make for `what`.
    fn class(&mut self, tool: &str, args: Value, what: &str) -> Result<(), String> {
        use crate::gate::{Class, Verdict};
        match crate::mcp::classified_in(self.daemon, &self.grant, Some(self.after), tool, &args) {
            Verdict::Deny(why) => Err(format!("{what} from {}: {why}", self.from)),
            Verdict::Ask(ask) => {
                self.classed.base |= ask.class == Class::Base;
                self.reason(ask.reasons.into_iter().map(|why| format!("{what}: {why}")));
                Ok(())
            }
            Verdict::Allow => Ok(()),
        }
    }

    fn reason(&mut self, reasons: impl IntoIterator<Item = String>) {
        for reason in reasons {
            if !self.classed.reasons.contains(&reason) {
                self.classed.reasons.push(reason);
            }
        }
    }

    /// A change no agent's tool makes here: the user's alone, base.
    fn base(&mut self, why: String) {
        self.classed.base = true;
        self.reason([why]);
    }

    /// A change of the project's settings an agent's write would make: to judge.
    fn settings(&mut self) {
        self.reason([crate::actions::asked("settings")]);
    }

    fn say(&mut self, words: String) {
        self.classed.words.push(words);
    }

    /// Each task added, changed or removed, classed as `tasks add`, `edit` or `remove` is.
    fn tasks(&mut self, before: &Project) -> Result<(), String> {
        let after = self.after;
        for (name, task) in &after.tasks {
            let now = serde_json::to_value(task).map_err(|error| error.to_string())?;
            let mut args = serde_json::json!({"name": name});
            match before.tasks.get(name) {
                None => {
                    args = now.clone();
                    args["action"] = "add".into();
                    args["name"] = name.as_str().into();
                    self.say(format!(
                        "It adds the task {name}: `{}`{}",
                        task.line,
                        said(task)
                    ));
                }
                Some(was) if was != task => {
                    let was_json = serde_json::to_value(was).map_err(|error| error.to_string())?;
                    args["action"] = "edit".into();
                    for (key, value) in now.as_object().into_iter().flatten() {
                        if was_json.get(key) != Some(value) {
                            args[key] = value.clone();
                        }
                    }
                    for key in was_json.as_object().into_iter().flatten().map(|(k, _)| k) {
                        if now.get(key).is_none() {
                            args[key] = Value::Null;
                        }
                    }
                    if was.ask_before_running && !task.ask_before_running {
                        self.say(format!("It turns off asking before the task {name} runs"));
                    }
                    self.say(format!(
                        "It changes the task {name}: `{}`{}",
                        task.line,
                        said(task)
                    ));
                }
                Some(_) => continue,
            }
            self.class("tasks", args, &format!("the task {name}"))?;
        }
        for name in before.tasks.keys() {
            if !after.tasks.contains_key(name) {
                self.say(format!("It removes the task {name}"));
                self.class(
                    "tasks",
                    serde_json::json!({"action": "remove", "name": name}),
                    &format!("the task {name}"),
                )?;
            }
        }
        Ok(())
    }

    /// Each setup step that is not the one at its place here, classed as `setup add` is.
    fn setup(&mut self, before: &Project) -> Result<(), String> {
        let after = self.after;
        for (at, step) in after.setup.iter().enumerate() {
            if before.setup.get(at) == Some(step) {
                continue;
            }
            let step = serde_json::to_value(step).map_err(|error| error.to_string())?;
            let what = format!("setup step {}", at + 1);
            self.say(format!("Its {what}: {step}"));
            self.class(
                "setup",
                serde_json::json!({"action": "add", "step": step}),
                &what,
            )?;
        }
        if after.setup.len() < before.setup.len() {
            self.say(format!(
                "It removes the setup steps after step {}",
                after.setup.len()
            ));
            self.reason([crate::actions::asked("setup")]);
        }
        Ok(())
    }

    /// The never list: what it no longer refuses is the user's word alone.
    fn never(&mut self, before: &Project) {
        let after = self.after;
        let line = |entries: Vec<&Vec<String>>| {
            entries
                .iter()
                .map(|entry| format!("`{}`", entry.join(" ")))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let removed: Vec<_> = before
            .never
            .iter()
            .filter(|entry| !after.never.contains(entry))
            .collect();
        let added: Vec<_> = after
            .never
            .iter()
            .filter(|entry| !before.never.contains(entry))
            .collect();
        if !removed.is_empty() {
            match after.never.is_empty() {
                true => self.say(format!(
                    "It empties the never list, which was {}",
                    line(removed)
                )),
                false => self.say(format!("It removes from the never list {}", line(removed))),
            }
            self.base(
                "it lets run what the project's never list refused: only the user's word changes \
                 that list"
                    .into(),
            );
        }
        if !added.is_empty() {
            self.say(format!("It adds to the never list {}", line(added)));
            self.settings();
        }
    }

    /// The rules, classed as `project set_rules` is.
    fn rules(&mut self, before: &Project) -> Result<(), String> {
        let after = self.after;
        let mut args = serde_json::json!({"action": "set_rules"});
        if before.rules != after.rules {
            let text = after.rules.clone().unwrap_or_default();
            match text.is_empty() {
                true => self.say("It removes the project's rules".into()),
                false => self.say(format!("It sets the project's rules: {text}")),
            }
            args["text"] = text.into();
        }
        if before.rules_alone != after.rules_alone {
            match after.rules_alone {
                true => self.say(
                    "It gives the project's agents its rules alone, without the global ones".into(),
                ),
                false => self.say("It gives the project's agents the global rules too".into()),
            }
            args["rules_alone"] = after.rules_alone.into();
        }
        if args.as_object().is_some_and(|args| args.len() > 1) {
            self.class("project", args, "its rules")?;
        }
        Ok(())
    }

    /// The message and commit checks: lines that run by themselves, which only the user sets
    /// here (`project.set_message_checks`, `project.set_commit_checks`).
    fn checks(&mut self, before: &Project) {
        let after = self.after;
        let lines = |lines: &[String]| {
            lines
                .iter()
                .map(|line| format!("`{line}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if before.message_checks != after.message_checks {
            match after.message_checks.is_empty() {
                true => self.say("It removes the message checks".into()),
                false => self.say(format!(
                    "Its message checks, run on each message of an agent: {}",
                    lines(&after.message_checks)
                )),
            }
            self.base(
                "it sets the message checks, lines that run by themselves on the agents' \
                 messages: only the user's word sets them"
                    .into(),
            );
        }
        for repository in &after.repositories {
            let was = before
                .repositories
                .iter()
                .find(|r| r.path == repository.path)
                .map(|r| r.commit_checks.clone())
                .unwrap_or_default();
            let checks = &repository.commit_checks;
            if was == *checks {
                continue;
            }
            self.say(format!(
                "The commit checks of {}: pre-commit {}; commit-msg {}",
                repository.path,
                lines(&checks.pre_commit),
                lines(&checks.commit_msg)
            ));
            self.base(
                "it sets the commit checks, lines that run by themselves at each commit: only \
                 the user's word sets them"
                    .into(),
            );
        }
    }

    /// Each repository's base branches and dependencies, classed as `project set_settings` is;
    /// a repository the project lacks, which no agent's tool adds, is the user's word, and checked
    /// as one the user adds (`projects::refused_repository`).
    fn repositories(&mut self, before: &Project, others: &[Project]) -> Result<(), String> {
        let after = self.after;
        for repository in &after.repositories {
            let was = before
                .repositories
                .iter()
                .find(|r| r.path == repository.path);
            let path = &repository.path;
            if was.is_none() {
                if let Some(why) = projects::refused_repository(others, before, path) {
                    return Err(format!("the repository {path} from {}: {why}", self.from));
                }
                self.say(format!("It adds the repository {path} to the project"));
                self.base(format!(
                    "it adds the repository {path} to the project: only the user's word adds one"
                ));
            }
            let mut changed = serde_json::json!({"path": path});
            if was.is_none_or(|was| was.base != repository.base) {
                self.say(format!(
                    "The base branches of {path}: {}",
                    repository.base.join(", ")
                ));
                changed["base"] = repository.base.clone().into();
            }
            if was.is_none_or(|was| was.depends_on != repository.depends_on) {
                if !repository.depends_on.is_empty() || was.is_some() {
                    self.say(format!(
                        "{path} depends on: {}",
                        repository.depends_on.join(", ")
                    ));
                }
                changed["depends_on"] = repository.depends_on.clone().into();
            }
            if changed.as_object().is_some_and(|changed| changed.len() > 1) {
                self.class(
                    "project",
                    serde_json::json!({"action": "set_settings", "repositories": [changed]}),
                    &format!("the repository {path}"),
                )?;
            }
        }
        Ok(())
    }

    /// The branch and pull request conventions, which no agent's tool writes: the user's word,
    /// base. A pull request's template is text given to the agents, as the rules are.
    fn conventions(&mut self, before: &Project) {
        let after = self.after;
        let why = "it sets the project's branch or pull request conventions: only the user's word \
                   sets them";
        if before.branch_prefix != after.branch_prefix {
            self.say(format!("Its branch prefix: {:?}", after.branch_prefix));
            self.base(why.into());
        }
        if before.branch_format != after.branch_format {
            match &after.branch_format {
                Some(format) => self.say(format!("Its branch format: {format}")),
                None => self.say("It removes its branch format".into()),
            }
            self.base(why.into());
        }
        if before.pull_requests != after.pull_requests {
            let conventions = serde_json::to_string(&after.pull_requests).unwrap_or_default();
            self.say(format!("Its pull request conventions: {conventions}"));
            self.base(why.into());
        }
    }
}

/// A task's flags, after its line.
fn said(task: &flottille_core::project::Task) -> String {
    let mut flags = Vec::new();
    if let Some(repository) = &task.repository {
        flags.push(format!("in {repository}"));
    }
    for (on, flag) in [
        (task.serve, "keeps running"),
        (task.check, "a check"),
        (task.read_only, "changes nothing"),
        (task.ask_before_running, "asks before it runs"),
    ] {
        if on {
            flags.push(flag.to_owned());
        }
    }
    if !task.needs.is_empty() {
        flags.push(format!("needs {}", task.needs.join(", ")));
    }
    match flags.is_empty() {
        true => String::new(),
        false => format!(" ({})", flags.join(", ")),
    }
}

/// The words that say, to the judge and the user, which project of this machine the launch runs
/// in, and each setting of the sender's it lays there (`classed`). `from` is the sender's name,
/// `machine` its key, and `name_of` the name of the member whose machine has a key.
pub(crate) fn words(
    found: &Found,
    asked: &LaunchAsk,
    (from, machine): (&str, &str),
    name_of: impl Fn(&str) -> Option<String>,
    classed: &Classed,
) -> Vec<String> {
    let mut said = Vec::new();
    let name = match found {
        Found::Made { name, folder } => {
            said.push(format!(
                "It makes the project {name} on this machine, in {}, its repositories cloned \
                 from what {from} sends",
                folder.display()
            ));
            name
        }
        Found::Here(project) => {
            let name = &project.name;
            match project.from_machine.as_deref() {
                _ if asked.settings.is_none() => {}
                Some(owner) if owner == machine => {}
                Some(owner) => {
                    let owner = name_of(owner).unwrap_or_else(|| "another machine".to_owned());
                    said.push(format!(
                        "In the project {name}, which Flottille made here for {owner}'s launches: \
                         it keeps {owner}'s settings, not {from}'s"
                    ))
                }
                None => said.push(format!(
                    "In the project {name}, which the user registered here: it keeps its own \
                     settings, not {from}'s"
                )),
            }
            name
        }
    };
    // Recognised again (`again`), a project to make may be one made meanwhile for another
    // machine: it takes none of these settings.
    let laid = match found {
        Found::Made { .. } => true,
        Found::Here(project) => takes(project, machine),
    };
    if laid && !classed.words.is_empty() {
        said.push(format!(
            "{name} takes these settings of {} on {from}",
            asked.project
        ));
        said.extend(classed.words.iter().cloned());
    }
    said
}

/// The project `found` names, recognised again, under `making_projects`, held until the launch
/// is recorded and its project made: two launches of a new project at once make it once, and a
/// project made for a launch that did not start is not removed (`unmake`) under one that runs in
/// it. Refused when it is no longer the one the launch was judged for.
pub(crate) async fn again<'a>(
    daemon: &'a Daemon,
    found: Found,
    asked: &LaunchAsk,
    here: &str,
) -> Result<(Found, tokio::sync::MutexGuard<'a, ()>), String> {
    let making = daemon.making_projects.lock().await;
    let name = match &found {
        Found::Here(project) if projects::exists(daemon, &project.name) => {
            return Ok((found, making));
        }
        Found::Here(project) => &project.name,
        Found::Made { name, .. } => name,
    };
    let again = recognise(daemon, asked, here).await?;
    let same = match &again {
        Found::Here(project) => &project.name == name,
        Found::Made { name: again, .. } => again == name,
    };
    if !same {
        return Err(format!(
            "the projects of {here} changed while the launch was judged: launch it again"
        ));
    }
    Ok((again, making))
}

/// Makes the project `found` names when it is to be made, for a launch of the machine `machine`
/// (its key).
pub(crate) fn made(
    daemon: &Daemon,
    found: &Found,
    asked: &LaunchAsk,
    machine: &str,
) -> Result<(), String> {
    match found {
        Found::Here(_) => Ok(()),
        Found::Made { name, folder } => {
            projects::make_travelled(daemon, name, folder, &asked.repositories, machine)
                .map(|_| ())
                .map_err(|failure| failure.message)
        }
    }
}

/// Lays the settings of the launch `asked`, from the machine `machine` (its key), on `project`
/// when Flottille made it for that very machine's launches, and only on the file `classed` was
/// (`Classed::text`): a project the user registered keeps its own, and one made for another
/// machine keeps that machine's. Refused when they cannot be used here, or the project changed
/// since.
pub(crate) fn take_settings(
    daemon: &Daemon,
    project: &str,
    asked: &Value,
    machine: &str,
    classed: Option<&str>,
) -> Result<(), String> {
    let Some(settings) = asked.get("settings").filter(|settings| !settings.is_null()) else {
        return Ok(());
    };
    let here = projects::load_project(daemon, project).map_err(|failure| failure.message)?;
    if !takes(&here, machine) {
        return Ok(());
    }
    projects::take_travelled(daemon, project, settings, machine, classed)
        .map(|_| ())
        .map_err(|failure| failure.message)
}

/// Removes the project made for the launch `launch`, which did not start, once nothing uses it:
/// its agent ended, its workspace removed.
pub(crate) fn unmake(daemon: &std::sync::Arc<Daemon>, launch: &flottille_core::store::Launch) {
    if launch.asked["made"] != true {
        return;
    }
    let name = launch.project.clone();
    // Where it was made: the folder for projects from other machines may have changed since.
    let folder = launch.asked["folder"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| folder_for_others(daemon).join(&name));
    let daemon = daemon.clone();
    let machine = launch.machine.clone();
    let this = launch.launch.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            // A launch being recorded in it finds it gone, or is seen here (`again`).
            let making = daemon.making_projects.lock().await;
            // Another launch of that machine that may still start in it keeps it.
            let others = daemon
                .read(|store| {
                    store.launches(
                        "in",
                        Some(&machine),
                        flottille_core::store::LaunchFilter::All,
                    )
                })
                .unwrap_or_default();
            if others.iter().any(|other| {
                other.launch != this
                    && other.project == name
                    && matches!(other.state.as_str(), "held" | "accepted" | "started")
            }) {
                return;
            }
            match projects::unmake_travelled(&daemon, &name, &folder, &machine) {
                Ok(()) => return,
                Err(failure)
                    if failure.message.ends_with("is in use")
                        && tokio::time::Instant::now() < deadline =>
                {
                    drop(making);
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                Err(failure) => {
                    eprintln!(
                        "flottille daemon: the project {name}, made for launch {this}, is kept: {}",
                        failure.message
                    );
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_is_the_same_place_however_it_is_written() {
        let place = "github.com/acme/ledgerly";
        for url in [
            "https://github.com/acme/ledgerly.git",
            "https://user:token@GitHub.com/acme/ledgerly/",
            "git@github.com:acme/ledgerly.git",
            "ssh://git@github.com:22/acme/ledgerly",
        ] {
            assert_eq!(same_place(url), place, "{url}");
        }
        assert_ne!(same_place("git@github.com:acme/other.git"), place);
    }
}
