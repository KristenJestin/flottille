//! `forge review`: a review submitted on a pull request, with comments on lines of its diff
//! (`docs/tools.md`, "The forge"). The gate holds `approve` and `request_changes` for the user in
//! every mode (`gate::decides_a_review`); this module is what runs once it let the call through.

use std::io::Write;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Checkout, Forge, ForgeCall, NO_FORGE, after, bitbucket, chosen, declared_forge, gh_json,
    gh_text, place_checkouts, signed_in,
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

/// The repository `asked` names and its declared forge, once the checks every write makes
/// passed, and the refusal of what the forge cannot do whatever the pull request is.
async fn resolved(
    daemon: &Daemon,
    place: &Place,
    asked: &ForgeCall,
) -> Result<(Checkout, Forge), Outcome> {
    let checkout = place_checkouts(daemon, place)
        .and_then(|all| chosen(all, asked.repository.as_deref(), "review", true))
        .map(|mut checkouts| checkouts.remove(0))?;
    let forge = match declared_forge(&checkout).await {
        Ok(forge) => forge,
        Err(None) => {
            return Err(Outcome::Refused(format!(
                "refused: {}: {NO_FORGE}; nothing was posted: ask the user to set it",
                checkout.path
            )));
        }
        Err(Some(why)) => return Err(Outcome::Refused(why)),
    };
    // `bkt` has no request-changes command, and Data Center's API wants the user's own slug,
    // which Flottille never asks for.
    if asked.verdict.as_deref() == Some("request_changes")
        && matches!(forge, Forge::Bitbucket { .. })
        && !bitbucket::cloud(&forge)
    {
        return Err(Outcome::Refused(format!(
            "refused: {} is on Bitbucket Data Center, where bkt cannot request changes: nothing \
             was posted; comment on it, or leave the verdict to the user",
            forge.slug()
        )));
    }
    if matches!(forge, Forge::GitHub { .. })
        && let Err(problem) = signed_in().await
    {
        return Err(Outcome::Failed(problem));
    }
    Ok((checkout, forge))
}

/// The state of the pull request and the commit its branch is at, read now. An error when either
/// cannot be read: an approval is never given on a pull request nobody could look at.
async fn page(forge: &Forge, number: u64) -> Result<(&'static str, String), String> {
    let read = match forge {
        Forge::GitHub { .. } => {
            let (repo, numbered) = (forge.gh_repository(), number.to_string());
            let fields = "state,headRefOid";
            gh_json(&["pr", "view", &numbered, "-R", &repo, "--json", fields])
                .await
                .map(|viewed| {
                    let state = match viewed["state"].as_str() {
                        Some("OPEN") => Some("open"),
                        Some("MERGED") => Some("merged"),
                        Some("CLOSED") => Some("closed"),
                        _ => None,
                    };
                    let head = viewed["headRefOid"].as_str().filter(|h| !h.is_empty());
                    (state, head.map(str::to_owned))
                })
        }
        Forge::Bitbucket { .. } => bitbucket::page_now(forge, number)
            .await
            .map(|(state, head)| (Some(state), head)),
    };
    match read {
        Ok((Some(state), Some(head))) => Ok((state, head)),
        Ok(_) => Err("its state or its head is not there".to_owned()),
        Err(why) => Err(why),
    }
    .map_err(|why| {
        format!(
            "refused: the state and the head of the pull request #{number} of {} could not be \
             read ({why}); nothing was posted",
            forge.slug()
        )
    })
}

/// The commit the pull request's branch is at, for the card of an approval or a change request:
/// the card says it, and the post is refused when it moved since.
pub(crate) async fn head_of(
    daemon: &Daemon,
    place: &Place,
    asked: &ForgeCall,
) -> Result<String, String> {
    let said = |outcome: Outcome| match outcome {
        Outcome::Done(text)
        | Outcome::Refused(text)
        | Outcome::Invalid(text)
        | Outcome::Failed(text) => text,
        Outcome::Held(text) => text,
    };
    let (_, forge) = resolved(daemon, place, asked).await.map_err(said)?;
    page(&forge, asked.number.unwrap_or_default())
        .await
        .map(|(_, head)| head)
}

