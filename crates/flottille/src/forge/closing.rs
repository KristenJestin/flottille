//! The comment a merged or closed pull request was closed with, for `forge show`
//! (`docs/forges.md`, "The closing comment"). GitHub gives neither a closing comment nor, for a
//! closed pull request, who closed it, so the closer is read from the issue events; Bitbucket's is
//! found among what its detail already read (`bitbucket::detailed_with`).

use flottille_core::declared;

use super::{Comment, Forge, Pull, gh_json, masked};

/// The last comment `closer` wrote at or before `until`, among `comments` (oldest first) that say
/// their time. None when they wrote none, or the forge gives no time.
pub(super) fn last_before(comments: &[Comment], closer: &str, until: &str) -> Option<Comment> {
    comments
        .iter()
        .rev()
        .find(|c| {
            c.author == closer
                && !c.body.is_empty()
                && c.at.as_deref().is_some_and(|at| at <= until)
        })
        .cloned()
}

/// Tells `pull`, when it is a merged or closed one of GitHub, the comment it was closed with: the
/// last its closer wrote before closing it. Best effort: when the events cannot be read, nothing
/// is said of a closing comment and the rest of the pull request is shown as usual.
pub(super) async fn attach(forge: &Forge, pull: &mut Pull) {
    let Forge::GitHub { owner, name } = forge else {
        return;
    };
    if !matches!(pull.state, "merged" | "closed") {
        return;
    }
    let path = format!(
        "repos/{owner}/{name}/issues/{}/events?per_page=100",
        pull.number
    );
    let args = [
        "api",
        &path,
        "--hostname",
        declared::GITHUB,
        "--paginate",
        "--slurp",
    ];
    let Ok(pages) = gh_json(&args).await else {
        return;
    };
    // The newest of the events that end a pull request, a reopened one having several.
    let ended = pages
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|page| page.as_array().into_iter().flatten())
        .rfind(|event| matches!(event["event"].as_str(), Some("merged" | "closed")));
    let Some(ended) = ended else {
        return;
    };
    let (Some(closer), Some(at)) = (
        ended["actor"]["login"].as_str(),
        ended["created_at"].as_str(),
    ) else {
        return;
    };
    pull.closing = last_before(&pull.comments, &masked(closer), at);
}
