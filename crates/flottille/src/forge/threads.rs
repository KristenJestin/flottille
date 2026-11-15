//! The review threads of a pull request: the comments on lines of its diff with their replies,
//! the file and line each is on, and whether it is resolved or outdated; and `forge reply` and
//! `forge resolve`, which answer in one and mark it resolved or not (`docs/tools.md`, "The
//! forge"; `docs/forges.md`, "Review threads").
//!
//! GitHub has review threads in its GraphQL API alone (`reviewThreads`,
//! `addPullRequestReviewThreadReply`, `resolveReviewThread`), which `gh api graphql` reaches: the
//! REST API has neither their resolution nor a reply that keeps them whole. Bitbucket's are the
//! inline comments it already gives with the others, a thread named by its first comment.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Comment, Forge, ForgeCall, NO_FORGE, bitbucket, chosen, declared_forge, gh_json, is_bot,
    masked, place_checkouts, signed_in,
};
use crate::daemon::Daemon;
use crate::mcp::Outcome;
use crate::place::Place;

/// The most threads one read of a GitHub pull request gives, and comments of each: GraphQL pages
/// by 100 at most.
const MOST: u64 = 100;

/// A review thread, as `forge show` says it with `detail: full`.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    /// What `reply` and `resolve` name it by: GitHub's node id, the number of Bitbucket's first
    /// comment.
    pub id: String,
    pub path: String,
    /// The line it is on (the last of a range), in the file of its side; none when the forge says
    /// none (a comment on a whole file).
    pub line: Option<u64>,
    /// The first line of a range, when it is one.
    pub start: Option<u64>,
    /// `new`, the file after the change, or `old`, before it.
    pub side: &'static str,
    /// Whether it is resolved; none where the forge has no such state for it.
    pub resolved: Option<bool>,
    /// The code it is on changed since it was written: its line is the one it was written on.
    pub outdated: bool,
    /// Its comments, the first one first.
    pub comments: Vec<Comment>,
    /// How many comments the forge says it has, where it says it and `comments` holds fewer.
    pub comments_total: Option<u64>,
}

impl Thread {
    /// `src/login.rs line 14, new side`, `README.md lines 2 to 4, old side`, `src/a.rs`.
    pub fn place(&self) -> String {
        let lines = match (self.start.filter(|s| Some(*s) != self.line), self.line) {
            (Some(start), Some(line)) => format!(" lines {start} to {line}, {} side", self.side),
            (_, Some(line)) => format!(" line {line}, {} side", self.side),
            (_, None) => String::new(),
        };
        format!("{}{lines}", self.path)
    }

    /// `unresolved`, `resolved, outdated`…
    pub fn state(&self) -> String {
        let mut said = match self.resolved {
            Some(true) => "resolved",
            Some(false) => "unresolved",
            None => "no resolved state on this server",
        }
        .to_owned();
        if self.outdated {
            said.push_str(", outdated: the code changed since, the line is where it was written");
        }
        said
    }
}

/// A thread as an agent names it: GitHub's node id is a text, Bitbucket's comment a number.
#[derive(Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum ThreadId {
    Text(String),
    Number(u64),
}

impl ThreadId {
    pub(crate) fn text(&self) -> String {
        match self {
            ThreadId::Text(text) => text.trim().to_owned(),
            ThreadId::Number(number) => number.to_string(),
        }
    }
}

/// Why a `reply` or a `resolve` cannot be done whatever the forge says, before the user is asked.
pub(super) fn malformed(asked: &ForgeCall) -> Option<String> {
    let action = asked.action.as_str();
    if asked.number.is_none() {
        return Some(format!(
            "refused: forge {action} needs number, the pull request's: forge list lists them"
        ));
    }
    let Some(thread) = asked.thread.as_ref().map(ThreadId::text) else {
        return Some(format!(
            "refused: forge {action} needs thread, as forge show with detail full gives it"
        ));
    };
    // GitHub's node ids and Bitbucket's numbers: a value of the call, never an option of it nor a
    // part of an address.
    let fit = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '=');
    if thread.is_empty()
        || thread.len() > 200
        || thread.starts_with('-')
        || !thread.chars().all(fit)
    {
        return Some(format!(
            "refused: thread: {thread:?} is not a review thread's id, as forge show with detail \
             full gives it"
        ));
    }
    match action {
        "reply" if !asked.body.as_deref().is_some_and(|b| !b.trim().is_empty()) => {
            Some("refused: forge reply needs body, the reply".into())
        }
        "resolve" if asked.resolved.is_none() => Some(
            "refused: forge resolve needs resolved: true to resolve the thread, false to open it \
             again"
                .into(),
        ),
        _ => None,
    }
}

