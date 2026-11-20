//! The review of a pull request (#1091): the pull request's head, fetched by the daemon's own
//! git into a detached worktree of the project's repository, which Flottille alone makes and
//! removes (`docs/extensions.md`, "Review"). The review reads it as it reads a workspace's change;
//! nothing is posted on the forge.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use flottille_core::paths::ensure_private_dir;
use flottille_core::project::Project;
use flottille_core::store::now_ms;
use serde::{Deserialize, Serialize};

use super::{Diff, Snapshot, git_line, git_lines, snapshot};
use crate::daemon::Daemon;
use crate::git::{self, Class};
use crate::projects::Failure;

/// The pull request a review reads, as its row keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct PullRequest {
    /// The repository of the project it is on, by its path.
    pub repository: String,
    /// The forge's name for it (`acme/app`).
    pub slug: String,
    pub number: u64,
    /// The commit its head was at when the review started.
    pub head: String,
    /// The repository's own folder, whose worktree the place is.
    pub folder: String,
    /// The place: the detached worktree at its head.
    pub place: String,
}

/// What a pull request's review starts from.
pub(super) struct Prepared {
    pub pull: PullRequest,
    pub diff: Diff,
    /// The files the review reads, from the top of the place.
    pub files: Vec<String>,
    pub before: Snapshot,
}

static MADE: AtomicU64 = AtomicU64::new(0);

/// Where the refs of a fetch are, in the user's repository, until the place holds the head.
const REFS: &str = "refs/flottille/review/";

fn not_in(number: u64, from: &str, commits: &[String]) -> Failure {
    Failure::refused(format!(
        "{from} is not a commit of pull request #{number}: it holds {} commit{} after its base",
        commits.len(),
        if commits.len() == 1 { "" } else { "s" }
    ))
}

async fn commit_of(folder: &Path, name: &str) -> Option<String> {
    git_line(
        folder,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{name}^{{commit}}"),
        ],
    )
    .await
}

/// Reads pull request `number` on its forge, fetches its head and its base branch, makes its
/// place, and says what the review reads: from `from` (a commit of the pull request) or from the
/// merge base. Refused, nothing left behind, for a pull request the forge does not have, a
/// repository with no forge, a `from` that is not one of its commits, or an empty diff.
pub(super) async fn prepare(
    daemon: &Daemon,
    project: &Project,
    number: u64,
    repository: Option<&str>,
    from: Option<&str>,
) -> Result<Prepared, Failure> {
    let target = crate::forge::pull_to_review(project, repository, number)
        .await
        .map_err(Failure::refused)?;
    let name = format!(
        "pr{number}-{}-{}",
        now_ms(),
        MADE.fetch_add(1, Ordering::Relaxed)
    );
    let place = daemon.paths.reviews().join(&name);
    ensure_private_dir(&daemon.paths.reviews())
        .map_err(|error| Failure::refused(error.to_string()))?;
    let head_name = format!("{REFS}{name}-head");
    let base_name = format!("{REFS}{name}-base");
    let result = fetched(&target, number, from, &place, &head_name, &base_name).await;
    // The refs only carried the fetch: the place holds the head from here on.
    for name in [&head_name, &base_name] {
        let _ = git::output(
            &target.folder,
            &["update-ref", "-d", "--end-of-options", name],
            Class::Work,
        )
        .await;
    }
    match result {
        Ok(prepared) => Ok(prepared),
        Err(failure) => {
            remove(&target.folder, &place).await;
            Err(failure)
        }
    }
}

