//! `forge review`: a review submitted on a pull request, with comments on lines of its diff
//! (`docs/tools.md`, "The forge"). The gate holds `approve` and `request_changes` for the user in
//! every mode (`gate::decides_a_review`); this module is what runs once it let the call through.

use std::io::Write;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Forge, ForgeCall, NO_FORGE, after, bitbucket, chosen, declared_forge, gh_text, place_checkouts,
    seen_of, signed_in,
};
use crate::daemon::Daemon;
use crate::mcp::Outcome;
use crate::place::Place;

/// The verdicts of a review, as `forge review` names them.
pub const VERDICTS: [&str; 3] = ["comment", "approve", "request_changes"];

/// A comment on a line of the pull request's diff: its new side, as the forges' own commands
/// take it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inline {
    path: String,
    line: u64,
    body: String,
}

/// Why a review cannot be done whatever the forge says, before the user is asked.
pub(super) fn malformed(asked: &ForgeCall) -> Option<String> {
    if asked.number.is_none() {
        return Some(
            "refused: forge review needs number, the pull request's: forge list lists them".into(),
        );
    }
    let verdict = asked.verdict.as_deref().unwrap_or_default();
    if !VERDICTS.contains(&verdict) {
        return Some(format!(
            "refused: forge review needs verdict, one of {}",
            VERDICTS.join(", ")
        ));
    }
    let said = |text: &str| !text.trim().is_empty();
    if verdict != "approve" && !asked.body.as_deref().is_some_and(said) {
        return Some(format!(
            "refused: forge review {verdict} needs body, what the review says"
        ));
    }
    for inline in asked.comments.iter().flatten() {
        // The path is one value of `bkt pr comment --file`, never an option of its own.
        if !said(&inline.path)
            || inline.path.starts_with('-')
            || inline.path.contains(char::is_control)
        {
            return Some(format!(
                "refused: comments: {:?} is not a path of the diff, not starting with -",
                inline.path
            ));
        }
        if inline.line == 0 {
            return Some(format!(
                "refused: comments: {} needs line, the new file's, from 1",
                inline.path
            ));
        }
        if !said(&inline.body) {
            return Some(format!(
                "refused: comments: the comment on {} line {} needs a body",
                inline.path, inline.line
            ));
        }
    }
    None
}

/// `forge review` of a pull request of a repository of the place, by its number.
pub(super) async fn review(daemon: &Daemon, place: &Place, asked: ForgeCall) -> Outcome {
    if let Some(why) = asked.malformed() {
        return Outcome::Refused(why);
    }
    let number = asked.number.unwrap_or_default();
    let verdict = asked.verdict.as_deref().unwrap_or_default();
    let body = asked.body.as_deref().filter(|b| !b.trim().is_empty());
    let checkout = match place_checkouts(daemon, place)
        .and_then(|all| chosen(all, asked.repository.as_deref(), "review", true))
    {
        Ok(mut checkouts) => checkouts.remove(0),
        Err(refusal) => return refusal,
    };
    let forge = match declared_forge(&checkout).await {
        Ok(forge) => forge,
        Err(None) => {
            return Outcome::Refused(format!(
                "refused: {}: {NO_FORGE}; nothing was posted: ask the user to set it",
                checkout.path
            ));
        }
        Err(Some(why)) => return Outcome::Refused(why),
    };
    let slug = forge.slug();
    if verdict == "request_changes" && !bitbucket_cloud_or_github(&forge) {
        return Outcome::Refused(format!(
            "refused: {slug} is on Bitbucket Data Center, where bkt cannot request changes: \
             nothing was posted; comment on it, or leave the verdict to the user"
        ));
    }
    if matches!(forge, Forge::GitHub { .. })
        && let Err(problem) = signed_in().await
    {
        return Outcome::Failed(problem);
    }
    // An approval or a change request is for a pull request that is still open: the user answered
    // later, and the forge's own refusal says too little.
    if verdict != "comment" {
        let state = match &forge {
            Forge::GitHub { .. } => match seen_of(&forge, number).await {
                Ok(seen) => Some(seen.state),
                Err(refusal) => return refusal,
            },
            Forge::Bitbucket { .. } => bitbucket::state_now(&forge, number).await,
        };
        if let Some(state) = state.filter(|state| *state != "open") {
            return Outcome::Refused(format!(
                "refused: the pull request #{number} of {slug} is {state}, not open; nothing was \
                 posted"
            ));
        }
    }
    let inline = asked.comments.as_deref().unwrap_or_default();
    let done = match &forge {
        Forge::GitHub { .. } => github(&forge, number, verdict, body, inline).await,
        Forge::Bitbucket { .. } => bitbucket_review(&forge, number, verdict, body, inline).await,
    };
    if let Err(why) = done {
        return Outcome::Failed(why);
    }
    let key = number.to_string();
    let on = (
        place.project.as_str(),
        forge.kind(),
        checkout.path.as_str(),
        key.as_str(),
    );
    for text in body
        .into_iter()
        .chain(inline.iter().map(|i| i.body.as_str()))
    {
        crate::posts::record_comment(daemon, on, None, text);
    }
    let what = match verdict {
        "approve" => "The approval",
        "request_changes" => "The change request",
        _ => "The review",
    };
    let with = match inline.len() {
        0 => String::new(),
        1 => ", with 1 comment on a line,".to_owned(),
        n => format!(", with {n} comments on lines,"),
    };
    Outcome::Done(format!(
        "{what}{with} was posted on pull request #{number} of {slug}."
    ))
}

