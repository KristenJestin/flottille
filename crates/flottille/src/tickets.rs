//! Tickets (`docs/tickets.md`): what the tracker says of the ticket a workspace is linked to, its
//! title, state, description and comments. Reading only, through the user's own command lines,
//! spawned without a shell: `gh` for the issues of GitHub, `acli` (Atlassian's) for Jira. Flottille
//! never reads, keeps nor prints a token, and of each one's sign-in status it reads the exit code
//! alone. No write command of either is reachable from here: the only calls are `gh issue view`,
//! `acli jira auth status` and `acli jira workitem view`.
//!
//! A ticket is read on demand, never in the background, and kept nowhere. Every text kept from a
//! tracker is masked before it is shown or stored.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use flottille_core::protocol::{WorkspaceName, WorkspaceTicket};
use flottille_core::store::now_ms;
use flottille_core::ticket::{Key, Tracker};
use flottille_core::workspace::Workspace;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::forge::{self, Forge, Gh, Remote, masked};
use crate::git::{self, Class};
use crate::mcp::{Outcome, arguments};
use crate::place::Place;
use crate::projects::Failure;

/// A call of `acli` is cut after this.
const ACLI_TIME: Duration = Duration::from_secs(30);
/// What is kept of a description, of one comment, and how many comments (the newest).
const DESCRIPTION_CHARS: usize = 20_000;
const COMMENT_CHARS: usize = 4_000;
const COMMENTS: usize = 30;
/// What `gh issue view` is asked for.
const GH_FIELDS: &str = "number,title,state,body,comments,url";
/// What `acli jira workitem view` is asked for.
const ACLI_FIELDS: &str = "summary,status,description,comment";

/// A ticket as its tracker said it, its texts masked.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticket {
    /// `#123` or `ABC-123`.
    pub reference: String,
    pub tracker: Tracker,
    /// `owner/name` of the GitHub repository the issue is in.
    pub repository: Option<String>,
    pub title: String,
    /// `open` or `closed` for GitHub; Jira's own word for the status (`In Progress`).
    pub state: String,
    pub url: Option<String>,
    pub description: String,
    /// Oldest first, the newest [`COMMENTS`] only.
    pub comments: Vec<Comment>,
    /// How many older comments are not kept.
    pub older: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub author: String,
    pub at: Option<String>,
    pub body: String,
}

impl Ticket {
    fn to_json(&self) -> Value {
        json!({
            "ticket": self.reference,
            "tracker": self.tracker.as_str(),
            "repository": self.repository,
            "title": self.title,
            "state": self.state,
            "url": self.url,
            "description": self.description,
            "comments": self.comments.iter()
                .map(|c| json!({"author": c.author, "at": c.at, "body": c.body}))
                .collect::<Vec<_>>(),
            "older_comments": self.older,
        })
    }

    /// What `ticket_read` answers.
    fn text(&self) -> String {
        let place = match (&self.repository, self.tracker) {
            (Some(repository), _) => format!(" of {repository} (GitHub)"),
            (None, _) => " (Jira)".to_owned(),
        };
        let mut text = format!(
            "Ticket {}{place}, {}: {}\n",
            self.reference, self.state, self.title
        );
        if let Some(url) = &self.url {
            text.push_str(&format!("  {url}\n"));
        }
        if self.description.is_empty() {
            text.push_str("\nNo description.\n");
        } else {
            text.push_str(&format!("\nDescription:\n{}\n", self.description));
        }
        if !self.comments.is_empty() {
            text.push_str(&format!("\nComments ({}):\n", self.comments.len()));
            if self.older > 0 {
                text.push_str(&format!("  ({} older ones are not shown)\n", self.older));
            }
            for comment in &self.comments {
                let at = comment
                    .at
                    .as_deref()
                    .map_or(String::new(), |at| format!(", {at}"));
                text.push_str(&format!(
                    "- {}{at}: {}\n",
                    comment.author,
                    comment.body.replace('\n', "\n    ")
                ));
            }
        }
        text
    }
}

