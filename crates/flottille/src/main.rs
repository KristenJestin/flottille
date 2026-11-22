// The tool schemas are large `json!` literals: the default limit of 128 is too low for them.
#![recursion_limit = "256"]

mod about;
mod acp;
mod across;
mod actions;
mod adf;
mod agent_command;
mod agent_folders;
mod agents;
mod appearance;
mod approval_command;
mod approvals;
mod bridge;
mod changes_command;
mod chat;
mod children;
mod claude;
mod claude_import;
mod client;
mod codex;
mod codex_bridge;
mod command_command;
mod commands;
mod commit_checks;
mod compaction;
mod daemon;
mod deliveries;
mod desktop;
mod developer;
mod display;
mod drafts;
mod extensions;
mod files;
mod findings;
mod follow;
mod forge;
mod gate;
mod git;
mod hub_session;
mod import_command;
mod inactive;
mod inbox;
mod inbox_command;
mod items;
mod judge;
mod landlock;
mod launches;
mod leaves;
mod link;
mod locks;
mod mcp;
mod message_checks;
mod models;
mod mounts;
mod nofollow;
mod notifications;
mod notifications_command;
mod opencode;
mod people;
mod pictures;
mod pilot;
mod place;
mod posts;
mod program;
mod programs;
mod project_command;
mod projects;
mod providers;
mod push_folder;
mod pushed;
mod quota;
mod read_commands;
mod reads;
mod ready;
mod remote;
mod remote_command;
mod remote_config;
mod results;
mod roles;
mod rules;
mod run_methods;
mod runs;
mod runs_command;
mod service_command;
mod services;
mod session_name;
mod session_read;
mod setup;
mod show;
mod sign_in;
mod skill_command;
mod skills;
mod ssh_agent;
mod subscriptions;
mod sweep;
mod tasks;
mod test_time;
mod tickets;
mod titles;
mod travel;
mod tree;
mod tunnel;
mod tunnel_command;
mod turn_failures;
mod update;
mod updates;
mod vault;
mod vault_command;
mod watchdog;
mod web;
mod whatsnew;
mod workspace_command;
mod workspaces;
mod workspaces_folder;

use std::io::Write;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use flottille_core::paths::Paths;
use flottille_core::protocol::WatchParams;
use serde_json::{Value, json};

use crate::agent_command::AgentAction;
use crate::client::Client;
use crate::command_command::CommandAction;
use crate::project_command::ProjectAction;
use crate::remote_command::RemoteAction;
use crate::runs_command::RunsAction;
use crate::service_command::ServiceAction;
use crate::skill_command::SkillAction;
use crate::tunnel_command::TunnelAction;
use crate::vault_command::VaultAction;
use crate::workspace_command::WorkspaceAction;

pub const VERSION: &str = env!("FLOTTILLE_VERSION");

