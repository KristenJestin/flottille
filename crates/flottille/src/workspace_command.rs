//! `flottille workspace`: making, following and removing the workspaces of a project.

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::protocol::{WorkspaceCreate, WorkspaceList, WorkspaceName};
use flottille_core::store::now_ms;
use serde_json::Value;

use crate::client::Client;
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum WorkspaceAction {
    /// Make a workspace: a worktree per repository on one branch, then the recipe, in the
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
        /// Fetch nothing: start from the tracking refs as they are
        #[arg(long)]
        offline: bool,
        #[arg(long)]
        json: bool,
    },
    /// The workspaces, of every project or of one
    List {
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// One workspace: its repositories and where its preparation stands
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
            offline,
            json,
        } => {
            let workspace = ask(
                "workspace.create",
                serde_json::to_value(WorkspaceCreate {
                    project,
                    name,
                    repositories,
                    branch,
                    offline,
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
        WorkspaceAction::Show {
            project,
            name,
            json,
        } => {
            let workspace = ask("workspace.show", named(project, name)?)?;
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
    match workspace["state"].as_str() {
        Some("ready") => shown.push_str("Ready.\n"),
        Some("failed") => shown.push_str(&failure_text(workspace)),
        _ => shown.push_str(&format!(
            "The worktrees are made; the recipe goes on in the background: follow it with \
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
        let freshness = if base["fetched_now"] == true {
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
    shown
}

/// A step as the recipe said it.
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
