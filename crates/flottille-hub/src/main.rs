//! `flottille-hub` (`docs/remote.md`, "The hub"): the user's hub. A start makes its state (its key
//! and, with no member, the setup code it prints once), reads it back, and serves the devices'
//! control sessions on its listen address until it is stopped.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use flottille_hub::{Hub, LISTEN, SystemClock};
use flottille_link::Hello;

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
    /// Where the hub listens: an IP address and a port. Another address than loopback takes TLS,
    /// or --plain
    #[arg(long, value_name = "ADDRESS", default_value = LISTEN)]
    listen: SocketAddr,
    /// TLS: the certificate, in PEM (its chain after it); without it, the hub is plain, for a
    /// reverse proxy that holds the certificate
    #[arg(long, value_name = "FILE", requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    /// Plain on another address than loopback: for a reverse proxy on another host that holds
    /// the certificate. The sessions cross the network between them in clear
    #[arg(long, conflicts_with = "tls_cert")]
    plain: bool,
    /// TLS: the certificate's private key, in PEM
    #[arg(long, value_name = "FILE", requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// TLS by ACME for this domain: not built yet
    #[arg(long, value_name = "DOMAIN")]
    acme: Option<String>,
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
    match cli.command {
        Some(Command::SetupCode { reset: true }) => {
            println!("{}", flottille_hub::reset(&folder)?);
            return Ok(());
        }
        Some(Command::SetupCode { reset: false }) => {
            println!("{}", flottille_hub::setup_code(&folder)?);
            return Ok(());
        }
        None => {}
    }
    if let Some(domain) = cli.acme {
        return Err(format!(
            "--acme {domain}: ACME is not built yet; give a certificate with --tls-cert and \
             --tls-key, or run the hub plain behind a reverse proxy that holds one"
        )
        .into());
    }
    let tls = match (&cli.tls_cert, &cli.tls_key) {
        (Some(certificate), Some(key)) => Some(flottille_hub::tls_server(certificate, key)?),
        _ => None,
    };
    if tls.is_none() && !cli.plain && !cli.listen.ip().is_loopback() {
        return Err(format!(
            "--listen {}: another address than loopback takes TLS (--tls-cert and --tls-key), or \
             --plain when a reverse proxy on another host holds the certificate; without either, \
             every session would cross the network in clear",
            cli.listen
        )
        .into());
    }
    let mut hub = Hub::open(&folder, SystemClock)?;
    eprintln!(
        "flottille-hub: state in {}; the hub's key is {}",
        folder.display(),
        hub.key().fingerprint()
    );
    if let Some(line) = hub.take_announcement() {
        eprintln!("flottille-hub: {line}");
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(cli.listen)
            .await
            .map_err(|error| format!("cannot listen on {}: {error}", cli.listen))?;
        let how = if tls.is_some() {
            "TLS on the certificate given"
        } else {
            "plain: the traffic is in clear; put it behind a reverse proxy that holds a certificate"
        };
        let served =
            flottille_hub::serve(hub, listener, tls, Hello::ours(env!("CARGO_PKG_VERSION")))?;
        let stopped = stops()?;
        eprintln!("flottille-hub: listening on {} ({how})", served.address());
        stopped.await;
        eprintln!("flottille-hub: stopping");
        served.stop().await;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

/// Until the hub is asked to stop: an interrupt, or a termination. Listened for before the hub
/// says where it listens, so that a stop that follows at once is taken.
fn stops() -> std::io::Result<impl std::future::Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
    })
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
