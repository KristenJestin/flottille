//! `flottille connection`: the user's MCP servers, on the command line.

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Map, Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum ConnectionAction {
    /// Each connection, its state and its actions
    List {
        #[arg(long)]
        json: bool,
    },
    /// Declare a connection: a command spawned without a shell, or a URL; given to agents at once
    Add {
        name: String,
        /// One sentence on what it is for, which agents read
        #[arg(long)]
        about: String,
        #[command(flatten)]
        reach: Reach,
        #[command(flatten)]
        given: Given,
        #[arg(long)]
        json: bool,
    },
    /// Change a connection's declaration; pinned again at once unless its server announces a change
    Edit {
        name: String,
        #[arg(long)]
        about: Option<String>,
        #[command(flatten)]
        reach: Reach,
        #[command(flatten)]
        given: Given,
        /// Turn it on
        #[arg(long, conflicts_with = "off")]
        on: bool,
        /// Turn it off: its tool leaves every session
        #[arg(long)]
        off: bool,
        #[arg(long)]
        json: bool,
    },
    /// Remove a connection; its server is stopped
    Remove { name: String },
    /// Start its server and show what it announces against what you accepted
    Review {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Sign a URL connection in with OAuth, in your default browser; its tokens go to the vault
    Login {
        name: String,
        /// Print the address to open instead of opening the browser
        #[arg(long)]
        no_browser: bool,
    },
    /// Accept what the last review showed, or set actions: judged, passes, asked or off
    Accept {
        name: String,
        /// action=setting, again for each
        #[arg(long = "set", value_name = "ACTION=SETTING")]
        set: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
pub struct Reach {
    /// The program and its arguments, after --: spawned without a shell
    #[arg(long, num_args = 1.., allow_hyphen_values = true, conflicts_with = "url")]
    command: Option<Vec<String>>,
    /// An https:// URL, or http:// on this machine
    #[arg(long)]
    url: Option<String>,
}

#[derive(clap::Args)]
pub struct Given {
    /// VARIABLE=entry: a vault entry put in the server's environment; again for each
    #[arg(long = "env", value_name = "VARIABLE=ENTRY")]
    env: Vec<String>,
    /// Header=entry: a vault entry sent as a header to the URL; again for each
    #[arg(long = "header", value_name = "HEADER=ENTRY")]
    headers: Vec<String>,
    /// argument=v1,v2: the values an argument of that name may take; again for each
    #[arg(long = "only", value_name = "ARGUMENT=VALUES")]
    only: Vec<String>,
    /// A project whose agents do not get it; again for each
    #[arg(long = "except", value_name = "PROJECT")]
    except: Vec<String>,
    /// The OAuth client id you registered with the server's authorization server, for a URL
    #[arg(long = "client-id", value_name = "ID")]
    client_id: Option<String>,
}

fn pairs(given: &[String], what: &str) -> Result<Vec<(String, String)>> {
    given
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
                .ok_or_else(|| format!("{pair}: a {what} is NAME=VALUE").into())
        })
        .collect()
}

fn vault(given: &[String], what: &str) -> Result<Option<Value>> {
    if given.is_empty() {
        return Ok(None);
    }
    Ok(Some(Value::Object(
        pairs(given, what)?
            .into_iter()
            .map(|(name, entry)| (name, json!({"vault": entry})))
            .collect(),
    )))
}

fn declared(params: &mut Map<String, Value>, reach: Reach, given: Given) -> Result<()> {
    if let Some(command) = reach.command {
        params.insert("command".into(), json!(command));
    }
    if let Some(url) = reach.url {
        params.insert("url".into(), json!(url));
    }
    if let Some(env) = vault(&given.env, "variable")? {
        params.insert("env".into(), env);
    }
    if let Some(headers) = vault(&given.headers, "header")? {
        params.insert("headers".into(), headers);
    }
    if !given.only.is_empty() {
        let only: Map<String, Value> = pairs(&given.only, "limit")?
            .into_iter()
            .map(|(argument, values)| (argument, json!(values.split(',').collect::<Vec<_>>())))
            .collect();
        params.insert("only".into(), Value::Object(only));
    }
    if !given.except.is_empty() {
        params.insert("except".into(), json!(given.except));
    }
    if let Some(id) = given.client_id {
        params.insert("client_id".into(), json!(id));
    }
    Ok(())
}

pub fn run(paths: &Paths, action: ConnectionAction) -> Result<ExitCode> {
    if let ConnectionAction::Login { name, no_browser } = action {
        return login(paths, &name, no_browser);
    }
    let mut client = Client::connect_or_start(paths)?;
    let (answer, json) = match action {
        ConnectionAction::List { json } => {
            let answer = client.request("connection.list", json!({}))?;
            if !json {
                print!("{}", list_text(&answer));
                return Ok(ExitCode::SUCCESS);
            }
            (answer, true)
        }
        ConnectionAction::Add {
            name,
            about,
            reach,
            given,
            json,
        } => {
            let mut params = Map::new();
            params.insert("name".into(), json!(name));
            params.insert("about".into(), json!(about));
            declared(&mut params, reach, given)?;
            let answer = client.request("connection.add", Value::Object(params))?;
            if !json {
                match answer["state"].as_str() {
                    Some("ready") => println!(
                        "Declared the connection {name}: it is given to your agents, every \
                         action judged (`flottille connection review {name}` to change that)."
                    ),
                    Some("off") => println!(
                        "Declared the connection {name}: it is off, and given to your agents, \
                         every action judged, when it is turned on."
                    ),
                    _ => println!(
                        "Declared the connection {name}: review it to give it to your agents \
                         (`flottille connection review {name}`)."
                    ),
                }
            }
            (answer, json)
        }
        ConnectionAction::Edit {
            name,
            about,
            reach,
            given,
            on,
            off,
            json,
        } => {
            let mut params = Map::new();
            params.insert("name".into(), json!(name));
            if let Some(about) = about {
                params.insert("about".into(), json!(about));
            }
            if on || off {
                params.insert("enabled".into(), json!(on));
            }
            declared(&mut params, reach, given)?;
            let answer = client.request("connection.edit", Value::Object(params))?;
            if !json {
                println!(
                    "The connection {name} is {}.",
                    answer["state"].as_str().unwrap_or("?")
                );
            }
            (answer, json)
        }
        ConnectionAction::Remove { name } => {
            client.request("connection.remove", json!({"name": name}))?;
            println!("Removed the connection {name}.");
            return Ok(ExitCode::SUCCESS);
        }
        ConnectionAction::Review { name, json } => {
            let answer = client.request("connection.review", json!({"name": name}))?;
            if !json {
                print!("{}", review_text(&answer));
            }
            (answer, json)
        }
        ConnectionAction::Login { .. } => unreachable!("signed in above"),
        ConnectionAction::Accept { name, set, json } => {
            let actions: Map<String, Value> = pairs(&set, "setting")?
                .into_iter()
                .map(|(action, setting)| (action, json!(setting)))
                .collect();
            let answer = client.request(
                "connection.accept",
                json!({"name": name, "actions": actions}),
            )?;
            if !json {
                println!(
                    "The connection {name} is {}: its tool is {}.",
                    answer["state"].as_str().unwrap_or("?"),
                    answer["tool"].as_str().unwrap_or("?")
                );
            }
            (answer, json)
        }
    };
    if json {
        println!("{answer}");
    }
    Ok(ExitCode::SUCCESS)
}

/// How long the command waits for the sign-in to end: the daemon's own wait, and a little more.
const SIGN_IN_WAIT: std::time::Duration = std::time::Duration::from_secs(310);

/// Begins the sign-in, opens its address in the default browser (or prints it), and waits for the
/// daemon to say how it ended. The events are watched before it begins: none is missed.
fn login(paths: &Paths, name: &str, no_browser: bool) -> Result<ExitCode> {
    let mut watcher = Client::connect_or_start(paths)?;
    watcher.request("events.watch", json!({}))?;
    let mut client = Client::connect_or_start(paths)?;
    let answer = client.request("connection.login", json!({"name": name}))?;
    let url = answer["url"].as_str().ok_or("the daemon gave no address")?;
    if no_browser {
        println!("Open {url} in your browser to sign in to the connection {name}.");
    } else {
        println!("Opening {url} in your browser to sign in to the connection {name}.");
        flottille_window::open::system()(std::path::Path::new(url));
    }
    let deadline = std::time::Instant::now() + SIGN_IN_WAIT;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(event) = watcher.next_event_within(left)? else {
            eprintln!("The sign-in of {name} did not end in time: begin it again.");
            return Ok(ExitCode::FAILURE);
        };
        if event.data["name"] != name {
            continue;
        }
        match event.name.as_str() {
            "connection.signed_in" => {
                println!("Signed in to the connection {name}.");
                return Ok(ExitCode::SUCCESS);
            }
            "connection.sign_in_failed" => {
                eprintln!(
                    "The sign-in of {name} failed: {}",
                    event.data["why"].as_str().unwrap_or("no reason given")
                );
                return Ok(ExitCode::FAILURE);
            }
            _ => {}
        }
    }
}