fn cut(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}… (cut)", &text[..end]),
        None => text.to_owned(),
    }
}

/// The text of one field of a tracker's JSON, masked.
fn said(value: &Value) -> String {
    masked(value.as_str().unwrap_or_default().trim())
}

/// Reads `key`, the ticket of `workspace`, on its tracker. `Err` says why it cannot be read, masked,
/// with what to do.
pub async fn read(workspace: &Workspace, key: &Key) -> Result<Ticket, String> {
    match key {
        Key::Issue(number) => {
            let forge = github_of(workspace).await?;
            read_issue(&forge, *number).await
        }
        Key::Jira(key) => read_work_item(key).await,
    }
}

/// The repository whose issues `#123` names: the first repository of the workspace that is on
/// GitHub.
async fn github_of(workspace: &Workspace) -> Result<Forge, String> {
    for repository in &workspace.repositories {
        let folder = match repository.path.as_str() {
            "." => workspace.project_folder.clone(),
            path => workspace.project_folder.join(path),
        };
        let url = git::output(
            &folder,
            &["remote", "get-url", repository.base.remote.as_str()],
            Class::Read,
        )
        .await;
        if let Ok(url) = url
            && let Remote::Forge(forge @ Forge::GitHub { .. }) = forge::remote_of(&url, &[])
        {
            return Ok(forge);
        }
    }
    Err(format!(
        "no repository of workspace {} is on GitHub, so there are no issues to read for #<n>; \
         a Jira key (ABC-123) does not need one",
        workspace.name
    ))
}

async fn read_issue(forge: &Forge, number: u64) -> Result<Ticket, String> {
    forge::signed_in().await?;
    let slug = forge.slug();
    let issue = forge::gh_json(&[
        "issue",
        "view",
        &number.to_string(),
        "-R",
        &slug,
        "--json",
        GH_FIELDS,
    ])
    .await?;
    let comments: Vec<Comment> = issue["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|comment| Comment {
            author: said(&comment["author"]["login"]),
            at: comment["createdAt"].as_str().map(str::to_owned),
            body: cut(&said(&comment["body"]), COMMENT_CHARS),
        })
        .collect();
    Ok(Ticket {
        reference: format!("#{number}"),
        tracker: Tracker::GitHub,
        repository: Some(slug),
        title: said(&issue["title"]),
        state: said(&issue["state"]).to_lowercase(),
        url: issue["url"].as_str().map(masked),
        description: cut(&said(&issue["body"]), DESCRIPTION_CHARS),
        older: comments.len().saturating_sub(COMMENTS),
        comments: newest(comments),
    })
}

fn newest(mut comments: Vec<Comment>) -> Vec<Comment> {
    let older = comments.len().saturating_sub(COMMENTS);
    comments.drain(..older);
    comments
}

async fn read_work_item(key: &str) -> Result<Ticket, String> {
    match acli(&["jira", "auth", "status"]).await {
        Err(Gh::Missing) => {
            return Err(
                "acli, Atlassian's command line, is not on the daemon's PATH: install it \
                 (https://developer.atlassian.com/cloud/acli/) and sign in with `acli jira auth \
                 login` to read Jira tickets; the rest of Flottille works without it"
                    .into(),
            );
        }
        Err(Gh::Failed(why)) => return Err(format!("acli cannot run: {why}")),
        Ok((false, _, _)) => {
            return Err(
                "acli is not signed in to Jira: run `acli jira auth login` to read Jira \
                 tickets; the rest of Flottille works without it"
                    .into(),
            );
        }
        Ok((true, _, _)) => {}
    }
    let (ok, stdout, stderr) = match acli(&[
        "jira",
        "workitem",
        "view",
        key,
        "--json",
        "--fields",
        ACLI_FIELDS,
    ])
    .await
    {
        Ok(output) => output,
        Err(Gh::Missing) => return Err("acli is not on the daemon's PATH".into()),
        Err(Gh::Failed(why)) => return Err(format!("acli jira workitem view: {why}")),
    };
    if !ok {
        let said = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(format!(
            "acli jira workitem view {key} failed: {}",
            masked(&said.chars().take(300).collect::<String>())
        ));
    }
    let item: Value = serde_json::from_str(&stdout)
        .map_err(|error| format!("acli jira workitem view answered what is not JSON: {error}"))?;
    Ok(work_item(key, &item))
}

