//! `flottille agent` and `flottille doctor`: starting a Claude agent, writing to it, reading it
//! and stopping it, and what the machine needs for one (`docs/agents.md`, `docs/cli.md`).

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::protocol::{AgentName, AgentRun, AgentSend};
use serde_json::Value;

use crate::chat;
use crate::client::Client;
use crate::follow::{Turn, follow};
use crate::providers;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum AgentAction {
    /// Start a root Claude agent in a project with a first message, and return at once
    Start {
        project: String,
        message: String,
        /// The workspace it works in (default: the project folder itself)
        #[arg(long)]
        workspace: Option<String>,
        /// The provider it runs on, for life (default: the project's)
        #[arg(long)]
        provider: Option<String>,
        /// The model (default: the project's, else the provider's); never replaced by another
        #[arg(long)]
        model: Option<String>,
        /// The model's effort (default: its own)
        #[arg(long)]
        effort: Option<String>,
        /// Its role: lead (default) or worker
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Write to a root agent
    Send {
        agent: String,
        message: String,
        /// Print the reply as it streams, until the turn that plays the message ends
        #[arg(long, short)]
        follow: bool,
        /// Print the answer (with --follow, once the turn ended)
        #[arg(long)]
        json: bool,
    },
    /// Talk to a root agent from the terminal: each line you type is sent, and the reply streams
    Chat { agent: String },
    /// A page of an agent's conversation: any agent of a tree
    Read {
        agent: String,
        /// Read what follows this event number
        #[arg(long, value_name = "N")]
        after: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// Stop a root agent and its tree, the deepest first
    Stop {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Start an ended root again on its own session
    Resume {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Set a session's mode: in ask, every call to judge comes to you; in auto, the judge you
    /// chose in Settings decides it (`docs/design.md` §7)
    Mode {
        agent: String,
        /// ask or auto
        mode: String,
        #[arg(long)]
        json: bool,
    },
    /// The agents, of every project or of one
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// The tree of an agent's root, each agent before its children, and what each does
    Tree {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Start a root Claude agent as `start` does, and print its reply to the first message
    Run {
        project: String,
        message: String,
        /// The workspace it works in (default: the project folder itself)
        #[arg(long)]
        workspace: Option<String>,
        /// The provider it runs on, for life (default: the project's)
        #[arg(long)]
        provider: Option<String>,
        /// The model (default: the project's, else the provider's); never replaced by another
        #[arg(long)]
        model: Option<String>,
        /// The model's effort (default: its own)
        #[arg(long)]
        effort: Option<String>,
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
}

pub fn run(paths: &Paths, action: AgentAction) -> Result<ExitCode> {
    match action {
        AgentAction::Start {
            project,
            message,
            workspace,
            provider,
            model,
            effort,
            role,
            json,
        } => {
            let started = Client::connect_or_start(paths)?.request(
                "agent.start",
                serde_json::to_value(AgentRun {
                    project,
                    workspace,
                    provider,
                    model,
                    effort,
                    mode: None,
                    role,
                    message,
                })?,
            )?;
            if json {
                println!("{started}");
            } else {
                let agent = &started["agent"];
                let id = text(agent, "id");
                println!(
                    "Started agent {id} ({}) in {}, {}.\nFollow it with `flottille agent read \
                     {id}`, or `flottille watch --after {}`.",
                    text(agent, "role"),
                    text(agent, "project"),
                    place(agent),
                    started["after"]
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        AgentAction::Send {
            agent,
            message,
            follow: following,
            json,
        } => {
            let mut client = Client::connect_or_start(paths)?;
            let mut sent = client.request(
                "agent.send",
                serde_json::to_value(AgentSend {
                    agent: agent.clone(),
                    message,
                    images: Vec::new(),
                })?,
            )?;
            let mut code = ExitCode::SUCCESS;
            if following {
                code = follow(
                    paths,
                    &mut client,
                    &agent,
                    sent["after"].as_u64(),
                    Turn::Sent(text(&sent, "delivery").to_owned()),
                    json,
                )?;
                // The agent as the turn left it.
                sent["agent"] = shown_agent(&mut client, &agent)?;
            }
            if json {
                println!("{sent}");
            } else if !following {
                println!(
                    "Sent to agent {agent}: it plays after the turn that runs, if one does.\n\
                     Read what follows with `flottille agent read {agent} --after {}`.",
                    sent["after"]
                );
            }
            Ok(code)
        }
        AgentAction::Chat { agent } => chat::run(paths, &agent),
        AgentAction::Read { agent, after, json } => {
            let page = Client::connect_or_start(paths)?.request(
                "agent.read",
                serde_json::to_value(AgentName {
                    agent: agent.clone(),
                    after,
                })?,
            )?;
            if json {
                println!("{page}");
            } else {
                print!("{}", read_text(&page));
            }
            Ok(ExitCode::SUCCESS)
        }
        AgentAction::Stop { agent, json } => {
            let stopped = Client::connect_or_start(paths)?.request(
                "agent.stop",
                serde_json::to_value(AgentName { agent, after: None })?,
            )?;
            if json {
                println!("{stopped}");
            } else {
                let agent = &stopped["agent"];
                println!(
                    "Stopped agent {} and its tree; it is {} now. Its workspace stays.",
                    text(agent, "id"),
                    text(agent, "state")
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        AgentAction::Resume { agent, json } => {
            let resumed = Client::connect_or_start(paths)?.request(
                "agent.resume",
                serde_json::to_value(AgentName { agent, after: None })?,
            )?;
            if json {
                println!("{resumed}");
            } else {
                println!(
                    "Resumed agent {} on its own session; it is {} now. Its next message starts it.",
                    text(&resumed["agent"], "id"),
                    text(&resumed["agent"], "state")
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        AgentAction::Mode { agent, mode, json } => {
            let set = Client::connect_or_start(paths)?.request(
                "agent.mode",
                serde_json::json!({"agent": agent, "mode": mode}),
            )?;
            if json {
                println!("{set}");
            } else {
                println!(
                    "Agent {} is in {} mode now.",
                    text(&set["agent"], "id"),
                    text(&set["agent"], "gate_mode")
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        AgentAction::List { project, json } => {
            let listed = Client::connect_or_start(paths)?
                .request("agent.list", serde_json::json!({"project": project}))?;
            if json {
                println!("{listed}");
            } else {
                print!("{}", list_text(&listed));
            }
            Ok(ExitCode::SUCCESS)
        }
        AgentAction::Tree { agent, json } => {
            let tree = Client::connect_or_start(paths)?.request(
                "agent.tree",
                serde_json::to_value(AgentName { agent, after: None })?,
            )?;
            if json {
                println!("{tree}");
            } else {
                print!("{}", tree_text(&tree));
            }
            Ok(ExitCode::SUCCESS)
        }
        AgentAction::Run {
            project,
            message,
            workspace,
            provider,
            model,
            effort,
            json,
        } => run_agent(
            paths,
            AgentRun {
                project,
                workspace,
                provider,
                model,
                effort,
                mode: None,
                role: None,
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
    }
}

/// Starts the agent, then follows its events until its first turn ends.
fn run_agent(paths: &Paths, asked: AgentRun, json: bool) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let started = client.request("agent.start", serde_json::to_value(asked)?)?;
    let agent = &started["agent"];
    let id = text(agent, "id").to_owned();
    if !json {
        println!("Agent {id}, {} in {}", text(agent, "project"), place(agent));
    }
    let code = follow(
        paths,
        &mut client,
        &id,
        started["after"].as_u64(),
        Turn::First,
        json,
    )?;
    if json {
        println!("{}", shown_agent(&mut client, &id)?);
    }
    Ok(code)
}

/// The agent as `agent.show` gives it.
fn shown_agent(client: &mut Client, agent: &str) -> Result<Value> {
    let shown = client.request(
        "agent.show",
        serde_json::to_value(AgentName {
            agent: agent.to_owned(),
            after: None,
        })?,
    )?;
    Ok(shown["agent"].clone())
}

pub fn doctor(paths: &Paths, json: bool) -> Result<ExitCode> {
    let report = providers::doctor(paths);
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

/// Each provider, what it was checked for, and whether its agents can run.
fn doctor_text(report: &Value) -> String {
    let mark = |ok: bool| if ok { "ok  " } else { "MISSING" };
    let mut shown = String::new();
    for provider in report["providers"].as_array().into_iter().flatten() {
        let title = text(provider, "title");
        if provider["enabled"] != true {
            shown.push_str(&format!(
                "{title} agents cannot run: {} {}\n",
                text(provider, "name"),
                text(provider, "why")
            ));
            continue;
        }
        if let Some(command) = provider["override"].as_str() {
            shown.push_str(&format!(
                "warn {title}: its agents run {command} (FLOTTILLE_AGENT_COMMAND, for tests)\n"
            ));
        } else {
            let program = &provider["program"];
            shown.push_str(&if program["present"] == true {
                format!(
                    "{} {}: {} ({})\n",
                    mark(true),
                    text(program, "name"),
                    program["version"].as_str().unwrap_or("version unknown"),
                    text(program, "path")
                )
            } else {
                format!("{} {}: not on PATH\n", mark(false), text(program, "name"))
            });
            let login = &provider["login"];
            shown.push_str(&if login["present"] == true {
                format!("{} {title} login: {}\n", mark(true), text(login, "path"))
            } else if let Some(key) = login["api_key"].as_str() {
                format!("{} {title} login: {key} is set\n", mark(true))
            } else {
                format!(
                    "warn {title} login: no {}, and no API key\n",
                    text(login, "path")
                )
            });
        }
        shown.push_str(&match provider["why"].as_str() {
            None => format!("{title} agents can run.\n"),
            Some(why) => format!(
                "{title} agents cannot run: {} {why}\n",
                text(provider, "name")
            ),
        });
    }
    for warning in report["warnings"].as_array().into_iter().flatten() {
        shown.push_str(&format!("warn {}\n", warning.as_str().unwrap_or_default()));
    }
    shown
}

pub(crate) fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

pub(crate) fn place(agent: &Value) -> String {
    match agent["workspace"].as_str() {
        Some(workspace) => format!("workspace {workspace}"),
        None => "main".into(),
    }
}

/// A page of a conversation, and how to read on.
fn read_text(page: &Value) -> String {
    let mut out = format!("The conversation of {}:\n", text(page, "name"));
    let lines: Vec<&str> = page["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if lines.is_empty() {
        out.push_str("(nothing after this point)\n");
    }
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    let (id, after) = (text(page, "agent"), &page["after"]);
    if page["more"] == true {
        out.push_str(&format!(
            "More follows: flottille agent read {id} --after {after}\n"
        ));
    } else {
        out.push_str(&format!(
            "That is all for now; later, read on with `flottille agent read {id} --after {after}`.\n"
        ));
    }
    out
}

/// The agents in short, one line each.
fn list_text(listed: &Value) -> String {
    let agents = listed["agents"].as_array().map_or(&[][..], Vec::as_slice);
    if agents.is_empty() {
        return "No agent yet: `flottille agent start` starts one.\n".into();
    }
    agents
        .iter()
        .map(|agent| format!("{}\n", agent_line(agent, None)))
        .collect()
}

/// An agent of a tree, each indented under its parent, with what it does.
fn tree_text(tree: &Value) -> String {
    tree["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| format!("{}\n", agent_line(&entry["agent"], Some(entry))))
        .collect()
}

fn agent_line(agent: &Value, entry: Option<&Value>) -> String {
    let named = match agent["title"].as_str() {
        Some(title) => format!("\"{title}\" (agent {})", text(agent, "id")),
        None => format!("agent {}", text(agent, "id")),
    };
    let mut line = format!(
        "{}{named}, {} in {}, {}, on {}",
        "  ".repeat(agent["depth"].as_u64().unwrap_or(0) as usize),
        text(agent, "role"),
        text(agent, "project"),
        place(agent),
        text(agent, "provider"),
    );
    if let Some(model) = agent["model"].as_str() {
        line.push_str(&format!(" ({model})"));
    }
    match entry {
        Some(entry) => {
            line.push_str(&format!(": {}", text(entry, "doing")));
            match entry["waiting_on"].as_str() {
                Some("user") => line.push_str("; waits for the user"),
                Some(on) => line.push_str(&format!("; waits for its {on}")),
                None => {}
            }
            if entry["needs_user"] == true {
                line.push_str("; a request waits for you (`flottille approvals`)");
            }
        }
        None => line.push_str(&format!(": {}", text(agent, "state"))),
    }
    line
}

/// The agent, then its conversation: chunks joined, thoughts and tool calls in brief.
fn show_text(shown: &Value) -> String {
    let agent = &shown["agent"];
    let mut out = format!(
        "Agent {}: {}{}\n  project: {}, in {}\n  folder: {}\n  provider: {}\n  model: {}\n",
        text(agent, "id"),
        text(agent, "state"),
        agent["state_reason"]
            .as_str()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default(),
        text(agent, "project"),
        place(agent),
        text(agent, "folder"),
        text(agent, "provider"),
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
            "agent.ended" => format!("[ended: {}]", text(data, "reason")),
            "agent.resumed" => "[resumed by the user]".into(),
            _ => continue,
        };
        speaking = None;
        out.push_str(&format!("\n{line}"));
    }
    out.push('\n');
    out
}
