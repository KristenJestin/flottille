//! `flottille-hub` (`docs/remote.md`, "The hub"): the user's hub. A start makes its state (its
//! key) or reads it back, and serves the devices' control sessions on its listen address, and the
//! commands of its host on its socket, until it is stopped. Those commands (`pair`, `role`,
//! `remove`, `devices`) talk to the hub that runs, beside it: in a container, `docker exec`.

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use flottille_hub::{Hub, LISTEN, SOCKET_FILE, SystemClock};
use flottille_link::Hello;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "flottille-hub",
    version,
    about = "Flottille's hub: the user's member list, pair mode and presence"
)]
struct Cli {
    /// The hub's state folder [default: $XDG_STATE_HOME/flottille-hub]
    #[arg(long, value_name = "DIR", global = true)]
    state: Option<PathBuf>,
    /// The address the devices reach the hub at, which a pair code holds: through a reverse
    /// proxy mostly (https://hub.example.net). Given to the hub, it is the default of `pair`;
    /// without it, the address the hub listens on, unless that is a loopback or a wildcard
    /// address, which no other device dials: then `pair` asks for one
    #[arg(long, value_name = "URL", global = true)]
    public_url: Option<String>,
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
    /// Pair mode: prints a code and its QR for one device to pair with, then waits until it has,
    /// or until the code's minutes pass. Run beside the hub that runs
    Pair {
        /// How long the code is valid, in minutes: 1 to 15
        #[arg(long, default_value_t = flottille_hub::PAIR_MINUTES)]
        minutes: u64,
    },
    /// Changes the role of a device: runner, controller or viewer
    Role {
        /// The device: its name, or its key's fingerprint
        device: String,
        role: String,
    },
    /// Removes a device: every other device cuts its links to it
    Remove {
        /// The device: its name, or its key's fingerprint
        device: String,
    },
    /// The devices paired, with their roles
    Devices {
        #[arg(long)]
        json: bool,
    },
    /// Empties the member list, with the hub stopped: every device must be paired again
    Reset,
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
        Some(Command::Reset) => {
            flottille_hub::reset(&folder)?;
            println!(
                "The member list is empty: every device must be paired again (flottille-hub pair)"
            );
            return Ok(());
        }
        Some(Command::Pair { minutes }) => return pair(&folder, cli.public_url, minutes),
        Some(Command::Role { device, role }) => {
            let done = ask(&folder, json!({"role": {"device": device, "role": role}}))?;
            println!(
                "{} is now a {}",
                done["name"].as_str().unwrap_or_default(),
                done["role"].as_str().unwrap_or_default()
            );
            return Ok(());
        }
        Some(Command::Remove { device }) => {
            let done = ask(&folder, json!({"remove": {"device": device}}))?;
            println!(
                "{} was removed: every device cuts its links to it",
                done["name"].as_str().unwrap_or_default()
            );
            return Ok(());
        }
        Some(Command::Devices { json }) => {
            let devices = ask(&folder, json!({"devices": {}}))?;
            if json {
                println!("{devices}");
            }
            for device in devices.as_array().into_iter().flatten().filter(|_| !json) {
                println!(
                    "{}  {}  {}  added by {}",
                    device["name"].as_str().unwrap_or_default(),
                    device["role"].as_str().unwrap_or_default(),
                    device["fingerprint"].as_str().unwrap_or_default(),
                    device["added_by"].as_str().unwrap_or_default(),
                );
            }
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
    let hub = Hub::open(&folder, SystemClock)?;
    eprintln!(
        "flottille-hub: state in {}; the hub's key is {}",
        folder.display(),
        hub.key().fingerprint()
    );
    if hub.members().is_none() {
        eprintln!(
            "flottille-hub: no device is paired yet: run flottille-hub --state {} pair beside \
             this hub, as its user (in a container, with docker exec)",
            folder.display()
        );
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
        if let Some(url) = cli.public_url {
            served.set_public_url(url);
        }
        let socket = folder.join(SOCKET_FILE);
        served
            .serve_local(&socket)
            .map_err(|error| format!("{} cannot be listened on: {error}", socket.display()))?;
        let stopped = stops()?;
        eprintln!("flottille-hub: listening on {} ({how})", served.address());
        stopped.await;
        eprintln!("flottille-hub: stopping");
        served.stop().await;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

/// The socket of the hub that runs with the state `folder`, or why there is none to reach.
fn reach(folder: &Path) -> Result<UnixStream, String> {
    let socket = folder.join(SOCKET_FILE);
    UnixStream::connect(&socket).map_err(|error| {
        format!(
            "no flottille-hub runs with {} ({error}): these commands talk to the hub that runs; \
             start it first, then run this beside it (in a container, docker exec)",
            folder.display()
        )
    })
}

/// One command to the hub that runs, and the line it answers.
fn say(stream: &mut UnixStream, asked: &Value) -> Result<(), String> {
    writeln!(stream, "{asked}").map_err(|error| format!("the hub did not take it: {error}"))
}

fn hear(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| format!("the hub did not answer: {error}"))?;
    serde_json::from_str(&line).map_err(|_| "the hub stopped before it answered".to_owned())
}

/// `asked` to the hub that runs: what it did, or its refusal.
fn ask(folder: &Path, asked: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let mut stream = reach(folder)?;
    say(&mut stream, &asked)?;
    let answer = hear(&mut BufReader::new(stream))?;
    match answer.get("refused") {
        Some(reason) => Err(reason.as_str().unwrap_or_default().to_owned().into()),
        None => Ok(answer["ok"].clone()),
    }
}

/// `flottille-hub pair`: pair mode, opened on the hub that runs; its code and QR printed, then how
/// it ended. Stopping the command closes it.
fn pair(
    folder: &Path,
    url: Option<String>,
    minutes: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = reach(folder)?;
    say(
        &mut stream,
        &json!({"pair": {"url": url, "minutes": minutes}}),
    )?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let opened = hear(&mut reader)?;
    if let Some(reason) = opened.get("refused") {
        return Err(reason.as_str().unwrap_or_default().to_owned().into());
    }
    let code = opened["code"].as_str().unwrap_or_default();
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "Pair mode is open for {} s, for one device, with the hub at {}:\n\n{code}\n",
        opened["left"].as_u64().unwrap_or_default(),
        opened["url"].as_str().unwrap_or_default(),
    )?;
    if let Some(drawn) = flottille_hub::qr(code) {
        writeln!(out, "{drawn}")?;
    }
    writeln!(
        out,
        "On the device: Connect to your hub, then paste the code or scan the QR; on a computer's \
         command line, flottille remote pair, then give it the code. Whoever has this code can \
         pair a device until it is used: keep it to yourself. Waiting..."
    )?;
    out.flush()?;
    let ended = hear(&mut reader)?;
    if let Some(paired) = ended.get("paired") {
        let name = paired["name"].as_str().unwrap_or_default();
        if paired["again"] == json!(true) {
            println!(
                "{name} ({}) is paired already: it keeps its card, a {}",
                paired["fingerprint"].as_str().unwrap_or_default(),
                paired["role"].as_str().unwrap_or_default()
            );
        } else {
            println!(
                "{name} ({}) is paired, a {}",
                paired["fingerprint"].as_str().unwrap_or_default(),
                paired["role"].as_str().unwrap_or_default()
            );
        }
        return Ok(());
    }
    Err(ended["over"]
        .as_str()
        .unwrap_or("pair mode ended")
        .to_owned()
        .into())
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