/// Whether the forge takes a change request from `bkt` or `gh`: Data Center does not (`bkt` has no
/// such command, and its API wants the user's own slug, which Flottille never asks for).
fn bitbucket_cloud_or_github(forge: &Forge) -> bool {
    !matches!(forge, Forge::Bitbucket { .. }) || bitbucket::cloud(forge)
}

/// GitHub's review API: the verdict, the body and the comments in one call, so that a review is
/// posted whole or not at all. The JSON goes through a file `gh` reads, which is gone once it is
/// done.
async fn github(
    forge: &Forge,
    number: u64,
    verdict: &str,
    body: Option<&str>,
    inline: &[Inline],
) -> Result<(), String> {
    let event = match verdict {
        "approve" => "APPROVE",
        "request_changes" => "REQUEST_CHANGES",
        _ => "COMMENT",
    };
    let comments: Vec<Value> = inline
        .iter()
        .map(|i| json!({"path": i.path, "line": i.line, "side": "RIGHT", "body": i.body}))
        .collect();
    let review = json!({"event": event, "body": body.unwrap_or_default(), "comments": comments});
    let mut input = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    input
        .write_all(review.to_string().as_bytes())
        .map_err(|error| error.to_string())?;
    let path = format!("repos/{}/pulls/{number}/reviews", forge.slug());
    let file = input.path().to_string_lossy().into_owned();
    gh_text(&[
        "api",
        &path,
        "--hostname",
        flottille_core::declared::GITHUB,
        "--method",
        "POST",
        "--input",
        &file,
    ])
    .await
    .map(|_| ())
}

/// `bkt`'s comments, then its verdict: the body, each comment on a line (`bkt pr comment --file
/// --to-line`), then `bkt pr approve` or, on Cloud, the API's `request-changes`. The verdict
/// comes last, so that a comment that fails leaves the pull request without it, and a failure
/// says what was already posted.
async fn bitbucket_review(
    forge: &Forge,
    number: u64,
    verdict: &str,
    body: Option<&str>,
    inline: &[Inline],
) -> Result<(), String> {
    let context = bitbucket::Contexts::default().of(forge).await?;
    let id = number.to_string();
    let mut done = Vec::new();
    if let Some(body) = body {
        let flags = ["--text", body];
        bitbucket::text(&bitbucket::command(
            forge,
            &context,
            &["pr", "comment", &id],
            &flags,
        ))
        .await?;
        done.push("the review's comment");
    }
    for i in inline {
        let line = i.line.to_string();
        let flags = ["--text", &i.body, "--file", &i.path, "--to-line", &line];
        let args = bitbucket::command(forge, &context, &["pr", "comment", &id], &flags);
        bitbucket::text(&args)
            .await
            .map_err(|why| after(why, &done))?;
        if !done.contains(&"a comment on a line") {
            done.push("a comment on a line");
        }
    }
    match verdict {
        "approve" => {
            let args = bitbucket::command(forge, &context, &["pr", "approve", &id], &[]);
            bitbucket::text(&args)
                .await
                .map_err(|why| after(why, &done))?;
        }
        "request_changes" => {
            let path = format!(
                "{}/pullrequests/{id}/request-changes",
                bitbucket::cloud_repository(forge)
            );
            let args = ["api", &path, "--context", &context, "--method", "POST"];
            bitbucket::text(&args)
                .await
                .map_err(|why| after(why, &done))?;
        }
        _ => {}
    }
    Ok(())
}