/// Why `thread` is no thread of Bitbucket, whose are numbers (their first comment's): the same
/// words before the user is asked and after.
pub(crate) fn bitbucket_thread(thread: &Value) -> Option<String> {
    let thread = match thread {
        Value::String(text) => text.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    (!thread.chars().all(|c| c.is_ascii_digit())).then(|| {
        format!(
            "refused: thread: {thread:?} is not a thread of Bitbucket, the number of its first \
             comment as forge show with detail full gives it; nothing was written"
        )
    })
}

/// The review threads of a GitHub pull request, read now: the first `MOST`, each with its first
/// `MOST` comments, and whether there are more.
pub(super) async fn github(forge: &Forge, number: u64) -> Result<(Vec<Thread>, bool), String> {
    let Forge::GitHub { owner, name } = forge else {
        return Err("not a GitHub repository".into());
    };
    let query = format!(
        "query($owner: String!, $name: String!, $number: Int!) {{ repository(owner: $owner, \
         name: $name) {{ pullRequest(number: $number) {{ reviewThreads(first: {MOST}) {{ \
         totalCount nodes {{ id isResolved isOutdated path line startLine originalLine \
         originalStartLine diffSide comments(first: {MOST}) {{ totalCount nodes {{ author {{ login }} \
         body createdAt url }} }} }} }} }} }} }}"
    );
    let (owner, name, number) = (
        format!("owner={owner}"),
        format!("name={name}"),
        format!("number={number}"),
    );
    let said = gh_json(&[
        "api",
        "graphql",
        "--hostname",
        flottille_core::declared::GITHUB,
        "-f",
        &format!("query={query}"),
        "-f",
        &owner,
        "-f",
        &name,
        "-F",
        &number,
    ])
    .await?;
    let threads = &said["data"]["repository"]["pullRequest"]["reviewThreads"];
    let nodes = threads["nodes"]
        .as_array()
        .ok_or_else(|| "gh api graphql answered no review threads".to_owned())?;
    let more = threads["totalCount"].as_u64().unwrap_or(0) > nodes.len() as u64;
    Ok((nodes.iter().filter_map(github_thread).collect(), more))
}

/// One thread of GitHub by its id, whatever its place among the pull request's: `node(id:)` is one
/// call without a page, and says the pull request and repository the thread is on. None when it is
/// no thread, or one of another pull request or repository.
async fn github_thread_of(forge: &Forge, number: u64, id: &str) -> Result<Option<Thread>, String> {
    let query = "query($thread: ID!) { node(id: $thread) { ... on PullRequestReviewThread { id \
                 isResolved isOutdated path line startLine originalLine originalStartLine \
                 diffSide pullRequest { number repository { nameWithOwner } } } } }";
    let said = gh_json(&[
        "api",
        "graphql",
        "--hostname",
        flottille_core::declared::GITHUB,
        "-f",
        &format!("query={query}"),
        "-f",
        &format!("thread={id}"),
    ])
    .await;
    let said = match said {
        Ok(said) => said,
        Err(why) if super::not_found(&why) => return Ok(None),
        Err(why) => return Err(why),
    };
    let node = &said["data"]["node"];
    let on = &node["pullRequest"];
    let ours = on["number"].as_u64() == Some(number)
        && on["repository"]["nameWithOwner"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(&forge.slug()));
    Ok(ours.then_some(node).and_then(github_thread))
}

/// A thread of GitHub's GraphQL answer: an outdated one says no `line`, only the one it was
/// written on.
fn github_thread(node: &Value) -> Option<Thread> {
    let text = |value: &Value| masked(value.as_str().unwrap_or_default());
    let outdated = node["isOutdated"] == true;
    let (line, start) = match node["line"].as_u64() {
        Some(line) => (Some(line), node["startLine"].as_u64()),
        None => (
            node["originalLine"].as_u64(),
            node["originalStartLine"].as_u64(),
        ),
    };
    Some(Thread {
        id: node["id"].as_str()?.to_owned(),
        path: text(&node["path"]),
        line,
        start,
        side: match node["diffSide"].as_str() {
            Some("LEFT") => "old",
            _ => "new",
        },
        resolved: node["isResolved"].as_bool(),
        outdated,
        comments: node["comments"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|comment| Comment {
                author: text(&comment["author"]["login"]),
                handle: None,
                state: None,
                body: text(&comment["body"]),
                bot: is_bot(&comment["author"]),
                at: comment["createdAt"]
                    .as_str()
                    .and_then(crate::session_read::form),
                id: comment["url"].as_str().and_then(crate::posts::comment_id),
                recorded: false,
            })
            .collect(),
        comments_total: node["comments"]["totalCount"].as_u64(),
    })
}

/// The threads of a Bitbucket Cloud pull request's comments: each inline comment that answers
/// none, with every comment that answers it or one of its answers (`parent`), oldest first.
/// Cloud says a thread resolved by its first comment's `resolution`, outdated by `inline.outdated`.
pub(super) fn cloud(values: &[Value], comments: &[Comment]) -> Vec<Thread> {
    fn parent(value: &Value) -> Option<u64> {
        value["parent"]["id"].as_u64()
    }
    // The first comment of a reply's thread, followed up its parents; a loop or a missing one
    // ends it.
    fn root_of(values: &[Value], value: &Value) -> Option<u64> {
        let mut id = value["id"].as_u64()?;
        let mut up = parent(value);
        for _ in 0..values.len() {
            let Some(next) = up else {
                return Some(id);
            };
            id = next;
            up = parent(values.iter().find(|v| v["id"].as_u64() == Some(next))?);
        }
        None
    }
    let by_id = |id: Option<u64>| {
        let id = id?.to_string();
        comments
            .iter()
            .find(|c| c.id.as_deref() == Some(id.as_str()))
            .cloned()
    };
    values
        .iter()
        .filter(|value| parent(value).is_none() && value["inline"].is_object())
        .filter(|value| value["deleted"] != true)
        .filter_map(|root| {
            let inline = &root["inline"];
            let (line, side) = match (inline["to"].as_u64(), inline["from"].as_u64()) {
                (Some(to), _) => (Some(to), "new"),
                (None, Some(from)) => (Some(from), "old"),
                (None, None) => (None, "new"),
            };
            let start = match side {
                "new" => inline["start_to"].as_u64(),
                _ => inline["start_from"].as_u64(),
            };
            let mut thread: Vec<&Value> = values
                .iter()
                .filter(|value| root_of(values, value) == root["id"].as_u64())
                .collect();
            thread.sort_by_key(|value| value["created_on"].as_str().unwrap_or_default().to_owned());
            Some(Thread {
                id: root["id"].as_u64()?.to_string(),
                path: masked(inline["path"].as_str().unwrap_or_default()),
                line,
                start,
                side,
                resolved: Some(root["resolution"].is_object()),
                outdated: inline["outdated"] == true,
                comments: thread
                    .into_iter()
                    .filter_map(|value| by_id(value["id"].as_u64()))
                    .collect(),
                comments_total: None,
            })
        })
        .collect()
}

/// The threads of a Data Center pull request's activities: each comment on a line
/// (`commentAnchor`) with its replies. Data Center says a thread resolved by `threadResolved`
/// where its version has it, a task by its `state`, and outdated by `orphaned`.
pub(super) fn data_center(activities: &[Value], comments: &[Comment]) -> Vec<Thread> {
    fn ids(comment: &Value, out: &mut Vec<String>) {
        out.extend(comment["id"].as_u64().map(|id| id.to_string()));
        for reply in comment["comments"].as_array().into_iter().flatten() {
            ids(reply, out);
        }
    }
    activities
        .iter()
        .filter(|a| a["action"] == "COMMENTED" && a["commentAnchor"]["path"].is_string())
        .filter(|a| a["comment"]["parent"].is_null())
        .filter_map(|activity| {
            let (comment, anchor) = (&activity["comment"], &activity["commentAnchor"]);
            let mut own = Vec::new();
            ids(comment, &mut own);
            let old = anchor["fileType"] == "FROM" || anchor["lineType"] == "REMOVED";
            let resolved = match (comment["threadResolved"].as_bool(), &comment["severity"]) {
                (Some(resolved), _) => Some(resolved),
                (None, severity) if severity == "BLOCKER" => Some(comment["state"] == "RESOLVED"),
                (None, _) => None,
            };
            Some(Thread {
                id: comment["id"].as_u64()?.to_string(),
                path: masked(anchor["path"].as_str().unwrap_or_default()),
                line: anchor["line"].as_u64(),
                start: None,
                side: if old { "old" } else { "new" },
                resolved,
                outdated: anchor["orphaned"] == true,
                comments: own
                    .iter()
                    .filter_map(|id| comments.iter().find(|c| c.id.as_deref() == Some(id)))
                    .cloned()
                    .collect(),
                comments_total: None,
            })
        })
        .collect()
}

/// `forge reply` and `forge resolve`, once the gate let them through: an answer in a review
/// thread of a pull request of a repository of the place, or its resolution.
pub(super) async fn call(daemon: &Daemon, place: &Place, asked: ForgeCall) -> Outcome {
    if let Some(why) = malformed(&asked) {
        return Outcome::Refused(why);
    }
    let action = asked.action.as_str();
    let number = asked.number.unwrap_or_default();
    let thread = asked
        .thread
        .as_ref()
        .map(ThreadId::text)
        .unwrap_or_default();
    let checkout = match place_checkouts(daemon, place)
        .and_then(|all| chosen(all, asked.repository.as_deref(), action, true))
    {
        Ok(mut checkouts) => checkouts.remove(0),
        Err(refusal) => return refusal,
    };
    let forge = match declared_forge(&checkout).await {
        Ok(forge) => forge,
        Err(None) => {
            return Outcome::Refused(format!(
                "refused: {}: {NO_FORGE}; nothing was written: ask the user to set it",
                checkout.path
            ));
        }
        Err(Some(why)) => return Outcome::Refused(why),
    };
    let slug = forge.slug();
    let of = format!("pull request #{number} of {slug}");
    if matches!(forge, Forge::Bitbucket { .. })
        && let Some(why) = bitbucket_thread(&json!(thread))
    {
        return Outcome::Refused(why);
    }
    // Only a thread of this pull request, of a repository of the place, is written to: a GitHub
    // node id names a thread anywhere on GitHub, a Bitbucket comment number any comment.
    let found = match thread_of(&forge, number, &thread).await {
        Ok(Ok(found)) => found,
        Ok(Err(more)) => {
            let more = match more {
                true => ", and the forge has more than were read: it may be past them",
                false => "",
            };
            return Outcome::Refused(format!(
                "refused: the {of} has no review thread {thread}{more}: forge show with detail \
                 full lists its threads; nothing was written"
            ));
        }
        Err(Refusal::Refused(why)) => return Outcome::Refused(why),
        Err(Refusal::Failed(why)) => return Outcome::Failed(why),
    };
    if let ("resolve", Some(resolved)) = (action, asked.resolved)
        && found.resolved == Some(resolved)
    {
        return Outcome::Done(format!(
            "The review thread {thread} on {} of the {of} was already {}; nothing was changed.",
            found.place(),
            resolved_word(resolved)
        ));
    }
    let body = asked.body.as_deref().unwrap_or_default();
    let done = match (&forge, action) {
        (Forge::GitHub { .. }, "resolve") => {
            github_resolve(&thread, asked.resolved.unwrap_or(true))
                .await
                .map(|()| None)
        }
        (Forge::GitHub { .. }, _) => github_reply(&thread, body).await,
        (Forge::Bitbucket { .. }, "resolve") => {
            bitbucket_resolve(&forge, number, &thread, asked.resolved.unwrap_or(true))
                .await
                .map(|()| None)
        }
        (Forge::Bitbucket { .. }, _) => bitbucket_reply(&forge, number, &thread, body).await,
    };
    let id = match done {
        Ok(id) => id,
        Err(Refusal::Refused(why)) => return Outcome::Refused(why),
        Err(Refusal::Failed(why)) => return Outcome::Failed(why),
    };
    let on = format!(" on {}", found.place());
    match (action, asked.resolved) {
        ("resolve", Some(resolved)) => Outcome::Done(format!(
            "The review thread {thread}{on} of the {of} is now {}.",
            resolved_word(resolved)
        )),
        _ => {
            let key = number.to_string();
            let record = (
                place.project.as_str(),
                forge.kind(),
                checkout.path.as_str(),
                key.as_str(),
            );
            crate::posts::record_comment(daemon, record, id, body);
            Outcome::Done(format!(
                "The reply was posted in the review thread {thread}{on} of the {of}."
            ))
        }
    }
}

/// The thread `thread` of the pull request `number`, read now, and whether the forge holds more
/// than was read when it is not one of them (a comment of another pull request, a general
/// comment, a reply that starts no thread, or one past what was read).
async fn thread_of(
    forge: &Forge,
    number: u64,
    thread: &str,
) -> Result<Result<Thread, bool>, Refusal> {
    if matches!(forge, Forge::GitHub { .. }) {
        signed_in().await.map_err(Refusal::Failed)?;
        let found = github_thread_of(forge, number, thread).await?;
        return Ok(found.ok_or(false));
    }
    let context = bitbucket::Contexts::default().of(forge).await?;
    let Some(pull) = bitbucket::pull_numbered(forge, &context, number).await? else {
        return Err(Refusal::Refused(format!(
            "refused: {} has no pull request #{number}: forge list lists them; nothing was written",
            forge.slug()
        )));
    };
    let (threads, cut) = pull.threads.unwrap_or_else(|| Ok(Default::default()))?;
    Ok(threads.into_iter().find(|t| t.id == thread).ok_or(cut))
}

fn resolved_word(resolved: bool) -> &'static str {
    match resolved {
        true => "resolved",
        false => "unresolved",
    }
}

