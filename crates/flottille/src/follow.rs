//! Following an agent's turn from the terminal: what `agent run`, `agent send --follow` and
//! `agent chat` print while the turn they started goes on (`docs/cli.md`).

use std::collections::HashSet;
use std::io::Write;
use std::process::ExitCode;

use flottille_core::paths::Paths;
use flottille_core::protocol::{AgentName, WatchParams};
use serde_json::Value;

use crate::agent_command::text;
use crate::client::Client;
use crate::program;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Which turn is followed.
pub enum Turn {
    /// The agent's first: everything after the event number belongs to it (`agent run`).
    First,
    /// The one that plays the message just sent, named by the id of its delivery (the answer to
    /// `agent.send`). The agent may be in a turn already, which this one is not: the message is
    /// queued behind it, and the turn ends that follow are not for it. Another client's message
    /// queued around it is not it either; a prompt that takes both is one turn, and answers both.
    Sent(String),
}

/// Follows the agent's events after `after` until the turn ends, printing its reply as it
/// streams, and says in one line on the standard error each request of its tree that waits for
/// the user. `client` is a connection of its own for requests: the one that follows only
/// receives events. A failed or ended agent is a failure.
pub fn follow(
    paths: &Paths,
    client: &mut Client,
    agent: &str,
    after: Option<u64>,
    turn: Turn,
    json: bool,
) -> Result<ExitCode> {
    let mut tree = ids_of_tree(client, agent)?;
    let mut events = Client::connect_or_start(paths)?;
    events.request("events.watch", serde_json::to_value(WatchParams { after })?)?;
    let mut shown_requests = HashSet::new();
    let (awaited, mut playing) = match turn {
        Turn::First => (None, true),
        Turn::Sent(delivery) => (Some(delivery), false),
    };
    let mut out = std::io::stdout();
    loop {
        let event = events
            .next_event()?
            .ok_or("the daemon closed the connection")?;
        let data = &event.data;
        match event.name.as_str() {
            // The tree grows while it is followed.
            "agent.created" if tree.contains(text(data, "parent")) => {
                tree.insert(text(data, "agent").to_owned());
                continue;
            }
            "approval.requested" | "approval.forwarded"
                if data["held_by"] == "user"
                    && tree.contains(text(data, "agent"))
                    && shown_requests.insert(data["number"].as_u64()) =>
            {
                let (name, number) = (program::name(), &data["number"]);
                eprintln!(
                    "A request waits for you: #{number} of agent {}. Answer with `{name} \
                     approve {number}` or `{name} deny {number}`.",
                    text(data, "agent")
                );
                continue;
            }
            _ => {}
        }
        if data["agent"] != agent {
            continue;
        }
        match event.name.as_str() {
            "delivery.sent" if mentions(data, awaited.as_deref()) => playing = true,
            // The turn failed: the message goes back to the queue and is played again.
            "delivery.given_back" if mentions(data, awaited.as_deref()) => playing = false,
            "delivery.superseded" if mentions(data, awaited.as_deref()) => {
                eprintln!(
                    "{}: agent {agent} ended before it read your message",
                    program::name()
                );
                return Ok(ExitCode::FAILURE);
            }
            "agent.message" if playing && !json && data["replay"] != true => {
                write!(out, "{}", text(data, "text"))?;
                out.flush()?;
            }
            "agent.turn_ended" if playing => {
                let stop = text(data, "stop_reason");
                if !json {
                    writeln!(out)?;
                    if stop != "end_turn" {
                        writeln!(out, "(the turn ended: {stop})")?;
                    }
                }
                return Ok(ExitCode::SUCCESS);
            }
            "agent.failed" => {
                eprintln!(
                    "{}: agent {agent} failed: {}",
                    program::name(),
                    text(data, "reason")
                );
                return Ok(ExitCode::FAILURE);
            }
            "agent.ended" => {
                eprintln!(
                    "{}: agent {agent} ended: {}",
                    program::name(),
                    text(data, "reason")
                );
                return Ok(ExitCode::FAILURE);
            }
            // A death replaces the session, which goes on with the turn; the third fails it.
            "agent.died" => {
                eprintln!(
                    "{}: the process of agent {agent} died, its session is replaced",
                    program::name()
                );
            }
            _ => {}
        }
    }
}

/// Whether the event's `deliveries` hold the delivery awaited.
fn mentions(data: &Value, awaited: Option<&str>) -> bool {
    awaited.is_some_and(|id| {
        data["deliveries"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|d| d == id))
    })
}

/// The ids of the agent's tree, its root's.
fn ids_of_tree(client: &mut Client, agent: &str) -> Result<HashSet<String>> {
    let tree = client.request(
        "agent.tree",
        serde_json::to_value(AgentName {
            agent: agent.to_owned(),
            after: None,
        })?,
    )?;
    Ok(tree["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| text(&entry["agent"], "id").to_owned())
        .collect())
}
