mod acp;
mod actions;
mod agent_command;
mod agents;
mod approval_command;
mod approvals;
mod bridge;
mod chat;
mod claude;
mod client;
mod codex;
mod codex_bridge;
mod command_command;
mod commands;
mod daemon;
mod deliveries;
mod files;
mod follow;
mod forge;
mod gate;
mod git;
mod inbox;
mod inbox_command;
mod judge;
mod mcp;
mod opencode;
mod pilot;
mod place;
mod program;
mod project_command;
mod projects;
mod providers;
mod roles;
mod rules;
mod run_methods;
mod runs;
mod runs_command;
mod setup;
mod skill_command;
mod skills;
mod sweep;
mod tasks;
mod tickets;
mod tree;
mod update;
mod workspace_command;
mod workspaces;

use std::io::Write;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use flottille_core::paths::Paths;
use flottille_core::protocol::WatchParams;
use serde_json::Value;

use crate::agent_command::AgentAction;
use crate::client::Client;
use crate::command_command::CommandAction;
use crate::project_command::ProjectAction;
use crate::runs_command::RunsAction;
use crate::skill_command::SkillAction;
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
    /// The skills of a project: texts its agents read with a tool when their task needs them
    #[command(alias = "reference")]
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Make, follow and remove the workspaces of a project
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// The commands of a project: named instructions, sent to an agent by /name
    #[command(name = "command", alias = "procedure")]
    Named {
        #[command(subcommand)]
        action: CommandAction,
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
    /// The commands your agents run and the tasks you ran: the ones running now, and stopping one
    #[command(alias = "commands")]
    Runs {
        #[command(subcommand)]
        action: RunsAction,
    },
    /// Allow a request: its call is checked again, performed, and its result handed to the agent
    Approve { number: u64 },
    /// Deny a request: the agent is told nothing was done
    Deny { number: u64 },
    /// Whether Claude agents can run here: claude, its version and its login
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Enable or disable a provider in providers.toml
    Provider {
        #[command(subcommand)]
        action: ProviderAction,
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
    /// Speak ACP on standard input and output in front of an agent CLI: what the daemon starts
    /// for each agent
    #[command(hide = true)]
    Bridge {
        #[command(subcommand)]
        agent: BridgeAgent,
    },
    /// Open the window: your projects, where their work happens, their sessions, and what waits
    /// for you
    Window,
    /// Replace Flottille with the latest build of a channel, its sum checked first
    Update {
        /// stable, beta or alpha; the channel of this build by default
        #[arg(long, value_parser = ["stable", "beta", "alpha"])]
        channel: Option<String>,
        /// The branch whose alpha to take (`feat/46-packages`), with `--channel alpha`; this
        /// build's by default
        #[arg(long)]
        branch: Option<String>,
        /// Install it even when it is this version, or an older one
        #[arg(long)]
        force: bool,
    },
    /// Print the daemon's events, one JSON object per line
    Watch {
        /// Print the stored events numbered after this one first
        #[arg(long, value_name = "N")]
        after: Option<u64>,
        /// Watch this workspace too: its pull requests are read again about once a minute while
        /// this runs
        #[arg(long, num_args = 2, value_names = ["PROJECT", "WORKSPACE"])]
        forge: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ProviderAction {
    /// Let agents run on it
    Enable { name: String },
    /// Start no more agents on it: those running now go on, and fail at their next start
    Disable { name: String },
}

#[derive(Subcommand)]
enum BridgeAgent {
    /// The user's own `claude`, in its stream-json mode
    Claude,
    /// The user's own `codex`, its app server on stdio
    Codex,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Stop the running daemon
    Stop,
}

/// The command line, its help and its errors naming the program as it was called.
fn parse() -> Cli {
    let name = program::name();
    let mut matches = Cli::command().name(name).bin_name(name).get_matches();
    Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|error| error.exit())
}

fn main() -> ExitCode {
    let cli = parse();
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{}: {error}", program::name());
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
                let pid = client::locked_by(&paths);
                client.request("daemon.stop", Value::Null)?;
                client::wait_until_stopped(&paths, &pid)?;
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
        Command::Skill { action } => return skill_command::run(&paths, action),
        Command::Workspace { action } => return workspace_command::run(&paths, action),
        Command::Agent { action } => return agent_command::run(&paths, action),
        Command::Named { action } => return command_command::run(&paths, action),
        Command::Doctor { json } => return agent_command::doctor(&paths, json),
        Command::Provider { action } => {
            let (method, name) = match action {
                ProviderAction::Enable { name } => ("provider.enable", name),
                ProviderAction::Disable { name } => ("provider.disable", name),
            };
            let answer = Client::connect_or_start(&paths)?
                .request(method, serde_json::json!({"name": name}))?;
            let name = answer["name"].as_str().unwrap_or_default();
            if let Some(warning) = answer["warning"].as_str() {
                eprintln!("flottille: {warning}");
            }
            if answer["enabled"] == true {
                println!("{name} is enabled.");
            } else {
                println!(
                    "{name} is disabled: its agents running now go on, and fail at their next start."
                );
            }
        }
        Command::Runs { action } => runs_command::run(&paths, action)?,
        Command::Approvals { json } => approval_command::approvals(&paths, json)?,
        Command::Approve { number } => approval_command::approve(&paths, number)?,
        Command::Deny { number } => approval_command::deny(&paths, number)?,
        Command::Inbox { after, limit, json } => inbox_command::inbox(&paths, after, limit, json)?,
        Command::Mcp => pilot::run(&paths)?,
        Command::Update {
            channel,
            branch,
            force,
        } => return update::run(&paths, channel, branch, force),
        Command::Bridge {
            agent: BridgeAgent::Claude,
        } => return bridge::run(&paths),
        Command::Bridge {
            agent: BridgeAgent::Codex,
        } => return codex_bridge::run(&paths),
        Command::Window => {
            let paths = paths.clone();
            flottille_window::run(std::sync::Arc::new(move || {
                Client::connect_or_start(&paths)
                    .map(Client::into_stream)
                    .map_err(|error| std::io::Error::other(error.to_string()))
            }));
        }
        Command::Watch { after, forge } => {
            let mut client = Client::connect_or_start(&paths)?;
            client.request("events.watch", serde_json::to_value(WatchParams { after })?)?;
            // Not waited for: its answer is not an event, and the events of its read come
            // meanwhile.
            if let [project, name] = &forge[..] {
                client.send(
                    "workspace.forge",
                    serde_json::json!({"project": project, "name": name, "watch": true}),
                )?;
            }
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
