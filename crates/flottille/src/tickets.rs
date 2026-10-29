//! Tickets (`docs/tickets.md`): what the tracker says of the ticket a workspace is linked to, its
//! title, state, description and comments, and the agents' `tickets`, which list, search and read
//! the tracker's tickets and write GitHub issues. Through the user's own command lines, spawned
//! without a shell: `gh` for the issues of GitHub, `acli` (Atlassian's) for Jira. Flottille never
//! reads, keeps nor prints a token, and of each one's sign-in status it reads the exit code alone.
//! Jira is read only: the only `acli` calls are `jira auth status`, `jira workitem view` and
//! `jira workitem search`. A write to GitHub is a `tickets` call the gate let through.
//!
//! A ticket is read on demand, never in the background, and kept nowhere. Every text kept from a
//! tracker is masked before it is shown or stored.

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
                    last.author,
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
/// otherwise.
async fn jira_site(site: &str) -> Result<(), String> {
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
                 was read; switch acli to it (`acli jira auth switch --site {site}`) or correct \
                 the project's tracker",
                masked(&active)
            ));
        }
        None => {
            return Err(format!(
                "acli did not say which site it is signed in to (no Site: line in `acli jira \
                 auth status`), so it cannot be checked to be {site}: nothing was read"
            ));
        }
    }
    Ok(())
}

