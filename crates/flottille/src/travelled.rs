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
    if let Some(settings) = &asked.settings
        && !serde_json::to_string(settings).is_ok_and(|text| text.len() <= LONGEST_SETTINGS)
    {
        return Err(format!(
            "the settings of {} are more than {} KiB",
            asked.project,
            LONGEST_SETTINGS / 1024
        ));
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

/// The words that say, to the judge and the user, which project of this machine the launch runs
/// in, and the settings of the sender's it takes, with what they run without an agent's call.
pub(crate) fn words(found: &Found, asked: &LaunchAsk, from: &str) -> Vec<String> {
    let (name, travelled) = match found {
        Found::Made { name, folder } => {
            let mut said = vec![format!(
                "It makes the project {name} on this machine, in {}, its repositories cloned \
                 from what {from} sends",
                folder.display()
            )];
            said.extend(settings_words(name, asked, from));
            return said;
        }
        Found::Here(project) => (&project.name, project.from_machine.is_some()),
    };
    if !travelled {
        let mut said = Vec::new();
        if asked.settings.is_some() {
            said.push(format!(
                "In the project {name}, which the user registered here: it keeps its own \
                 settings, not {from}'s"
            ));
        }
        return said;
    }
    settings_words(name, asked, from)
}

fn settings_words(name: &str, asked: &LaunchAsk, from: &str) -> Vec<String> {
    let Some(settings) = &asked.settings else {
        return Vec::new();
    };
    let mut said = format!("{name} takes the settings of {} on {from}", asked.project);
    let runs = runs(settings);
    if !runs.is_empty() {
        said.push_str(&format!(", which run: {}", runs.join("; ")));
    }
    vec![said]
}

/// What `settings` run by themselves, without an agent's call: the setup's steps, the message
/// checks and the commit checks.
fn runs(settings: &Value) -> Vec<String> {
    let mut runs = Vec::new();
    for step in settings["setup"].as_array().into_iter().flatten() {
        if let Some(line) = step["run"].as_str() {
            runs.push(format!("setup `{line}`"));
        }
        if let Some(task) = step["task"].as_str() {
            let line = settings["tasks"][task]["line"].as_str().unwrap_or_default();
            runs.push(format!("setup task {task} `{line}`"));
        }
    }
    for line in settings["message_checks"].as_array().into_iter().flatten() {
        if let Some(line) = line.as_str() {
            runs.push(format!("message check `{line}`"));
        }
    }
    for repository in settings["repositories"].as_array().into_iter().flatten() {
        for hook in ["pre_commit", "commit_msg"] {
            for line in repository["commit_checks"][hook]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(line) = line.as_str() {
                    runs.push(format!("commit check `{line}`"));
                }
            }
        }
    }
    runs
}

/// The project `found` names, made now when it is to be.
pub(crate) fn made(
    daemon: &Daemon,
    found: Found,
    asked: &LaunchAsk,
    from: &str,
) -> Result<Project, String> {
    match found {
        Found::Here(project) => Ok(*project),
        Found::Made { name, folder } => {
            projects::make_travelled(daemon, &name, &folder, &asked.repositories, from)
                .map_err(|failure| failure.message)
        }
    }
}

/// Lays the settings of the launch `asked`, from `from`, on `project` when Flottille made it: a
/// project the user registered keeps its own. Refused when they cannot be used here.
pub(crate) fn take_settings(
    daemon: &Daemon,
    project: &str,
    asked: &Value,
    from: &str,
) -> Result<(), String> {
    let Some(settings) = asked.get("settings").filter(|settings| !settings.is_null()) else {
        return Ok(());
    };
    let travelled = projects::load_project(daemon, project)
        .map_err(|failure| failure.message)?
        .from_machine
        .is_some();
    if !travelled {
        return Ok(());
    }
    projects::take_travelled(daemon, project, settings, from)
        .map(|_| ())
        .map_err(|failure| failure.message)
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
