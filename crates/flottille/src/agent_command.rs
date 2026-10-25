//! `flottille agent` and `flottille doctor`: a minimal way to run a Claude agent and read it, and
//! what the machine needs for one (`docs/agents.md`).

use std::io::Write;
use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::protocol::{AgentName, AgentRun, WatchParams};
use serde_json::Value;

use crate::claude;
use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum AgentAction {
    /// Start a Claude agent in a project with a first message, and print its reply
    Run {
        project: String,
        message: String,
        /// The workspace it works in (default: the project folder itself)
        #[arg(long)]
        workspace: Option<String>,
        /// The model (default: the agent's own); never replaced by another
        #[arg(long)]
        model: Option<String>,
        /// Print the agent, as `agent show --json` does, once its turn ended
        #[arg(long)]
        json: bool,
    },
    /// An agent and its conversation
    Show {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Install the Claude adapter Flottille runs, at its pinned version, into Flottille's data
    /// folder (with npm, never npx)
    Install,
}

pub fn run(paths: &Paths, action: AgentAction) -> Result<ExitCode> {
    match action {
        AgentAction::Run {
            project,
            message,
            workspace,
            model,
            json,
        } => run_agent(
            paths,
            AgentRun {
                project,
                workspace,
                model,
                effort: None,
                mode: None,
                message,
            },
            json,
        ),
        AgentAction::Show { agent, json } => {
            let shown = Client::connect_or_start(paths)?.request(
                "agent.show",
                serde_json::to_value(AgentName { agent, after: None })?,
            )?;
            if json {
                println!("{shown}");
            } else {
                print!("{}", show_text(&shown));
            }
            Ok(ExitCode::SUCCESS)
        }
        AgentAction::Install => {
            println!(
                "Installing {}@{} into {}",
                claude::ADAPTER,
                claude::ADAPTER_VERSION,
                claude::adapter_prefix(paths).display()
            );
            claude::install(paths)?;
            println!("Installed. `flottille doctor` says whether Claude agents can run.");
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Starts the agent, then follows its events until its first turn ends.
fn run_agent(paths: &Paths, asked: AgentRun, json: bool) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let started = client.request("agent.run", serde_json::to_value(asked)?)?;
    let agent = &started["agent"];
    let id = text(agent, "id").to_owned();
    if !json {
        println!("Agent {id}, {} in {}", text(agent, "project"), place(agent));
    }
    let mut events = Client::connect_or_start(paths)?;
    events.request(
        "events.watch",
        serde_json::to_value(WatchParams {
            after: started["after"].as_u64(),
        })?,
    )?;
    let mut stdout = std::io::stdout().lock();
    let code = loop {
        let event = events
            .next_event()?
            .ok_or("the daemon closed the connection")?;
        if event.data["agent"] != id.as_str() {
            continue;
        }
        match event.name.as_str() {
            "agent.message" if !json && event.data["replay"] != true => {
                write!(stdout, "{}", text(&event.data, "text"))?;
                stdout.flush()?;
            }
            "agent.turn_ended" => {
                let stop = text(&event.data, "stop_reason");
                if !json {
                    writeln!(stdout)?;
                    if stop != "end_turn" {
                        writeln!(stdout, "(the turn ended: {stop})")?;
                    }
                }
                break ExitCode::SUCCESS;
            }
            "agent.failed" => {
                eprintln!(
                    "flottille: agent {id} failed: {}",
                    text(&event.data, "reason")
                );
                break ExitCode::FAILURE;
            }
            // A death replaces the session, which goes on with the turn; the third fails it.
            "agent.died" => {
                eprintln!("flottille: the process of agent {id} died, its session is replaced");
            }
            _ => {}
        }
    };
    if json {
        let shown = client.request(
            "agent.show",
            serde_json::to_value(AgentName {
                agent: id,
                after: None,
            })?,
        )?;
        writeln!(stdout, "{}", shown["agent"])?;
    }
    Ok(code)
}

pub fn doctor(paths: &Paths, json: bool) -> Result<ExitCode> {
    let report = claude::doctor(paths);
    if json {
        println!("{report}");
    } else {
        print!("{}", doctor_text(&report));
    }
    Ok(if report["ready"] == true {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn doctor_text(report: &Value) -> String {
    let mark = |ok: bool| if ok { "ok  " } else { "MISSING" };
    let program = |name: &str, entry: &Value, missing: &str| {
        if entry["present"] == true {
            format!(
                "{} {name}: {} ({})\n",
                mark(true),
                entry["version"].as_str().unwrap_or("version unknown"),
                text(entry, "path")
            )
        } else {
            format!("{} {name}: {missing}\n", mark(false))
        }
    };
    let mut shown = program(
        "Node",
        &report["node"],
        "not on PATH; the Claude adapter runs on it",
    );
    shown.push_str(&program(
        "claude",
        &report["claude"],
        "not on PATH; install Claude Code and sign in with it",
    ));
    let login = &report["login"];
    shown.push_str(&if login["present"] == true {
        format!("{} Claude login: {}\n", mark(true), text(login, "path"))
    } else {
        format!(
            "{} Claude login: no {}; run `claude` and sign in (on macOS the login is in the \
             Keychain and there is no file)\n",
            mark(false),
            text(login, "path")
        )
    });
    let adapter = &report["adapter"];
    let pinned = text(adapter, "pinned");
    shown.push_str(&match adapter["version"].as_str() {
        Some(version) if version == pinned => {
            format!("{} Claude adapter: {version}\n", mark(true))
        }
        Some(version) => format!(
            "{} Claude adapter: {version} installed, {pinned} needed; run `flottille agent install`\n",
            mark(false)
        ),
        None => format!(
            "{} Claude adapter: not installed; run `flottille agent install`\n",
            mark(false)
        ),
    });
    shown.push_str(if report["ready"] == true {
        "Claude agents can run.\n"
    } else {
        "Claude agents cannot run until what is missing is there.\n"
    });
    shown
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn place(agent: &Value) -> String {
    match agent["workspace"].as_str() {
        Some(workspace) => format!("workspace {workspace}"),
        None => "main".into(),
    }
}

/// The agent, then its conversation: chunks joined, thoughts and tool calls in brief.
fn show_text(shown: &Value) -> String {
    let agent = &shown["agent"];
    let mut out = format!(
        "Agent {}: {}{}\n  project: {}, in {}\n  folder: {}\n  model: {}\n",
        text(agent, "id"),
        text(agent, "state"),
        agent["state_reason"]
            .as_str()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default(),
        text(agent, "project"),
        place(agent),
        text(agent, "folder"),
        agent["taken"]["model"]
            .as_str()
            .or(agent["chosen"]["model"].as_str())
            .unwrap_or("the agent's default"),
    );
    let mut speaking: Option<&str> = None;
    for event in shown["events"].as_array().into_iter().flatten() {
        let data = &event["data"];
        let name = event["name"].as_str().unwrap_or_default();
        let replay = if data["replay"] == true {
            " (replayed)"
        } else {
            ""
        };
        let said = match name {
            "agent.prompted" => Some(("you", text(data, "text"))),
            "agent.user_message" => Some(("you", text(data, "text"))),
            "agent.message" => Some(("agent", text(data, "text"))),
            _ => None,
        };
        if let Some((who, said)) = said {
            if speaking != Some(who) {
                out.push_str(&format!("\n{who}{replay}: "));
                speaking = Some(who);
            }
            out.push_str(said);
            continue;
        }
        let line = match name {
            "agent.thought" => format!("(thinking) {}", text(data, "text")),
            "agent.tool_call" if data["update"] != true => {
                format!("[tool] {} {}", text(data, "title"), text(data, "status"))
            }
            "agent.permission_answered" => format!(
                "[permission] {} {}",
                text(data, "tool"),
                if data["allowed"] == true {
                    "allowed"
                } else {
                    "rejected"
                }
            ),
            "agent.turn_ended" => format!("[turn ended: {}]", text(data, "stop_reason")),
            "agent.started" => format!(
                "[process {} started, session {}]",
                data["pid"],
                text(data, "opened")
            ),
            "agent.released" => format!("[process released: {}]", text(data, "reason")),
            "agent.compacted" => "[context compacted]".into(),
            "agent.died" => "[the process died]".into(),
            "agent.failed" => format!("[failed: {}]", text(data, "reason")),
            _ => continue,
        };
        speaking = None;
        out.push_str(&format!("\n{line}"));
    }
    out.push('\n');
    out
}
