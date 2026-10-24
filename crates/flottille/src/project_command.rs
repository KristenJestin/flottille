//! `flottille project`: registering folders and looking after what Flottille keeps for them.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::protocol::{ProjectFolder, ProjectName};
use serde_json::Value;

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum ProjectAction {
    /// Register a folder: its repositories are detected and shown, and you confirm them
    Add {
        folder: PathBuf,
        /// The project's name (default: a slug of the folder's name)
        #[arg(long)]
        name: Option<String>,
        /// Register without asking
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// The registered projects
    List {
        #[arg(long)]
        json: bool,
    },
    /// One project's configuration
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Forget a project: its configuration, never its folder
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Detect the repositories again and say how they differ from the configuration
    Check {
        name: String,
        #[arg(long)]
        json: bool,
    },
}

/// Fails (without a message of its own) when `list` found a file it cannot use, or `check` a
/// difference: the output says what.
pub fn run(paths: &Paths, action: ProjectAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let mut ask = |method: &str, params: Value| client.request(method, params);
    let name = |name: String| serde_json::to_value(ProjectName { name });
    let outcome = |ok: bool| {
        if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    };
    match action {
        ProjectAction::Add {
            folder,
            name,
            yes,
            json,
        } => {
            let folder = std::path::absolute(&folder)?;
            let folder = folder
                .to_str()
                .ok_or("the folder's path is not valid UTF-8")?
                .to_owned();
            let found = ask(
                "project.detect",
                serde_json::to_value(ProjectFolder {
                    folder,
                    name,
                    repositories: None,
                })?,
            )?;
            let file = paths.projects().join(format!(
                "{}.toml",
                found["name"].as_str().unwrap_or_default()
            ));
            eprintln!("{}", detected_text(&found, &file));
            if !yes {
                confirm()?;
            }
            // What was shown is what is registered: the daemon refuses if the folder changed meanwhile.
            let project = ask(
                "project.add",
                serde_json::json!({
                    "folder": found["folder"],
                    "name": found["name"],
                    "repositories": found["repositories"],
                }),
            )?;
            if json {
                println!("{project}");
            } else {
                println!(
                    "Registered project {}. Its configuration is {}.",
                    project["name"].as_str().unwrap_or_default(),
                    file.display()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        ProjectAction::List { json } => {
            let list = ask("project.list", Value::Null)?;
            let errors = list["errors"].as_array().map_or(&[][..], Vec::as_slice);
            if json {
                println!("{list}");
            } else {
                print!("{}", list_text(&list));
            }
            for error in errors {
                eprintln!(
                    "{}: {}",
                    error["file"].as_str().unwrap_or_default(),
                    error["message"].as_str().unwrap_or_default()
                );
            }
            Ok(outcome(errors.is_empty()))
        }
        ProjectAction::Show { name: wanted, json } => {
            let project = ask("project.show", name(wanted)?)?;
            if json {
                println!("{project}");
            } else {
                print!("{}", show_text(&project));
            }
            Ok(ExitCode::SUCCESS)
        }
        ProjectAction::Remove { name: wanted, json } => {
            let removed = ask("project.remove", name(wanted.clone())?)?;
            if json {
                println!("{removed}");
            } else {
                println!("Removed project {wanted}. Its folder was not touched.");
            }
            Ok(ExitCode::SUCCESS)
        }
        ProjectAction::Check { name: wanted, json } => {
            let result = ask("project.check", name(wanted)?)?;
            let same = list_of(&result["new"]).is_empty() && list_of(&result["missing"]).is_empty();
            if json {
                println!("{result}");
            } else {
                print!("{}", check_text(&result));
            }
            Ok(outcome(same))
        }
    }
}

/// Asks on the standard input; anything but a yes, or no answer at all, refuses.
fn confirm() -> Result<()> {
    eprint!("Register it? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer)? == 0 {
        eprintln!();
        return Err(
            "not registered: nothing to read an answer from, use --yes to register without asking"
                .into(),
        );
    }
    match answer.trim().to_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => Err("not registered".into()),
    }
}

fn list_of(value: &Value) -> Vec<&str> {
    value.as_array().map_or_else(Vec::new, |items| {
        items.iter().filter_map(Value::as_str).collect()
    })
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn detected_text(found: &Value, file: &std::path::Path) -> String {
    let repositories = list_of(&found["repositories"]);
    let mut shown = format!(
        "Project {}, folder {}\n{}:\n",
        text(found, "name"),
        text(found, "folder"),
        match repositories.len() {
            1 => "1 repository".to_owned(),
            count => format!("{count} repositories"),
        },
    );
    for repository in &repositories {
        shown.push_str(&format!("  {repository}\n"));
    }
    shown.push_str(&format!(
        "Each takes the remote origin and the base branch main; edit {} to change that.",
        file.display()
    ));
    shown
}

fn list_text(list: &Value) -> String {
    let projects = list["projects"].as_array().map_or(&[][..], Vec::as_slice);
    if projects.is_empty() && list["errors"].as_array().is_none_or(Vec::is_empty) {
        return "No project is registered. Add one with `flottille project add <folder>`.\n".into();
    }
    let width = |key: &str, title: &str| {
        projects
            .iter()
            .map(|project| text(project, key).chars().count())
            .chain([title.len()])
            .max()
            .unwrap_or(0)
    };
    let (names, folders) = (width("name", "NAME"), width("folder", "FOLDER"));
    let mut table = format!("{:names$}  {:folders$}  REPOSITORIES\n", "NAME", "FOLDER");
    for project in projects {
        let count = project["repositories"].as_array().map_or(0, Vec::len);
        table.push_str(&format!(
            "{:names$}  {:folders$}  {count}\n",
            text(project, "name"),
            text(project, "folder"),
        ));
    }
    table
}

fn show_text(project: &Value) -> String {
    let mut shown = format!("{}\n", text(project, "name"));
    let set = &project["default_repositories"];
    let takes = match set.as_str() {
        Some(_) => "all repositories".to_owned(),
        None => list_of(set).join(", "),
    };
    for (label, value) in [
        ("folder", text(project, "folder").to_owned()),
        ("branch prefix", text(project, "branch_prefix").to_owned()),
        (
            "workspaces root",
            text(project, "workspaces_root").to_owned(),
        ),
        ("workspaces take", takes),
        ("children at once", project["max_children"].to_string()),
    ] {
        shown.push_str(&format!("  {label}: {value}\n"));
    }
    let never = list_of(&project["never"]);
    if !never.is_empty() {
        shown.push_str(&format!("  never: {}\n", never.join(", ")));
    }

    shown.push_str("Repositories:\n");
    for repository in project["repositories"].as_array().into_iter().flatten() {
        shown.push_str(&format!(
            "  {}: remote {}, base {}",
            text(repository, "path"),
            text(repository, "remote"),
            list_of(&repository["base"]).join(" then "),
        ));
        let after = list_of(&repository["depends_on"]);
        if !after.is_empty() {
            shown.push_str(&format!(", after {}", after.join(", ")));
        }
        shown.push('\n');
    }
    if let Some(variables) = project["variables"].as_object() {
        shown.push_str("Variables:\n");
        for (name, value) in variables {
            shown.push_str(&format!(
                "  {name} = {}\n",
                value.as_str().unwrap_or_default()
            ));
        }
    }
    if let Some(commands) = project["commands"].as_object() {
        shown.push_str("Commands:\n");
        for (name, command) in commands {
            let flags: Vec<String> = ["serve", "check", "read_only", "ask_before_running"]
                .into_iter()
                .filter(|flag| command[flag] == true)
                .map(|flag| flag.replace('_', " "))
                .collect();
            let mut details: Vec<String> = command["repository"]
                .as_str()
                .map(|repository| format!("in {repository}"))
                .into_iter()
                .collect();
            if !flags.is_empty() {
                details.push(flags.join(", "));
            }
            shown.push_str(&format!("  {name}: {}", text(command, "line")));
            if !details.is_empty() {
                shown.push_str(&format!(" ({})", details.join("; ")));
            }
            shown.push('\n');
        }
    }
    shown
}

fn check_text(result: &Value) -> String {
    let (new, missing) = (list_of(&result["new"]), list_of(&result["missing"]));
    if new.is_empty() && missing.is_empty() {
        return format!(
            "Project {}: the repositories match the configuration.\n",
            text(result, "name")
        );
    }
    let mut shown = String::new();
    if !new.is_empty() {
        shown.push_str(&format!(
            "Repositories found and not in the configuration: {}\n",
            new.join(", ")
        ));
    }
    if !missing.is_empty() {
        shown.push_str(&format!(
            "Repositories in the configuration, not found: {}\n",
            missing.join(", ")
        ));
    }
    shown
}
