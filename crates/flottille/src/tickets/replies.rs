//! `tickets reply` (#934, #1068, `docs/tickets.md`, "Replies"): an answer to a comment of a ticket,
//! which any comment has, not only an agent's own.
//!
//! GitHub has no threads on an issue, so a reply is a new comment that says whom it answers and
//! quotes it (`> @alice wrote:`). Jira Cloud has them, one level deep: a reply is `POST
//! /rest/api/3/issue/<key>/comment` with `parentId`, the comment answered or, when that is itself
//! a reply, the first comment of its thread, and the answer alone (no quote, no "wrote:"), with
//! the project's tracker entry, as `acli`'s `comment create` has no way to name a parent. A comment
//! that mentions someone goes the same way: `[@Display Name](mention:<accountId>)` is a mention
//! node, the only text that notifies on Jira.
//! A comment restricted to a role or a group, or a Service Management internal note, is not
//! answered: the answer would stand in the open under it, and the API call carries no visibility.
//! Nor is a comment GitHub hides (minimized).

use std::collections::HashMap;

use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use flottille_core::ticket::Key;
use serde_json::{Value, json};

use super::worklogs::Site;
use super::{Comment, TicketsCall, jira_site};
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
            "restricted to {who}, and a reply is posted without a visibility"
        ));
    }
    (comment["jsdPublic"] == false)
        .then(|| "an internal note, and a reply is posted without a visibility".to_owned())
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
    // A deleted user has no login: no `@` that would mention a stranger.
    let who = match comment.author.is_empty() {
        true => "A deleted user".to_owned(),
        false => format!("@{}", comment.author),
    };
    format!("> {who} wrote:\n{}\n\n{body}", quoted(comment))
}

/// Why a Jira text cannot be written, before anyone is asked: a mention that names no account, a
/// reply or a comment that mentions without the entry the project's tracker names.
pub(super) fn unwritable(tracker: &ProjectTracker, action: &str, args: &Value) -> Option<String> {
    let ProjectTracker::Jira { worklog_token, .. } = tracker else {
        return None;
    };
    if action == "reply" && worklog_token.is_none() {
        return Some(super::worklog::no_entry_to(
            "reply under a Jira comment with",
            "replies",
        ));
    }
    let body = args["body"]
        .as_str()
        .map(str::trim)
        .filter(|b| !b.is_empty());
    if !["comment", "reply", "create", "edit"].contains(&action) || body.is_none() {
        return None;
    }
    let mentioned = crate::adf::mentions(&crate::adf::from_markdown(body?));
    if let Some((id, text)) = mentioned
        .iter()
        .find(|(id, _)| !super::worklogs::is_account(id))
    {
        return Some(format!(
            "refused: the mention [{}](mention:{}) names no Jira account: after mention: goes the \
             account id (24 hexadecimal digits, or <number>:<uuid>) that tickets people gives, \
             not a name; nothing was written",
            super::masked(text),
            super::masked(id)
        ));
    }
    (action == "comment" && !mentioned.is_empty() && worklog_token.is_none())
        .then(|| super::worklog::no_entry_to("mention someone in a comment with", "comments"))
}

/// "It mentions Ana, Ben, who are notified": the people the text mentions, by display name.
fn notified(body: &str) -> Option<String> {
    let names: Vec<String> = crate::adf::mentions(&crate::adf::from_markdown(body))
        .into_iter()
        .map(|(_, text)| text.trim_start_matches('@').to_owned())
        .collect();
    (!names.is_empty()).then(|| {
        format!(
            "It mentions {}, who {} notified",
            names.join(", "),
            if names.len() == 1 { "is" } else { "are" }
        )
    })
}

/// What the card says of a Jira text that mentions someone: who is notified, and, for a comment,
/// that it goes through Jira's API with the project's entry. Refused when the entry is not there.
fn mentions_say(daemon: &Daemon, project: &Project, args: &Value) -> Result<Vec<String>, String> {
    let action = args["action"].as_str().unwrap_or_default();
    let Some(body) = args["body"]
        .as_str()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    else {
        return Ok(Vec::new());
    };
    if !matches!(project.tracker, Some(ProjectTracker::Jira { .. }))
        || !["comment", "reply", "create", "edit"].contains(&action)
    {
        return Ok(Vec::new());
    }
    let Some(who) = notified(body) else {
        return Ok(Vec::new());
    };
    let mut says = Vec::new();
    if action == "comment" {
        let (entry, _, _) = super::worklog::entry_of(daemon, project)?;
        says.push(format!(
            "It posts the comment through Jira's API with the vault entry {entry}, so that its \
             mentions notify"
        ));
    }
    says.push(who);
    Ok(says)
}

