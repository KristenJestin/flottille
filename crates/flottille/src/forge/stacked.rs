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
            let path = format!("repos/{}/branches/{branch}", forge.slug());
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
        Forge::Bitbucket { .. } => bitbucket::branch_exists(forge, branch).await,
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

/// Where the workspace's `base` went, when it is gone from the forge: the base of its merged pull
/// request, else the first branch of the repository's own base rule the forge has. None while the
/// base is there, when it is one of the rule's (the project's own, not a stacked one), or when
/// the forge could not say.
pub(super) async fn moved(forge: &Forge, base: &str, rule: &[String]) -> Option<String> {
    if rule.iter().any(|branch| branch == base) || has_branch(forge, base).await != Ok(false) {
        return None;
    }
    let merged = merged_into(forge, base).await.ok().flatten();
    let to = match merged.filter(|to| branch_name(to)) {
        Some(to) => to,
        None => {
            let mut found = None;
            for branch in rule {
                if has_branch(forge, branch).await == Ok(true) {
                    found = Some(branch.clone());
                    break;
                }
            }
            found.or_else(|| rule.first().cloned())?
        }
    };
    (to != base).then_some(to)
}
