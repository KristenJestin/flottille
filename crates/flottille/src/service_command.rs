//! `flottille service`: the services of the machine (`docs/cli.md`, "Services"), listed, shown,
//! written in `services.toml`, started and stopped through the daemon.

use clap::{Args, Subcommand};
use flottille_core::ansi;
use flottille_core::paths::Paths;
use serde_json::{Map, Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum ServiceAction {
    /// The services, each with its state, its folder and its address
    List {
        #[arg(long)]
        json: bool,
    },
    /// One service: its keys and its state
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Add a service to services.toml
    Add {
        name: String,
        #[command(flatten)]
        keys: Keys,
        #[arg(long)]
        json: bool,
    },
    /// Change a service: only what is given changes
    Edit {
        name: String,
        #[command(flatten)]
        keys: Keys,
        #[arg(long)]
        json: bool,
    },
    /// Remove a service from services.toml
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Start a service, and what it needs first
    Start {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop a service: what it needs and what needs it go on
    Stop {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// The end of what a service wrote
    Output {
        name: String,
        /// The last lines, 1 to 1000
        #[arg(long)]
        tail: Option<usize>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
pub struct Keys {
    /// One command line, never run by a shell
    #[arg(long)]
    line: Option<String>,
    /// Where it runs: absolute, or ~/…; the home when left out ("" puts it back there)
    #[arg(long)]
    folder: Option<String>,
    /// The services it needs, separated by commas ("" for none)
    #[arg(long)]
    needs: Option<String>,
    /// Ready once a line of its output holds this text
    #[arg(long, conflicts_with_all = ["ready_url", "ready_command"])]
    ready_line: Option<String>,
    /// Ready once this address answers ({url} is its address)
    #[arg(long, conflicts_with = "ready_command")]
    ready_url: Option<String>,
    /// Ready once this line exits with 0
    #[arg(long)]
    ready_command: Option<String>,
    /// Where it answers: an http:// or https:// address
    #[arg(long)]
    url: Option<String>,
    /// Held for approval each time an agent starts it
    #[arg(long)]
    ask_before_running: Option<bool>,
}

impl Keys {
    fn params(&self, name: &str) -> Value {
        let mut params = Map::new();
        params.insert("name".into(), name.into());
        if let Some(line) = &self.line {
            params.insert("line".into(), line.as_str().into());
        }
        if let Some(folder) = &self.folder {
            params.insert("folder".into(), folder.as_str().into());
        }
        if let Some(needs) = &self.needs {
            let needs: Vec<&str> = needs
                .split(',')
                .map(str::trim)
                .filter(|need| !need.is_empty())
                .collect();
            params.insert("needs".into(), needs.into());
        }
        for (key, value) in [
            ("line", &self.ready_line),
            ("url", &self.ready_url),
            ("command", &self.ready_command),
        ] {
            if let Some(value) = value {
                params.insert("ready".into(), json!({ key: value }));
            }
        }
        if let Some(url) = &self.url {
            params.insert("url".into(), url.as_str().into());
        }
        if let Some(ask) = self.ask_before_running {
            params.insert("ask_before_running".into(), ask.into());
        }
        Value::Object(params)
    }
}

pub fn run(paths: &Paths, action: ServiceAction) -> Result<()> {
    let mut client = Client::connect_or_start(paths)?;
    let (answer, json, text) = match action {
        ServiceAction::List { json } => {
            let listed = client.request("service.list", json!({}))?;
            let text = list_text(&listed);
            (listed, json, text)
        }
        ServiceAction::Show { name, json } => {
            let shown = client.request("service.show", json!({"name": name}))?;
            let text = show_text(&shown);
            (shown, json, text)
        }
        ServiceAction::Add { name, keys, json } => {
            let added = client.request("service.add", keys.params(&name))?;
            (added, json, format!("Service {name} added.\n"))
        }
        ServiceAction::Edit { name, keys, json } => {
            let edited = client.request("service.edit", keys.params(&name))?;
            (edited, json, format!("Service {name} changed.\n"))
        }
        ServiceAction::Remove { name, json } => {
            let removed = client.request("service.remove", json!({"name": name}))?;
            (removed, json, format!("Service {name} removed.\n"))
        }
        ServiceAction::Start { name, json } => {
            let started = client.request("service.start", json!({"name": name}))?;
            let names: Vec<&str> = started["started"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let mut text = match names.is_empty() {
                true => format!("Nothing started: {name} runs.\n"),
                false => format!("Started {}.\n", names.join(", ")),
            };
            text.push_str(&line_text(&started["service"]));
            (started, json, text)
        }
        ServiceAction::Stop { name, json } => {
            let stopped = client.request("service.stop", json!({"name": name}))?;
            let text = line_text(&stopped["service"]);
            (stopped, json, text)
        }
        ServiceAction::Output { name, tail, json } => {
            let output = client.request("service.output", json!({"name": name, "tail": tail}))?;
            let mut text = String::new();
            if output["earlier"] == true {
                text.push_str("(earlier lines left out)\n");
            }
            let written = ansi::strip(output["output"].as_str().unwrap_or_default());
            if !written.is_empty() {
                text.push_str(&written);
                text.push('\n');
            }
            (output, json, text)
        }
    };
    if json {
        println!("{answer}");
    } else {
        print!("{text}");
    }
    Ok(())
}

/// One line of a service: `mongo ready (/home/me/tools/mongo) at http://…`, and why it failed.
fn line_text(service: &Value) -> String {
    let text = |key: &str| service[key].as_str().unwrap_or_default();
    let mut line = format!("{} {} ({})", text("name"), text("state"), text("folder"));
    if let Some(address) = service["address"].as_str() {
        line.push_str(&format!(" at {address}"));
    }
    if let Some(why) = service["why"].as_str() {
        line.push_str(&format!(": {why}"));
    }
    let stopped: Vec<&str> = service["needs_stopped"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !stopped.is_empty() {
        line.push_str(&format!(
            "; what it needs is not running: {}",
            stopped.join(", ")
        ));
    }
    line.push('\n');
    line
}

fn list_text(listed: &Value) -> String {
    let services = listed["services"].as_array().map_or(&[][..], Vec::as_slice);
    if services.is_empty() {
        return "No service: add one with `flottille service add <name> --line <line>`.\n".into();
    }
    services.iter().map(line_text).collect()
}

fn show_text(service: &Value) -> String {
    let mut text = line_text(service);
    text.push_str(&format!(
        "line: {}\n",
        service["line"].as_str().unwrap_or_default()
    ));
    if let Some(needs) = service["needs"].as_array()
        && !needs.is_empty()
    {
        let needs: Vec<&str> = needs.iter().filter_map(Value::as_str).collect();
        text.push_str(&format!("needs: {}\n", needs.join(", ")));
    }
    if !service["ready"].is_null() {
        text.push_str(&format!("ready: {}\n", service["ready"]));
    }
    if service["ask_before_running"] == true {
        text.push_str("held for approval each time an agent starts it\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_is_one_line_with_its_state_its_folder_and_what_it_needs() {
        let service = json!({"name": "auth", "state": "ready", "folder": "/work/auth",
            "address": "http://localhost:3001", "why": null, "needs_stopped": ["mongo"]});
        assert_eq!(
            line_text(&service),
            "auth ready (/work/auth) at http://localhost:3001; what it needs is not running: \
             mongo\n"
        );
    }
}
