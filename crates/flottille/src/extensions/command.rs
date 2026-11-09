//! `flottille extension`, and the command lines the extensions declare.

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use super::Command;
use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum ExtensionAction {
    /// Each part, whether it is on, and in which projects
    List {
        #[arg(long)]
        json: bool,
    },
    /// Turn it on, for all projects or, with --project, back on in one
    On {
        name: String,
        #[arg(long, value_name = "P")]
        project: Option<String>,
    },
    /// Turn it off, for all projects or, with --project, in one; what it keeps stays
    Off {
        name: String,
        #[arg(long, value_name = "P")]
        project: Option<String>,
    },
}

pub fn run(paths: &Paths, command: Command) -> Result<ExitCode> {
    match command {
        Command::Extension { action } => extension(paths, action),
        Command::Spec { action } => super::specs::command::run(paths, action),
        Command::Review { session, json } => review(paths, &session, json),
    }
}

/// `flottille review <session>`: the review starts, or why not, before any agent starts.
fn review(paths: &Paths, session: &str, json: bool) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let answer = client.request("review.start", json!({"agent": session}))?;
    if json {
        println!("{answer}");
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "Started the review of session {session}, round {}: its pilot is agent {}, its passes {}. \
         The findings are delivered to the session; `flottille agent show {session}` follows it.",
        answer["round"],
        answer["pilot"].as_str().unwrap_or_default(),
        answer["passes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(ExitCode::SUCCESS)
}

fn extension(paths: &Paths, action: ExtensionAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let (name, project, enabled) = match action {
        ExtensionAction::List { json } => {
            let answer = client.request("extension.list", json!({}))?;
            if json {
                println!("{answer}");
            } else {
                print!("{}", list_text(&answer));
            }
            return Ok(ExitCode::SUCCESS);
        }
        ExtensionAction::On { name, project } => (name, project, true),
        ExtensionAction::Off { name, project } => (name, project, false),
    };
    let answer = client.request(
        "extension.set",
        json!({"name": name, "project": project, "enabled": enabled}),
    )?;
    let title = answer["title"].as_str().unwrap_or(&name);
    let state = if enabled { "on" } else { "off" };
    match &project {
        Some(project) if answer["enabled"] == true || !enabled => {
            println!("{title} is {state} in project {project}.")
        }
        Some(project) => println!(
            "{title} is on in project {project}, and off for all projects: `flottille extension \
             on {name}` turns it on."
        ),
        None => println!("{title} is {state}."),
    }
    Ok(ExitCode::SUCCESS)
}

fn list_text(answer: &Value) -> String {
    let mut text = String::new();
    for extension in answer["extensions"].as_array().into_iter().flatten() {
        let state = if extension["enabled"] == true {
            "on"
        } else {
            "off"
        };
        text.push_str(&format!(
            "{} ({}): {state}. {}\n",
            extension["name"].as_str().unwrap_or_default(),
            extension["title"].as_str().unwrap_or_default(),
            extension["sentence"].as_str().unwrap_or_default(),
        ));
        if let Some(error) = extension["error"].as_str() {
            text.push_str(&format!("  cannot be used: {error}\n"));
        }
        for project in extension["projects"].as_array().into_iter().flatten() {
            let name = project["project"].as_str().unwrap_or_default();
            let line = match (project["error"].as_str(), project["switch"].as_bool()) {
                (Some(error), _) => format!("cannot be used: {error}"),
                (None, Some(false)) => "off, by its switch".into(),
                (None, _) if project["enabled"] == true => "on".into(),
                (None, _) => "off".into(),
            };
            text.push_str(&format!("  {name}: {line}\n"));
        }
    }
    for error in answer["errors"].as_array().into_iter().flatten() {
        text.push_str(&format!("{}\n", error.as_str().unwrap_or_default()));
    }
    text
}
