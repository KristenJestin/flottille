//! `flottille --machine <m> tunnel open <run>`, `tunnel list` and `tunnel close` (`docs/cli.md`):
//! a run's port on another machine reached on a local port (`docs/remote.md`, "Tunnels").

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::{self, Client};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum TunnelAction {
    /// Reach a run's port on another machine (`--machine`) on a local port: it stays open until
    /// the run ends or this command is stopped
    Open {
        run: String,
        /// The run's port, when it listens on several and reports no address
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        json: bool,
    },
    /// The tunnels this machine carries (`--machine` lists the ones to that machine only)
    List {
        #[arg(long)]
        json: bool,
    },
    /// Close one of them
    Close { tunnel: String },
}

/// Whether the action is about this machine's own tunnels: `--machine` then only filters.
pub fn is_local(action: &TunnelAction) -> bool {
    !matches!(action, TunnelAction::Open { .. })
}

pub fn run(paths: &Paths, action: TunnelAction, machine: Option<String>) -> Result<()> {
    match action {
        TunnelAction::Open { run, port, json } => open(paths, run, port, json),
        TunnelAction::List { json } => {
            let mut client = Client::connect_or_start(paths)?;
            let mut listed = client.request("tunnel.list", json!({}))?;
            if let Some(machine) = &machine {
                listed["tunnels"] = listed["tunnels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|tunnel| tunnel["machine"] == machine.as_str())
                    .cloned()
                    .collect();
            }
            if json {
                println!("{listed}");
            } else {
                print!("{}", list_text(&listed));
            }
            Ok(())
        }
        TunnelAction::Close { tunnel } => {
            Client::connect_or_start(paths)?.request("tunnel.close", json!({"tunnel": tunnel}))?;
            println!("Tunnel {tunnel} is closed.");
            Ok(())
        }
    }
}

fn open(paths: &Paths, run: String, port: Option<u16>, json: bool) -> Result<()> {
    if client::aimed().is_none() {
        return Err("a tunnel is to another machine's run: name it with --machine".into());
    }
    let mut client = Client::connect_or_start(paths)?;
    let opened = client.request("tunnel.open", json!({"run": run, "port": port}))?;
    if json {
        println!("{opened}");
    } else {
        println!(
            "Run {run} of {}, port {}, is on {}. It stays open until the run ends or you press Ctrl+C.",
            opened["machine"].as_str().unwrap_or("?"),
            opened["port"],
            opened["address"].as_str().unwrap_or("?"),
        );
    }
    // The tunnel lives as long as this connection: its end is the daemon's last word.
    let reason = loop {
        match client.next_event()? {
            Some(event) if event.name == "tunnel.closed" => {
                break event.data["reason"]
                    .as_str()
                    .unwrap_or("it ended")
                    .to_owned();
            }
            Some(_) => {}
            None => break "the daemon closed the connection".to_owned(),
        }
    };
    if json {
        println!("{}", json!({"closed": reason}));
    } else {
        println!("The tunnel is closed: {reason}.");
    }
    Ok(())
}

fn list_text(listed: &Value) -> String {
    let tunnels = listed["tunnels"].as_array().map_or(&[][..], Vec::as_slice);
    if tunnels.is_empty() {
        return "No tunnel is open.\n".into();
    }
    let mut text = String::new();
    for tunnel in tunnels {
        let connections = tunnel["connections"].as_u64().unwrap_or(0);
        let reached = match tunnel["sign_in"].as_str() {
            Some(name) => format!("the sign-in of {name}"),
            None => format!(
                "port {} of run {}",
                tunnel["port"],
                tunnel["run"].as_str().unwrap_or("?")
            ),
        };
        text.push_str(&format!(
            "{} {} -> {reached} on {} ({connections} connection{})\n",
            tunnel["tunnel"].as_str().unwrap_or("?"),
            tunnel["address"].as_str().unwrap_or("?"),
            tunnel["machine"].as_str().unwrap_or("?"),
            if connections == 1 { "" } else { "s" },
        ));
    }
    text.push_str("Close one with `flottille tunnel close <tunnel>`.\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tunnel_is_listed_with_its_local_address_its_run_and_its_machine() {
        let listed = json!({"tunnels": [
            {"tunnel": "t1", "address": "127.0.0.1:4501", "port": 3000, "run": "r7",
                "machine": "server", "connections": 1},
            {"tunnel": "t2", "address": "127.0.0.1:4502", "port": 8080, "run": "r8",
                "machine": "server", "connections": 0}]});
        assert_eq!(
            list_text(&listed),
            "t1 127.0.0.1:4501 -> port 3000 of run r7 on server (1 connection)\n\
             t2 127.0.0.1:4502 -> port 8080 of run r8 on server (0 connections)\n\
             Close one with `flottille tunnel close <tunnel>`.\n"
        );
        assert_eq!(list_text(&json!({"tunnels": []})), "No tunnel is open.\n");
    }
}
