//! Tickets (`docs/tickets.md`): what the tracker says of the ticket a workspace is linked to, its
//! title, state, description and comments, and the agents' `tickets`, which list, search and read
//! the tracker's tickets and write them. Through the user's own command lines, spawned without a
//! shell: `gh` for the issues of GitHub, `acli` (Atlassian's) for Jira. Flottille never reads,
//! keeps nor prints a token, and of each one's sign-in status it reads the exit code alone. A
//! write is a `tickets` call the gate let through; on Jira, once `acli` is checked to be signed in
//! to the tracker's site.
//!
//! A workspace's ticket is read on demand and kept nowhere; every ticket of the tracker is also
//! read in the background by the items (`changed`, `docs/items.md`), which `tickets list` and
//! `search` answer from and whose events a subscription to a ticket filters (`events`). Every
//! text kept from a tracker is masked before it is shown or stored.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use flottille_core::protocol::{ProjectTicket, WorkspaceName, WorkspaceTicket};
use flottille_core::store::now_ms;
use flottille_core::ticket::{Key, Tracker};
use flottille_core::workspace::Workspace;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::forge::{self, Checkout, Forge, Gh, masked};
use crate::mcp::{Outcome, arguments, full_detail, workspace_named};
use crate::place::Place;
use crate::projects::{self, Failure};

mod events;

pub use events::{EVENTS, offer, reference_of, target_named};

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
    /// The description ended with Flottille's mark: an agent wrote it.
    pub description_by_agent: bool,
    /// Oldest first, the newest [`COMMENTS`] only.
    pub comments: Vec<Comment>,
    /// How many older comments are not kept.
    pub older: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub author: String,
    pub at: Option<String>,
    /// Without the mark of Flottille, when it had one.
    pub body: String,
    /// An agent of Flottille posted it, as its mark says.
    pub agent: bool,
}

impl Comment {
    /// Its author, and that an agent posted it when one did.
    fn by(&self) -> String {
        match self.agent {
            true => format!("{} ({})", self.author, forge::BY_AGENT),
            false => self.author.clone(),
        }
    }
}

