//! Turning remote on and saying where this machine is reached, from the window and the command
//! line (`docs/remote.md`, "Identity and keys"; `docs/protocol.md`, "Remote"): `remote.settings`
//! reads the choice and what the machine offers, `remote.configure` writes it in `daemon.toml`
//! (keeping every other key and comment of the user's file) and takes it at once: the link
//! starts, stops or listens elsewhere without the daemon being started again. Both are answered to
//! this machine's own clients only.
//!
//! A link that listens somewhere else is closed and made again, which cuts the links it carries
//! (a run on another machine, a tunnel): while an agent runs, the change is written and waits for
//! the next start, and `remote.settings` says so (`waits`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::remote::PORT;

/// What every message that says remote is off says to do.
pub const OFF: &str =
    "remote is off: turn it on in Settings, Machines, or with flottille remote on";

/// How long the port of a link just closed may take to be free again.
const REBIND: Duration = Duration::from_secs(4);

/// One address of this machine a link may listen on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub address: Ipv4Addr,
    /// A Tailscale or WireGuard address: only the user's own network reaches it.
    pub tailnet: bool,
}

/// Whether an interface is a tailnet's: an address of the shared range 100.64.0.0/10, or an
/// interface named for Tailscale or WireGuard.
pub fn is_tailnet(name: &str, address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    (first == 100 && (64..128).contains(&second))
        || name.starts_with("tailscale")
        || name.starts_with("wg")
}

/// The addresses of this machine's own interfaces, a tailnet's first and the others as the system
/// lists them; loopback and interfaces that are down are left out. A test build reads
/// `FLOTTILLE_TEST_INTERFACES` (`[["eth0","192.168.1.20"],…]`) in their place.
pub fn interfaces() -> Vec<Interface> {
    let found: Vec<(String, Ipv4Addr)> = match test_interfaces() {
        Some(found) => found,
        None => system_interfaces(),
    };
    let (mut tailnet, others): (Vec<_>, Vec<_>) = found
        .into_iter()
        .map(|(name, address)| Interface {
            tailnet: is_tailnet(&name, address),
            name,
            address,
        })
        .partition(|interface| interface.tailnet);
    tailnet.extend(others);
    tailnet
}

fn test_interfaces() -> Option<Vec<(String, Ipv4Addr)>> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let text = std::env::var("FLOTTILLE_TEST_INTERFACES").ok()?;
    let listed: Vec<(String, Ipv4Addr)> = serde_json::from_str(&text).ok()?;
    Some(listed)
}

