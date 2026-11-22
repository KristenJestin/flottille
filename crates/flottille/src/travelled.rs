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
    Ok(())
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
    daemon.paths.data.join("projects")
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
}

/// Whether the settings of a launch of `from` are laid on `project`: only on one Flottille made
/// for that very machine's launches.
fn takes(project: &Project, from: &str) -> bool {
    project.from_machine.as_deref() == Some(from)
}

/// Classes what laying the settings of `asked`, from `from`, would change in the project `found`
/// names (nothing for a project they are not laid on), as the gate classes the same writes of an
/// agent here. Refused when the gate refuses one of them.
pub(crate) fn classed(
    daemon: &Daemon,
    found: &Found,
    asked: &LaunchAsk,
    from: &str,
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
            project.from_machine = Some(from.to_owned());
            let text = project.to_toml().map_err(|error| error.to_string())?;
            (project, text)
        }
        Found::Here(project) if takes(project, from) => {
            let text = projects::text(daemon, &project.name).map_err(|f| f.message)?;
            ((**project).clone(), text)
        }
        Found::Here(_) => return Ok(Classed::default()),
    };
    let after = projects::as_travelled(daemon, &before.name, &text, settings, from)
        .map_err(|failure| failure.message)?;
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
        classed: Classed::default(),
    };
    classing.tasks(&before)?;
    classing.setup(&before)?;
    classing.never(&before);
    classing.rules(&before)?;
    classing.checks(&before);
    classing.repositories(&before)?;
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

    /// Each repository's base branches and dependencies, classed as `project set_settings` is.
    fn repositories(&mut self, before: &Project) -> Result<(), String> {
        let after = self.after;
        for repository in &after.repositories {
            let was = before
                .repositories
                .iter()
                .find(|r| r.path == repository.path);
            let path = &repository.path;
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

    /// The branch and pull request conventions: settings like any other, to judge.
    fn conventions(&mut self, before: &Project) {
        let after = self.after;
        if before.branch_prefix != after.branch_prefix {
            self.say(format!("Its branch prefix: {:?}", after.branch_prefix));
            self.settings();
        }
        if before.branch_format != after.branch_format {
            match &after.branch_format {
                Some(format) => self.say(format!("Its branch format: {format}")),
                None => self.say("It removes its branch format".into()),
            }
            self.settings();
        }
        if before.pull_requests != after.pull_requests {
            let conventions = serde_json::to_string(&after.pull_requests).unwrap_or_default();
            self.say(format!("Its pull request conventions: {conventions}"));
            self.settings();
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
/// in, and each setting of the sender's it lays there (`classed`).
pub(crate) fn words(
    found: &Found,
    asked: &LaunchAsk,
    from: &str,
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
                Some(owner) if owner == from => {}
                Some(owner) => said.push(format!(
                    "In the project {name}, which Flottille made here for {owner}'s launches: it \
                     keeps {owner}'s settings, not {from}'s"
                )),
                None => said.push(format!(
                    "In the project {name}, which the user registered here: it keeps its own \
                     settings, not {from}'s"
                )),
            }
            name
        }
    };
    if !classed.words.is_empty() {
        said.push(format!(
            "{name} takes these settings of {} on {from}",
            asked.project
        ));
        said.extend(classed.words.iter().cloned());
    }
    said
}

/// The project `found` names, recognised again and made now when it is to be, under
/// `making_projects`: two launches of a new project at once make it once. Refused when it is no
/// longer the one the launch was judged for.
pub(crate) async fn again<'a>(
    daemon: &'a Daemon,
    found: Found,
    asked: &LaunchAsk,
    here: &str,
) -> Result<(Found, Option<tokio::sync::MutexGuard<'a, ()>>), String> {
    let Found::Made { name, .. } = &found else {
        return Ok((found, None));
    };
    let making = daemon.making_projects.lock().await;
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
    Ok((again, Some(making)))
}

/// Makes the project `found` names when it is to be made, for a launch of `from`.
pub(crate) fn made(
    daemon: &Daemon,
    found: &Found,
    asked: &LaunchAsk,
    from: &str,
) -> Result<(), String> {
    match found {
        Found::Here(_) => Ok(()),
        Found::Made { name, folder } => {
            projects::make_travelled(daemon, name, folder, &asked.repositories, from)
                .map(|_| ())
                .map_err(|failure| failure.message)
        }
    }
}

/// Lays the settings of the launch `asked`, from `from`, on `project` when Flottille made it for
/// that very machine's launches: a project the user registered keeps its own, and one made for
/// another machine keeps that machine's. Refused when they cannot be used here.
pub(crate) fn take_settings(
    daemon: &Daemon,
    project: &str,
    asked: &Value,
    from: &str,
) -> Result<(), String> {
    let Some(settings) = asked.get("settings").filter(|settings| !settings.is_null()) else {
        return Ok(());
    };
    let here = projects::load_project(daemon, project).map_err(|failure| failure.message)?;
    if !takes(&here, from) {
        return Ok(());
    }
    projects::take_travelled(daemon, project, settings, from)
        .map(|_| ())
        .map_err(|failure| failure.message)
}

/// Removes the project made for the launch `launch` of `from`, which did not start, once nothing
/// uses it: its agent ended, its workspace removed.
pub(crate) fn unmake(daemon: &std::sync::Arc<Daemon>, launch: &flottille_core::store::Launch) {
    if launch.asked["made"] != true {
        return;
    }
    let from = launch.asked["from"].as_str().unwrap_or_default().to_owned();
    let name = launch.project.clone();
    let folder = folder_for_others(daemon).join(&name);
    let daemon = daemon.clone();
    let machine = launch.machine.clone();
    let this = launch.launch.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
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
            match projects::unmake_travelled(&daemon, &name, &folder, &from) {
                Ok(()) => return,
                Err(failure)
                    if failure.message.ends_with("is in use")
                        && tokio::time::Instant::now() < deadline =>
                {
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
