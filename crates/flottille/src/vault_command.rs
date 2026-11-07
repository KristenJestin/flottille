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
        #[arg(long)]
        json: bool,
    },
    /// Remove an entry and its secret from the keyring
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(paths: &Paths, action: VaultAction) -> Result<ExitCode> {
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
            json,
        } => {
            let set = ask(
                "vault.set",
                json!({"name": name, "secret": secret, "description": description, "url": url,
                    "username": username, "project": project}),
            )?;
            let text = format!("Kept the secret of {name} in the keyring.");
            (set, json, text)
        }
        VaultAction::Remove { name, json } => {
            let removed = ask("vault.remove", json!({"name": name}))?;
            let text = format!("Removed {name} and its secret.");
            (removed, json, text)
        }
    };
    if json {
        println!("{answer}");
    } else {
        println!("{text}");
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
        ] {
            if let Some(value) = entry[key].as_str() {
                text.push_str(&format!("  {label}{value}"));
            }
        }
    }
    text
}