async fn fetched(
    target: &crate::forge::ToReview,
    number: u64,
    from: Option<&str>,
    place: &Path,
    head_name: &str,
    base_name: &str,
) -> Result<Prepared, Failure> {
    let head_spec = format!("+{}:{head_name}", target.head_ref);
    let base_spec = format!("+refs/heads/{}:{base_name}", target.base);
    let fetch = [
        "--quiet",
        "--no-tags",
        "--end-of-options",
        &target.remote,
        &head_spec,
        &base_spec,
    ];
    git::output_remote(&target.folder, &target.remote, "fetch", &fetch, Class::Read)
        .await
        .map_err(|reason| {
            Failure::refused(format!(
                "pull request #{number} of {} could not be fetched from {} ({}): nothing was \
                 started",
                target.slug,
                target.remote,
                reason.split_whitespace().collect::<Vec<_>>().join(" ")
            ))
        })?;
    let (Some(head), Some(base)) = (
        commit_of(&target.folder, head_name).await,
        commit_of(&target.folder, base_name).await,
    ) else {
        return Err(Failure::refused(format!(
            "pull request #{number} of {} was fetched, and its head or its base branch {} is no \
             commit",
            target.slug, target.base
        )));
    };
    let place_text = place.to_string_lossy().into_owned();
    let filters_off = filters_off(&target.folder).await;
    let env = as_env(&filters_off);
    git::output_with(
        &target.folder,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            "--end-of-options",
            &place_text,
            &head,
        ],
        Class::Work,
        &env,
    )
    .await
    .map_err(|reason| {
        Failure::refused(format!(
            "the place of the review of pull request #{number} could not be made: {reason}"
        ))
    })?;
    let merge_base = git_line(place, &["merge-base", &base, &head])
        .await
        .ok_or_else(|| {
            Failure::refused(format!(
                "pull request #{number} shares no history with {}: there is no merge base",
                target.base
            ))
        })?;
    let commits = git_lines(place, &["rev-list", &format!("{merge_base}..{head}")]).await;
    let start = match from {
        None => merge_base.clone(),
        Some(from) => {
            // Never an option of git: a commit is named by its hash or a ref, never by a dash.
            if from.starts_with('-') {
                return Err(not_in(number, from, &commits));
            }
            let Some(commit) = commit_of(place, from).await else {
                return Err(not_in(number, from, &commits));
            };
            if !commits.contains(&commit) {
                return Err(not_in(number, from, &commits));
            }
            commit
        }
    };
    // The place is the repository's copy, whatever its path in the project: its files are said
    // from its top, which is where the review's agents work.
    // Commit to commit: the working tree, just written, is racily clean, and comparing it would
    // run the user's clean filters on the pull request's files.
    let files: Vec<String> = git::output_with(
        place,
        &["diff", "--name-only", &start, "HEAD"],
        Class::Read,
        &env,
    )
    .await
    .map(|out| out.lines().map(str::to_owned).collect())
    .unwrap_or_default();
    if files.is_empty() {
        return Err(Failure::refused(match from {
            Some(from) => {
                format!("nothing to review: pull request #{number} changes nothing after {from}")
            }
            None => format!(
                "nothing to review: pull request #{number} has no change from its base ({})",
                target.base
            ),
        }));
    }
    let before = snapshot(".", place, &env).await;
    Ok(Prepared {
        pull: PullRequest {
            repository: target.path.clone(),
            slug: target.slug.clone(),
            number,
            head,
            folder: target.folder.display().to_string(),
            place: place_text,
        },
        diff: Diff {
            path: ".".to_owned(),
            folder: place.display().to_string(),
            base: target.base.clone(),
            from: start,
            rules_from: merge_base,
        },
        files,
        before,
    })
}

/// The environment of a checkout of a pull request that runs none of the user's filters on its
/// files: the content is a stranger's, and Large File Storage would download from the server the
/// pull request's own `.lfsconfig` names. Every driver the repository's configuration names, and
/// Large File Storage's, is given no program and is not required: its files are written as the
/// commit holds them.
pub(super) async fn filters_off(folder: &Path) -> Vec<(String, String)> {
    let named = git::output(
        folder,
        &["config", "--name-only", "--get-regexp", r"^filter\."],
        Class::Read,
    )
    .await
    .unwrap_or_default();
    let mut drivers: Vec<&str> = named
        .lines()
        .filter_map(|key| key.strip_prefix("filter.")?.rsplit_once('.'))
        .map(|(driver, _)| driver)
        .chain(["lfs"])
        .collect();
    drivers.sort_unstable();
    drivers.dedup();
    // A count of ours would drop the `GIT_CONFIG_KEY_n` the daemon was started with.
    let inherited = std::env::var("GIT_CONFIG_COUNT")
        .ok()
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0);
    filter_env(&drivers, inherited)
}

/// What `git::output_with` takes of an environment `filters_off` made.
pub(super) fn as_env(env: &[(String, String)]) -> Vec<(&str, &std::ffi::OsStr)> {
    env.iter()
        .map(|(name, value)| (name.as_str(), std::ffi::OsStr::new(value)))
        .collect()
}

/// The environment `filters_off` gives git for `drivers`: `inherited` is the number of
/// `GIT_CONFIG_KEY_n` the daemon's own environment already holds, which a count of ours would drop.
fn filter_env(drivers: &[&str], inherited: usize) -> Vec<(String, String)> {
    let mut env = vec![("GIT_LFS_SKIP_SMUDGE".to_owned(), "1".to_owned())];
    let mut count = inherited;
    for driver in drivers {
        for (key, value) in [
            ("smudge", ""),
            ("process", ""),
            ("required", "false"),
            ("clean", ""),
        ] {
            env.push((
                format!("GIT_CONFIG_KEY_{count}"),
                format!("filter.{driver}.{key}"),
            ));
            env.push((format!("GIT_CONFIG_VALUE_{count}"), value.to_owned()));
            count += 1;
        }
    }
    env.push(("GIT_CONFIG_COUNT".to_owned(), count.to_string()));
    env
}

