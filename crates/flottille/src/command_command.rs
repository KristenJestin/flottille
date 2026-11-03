//! `flottille command`: the commands of a project, named instructions that `/name` sends to an
//! agent (`docs/commands.md`, `docs/cli.md`).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;
use crate::import_command::{self, Import};
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum CommandAction {
    /// The commands of a project
    List {
        project: String,
        #[arg(long)]
        json: bool,
    },
    /// One command: its description and its instructions
    Show {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Add a command to the project's configuration, or one of Flottille's templates
    Add {
        project: String,
        /// Its name; the template's when it is left out
        #[arg(required_unless_present = "template")]
        name: Option<String>,
        /// Write one of Flottille's commands: open-prs, merge-stack or close
        #[arg(long, conflicts_with_all = ["description", "instructions", "instructions_file"])]
        template: Option<String>,
        /// One line: what it does
        #[arg(long, required_unless_present = "template")]
        description: Option<String>,
        /// What the agent is sent
        #[arg(
            long,
            conflicts_with = "instructions_file",
            required_unless_present_any = ["instructions_file", "template"]
        )]
        instructions: Option<String>,
        /// The instructions, read from this file
        #[arg(long, value_name = "FILE")]
        instructions_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Change what is given of a command; the rest stays
    Edit {
        project: String,
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, conflicts_with = "instructions_file")]
        instructions: Option<String>,
        #[arg(long, value_name = "FILE")]
        instructions_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List the commands of the project's .claude folder, or import them: each reads its file
    /// every time it is used, nothing is copied nor written in the project
    Import(Import),
    /// Remove a command from the project's configuration
    Remove {
        project: String,
        name: String,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(paths: &Paths, action: CommandAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    if let CommandAction::Import(import) = action {
        return import_command::run(
            &mut client,
            "command",
            |error| in_full(paths, error),
            import,
        );
    }
    let mut ask = |method: &str, params: Value| {
        client
            .request(method, params)
            .map_err(|error| Box::<dyn std::error::Error>::from(in_full(paths, &error.to_string())))
    };
    match action {
        CommandAction::Import(_) => unreachable!("imports return above"),
        CommandAction::List { project, json } => {
            let list = ask("command.list", json!({"project": project}))?;
            if json {
                println!("{list}");
            } else {
                print!("{}", list_text(&list));
            }
        }
        CommandAction::Show {
            project,
            name,
            json,
        } => {
            let command = ask("command.show", json!({"project": project, "name": name}))?;
            if json {
                println!("{command}");
            } else {
                print!("{}", show_text(&project, &command));
            }
        }
        CommandAction::Add {
            project,
            name,
            template,
            description,
            instructions,
            instructions_file,
            json,
        } => {
            let mut params = json!({"project": project, "name": name});
            if let Some(template) = template {
                params["template"] = template.into();
            } else {
                let instructions = instructions_of(instructions, instructions_file)?
                    .ok_or("give the instructions, with --instructions or --instructions-file")?;
                params["description"] = description.into();
                params["instructions"] = instructions.into();
            }
            let added = ask("command.add", params)?;
            if json {
                println!("{added}");
            } else {
                println!(
                    "Added command {} to project {project}: /{} in a session's composer sends \
                     it. Edit it in the window, or with `flottille command edit`.",
                    text(&added["name"]),
                    text(&added["name"])
                );
            }
        }
        CommandAction::Edit {
            project,
            name,
            description,
            instructions,
            instructions_file,
            json,
        } => {
            let mut params = json!({"project": project, "name": name});
            if let Some(description) = description {
                params["description"] = description.into();
            }
            if let Some(instructions) = instructions_of(instructions, instructions_file)? {
                params["instructions"] = instructions.into();
            }
            let edited = ask("command.edit", params)?;
            if json {
                println!("{edited}");
            } else {
                println!("Changed command {name} of project {project}.");
            }
        }
        CommandAction::Remove {
            project,
            name,
            json,
        } => {
            let removed = ask("command.remove", json!({"project": project, "name": name}))?;
            if json {
                println!("{removed}");
            } else {
                println!("Removed command {name} from project {project}.");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn instructions_of(text: Option<String>, file: Option<PathBuf>) -> Result<Option<String>> {
    Ok(match (text, file) {
        (Some(text), _) => Some(text),
        (None, Some(file)) => Some(
            std::fs::read_to_string(&file)
                .map_err(|error| format!("cannot read {}: {error}", file.display()))?,
        ),
        (None, None) => None,
    })
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn list_text(list: &Value) -> String {
    let commands = list["commands"].as_array().map_or(&[][..], Vec::as_slice);
    if commands.is_empty() {
        return format!(
            "Project {} has no command: add one with `flottille command add`.\n",
            text(&list["project"])
        );
    }
    commands
        .iter()
        .map(|command| {
            format!(
                "/{}  {}\n",
                text(&command["name"]),
                text(&command["description"])
            )
        })
        .collect()
}

fn show_text(project: &str, command: &Value) -> String {
    let head = format!(
        "Command {} of project {project}: {}\n",
        text(&command["name"]),
        text(&command["description"]),
    );
    // A command that points at a file says which, and why its text is missing when it is.
    let origin = match command["path"].as_str() {
        Some(path) => format!("It reads {path} each time it is used.\n"),
        None => String::new(),
    };
    if let Some(why) = command["problem"].as_str() {
        return format!("{head}{origin}Its instructions cannot be read: {why}\n");
    }
    let instructions: String = text(&command["instructions"])
        .lines()
        .map(|line| format!("  {line}\n"))
        .collect();
    format!("{head}{origin}Instructions:\n{instructions}")
}
