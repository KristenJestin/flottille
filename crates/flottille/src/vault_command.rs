//! `flottille vault`: the user's secrets, kept by the keyring, that agents use through
//! Flottille's tools and never see (`docs/vault.md`). A secret is read from the standard input,
//! or typed at a prompt that does not echo it: never an argument, which the shell's history and
//! the process list would keep.

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum VaultAction {
    /// The entries: their names and what they are for; never a secret
    List {
        /// Only the entries the agents of this project see
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Add an entry, or replace one whole: its secret is read from the standard input, or typed
    /// at a prompt
    Set {
        name: String,
        /// What it is for, as the agents read it
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        username: Option<String>,
        /// The only project whose agents see it; every project's when left out
        #[arg(long)]
        project: Option<String>,
        /// The one use that may read it: worklog:<project> or connection:<name>; any feature
        /// that names it when left out (an entry set again keeps its use)
        #[arg(long = "for", value_name = "USE")]
        bound: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Change what an entry says, keeping its secret in the keyring as it is: what a flag does
    /// not name stays, and an empty value clears it. To replace the secret, use `set`
    Edit {
        name: String,
        /// What it is for, as the agents read it
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        username: Option<String>,
        /// The only project whose agents see it; every project's when empty
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// agent-browser's credential provider, for the logins Flottille runs: one request on the
    /// standard input, one answer on the standard output (`agent-browser.plugin.v1`)
    #[command(hide = true)]
    Provide {
        /// The socket of the login that runs
        socket: std::path::PathBuf,
    },
    /// Share an entry with another machine of yours (--machine): its secret goes, signed by this
    /// device, from this machine's keyring into that machine's, never into a file
    Share {
        name: String,
        /// The machine it goes to: the command's --machine, which here names the machine the
        /// entry is shared with, not one this command drives.
        #[arg(skip)]
        machine: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove an entry and its secret from the keyring
    Remove {
        name: String,
        /// Remove it though the feature it is bound to names it, which then stops working
        #[arg(long)]
        confirmed: bool,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(paths: &Paths, action: VaultAction) -> Result<ExitCode> {
    if let VaultAction::Share {
        name,
        machine,
        json,
    } = action
    {
        return share(paths, &name, machine, json);
    }
    // Read before the daemon is reached: a prompt is the first thing the user sees.
    let secret = match &action {
        VaultAction::Set { name, .. } => Some(read_secret(name)?),
        _ => None,
    };
    let mut client = Client::connect_or_start(paths)?;
    let mut ask = |method: &str, params: Value| {
        client
            .request(method, params)
            .map_err(|error| Box::<dyn std::error::Error>::from(in_full(paths, &error.to_string())))
    };
    let (answer, json, text) = match action {
        VaultAction::List { project, json } => {
            let listed = ask("vault.list", json!({"project": project}))?;
            let text = list_text(&listed);
            (listed, json, text)
        }
        VaultAction::Set {
            name,
            description,
            url,
            username,
            project,
            bound,
            json,
        } => {
            let set = ask(
                "vault.set",
                json!({"name": name, "secret": secret, "description": description, "url": url,
                    "username": username, "project": project, "for": bound}),
            )?;
            let text = format!("Kept the secret of {name} in the keyring.");
            (set, json, text)
        }
        VaultAction::Edit {
            name,
            description,
            url,
            username,
            project,
            json,
        } => {
            // What a flag leaves out is what the entry says now: the call rewrites the metadata
            // whole, and carries no secret.
            let listed = ask("vault.list", json!({}))?;
            let Some(entry) = listed["entries"]
                .as_array()
                .and_then(|all| all.iter().find(|entry| entry["name"] == name.as_str()))
            else {
                return Err(format!("the vault has no entry {name}").into());
            };
            let kept = |key: &str, given: Option<String>| {
                given.or_else(|| entry[key].as_str().map(str::to_owned))
            };
            let edited = ask(
                "vault.set",
                json!({"name": name, "description": kept("description", description),
                    "url": kept("url", url), "username": kept("username", username),
                    "project": kept("project", project)}),
            )?;
            let text = format!("Changed {name}; its secret is kept as it was.");
            (edited, json, text)
        }
        VaultAction::Remove {
            name,
            confirmed,
            json,
        } => {
            let removed = ask(
                "vault.remove",
                json!({"name": name, "confirmed": confirmed}),
            )?;
            let text = format!("Removed {name} and its secret.");
            (removed, json, text)
        }
        VaultAction::Provide { .. } => unreachable!("answered before any folder is read"),
        VaultAction::Share { .. } => unreachable!("shared above"),
    };
    if json {
        println!("{answer}");
    } else {
        println!("{text}");
    }
    Ok(ExitCode::SUCCESS)
}

/// `flottille vault share <name> --machine <machine>`: a request of this machine's, signed with
/// this device's key, which the daemon never holds, and which goes with the secret for the other
/// machine to check (`docs/remote.md`, "Vault entries").
fn share(paths: &Paths, name: &str, machine: Option<String>, json: bool) -> Result<ExitCode> {
    let Some(machine) = machine else {
        return Err(
            "name the machine to share it with: flottille vault share <name> --machine \
                    <machine>"
                .into(),
        );
    };
    let said = |error: Box<dyn std::error::Error>| in_full(paths, &error.to_string());
    let this = Client::connect_or_start(paths)?
        .request("remote.machines", json!({}))
        .map_err(said)?["this"]["name"]
        .as_str()
        .map(str::to_owned)
        .ok_or("this machine is no member of the user's yet: flottille remote setup, or join")?;
    crate::client::aim(this);
    let shared = Client::connect_or_start(paths)?
        .request("vault.share", json!({"name": name, "machine": machine}))
        .map_err(said)?;
    if json {
        println!("{shared}");
    } else {
        println!("Shared {name} with {machine}: its keyring holds it now.");
    }
    Ok(ExitCode::SUCCESS)
}

/// The secret: what the standard input holds, less its last line end, or what is typed at a
/// prompt that does not echo it.
fn read_secret(name: &str) -> Result<String> {
    let mut stdin = std::io::stdin();
    let mut secret = String::new();
    if stdin.is_terminal() {
        eprint!("Secret of {name}: ");
        std::io::stderr().flush()?;
        let _quiet = Quiet::on();
        stdin.read_line(&mut secret)?;
        eprintln!();
    } else {
        stdin.read_to_string(&mut secret)?;
    }
    let secret = secret
        .strip_suffix('\n')
        .map(|rest| rest.strip_suffix('\r').unwrap_or(rest))
        .unwrap_or(&secret)
        .to_owned();
    if secret.is_empty() {
        return Err(
            "no secret was given: write it on the standard input, or type it at the prompt".into(),
        );
    }
    Ok(secret)
}

/// The terminal's echo off while it lives, given back when it ends and when a signal ends the
/// command at the prompt (^C, a hang-up, a kill).
struct Quiet(Option<libc::termios>);

/// The signals that end the command while the echo is off.
const ENDING: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

/// The terminal's flags before the prompt, for `give_back`: a signal handler reads them, so they
/// are written once before it is installed and never moved.
static mut WAS: std::mem::MaybeUninit<libc::termios> = std::mem::MaybeUninit::uninit();

/// Gives the terminal its flags back, then ends the command as the signal would have.
extern "C" fn give_back(signal: libc::c_int) {
    // SAFETY: tcsetattr, signal and raise are async-signal-safe; `WAS` was written before this
    // handler was installed and is not written again while it is.
    unsafe {
        libc::tcsetattr(0, libc::TCSANOW, (&raw const WAS).cast::<libc::termios>());
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

impl Quiet {
    fn on() -> Quiet {
        // SAFETY: tcgetattr and tcsetattr only read and set the terminal's flags of fd 0; `WAS`
        // is written before the handlers that read it are installed.
        unsafe {
            let mut was: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut was) != 0 {
                return Quiet(None);
            }
            (&raw mut WAS).cast::<libc::termios>().write(was);
            for signal in ENDING {
                libc::signal(signal, give_back as *const () as libc::sighandler_t);
            }
            let mut quiet = was;
            quiet.c_lflag &= !libc::ECHO;
            quiet.c_lflag |= libc::ECHONL;
            if libc::tcsetattr(0, libc::TCSANOW, &quiet) != 0 {
                for signal in ENDING {
                    libc::signal(signal, libc::SIG_DFL);
                }
                return Quiet(None);
            }
            Quiet(Some(was))
        }
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        if let Some(was) = &self.0 {
            // SAFETY: as in `on`, with the flags it read.
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, was);
                for signal in ENDING {
                    libc::signal(signal, libc::SIG_DFL);
                }
            }
        }
    }
}

fn list_text(listed: &Value) -> String {
    let entries = listed["entries"].as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        return "The vault holds no entry.".to_owned();
    }
    let mut text = String::from("The vault's entries (their secrets are never shown):");
    for entry in &entries {
        text.push_str(&format!(
            "\n  {}",
            entry["name"].as_str().unwrap_or_default()
        ));
        for (key, label) in [
            ("description", ""),
            ("url", "url "),
            ("username", "username "),
            ("project", "project "),
            ("for", "for "),
        ] {
            if let Some(value) = entry[key].as_str() {
                text.push_str(&format!("  {label}{value}"));
            }
        }
    }
    text
}