#[derive(Parser)]
#[command(name = "flottille", version = VERSION, about = "Runs coding agents on your own machine")]
struct Cli {
    /// Drive the user's machine of this name: each request is signed with this device's key and
    /// carried by this machine's daemon (`FLOTTILLE_MACHINE` too)
    #[arg(long, global = true, value_name = "NAME")]
    machine: Option<String>,
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
    /// The vault: secrets kept by the keyring, that agents use through Flottille's tools and
    /// never see
    Vault {
        #[command(subcommand)]
        action: VaultAction,
    },
    /// Flottille on the user's other machines: the hub, the members, remote on and off
    Remote {
        #[command(subcommand)]
        action: RemoteAction,
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
    /// A run's port on another machine reached on a local port (`--machine`): opened, listed,
    /// closed
    Tunnel {
        #[command(subcommand)]
        action: TunnelAction,
    },
    /// The services of the machine, shared by every project: listed, written, started, stopped
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Allow a request: its call is checked again, performed, and its result handed to the agent
    Approve {
        number: u64,
        /// For a workspace's removal: remove it even with work that would be lost
        #[arg(long)]
        discard: bool,
    },
    /// Deny a request: the agent is told nothing was done
    Deny { number: u64 },
    /// Whether Claude agents can run here: claude, its version and its login
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// The developer mode: a log of what Flottille decides and why, off by default
    Developer {
        #[command(subcommand)]
        action: DeveloperAction,
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
    /// The notifications Flottille keeps for you, read and unread (`read` marks them read)
    Notifications {
        /// Only the unread ones
        #[arg(long)]
        unread: bool,
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<NotificationsAction>,
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
    /// Read a page given on standard input, for the daemon's `web` tool, under the limits the
    /// daemon set: what it holds, as JSON
    #[command(name = "read-page", hide = true)]
    ReadPage { kind: String, url: String },
    /// Open the window: your projects, where their work happens, their sessions, and what waits
    /// for you
    Window {
        /// The page to open, as a window that started again from a new build names it
        #[arg(long, hide = true)]
        open: Option<String>,
    },
    /// Replace Flottille with the latest build of a channel, its sum checked first
    Update {
        /// stable, beta or alpha; the one last chosen, or this build's, by default
        #[arg(long, value_parser = ["stable", "beta", "alpha"])]
        channel: Option<String>,
        /// The branch whose alpha to take (`feat/46-packages`), with `--channel alpha`; by default
        /// the branch last chosen with alpha, else this build's
        #[arg(long)]
        branch: Option<String>,
        /// Install this release exactly (`0.3.0-beta.12`), of any channel; the channel
        /// remembered stays as it is
        #[arg(long, value_name = "X.Y.Z[-PRE]", conflicts_with_all = ["channel", "branch"])]
        version: Option<String>,
        /// Install it even when it is this version, or an older one
        #[arg(long, conflicts_with = "stage")]
        force: bool,
        /// Download it beside the binary, applied at the daemon's next start, without touching
        /// what runs
        #[arg(long)]
        stage: bool,
    },
    /// What changed since the version you last ran, or since one you name
    Changes {
        /// Count from this version instead of the one last run
        #[arg(long, value_name = "X.Y.Z[-PRE]")]
        since: Option<String>,
        #[arg(long)]
        json: bool,
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
    #[command(flatten)]
    Extensions(extensions::Command),
}

#[derive(Subcommand)]
enum NotificationsAction {
    /// Mark a notification read, or all of them
    Read {
        /// The notification's number, as `flottille notifications` gives it
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        id: Option<u64>,
        /// Mark every notification read
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum ProviderAction {
    /// Each provider, whether its agents can run, and the Auto judge
    List {
        #[arg(long)]
        json: bool,
    },
    /// Let agents run on it
    Enable { name: String },
    /// Start no more agents on it: those running now go on, and fail at their next start
    Disable { name: String },
    /// Choose the provider, model and effort a new session starts on
    NewSession {
        /// The provider; with no model or effort, they are the ones it proposes
        #[arg(required_unless_present = "clear")]
        provider: Option<String>,
        #[arg(long, requires = "provider")]
        model: Option<String>,
        #[arg(long, requires = "provider")]
        effort: Option<String>,
        /// Go back to what the provider proposes
        #[arg(long, conflicts_with = "provider")]
        clear: bool,
    },
}

#[derive(Subcommand)]
enum DeveloperAction {
    /// Whether it is on, and where its log is
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Log what Flottille decides from now on
    On,
    /// Stop logging
    Off,
    /// The findings agents filed about Flottille itself, with their occurrences
    Reports {
        /// The findings as JSON: the export the lead agent reads
        #[arg(long)]
        json: bool,
        /// Write them to a file of the state folder, and say where
        #[arg(long)]
        export: bool,
        #[command(subcommand)]
        action: Option<ReportsAction>,
    },
}

#[derive(Subcommand)]
enum ReportsAction {
    /// Mark a finding filed, with the address of its issue
    Filed {
        /// The finding's number
        id: u64,
        /// The issue's address
        issue: String,
    },
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
    // What runs here, or is this machine's own process, drives no other: it refuses the flag
    // and ignores the variable, which a shell may have exported for the commands it aims.
    let here = match &cli.command {
        Command::Daemon { action: None } => Some("daemon"),
        Command::Window { .. } => Some("window"),
        Command::Update { .. } => Some("update"),
        Command::Mcp => Some("mcp"),
        Command::Bridge { .. } => Some("bridge"),
        Command::Remote { .. } => Some("remote"),
        // These check this machine alone (its `claude`, its keyring's login, its page reader).
        Command::Doctor { .. } => Some("doctor"),
        Command::Vault {
            action: VaultAction::Provide { .. },
        } => Some("vault provide"),
        Command::ReadPage { .. } => Some("read-page"),
        _ => None,
    };
    let flagged = cli.machine.clone().filter(|name| !name.is_empty());
    // `vault share` and `connection share` are this machine's requests: their --machine names
    // where the entry or the connection goes.
    let mut cli = cli;
    let sharing = match &mut cli.command {
        Command::Vault {
            action: VaultAction::Share { machine, .. },
        }
        | Command::Extensions(extensions::Command::Connection {
            action: extensions::ConnectionAction::Share { machine, .. },
        }) => {
            *machine = flagged.clone();
            true
        }
        _ => false,
    };
    if let (Some(command), Some(_)) = (here, &flagged) {
        return Err(format!(
            "flottille {command} is this machine's own: it drives no other (leave out --machine)"
        )
        .into());
    }
    // The page's reader runs with no environment: it needs no folder of Flottille's.
    if let Command::ReadPage { kind, url } = &cli.command {
        return Ok(web::read_page(kind, url));
    }
    // agent-browser runs it with the environment of a connection's server, which names no socket
    // of the daemon's: it reaches the login that runs at the socket it is given.
    if let Command::Vault {
        action: VaultAction::Provide { socket },
    } = &cli.command
    {
        return Ok(extensions::vault_provide(socket));
    }
    let paths = Paths::from_env()?;
    let machine = flagged.or_else(|| {
        std::env::var("FLOTTILLE_MACHINE")
            .ok()
            .filter(|name| !name.is_empty())
    });
    // A tunnel's list and closing are this machine's own: the flag only filters them.
    let own_tunnels =
        matches!(&cli.command, Command::Tunnel { action } if tunnel_command::is_local(action));
    if here.is_none()
        && !sharing
        && !own_tunnels
        && let Some(machine) = machine.clone()
    {
        client::aim(machine);
    }
    match cli.command {
        Command::Daemon { action: None } => daemon::run(&paths)?,
        Command::Daemon {
            action: Some(DaemonAction::Stop),
        } if client::aimed().is_some() => {
            Client::connect_or_start(&paths)?.request("daemon.stop", json!({}))?;
            println!(
                "The daemon of {} is stopping.",
                client::aimed().unwrap_or_default()
            );
        }
        Command::Daemon {
            action: Some(DaemonAction::Stop),
        } => match Client::connect(&paths)? {
            None => println!("No daemon is running."),
            Some(client) => match client::stop(&paths, client)? {
                client::Stopped::Asked => println!("The daemon stopped."),
                client::Stopped::Ended(why, processes) => println!(
                    "The daemon did not stop ({why}): it was ended, with the {processes} \
                     process(es) it ran."
                ),
            },
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
        Command::Vault { action } => return vault_command::run(&paths, action),
        Command::Remote { action } => return remote_command::run(&paths, action),
        Command::Workspace { action } => return workspace_command::run(&paths, action),
        Command::Agent { action } => return agent_command::run(&paths, action),
        Command::Named { action } => return command_command::run(&paths, action),
        Command::Doctor { json } => return agent_command::doctor(&paths, json),
        Command::Provider { action } => {
            let (method, name) = match action {
                ProviderAction::NewSession {
                    provider,
                    model,
                    effort,
                    clear: _,
                } => {
                    let answer = Client::connect_or_start(&paths)?.request(
                        "provider.set_new_session",
                        serde_json::json!({"provider": provider, "model": model, "effort": effort}),
                    )?;
                    print!("{}", providers::new_session_text(&answer["new_session"]));
                    return Ok(ExitCode::SUCCESS);
                }
                ProviderAction::List { json } => {
                    let answer = Client::connect_or_start(&paths)?
                        .request("provider.list", serde_json::json!({}))?;
                    if json {
                        println!("{answer}");
                    } else {
                        print!("{}", providers::list_text(&answer));
                    }
                    return Ok(ExitCode::SUCCESS);
                }
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
        Command::Developer { action } => {
            let mut client = Client::connect_or_start(&paths)?;
            let (answer, json) = match action {
                DeveloperAction::Show { json } => {
                    (client.request("developer.show", json!({}))?, json)
                }
                DeveloperAction::On => (
                    client.request("developer.set", json!({"log": true}))?,
                    false,
                ),
                DeveloperAction::Off => (
                    client.request("developer.set", json!({"log": false}))?,
                    false,
                ),
                DeveloperAction::Reports {
                    json,
                    export,
                    action,
                } => {
                    match (action, export) {
                        (Some(ReportsAction::Filed { id, issue }), _) => {
                            client.request(
                                "developer.report_filed",
                                json!({"id": id, "issue": issue}),
                            )?;
                            println!("Finding #{id} is marked filed: {issue}");
                        }
                        (None, true) => {
                            let said = client.request("developer.reports_export", json!({}))?;
                            println!(
                                "{} finding(s) written to {}",
                                said["count"],
                                said["file"].as_str().unwrap_or_default()
                            );
                        }
                        (None, false) => {
                            let said = client.request("developer.reports", json!({}))?;
                            if json {
                                println!("{said}");
                            } else {
                                print!("{}", findings::listing(&said));
                            }
                        }
                    }
                    return Ok(ExitCode::SUCCESS);
                }
            };
            if json {
                println!("{answer}");
            } else {
                println!(
                    "Developer mode is {}. Its log: {}",
                    if answer["log"] == true { "on" } else { "off" },
                    answer["file"].as_str().unwrap_or_default()
                );
            }
        }
        Command::Runs { action } => runs_command::run(&paths, action)?,
        Command::Tunnel { action } => tunnel_command::run(&paths, action, machine)?,
        Command::Service { action } => service_command::run(&paths, action)?,
        Command::Approvals { json } => approval_command::approvals(&paths, json)?,
        Command::Approve { number, discard } => approval_command::approve(&paths, number, discard)?,
        Command::Deny { number } => approval_command::deny(&paths, number)?,
        Command::Inbox { after, limit, json } => inbox_command::inbox(&paths, after, limit, json)?,
        Command::Notifications {
            unread,
            json,
            action,
        } => match action {
            Some(NotificationsAction::Read { id, all }) => {
                notifications_command::read(&paths, id, all)?
            }
            None => notifications_command::list(&paths, unread, json)?,
        },
        Command::Mcp => pilot::run(&paths)?,
        Command::Extensions(command) => return extensions::run(&paths, command),
        Command::Changes { since, json } => changes_command::run(&paths, since, json)?,
        Command::Update {
            channel,
            branch,
            version,
            force,
            stage,
        } => return update::run(&paths, channel, branch, version, force, stage),
        Command::Bridge {
            agent: BridgeAgent::Claude,
        } => return bridge::run(&paths),
        Command::Bridge {
            agent: BridgeAgent::Codex,
        } => return codex_bridge::run(&paths),
        Command::ReadPage { .. } => unreachable!("the page's reader returns before the paths"),
        Command::Window { open } => {
            // One window: a second command brings the open one to the front and leaves. When
            // that cannot be told (no runtime folder to hold the place), another window is
            // better than none.
            let instance = match flottille_window::instance::acquire(&paths) {
                Ok(flottille_window::instance::Start::First(instance)) => Some(instance),
                Ok(flottille_window::instance::Start::Raised) => {
                    println!("Flottille's window is already open: asked it to come to the front.");
                    return Ok(ExitCode::SUCCESS);
                }
                Err(error) => {
                    eprintln!("flottille: cannot tell whether a window is open: {error}");
                    None
                }
            };
            let device = remote_command::window_device(&paths);
            // What only the user's device may ask (`vault.share`) is signed with its key, read
            // when it is asked, as the command line reads it.
            let signing = paths.clone();
            flottille_window::link::sign_with(std::sync::Arc::new(move |machine, request| {
                let key = remote_command::device_key(&signing)
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                let signed = flottille_link::Request::sign(
                    &key,
                    &request.id,
                    machine,
                    &request.method,
                    &request.params,
                    std::time::SystemTime::now(),
                )
                .map_err(|error| std::io::Error::other(error.to_string()))?;
                serde_json::to_value(signed).map_err(std::io::Error::other)
            }));
            let paths = paths.clone();
            flottille_window::run(
                std::sync::Arc::new(move || {
                    Client::connect_or_start(&paths)
                        .map(Client::into_stream)
                        .map_err(|error| std::io::Error::other(error.to_string()))
                }),
                Some(device),
                instance,
                flottille_window::Build {
                    full: VERSION.to_owned(),
                    reexec: std::sync::Arc::new(window_again),
                    open,
                },
            );
        }
        Command::Watch { after, forge } if client::aimed().is_some() => {
            return watch_another(&paths, after, &forge);
        }
        Command::Watch { after, forge } => {
            let mut client = Client::connect_or_start(&paths)?;
            client.watch(WatchParams {
                after,
                client: None,
                forge: None,
            })?;
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

/// The window becomes `flottille window` of the binary that replaced its own (`docs/releases.md`,
/// "Updating"), on `page`: false, and nothing done, when the binary was not replaced.
fn window_again(page: Option<String>) -> bool {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let new = client::program_to_start(exe.clone());
    if new == exe {
        return false;
    }
    let mut command = std::process::Command::new(&new);
    command.arg("window");
    if let Some(page) = page {
        command.args(["--open", &page]);
    }
    let error = command.exec();
    eprintln!(
        "flottille: the window cannot start again from {}: {error}",
        new.display()
    );
    false
}

/// `flottille watch` of another machine: its events, carried by this machine's daemon. When the
/// link drops, or the machine is off, the watch waits and resumes after the last event it printed,
/// each time with a request signed again.
fn watch_another(
    paths: &Paths,
    after: Option<u64>,
    forge: &[String],
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let mut after = after;
    let mut said: Option<String> = None;
    let mut stdout = std::io::stdout().lock();
    loop {
        let mut client = Client::connect_or_start(paths)?;
        // The forge is asked for on the watch's own stream: the other machine ends each request
        // of its own at its answer, and a watch of a workspace lasts as long as its stream.
        match client.watch(WatchParams {
            after,
            client: None,
            forge: match forge {
                [project, name] => Some([project.clone(), name.clone()]),
                _ => None,
            },
        }) {
            Ok(answer) => {
                said = None;
                // Without `after`, the stream starts after the number it answered: a resume
                // starts there too.
                after = after.or_else(|| answer["after"].as_u64());
                while let Ok(Some(event)) = client.next_event() {
                    after = Some(event.seq);
                    if writeln!(stdout, "{}", serde_json::to_string(&event)?).is_err() {
                        return Ok(ExitCode::SUCCESS);
                    }
                }
            }
            Err(error)
                if error
                    .downcast_ref::<client::DaemonError>()
                    .is_some_and(|error| error.code == "unavailable") =>
            {
                let error = error.to_string();
                if said.as_deref() != Some(error.as_str()) {
                    eprintln!("flottille: {error}; the watch resumes once it is back");
                    said = Some(error);
                }
            }
            Err(error) => return Err(error),
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
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
