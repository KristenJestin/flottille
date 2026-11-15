//! `flottille-hub` (`docs/remote.md`, "The hub"): the user's hub. It serves nothing yet: its
//! listener comes with the transport. A start makes its state (its key and, with no member, the
//! setup code it prints once) and reads it back.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use flottille_hub::{Hub, SystemClock};

#[derive(Parser)]
#[command(
    name = "flottille-hub",
    version,
    about = "Flottille's hub: the user's member list, invitations and presence"
)]
struct Cli {
    /// The hub's state folder [default: $XDG_STATE_HOME/flottille-hub]
    #[arg(long, value_name = "DIR", global = true)]
    state: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Prints the setup code again, while no member exists
    SetupCode {
        /// Empties the member list and makes a new setup code: every device must be set up again
        #[arg(long)]
        reset: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flottille-hub: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let folder = match cli.state {
        Some(folder) => folder,
        None => default_state()?,
    };
    let mut hub = Hub::open(&folder, SystemClock)?;
    match cli.command {
        None => {
            eprintln!(
                "flottille-hub: state in {}; the hub's key is {}",
                folder.display(),
                hub.key().fingerprint()
            );
            if let Some(line) = hub.take_announcement() {
                eprintln!("flottille-hub: {line}");
            }
            eprintln!(
                "flottille-hub: this version serves nothing yet: its listener comes with the transport"
            );
        }
        Some(Command::SetupCode { reset: false }) => println!("{}", hub.setup_code()?),
        Some(Command::SetupCode { reset: true }) => println!("{}", hub.reset()?),
    }
    Ok(())
}

/// `$XDG_STATE_HOME/flottille-hub`, or the specification's fallback in the home; a relative path
/// in either variable is ignored, as the specification says.
fn default_state() -> Result<PathBuf, String> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute("XDG_STATE_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".local/state")))
        .map(|state| state.join("flottille-hub"))
        .ok_or_else(|| {
            "neither XDG_STATE_HOME nor HOME is set to an absolute path: give the state folder with \
             --state"
                .to_owned()
        })
}