/// A Jira work item as `acli jira workitem view --json` prints it: Jira's REST shape (`key`,
/// `self`, `fields`). Its description and comments are Atlassian Document Format, or plain text.
fn work_item(key: &str, item: &Value) -> Ticket {
    let fields = &item["fields"];
    let comments: Vec<Comment> = fields["comment"]["comments"]
        .as_array()
        .or(fields["comment"].as_array())
        .into_iter()
        .flatten()
        .map(|comment| Comment {
            author: said(&comment["author"]["displayName"]),
            at: comment["created"].as_str().map(str::to_owned),
            body: cut(&masked(&document_text(&comment["body"])), COMMENT_CHARS),
        })
        .collect();
    Ticket {
        reference: key.to_owned(),
        tracker: Tracker::Jira,
        repository: None,
        title: said(&fields["summary"]),
        state: said(&fields["status"]["name"]),
        // The site is the host of `self`, the REST address of the item.
        url: item["self"]
            .as_str()
            .and_then(|own| own.strip_prefix("https://"))
            .and_then(|rest| rest.split('/').next())
            .map(|host| masked(&format!("https://{host}/browse/{key}"))),
        description: cut(
            &masked(&document_text(&fields["description"])),
            DESCRIPTION_CHARS,
        ),
        older: comments.len().saturating_sub(COMMENTS),
        comments: newest(comments),
    }
}

/// The plain text of a Jira text field: a string as it is, an Atlassian Document Format tree as
/// its paragraphs, headings and list items, one per line. Nothing is rendered as HTML.
pub fn document_text(field: &Value) -> String {
    let mut out = String::new();
    match field {
        Value::String(text) => out.push_str(text),
        Value::Object(_) => block(field, &mut out),
        _ => {}
    }
    // Three line breaks or more are one empty line.
    let mut text = String::new();
    let mut breaks = 0;
    for c in out.trim().chars() {
        breaks = if c == '\n' { breaks + 1 } else { 0 };
        if breaks <= 2 {
            text.push(c);
        }
    }
    text
}

/// A node of the tree, and what is under it.
fn block(node: &Value, out: &mut String) {
    let children = |out: &mut String| {
        for child in node["content"].as_array().into_iter().flatten() {
            block(child, out);
        }
    };
    match node["type"].as_str().unwrap_or_default() {
        "text" => out.push_str(node["text"].as_str().unwrap_or_default()),
        "hardBreak" => out.push('\n'),
        "mention" | "emoji" => out.push_str(
            node["attrs"]["text"]
                .as_str()
                .or(node["attrs"]["shortName"].as_str())
                .unwrap_or_default(),
        ),
        "inlineCard" | "blockCard" => {
            out.push_str(node["attrs"]["url"].as_str().unwrap_or_default());
            out.push('\n');
        }
        "rule" => out.push_str("---\n"),
        "bulletList" | "orderedList" => {
            let ordered = node["type"] == "orderedList";
            for (at, item) in node["content"].as_array().into_iter().flatten().enumerate() {
                let mut line = String::new();
                block(item, &mut line);
                let bullet = match ordered {
                    true => format!("{}. ", at + 1),
                    false => "- ".to_owned(),
                };
                out.push_str(&format!("{bullet}{}\n", line.trim().replace('\n', "\n  ")));
            }
            out.push('\n');
        }
        "tableRow" => {
            let cells: Vec<String> = node["content"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|cell| {
                    let mut text = String::new();
                    block(cell, &mut text);
                    text.trim().replace('\n', " ")
                })
                .collect();
            out.push_str(&cells.join(" | "));
            out.push('\n');
        }
        "paragraph" | "heading" | "codeBlock" | "blockquote" | "panel" => {
            children(out);
            out.push_str("\n\n");
        }
        // The document, a list item, a table and anything unknown: its text, if it has any.
        _ => children(out),
    }
}

