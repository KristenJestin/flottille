//! `flottille reference`: the references of a project, documents its agents read with a tool.

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::protocol::{ReferenceAdd, ReferenceList, ReferenceRemove};
use serde_json::Value;

use crate::client::Client;
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum ReferenceAction {
    /// The references of a project
    List {
        project: String,
        #[arg(long)]
        json: bool,
    },
    /// Add a reference: a file or a folder of the project folder, with a line saying when to read it
    Add {
        project: String,
        name: String,
        /// A file or a folder, relative to the project folder
        path: String,
        /// One line: what the agents are told of it
        #[arg(long)]
        description: String,
        #[arg(long)]
        json: bool,
    },
    /// Remove a reference; the files are never touched
    Remove {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(paths: &Paths, action: ReferenceAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let mut ask = |method: &str, params: Value| {
        client
            .request(method, params)
            .map_err(|error| Box::<dyn std::error::Error>::from(in_full(paths, &error.to_string())))
    };
    let (answer, json, text) = match action {
        ReferenceAction::List { project, json } => {
            let list = ask(
                "reference.list",
                serde_json::to_value(ReferenceList {
                    project: project.clone(),
                })?,
            )?;
            let text = list_text(&project, &list);
            (list, json, text)
        }
        ReferenceAction::Add {
            project,
            name,
            path,
            description,
            json,
        } => {
            let added = ask(
                "reference.add",
                serde_json::to_value(ReferenceAdd {
                    project: project.clone(),
                    name: name.clone(),
                    description,
                    path,
                })?,
            )?;
            let text = format!(
                "Added reference {name} to {project}: {}.",
                added["path"].as_str().unwrap_or_default()
            );
            (added, json, text)
        }
        ReferenceAction::Remove {
            project,
            name,
            json,
        } => {
            let removed = ask(
                "reference.remove",
                serde_json::to_value(ReferenceRemove {
                    project: project.clone(),
                    name: name.clone(),
                })?,
            )?;
            let text =
                format!("Removed reference {name} from {project}. Its files were not touched.");
            (removed, json, text)
        }
    };
    if json {
        println!("{answer}");
    } else {
        println!("{text}");
    }
    Ok(ExitCode::SUCCESS)
}

fn list_text(project: &str, list: &Value) -> String {
    let references = list["references"].as_array().map_or(&[][..], Vec::as_slice);
    if references.is_empty() {
        return format!(
            "Project {project} has no references. Add one with `flottille reference add \
             {project} <name> <path> --description <line>`."
        );
    }
    let text =
        |reference: &Value, key: &str| reference[key].as_str().unwrap_or_default().to_owned();
    let names = references
        .iter()
        .map(|r| text(r, "name").chars().count())
        .chain([4])
        .max()
        .unwrap_or(4);
    let paths = references
        .iter()
        .map(|r| text(r, "path").chars().count())
        .chain([4])
        .max()
        .unwrap_or(4);
    let mut table = format!("{:names$}  {:paths$}  DESCRIPTION\n", "NAME", "PATH");
    for reference in references {
        table.push_str(&format!(
            "{:names$}  {:paths$}  {}\n",
            text(reference, "name"),
            text(reference, "path"),
            text(reference, "description"),
        ));
    }
    table.trim_end().to_owned()
}
