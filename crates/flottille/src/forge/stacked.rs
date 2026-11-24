//! The base of a stacked workspace (#1323, #1245; `docs/forges.md`, "A workspace's base"): a
//! workspace started from another ticket's branch, once that branch is merged and deleted on the
//! forge, goes where it went. Only what the forge says counts: the branch is gone from it, and a
//! merged pull request of that branch names where it went.

use super::{Forge, bitbucket, branch_name, declared, gh_json, gh_text};

/// Whether the forge has the branch: the error it answered with when it could not say. A Data
/// Center repository cannot be asked (`bkt` has no such call), and has every branch.
pub(super) async fn has_branch(forge: &Forge, branch: &str) -> Result<bool, String> {
    let found = match forge {
        Forge::GitHub { .. } => {
            let path = format!("repos/{}/branches/{}", forge.slug(), encoded(branch));
            gh_text(&[
                "api",
                &path,
                "--hostname",
                declared::GITHUB,
                "--jq",
                ".name",
            ])
            .await
            .map(|_| ())
        }
        Forge::Bitbucket { .. } if !bitbucket::cloud(forge) => return Ok(true),
        Forge::Bitbucket { .. } => bitbucket::branch_exists(forge, &encoded(branch)).await,
    };
    match found {
        Ok(()) => Ok(true),
        Err(why) if why.contains("404") || why.contains("Not Found") => Ok(false),
        Err(why) => Err(why),
    }
}

/// The base of the merged pull request of `branch`, from the repository itself.
async fn merged_into(forge: &Forge, branch: &str) -> Result<Option<String>, String> {
    match forge {
        Forge::GitHub { .. } => {
            let repo = forge.gh_repository();
            let listed = gh_json(&[
                "pr",
                "list",
                "-R",
                &repo,
                "--head",
                branch,
                "--state",
                "merged",
                "--json",
                "number,baseRefName,isCrossRepository",
                "--limit",
                "5",
            ])
            .await?;
            Ok(listed
                .as_array()
                .into_iter()
                .flatten()
                .find(|pull| pull["isCrossRepository"] == false)
                .and_then(|pull| pull["baseRefName"].as_str())
                .map(str::to_owned))
        }
        Forge::Bitbucket { .. } => bitbucket::merged_into(forge, branch).await,
    }
}

/// A branch name as a piece of a path: `%` is the only character a branch may have that a path
/// reads as its own (`branch_name` refuses `?` and `#`).
fn encoded(branch: &str) -> String {
    branch.replace('%', "%25")
}

/// Where a stacked base that is gone from the forge went, by its merged pull request: the base
/// that pull request went into, once the forge confirms it has that branch (a branch name used
/// again can have an old pull request into a base that is gone too).
pub(super) async fn successor(forge: &Forge, base: &str) -> Option<String> {
    let to = merged_into(forge, base)
        .await
        .ok()
        .flatten()
        .filter(|to| branch_name(to) && to != base)?;
    (has_branch(forge, &to).await == Ok(true)).then_some(to)
}

/// Where a workspace's base went, and how it is known.
pub(super) struct Moved {
    pub to: String,
    /// Said by a merged pull request of the base; else a guess from the repository's base rule.
    pub merged: bool,
}

/// Where the workspace's `base` went, when it is gone from the forge: the base of its merged pull
/// request, else the first branch of the repository's own base rule the forge confirms it has.
/// None while the base is there, when it is one of the rule's (the project's own, not a stacked
/// one), or when the forge could not say: an answer that is not a clear "not found" for the base
/// and a clear "found" for where it went moves nothing.
pub(super) async fn moved(forge: &Forge, base: &str, rule: &[String]) -> Option<Moved> {
    if rule.iter().any(|branch| branch == base) || has_branch(forge, base).await != Ok(false) {
        return None;
    }
    if let Some(to) = successor(forge, base).await {
        return Some(Moved { to, merged: true });
    }
    for branch in rule {
        if has_branch(forge, branch).await == Ok(true) {
            return Some(Moved {
                to: branch.clone(),
                merged: false,
            });
        }
    }
    None
}