/// `acli <args>`, as `forge::gh` runs `gh`: spawned without a shell, with the user's own sign-in,
/// nothing waiting for an answer: whether it succeeded, its output and its errors.
async fn acli(args: &[&str]) -> Result<(bool, String, String), Gh> {
    let mut command = tokio::process::Command::new("acli");
    command
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => Gh::Missing,
        _ => Gh::Failed(error.to_string()),
    })?;
    match tokio::time::timeout(ACLI_TIME, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok((
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )),
        Ok(Err(error)) => Err(Gh::Failed(error.to_string())),
        Err(_) => Err(Gh::Failed(format!(
            "acli {} took more than {} s",
            args[..2].join(" "),
            ACLI_TIME.as_secs()
        ))),
    }
}

fn find(daemon: &Daemon, project: &str, name: &str) -> Result<Workspace, Failure> {
    daemon
        .read(|store| store.workspace(project, name))?
        .ok_or_else(|| {
            Failure::new(
                "not-found",
                format!("project {project} has no workspace named {name}"),
            )
        })
}

/// What the protocol shows of a workspace's ticket (`workspace.ticket`).
async fn state_of(workspace: &Workspace) -> Value {
    let mut state = json!({
        "project": workspace.project,
        "workspace": workspace.name,
        "ticket": workspace.ticket,
        "read_at": now_ms(),
        "problem": null,
    });
    let Some(text) = &workspace.ticket else {
        return state;
    };
    let read = match Key::parse(text) {
        Ok(key) => read(workspace, &key).await,
        Err(why) => Err(why),
    };
    match read {
        Ok(ticket) => {
            if let (Value::Object(state), Value::Object(read)) = (&mut state, ticket.to_json()) {
                state.extend(read);
            }
        }
        Err(problem) => state["problem"] = problem.into(),
    }
    state
}

/// `workspace.ticket`: the workspace's ticket, read now.
pub async fn call(daemon: Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let asked: WorkspaceName = serde_json::from_value(params)?;
    let workspace = find(&daemon, &asked.project, &asked.name)?;
    Ok(state_of(&workspace).await)
}