/// Why a write was not done: the forge cannot do it (nothing was asked), or it failed.
enum Refusal {
    Refused(String),
    Failed(String),
}

impl From<String> for Refusal {
    fn from(why: String) -> Refusal {
        Refusal::Failed(why)
    }
}

/// `addPullRequestReviewThreadReply`: the reply's id, as its link names it.
async fn github_reply(thread: &str, body: &str) -> Result<Option<String>, Refusal> {
    let query = "mutation($thread: ID!, $body: String!) { addPullRequestReviewThreadReply(input: \
                 {pullRequestReviewThreadId: $thread, body: $body}) { comment { url } } }";
    let said = gh_json(&[
        "api",
        "graphql",
        "--hostname",
        flottille_core::declared::GITHUB,
        "-f",
        &format!("query={query}"),
        "-f",
        &format!("thread={thread}"),
        "-f",
        &format!("body={body}"),
    ])
    .await?;
    Ok(
        said["data"]["addPullRequestReviewThreadReply"]["comment"]["url"]
            .as_str()
            .and_then(crate::posts::comment_id),
    )
}

/// `resolveReviewThread` or `unresolveReviewThread`.
async fn github_resolve(thread: &str, resolved: bool) -> Result<(), Refusal> {
    let mutation = match resolved {
        true => "resolveReviewThread",
        false => "unresolveReviewThread",
    };
    let query = format!(
        "mutation($thread: ID!) {{ {mutation}(input: {{threadId: $thread}}) {{ thread {{ id \
         isResolved }} }} }}"
    );
    gh_json(&[
        "api",
        "graphql",
        "--hostname",
        flottille_core::declared::GITHUB,
        "-f",
        &format!("query={query}"),
        "-f",
        &format!("thread={thread}"),
    ])
    .await?;
    Ok(())
}

