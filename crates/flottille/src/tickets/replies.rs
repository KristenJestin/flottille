//! `tickets reply` (#934, `docs/tickets.md`, "Replies"): an answer to a comment of a ticket, which
//! any comment has, not only an agent's own.
//!
//! Neither tracker has threads on a ticket, so a reply is a new comment that says whom it answers
//! and what it quotes. On GitHub the quote is Markdown (`> @alice wrote:`). On Jira it is the
//! document Jira's own Reply makes: a mention of the author, the comment in a quote, the answer.
//! `acli jira workitem comment create` has no flag for a visibility (it uses the project's
//! default), so a comment restricted to a role or a group, or a Service Management internal note,
//! is not answered: the quote would go out to everyone. Nor is a comment GitHub hides
//! (minimized).

use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use flottille_core::ticket::Key;
use serde_json::{Value, json};

use super::{AdfFile, Comment, TicketsCall, acli_run, jira_site};
use crate::daemon::Daemon;
use crate::mcp::Outcome;

/// How much of the comment answered is quoted.
const QUOTED_CHARS: usize = 500;

/// Why a Jira comment is not for everyone, as the refusal says it, from its `visibility` (`{"type":
/// "role", "value": "Administrators"}`) or its `jsdPublic` (`false`: a Service Management internal
/// note); none for a comment everyone who sees the work item can read.
pub(super) fn restriction(comment: &Value) -> Option<String> {
    let visibility = &comment["visibility"];
    if !visibility.is_null() {
        let who = visibility["value"]
            .as_str()
            .map_or_else(|| "a role or a group".to_owned(), super::masked);
        return Some(format!(
            "restricted to {who}, and acli cannot post a comment in a visibility (its comment \
             create uses the project's default)"
        ));
    }
    (comment["jsdPublic"] == false).then(|| {
        "an internal note, and acli cannot post one (its comment create uses the project's \
         default)"
            .to_owned()
    })
}

/// Why a GitHub comment is not for everyone: it is hidden (minimized), and a quote would show it.
pub(super) fn hidden(comment: &Value) -> Option<String> {
    (comment["isMinimized"] == true).then(|| "hidden (minimized) on GitHub".to_owned())
}