/// A comment read from a tracker: its text without the mark of Flottille, and whether it had one.
fn comment_of(author: String, at: Option<&str>, text: &str) -> Comment {
    let (body, agent) = forge::unmarked(text);
    Comment {
        author,
        at: at.map(str::to_owned),
        body: cut(body, COMMENT_CHARS),
        agent,
    }
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

    /// What `tickets show` answers: the comments themselves only when `full`, otherwise their
    /// number and the way to get them, for the ticket that was named when one was.
    fn text(&self, full: bool, named: Option<&str>) -> String {
        let again = match named {
            Some(ticket) => format!(
                "{{\"action\": \"show\", \"ticket\": {}, \"detail\": \"full\"}}",
                Value::from(ticket)
            ),
            None => "{\"action\": \"show\", \"detail\": \"full\"}".to_owned(),
        };
        let place = match (&self.repository, self.tracker) {
            (Some(repository), _) => format!(" of {repository} (GitHub)"),
            (None, _) => " (Jira)".to_owned(),
        };
        // The tracker's own fields may lead the agent to a page; what it asked with never does,
        // nor what an agent wrote: a title (unmarked, it may be any agent's) or a marked text.
        if !self.description_by_agent {
            crate::web::shown(&self.description, crate::web::Source::Link);
        }
        if let Some(url) = &self.url {
            crate::web::shown(url, crate::web::Source::Link);
        }
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
        if !self.comments.is_empty() && !full {
            let older = self.older;
            text.push_str(&format!(
                "\nComments: {}{}, read them with tickets {again}\n",
                self.comments.len() + older,
                self.comments.last().map_or(String::new(), |last| format!(
                    ", the newest by {}{}",
                    last.by(),
                    last.at
                        .as_deref()
                        .map_or(String::new(), |at| format!(" on {at}"))
                ))
            ));
        } else if !self.comments.is_empty() {
            text.push_str(&format!("\nComments ({}):\n", self.comments.len()));
            if self.older > 0 {
                text.push_str(&format!("  ({} older ones are not shown)\n", self.older));
            }
            for comment in &self.comments {
                if !comment.agent {
                    crate::web::shown(&comment.body, crate::web::Source::Link);
                }
                let at = comment
                    .at
                    .as_deref()
                    .map_or(String::new(), |at| format!(", {at}"));
                text.push_str(&format!(
                    "- {}{at}: {}\n",
                    comment.by(),
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

/// What is said of a project whose tracker is not declared: no ticket is linked nor read.
pub const NO_TRACKER: &str = "no tracker set for this project: choose it in the project's \
    Settings (the window), or write its [tracker] in the project's file; Flottille never guesses it";

/// The ticket `text` as the project's tracker takes it (`#123`, `ABC-123`), or why it is not one
/// of its tickets: written otherwise, of another tracker, of another Jira project, or no tracker.
pub fn accepted(project: &Project, text: &str) -> Result<Key, String> {
    let key = Key::parse(text)?;
    let tracker = project.tracker.as_ref().ok_or(NO_TRACKER)?;
    tracker.accepts(&key)?;
    Ok(key)
}

/// Reads `key` on the project's tracker. `Err` says why it cannot be read, masked, with what to
/// do.
pub async fn read(project: &Project, key: &Key) -> Result<Ticket, String> {
    let tracker = project.tracker.as_ref().ok_or(NO_TRACKER)?;
    tracker.accepts(key)?;
    match (tracker, key) {
        (ProjectTracker::Github { repository }, Key::Issue(number)) => {
            let forge = github_of(project, repository).await?;
            read_issue(&forge, *number).await
        }
        (ProjectTracker::Jira { site, .. }, Key::Jira(key)) => read_work_item(key, site).await,
        _ => unreachable!("a key the tracker accepts"),
    }
}

/// The GitHub repository whose issues the tracker reads: the forge the project declares for its
/// repository `path`, once the remote of its main checkout is checked to lead there.
async fn github_of(project: &Project, path: &str) -> Result<Forge, String> {
    let repository = project
        .repositories
        .iter()
        .find(|r| r.path == path)
        .ok_or_else(|| format!("the tracker's repository {path} is not one of the project's"))?;
    let checkout = Checkout {
        path: path.to_owned(),
        folder: match path {
            "." => project.folder.clone(),
            path => project.folder.join(path),
        },
        remote: repository.remote.clone(),
        declared: repository.forge.clone(),
    };
    match forge::declared_forge(&checkout).await {
        Ok(forge @ Forge::GitHub { .. }) => Ok(forge),
        Ok(_) | Err(None) => Err(format!(
            "the tracker's repository {path} is not declared on GitHub: set its forge in the \
             project's settings"
        )),
        Err(Some(why)) => Err(why),
    }
}

async fn read_issue(forge: &Forge, number: u64) -> Result<Ticket, String> {
    forge::signed_in().await?;
    let slug = forge.slug();
    let issue = forge::gh_json(&[
        "issue",
        "view",
        &number.to_string(),
        "-R",
        &forge.gh_repository(),
        "--json",
        GH_FIELDS,
    ])
    .await?;
    let comments: Vec<Comment> = issue["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|comment| {
            comment_of(
                said(&comment["author"]["login"]),
                comment["createdAt"].as_str(),
                &said(&comment["body"]),
            )
        })
        .collect();
    let body = said(&issue["body"]);
    let (description, description_by_agent) = forge::unmarked(&body);
    Ok(Ticket {
        reference: format!("#{number}"),
        tracker: Tracker::GitHub,
        repository: Some(slug),
        title: said(&issue["title"]),
        state: said(&issue["state"]).to_lowercase(),
        url: issue["url"].as_str().map(masked),
        description: cut(description, DESCRIPTION_CHARS),
        description_by_agent,
        older: comments.len().saturating_sub(COMMENTS),
        comments: newest(comments),
    })
}

fn newest(mut comments: Vec<Comment>) -> Vec<Comment> {
    let older = comments.len().saturating_sub(COMMENTS);
    comments.drain(..older);
    comments
}

/// The site `acli`'s active sign-in is on: the `Site:` line of `acli jira auth status`, nothing
/// else of what it prints (an e-mail may be there: never kept).
fn signed_in_site(status: &str) -> Option<String> {
    status.lines().find_map(|line| {
        let (label, site) = line.trim().split_once(':')?;
        let site = site
            .trim()
            .trim_start_matches("https://")
            .trim_end_matches('/');
        (label.trim().eq_ignore_ascii_case("site") && !site.is_empty())
            .then(|| site.to_ascii_lowercase())
    })
}

/// That `acli` is there, signed in, and to the tracker's site: nothing is asked of Jira
/// otherwise. `nothing` says what was not done then: `read` or `written`.
async fn jira_site(site: &str, nothing: &str) -> Result<(), String> {
    let status = match acli(&["jira", "auth", "status"]).await {
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
        Ok((true, stdout, _)) => stdout,
    };
    match signed_in_site(&status) {
        Some(active) if active == site => {}
        Some(active) => {
            return Err(format!(
                "acli is signed in to {}, and this project's tracker is Jira on {site}: nothing \
                 was {nothing}; switch acli to it (`acli jira auth switch --site {site}`) or \
                 correct the project's tracker",
                masked(&active)
            ));
        }
        None => {
            return Err(format!(
                "acli did not say which site it is signed in to (no Site: line in `acli jira \
                 auth status`), so it cannot be checked to be {site}: nothing was {nothing}"
            ));
        }
    }
    Ok(())
}

async fn read_work_item(key: &str, site: &str) -> Result<Ticket, String> {
    jira_site(site, "read").await?;
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
    // The `Site:` line said where the sign-in is; the item itself may name another site.
    match site_of(&item) {
        Some(from) if answered_by(&item, &from, site) => Ok(work_item(key, &item, site)),
        Some(from) => Err(format!(
            "acli answered {key} from {}, and this project's tracker is Jira on {site}: nothing \
             of it is shown; check the site acli is signed in to (`acli jira auth status`)",
            masked(&from)
        )),
        None => Err(format!(
            "acli answered {key} without saying its site (no self link), so it cannot be checked \
             to be {site}: nothing of it is shown"
        )),
    }
}

/// The site a work item comes from: the host of its `self` link, its REST address, lowercase.
/// `None` when the item has no `self` or it cannot be parsed.
fn site_of(item: &Value) -> Option<String> {
    let link = item["self"].as_str()?;
    let rest = link
        .strip_prefix("https://")
        .or_else(|| link.strip_prefix("http://"))?;
    rest.split('/')
        .next()
        .filter(|host| !host.is_empty())
        .map(str::to_ascii_lowercase)
}

/// Whether a work item whose `self` link is on `host` may be the tracker's `site` answering. With
/// an API token `acli` answers on the site's own host; signed in with OAuth, on an Atlassian host
/// that names no site (`*.atl-paas.net`, or `api.atlassian.com/ex/jira/<cloud id>`), which says
/// nothing of which site answered. That guarantee is the active sign-in's, checked before any
/// read (`jira_site`); what is refused here is a link that names another site.
fn answered_by(item: &Value, host: &str, site: &str) -> bool {
    let gateway = item["self"].as_str().is_some_and(|link| {
        link.to_ascii_lowercase()
            .starts_with("https://api.atlassian.com/ex/jira/")
    });
    host == site || host.ends_with(".atl-paas.net") || gateway
}

/// A Jira work item as `acli jira workitem view --json` prints it: Jira's REST shape (`key`,
/// `self`, `fields`). Its description and comments are Atlassian Document Format, or plain text.
fn work_item(key: &str, item: &Value, site: &str) -> Ticket {
    let fields = &item["fields"];
    let comments: Vec<Comment> = fields["comment"]["comments"]
        .as_array()
        .or(fields["comment"].as_array())
        .into_iter()
        .flatten()
        .map(|comment| {
            comment_of(
                said(&comment["author"]["displayName"]),
                comment["created"].as_str(),
                &masked(&document_text(&comment["body"])),
            )
        })
        .collect();
    let description = masked(&document_text(&fields["description"]));
    let (description, description_by_agent) = forge::unmarked(&description);
    Ticket {
        reference: key.to_owned(),
        tracker: Tracker::Jira,
        repository: None,
        title: said(&fields["summary"]),
        state: said(&fields["status"]["name"]),
        // The configured site: the host of `self` may be an Atlassian one that is no site.
        url: Some(masked(&format!("https://{site}/browse/{key}"))),
        description: cut(description, DESCRIPTION_CHARS),
        description_by_agent,
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
async fn state_of(daemon: &Daemon, workspace: &Workspace) -> Value {
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
    let read = match projects::load_project(daemon, &workspace.project) {
        Err(failure) => Err(failure.message),
        Ok(project) => match Key::parse(text) {
            Ok(key) => read(&project, &key).await,
            Err(why) => Err(why),
        },
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
    Ok(state_of(&daemon, &workspace).await)
}

/// `workspace.link_ticket`: links the workspace to a ticket, or to none; an event says it.
pub async fn link(daemon: Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let asked: WorkspaceTicket = serde_json::from_value(params)?;
    let workspace = find(&daemon, &asked.project, &asked.name)?;
    let project = projects::load_project(&daemon, &asked.project)?;
    let ticket = asked
        .ticket
        .as_deref()
        .map(|text| accepted(&project, text).map(|key| key.text()))
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
    Ok(state_of(&daemon, &workspace).await)
}

/// `project.ticket`: a ticket of the project's tracker, read now, before any workspace is linked
/// to it: what the new-workspace form shows as the user types.
pub async fn preview(daemon: Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let asked: ProjectTicket = serde_json::from_value(params)?;
    let project = projects::load_project(&daemon, &asked.project)?;
    let mut state = json!({
        "project": project.name,
        "ticket": asked.ticket,
        "tracker": project.tracker.as_ref().map(ProjectTracker::kind),
        "tracker_said": project.tracker.as_ref().map(ProjectTracker::said),
        "problem": null,
    });
    let read = match accepted(&project, &asked.ticket) {
        Ok(key) => {
            state["ticket"] = key.text().into();
            read(&project, &key).await
        }
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
    Ok(state)
}

/// A call of `tickets`: its action and what the action takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketsCall {
    action: String,
    ticket: Option<String>,
    workspace: Option<String>,
    query: Option<String>,
    state: Option<String>,
    label: Option<String>,
    assignee: Option<String>,
    detail: Option<String>,
    title: Option<String>,
    body: Option<String>,
    labels: Option<Vec<String>>,
    /// `status`: the status to move to.
    status: Option<String>,
    /// A Jira creation: the tracker's project, and the work item's type.
    project: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    reason: Option<String>,
    remove_labels: Option<Vec<String>>,
    assignees: Option<Vec<String>>,
    remove_assignees: Option<Vec<String>>,
    /// subscribe: the events it waits for (`events::EVENTS`).
    events: Option<Vec<String>>,
    /// subscribe: whose comments count (`subscriptions::FROM`).
    from: Option<String>,
    /// subscribe: how long it waits at most.
    minutes: Option<u64>,
}

/// The writes of `tickets`.
const WRITES: [&str; 6] = ["create", "comment", "edit", "close", "reopen", "status"];

/// Why a write of `tickets` cannot be done on the project's tracker, before anyone is asked: a
/// ticket or a Jira project the tracker does not take, a status not named.
pub fn unwritable(project: &Project, args: &Value) -> Option<String> {
    let action = args["action"].as_str()?;
    if !WRITES.contains(&action) {
        return None;
    }
    let tracker = project.tracker.as_ref()?;
    if let Some(Err(why)) = args["ticket"].as_str().map(|text| accepted(project, text)) {
        return Some(format!("refused: {why}"));
    }
    let named = args["status"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty());
    match tracker {
        ProjectTracker::Jira { projects, .. } if action == "create" => {
            created_in(tracker, projects, args["project"].as_str())
                .err()
                .map(|why| format!("refused: {why}"))
        }
        _ if action == "status" && !named => {
            Some("refused: tickets status needs status, the name of the status to move to".into())
        }
        // What `acli` is never given: a label, an assignee and a close reason are GitHub's.
        ProjectTracker::Jira { .. } => [
            "reason",
            "labels",
            "remove_labels",
            "assignees",
            "remove_assignees",
        ]
        .into_iter()
        .find(|key| action != "create" && !args[key].is_null())
        .map(|key| format!("refused: tickets {action} on Jira takes no {key}: it is GitHub's")),
        _ => None,
    }
}

/// The Jira project a creation goes to: the one named, or the tracker's only one.
fn created_in<'a>(
    tracker: &ProjectTracker,
    projects: &'a [String],
    named: Option<&str>,
) -> Result<&'a str, String> {
    match named.map(str::trim) {
        Some(name) => projects
            .iter()
            .find(|p| p.eq_ignore_ascii_case(name))
            .map(String::as_str)
            .ok_or_else(|| {
                format!(
                    "{name} is not a project of this project's tracker ({})",
                    tracker.said()
                )
            }),
        None if projects.len() == 1 => Ok(&projects[0]),
        None => Err(format!(
            "tickets create on Jira needs project, one of the tracker's: {}",
            projects.join(", ")
        )),
    }
}

/// `tickets`, once the gate let it through: the tickets of the project's tracker, listed,
/// searched, read and written through the user's own `gh` or `acli`. Never another tracker's.
pub async fn tickets(daemon: &Arc<Daemon>, agent: &str, place: &Place, args: Value) -> Outcome {
    let asked: TicketsCall = match arguments("tickets", args.clone()) {
        Ok(asked) => asked,
        Err(refusal) => return refusal,
    };
    let project = match projects::load_project(daemon, &place.project) {
        Ok(project) => project,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    if let Some(refusal) = unwritable(&project, &args) {
        return Outcome::Refused(refusal);
    }
    match asked.action.as_str() {
        "list" | "search" => listed(daemon, &project, &asked).await,
        "show" => show(daemon, place, &project, &asked).await,
        "subscribe" => events::subscribe(daemon, agent, place, &project, &asked).await,
        "unsubscribe" => events::unsubscribe(daemon, agent, &project, &asked),
        "create" => create(&project, &asked).await,
        "comment" | "edit" | "close" | "reopen" | "status" => {
            let key = match key_of(daemon, place, &project, &asked) {
                Ok(key) => key,
                Err(refusal) => return refusal,
            };
            match asked.action.as_str() {
                "status" => status(&project, &asked, &key).await,
                _ => write(&project, &asked, &key).await,
            }
        }
        action => Outcome::Refused(format!(
            "refused: tickets has no action {action:?}: its actions are {}",
            crate::gate::TICKET_ACTIONS
        )),
    }
}

/// How many tickets one list asks for: more than a project keeps open, said when reached.
const LISTED: usize = 200;

/// A ticket of a list.
struct Listed {
    reference: String,
    state: String,
    title: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    url: Option<String>,
}

/// `tickets list` and `tickets search`: every ticket of a state, by label, assignee and, for a
/// search, words, in one call.
async fn listed(daemon: &Daemon, project: &Project, asked: &TicketsCall) -> Outcome {
    let state = asked.state.as_deref().unwrap_or("open");
    if !["open", "closed", "all"].contains(&state) {
        return Outcome::Refused(format!(
            "refused: tickets has no state {state:?}: its states are open (the default), closed \
             and all"
        ));
    }
    let query = match (asked.action.as_str(), asked.query.as_deref()) {
        ("search", Some(query)) if !query.trim().is_empty() => Some(query.trim()),
        ("search", _) => {
            return Outcome::Refused(
                "refused: tickets search needs query, the words to find; list takes none".into(),
            );
        }
        _ => None,
    };
    let Some(tracker) = &project.tracker else {
        return Outcome::Refused(format!("refused: {NO_TRACKER}"));
    };
    if let (ProjectTracker::Github { .. }, Some(query)) = (tracker, query) {
        if reaches(query) {
            return Outcome::Refused(
                "refused: tickets search looks in the tracker's repository only: its query \
                 cannot hold repo:, org:, user:, owner:, is:pr or type:pr; give the words alone"
                    .into(),
            );
        }
        // The flags are joined into GitHub's search query by `gh`: a value with a quote or a
        // colon could carry a qualifier of its own.
        for (flag, value) in [("label", &asked.label), ("assignee", &asked.assignee)] {
            if value.as_deref().is_some_and(|v| v.contains(['"', ':'])) {
                return Outcome::Refused(format!(
                    "refused: tickets search takes a {flag} without a quote or a colon: they \
                     could add a qualifier that leaves the tracker's repository"
                ));
            }
        }
    }
    let filters = Filters {
        state,
        label: asked.label.as_deref(),
        assignee: asked.assignee.as_deref(),
        query,
    };
    // What the items' rounds keep, once one read the tracker; the tracker itself before.
    let cursor = daemon
        .read(|store| store.cursors(&project.name))
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.kind == "ticket" && c.cursor.is_some());
    let found = match (&cursor, tracker) {
        (Some(_), _) => Ok((stored_tickets(daemon, project, &filters), 0)),
        (None, ProjectTracker::Github { repository }) => match github_of(project, repository).await
        {
            Ok(forge) => list_issues(&forge, &filters).await,
            Err(why) => Err(why),
        },
        (None, ProjectTracker::Jira { site, projects }) => {
            list_work_items(site, projects, &filters).await
        }
    };
    let (tickets, left_out) = match found {
        Ok(found) => found,
        Err(why) => return Outcome::Failed(format!("the tickets could not be listed: {why}")),
    };
    let head = match state {
        "open" => "The open tickets",
        "closed" => "The closed tickets",
        _ => "All tickets",
    };
    // Every filter the list was asked with is said: a list filtered by a label must not read
    // like the whole list.
    let mut header = format!("{head} of {}", tracker.said());
    if let Some(label) = filters.label {
        header.push_str(&format!(" labelled {label}"));
    }
    if let Some(assignee) = filters.assignee {
        header.push_str(&format!(", assigned to {assignee}"));
    }
    if let Some(query) = query {
        header.push_str(&format!(" matching {query:?}"));
    }
    let mut text = format!("{header}: {}", tickets.len());
    if let Some(cursor) = &cursor {
        text.push_str(&format!(
            " ({})",
            crate::items::as_of(cursor.cursor.unwrap_or_default())
        ));
        if let Some(problem) = &cursor.problem {
            text.push_str(&format!("; its last read failed: {problem}"));
        }
    }
    text.push('\n');
    for ticket in &tickets {
        if let Some(url) = &ticket.url {
            crate::web::shown(url, crate::web::Source::Link);
        }
        let mut line = format!("- {} {}: {}", ticket.reference, ticket.state, ticket.title);
        if !ticket.labels.is_empty() {
            line.push_str(&format!(" [{}]", ticket.labels.join(", ")));
        }
        if !ticket.assignees.is_empty() {
            line.push_str(&format!(" ({})", ticket.assignees.join(", ")));
        }
        if let Some(url) = &ticket.url {
            line.push_str(&format!("  {url}"));
        }
        text.push_str(&line);
        text.push('\n');
    }
    if tickets.len() >= LISTED {
        text.push_str(&format!(
            "(the newest {LISTED}: narrow with label, assignee or a search)\n"
        ));
    }
    // GitHub's count is not given: it would tell how many issues of other repositories matched.
    // Jira's filter is by site, not by hidden repositories.
    if left_out > 0 && matches!(tracker, ProjectTracker::Jira { .. }) {
        text.push_str(&format!(
            "({left_out} answered from another site than the tracker's are left out)\n"
        ));
    }
    text.push_str("tickets {\"action\": \"show\", \"ticket\": …} reads one.");
    Outcome::Done(text)
}

/// The tickets the items' rounds keep, as a list keeps them: by state, label, assignee and, for a
/// search, the words of their title, the most recently changed first.
fn stored_tickets(daemon: &Daemon, project: &Project, filters: &Filters<'_>) -> Vec<Listed> {
    let kind = flottille_core::item::Filter {
        kind: Some("ticket".into()),
        ..Default::default()
    };
    let words: Vec<String> = filters
        .query
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let named = |names: &[String], asked: Option<&str>| {
        asked.is_none_or(|asked| names.iter().any(|n| n.eq_ignore_ascii_case(asked)))
    };
    daemon
        .read(|store| store.items(&project.name, &kind))
        .unwrap_or_default()
        .into_iter()
        .filter(|item| filters.state == "all" || item.state == filters.state)
        .filter(|item| named(&item.labels, filters.label))
        .filter(|item| named(&item.assignees, filters.assignee))
        .filter(|item| {
            let title = item.title.to_lowercase();
            words.iter().all(|word| title.contains(word))
        })
        .take(LISTED)
        .map(|item| Listed {
            reference: match item.repository.is_empty() {
                true => item.key.clone(),
                false => format!("#{}", item.key),
            },
            state: item.details["status"]
                .as_str()
                .map_or_else(|| item.state.clone(), str::to_owned),
            title: item.title,
            labels: item.labels,
            assignees: item.assignees,
            url: item.url,
        })
        .collect()
}

/// The qualifiers of GitHub's search that reach beyond the tracker's repository, or to pull
/// requests, refused in a search's query.
const REACHING: [&str; 6] = ["repo:", "org:", "user:", "owner:", "is:pr", "type:pr"];

/// Whether a word of the query starts with one of them: at its start, or after a space, an
/// opening parenthesis or `-`, which is where GitHub reads a qualifier.
fn reaches(query: &str) -> bool {
    let lower = query.to_lowercase();
    let mut word_start = true;
    for (at, c) in lower.char_indices() {
        if word_start && REACHING.iter().any(|q| lower[at..].starts_with(q)) {
            return true;
        }
        word_start = c.is_whitespace() || c == '(' || c == '-';
    }
    false
}

/// What a list keeps.
struct Filters<'a> {
    state: &'a str,
    label: Option<&'a str>,
    assignee: Option<&'a str>,
    query: Option<&'a str>,
}

/// `gh issue list` of the tracker's repository.
async fn list_issues(forge: &Forge, filters: &Filters<'_>) -> Result<(Vec<Listed>, usize), String> {
    forge::signed_in().await?;
    let repo = forge.gh_repository();
    let limit = LISTED.to_string();
    let mut args = vec![
        "issue",
        "list",
        "-R",
        &repo,
        "--state",
        filters.state,
        "--limit",
        &limit,
    ];
    for (flag, value) in [
        ("--label", filters.label),
        ("--assignee", filters.assignee),
        ("--search", filters.query),
    ] {
        if let Some(value) = value {
            args.extend([flag, value]);
        }
    }
    args.extend(["--json", "number,title,state,labels,assignees,url"]);
    let issues = forge::gh_json(&args).await?;
    let names = |list: &Value, key: &str| -> Vec<String> {
        list.as_array()
            .into_iter()
            .flatten()
            .map(|item| said(&item[key]))
            .filter(|name| !name.is_empty())
            .collect()
    };
    // Only the tracker's repository's issues are kept, whatever `gh` answered.
    let ours = format!("https://{repo}/issues/").to_lowercase();
    let all = issues.as_array().cloned().unwrap_or_default();
    let (kept, other): (Vec<Value>, Vec<Value>) = all.into_iter().partition(|issue| {
        issue["url"]
            .as_str()
            .is_some_and(|url| url.to_lowercase().starts_with(&ours))
    });
    let tickets = kept
        .iter()
        .filter_map(|issue| {
            Some(Listed {
                reference: format!("#{}", issue["number"].as_u64()?),
                state: said(&issue["state"]).to_lowercase(),
                title: said(&issue["title"]),
                labels: names(&issue["labels"], "name"),
                assignees: names(&issue["assignees"], "login"),
                url: issue["url"].as_str().map(masked),
            })
        })
        .collect();
    Ok((tickets, other.len()))
}

/// A text as JQL quotes it.
fn jql_quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The JQL of a list: the tracker's projects, the state as Jira's status category, and the
/// filters, each quoted.
fn jql(projects: &[String], filters: &Filters<'_>) -> String {
    let projects: Vec<String> = projects.iter().map(|p| jql_quoted(p)).collect();
    let mut clauses = vec![format!("project in ({})", projects.join(", "))];
    match filters.state {
        "open" => clauses.push("statusCategory != Done".into()),
        "closed" => clauses.push("statusCategory = Done".into()),
        _ => {}
    }
    if let Some(label) = filters.label {
        clauses.push(format!("labels = {}", jql_quoted(label)));
    }
    if let Some(assignee) = filters.assignee {
        clauses.push(format!("assignee = {}", jql_quoted(assignee)));
    }
    if let Some(query) = filters.query {
        clauses.push(format!("text ~ {}", jql_quoted(query)));
    }
    format!("{} ORDER BY created DESC", clauses.join(" AND "))
}

/// `acli jira workitem search` on the tracker's projects, once `acli` is checked to be signed in
/// to the tracker's site. An item whose `self` names another site, or cannot be read, is left
/// out and counted; one that has no `self` is kept.
async fn list_work_items(
    site: &str,
    projects: &[String],
    filters: &Filters<'_>,
) -> Result<(Vec<Listed>, usize), String> {
    jira_site(site, "read").await?;
    let query = jql(projects, filters);
    let limit = LISTED.to_string();
    let (ok, stdout, stderr) = match acli(&[
        "jira",
        "workitem",
        "search",
        "--jql",
        &query,
        "--fields",
        "key,summary,status,assignee,labels",
        "--json",
        "--limit",
        &limit,
    ])
    .await
    {
        Ok(output) => output,
        Err(Gh::Missing) => return Err("acli is not on the daemon's PATH".into()),
        Err(Gh::Failed(why)) => return Err(format!("acli jira workitem search: {why}")),
    };
    if !ok {
        let said = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(format!(
            "acli jira workitem search failed: {}",
            masked(&said.chars().take(300).collect::<String>())
        ));
    }
    let found: Value = serde_json::from_str(&stdout)
        .map_err(|error| format!("acli jira workitem search answered what is not JSON: {error}"))?;
    // Assumed: the items as an array, or under `issues` as Jira's REST search gives them.
    let items = found
        .as_array()
        .or(found["issues"].as_array())
        .cloned()
        .unwrap_or_default();
    let mut left_out = 0;
    let mut tickets = Vec::new();
    for item in &items {
        // Only an item with no `self` at all is kept: the search was of the signed-in site's
        // projects. One that has a `self` naming another site, or that cannot be read, is not.
        let unlinked = item.get("self").is_none_or(Value::is_null);
        if !unlinked && site_of(item).is_none_or(|from| !answered_by(item, &from, site)) {
            left_out += 1;
            continue;
        }
        let fields = &item["fields"];
        let Some(key) = item["key"].as_str() else {
            continue;
        };
        tickets.push(Listed {
            reference: masked(key),
            state: said(&fields["status"]["name"]),
            title: said(&fields["summary"]),
            labels: fields["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .map(said)
                .collect(),
            assignees: Some(said(&fields["assignee"]["displayName"]))
                .filter(|name| !name.is_empty())
                .into_iter()
                .collect(),
            url: Some(format!("https://{site}/browse/{}", masked(key))),
        });
    }
    Ok((tickets, left_out))
}

/// The ticket a call names, or the one its workspace (the agent's own, or the one named) is
/// linked to; a key of another tracker is refused.
fn key_of(
    daemon: &Daemon,
    place: &Place,
    project: &Project,
    asked: &TicketsCall,
) -> Result<Key, Outcome> {
    // A ticket named needs no workspace; without one, the workspace's own ticket is meant.
    let by_name = asked.ticket.is_some() && place.workspace.is_none() && asked.workspace.is_none();
    let linked = if by_name {
        None
    } else {
        let tool = format!("tickets {}", asked.action);
        let name = workspace_named(daemon, place, asked.workspace.as_deref(), &tool)?;
        match daemon.read(|store| store.workspace(&place.project, &name)) {
            Ok(Some(workspace)) => Some(workspace),
            Ok(None) => return Err(Outcome::Failed(format!("the workspace {name} is gone"))),
            Err(error) => return Err(Outcome::Failed(error.to_string())),
        }
    };
    match (
        &asked.ticket,
        linked.as_ref().and_then(|w| w.ticket.as_ref()),
    ) {
        (Some(text), _) | (None, Some(text)) => {
            accepted(project, text).map_err(|why| Outcome::Refused(format!("refused: {why}")))
        }
        (None, None) => Err(Outcome::Refused(format!(
            "refused: workspace {} is linked to no ticket: name one of the project's tracker, \
             or ask the user to link it",
            linked.map_or_else(String::new, |w| w.name)
        ))),
    }
}

/// `tickets show`: the ticket of the agent's own workspace, or another ticket of the same
/// tracker.
async fn show(
    daemon: &Arc<Daemon>,
    place: &Place,
    project: &Project,
    asked: &TicketsCall,
) -> Outcome {
    let full = match full_detail("tickets", asked.detail.as_deref()) {
        Ok(full) => full,
        Err(refusal) => return refusal,
    };
    let key = match key_of(daemon, place, project, asked) {
        Ok(key) => key,
        Err(refusal) => return refusal,
    };
    // A round of the project running stores a read of its own; none starts during this one.
    let held = crate::items::held_now(daemon, &project.name);
    match read(project, &key).await {
        Ok(ticket) => {
            if held.is_some() {
                events::remember(daemon, project, &key, &ticket);
            }
            Outcome::Done(ticket.text(full, asked.ticket.as_deref()))
        }
        Err(problem) => Outcome::Failed(format!(
            "the ticket {} could not be read: {problem}",
            key.text()
        )),
    }
}

/// The GitHub repository of the project's tracker, for a write: a Jira one writes through `acli`.
async fn issues_of(project: &Project) -> Result<Forge, Outcome> {
    match &project.tracker {
        Some(ProjectTracker::Github { repository }) => github_of(project, repository)
            .await
            .map_err(|why| Outcome::Refused(format!("refused: {why}; nothing was written"))),
        Some(ProjectTracker::Jira { .. }) => unreachable!("a Jira tracker writes through acli"),
        None => Err(Outcome::Refused(format!("refused: {NO_TRACKER}"))),
    }
}

/// `tickets create`: a new issue of the tracker's repository, or a work item of one of its Jira
/// projects.
async fn create(project: &Project, asked: &TicketsCall) -> Outcome {
    let Some(title) = asked.title.as_deref().filter(|t| !t.trim().is_empty()) else {
        return Outcome::Refused("refused: tickets create needs title".into());
    };
    if let Some(tracker @ ProjectTracker::Jira { site, projects }) = &project.tracker {
        return create_work_item(tracker, site, projects, title, asked).await;
    }
    let forge = match issues_of(project).await {
        Ok(forge) => forge,
        Err(refusal) => return refusal,
    };
    if let Err(problem) = forge::signed_in().await {
        return Outcome::Failed(problem);
    }
    let repo = forge.gh_repository();
    let body = forge::marked(asked.body.as_deref().unwrap_or_default());
    let mut args = vec![
        "issue", "create", "-R", &repo, "--title", title, "--body", &body,
    ];
    for label in asked.labels.iter().flatten() {
        args.extend(["--label", label.as_str()]);
    }
    match forge::gh_text(&args).await {
        Ok(said) => Outcome::Done(format!(
            "The ticket was opened in {}: {}",
            forge.slug(),
            said.trim()
        )),
        Err(why) => Outcome::Failed(why),
    }
}

/// `tickets comment`, `edit`, `close`, `reopen` and `status` of an issue or a work item.
async fn write(project: &Project, asked: &TicketsCall, key: &Key) -> Outcome {
    let action = asked.action.as_str();
    let body = asked.body.as_deref().filter(|b| !b.trim().is_empty());
    let title = asked.title.as_deref().filter(|t| !t.trim().is_empty());
    let named = |list: &'_ Option<Vec<String>>| -> Vec<String> {
        list.iter()
            .flatten()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    };
    let flagged = [
        ("--add-label", named(&asked.labels)),
        ("--remove-label", named(&asked.remove_labels)),
        ("--add-assignee", named(&asked.assignees)),
        ("--remove-assignee", named(&asked.remove_assignees)),
    ];
    match (action, body, title) {
        ("comment", None, _) => {
            return Outcome::Refused("refused: tickets comment needs body, the comment".into());
        }
        ("edit", None, None) if flagged.iter().all(|(_, names)| names.is_empty()) => {
            return Outcome::Refused(
                "refused: tickets edit needs title, body, labels or assignees: what changes".into(),
            );
        }
        _ => {}
    }
    let Key::Issue(number) = key else {
        let Some(ProjectTracker::Jira { site, .. }) = &project.tracker else {
            unreachable!("a key the tracker accepts");
        };
        return write_work_item(site, &key.text(), asked, body, title).await;
    };
    let forge = match issues_of(project).await {
        Ok(forge) => forge,
        Err(refusal) => return refusal,
    };
    if let Err(problem) = forge::signed_in().await {
        return Outcome::Failed(problem);
    }
    let repo = forge.gh_repository();
    let number = number.to_string();
    let mut args = vec!["issue", action, &number, "-R", &repo];
    // `gh issue comment` takes no title, and the gate refused one there.
    if let Some(title) = title.filter(|_| action == "edit") {
        args.extend(["--title", title]);
    }
    let marked = body.map(forge::marked);
    if let Some(body) = marked
        .as_deref()
        .filter(|_| action == "comment" || action == "edit")
    {
        args.extend(["--body", body]);
    }
    if action == "edit" {
        for (flag, names) in &flagged {
            for name in names {
                args.extend([*flag, name.as_str()]);
            }
        }
    }
    let reason = asked.reason.as_deref().filter(|_| action == "close");
    if let Some(reason) = reason {
        args.extend(["--reason", reason]);
    }
    let slug = forge.slug();
    match forge::gh_text(&args).await {
        Ok(_) => Outcome::Done(match action {
            "comment" => format!("The comment was posted on #{number} of {slug}."),
            "edit" => format!("#{number} of {slug} was changed."),
            "close" => match reason {
                Some(reason) => format!("#{number} of {slug} was closed as {reason}."),
                None => format!("#{number} of {slug} was closed."),
            },
            _ => format!("#{number} of {slug} was reopened."),
        }),
        Err(why) => Outcome::Failed(why),
    }
}

/// `acli <args>` for a write, or a search a write needs: what it printed, or why it failed,
/// masked.
async fn acli_run(args: &[&str]) -> Result<String, String> {
    let what = format!("acli {}", args[..3].join(" "));
    match acli(args).await {
        Ok((true, stdout, _)) => Ok(stdout),
        Ok((false, stdout, stderr)) => {
            let said = stderr
                .lines()
                .chain(stdout.lines())
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            Err(format!(
                "{what} failed: {}",
                masked(&said.chars().take(300).collect::<String>())
            ))
        }
        Err(Gh::Missing) => Err("acli is not on the daemon's PATH".into()),
        Err(Gh::Failed(why)) => Err(format!("{what}: {why}")),
    }
}

/// `tickets create` on Jira: a work item of one of the tracker's projects, of the type named or
/// the project's default one.
async fn create_work_item(
    tracker: &ProjectTracker,
    site: &str,
    projects: &[String],
    title: &str,
    asked: &TicketsCall,
) -> Outcome {
    let project = match created_in(tracker, projects, asked.project.as_deref()) {
        Ok(project) => project,
        Err(why) => return Outcome::Refused(format!("refused: {why}")),
    };
    if let Err(problem) = jira_site(site, "written").await {
        return Outcome::Failed(problem);
    }
    let labels = asked.labels.as_deref().unwrap_or_default().join(",");
    let mut args = vec![
        "jira",
        "workitem",
        "create",
        "--project",
        project,
        "--summary",
        title,
    ];
    let footed = asked
        .body
        .as_deref()
        .filter(|b| !b.trim().is_empty())
        .map(forge::footed);
    if let Some(body) = footed.as_deref() {
        args.extend(["--description", body]);
    }
    if let Some(kind) = asked.kind.as_deref().filter(|k| !k.trim().is_empty()) {
        args.extend(["--type", kind]);
    }
    if !labels.is_empty() {
        args.extend(["--label", &labels]);
    }
    args.push("--json");
    let said = match acli_run(&args).await {
        Ok(said) => said,
        Err(why) => return Outcome::Failed(why),
    };
    // Assumed: the created work item as Jira's REST answers it, `{"key": …}`.
    let created = serde_json::from_str::<Value>(&said).ok();
    match created.as_ref().and_then(|item| item["key"].as_str()) {
        Some(key) => {
            let key = masked(key);
            Outcome::Done(format!(
                "The ticket {key} was opened in {project}: https://{site}/browse/{key}"
            ))
        }
        None => Outcome::Done(format!(
            "The ticket was opened in {project}: {}",
            masked(said.trim())
        )),
    }
}

/// `tickets comment`, `edit`, `close`, `reopen` and `status` of a Jira work item, once `acli` is
/// checked to be signed in to the tracker's site.
async fn write_work_item(
    site: &str,
    key: &str,
    asked: &TicketsCall,
    body: Option<&str>,
    title: Option<&str>,
) -> Outcome {
    if let Err(problem) = jira_site(site, "written").await {
        return Outcome::Failed(problem);
    }
    let action = asked.action.as_str();
    let mut args = vec!["jira", "workitem"];
    match action {
        "comment" => args.extend(["comment", "create", "--key", key]),
        "edit" => args.extend(["edit", "--key", key]),
        _ => return transition(site, key, asked).await,
    }
    if let Some(title) = title.filter(|_| action == "edit") {
        args.extend(["--summary", title]);
    }
    let footed = body.map(forge::footed);
    match (action, footed.as_deref()) {
        ("comment", Some(body)) => args.extend(["--body", body]),
        ("edit", Some(body)) => args.extend(["--description", body]),
        _ => {}
    }
    if action == "edit" {
        args.push("--yes");
    }
    match acli_run(&args).await {
        Ok(_) if action == "comment" => Outcome::Done(format!("The comment was posted on {key}.")),
        Ok(_) => Outcome::Done(format!("{key} was changed.")),
        Err(why) => Outcome::Failed(why),
    }
}

/// A status of a Jira project: its name, and its category's key (`new`, `indeterminate`,
/// `done`).
struct Status {
    name: String,
    category: String,
}

/// `a, b and c`.
fn and_listed(names: &[&str]) -> String {
    match names {
        [] => "none".into(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The statuses of a Jira project, as its work items stand: `acli` lists no work item's
/// transitions, so these are what a status named is found among, and what a close or a reopen
/// moves to, never a guessed name.
async fn statuses_of(site: &str, project: &str) -> Result<Vec<Status>, String> {
    let query = format!("project = {} ORDER BY updated DESC", jql_quoted(project));
    let limit = LISTED.to_string();
    let answer = acli_run(&[
        "jira",
        "workitem",
        "search",
        "--jql",
        &query,
        "--fields",
        "key,status",
        "--json",
        "--limit",
        &limit,
    ])
    .await?;
    let found: Value = serde_json::from_str(&answer)
        .map_err(|error| format!("acli jira workitem search answered what is not JSON: {error}"))?;
    let mut statuses: Vec<Status> = Vec::new();
    for item in found
        .as_array()
        .or(found["issues"].as_array())
        .into_iter()
        .flatten()
    {
        let unlinked = item.get("self").is_none_or(Value::is_null);
        if !unlinked && site_of(item).is_none_or(|from| !answered_by(item, &from, site)) {
            continue;
        }
        let status = &item["fields"]["status"];
        let name = said(&status["name"]);
        if name.is_empty() || statuses.iter().any(|s| s.name.eq_ignore_ascii_case(&name)) {
            continue;
        }
        statuses.push(Status {
            name,
            category: status["statusCategory"]["key"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase(),
        });
    }
    Ok(statuses)
}

/// `tickets close`, `reopen` and `status` of a Jira work item: a transition to a status of its
/// project's, the one named, or its only done status, or its only to-do one.
async fn transition(site: &str, key: &str, asked: &TicketsCall) -> Outcome {
    let project = key.rsplit_once('-').map_or(key, |(project, _)| project);
    let statuses = match statuses_of(site, project).await {
        Ok(statuses) => statuses,
        Err(why) => return Outcome::Failed(why),
    };
    let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
    let known = and_listed(&names);
    let target = match asked.action.as_str() {
        "status" => {
            let wanted = asked.status.as_deref().unwrap_or_default().trim();
            match names.iter().find(|name| name.eq_ignore_ascii_case(wanted)) {
                Some(name) => *name,
                None => {
                    return Outcome::Refused(format!(
                        "refused: {} is not a status of {project}'s work items, which are \
                         {known}; nothing was written",
                        masked(wanted)
                    ));
                }
            }
        }
        action => {
            let (category, kind) = match action {
                "close" => ("done", "done"),
                _ => ("new", "to-do"),
            };
            let found: Vec<&str> = statuses
                .iter()
                .filter(|s| s.category == category)
                .map(|s| s.name.as_str())
                .collect();
            match found.as_slice() {
                [one] => *one,
                [] => {
                    return Outcome::Refused(format!(
                        "refused: no work item of {project} is in a {kind} status, so Flottille \
                         cannot tell which status {action}s {key}: move it with status, naming \
                         one of {known}; nothing was written"
                    ));
                }
                several => {
                    return Outcome::Refused(format!(
                        "refused: {project} has several {kind} statuses, {}: move {key} with \
                         status, naming one; nothing was written",
                        and_listed(several)
                    ));
                }
            }
        }
    };
    let args = [
        "jira",
        "workitem",
        "transition",
        "--key",
        key,
        "--status",
        target,
        "--yes",
    ];
    match acli_run(&args).await {
        Ok(_) => Outcome::Done(match asked.action.as_str() {
            "close" => format!("{key} was closed: moved to {target}."),
            "reopen" => format!("{key} was reopened: moved to {target}."),
            _ => format!("{key} was moved to {target}."),
        }),
        Err(why) => Outcome::Failed(format!(
            "acli could not move {key} to {target}: {why}; the statuses of {project}'s work \
             items are {known}"
        )),
    }
}

/// What `gh` says without the `project` scope, which `gh project` and the project items of an
/// issue need: how the user adds it. Flottille never runs it.
fn scoped(why: String) -> String {
    if why.contains("missing required scope") {
        "gh has no project scope: run `gh auth refresh -s project` yourself, then ask again \
         (Flottille never runs it)"
            .into()
    } else {
        why
    }
}

/// `tickets status`: the Status of an issue on one of the GitHub Projects boards it is on, set
/// through `gh project item-edit` with the ids read from the issue and the board's fields.
async fn status(project: &Project, asked: &TicketsCall, key: &Key) -> Outcome {
    let Some(wanted) = asked
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Outcome::Refused(
            "refused: tickets status needs status, the name of a column of the board".into(),
        );
    };
    let Key::Issue(number) = key else {
        return write(project, asked, key).await;
    };
    let forge = match issues_of(project).await {
        Ok(forge) => forge,
        Err(refusal) => return refusal,
    };
    if let Err(problem) = forge::signed_in().await {
        return Outcome::Failed(problem);
    }
    let (repo, slug, number) = (forge.gh_repository(), forge.slug(), number.to_string());
    let owner = slug
        .split_once('/')
        .map_or(slug.as_str(), |(owner, _)| owner);
    let viewed = match forge::gh_json(&[
        "issue",
        "view",
        &number,
        "-R",
        &repo,
        "--json",
        "projectItems",
    ])
    .await
    {
        Ok(viewed) => viewed,
        Err(why) => return Outcome::Failed(scoped(why)),
    };
    let items = viewed["projectItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let title = |item: &Value| said(&item["project"]["title"]);
    let boards = items.iter().map(title).collect::<Vec<_>>().join(", ");
    let item = match (items.len(), asked.project.as_deref().map(str::trim)) {
        (0, _) => {
            return Outcome::Refused(format!(
                "refused: #{number} of {slug} is on no project board: it has no status to set"
            ));
        }
        (1, None) => &items[0],
        (count, None) => {
            return Outcome::Refused(format!(
                "refused: #{number} of {slug} is on {count} project boards: {boards}: name one \
                 with project"
            ));
        }
        (_, Some(named)) => {
            let Some(item) = items.iter().find(|item| {
                title(item).eq_ignore_ascii_case(named) || item["project"]["id"] == named
            }) else {
                return Outcome::Refused(format!(
                    "refused: #{number} of {slug} is not on a project board named {named}: its \
                     boards are {boards}"
                ));
            };
            item
        }
    };
    let (board, board_id, item_id) = (title(item), said(&item["project"]["id"]), said(&item["id"]));
    // `gh project` takes a board's number and its owner; the issue only says the board's id, so
    // the repository's owner's boards are searched for it.
    let listed = match forge::gh_json(&[
        "project", "list", "--owner", owner, "--format", "json", "--limit", "100",
    ])
    .await
    {
        Ok(listed) => listed,
        Err(why) => return Outcome::Failed(scoped(why)),
    };
    let Some(at) = listed["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"] == board_id.as_str())
        .and_then(|p| p["number"].as_u64())
    else {
        return Outcome::Failed(format!(
            "the board {board} is not one of {owner}'s: gh project cannot set a status on a \
             board of another owner"
        ));
    };
    let at = at.to_string();
    let fields = match forge::gh_json(&[
        "project",
        "field-list",
        &at,
        "--owner",
        owner,
        "--format",
        "json",
        "--limit",
        "100",
    ])
    .await
    {
        Ok(fields) => fields,
        Err(why) => return Outcome::Failed(scoped(why)),
    };
    let Some(field) = fields["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| said(&f["name"]).eq_ignore_ascii_case("status") && f["options"].is_array())
    else {
        return Outcome::Failed(format!("the board {board} has no Status field to set"));
    };
    let options = field["options"].as_array().cloned().unwrap_or_default();
    let Some(option) = options
        .iter()
        .find(|o| said(&o["name"]).eq_ignore_ascii_case(wanted))
    else {
        let names: Vec<String> = options.iter().map(|o| said(&o["name"])).collect();
        return Outcome::Refused(format!(
            "refused: {wanted} is no status of {board}: its statuses are {}",
            names.join(", ")
        ));
    };
    let (field_id, option_id, now) = (
        said(&field["id"]),
        said(&option["id"]),
        said(&option["name"]),
    );
    match forge::gh_text(&[
        "project",
        "item-edit",
        "--id",
        &item_id,
        "--project-id",
        &board_id,
        "--field-id",
        &field_id,
        "--single-select-option-id",
        &option_id,
    ])
    .await
    {
        Ok(_) => Outcome::Done(format!("#{number} of {slug} is now {now} on {board}.")),
        Err(why) => Outcome::Failed(scoped(why)),
    }
}

/// What `gh issue list` is asked of each issue a round of the items lists: the cheap fields
/// alone; the comments are read one issue at a time (`Tickets::detail`).
const LIST_FIELDS: &str = "number,title,state,author,assignees,labels,url,updatedAt,createdAt";
/// What a round of the items asks Jira of each work item.
const ITEM_JIRA_FIELDS: &str =
    "key,summary,status,assignee,reporter,labels,issuetype,priority,updated,created";
const ALL_COUNT: usize = 1000;

/// The tickets of a project's tracker as the items read them (`docs/items.md`): its sign-in
/// checked once.
pub enum Tickets {
    GitHub { forge: Forge, repository: String },
    Jira { site: String, projects: Vec<String> },
}

impl Tickets {
    /// Ready to read: GitHub's repository declared and agreeing with its remote, `gh` signed in;
    /// `acli` signed in to the tracker's site.
    pub async fn open(project: &Project) -> Result<Tickets, String> {
        match project.tracker.as_ref().ok_or(NO_TRACKER)? {
            ProjectTracker::Github { repository } => {
                let forge = github_of(project, repository).await?;
                forge::signed_in().await?;
                Ok(Tickets::GitHub {
                    forge,
                    repository: repository.clone(),
                })
            }
            ProjectTracker::Jira { site, projects } => {
                jira_site(site, "read").await?;
                Ok(Tickets::Jira {
                    site: site.clone(),
                    projects: projects.clone(),
                })
            }
        }
    }

    /// The forge whose user `mine` is said of: GitHub's; Jira says it in its own search.
    pub fn forge(&self) -> Option<&Forge> {
        match self {
            Tickets::GitHub { forge, .. } => Some(forge),
            Tickets::Jira { .. } => None,
        }
    }

    /// The tickets that changed since `since` (milliseconds), all of them without it, whatever
    /// their state and whoever opened them, with their cheap fields. An issue stored that GitHub
    /// now lists in another repository (transferred) is no longer the tracker's: it is closed.
    pub async fn list(
        &self,
        since: Option<u64>,
        now: u64,
        stored: &[flottille_core::item::Item],
    ) -> Result<crate::items::Listing, String> {
        let (forge, repository) = match self {
            Tickets::Jira { site, projects } => {
                let mut items = Vec::new();
                let mut notes = Vec::new();
                for key in projects {
                    let (found, cut) = changed_work_items(site, key, since, now).await?;
                    items.extend(found);
                    if cut {
                        notes.push(format!("only {ALL_COUNT} work items of {key} are listed"));
                    }
                }
                return Ok(crate::items::Listing {
                    items,
                    note: (!notes.is_empty()).then(|| notes.join("; ")),
                    details: false,
                });
            }
            Tickets::GitHub { forge, repository } => (forge, repository),
        };
        let repo = forge.gh_repository();
        let search =
            since.map(|since| format!("updated:>={}", crate::session_read::shown_date(since)));
        let mut args = vec![
            "issue",
            "list",
            "-R",
            &repo,
            "--state",
            "all",
            "--limit",
            forge::ALL,
        ];
        if let Some(search) = &search {
            args.extend(["--search", search]);
        }
        args.extend(["--json", LIST_FIELDS]);
        let issues = forge::gh_json(&args).await?;
        let issues = issues.as_array().cloned().unwrap_or_default();
        let mut items = Vec::new();
        for issue in &issues {
            let Some(number) = issue["number"].as_u64() else {
                continue;
            };
            match listed_issue(&repo, repository, issue) {
                Some(fetched) => items.push(fetched),
                // Only the tracker's repository's issues are kept, whatever `gh` answered.
                None => {
                    let key = number.to_string();
                    if let Some(gone) = stored.iter().find(|item| item.key == key) {
                        items.push(crate::items::Fetched::removed(gone.clone()));
                    }
                }
            }
        }
        Ok(crate::items::Listing {
            items,
            note: (issues.len() >= ALL_COUNT).then(|| {
                format!(
                    "only the newest {ALL_COUNT} issues are listed; older ones, if any, are not"
                )
            }),
            details: true,
        })
    }

    /// The stored open issues that GitHub no longer lists open, each read alone (`forge::swept`):
    /// a deleted one is not found, a transferred one answers with another repository's url.
    /// Jira has none to find this way.
    pub async fn swept(
        &self,
        stored: &[flottille_core::item::Item],
        listed: &[crate::items::Fetched],
    ) -> Result<Vec<crate::items::Fetched>, String> {
        let Tickets::GitHub { forge, repository } = self else {
            return Ok(Vec::new());
        };
        let repo = forge.gh_repository();
        forge::swept("issue", &repo, LIST_FIELDS, stored, listed, |issue| {
            listed_issue(&repo, repository, issue)
        })
        .await
    }

    /// Reads the comments of a listed issue; false when GitHub no longer has it.
    pub async fn detail(&self, fetched: &mut crate::items::Fetched) -> Result<bool, String> {
        let Tickets::GitHub { forge, .. } = self else {
            return Ok(true);
        };
        let viewed = match forge::gh_json(&[
            "issue",
            "view",
            &fetched.item.key,
            "-R",
            &forge.gh_repository(),
            "--json",
            "comments",
        ])
        .await
        {
            Ok(viewed) => viewed,
            Err(error) if forge::not_found(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        fetched.comments = viewed["comments"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|comment| {
                crate::items::Said::of(&forge::Comment {
                    author: said(&comment["author"]["login"]),
                    state: None,
                    body: said(&comment["body"]),
                    bot: false,
                })
            })
            .collect();
        fetched.item.details["comments"] = json!(fetched.comments.len());
        fetched.detailed = true;
        Ok(true)
    }
}

/// An issue as `gh issue list --json LIST_FIELDS` says it, of the repository `repository` of the
/// project (`repo` for `gh`); none when it is another repository's (transferred there).
fn listed_issue(repo: &str, repository: &str, issue: &Value) -> Option<crate::items::Fetched> {
    let number = issue["number"].as_u64()?;
    let ours = format!("https://{repo}/issues/").to_lowercase();
    if !issue["url"]
        .as_str()
        .is_some_and(|url| url.to_lowercase().starts_with(&ours))
    {
        return None;
    }
    Some(crate::items::Fetched {
        item: flottille_core::item::Item {
            project: String::new(),
            kind: "ticket".into(),
            source: "github".into(),
            repository: repository.to_owned(),
            key: number.to_string(),
            title: said(&issue["title"]),
            state: said(&issue["state"]).to_lowercase(),
            author: Some(said(&issue["author"]["login"])).filter(|name| !name.is_empty()),
            assignees: forge::names_of(&issue["assignees"], "login"),
            labels: forge::names_of(&issue["labels"], "name"),
            url: issue["url"].as_str().map(masked),
            updated_at: issue["updatedAt"].as_str().map(masked),
            seen_at: 0,
            mine: false,
            details: json!({}),
            checks: Vec::new(),
        },
        comments: Vec::new(),
        reviews: Vec::new(),
        created_at: issue["createdAt"]
            .as_str()
            .and_then(crate::session_read::form),
        detailed: false,
    })
}

/// The work items of the Jira project `key` updated since `since`, and whether the list was cut
/// at its cap. The time is given to JQL as minutes before now (`updated >= "-12m"`), which Jira
/// reads the same whatever the time zone of the user's profile, where a date would be read in it.
/// The user's own are those a second search with `currentUser()` finds, in the same window: Jira
/// says who the user is without Flottille reading it.
async fn changed_work_items(
    site: &str,
    key: &str,
    since: Option<u64>,
    now: u64,
) -> Result<(Vec<crate::items::Fetched>, bool), String> {
    let mut query = format!("project = {}", jql_quoted(key));
    if let Some(since) = since {
        let minutes = now.saturating_sub(since).div_ceil(60_000).max(1);
        query.push_str(&format!(" AND updated >= \"-{minutes}m\""));
    }
    let items = jira_search(&format!("{query} ORDER BY updated DESC"), ITEM_JIRA_FIELDS).await?;
    let mine: Vec<String> = jira_search(
        &format!("{query} AND (assignee = currentUser() OR reporter = currentUser())"),
        "key",
    )
    .await?
    .iter()
    .filter_map(|item| item["key"].as_str().map(masked))
    .collect();
    let cut = items.len() >= ALL_COUNT;
    let fetched = items
        .iter()
        // An item whose `self` names another site is not the tracker's (`list_work_items`).
        .filter(|item| {
            item.get("self").is_none_or(Value::is_null)
                || site_of(item).is_some_and(|from| answered_by(item, &from, site))
        })
        .filter_map(|item| {
            let fields = &item["fields"];
            let key = masked(item["key"].as_str()?);
            let name = |value: &Value| Some(said(value)).filter(|name| !name.is_empty());
            Some(crate::items::Fetched {
                item: flottille_core::item::Item {
                    project: String::new(),
                    kind: "ticket".into(),
                    source: "jira".into(),
                    repository: String::new(),
                    url: Some(format!("https://{site}/browse/{key}")),
                    mine: mine.contains(&key),
                    key,
                    title: said(&fields["summary"]),
                    // Done, by its status's category, is closed; its status is in its details.
                    state: match fields["status"]["statusCategory"]["key"].as_str() {
                        Some("done") => "closed".into(),
                        _ => "open".into(),
                    },
                    author: name(&fields["reporter"]["displayName"]),
                    assignees: name(&fields["assignee"]["displayName"])
                        .into_iter()
                        .collect(),
                    labels: fields["labels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(said)
                        .collect(),
                    updated_at: fields["updated"].as_str().map(masked),
                    seen_at: 0,
                    details: json!({
                        "status": name(&fields["status"]["name"]),
                        "type": name(&fields["issuetype"]["name"]),
                        "priority": name(&fields["priority"]["name"]),
                        "category": name(&fields["status"]["statusCategory"]["key"]),
                    }),
                    checks: Vec::new(),
                },
                comments: Vec::new(),
                reviews: Vec::new(),
                created_at: fields["created"].as_str().and_then(crate::items::time_ms),
                detailed: false,
            })
        })
        .collect();
    Ok((fetched, cut))
}

/// `acli jira workitem search` of `query`, the items as an array.
async fn jira_search(query: &str, fields: &str) -> Result<Vec<Value>, String> {
    let (ok, stdout, stderr) = match acli(&[
        "jira",
        "workitem",
        "search",
        "--jql",
        query,
        "--fields",
        fields,
        "--json",
        "--limit",
        forge::ALL,
    ])
    .await
    {
        Ok(output) => output,
        Err(Gh::Missing) => return Err("acli is not on the daemon's PATH".into()),
        Err(Gh::Failed(why)) => return Err(format!("acli jira workitem search: {why}")),
    };
    if !ok {
        let said = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(format!(
            "acli jira workitem search failed: {}",
            masked(&said.chars().take(300).collect::<String>())
        ));
    }
    let found: Value = serde_json::from_str(&stdout)
        .map_err(|error| format!("acli jira workitem search answered what is not JSON: {error}"))?;
    Ok(found
        .as_array()
        .or(found["issues"].as_array())
        .cloned()
        .unwrap_or_default())
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
    fn the_site_of_acli_s_sign_in_is_its_site_line_alone() {
        let status =
            "✓ Authenticated\n  Site: Example.atlassian.net\n  Email: someone@example.com\n";
        assert_eq!(
            signed_in_site(status).as_deref(),
            Some("example.atlassian.net")
        );
        assert_eq!(
            signed_in_site("  site: https://other.atlassian.net/").as_deref(),
            Some("other.atlassian.net")
        );
        assert_eq!(signed_in_site("✓ Authenticated\n"), None);
    }

    #[test]
    fn a_self_link_on_the_site_or_an_atlassian_host_is_taken_and_another_site_is_not() {
        let taken = |link: &str| {
            let item = json!({"self": link});
            answered_by(&item, &site_of(&item).unwrap(), "acme.atlassian.net")
        };

        assert!(taken("https://Acme.atlassian.net/rest/api/3/issue/1"));
        assert!(taken(
            "https://jira-prod-xx-1-1.prod.atl-paas.net/rest/api/3/issue/1"
        ));
        assert!(taken(
            "https://api.atlassian.com/ex/jira/0f1e2d3c-aaaa-4bbb-8ccc-123456789abc/rest/api/3/issue/1"
        ));
        assert!(!taken("https://other.atlassian.net/rest/api/3/issue/1"));
        assert!(!taken(
            "https://api.atlassian.com/ex/confluence/0f1e/wiki/1"
        ));
        assert!(!taken(
            "https://atl-paas.net.example.com/rest/api/3/issue/1"
        ));
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

        let ticket = work_item("ABC-123", &item, "acme.atlassian.net");

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