/// Where a Bitbucket pull request's comments are written: Cloud's API, or Data Center's.
fn comments_path(forge: &Forge, number: u64) -> String {
    match forge {
        Forge::Bitbucket { owner, name, .. } if !bitbucket::cloud(forge) => {
            format!("/rest/api/1.0/projects/{owner}/repos/{name}/pull-requests/{number}/comments")
        }
        _ => format!(
            "{}/pullrequests/{number}/comments",
            bitbucket::cloud_repository(forge)
        ),
    }
}

/// A comment answering the thread's first one (`parent`): Cloud takes its text as
/// `content.raw`, Data Center as `text`. The new comment's number.
async fn bitbucket_reply(
    forge: &Forge,
    number: u64,
    thread: &str,
    body: &str,
) -> Result<Option<String>, Refusal> {
    let context = bitbucket::Contexts::default().of(forge).await?;
    let parent: u64 = thread.parse().map_err(|_| Refusal::Failed(thread.into()))?;
    let input = match bitbucket::cloud(forge) {
        true => json!({"content": {"raw": body}, "parent": {"id": parent}}),
        false => json!({"text": body, "parent": {"id": parent}}),
    }
    .to_string();
    let path = comments_path(forge, number);
    let args = [
        "api",
        &path,
        "--context",
        &context,
        "--method",
        "POST",
        "--input",
        &input,
    ];
    let said = bitbucket::json(&args).await?;
    Ok(said["id"].as_u64().map(|id| id.to_string()))
}

