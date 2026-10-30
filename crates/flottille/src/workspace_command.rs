//! `flottille workspace`: making, following and removing the workspaces of a project.

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::protocol::{
    WorkspaceCreate, WorkspaceForge, WorkspaceList, WorkspaceName, WorkspaceTicket,
};
use flottille_core::store::now_ms;
use serde_json::Value;

use crate::client::Client;
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum WorkspaceAction {
    /// Make a workspace: a worktree per repository on one branch, then the setup, in the
    /// background
    Create {
        project: String,
        name: String,
        /// The repositories it takes, separated by commas (default: the project's set)
        #[arg(long, value_delimiter = ',')]
        repositories: Option<Vec<String>>,
        /// The branch (default: <branch prefix>/<name>)
        #[arg(long)]
        branch: Option<String>,
        /// The base branch of one repository, in place of its rule: <repository>=<branch>, once
        /// per repository
        #[arg(long = "base", value_name = "REPOSITORY=BRANCH", value_parser = base_of)]
        bases: Vec<(String, String)>,
        /// Fetch nothing: start from the tracking refs as they are
        #[arg(long)]
        offline: bool,
        /// Link it to a ticket: #123 (an issue of the repository's GitHub) or a Jira key
        #[arg(long)]
        ticket: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// The workspaces, of every project or of one
    List {
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Link a workspace to a ticket (#123 or a Jira key), or to none with --clear
    Ticket {
        project: String,
        name: String,
        /// #123 (an issue of the repository's GitHub) or a Jira key such as ABC-123
        #[arg(required_unless_present = "clear", conflicts_with = "clear")]
        ticket: Option<String>,
        /// Unlink it
        #[arg(long)]
        clear: bool,
        #[arg(long)]
        json: bool,
    },
    /// One workspace: its repositories, where its preparation stands, its ticket, and each
    /// repository's pull request with its checks
    Show {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Resume a preparation that failed, from the step that failed
    Prepare {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Remove a workspace whose work is all committed and pushed; never forced
    Remove {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
}

/// `create` fails when the preparation already failed: the output says where.
pub fn run(paths: &Paths, action: WorkspaceAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let mut ask = |method: &str, params: Value| {
        client
            .request(method, params)
            .map_err(|error| Box::<dyn std::error::Error>::from(in_full(paths, &error.to_string())))
    };
    let named =
        |project: String, name: String| serde_json::to_value(WorkspaceName { project, name });
    let (shown, json) = match action {
        WorkspaceAction::Create {
            project,
            name,
            repositories,
            branch,
            bases,
            offline,
            ticket,
            json,
        } => {
            let workspace = ask(
                "workspace.create",
                serde_json::to_value(WorkspaceCreate {
                    project,
                    name,
                    repositories,
                    branch,
                    bases: bases.into_iter().collect(),
                    offline,
                    ticket,
                })?,
            )?;
            let text = created_text(&workspace);
            let failed = workspace["state"] == "failed";
            print(&workspace, json, &text);
            return Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            });
        }
        WorkspaceAction::List { project, json } => {
            let list = ask(
                "workspace.list",
                serde_json::to_value(WorkspaceList { project })?,
            )?;
            let text = list_text(&list);
            (list, (json, text))
        }
        WorkspaceAction::Ticket {
            project,
            name,
            ticket,
            clear: _,
            json,
        } => {
            let linked = ask(
                "workspace.link_ticket",
                serde_json::to_value(WorkspaceTicket {
                    project,
                    name,
                    ticket,
                })?,
            )?;
            let text = match linked["ticket"].as_str() {
                Some(_) => format!("Linked.\n{}", ticket_text(&linked)),
                None => format!(
                    "Workspace {} of {} is linked to no ticket.\n",
                    text(&linked, "workspace"),
                    text(&linked, "project")
                ),
            };
            (linked, (json, text))
        }
        WorkspaceAction::Show {
            project,
            name,
            json,
        } => {
            let mut workspace = ask("workspace.show", named(project.clone(), name.clone())?)?;
            if workspace["ticket"].is_string() {
                workspace["ticket_read"] =
                    ask("workspace.ticket", named(project.clone(), name.clone())?)?;
            }
            workspace["forge"] = ask(
                "workspace.forge",
                serde_json::to_value(WorkspaceForge {
                    project,
                    name,
                    watch: false,
                })?,
            )?;
            let text = show_text(&workspace);
            (workspace, (json, text))
        }
        WorkspaceAction::Prepare {
            project,
            name,
            json,
        } => {
            let workspace = ask("workspace.prepare", named(project, name)?)?;
            let text = prepared_text(&workspace);
            (workspace, (json, text))
        }
        WorkspaceAction::Remove {
            project,
            name,
            json,
        } => {
            let removed = ask("workspace.remove", named(project, name)?)?;
            let text = format!(
                "Removed workspace {} of {}: its worktrees and its folder. Its branches are kept.\n",
                text(&removed, "removed"),
                text(&removed, "project")
            );
            (removed, (json, text))
        }
    };
    print(&shown, json.0, &json.1);
    Ok(ExitCode::SUCCESS)
}

fn base_of(text: &str) -> std::result::Result<(String, String), String> {
    match text.split_once('=') {
        Some((repository, branch)) if !repository.is_empty() && !branch.is_empty() => {
            Ok((repository.to_owned(), branch.to_owned()))
        }
        _ => Err(format!("{text} is not <repository>=<branch>")),
    }
}

fn print(value: &Value, json: bool, text: &str) {
    if json {
        println!("{value}");
    } else {
        print!("{text}");
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn steps(workspace: &Value) -> &[Value] {
    workspace["steps"].as_array().map_or(&[][..], Vec::as_slice)
}

fn created_text(workspace: &Value) -> String {
    let mut shown = format!(
        "Workspace {} of {}, branch {}, in {}\n",
        text(workspace, "name"),
        text(workspace, "project"),
        text(workspace, "branch"),
        text(workspace, "folder")
    );
    shown.push_str(&repositories_text(workspace));
    if let Some(ticket) = workspace["ticket"].as_str() {
        shown.push_str(&format!("Ticket: {ticket}\n"));
    }
    match workspace["state"].as_str() {
        Some("ready") => shown.push_str("Ready.\n"),
        Some("failed") => shown.push_str(&failure_text(workspace)),
        _ => shown.push_str(&format!(
            "The worktrees are made; the setup goes on in the background: follow it with \
             `flottille workspace show {} {}`.\n",
            text(workspace, "project"),
            text(workspace, "name")
        )),
    }
    shown
}

/// One line per repository: its base, and how fresh it is.
fn repositories_text(workspace: &Value) -> String {
    let mut shown = String::new();
    for repository in workspace["repositories"].as_array().into_iter().flatten() {
        let base = &repository["base"];
        let commit: String = text(base, "commit").chars().take(12).collect();
        let freshness = if let Some(note) = base["note"].as_str() {
            note.to_owned()
        } else if base["fetched_now"] == true {
            format!("{commit}, fetched")
        } else {
            let age = match base["fetched_at"].as_u64() {
                Some(at) => format!(
                    "{} ago",
                    crate::duration(now_ms().saturating_sub(at) / 1000)
                ),
                None => "an unknown time (git kept no trace of it)".into(),
            };
            format!("offline: the tracking ref as of {age}, {commit}")
        };
        shown.push_str(&format!(
            "  {}: {}, {freshness}\n",
            text(repository, "path"),
            text(base, "branch")
        ));
    }
    shown
}

fn failure_text(workspace: &Value) -> String {
    let mut shown = String::new();
    for step in steps(workspace) {
        if step["state"] == "failed" {
            shown.push_str(&format!("Failed: {}\n", text(&step["failure"], "doing")));
            for line in text(&step["failure"], "output").lines() {
                shown.push_str(&format!("  | {line}\n"));
            }
            shown.push_str(&format!(
                "Fix it, then resume with `flottille workspace prepare {} {}`.\n",
                text(workspace, "project"),
                text(workspace, "name")
            ));
        }
    }
    shown
}

fn list_text(list: &Value) -> String {
    let workspaces = list["workspaces"].as_array().map_or(&[][..], Vec::as_slice);
    if workspaces.is_empty() {
        return "No workspace. Make one with `flottille workspace create <project> <name>`.\n"
            .into();
    }
    let width = |key: &str, title: &str| {
        workspaces
            .iter()
            .map(|w| text(w, key).chars().count())
            .chain([title.len()])
            .max()
            .unwrap_or(0)
    };
    let (projects, names, branches) = (
        width("project", "PROJECT"),
        width("name", "NAME"),
        width("branch", "BRANCH"),
    );
    let mut table = format!(
        "{:projects$}  {:names$}  {:branches$}  STATE\n",
        "PROJECT", "NAME", "BRANCH"
    );
    for workspace in workspaces {
        table.push_str(&format!(
            "{:projects$}  {:names$}  {:branches$}  {}\n",
            text(workspace, "project"),
            text(workspace, "name"),
            text(workspace, "branch"),
            text(workspace, "state"),
        ));
    }
    table
}

fn show_text(workspace: &Value) -> String {
    let mut shown = format!(
        "{}, branch {}, {}\n  project: {}\n  folder: {}\nRepositories:\n",
        text(workspace, "name"),
        text(workspace, "branch"),
        text(workspace, "state"),
        text(workspace, "project"),
        text(workspace, "folder"),
    );
    shown.push_str(&repositories_text(workspace));
    shown.push_str("Steps:\n");
    for step in steps(workspace) {
        let doing = describe(step);
        shown.push_str(&format!(
            "  {:>2}. {:8} {doing}\n",
            step["position"].as_u64().unwrap_or(0) + 1,
            text(step, "state"),
        ));
    }
    if workspace["state"] == "failed" {
        shown.push_str(&failure_text(workspace));
    }
    shown.push_str(&ticket_text(&workspace["ticket_read"]));
    shown.push_str(&forge_text(&workspace["forge"]));
    shown
}

/// The workspace's ticket as its tracker said it, or why it could not be read; nothing when the
/// workspace has none.
fn ticket_text(ticket: &Value) -> String {
    let Some(reference) = ticket["ticket"].as_str() else {
        return String::new();
    };
    if let Some(problem) = ticket["problem"].as_str() {
        return format!("Ticket {reference}: it could not be read: {problem}\n");
    }
    let mut shown = format!(
        "Ticket {reference}, {}: {}\n",
        text(ticket, "state"),
        text(ticket, "title")
    );
    if let Some(url) = ticket["url"].as_str() {
        shown.push_str(&format!("  {url}\n"));
    }
    let description = text(ticket, "description");
    if !description.is_empty() {
        shown.push_str("  Description:\n");
        for line in description.lines() {
            shown.push_str(&format!("    {line}\n"));
        }
    }
    let comments = ticket["comments"].as_array().map_or(&[][..], Vec::as_slice);
    if !comments.is_empty() {
        shown.push_str(&format!("  Comments ({}):\n", comments.len()));
        for comment in comments {
            let at = comment["at"]
                .as_str()
                .map_or(String::new(), |at| format!(", {at}"));
            let mut lines = text(comment, "body").lines();
            shown.push_str(&format!(
                "    - {}{at}: {}\n",
                text(comment, "author"),
                lines.next().unwrap_or_default()
            ));
            for line in lines {
                shown.push_str(&format!("        {line}\n"));
            }
        }
    }
    shown
}

/// Each repository's pull request and its checks, or why there is none.
fn forge_text(forge: &Value) -> String {
    let mut shown = "Pull requests:\n".to_owned();
    if let Some(problem) = forge["problem"].as_str() {
        shown.push_str(&format!("  {problem}.\n"));
    }
    for repository in forge["repositories"].as_array().into_iter().flatten() {
        let named = match repository["repository"].as_str() {
            Some(slug) => format!("{} ({slug})", text(repository, "path")),
            None => text(repository, "path").to_owned(),
        };
        let pull = &repository["pull_request"];
        let said = if let Some(note) = repository["note"].as_str() {
            note.to_owned()
        } else if let Some(error) = repository["error"].as_str() {
            format!("the forge could not be read: {error}")
        } else if !forge["problem"].is_null() && pull.is_null() {
            continue;
        } else if pull.is_null() {
            "no pull request".to_owned()
        } else {
            let mut said = format!(
                "#{} {} into {}",
                pull["number"],
                text(pull, "state"),
                text(pull, "base")
            );
            said.push_str(&match pull["mark"].as_str() {
                Some(mark) => format!(", checks {mark}"),
                None => ", no checks".to_owned(),
            });
            if let Some(review) = pull["review"].as_str() {
                said.push_str(&format!(", {review}"));
            }
            match pull["comments"].as_u64().unwrap_or(0) {
                0 => {}
                1 => said.push_str(", 1 comment"),
                n => said.push_str(&format!(", {n} comments")),
            }
            format!("{said}: {}\n    {}", text(pull, "title"), text(pull, "url"))
        };
        shown.push_str(&format!("  {named}: {said}\n"));
    }
    shown
}

/// A step as the setup said it.
fn describe(step: &Value) -> String {
    let place = step["repository"].as_str().unwrap_or(".");
    match step["kind"].as_str().unwrap_or_default() {
        "worktree" => format!("git worktree add {place}"),
        "copy" => {
            let globs: Vec<&str> = step["globs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            format!("copy {} in {place}", globs.join(" "))
        }
        "link" => format!("link {} in {place}", text(step, "path")),
        "root_entries" => "link the project folder's entries".into(),
        "run" => format!("{} (in {place})", text(step, "line")),
        kind => kind.to_owned(),
    }
}

fn prepared_text(workspace: &Value) -> String {
    let resumed = &workspace["resumed"];
    let (redone, retried) = (
        resumed["redone"].as_u64().unwrap_or(0),
        resumed["retried"].as_u64().unwrap_or(0),
    );
    if redone == 0 && retried == 0 && workspace["state"] == "ready" {
        return format!(
            "Nothing to resume: workspace {} is ready.\n",
            text(workspace, "name")
        );
    }
    format!(
        "Resumed workspace {}: {retried} step(s) retried, {redone} done again because their \
         result is gone. Follow it with `flottille workspace show {} {}`.\n",
        text(workspace, "name"),
        text(workspace, "project"),
        text(workspace, "name"),
    )
}
