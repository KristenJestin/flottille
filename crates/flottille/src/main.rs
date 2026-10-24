mod client;
mod daemon;
mod projects;

use std::io::Write;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use flottille_core::paths::Paths;
use flottille_core::protocol::WatchParams;
use serde_json::Value;

use crate::client::Client;

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
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flottille: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
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
        Command::Watch { after } => {
            let mut client = Client::connect_or_start(&paths)?;
            client.request("events.watch", serde_json::to_value(WatchParams { after })?)?;
            let mut stdout = std::io::stdout().lock();
            while let Some(event) = client.next_event()? {
                // A closed pipe (`watch | head`) ends the watch quietly.
                if writeln!(stdout, "{}", serde_json::to_string(&event)?).is_err() {
                    return Ok(());
                }
            }
            return Err("the daemon closed the connection".into());
        }
    }
    Ok(())
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

    #[test]
    fn durations_are_short() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(192), "3m 12s");
        assert_eq!(duration(7_380), "2h 3m");
        assert_eq!(duration(90_000), "1d 1h");
    }
}
