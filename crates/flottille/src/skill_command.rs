//! `flottille skill`: the skills of a project, texts its agents read with a tool when their task
//! needs them (`docs/skills.md`).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum SkillAction {
    /// The skills of a project
    List {
        project: String,
        #[arg(long)]
        json: bool,
    },
    /// One skill: its description, and its text when Flottille keeps it
    Show {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Add a skill: a file or a folder of the project folder, or a text Flottille keeps, with a
    /// line saying when to read it
    Add {
        project: String,
        name: String,
        #[command(flatten)]
        what: What,
        /// One line: what the agents are told of it
        #[arg(long)]
        description: String,
        #[arg(long)]
        json: bool,
    },
    /// Change what is given of a skill; the rest stays
    Edit {
        project: String,
        name: String,
        #[command(flatten)]
        what: What,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove a skill; the project's files are never touched
    Remove {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
}

/// What a skill is: one of a path, a text, a text's file.
#[derive(Args)]
#[group(multiple = false)]
pub struct What {
    /// A file or a folder, relative to the project folder
    #[arg(long)]
    path: Option<String>,
    /// The text, kept by Flottille in its data folder
    #[arg(long)]
    text: Option<String>,
    /// The text, read from this file and kept by Flottille
    #[arg(long, value_name = "FILE")]
    text_file: Option<PathBuf>,
}

impl What {
    fn into_params(self, params: &mut Value) -> Result<()> {
        if let Some(path) = self.path {
            params["path"] = path.into();
        }
        let text = match (self.text, self.text_file) {
            (Some(text), _) => Some(text),
            (None, Some(file)) => Some(
                std::fs::read_to_string(&file)
                    .map_err(|error| format!("cannot read {}: {error}", file.display()))?,
            ),
            (None, None) => None,
        };
        if let Some(text) = text {
            params["text"] = text.into();
        }
        Ok(())
    }
}

pub fn run(paths: &Paths, action: SkillAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let mut ask = |method: &str, params: Value| {
        client
            .request(method, params)
            .map_err(|error| Box::<dyn std::error::Error>::from(in_full(paths, &error.to_string())))
    };
    let (answer, json, text) = match action {
        SkillAction::List { project, json } => {
            let list = ask("skill.list", json!({"project": project}))?;
            let text = list_text(&project, &list);
            (list, json, text)
        }
        SkillAction::Show {
            project,
            name,
            json,
        } => {
            let skill = ask("skill.show", json!({"project": project, "name": name}))?;
            let mut text = format!(
                "Skill {name} of project {project}: {}\n{}",
                field(&skill, "description"),
                where_it_is(&skill)
            );
            if let Some(body) = skill["text"].as_str() {
                text.push_str(":\n");
                text.push_str(body.trim_end());
            }
            (skill, json, text)
        }
        SkillAction::Add {
            project,
            name,
            what,
            description,
            json,
        } => {
            let mut params = json!({"project": project, "name": name, "description": description});
            what.into_params(&mut params)?;
            let added = ask("skill.add", params)?;
            let text = format!("Added skill {name} to {project}: {}.", where_it_is(&added));
            (added, json, text)
        }
        SkillAction::Edit {
            project,
            name,
            what,
            description,
            json,
        } => {
            let mut params = json!({"project": project, "name": name});
            if let Some(description) = description {
                params["description"] = description.into();
            }
            what.into_params(&mut params)?;
            let edited = ask("skill.edit", params)?;
            let text = format!(
                "Changed skill {name} of {project}: {}.",
                where_it_is(&edited)
            );
            (edited, json, text)
        }
        SkillAction::Remove {
            project,
            name,
            json,
        } => {
            let removed = ask("skill.remove", json!({"project": project, "name": name}))?;
            let text = format!(
                "Removed skill {name} from {project}. The project's files were not touched."
            );
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

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

/// A path skill's path, or what a text skill is.
fn where_it_is(skill: &Value) -> String {
    match skill["path"].as_str() {
        Some(path) => path.to_owned(),
        None => "a text Flottille keeps".to_owned(),
    }
}

fn list_text(project: &str, list: &Value) -> String {
    let skills = list["skills"].as_array().map_or(&[][..], Vec::as_slice);
    if skills.is_empty() {
        return format!(
            "Project {project} has no skills. Add one with `flottille skill add {project} <name> \
             (--path <path> | --text <text>) --description <line>`."
        );
    }
    let names = skills
        .iter()
        .map(|s| field(s, "name").chars().count())
        .chain([4])
        .max()
        .unwrap_or(4);
    let places: Vec<String> = skills.iter().map(where_it_is).collect();
    let width = places
        .iter()
        .map(|p| p.chars().count())
        .chain([5])
        .max()
        .unwrap_or(5);
    let mut table = format!("{:names$}  {:width$}  DESCRIPTION\n", "NAME", "WHERE");
    for (skill, place) in skills.iter().zip(&places) {
        table.push_str(&format!(
            "{:names$}  {place:width$}  {}\n",
            field(skill, "name"),
            field(skill, "description"),
        ));
    }
    table.trim_end().to_owned()
}