/// What the card says of a reply, and of a Jira text that mentions someone. A reply: the comment
/// answered and the answer, and on GitHub the quote as it goes out. Refused, with why, when the
/// comment cannot be answered.
pub async fn says(daemon: &Daemon, project: &Project, args: &Value) -> Result<Vec<String>, String> {
    let mut says = mentions_say(daemon, project, args)?;
    if args["action"] != "reply" {
        return Ok(says);
    }
    let (Some(ticket), Some(named), Some(body)) = (
        args["ticket"].as_str(),
        args["comment"].as_str(),
        args["body"]
            .as_str()
            .map(str::trim)
            .filter(|b| !b.is_empty()),
    ) else {
        return Ok(says);
    };
    let Ok(key) = super::accepted(project, ticket) else {
        return Ok(says);
    };
    let (_, comment) = match answerable(daemon, project, &key, named).await {
        Ok(found) => found,
        Err(Outcome::Refused(why)) => return Err(why),
        // A card without its comment would say less than what is posted.
        Err(Outcome::Failed(why) | Outcome::Invalid(why) | Outcome::Held(why)) => {
            return Err(format!("refused: {why}; nothing was written"));
        }
        Err(Outcome::Done(_)) => return Ok(says),
    };
    if matches!(key, Key::Jira(_)) {
        let (entry, _, _) = super::worklog::entry_of(daemon, project)?;
        let who = match comment.author.is_empty() {
            true => String::new(),
            false => format!(" by {}", comment.author),
        };
        let mut card = vec![
            format!(
                "It replies under the comment {}{who}, in its thread: {}",
                comment.id.as_deref().unwrap_or_default(),
                super::cut(&comment.body, QUOTED_CHARS)
            ),
            format!("Its answer: {body}"),
            format!("It posts it through Jira's API with the vault entry {entry}"),
        ];
        card.append(&mut says);
        return Ok(card);
    }
    says.insert(
        0,
        format!(
            "It posts, the comment quoted first: {}",
            markdown(&comment, body)
        ),
    );
    // On GitHub the quote's `@login` notifies its author; a deleted user has none.
    if !comment.author.is_empty() {
        says.insert(
            1,
            format!("It mentions {}, who is notified", comment.author),
        );
    }
    Ok(says)
}

/// The comments of a work item as Jira's REST gives them: each id, and the id of the comment it
/// answers when it is a reply (`parentId`).
async fn parents(jira: &Site<'_>, key: &str) -> Result<HashMap<String, Option<String>>, String> {
    let mut found = HashMap::new();
    let mut start = 0;
    // A page holds up to a thousand comments; a ticket with more is read in pages.
    for _ in 0..50 {
        let at = start.to_string();
        let page = jira
            .get(
                &format!("issue/{key}/comment"),
                &[("startAt", at.as_str()), ("maxResults", "1000")],
            )
            .await?;
        let comments = page["comments"].as_array().map_or(&[][..], Vec::as_slice);
        for comment in comments {
            let Some(id) = comment["id"].as_str() else {
                continue;
            };
            let parent = match &comment["parentId"] {
                Value::Number(number) => Some(number.to_string()),
                Value::String(text) if !text.is_empty() => Some(text.clone()),
                _ => None,
            };
            found.insert(id.to_owned(), parent);
        }
        start += comments.len();
        let total = page["total"].as_u64().unwrap_or(0);
        if comments.is_empty() || start as u64 >= total {
            break;
        }
    }
    Ok(found)
}

/// Fills the `parent` of the comments of a Jira ticket `show` draws, when the project has its
/// entry; why they could not be read, when that is so. A project without the entry draws the
/// comments flat, as acli's view gives them.
pub(super) async fn threaded(
    daemon: &Daemon,
    project: &Project,
    key: &Key,
    ticket: &mut super::Ticket,
) -> Option<String> {
    let (
        Some(ProjectTracker::Jira {
            site,
            worklog_token,
            ..
        }),
        Key::Jira(work_item),
    ) = (&project.tracker, key)
    else {
        return None;
    };
    if worklog_token.is_none() || ticket.comments.is_empty() {
        return None;
    }
    let jira = match super::worklogs::signed_in(daemon, project, site, String::new()).await {
        Ok((jira, _, _)) => jira,
        Err(why) => return Some(why),
    };
    match parents(&jira, work_item).await {
        Ok(read) => {
            for comment in &mut ticket.comments {
                comment.parent = comment
                    .id
                    .as_ref()
                    .and_then(|id| read.get(id).cloned().flatten());
            }
            None
        }
        Err(why) => Some(why),
    }
}