async fn read_work_item(key: &str, site: &str) -> Result<Ticket, String> {
    jira_site(site).await?;
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
    // The `Site:` line said where the sign-in is; the item itself says where it came from.
    match site_of(&item) {
        Some(from) if from == site => Ok(work_item(key, &item)),
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
fn site_of(item: &Value) -> Option<String> {
    item["self"]
        .as_str()?
        .strip_prefix("https://")?
        .split('/')
        .next()
        .filter(|host| !host.is_empty())
        .map(str::to_ascii_lowercase)
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
        url: site_of(item).map(|host| masked(&format!("https://{host}/browse/{key}"))),
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
}

/// The writes of `tickets`.
const WRITES: [&str; 5] = ["create", "comment", "edit", "close", "reopen"];

/// Why a write of `tickets` cannot be done on the project's tracker, before anyone is asked:
/// Flottille reads Jira and writes nothing to it.
pub fn unwritable(project: &Project, args: &Value) -> Option<String> {
    let action = args["action"].as_str()?;
    match project.tracker.as_ref()? {
        ProjectTracker::Jira { .. } if WRITES.contains(&action) => Some(format!(
            "refused: tickets {action} is not supported on Jira: Flottille reads Jira tickets and \
             writes nothing to Jira; write the text for the user, who posts it"
        )),
        _ => None,
    }
}

/// `tickets`, once the gate let it through: the tickets of the project's tracker, listed,
/// searched and read through the user's own `gh` or `acli`, and GitHub issues written through
/// `gh`. Never another tracker's.
pub async fn tickets(daemon: &Daemon, place: &Place, args: Value) -> Outcome {
    let asked: TicketsCall = match arguments("tickets", args) {
        Ok(asked) => asked,
        Err(refusal) => return refusal,
    };
    let project = match projects::load_project(daemon, &place.project) {
        Ok(project) => project,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    if let Some(refusal) = unwritable(&project, &json!({"action": asked.action})) {
        return Outcome::Refused(refusal);
    }
    match asked.action.as_str() {
        "list" | "search" => listed(&project, &asked).await,
        "show" => show(daemon, place, &project, &asked).await,
        "create" => create(&project, &asked).await,
        "comment" | "edit" | "close" | "reopen" => {
            let key = match key_of(daemon, place, &project, &asked) {
                Ok(key) => key,
                Err(refusal) => return refusal,
            };
            write(&project, &asked, &key).await
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
async fn listed(project: &Project, asked: &TicketsCall) -> Outcome {
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
    let filters = Filters {
        state,
        label: asked.label.as_deref(),
        assignee: asked.assignee.as_deref(),
        query,
    };
    let found = match tracker {
        ProjectTracker::Github { repository } => match github_of(project, repository).await {
            Ok(forge) => list_issues(&forge, &filters).await,
            Err(why) => Err(why),
        },
        ProjectTracker::Jira { site, projects } => list_work_items(site, projects, &filters).await,
    };
    let (tickets, left_out) = match found {
        Ok(found) => found,
        Err(why) => return Outcome::Failed(format!("the tickets could not be listed: {why}")),
    };
    let mut text = format!(
        "The {state} tickets of {}{}: {}\n",
        tracker.said(),
        query.map_or(String::new(), |q| format!(" matching {q:?}")),
        tickets.len()
    );
    for ticket in &tickets {
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
    if left_out > 0 {
        text.push_str(&format!(
            "({left_out} answered from another site than the tracker's are left out)\n"
        ));
    }
    text.push_str("tickets {\"action\": \"show\", \"ticket\": …} reads one.");
    Outcome::Done(text)
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
    let tickets = issues
        .as_array()
        .into_iter()
        .flatten()
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
    Ok((tickets, 0))
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
/// to the tracker's site. An item that says another site is left out, and counted.
async fn list_work_items(
    site: &str,
    projects: &[String],
    filters: &Filters<'_>,
) -> Result<(Vec<Listed>, usize), String> {
    jira_site(site).await?;
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
        if site_of(item).as_deref() != Some(site) {
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
async fn show(daemon: &Daemon, place: &Place, project: &Project, asked: &TicketsCall) -> Outcome {
    let full = match full_detail("tickets", asked.detail.as_deref()) {
        Ok(full) => full,
        Err(refusal) => return refusal,
    };
    let key = match key_of(daemon, place, project, asked) {
        Ok(key) => key,
        Err(refusal) => return refusal,
    };
    match read(project, &key).await {
        Ok(ticket) => Outcome::Done(ticket.text(full, asked.ticket.as_deref())),
        Err(problem) => Outcome::Failed(format!(
            "the ticket {} could not be read: {problem}",
            key.text()
        )),
    }
}

/// The GitHub repository of the project's tracker, for a write: Jira was refused before.
async fn issues_of(project: &Project) -> Result<Forge, Outcome> {
    match &project.tracker {
        Some(ProjectTracker::Github { repository }) => github_of(project, repository)
            .await
            .map_err(|why| Outcome::Refused(format!("refused: {why}; nothing was written"))),
        Some(ProjectTracker::Jira { .. }) => Err(Outcome::Refused(
            "refused: Flottille writes nothing to Jira".into(),
        )),
        None => Err(Outcome::Refused(format!("refused: {NO_TRACKER}"))),
    }
}

/// `tickets create`: a new issue of the tracker's repository.
async fn create(project: &Project, asked: &TicketsCall) -> Outcome {
    let Some(title) = asked.title.as_deref().filter(|t| !t.trim().is_empty()) else {
        return Outcome::Refused("refused: tickets create needs title".into());
    };
    let forge = match issues_of(project).await {
        Ok(forge) => forge,
        Err(refusal) => return refusal,
    };
    if let Err(problem) = forge::signed_in().await {
        return Outcome::Failed(problem);
    }
    let repo = forge.gh_repository();
    let body = asked.body.as_deref().unwrap_or_default();
    let mut args = vec![
        "issue", "create", "-R", &repo, "--title", title, "--body", body,
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

/// `tickets comment`, `edit`, `close` and `reopen` of an issue.
async fn write(project: &Project, asked: &TicketsCall, key: &Key) -> Outcome {
    let action = asked.action.as_str();
    let body = asked.body.as_deref().filter(|b| !b.trim().is_empty());
    let title = asked.title.as_deref().filter(|t| !t.trim().is_empty());
    match (action, body, title) {
        ("comment", None, _) => {
            return Outcome::Refused("refused: tickets comment needs body, the comment".into());
        }
        ("edit", None, None) => {
            return Outcome::Refused(
                "refused: tickets edit needs title, body or both: what changes".into(),
            );
        }
        _ => {}
    }
    let Key::Issue(number) = key else {
        return Outcome::Refused("refused: Flottille writes nothing to Jira".into());
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
    if action != "close" && action != "reopen" {
        if let Some(title) = title {
            args.extend(["--title", title]);
        }
        if let Some(body) = body {
            args.extend(["--body", body]);
        }
    }
    let slug = forge.slug();
    match forge::gh_text(&args).await {
        Ok(_) => Outcome::Done(match action {
            "comment" => format!("The comment was posted on #{number} of {slug}."),
            "edit" => format!("#{number} of {slug} was changed."),
            "close" => format!("#{number} of {slug} was closed."),
            _ => format!("#{number} of {slug} was reopened."),
        }),
        Err(why) => Outcome::Failed(why),
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