/// `forge review` of a pull request of a repository of the place, by its number.
pub(super) async fn review(daemon: &Daemon, place: &Place, asked: ForgeCall) -> Outcome {
    if let Some(why) = asked.malformed() {
        return Outcome::Refused(why);
    }
    let number = asked.number.unwrap_or_default();
    let verdict = asked.verdict.as_deref().unwrap_or_default();
    let body = asked.body.as_deref().filter(|b| !b.trim().is_empty());
    let (checkout, forge) = match resolved(daemon, place, &asked).await {
        Ok(found) => found,
        Err(refusal) => return refusal,
    };
    let slug = forge.slug();
    // An approval or a change request is for a pull request that is still open, and goes on the
    // head the user read on the card: the user answered later, and the forge's own refusal says
    // too little.
    let mut pinned = None;
    if verdict != "comment" {
        match page(&forge, number).await {
            Ok(("open", head)) => pinned = Some(head),
            Ok((state, _)) => {
                return Outcome::Refused(format!(
                    "refused: the pull request #{number} of {slug} is {state}, not open; nothing \
                     was posted"
                ));
            }
            Err(why) => return Outcome::Refused(why),
        }
    }
    let inline = asked.comments.as_deref().unwrap_or_default();
    let mut posted = Vec::new();
    let done = match &forge {
        Forge::GitHub { .. } => {
            let sent = github(&forge, number, verdict, body, inline, pinned).await;
            if sent.is_ok() {
                posted.extend(
                    body.into_iter()
                        .chain(inline.iter().map(|i| i.body.as_str())),
                );
            }
            sent
        }
        Forge::Bitbucket { .. } => {
            bitbucket_review(&forge, number, verdict, body, inline, &mut posted).await
        }
    };
    // What the forge took is the agent's, even when the review stopped half way.
    let key = number.to_string();
    let on = (
        place.project.as_str(),
        forge.kind(),
        checkout.path.as_str(),
        key.as_str(),
    );
    for text in &posted {
        crate::posts::record_comment(daemon, on, None, text);
    }
    if let Err(why) = done {
        return Outcome::Failed(why);
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

/// GitHub's review API: the verdict, the body and the comments in one call, so that a review is
/// posted whole or not at all. An approval or a change request is given for the commit the user
/// read (`commit_id`), so that it never lands on a later one. The JSON goes through a file `gh`
/// reads, which is gone once it is done.
async fn github(
    forge: &Forge,
    number: u64,
    verdict: &str,
    body: Option<&str>,
    inline: &[Inline],
    head: Option<String>,
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
    let mut review =
        json!({"event": event, "body": body.unwrap_or_default(), "comments": comments});
    if let Some(head) = head {
        review["commit_id"] = Value::from(head);
    }
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
/// comes last, so that a comment that fails leaves the pull request without it. What the forge
/// took is pushed to `posted`, and a failure says how many comments that is.
async fn bitbucket_review<'a>(
    forge: &Forge,
    number: u64,
    verdict: &str,
    body: Option<&'a str>,
    inline: &'a [Inline],
    posted: &mut Vec<&'a str>,
) -> Result<(), String> {
    let context = bitbucket::Contexts::default().of(forge).await?;
    let id = number.to_string();
    // What the forge already took, as a failure says it.
    let done = |posted: &[&str]| -> Vec<String> {
        let mut done = Vec::new();
        // The body goes first: it is posted once anything is.
        if body.is_some() && !posted.is_empty() {
            done.push("the review's comment".to_owned());
        }
        let lines = posted.len() - done.len();
        if lines > 0 {
            done.push(format!("{lines} of {} line comments", inline.len()));
        }
        done
    };
    let failed = |why: String, posted: &[&str]| {
        let done = done(posted);
        after(why, &done.iter().map(String::as_str).collect::<Vec<_>>())
    };
    if let Some(body) = body {
        let flags = ["--text", body];
        let args = bitbucket::command(forge, &context, &["pr", "comment", &id], &flags);
        bitbucket::text(&args).await?;
        posted.push(body);
    }
    for i in inline {
        let line = i.line.to_string();
        let flags = ["--text", &i.body, "--file", &i.path, "--to-line", &line];
        let args = bitbucket::command(forge, &context, &["pr", "comment", &id], &flags);
        bitbucket::text(&args)
            .await
            .map_err(|why| failed(why, posted))?;
        posted.push(&i.body);
    }
    match verdict {
        "approve" => {
            let args = bitbucket::command(forge, &context, &["pr", "approve", &id], &[]);
            bitbucket::text(&args)
                .await
                .map_err(|why| failed(why, posted))?;
        }
        "request_changes" => {
            let path = format!(
                "{}/pullrequests/{id}/request-changes",
                bitbucket::cloud_repository(forge)
            );
            let args = ["api", &path, "--context", &context, "--method", "POST"];
            bitbucket::text(&args)
                .await
                .map_err(|why| failed(why, posted))?;
        }
        _ => {}
    }
    Ok(())
}