/// The calls to Jira's REST API as the project's entry signs them, or the refusal that says how to
/// set the entry. `to` completes "no vault entry to …", `never` is what Flottille never does with
/// a sign-in it was not given.
pub(super) async fn signed_in<'a>(
    daemon: &Daemon,
    project: &Project,
    site: &'a str,
    (to, never): (&str, &str),
) -> Result<Site<'a>, Outcome> {
    super::worklogs::signed_in(
        daemon,
        project,
        site,
        super::worklog::no_entry_to(to, never),
    )
    .await
    .map(|(jira, _, _)| jira)
    .map_err(Outcome::Refused)
}

/// `POST /rest/api/3/issue/<key>/comment` of `document`, a reply when `parent` is given, recorded
/// as the agent's comment under the id Jira answered. The secret goes to the site alone.
pub(super) async fn post_comment(
    daemon: &Daemon,
    on: (&str, &str, &str, &str),
    jira: &Site<'_>,
    (document, parent): (&Value, Option<u64>),
    by_id_only: bool,
) -> Result<(), Outcome> {
    let mut body = json!({"body": document});
    if let Some(parent) = parent {
        body["parentId"] = json!(parent);
    }
    let answer = jira
        .post(&format!("issue/{}/comment", on.3), &body)
        .await
        .map_err(Outcome::Failed)?;
    let id = answer["id"].as_str().map(|id| jira.clean(id));
    if id.is_some() || !by_id_only {
        crate::posts::record_comment(daemon, on, id, &crate::adf::to_markdown(document));
    }
    Ok(())
}

/// A comment of a Jira work item that mentions someone, through Jira's API.
pub(super) async fn comment_with_mentions(
    daemon: &Daemon,
    project: &Project,
    on: (&str, &str, &str, &str),
    site: &str,
    (document, by_id_only): (&Value, bool),
) -> Outcome {
    let jira = match signed_in(
        daemon,
        project,
        site,
        ("mention someone in a comment with", "comments"),
    )
    .await
    {
        Ok(jira) => jira,
        Err(refused) => return refused,
    };
    match post_comment(daemon, on, &jira, (document, None), by_id_only).await {
        Ok(()) => Outcome::Done(format!("The comment was posted on {}.", on.3)),
        Err(outcome) => outcome,
    }
}

/// `tickets reply`: an answer to the comment named, on the tracker of the project.
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
            let jira = match signed_in(
                daemon,
                project,
                site,
                ("reply under a Jira comment with", "replies"),
            )
            .await
            {
                Ok(jira) => jira,
                Err(refused) => return refused,
            };
            // One level deep: an answer to a reply goes under the first comment of its thread.
            let root = match parents(&jira, work_item).await {
                Ok(read) => match read.get(&id) {
                    Some(parent) => parent.clone().unwrap_or_else(|| id.clone()),
                    None => {
                        return Outcome::Failed(format!(
                            "Jira's own list of the comments of {reference} has no comment {id}: \
                             nothing was written"
                        ));
                    }
                },
                Err(why) => return Outcome::Failed(format!("{why}; nothing was written")),
            };
            let Ok(parent) = root.parse::<u64>() else {
                return Outcome::Failed(format!(
                    "Jira gave the comment {id} of {reference} a thread that is no number: \
                     nothing was written"
                ));
            };
            let document = crate::adf::from_markdown(body);
            let (repository, item) = super::events::item_of(project, key).unwrap_or_default();
            let on = (
                project.name.as_str(),
                ticket.tracker.as_str(),
                repository.as_str(),
                item.as_str(),
            );
            match post_comment(
                daemon,
                on,
                &jira,
                (&document, Some(parent)),
                asked.by_id_only,
            )
            .await
            {
                Ok(()) => Outcome::Done(done),
                Err(outcome) => outcome,
            }
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