/// `workspace.link_ticket`: links the workspace to a ticket, or to none; an event says it.
pub async fn link(daemon: Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let asked: WorkspaceTicket = serde_json::from_value(params)?;
    let workspace = find(&daemon, &asked.project, &asked.name)?;
    let ticket = asked
        .ticket
        .as_deref()
        .map(|text| Key::parse(text).map(|key| key.text()))
        .transpose()
        .map_err(Failure::refused)?;
    let event = json!({
        "project": workspace.project,
        "workspace": workspace.name,
        "ticket": ticket,
    });
    daemon.write(|store| {
        let event =
            store.set_ticket(workspace.id, ticket.as_deref(), ("workspace.ticket", event))?;
        Ok(((), event))
    })?;
    let workspace = find(&daemon, &asked.project, &asked.name)?;
    Ok(state_of(&workspace).await)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketRead {
    ticket: Option<String>,
}

/// `ticket_read`: the ticket of the agent's own workspace, or another ticket of the same tracker.
/// Read only, and never another tracker's.
pub async fn ticket_read(daemon: &Daemon, place: &Place, args: Value) -> Outcome {
    let asked: TicketRead = match arguments("ticket_read", args) {
        Ok(asked) => asked,
        Err(refusal) => return refusal,
    };
    let Some(name) = &place.workspace else {
        return Outcome::Refused(
            "refused: this agent works in the project folder, not in a workspace: ticket_read \
             reads a workspace's ticket"
                .into(),
        );
    };
    let workspace = match daemon.read(|store| store.workspace(&place.project, name)) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return Outcome::Failed(format!("the workspace {name} is gone")),
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let linked = workspace.ticket.as_deref().and_then(|t| Key::parse(t).ok());
    let key = match (&asked.ticket, &linked) {
        (Some(text), _) => match Key::parse(text) {
            Ok(key) => key,
            Err(why) => return Outcome::Refused(format!("refused: {why}")),
        },
        (None, Some(key)) => key.clone(),
        (None, None) => {
            return Outcome::Refused(format!(
                "refused: workspace {} is linked to no ticket: name one (#123 or ABC-123), or ask \
                 the user to link it",
                workspace.name
            ));
        }
    };
    // The workspace's tracker: its ticket's, GitHub's (its own repositories') when it has none.
    let tracker = linked.as_ref().map_or(Tracker::GitHub, Key::tracker);
    if key.tracker() != tracker {
        let (said, other) = match tracker {
            Tracker::GitHub => ("GitHub issues (#123)", "Jira"),
            Tracker::Jira => ("Jira (ABC-123)", "GitHub issues"),
        };
        return Outcome::Refused(format!(
            "refused: {} is a {other} ticket, and this workspace's tracker is {said}: ticket_read \
             reads the workspace's ticket and others of the same tracker",
            key.text()
        ));
    }
    match read(&workspace, &key).await {
        Ok(ticket) => Outcome::Done(ticket.text()),
        Err(problem) => Outcome::Failed(format!(
            "the ticket {} could not be read: {problem}",
            key.text()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_is_its_paragraphs_and_list_items_as_plain_text() {
        let document = json!({"type": "doc", "version": 1, "content": [
            {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Goal"}]},
            {"type": "paragraph", "content": [
                {"type": "text", "text": "Let users "},
                {"type": "text", "text": "log in", "marks": [{"type": "strong"}]},
                {"type": "hardBreak"},
                {"type": "mention", "attrs": {"id": "1", "text": "@alice"}}
            ]},
            {"type": "bulletList", "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "form"}]}]},
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "session"}]}]}
            ]},
            {"type": "orderedList", "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "ship"}]}]}
            ]},
            {"type": "mediaSingle", "content": [{"type": "media", "attrs": {"id": "x"}}]}
        ]});

        assert_eq!(
            document_text(&document),
            "Goal\n\nLet users log in\n@alice\n\n- form\n- session\n\n1. ship"
        );
        assert_eq!(
            document_text(&json!("plain <b>text</b>")),
            "plain <b>text</b>"
        );
        assert_eq!(document_text(&Value::Null), "");
    }

    #[test]
    fn a_work_item_gives_its_title_status_description_and_comments() {
        let item = json!({
            "key": "ABC-123", "self": "https://acme.atlassian.net/rest/api/3/issue/10001",
            "fields": {
                "summary": "Add the login page", "status": {"name": "In Progress"},
                "description": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Do it"}]}]},
                "comment": {"comments": [
                    {"author": {"displayName": "Alice"}, "created": "2026-10-01T10:00:00.000+0000",
                     "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "On it"}]}]}}
                ]}
            }
        });

        let ticket = work_item("ABC-123", &item);

        assert_eq!(ticket.title, "Add the login page");
        assert_eq!(ticket.state, "In Progress");
        assert_eq!(ticket.description, "Do it");
        assert_eq!(
            ticket.url.as_deref(),
            Some("https://acme.atlassian.net/browse/ABC-123")
        );
        assert_eq!(ticket.comments[0].author, "Alice");
        assert_eq!(ticket.comments[0].body, "On it");
    }
}