/// Cloud: `POST …/comments/<id>/resolve`, `DELETE` to open it again. Data Center: the comment
/// read for its version, then `PUT` with `threadResolved`, or with `state` for a task; a server
/// whose comments say neither has no resolved thread, and nothing is written.
async fn bitbucket_resolve(
    forge: &Forge,
    number: u64,
    thread: &str,
    resolved: bool,
) -> Result<(), Refusal> {
    let context = bitbucket::Contexts::default().of(forge).await?;
    let path = format!("{}/{thread}", comments_path(forge, number));
    if bitbucket::cloud(forge) {
        let path = format!("{path}/resolve");
        let method = if resolved { "POST" } else { "DELETE" };
        let args = ["api", &path, "--context", &context, "--method", method];
        bitbucket::text(&args).await?;
        return Ok(());
    }
    // The comment is read for its version, which the write must say: one edited in between is
    // refused (409), so it is read once more and written once more, no further.
    let mut again = true;
    loop {
        let comment = bitbucket::json(&["api", &path, "--context", &context]).await?;
        let Some(version) = comment["version"].as_u64() else {
            return Err(Refusal::Failed(format!(
                "Bitbucket Data Center's answer for the comment {thread} says no version, which \
                 a change needs: nothing was changed"
            )));
        };
        let input = if comment["threadResolved"].is_boolean() {
            json!({"version": version, "threadResolved": resolved})
        } else if comment["severity"] == "BLOCKER" {
            let state = if resolved { "RESOLVED" } else { "OPEN" };
            json!({"version": version, "state": state})
        } else {
            return Err(Refusal::Refused(format!(
                "refused: {} is on a Bitbucket Data Center whose comments say no resolved state \
                 (its version resolves no thread, only tasks): nothing was changed; reply in the \
                 thread instead",
                forge.slug()
            )));
        };
        let input = input.to_string();
        let args = [
            "api",
            &path,
            "--context",
            &context,
            "--method",
            "PUT",
            "--input",
            &input,
        ];
        match bitbucket::text(&args).await {
            Ok(_) => return Ok(()),
            // What the server said, not the address it was said for, which may hold the digits.
            Err(why)
                if why
                    .split_once("failed: ")
                    .is_some_and(|(_, said)| said.contains("409")) =>
            {
                if !std::mem::take(&mut again) {
                    return Err(Refusal::Failed(format!(
                        "the comment changed meanwhile: try again; nothing was changed ({why})"
                    )));
                }
            }
            Err(why) => return Err(why.into()),
        }
    }
}