fn blank(text: &Option<String>) -> Option<&str> {
    text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

/// The ticket, and its comment `named`, when an agent may answer it; else why not.
pub(super) async fn answerable(
    daemon: &Daemon,
    project: &Project,
    key: &Key,
    named: &str,
) -> Result<(super::Ticket, Comment), Outcome> {
    let ticket = super::read_known(daemon, project, key)
        .await
        .map_err(|problem| {
            Outcome::Failed(format!(
                "the ticket {} could not be read: {problem}",
                key.text()
            ))
        })?;
    let reference = &ticket.reference;
    let named = named.trim();
    let Some(found) = ticket
        .comments
        .iter()
        .find(|c| super::links::same_comment(c.id.as_deref(), named))
    else {
        return Err(Outcome::Refused(format!(
            "refused: {reference} has no comment {named} among the {} newest it can read; \
             nothing was written; show with detail full gives the id of each comment",
            ticket.comments.len()
        )));
    };
    if let Some(who) = &found.restricted {
        return Err(Outcome::Refused(format!(
            "refused: the comment {named} of {reference} is {who}: a reply would quote it to \
             everyone; nothing was written"
        )));
    }
    let found = found.clone();
    Ok((ticket, found))
}

/// The comment as a Markdown quote, to answer under.
fn quoted(comment: &Comment) -> String {
    super::cut(&comment.body, QUOTED_CHARS)
        .lines()
        .map(|line| match line.is_empty() {
            true => ">".to_owned(),
            false => format!("> {line}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A GitHub reply: the quote, then the answer.
fn markdown(comment: &Comment, body: &str) -> String {
    format!(
        "> @{} wrote:\n{}\n\n{body}",
        comment.author,
        quoted(comment)
    )
}

/// A Jira reply as a document: the author mentioned when Jira gave their account, the comment
/// quoted, then the answer. The quote is built by the converter, under its rules for the text of
/// a quote (a quote, a heading, a rule or a table in it is written as paragraphs, which Jira
/// takes).
fn document(comment: &Comment, body: &str) -> Value {
    let who = match &comment.account {
        Some(account) => {
            json!({"type": "mention", "attrs": {"id": account, "text": format!("@{}", comment.author)}})
        }
        None => json!({"type": "text", "text": comment.author}),
    };
    let mut content =
        vec![json!({"type": "paragraph", "content": [who, {"type": "text", "text": " wrote:"}]})];
    for part in [quoted(comment).as_str(), body] {
        if let Some(blocks) = crate::adf::from_markdown(part)["content"].as_array() {
            content.extend(blocks.iter().cloned());
        }
    }
    json!({"type": "doc", "version": 1, "content": content})
}

/// What the card says of a reply: the comment as it goes out, quote and answer, and on Jira whom
/// it mentions, who is notified. Refused, with why, when the comment cannot be answered.
pub(super) async fn says(
    daemon: &Daemon,
    project: &Project,
    args: &Value,
) -> Result<Vec<String>, String> {
    if args["action"] != "reply" {
        return Ok(Vec::new());
    }
    let (Some(ticket), Some(named), Some(body)) = (
        args["ticket"].as_str(),
        args["comment"].as_str(),
        args["body"]
            .as_str()
            .map(str::trim)
            .filter(|b| !b.is_empty()),
    ) else {
        return Ok(Vec::new());
    };
    let Ok(key) = super::accepted(project, ticket) else {
        return Ok(Vec::new());
    };
    let (_, comment) = match answerable(daemon, project, &key, named).await {
        Ok(found) => found,
        Err(Outcome::Refused(why)) => return Err(why),
        Err(_) => return Ok(Vec::new()),
    };
    let jira = matches!(key, Key::Jira(_));
    let mut says = vec![format!(
        "It posts, the comment quoted first: {}",
        match jira {
            true => crate::adf::to_markdown(&document(&comment, body)),
            false => markdown(&comment, body),
        }
    )];
    if jira && comment.account.is_some() {
        says.push(format!("It mentions {}, who is notified", comment.author));
    }
    Ok(says)
}

/// `tickets reply`: a comment that answers the one named, on the tracker of the project.
pub(super) async fn reply(
    daemon: &Daemon,
    project: &Project,
    asked: &TicketsCall,
    key: &Key,
) -> Outcome {
    let (Some(named), Some(body)) = (blank(&asked.comment), blank(&asked.body)) else {
        return Outcome::Refused("refused: tickets reply needs comment and body".into());
    };
    // Checked again right before the write: the comments may have changed since the card.
    let (ticket, comment) = match answerable(daemon, project, key, named).await {
        Ok(found) => found,
        Err(outcome) => return outcome,
    };
    let id = comment.id.clone().unwrap_or_default();
    let reference = &ticket.reference;
    let done = format!("The reply to the comment {id} of {reference} was posted.");
    match (&project.tracker, key) {
        (Some(ProjectTracker::Jira { site, .. }), Key::Jira(work_item)) => {
            if let Err(problem) = jira_site(site, "written").await {
                return Outcome::Failed(problem);
            }
            let document = document(&comment, body);
            let file = match AdfFile::json(daemon, &document) {
                Ok(file) => file,
                Err(why) => return Outcome::Failed(why),
            };
            let args = [
                "jira",
                "workitem",
                "comment",
                "create",
                "--key",
                work_item,
                "--body-file",
                file.path(),
            ];
            if let Err(why) = acli_run(&args).await {
                return Outcome::Failed(why);
            }
            let (repository, item) = super::events::item_of(project, key).unwrap_or_default();
            let on = (
                project.name.as_str(),
                ticket.tracker.as_str(),
                repository.as_str(),
                item.as_str(),
            );
            let posted = crate::adf::to_markdown(&document);
            let posted_id = super::posted_comment_id(daemon, on, site, &posted).await;
            crate::posts::record_comment(daemon, on, posted_id, &posted);
            Outcome::Done(done)
        }
        (Some(ProjectTracker::Github { .. }), Key::Issue(_)) => {
            let call = json!({"action": "comment", "ticket": key.text(),
                "body": markdown(&comment, body)});
            let Ok(call) = serde_json::from_value::<TicketsCall>(call) else {
                return Outcome::Failed("the reply could not be prepared".into());
            };
            match super::write(daemon, project, &call, key).await {
                Outcome::Done(_) => Outcome::Done(done),
                other => other,
            }
        }
        _ => Outcome::Refused("refused: the ticket is not of the project's tracker".into()),
    }
}
