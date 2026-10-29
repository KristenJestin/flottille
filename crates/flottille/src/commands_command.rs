//! `flottille commands`: the commands the agents run, listed by project, and stopped (`docs/cli.md`).

use clap::Subcommand;
use flottille_core::ansi;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum CommandsAction {
    /// The commands running now, of a project or of all of them
    List {
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Stop a running command and everything it started: the agent that ran it is told
    Stop { run: String },
}

pub fn run(paths: &Paths, action: CommandsAction) -> Result<()> {
    let mut client = Client::connect_or_start(paths)?;
    match action {
        CommandsAction::List { project, json } => {
            let listed = client.request("command.list", json!({"project": project}))?;
            if json {
                println!("{listed}");
            } else {
                print!("{}", commands_text(&listed));
            }
        }
        CommandsAction::Stop { run } => {
            let stopped = client.request("command.stop", json!({"run": run}))?;
            let state = stopped["run"]["state"].as_str().unwrap_or("?");
            println!("Run {run} is {state}; its agent is told it was you.");
        }
    }
    Ok(())
}

fn commands_text(listed: &Value) -> String {
    let commands = listed["commands"].as_array().map_or(&[][..], Vec::as_slice);
    if commands.is_empty() {
        return "No command is running.\n".into();
    }
    let mut text = String::new();
    for command in commands {
        let place = match (command["project"].as_str(), command["workspace"].as_str()) {
            (Some(project), Some(workspace)) => format!("{project}/{workspace}"),
            (Some(project), None) => project.to_owned(),
            _ => "?".to_owned(),
        };
        text.push_str(&format!(
            "{} agent {} ({place}): {}\n",
            command["run"].as_str().unwrap_or("?"),
            command["agent"].as_str().unwrap_or("?"),
            command["line"].as_str().unwrap_or("?"),
        ));
        // The program's own colours are for the window: a terminal shows the words.
        let last = ansi::strip(command["last_line"].as_str().unwrap_or_default());
        if !last.is_empty() {
            text.push_str(&format!("    {last}\n"));
        }
    }
    text.push_str("Stop one with `flottille commands stop <run>`.\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_shown_with_its_agent_its_place_and_its_last_line_without_colours() {
        let listed = json!({"commands": [
            {"run": "r1", "agent": "3f9a1c0b7d2e", "project": "app", "workspace": "login",
                "line": "npm run dev", "last_line": "\u{1b}[32mready\u{1b}[0m on :3000"}]});

        assert_eq!(
            commands_text(&listed),
            "r1 agent 3f9a1c0b7d2e (app/login): npm run dev\n    ready on :3000\n\
             Stop one with `flottille commands stop <run>`.\n"
        );
        assert_eq!(
            commands_text(&json!({"commands": []})),
            "No command is running.\n"
        );
    }
}