fn list_text(answer: &Value) -> String {
    let mut text = format!(
        "Connections are {}.\n",
        if answer["enabled"] == true {
            "on"
        } else {
            "off"
        }
    );
    if let Some(error) = answer["error"].as_str() {
        text.push_str(&format!("  cannot be used: {error}\n"));
    }
    for connection in answer["connections"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "{} ({}): {}. {}\n",
            connection["name"].as_str().unwrap_or_default(),
            connection["tool"].as_str().unwrap_or_default(),
            connection["state"].as_str().unwrap_or_default(),
            connection["about"].as_str().unwrap_or_default(),
        ));
        if let Some(why) = connection["cut_off"].as_str() {
            text.push_str(&format!("  cut off: {why}\n"));
        }
        if let Some(why) = connection["failing"].as_str() {
            text.push_str(&format!("  failing: {why}\n"));
        }
        if let Some(why) = connection["signed_out"].as_str() {
            text.push_str(&format!(
                "  signed out: {why}; sign in again with `flottille connection login {}`\n",
                connection["name"].as_str().unwrap_or_default()
            ));
        }
        for action in connection["actions"].as_array().into_iter().flatten() {
            text.push_str(&format!(
                "  - {}: {}\n",
                action["name"].as_str().unwrap_or_default(),
                action["setting"].as_str().unwrap_or_default()
            ));
        }
    }
    text
}