/// Removes a place, and what the repository holds of it: the worktree, then the folder if it is
/// still there. Nothing is reported: a place that cannot be removed now is removed by the sweep
/// of the next start. Never `git worktree prune`, which would drop every worktree of the user's
/// whose folder is away (a drive not mounted): when git could not remove the place, its own
/// registration alone goes.
pub(super) async fn remove(folder: &Path, place: &Path) {
    let registration = registration_of(place);
    let place_text = place.to_string_lossy().into_owned();
    let removed = git::output(
        folder,
        &[
            "worktree",
            "remove",
            "--force",
            "--end-of-options",
            &place_text,
        ],
        Class::Work,
    )
    .await
    .is_ok();
    let gone = place.to_owned();
    let _ = tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(gone);
        if !removed && let Some(registration) = registration {
            let _ = std::fs::remove_dir_all(registration);
        }
    })
    .await;
}

/// The git directory a place's `.git` file leads to: `<repository>/.git/worktrees/<name>`.
fn git_dir_of(place: &Path) -> Option<PathBuf> {
    let link = std::fs::read_to_string(place.join(".git")).ok()?;
    Some(PathBuf::from(link.trim().strip_prefix("gitdir:")?.trim()))
}

/// The repository's record of the place as one of its worktrees, when it is that place's and
/// nothing else: a folder of the repository's `worktrees` whose `gitdir` leads back to it.
fn registration_of(place: &Path) -> Option<PathBuf> {
    let registration = git_dir_of(place)?;
    if registration.parent()?.file_name()? != "worktrees" {
        return None;
    }
    let back = std::fs::read_to_string(registration.join("gitdir")).ok()?;
    (Path::new(back.trim()) == place.join(".git")).then_some(registration)
}

/// The repository a place is a worktree of, read from the place's own `.git` file.
fn repository_of(place: &Path) -> Option<PathBuf> {
    // `<repository>/.git/worktrees/<name>`: the git directory, which git takes for a folder.
    Some(git_dir_of(place)?.parent()?.parent()?.to_owned())
}

/// Removes every place no review of a running pilot holds: those a daemon that died left, and one
/// whose review ended while the removal failed. `keep` are the places of the reviews running.
pub(super) async fn sweep(daemon: &Daemon, keep: &[String]) {
    let Ok(entries) = std::fs::read_dir(daemon.paths.reviews()) else {
        return;
    };
    for entry in entries.flatten() {
        let place = entry.path();
        if keep.contains(&place.to_string_lossy().into_owned()) {
            continue;
        }
        match repository_of(&place) {
            Some(repository) => remove(&repository, &place).await,
            None => {
                let _ = std::fs::remove_dir_all(&place);
            }
        }
    }
}

/// Deletes the refs of a fetch a daemon that died left in the projects' repositories. They carry a
/// fetch to its place only, and no review is fetching at the daemon's start: none is held.
pub(super) async fn sweep_refs(daemon: &Daemon) {
    // A review makes its folder before it fetches: where none was ever made, no fetch of one left
    // a ref, and the user's repositories are not read for nothing at every start.
    if !daemon.paths.reviews().is_dir() {
        return;
    }
    for project in crate::projects::all(daemon) {
        for repository in &project.repositories {
            let folder = match repository.path.as_str() {
                "." => project.folder.clone(),
                path => project.folder.join(path),
            };
            if !folder.is_dir() {
                continue;
            }
            let Ok(refs) = git::output(
                &folder,
                &["for-each-ref", "--format=%(refname)", REFS],
                Class::Read,
            )
            .await
            else {
                continue;
            };
            for name in refs.lines().filter(|name| name.starts_with(REFS)) {
                let _ = git::output(
                    &folder,
                    &["update-ref", "-d", "--end-of-options", name],
                    Class::Work,
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn the_filters_off_numbering_starts_after_the_configuration_the_daemon_inherited() {
        let env = filter_env(&["lfs", "mark"], 2);

        // The daemon's own KEY_0 and KEY_1 stay: ours begin at 2, and the count covers both.
        assert_eq!(value(&env, "GIT_CONFIG_KEY_0"), None);
        assert_eq!(value(&env, "GIT_CONFIG_KEY_1"), None);
        assert_eq!(value(&env, "GIT_CONFIG_KEY_2"), Some("filter.lfs.smudge"));
        assert_eq!(value(&env, "GIT_CONFIG_COUNT"), Some("10"));
        assert_eq!(value(&env, "GIT_CONFIG_KEY_9"), Some("filter.mark.clean"));
    }

    #[test]
    fn the_filters_off_numbering_starts_at_zero_with_nothing_inherited() {
        let env = filter_env(&["lfs"], 0);

        assert_eq!(value(&env, "GIT_CONFIG_KEY_0"), Some("filter.lfs.smudge"));
        assert_eq!(value(&env, "GIT_CONFIG_COUNT"), Some("4"));
    }
}
