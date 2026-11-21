//! `flottille remote`: Flottille on the user's other machines (`docs/remote.md`). Here, pairing
//! with the hub (`pair`, with the code `flottille-hub pair` printed on the hub's host), the members
//! (`members`, `rename`, `icon`, `colour`, `remove`), and the addresses this machine may be reached
//! at: those its link listens on, and those the user declares by hand (a VPN address, a DNS name, a
//! forwarded port), which it tells the machines it is linked to.
//!
//! The device key that signs the user's actions is this command's, never the daemon's: it is
//! made and read here, in the session's keyring (or `remote/device.key` without one), and only
//! what it signed goes to the daemon.

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum RemoteAction {
    /// Turn remote on: this machine can join a hub and link to your other machines. Nothing is
    /// opened until you say where it is reached
    On {
        /// Where this machine is reached: an address and its port (7447 when it has none),
        /// `tailnet` (its first Tailscale or WireGuard address) or `nowhere` (the default: it
        /// dials the others, nobody dials it)
        #[arg(long)]
        listen: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Turn remote off: the links close, this machine stays paired with its hub
    Off {
        #[arg(long)]
        json: bool,
    },
    /// Say where this machine is reached: an address and its port (7447 when it has none),
    /// `tailnet` or `nowhere`
    Listen {
        address: String,
        #[arg(long)]
        json: bool,
    },
    /// Pair this machine with the user's hub, with the code `flottille-hub pair` printed on the
    /// hub's host, read on the standard input (asked for on a terminal), never on the command line.
    /// The name, icon and role proposed are this machine's own unless given
    Pair {
        /// This device's name among the user's [default: the host's name]
        #[arg(long)]
        name: Option<String>,
        /// What it may do [default: runner with agents signed in, controller without]
        #[arg(long, value_enum)]
        role: Option<RoleArg>,
        /// How it is drawn [default: laptop with a battery, desktop with a screen, server]
        #[arg(long, value_enum)]
        icon: Option<IconArg>,
        /// Its colour [default: one of its key's]
        #[arg(long, value_enum)]
        colour: Option<ColourArg>,
        /// Pair this machine with another hub than the one it is paired with
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        json: bool,
    },
    /// The members of the hub's list, as this machine last checked it
    Members {
        /// The runners instead, with what each offers: its projects and their free slots, its
        /// agents signed in
        #[arg(long)]
        runners: bool,
        /// With --runners: only those that would take a session of this project
        #[arg(long, requires = "runners")]
        project: Option<String>,
        /// With --project: only those where this agent is signed in
        #[arg(long, requires = "project")]
        agent: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Rename a member
    Rename {
        name: String,
        new_name: String,
        #[arg(long)]
        json: bool,
    },
    /// Change how a member is drawn
    Icon {
        name: String,
        #[arg(value_enum)]
        icon: IconArg,
        #[arg(long)]
        json: bool,
    },
    /// Change a member's colour
    Colour {
        name: String,
        #[arg(value_enum)]
        colour: ColourArg,
        #[arg(long)]
        json: bool,
    },
    /// Remove a member: its links are cut everywhere; it comes back only paired again on the hub
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// This machine's addresses: those its link listens on, and those declared by hand
    Address {
        #[command(subcommand)]
        action: AddressAction,
    },
}

/// What a device paired may do (`docs/remote.md`, "Roles").
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum RoleArg {
    Runner,
    Controller,
    Viewer,
}

impl RoleArg {
    fn role(self) -> flottille_link::Role {
        match self {
            RoleArg::Runner => flottille_link::Role::Runner,
            RoleArg::Controller => flottille_link::Role::Controller,
            RoleArg::Viewer => flottille_link::Role::Viewer,
        }
    }
}

/// How a device is drawn (`docs/remote.md`, "A device's card").
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum IconArg {
    Desktop,
    Laptop,
    Server,
    Phone,
    Tablet,
}

impl IconArg {
    fn icon(self) -> flottille_link::Icon {
        flottille_link::Icon::ALL[self as usize]
    }
}

/// A device's colour.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ColourArg {
    Grey,
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Purple,
}

impl ColourArg {
    fn colour(self) -> flottille_link::Colour {
        flottille_link::Colour::ALL[self as usize]
    }
}

#[derive(Subcommand)]
pub enum AddressAction {
    /// The addresses, found or declared
    List {
        #[arg(long)]
        json: bool,
    },
    /// Declare an address another machine reaches this one at: a host name or an IP address,
    /// and its port when it is not the link's own
    Add {
        address: String,
        #[arg(long)]
        json: bool,
    },
    /// Remove an address declared by hand
    Remove {
        address: String,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(paths: &Paths, action: RemoteAction) -> Result<ExitCode> {
    let mut client = Client::connect_or_start(paths)?;
    let mut ask = |method: &str, params: Value| {
        client
            .request(method, params)
            .map_err(|error| Box::<dyn std::error::Error>::from(in_full(paths, &error.to_string())))
    };
    let action = match action {
        RemoteAction::Address { action } => action,
        RemoteAction::On { listen, json } => {
            let mut params = json!({"on": true});
            if let Some(listen) = listen {
                params["listen"] = json!(listen);
            }
            let settings = ask("remote.configure", params)?;
            say_settings(&settings, "Remote is on.", json);
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Off { json } => {
            let settings = ask("remote.configure", json!({"on": false}))?;
            say_settings(
                &settings,
                "Remote is off: the links are closed, this machine stays paired with its hub.",
                json,
            );
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Listen { address, json } => {
            let settings = ask("remote.configure", json!({"listen": address}))?;
            say_settings(&settings, "", json);
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Members {
            runners: true,
            project,
            agent,
            json,
        } => {
            let params = match &project {
                Some(project) => json!({"project": project, "agent": agent}),
                None => json!({}),
            };
            let machines = ask("remote.machines", params)?;
            if json {
                println!("{machines}");
                return Ok(ExitCode::SUCCESS);
            }
            let offered = machines["offered"].as_array();
            let others = machines["machines"].as_array().into_iter().flatten();
            for machine in std::iter::once(&machines["this"]).chain(others) {
                let name = machine["name"].as_str().unwrap_or_default();
                let taken = offered.is_none_or(|offered| offered.iter().any(|n| n == name));
                if machine["role"] == "runner" && taken {
                    println!("{name}  {}", offer_line(machine));
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Members { json, .. } => {
            let members = ask("remote.members", json!({}))?;
            if json {
                println!("{members}");
            } else {
                for member in members["members"].as_array().into_iter().flatten() {
                    let name = member["name"].as_str().unwrap_or_default();
                    println!(
                        "{name}  {}  {}  added by {}{}",
                        member["role"].as_str().unwrap_or_default(),
                        member["fingerprint"].as_str().unwrap_or_default(),
                        member["added_by"].as_str().unwrap_or_default(),
                        if members["this"] == name {
                            "  (this machine)"
                        } else {
                            ""
                        }
                    );
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Rename {
            name,
            new_name,
            json,
        } => {
            let device = device_key(paths)?;
            let members = ask("remote.members", json!({}))?;
            let key = member_key(&members, &name)?;
            let renamed = signed_after(
                &members,
                flottille_link::Action::Rename {
                    key,
                    name: new_name.clone(),
                },
                &device,
            )?;
            let members = ask("remote.change", json!({"change": renamed}))?;
            if json {
                println!("{members}");
            } else {
                println!("{name} is now {new_name}");
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Icon { name, icon, json } => {
            look(paths, &mut ask, &name, Some(icon.icon()), None, json)?;
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Colour { name, colour, json } => {
            look(paths, &mut ask, &name, None, Some(colour.colour()), json)?;
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Remove { name, json } => {
            let device = device_key(paths)?;
            let members = ask("remote.members", json!({}))?;
            let key = member_key(&members, &name)?;
            let removal = signed_after(&members, flottille_link::Action::Remove { key }, &device)?;
            let members = ask("remote.change", json!({"change": removal}))?;
            if json {
                println!("{members}");
            } else {
                println!(
                    "{name} was removed: its links are cut, and it comes back only paired again on \
                     the hub"
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Pair {
            name,
            role,
            icon,
            colour,
            replace,
            json,
        } => {
            let code = read_code("The pair code: ", "no pair code")?;
            // Refused here when it is none, before anything is made.
            flottille_link::PairCode::from_code(&code)?;
            let device = device_key_to_pair(paths)?;
            let machines = ask("remote.machines", json!({}))?;
            let proposed = &machines["proposed"];
            let machine: flottille_link::PublicKey =
                serde_json::from_value(machines["this"]["key"].clone())?;
            let card = flottille_link::Proposed {
                key: device.public(),
                name: match name {
                    Some(name) => name,
                    None => proposed["name"]
                        .as_str()
                        .ok_or("the host has no name: give one with --name")?
                        .to_owned(),
                },
                role: match role {
                    Some(role) => role.role(),
                    None => serde_json::from_value(proposed["role"].clone())
                        .unwrap_or(flottille_link::Role::Controller),
                },
                icon: match icon {
                    Some(icon) => icon.icon(),
                    None => serde_json::from_value(proposed["icon"].clone())
                        .unwrap_or(flottille_link::Icon::Desktop),
                },
                colour: colour.map_or_else(
                    || flottille_link::Colour::of(&device.public()),
                    ColourArg::colour,
                ),
            };
            let paired = ask(
                "remote.pair",
                json!({"code": code.as_str(), "proposed": card, "replace": replace}),
            )?;
            let head: flottille_link::ChangeHash = serde_json::from_value(paired["head"].clone())?;
            // Claimed by the machine's daemon, which holds its key.
            let declared = flottille_link::Change::sign(
                head,
                flottille_link::Action::Machine {
                    key: device.public(),
                    machine,
                },
                &device,
            );
            let pinned = ask("remote.declare", json!({"change": declared}))?;
            if json {
                println!("{}", json!({"paired": paired, "pinned": pinned}));
            } else {
                println!(
                    "{} is paired with the hub at {} as a {}",
                    paired["name"].as_str().unwrap_or_default(),
                    pinned["url"].as_str().unwrap_or_default(),
                    paired["role"].as_str().unwrap_or_default(),
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
    };
    let (this, json) = match action {
        AddressAction::List { json } => (ask("remote.machines", json!({}))?["this"].clone(), json),
        AddressAction::Add { address, json } => (
            ask("remote.address.add", json!({"address": address}))?,
            json,
        ),
        AddressAction::Remove { address, json } => (
            ask("remote.address.remove", json!({"address": address}))?,
            json,
        ),
    };
    if json {
        println!("{this}");
        return Ok(ExitCode::SUCCESS);
    }
    for address in this["addresses"].as_array().into_iter().flatten() {
        println!(
            "{}  {}",
            address["address"].as_str().unwrap_or_default(),
            address["how"].as_str().unwrap_or_default()
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// A member's icon or colour changed, the other kept, signed with this device's key.
fn look(
    paths: &Paths,
    ask: &mut impl FnMut(&str, Value) -> Result<Value>,
    name: &str,
    icon: Option<flottille_link::Icon>,
    colour: Option<flottille_link::Colour>,
    json: bool,
) -> Result<()> {
    let device = device_key(paths)?;
    let members = ask("remote.members", json!({}))?;
    let key = member_key(&members, name)?;
    let entry = members["members"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|member| member["name"] == name)
        .cloned()
        .unwrap_or_default();
    let icon = icon
        .or_else(|| serde_json::from_value(entry["icon"].clone()).ok())
        .unwrap_or(flottille_link::Icon::Desktop);
    let colour = colour
        .or_else(|| serde_json::from_value(entry["colour"].clone()).ok())
        .unwrap_or_else(|| flottille_link::Colour::of(&key));
    let change = signed_after(
        &members,
        flottille_link::Action::Look { key, icon, colour },
        &device,
    )?;
    let members = ask("remote.change", json!({"change": change}))?;
    if json {
        println!("{members}");
    } else {
        println!("{name} is drawn as a {} {}", colour.word(), icon.word());
    }
    Ok(())
}

/// What a runner offers, as `remote members --runners` says it: `app (3 free), site (none
/// free)  agents: claude`; `offers nothing it said` for one whose offer did not come.
fn offer_line(machine: &Value) -> String {
    let offers = &machine["offers"];
    if offers.is_null() {
        return format!(
            "{}  offers nothing it said",
            machine["state"].as_str().unwrap_or("this machine")
        );
    }
    let projects: Vec<String> = offers["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|project| {
            let name = project["name"].as_str().unwrap_or_default();
            match project["slots"].as_u64().unwrap_or(0) {
                0 => format!("{name} (none free)"),
                slots => format!("{name} ({slots} free)"),
            }
        })
        .collect();
    let agents: Vec<&str> = offers["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let or_none = |list: String| {
        if list.is_empty() {
            "none".to_owned()
        } else {
            list
        }
    };
    format!(
        "projects: {}  agents: {}",
        or_none(projects.join(", ")),
        or_none(agents.join(", "))
    )
}

/// What `remote.configure` answered, as `--json` prints it or in words: what was done, then where
/// this machine is reached.
fn say_settings(settings: &Value, done: &str, json: bool) {
    if json {
        println!("{settings}");
        return;
    }
    let mut line = done.to_owned();
    if settings["on"] == true {
        let at = match settings["listen"].as_str() {
            Some(at) => format!("This machine is reached at {at}."),
            None => {
                "This machine is reached nowhere: it dials the others, nobody dials it.".to_owned()
            }
        };
        line = format!("{line} {at}").trim().to_owned();
        if settings["waits"] == true {
            line.push_str(" An agent runs: the change waits for the next start of Flottille.");
        }
    } else if line.is_empty() {
        line = "Remote is off: the choice is kept for when it is turned on.".to_owned();
    }
    println!("{line}");
}

/// The key of the member named `name` in `remote.members`' answer.
fn member_key(members: &Value, name: &str) -> Result<flottille_link::PublicKey> {
    let found = members["members"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|member| member["name"] == name)
        .ok_or_else(|| format!("{name} is not a member: flottille remote members lists them"))?;
    Ok(serde_json::from_value(found["key"].clone())?)
}

/// `action`, signed with `device` after the head of the list `remote.members` answered.
fn signed_after(
    members: &Value,
    action: flottille_link::Action,
    device: &flottille_link::DeviceKey,
) -> Result<flottille_link::Change> {
    let head: flottille_link::ChangeHash = serde_json::from_value(members["head"].clone())?;
    Ok(flottille_link::Change::sign(head, action, device))
}

/// A pair code, from the standard input: asked for when it is a terminal. Never an argument, which
/// other users of the machine and the shell's history would see.
fn read_code(prompt: &str, none: &str) -> Result<zeroize::Zeroizing<String>> {
    use std::io::{BufRead, IsTerminal, Write};
    if std::io::stdin().is_terminal() {
        eprint!("{prompt}");
        std::io::stderr().flush()?;
    }
    let mut code = zeroize::Zeroizing::new(String::new());
    std::io::stdin().lock().read_line(&mut code)?;
    let trimmed = zeroize::Zeroizing::new(code.trim().to_owned());
    if trimmed.is_empty() {
        return Err(format!("{none} was given: type it, or give it on the standard input").into());
    }
    Ok(trimmed)
}

/// The device key of this user session's clients, a member's: in the session's keyring, or, with
/// none, `remote/device.key`. A pairing makes it.
pub(crate) fn device_key(paths: &Paths) -> Result<flottille_link::DeviceKey> {
    kept_device_key(paths)?.ok_or_else(|| {
        "this device holds no key of a member: flottille remote pair, with the code flottille-hub \
         pair prints on the hub's host"
            .into()
    })
}

fn kept_device_key(paths: &Paths) -> Result<Option<flottille_link::DeviceKey>> {
    with_keyring(paths, |keyring, folder| {
        flottille_link::DeviceKey::load(keyring, folder)
    })
}

/// The device key a pairing is made with: the one kept, made once (`docs/remote.md`, "Identity and
/// keys"); one a setup or a join of before pair mode left pending, kept now; or a new one, kept at
/// once. A device paired again with its key finds its card on the hub.
fn device_key_to_pair(paths: &Paths) -> Result<flottille_link::DeviceKey> {
    if let Some(key) = kept_device_key(paths)? {
        return Ok(key);
    }
    let pending = with_keyring(paths, |keyring, folder| {
        flottille_link::DeviceKey::load_pending(keyring, folder)
    })?;
    let key = match pending {
        Some(pending) => pending,
        None => {
            let key = flottille_link::DeviceKey::make()?;
            let public = key.public();
            let pending = with_keyring(paths, |keyring, folder| key.keep_pending(keyring, folder))?;
            if pending.public() != public {
                return Err(ANOTHER_CLIENT.into());
            }
            pending
        }
    };
    keep_new(paths, key)
}

const ANOTHER_CLIENT: &str = "another client kept a device key at the same moment: run flottille \
                              remote members to see which one is a member";

/// `act` on the keyring of this session's clients, and the key folder.
fn with_keyring<T>(
    paths: &Paths,
    act: impl FnOnce(
        &ClientKeyring,
        &std::path::Path,
    ) -> std::result::Result<T, flottille_link::KeyError>,
) -> Result<T> {
    let keyring = if crate::vault::memory_chosen(paths) {
        ClientKeyring::None(NoKeyring)
    } else {
        ClientKeyring::Session(SessionKeyring)
    };
    Ok(act(&keyring, &crate::remote::folder(paths))?)
}

/// The device key of this session's clients as the window holds it: the same keyring and folder
/// as the commands', which the window signs with.
pub(crate) fn window_device(paths: &Paths) -> flottille_window::device::Device {
    let keyring: std::sync::Arc<dyn flottille_link::Keyring + Send + Sync> =
        if crate::vault::memory_chosen(paths) {
            std::sync::Arc::new(ClientKeyring::None(NoKeyring))
        } else {
            std::sync::Arc::new(ClientKeyring::Session(SessionKeyring))
        };
    flottille_window::device::Device::new(keyring, crate::remote::folder(paths))
}

/// The keyring of this session's clients: none in a test build whose configuration keeps the
/// daemon's secrets in its memory.
enum ClientKeyring {
    None(NoKeyring),
    Session(SessionKeyring),
}

impl flottille_link::Keyring for ClientKeyring {
    fn read(
        &self,
        attributes: &[(&str, &str)],
    ) -> std::result::Result<Option<zeroize::Zeroizing<Vec<u8>>>, flottille_link::KeyringError>
    {
        match self {
            ClientKeyring::None(keyring) => keyring.read(attributes),
            ClientKeyring::Session(keyring) => keyring.read(attributes),
        }
    }

    fn store(
        &self,
        label: &str,
        attributes: &[(&str, &str)],
        secret: &[u8],
    ) -> std::result::Result<(), flottille_link::KeyringError> {
        match self {
            ClientKeyring::None(keyring) => keyring.store(label, attributes, secret),
            ClientKeyring::Session(keyring) => keyring.store(label, attributes, secret),
        }
    }

    fn delete(
        &self,
        attributes: &[(&str, &str)],
    ) -> std::result::Result<(), flottille_link::KeyringError> {
        match self {
            ClientKeyring::None(keyring) => keyring.delete(attributes),
            ClientKeyring::Session(keyring) => keyring.delete(attributes),
        }
    }
}

/// Keeps `key`, the pending key of a pairing.
fn keep_new(paths: &Paths, key: flottille_link::DeviceKey) -> Result<flottille_link::DeviceKey> {
    let public = key.public();
    let kept = with_keyring(paths, |keyring, folder| key.promote(keyring, folder))?;
    if kept.public() != public {
        return Err(ANOTHER_CLIENT.into());
    }
    Ok(kept)
}

/// No keyring: a test build whose configuration keeps the daemon's secrets in its memory.
struct NoKeyring;

impl flottille_link::Keyring for NoKeyring {
    fn read(
        &self,
        _attributes: &[(&str, &str)],
    ) -> std::result::Result<Option<zeroize::Zeroizing<Vec<u8>>>, flottille_link::KeyringError>
    {
        Err(flottille_link::KeyringError::Absent)
    }

    fn store(
        &self,
        _label: &str,
        _attributes: &[(&str, &str)],
        _secret: &[u8],
    ) -> std::result::Result<(), flottille_link::KeyringError> {
        Err(flottille_link::KeyringError::Absent)
    }

    fn delete(
        &self,
        _attributes: &[(&str, &str)],
    ) -> std::result::Result<(), flottille_link::KeyringError> {
        Err(flottille_link::KeyringError::Absent)
    }
}

/// The Secret Service of the session; absent when there is none to reach (a headless server).
struct SessionKeyring;

impl SessionKeyring {
    fn with<T>(
        &self,
        act: impl AsyncFnOnce(oo7::Keyring) -> std::result::Result<T, flottille_link::KeyringError>,
    ) -> std::result::Result<T, flottille_link::KeyringError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| flottille_link::KeyringError::Failed(error.to_string()))?;
        runtime.block_on(async {
            let keyring = oo7::Keyring::new().await.map_err(|error| {
                if no_service(&error) {
                    flottille_link::KeyringError::Absent
                } else {
                    flottille_link::KeyringError::Failed(format!("the keyring: {error}"))
                }
            })?;
            act(keyring).await
        })
    }
}

/// Whether `error` says there is no Secret Service to reach: no session bus, or none that has it.
/// Any other failure is said, and the device key is not made in a file instead: a second identity
/// for a passing failure would be worse than a refusal.
pub(crate) fn no_service(error: &oo7::Error) -> bool {
    use oo7::zbus;
    let oo7::Error::DBus(oo7::dbus::Error::ZBus(error)) = error else {
        return false;
    };
    let absent = |name: &str| {
        matches!(
            name,
            "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner"
        )
    };
    match error {
        zbus::Error::Address(_) => true,
        // No bus at its address: what a session with none, a headless server's, gives.
        zbus::Error::InputOutput(error) | zbus::Error::Connection(error, _) => matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        ),
        zbus::Error::FDO(error) => matches!(
            **error,
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)
        ),
        zbus::Error::MethodError(name, _, _) => absent(name.as_str()),
        _ => false,
    }
}

impl flottille_link::Keyring for SessionKeyring {
    fn read(
        &self,
        attributes: &[(&str, &str)],
    ) -> std::result::Result<Option<zeroize::Zeroizing<Vec<u8>>>, flottille_link::KeyringError>
    {
        let failed = |error: oo7::Error| flottille_link::KeyringError::Failed(error.to_string());
        self.with(async |keyring| {
            let items = keyring.search_items(&attributes).await.map_err(failed)?;
            let item = match items.as_slice() {
                [] => return Ok(None),
                [item] => item,
                // Two clients kept one at the same moment: which is the member's cannot be told.
                more => return Err(flottille_link::KeyringError::Several(more.len())),
            };
            if item.is_locked().await.map_err(failed)? {
                return Err(flottille_link::KeyringError::Failed(
                    crate::vault::LOCKED.to_owned(),
                ));
            }
            let secret = item.secret().await.map_err(failed)?;
            Ok(Some(zeroize::Zeroizing::new(secret.as_bytes().to_vec())))
        })
    }

    fn store(
        &self,
        label: &str,
        attributes: &[(&str, &str)],
        secret: &[u8],
    ) -> std::result::Result<(), flottille_link::KeyringError> {
        self.with(async |keyring| {
            keyring
                .create_item(label, &attributes, secret, false)
                .await
                .map_err(|error| flottille_link::KeyringError::Failed(error.to_string()))
        })
    }

    fn delete(
        &self,
        attributes: &[(&str, &str)],
    ) -> std::result::Result<(), flottille_link::KeyringError> {
        self.with(async |keyring| {
            keyring
                .delete(&attributes)
                .await
                .map_err(|error| flottille_link::KeyringError::Failed(error.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oo7::zbus;

    fn dbus(error: zbus::Error) -> oo7::Error {
        oo7::Error::DBus(oo7::dbus::Error::ZBus(error))
    }

    #[test]
    fn only_a_secret_service_that_is_not_there_is_no_keyring() {
        let io = |kind: std::io::ErrorKind| {
            dbus(zbus::Error::InputOutput(std::sync::Arc::new(kind.into())))
        };
        // No session bus, or none that has a Secret Service: the key file.
        assert!(no_service(&dbus(zbus::Error::Address("no bus".to_owned()))));
        assert!(no_service(&io(std::io::ErrorKind::NotFound)));
        assert!(no_service(&io(std::io::ErrorKind::ConnectionRefused)));
        assert!(no_service(&dbus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::ServiceUnknown("org.freedesktop.secrets".to_owned())
        )))));
        // Anything else is a failure, said.
        assert!(!no_service(&io(std::io::ErrorKind::PermissionDenied)));
        assert!(!no_service(&io(std::io::ErrorKind::TimedOut)));
        assert!(!no_service(&dbus(zbus::Error::Failure("busy".to_owned()))));
        assert!(!no_service(&oo7::Error::DBus(oo7::dbus::Error::Dismissed)));
        assert!(!no_service(&dbus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::AccessDenied("no".to_owned())
        )))));
    }
}