/// The review as plain text, never Markdown: what changed, then each action.
fn review_text(answer: &Value) -> String {
    let mut text = format!(
        "The server of {}: {} {}\n",
        answer["name"].as_str().unwrap_or_default(),
        answer["server"]["now"]["name"].as_str().unwrap_or("?"),
        answer["server"]["now"]["version"].as_str().unwrap_or("?"),
    );
    for change in answer["changes"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "Changed: {}\n",
            change.as_str().unwrap_or_default()
        ));
    }
    let instructions = &answer["instructions"];
    if instructions["pinned"] != instructions["now"] {
        if let Some(old) = instructions["pinned"].as_str() {
            text.push_str("Instructions accepted before:\n");
            for line in old.lines() {
                text.push_str(&format!("  - {line}\n"));
            }
        }
        match instructions["now"].as_str() {
            Some(now) => {
                text.push_str("Instructions now:\n");
                for line in now.lines() {
                    text.push_str(&format!("  + {line}\n"));
                }
            }
            None => text.push_str("No instructions now.\n"),
        }
    }
    for action in answer["actions"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "{} ({}, {}):\n",
            action["name"].as_str().unwrap_or_default(),
            action["change"].as_str().unwrap_or_default(),
            action["setting"].as_str().unwrap_or("gone"),
        ));
        if action["change"] != "unchanged" {
            if let Some(old) = action["pinned"]["description"].as_str() {
                for line in old.lines() {
                    text.push_str(&format!("  - {line}\n"));
                }
            }
            if let Some(now) = action["now"]["description"].as_str() {
                for line in now.lines() {
                    text.push_str(&format!("  + {line}\n"));
                }
            }
        }
    }
    for limit in answer["limits"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "Limit on {}: covers {}; does not cover {}\n",
            limit["argument"].as_str().unwrap_or_default(),
            limit["covers"],
            limit["not"]
        ));
    }
    text.push_str(&format!(
        "Accept it with `flottille connection accept {}`.\n",
        answer["name"].as_str().unwrap_or_default()
    ));
    text
}
