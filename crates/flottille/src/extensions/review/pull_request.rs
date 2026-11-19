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
    let head_name = format!("refs/flottille/review/{name}-head");
    let base_name = format!("refs/flottille/review/{name}-base");
    let result = fetched(&target, number, from, &place, &head_name, &base_name).await;
    // The refs only carried the fetch: the place holds the head from here on.
    for name in [&head_name, &base_name] {
        let _ = git::output(
            &target.folder,
            &["update-ref", "-d", "--end-of-options", name],
            Class::Read,
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
    git::output(
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
    let files = git_lines(place, &["diff", "--name-only", &start]).await;
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
    let before = snapshot(".", place).await;
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
