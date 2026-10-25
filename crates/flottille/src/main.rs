mod acp;
mod agent_command;
mod agents;
mod approval_command;
mod approvals;
mod claude;
mod client;
mod daemon;
mod deliveries;
mod files;
mod gate;
mod git;
mod inbox;
mod inbox_command;
mod mcp;
mod pilot;
mod place;
mod project_command;
mod projects;
mod roles;
mod runs;
mod sweep;
mod tree;
mod workspace_command;
mod workspaces;

use std::io::Write;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use flottille_core::paths::Paths;
use flottille_core::protocol::WatchParams;
use serde_json::Value;

use crate::agent_command::AgentAction;
use crate::client::Client;
use crate::project_command::ProjectAction;
use crate::workspace_command::WorkspaceAction;

pub const VERSION: &str = env!("FLOTTILLE_VERSION");

#[derive(Parser)]
#[command(name = "flottille", version = VERSION, about = "Runs coding agents on your own machine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon in the foreground
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonAction>,
    },
    /// The daemon's version, uptime and agent counts
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Register projects and look after their configuration
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Make, follow and remove the workspaces of a project
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// Start, write to, read and stop Claude agents
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// The requests of your agents that wait for your answer
    Approvals {
        #[arg(long)]
        json: bool,
    },
    /// Allow a request: its call is checked again, performed, and its result handed to the agent
    Approve {
        number: u64,
        /// Allow the same call, exactly, for the whole tree of the agent's root from now on
        /// (never for a sensitive place)
        #[arg(long)]
        for_tree: bool,
    },
    /// Deny a request: the agent is told nothing was done
    Deny { number: u64 },
    /// Whether Claude agents can run here: Node, claude, its login, the adapter
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// What your root agents sent you: their reports, their questions, their failures
    Inbox {
        /// List what came after this event number
        #[arg(long, value_name = "N")]
        after: Option<u64>,
        /// At most this many entries (default 50)
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Serve the pilot MCP door on standard input and output, for an agent outside Flottille
    Mcp,
    /// Print the daemon's events, one JSON object per line
    Watch {
        /// Print the stored events numbered after this one first
        #[arg(long, value_name = "N")]
        after: Option<u64>,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Stop the running daemon
    Stop,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("flottille: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let paths = Paths::from_env()?;
    match cli.command {
        Command::Daemon { action: None } => daemon::run(&paths)?,
        Command::Daemon {
            action: Some(DaemonAction::Stop),
        } => match Client::connect(&paths)? {
            None => println!("No daemon is running."),
            Some(mut client) => {
                client.request("daemon.stop", Value::Null)?;
                client::wait_until_stopped(&paths)?;
                println!("The daemon stopped.");
            }
        },
        Command::Status { json } => {
            let status = Client::connect_or_start(&paths)?.request("daemon.status", Value::Null)?;
            if json {
                println!("{status}");
            } else {
                print_status(&status);
            }
        }
        Command::Project { action } => return project_command::run(&paths, action),
        Command::Workspace { action } => return workspace_command::run(&paths, action),
        Command::Agent { action } => return agent_command::run(&paths, action),
        Command::Doctor { json } => return agent_command::doctor(&paths, json),
        Command::Approvals { json } => approval_command::approvals(&paths, json)?,
        Command::Approve { number, for_tree } => {
            approval_command::approve(&paths, number, for_tree)?
        }
        Command::Deny { number } => approval_command::deny(&paths, number)?,
        Command::Inbox { after, limit, json } => inbox_command::inbox(&paths, after, limit, json)?,
        Command::Mcp => pilot::run(&paths)?,
        Command::Watch { after } => {
            let mut client = Client::connect_or_start(&paths)?;
            client.request("events.watch", serde_json::to_value(WatchParams { after })?)?;
            let mut stdout = std::io::stdout().lock();
            while let Some(event) = client.next_event()? {
                // A closed pipe (`watch | head`) ends the watch quietly.
                if writeln!(stdout, "{}", serde_json::to_string(&event)?).is_err() {
                    return Ok(ExitCode::SUCCESS);
                }
            }
            return Err("the daemon closed the connection".into());
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_status(status: &Value) {
    let count = |name: &str| status["agents"][name].as_u64().unwrap_or(0);
    let uptime = status["uptime_s"].as_u64().unwrap_or(0);
    println!("Flottille {}", status["version"].as_str().unwrap_or("?"));
    println!("Daemon: pid {}, up {}", status["pid"], duration(uptime));
    println!(
        "Agents: {} running, {} waiting for you, {} failed",
        count("running"),
        count("waiting_for_user"),
        count("failed")
    );
}

fn duration(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, _) => format!("{minutes}m {}s", seconds % 60),
        (0, _, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/cli.md` has a section, `### \`flottille <command>\``, for every command, and no
    /// private path.
    #[test]
    fn every_command_has_its_section_in_the_cli_docs() {
        use clap::CommandFactory;
        let docs = include_str!("../../../docs/cli.md");
        fn walk(command: &clap::Command, path: &str, docs: &str, missing: &mut Vec<String>) {
            for sub in command.get_subcommands().filter(|c| c.get_name() != "help") {
                let path = format!("{path} {}", sub.get_name());
                if !docs.contains(&format!("### `{path}`"))
                    && !docs.contains(&format!("### `{path} "))
                {
                    missing.push(path.clone());
                }
                walk(sub, &path, docs, missing);
            }
        }
        let mut missing = Vec::new();
        walk(&Cli::command(), "flottille", docs, &mut missing);
        assert!(missing.is_empty(), "docs/cli.md misses {missing:?}");
        assert!(!docs.contains("/home/"), "a private path is in docs/cli.md");
    }

    #[test]
    fn durations_are_short() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(192), "3m 12s");
        assert_eq!(duration(7_380), "2h 3m");
        assert_eq!(duration(90_000), "1d 1h");
    }
}
