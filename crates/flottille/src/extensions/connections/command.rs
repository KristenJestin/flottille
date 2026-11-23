//! `flottille connection`: the user's MCP servers, on the command line.

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use flottille_core::protocol::WatchParams;
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
    /// Share an https:// connection with another machine of yours (--machine), and the vault
    /// entries it names that you choose: signed by this device; it signs in for itself there
    Share {
        name: String,
        /// A vault entry the connection names, to share with it: again for each
        #[arg(long = "entry", value_name = "ENTRY")]
        entries: Vec<String>,
        /// The machine it goes to: the command's --machine, which here names the machine the
        /// connection is shared with, not one this command drives.
        #[arg(skip)]
        machine: Option<String>,
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
    /// Where a vault entry goes in the URL's requests: header:NAME=ENTRY, with a prefix before
    /// the entry's name ("header:Authorization=Bearer notes-token"), query:NAME=ENTRY,
    /// cookie:NAME=ENTRY or basic=ENTRY (the entry's username and secret); again for each
    #[arg(long = "secret", value_name = "IN:NAME=ENTRY")]
    secrets: Vec<String>,
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

/// `--secret`, as a line of `secrets`.
fn secret_line(given: &str) -> Result<Value> {
    let wrong = || format!("{given}: a secret is IN:NAME=ENTRY or basic=ENTRY");
    let (place, value) = given.split_once('=').ok_or_else(wrong)?;
    if place == "basic" {
        return Ok(json!({"in": "basic", "entry": value}));
    }
    let (place, name) = place.split_once(':').ok_or_else(wrong)?;
    let mut line = json!({"in": place, "name": name});
    match value.rsplit_once(' ') {
        Some((prefix, entry)) => {
            line["prefix"] = json!(format!("{prefix} "));
            line["entry"] = json!(entry);
        }
        None => line["entry"] = json!(value),
    }
    Ok(line)
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
    if !given.secrets.is_empty() {
        let lines = given
            .secrets
            .iter()
            .map(|line| secret_line(line))
            .collect::<Result<Vec<_>>>()?;
        params.insert("secrets".into(), json!(lines));
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
        if let Some(machine) = crate::client::aimed() {
            return login_on(paths, &name, no_browser, machine);
        }
        return login(paths, &name, no_browser);
    }
    if let ConnectionAction::Share {
        name,
        entries,
        machine,
        json,
    } = action
    {
        return share(paths, &name, &entries, machine, json);
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
        ConnectionAction::Share { .. } => unreachable!("shared above"),
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
    watcher.watch(WatchParams::default())?;
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

/// `flottille connection share <name> --machine <machine> [--entry <entry>]…`: a request of this
/// machine's, signed with this device's key, which the daemon never holds, and which goes with
/// the declaration and the entries for the other machine to check (`docs/remote.md`,
/// "Connections").
fn share(
    paths: &Paths,
    name: &str,
    entries: &[String],
    machine: Option<String>,
    json: bool,
) -> Result<ExitCode> {
    let Some(machine) = machine else {
        return Err(
            "name the machine to share it with: flottille connection share <name> --machine \
             <machine>"
                .into(),
        );
    };
    let this =
        Client::connect_or_start(paths)?.request("remote.machines", json!({}))?["this"]["name"]
            .as_str()
            .map(str::to_owned)
            .ok_or("this machine is no member of the user's yet: flottille remote pair")?;
    crate::client::aim(this);
    let shared = Client::connect_or_start(paths)?.request(
        "connection.share",
        json!({"name": name, "machine": machine, "entries": entries}),
    )?;
    if json {
        println!("{shared}");
    } else if entries.is_empty() {
        println!(
            "Shared the connection {name} with {machine}; it signs in there for itself \
             (`flottille --machine {machine} connection login {name}`)."
        );
    } else {
        println!(
            "Shared the connection {name} with {machine}, with {}; it signs in there for itself \
             (`flottille --machine {machine} connection login {name}`).",
            entries.join(", ")
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The sign-in of a connection of another machine, `machine`: its daemon runs it; a tunnel from
/// this machine's `127.0.0.1`, on the port its redirect names, carries the browser's redirect to
/// it, and its end is the sign-in's (`docs/remote.md`, "Connections").
fn login_on(paths: &Paths, name: &str, no_browser: bool, machine: &str) -> Result<ExitCode> {
    let answer =
        Client::connect_or_start(paths)?.request("connection.login", json!({"name": name}))?;
    let url = answer["url"].as_str().ok_or("the daemon gave no address")?;
    sign_in_opened(url)?;
    // Held open while the sign-in waits: the browser comes back through it.
    let mut tunnel = Client::connect_or_start(paths)?;
    tunnel.request("tunnel.open", json!({"sign_in": name}))?;
    if no_browser {
        println!("Open {url} in your browser to sign in to the connection {name} on {machine}.");
    } else {
        println!("Opening {url} in your browser to sign in to the connection {name} on {machine}.");
        flottille_window::open::system()(std::path::Path::new(url));
    }
    loop {
        match tunnel.next_event()? {
            Some(event) if event.name == "tunnel.closed" => {
                if event.data["signed_in"] == true {
                    println!("Signed in to the connection {name} on {machine}.");
                    return Ok(ExitCode::SUCCESS);
                }
                eprintln!(
                    "The sign-in of {name} on {machine} did not end well: {}",
                    event.data["reason"].as_str().unwrap_or("no reason given")
                );
                return Ok(ExitCode::FAILURE);
            }
            Some(_) => {}
            None => {
                eprintln!("The daemon closed the connection before the sign-in of {name} ended.");
                return Ok(ExitCode::FAILURE);
            }
        }
    }
}

/// Whether the address of a sign-in another machine's daemon sent may be printed and opened: the
/// window's own check (`sign_in_address`), that machine's word being no more trusted here.
fn sign_in_opened(url: &str) -> std::result::Result<(), String> {
    flottille_window::extensions::connections::sign_in_address(url)
        .map_err(|why| format!("the sign-in address the other machine sent is not opened: {why}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_flag_is_read_as_a_line_of_secrets() {
        for (given, line) in [
            (
                "header:Authorization=Bearer notes-token",
                json!({"in": "header", "name": "Authorization", "prefix": "Bearer ",
                    "entry": "notes-token"}),
            ),
            // The prefix is all before the last space.
            (
                "header:Authorization=Splunk Bearer notes-token",
                json!({"in": "header", "name": "Authorization", "prefix": "Splunk Bearer ",
                    "entry": "notes-token"}),
            ),
            (
                "header:X-Key=notes-key",
                json!({"in": "header", "name": "X-Key", "entry": "notes-key"}),
            ),
            (
                "query:userToken=design-usertoken",
                json!({"in": "query", "name": "userToken", "entry": "design-usertoken"}),
            ),
            (
                "cookie:session=notes-session",
                json!({"in": "cookie", "name": "session", "entry": "notes-session"}),
            ),
            (
                "basic=notes-login",
                json!({"in": "basic", "entry": "notes-login"}),
            ),
        ] {
            assert_eq!(secret_line(given).unwrap(), line, "{given}");
        }
        for wrong in ["header=notes-token", "notes-token", "query:userToken"] {
            assert!(secret_line(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn an_address_another_machine_sent_is_checked_before_it_is_shown_or_opened() {
        assert!(sign_in_opened("https://auth.example.com/authorize?state=x").is_ok());
        for wrong in [
            // Control characters would rewrite the terminal the address is printed to.
            "https://auth.example.com/\u{1b}]8;;https://elsewhere.example.com\u{7}",
            "https://auth.example.com/\nOpen this instead",
            // Only an https address is opened, or a plain http one on this machine.
            "file:///home/someone/.ssh/id_ed25519",
            "http://auth.example.com/authorize",
            "javascript:alert(1)",
            "",
        ] {
            assert!(sign_in_opened(wrong).is_err(), "{wrong:?}");
        }
    }
}