fn system_interfaces() -> Vec<(String, Ipv4Addr)> {
    let mut found = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` fills `list` with a chain it owns until `freeifaddrs`; every pointer
    // read below is one of that chain, and the chain is freed once, at the end.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return found;
        }
        let mut at = list;
        while !at.is_null() {
            let one = &*at;
            at = one.ifa_next;
            if one.ifa_addr.is_null()
                || i32::from((*one.ifa_addr).sa_family) != libc::AF_INET
                || one.ifa_flags & libc::IFF_UP as u32 == 0
                || one.ifa_flags & libc::IFF_LOOPBACK as u32 != 0
            {
                continue;
            }
            let socket = &*one.ifa_addr.cast::<libc::sockaddr_in>();
            let address = Ipv4Addr::from(u32::from_be(socket.sin_addr.s_addr));
            let name = std::ffi::CStr::from_ptr(one.ifa_name)
                .to_string_lossy()
                .into_owned();
            found.push((name, address));
        }
        libc::freeifaddrs(list);
    }
    found
}

/// Where a link listens, as the user said it: `nowhere`, `tailnet` (the first tailnet address of
/// this machine), or an address with its port (`7447` when it has none).
pub fn parse_listen(text: &str) -> Result<Option<SocketAddr>, String> {
    let text = text.trim();
    match text {
        "nowhere" => return Ok(None),
        "tailnet" => {
            return interfaces()
                .into_iter()
                .find(|interface| interface.tailnet)
                .map(|interface| Some(SocketAddr::new(IpAddr::V4(interface.address), PORT)))
                .ok_or_else(|| {
                    "this machine has no tailnet address (Tailscale or WireGuard): give an \
                     address instead"
                        .to_owned()
                });
        }
        _ => {}
    }
    if let Ok(address) = text.parse::<SocketAddr>() {
        return Ok(Some(address));
    }
    text.parse::<IpAddr>()
        .map(|ip| Some(SocketAddr::new(ip, PORT)))
        .map_err(|_| {
            format!(
                "\"{text}\" is not an IP address and a port (0.0.0.0:{PORT}, 100.64.0.7:{PORT}), \
                 nor nowhere or tailnet"
            )
        })
}

/// Whether a link listening at `actual` is where `wanted` says: a port 0 is any.
fn listens_as(wanted: Option<SocketAddr>, actual: Option<SocketAddr>) -> bool {
    match (wanted, actual) {
        (None, None) => true,
        (Some(wanted), Some(actual)) => {
            wanted.ip() == actual.ip() && (wanted.port() == 0 || wanted.port() == actual.port())
        }
        _ => false,
    }
}

/// `daemon.toml`'s text with `[remote]` changed as asked: `on`, and `listen` (`Some(None)` takes
/// the key away). Every other key and every comment stays as the user wrote it.
fn edited(
    text: &str,
    on: Option<bool>,
    listen: Option<Option<SocketAddr>>,
) -> Result<String, String> {
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            format!(
                "daemon.toml is not valid TOML, so it is left as it is: {}",
                error.message()
            )
        })?;
    if document.get("remote").is_some_and(|item| !item.is_table()) {
        return Err(
            "daemon.toml has a `remote` that is not a table, so it is left as it is".into(),
        );
    }
    let table = document
        .entry("remote")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("daemon.toml has a `remote` that is not a table")?;
    if let Some(on) = on {
        table["on"] = toml_edit::value(on);
    }
    match listen {
        Some(Some(address)) => table["listen"] = toml_edit::value(address.to_string()),
        Some(None) => {
            table.remove("listen");
        }
        None => {}
    }
    Ok(document.to_string())
}

/// `text` written as `daemon.toml`, whole: a temporary file renamed into place.
fn write(paths: &Paths, text: &str) -> Result<(), String> {
    let file = paths.config.join("daemon.toml");
    std::fs::create_dir_all(&paths.config)
        .map_err(|error| format!("{} is not made: {error}", paths.config.display()))?;
    let temporary = file.with_extension("toml.new");
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, &file))
        .map_err(|error| format!("daemon.toml is not written: {error}"))
}

fn read(paths: &Paths) -> String {
    std::fs::read_to_string(paths.config.join("daemon.toml")).unwrap_or_default()
}

/// What `remote.settings` and `remote.configure` answer.
fn shown(daemon: &Daemon) -> Value {
    let paths = &daemon.paths;
    let on = crate::remote::on(paths);
    let (configured, problem) = match crate::remote::listen(paths) {
        Ok(listen) => (listen, None),
        Err(why) => (None, Some(why)),
    };
    let link = match daemon.link() {
        Some(Ok(link)) => Some(link),
        _ => None,
    };
    let listening = link.as_ref().and_then(|link| link.listening());
    let waits = link.is_some() && on && !listens_as(configured, listening);
    json!({
        "on": on,
        "listen": configured.map(|address| address.to_string()),
        "listening": listening.map(|address| address.to_string()),
        "waits": waits,
        "port": PORT,
        "problem": problem,
        "interfaces": interfaces()
            .into_iter()
            .map(|interface| json!({
                "name": interface.name,
                "address": interface.address.to_string(),
                "tailnet": interface.tailnet,
            }))
            .collect::<Vec<_>>(),
    })
}

pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "remote.settings" => Ok(shown(&daemon)),
        "remote.configure" => configure(&daemon, &params).await,
        _ => Err(Failure::new(
            flottille_protocol::code::UNKNOWN_METHOD,
            format!("unknown method {method}"),
        )),
    }
}

async fn configure(daemon: &Arc<Daemon>, params: &Value) -> Result<Value, Failure> {
    let invalid = |why: String| Failure::new(flottille_protocol::code::INVALID_PARAMS, why);
    let on = match &params["on"] {
        Value::Null => None,
        Value::Bool(on) => Some(*on),
        _ => return Err(invalid("on is true or false".into())),
    };
    let listen = match &params["listen"] {
        Value::Null => None,
        Value::String(text) => Some(parse_listen(text).map_err(invalid)?),
        _ => return Err(invalid("listen is an address, tailnet or nowhere".into())),
    };
    let paths = &daemon.paths;
    let before = read(paths);
    let text = edited(&before, on, listen).map_err(Failure::refused)?;
    write(paths, &text).map_err(Failure::refused)?;
    match apply(daemon).await {
        Ok(()) => Ok(shown(daemon)),
        Err(why) => {
            // What was refused changes nothing: the file is as it was, the link too.
            let _ = write(paths, &before);
            let _ = apply(daemon).await;
            Err(Failure::refused(why))
        }
    }
}

/// Takes what `daemon.toml` says now: the link starts when it is on and none runs, stops when it is
/// off, and listens elsewhere when that is asked and no agent runs (it waits otherwise).
async fn apply(daemon: &Arc<Daemon>) -> Result<(), String> {
    let paths = &daemon.paths;
    let on = crate::remote::on(paths);
    let wanted = crate::remote::listen(paths)?;
    let running = match daemon.link() {
        Some(Ok(link)) => Some(link),
        _ => None,
    };
    match (running, on) {
        (None, false) => {
            // A link that failed to start is no link: it goes with the switch.
            daemon.set_link(None);
            Ok(())
        }
        (Some(link), false) => {
            daemon.set_link(None);
            link.stop().await;
            Ok(())
        }
        (None, true) => start(daemon, paths).await,
        (Some(link), true) => {
            if listens_as(wanted, link.listening()) {
                return Ok(());
            }
            // Closing the link cuts what it carries: an agent that runs is not cut under.
            if !daemon.agents.with_process().is_empty() {
                return Ok(());
            }
            daemon.set_link(None);
            link.stop().await;
            drop(link);
            start(daemon, paths).await
        }
    }
}

/// Starts the link as `daemon.toml` says: the machine key is made at the first time. The port of
/// a link just closed is waited for a few seconds.
async fn start(daemon: &Arc<Daemon>, paths: &Paths) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + REBIND;
    loop {
        let key = crate::remote::machine_key(paths)
            .map_err(|error| error.to_string())?
            .ok_or("remote is off")?;
        match crate::daemon::start_link(daemon, key) {
            Ok(()) => return Ok(()),
            Err(why) if tokio::time::Instant::now() >= deadline => {
                return Err(format!("the link did not start: {why}"));
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(150)).await,
        }
    }
}
