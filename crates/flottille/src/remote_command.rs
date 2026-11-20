//! `flottille remote`: Flottille on the user's other machines (`docs/remote.md`). Here, the hub's
//! first member (`setup`), pairing (`invite`, `join`, `requests`, `accept`, `refuse`), the members
//! (`members`, `rename`, `remove`), and the addresses this machine may be reached at: those its link
//! listens on, and those the user declares by hand (a VPN address, a DNS name, a forwarded port),
//! which it tells the machines it is linked to.
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
    /// Make this machine the first member of a new hub, with the setup code it printed, read on
    /// the standard input (asked for on a terminal), never on the command line
    Setup {
        /// The hub's URL: http:// or https://, its host and its port
        url: String,
        /// This device's name among the user's [default: the host's name]
        #[arg(long)]
        name: Option<String>,
        /// Set this machine up with another hub than the one it is set up with
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        json: bool,
    },
    /// Ask the hub for an invitation: a code, valid 5 minutes, once, that a new device joins with
    Invite {
        #[arg(long)]
        json: bool,
    },
    /// Join the user's hub with an invitation's code, read on the standard input (asked for on a
    /// terminal), never on the command line: this machine's device prints the four words to
    /// compare, then waits until the member that invited accepts it
    Join {
        #[arg(long)]
        json: bool,
    },
    /// The join requests that wait for this machine's member, with their four words
    Requests {
        #[arg(long)]
        json: bool,
    },
    /// Accept a join request, once its four words are those the new device shows
    Accept {
        /// The request: the new device's fingerprint, as `flottille remote requests` says it
        request: String,
        /// The new device's name among the user's
        #[arg(long)]
        name: String,
        /// What it may do: run agents, control only, or view only
        #[arg(long, value_enum)]
        role: RoleArg,
        #[arg(long)]
        json: bool,
    },
    /// Refuse a join request
    Refuse {
        request: String,
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
    /// Remove a member: its links are cut everywhere, and its key never joins again
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

/// What a device accepted may do (`docs/remote.md`, "Roles").
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
        RemoteAction::Invite { json } => {
            let hub: flottille_link::PublicKey =
                serde_json::from_value(ask("remote.machines", json!({}))?["hub"]["key"].clone())
                    .map_err(|_| {
                        "this machine is not set up with a hub: flottille remote setup, or \
                         flottille remote join"
                    })?;
            let device = device_key(paths)?;
            let ask_ = flottille_link::InvitationAsk::sign(&hub, unix_now(), &device)?;
            let invited = ask("remote.invite", json!({"ask": ask_}))?;
            if json {
                println!("{invited}");
            } else {
                let code = invited["code"].as_str().unwrap_or_default();
                println!(
                    "An invitation to the hub at {}, valid 5 minutes, once, from {} ({}):\n\n{code}\n\n\
                     The QR holds the same text. On the new device: flottille remote join, then \
                     give it the code",
                    invited["url"].as_str().unwrap_or_default(),
                    invited["inviter"].as_str().unwrap_or_default(),
                    invited["fingerprint"].as_str().unwrap_or_default(),
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Join { json } => {
            let code = read_code("The invitation's code: ", "no invitation code")?;
            let invitation = flottille_link::Invitation::from_code(&code)?;
            let resumed = settle_pending(paths, &mut ask)?;
            let machine: flottille_link::PublicKey =
                serde_json::from_value(ask("remote.machines", json!({}))?["this"]["key"].clone())?;
            // A join accepted while its command was gone goes on with its answer and its key.
            let (device, new, accepted) = match resumed {
                Some((accepted, pending)) => (pending, true, accepted),
                None => {
                    let (device, new) = device_key_to_join(paths)?;
                    let words = invitation.words(&device.public());
                    if json {
                        println!("{}", json!({"words": words}));
                    } else {
                        println!(
                            "Verification words: {}\nThe device that invited shows them too: \
                             accept there only if they are the same. Waiting for it...",
                            words.join(" ")
                        );
                    }
                    std::io::Write::flush(&mut std::io::stdout())?;
                    let accepted = ask(
                        "remote.join",
                        json!({"code": code.as_str(), "key": device.public()}),
                    )?;
                    (device, new, accepted)
                }
            };
            let head: flottille_link::ChangeHash =
                serde_json::from_value(accepted["head"].clone())?;
            // Claimed by the machine's daemon, which holds its key.
            let declared = flottille_link::Change::sign(
                head,
                flottille_link::Action::Machine {
                    key: device.public(),
                    machine,
                },
                &device,
            );
            let joined = ask("remote.declare", json!({"change": declared}))?;
            // Kept only once the machine is pinned with it: a declaration the hub did not take
            // leaves it pending, for the next command to settle by the daemon's list.
            if new {
                keep_new(paths, device)?;
            }
            if json {
                println!("{}", json!({"accepted": accepted, "joined": joined}));
            } else {
                println!(
                    "{} joined the hub at {} as a {}, added by {}",
                    accepted["name"].as_str().unwrap_or_default(),
                    joined["url"].as_str().unwrap_or_default(),
                    accepted["role"].as_str().unwrap_or_default(),
                    accepted["added_by"].as_str().unwrap_or_default(),
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Requests { json } => {
            let requests = ask("remote.requests", json!({}))?;
            if json {
                println!("{requests}");
            } else {
                for request in requests.as_array().into_iter().flatten() {
                    println!(
                        "{}  {}",
                        request["request"].as_str().unwrap_or_default(),
                        words_of(request)
                    );
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Accept {
            request,
            name,
            role,
            json,
        } => {
            let requests = ask("remote.requests", json!({}))?;
            let Some(found) = requests
                .as_array()
                .into_iter()
                .flatten()
                .find(|waiting| waiting["request"] == request.as_str())
                .cloned()
            else {
                return Err(format!(
                    "no join request of {request} waits for this machine: flottille remote \
                     requests lists them"
                )
                .into());
            };
            if !json {
                println!("Verification words: {}", words_of(&found));
            }
            let device = device_key(paths)?;
            let key: flottille_link::PublicKey = serde_json::from_value(found["key"].clone())?;
            let members = ask("remote.members", json!({}))?;
            let added = signed_after(
                &members,
                flottille_link::Action::Add {
                    key,
                    name: name.clone(),
                    role: role.role(),
                },
                &device,
            )?;
            let members = ask("remote.change", json!({"change": added}))?;
            if json {
                println!("{members}");
            } else {
                println!("{name} was added, its entry signed by this device");
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Refuse { request, json } => {
            let refused = ask("remote.refuse", json!({"request": request}))?;
            if json {
                println!("{refused}");
            } else {
                println!("The join request of {request} was refused");
            }
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
        RemoteAction::Remove { name, json } => {
            let device = device_key(paths)?;
            let members = ask("remote.members", json!({}))?;
            let key = member_key(&members, &name)?;
            let removal = signed_after(&members, flottille_link::Action::Remove { key }, &device)?;
            let members = ask("remote.change", json!({"change": removal}))?;
            if json {
                println!("{members}");
            } else {
                println!("{name} was removed: its links are cut, and its key never joins again");
            }
            return Ok(ExitCode::SUCCESS);
        }
        RemoteAction::Setup {
            url,
            name,
            replace,
            json,
        } => {
            let name = match name {
                Some(name) => name,
                None => host_name()?,
            };
            let code = read_code("The hub's setup code: ", "no setup code")?;
            settle_pending(paths, &mut ask)?;
            let (device, new) = device_key_to_join(paths)?;
            let machine: flottille_link::PublicKey =
                serde_json::from_value(ask("remote.machines", json!({}))?["this"]["key"].clone())?;
            let mut list = flottille_link::MemberList::start(&name, &device)?;
            // Claimed by the machine's daemon, which holds its key.
            list.sign(
                flottille_link::Action::Machine {
                    key: device.public(),
                    machine,
                },
                &device,
            );
            let set = ask(
                "remote.setup",
                json!({
                    "url": url,
                    "code": code.as_str(),
                    "changes": list.changes(),
                    "replace": replace,
                }),
            )?;
            if new {
                keep_new(paths, device)?;
            }
            if json {
                println!("{set}");
            } else {
                println!(
                    "{name} is the first member of the hub at {}, whose key is {}: the root every \
                     device that joins pins",
                    set["url"].as_str().unwrap_or_default(),
                    set["hub"].as_str().unwrap_or_default()
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

/// The four words of a join request, as `remote.requests` says them.
fn words_of(request: &Value) -> String {
    request["words"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ")
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

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// A code (the hub's setup code, an invitation's), from the standard input: asked for when it is
/// a terminal. Never an argument, which other users of the machine and the shell's history would
/// see.
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
/// none, `remote/device.key`. Only a setup or a join that succeeded keeps one.
pub(crate) fn device_key(paths: &Paths) -> Result<flottille_link::DeviceKey> {
    kept_device_key(paths)?.ok_or_else(|| {
        "this device holds no key of a member: flottille remote setup, or flottille remote join"
            .into()
    })
}

fn kept_device_key(paths: &Paths) -> Result<Option<flottille_link::DeviceKey>> {
    with_keyring(paths, |keyring, folder| {
        flottille_link::DeviceKey::load(keyring, folder)
    })
}

/// The device key a setup or a join is made with: the one kept, or a new one, kept as pending
/// before the hub sees it and kept only once the hub took it (`keep_new`). A join that did not
/// succeed leaves no key kept, so the next one sends a key the hub never saw, whose words it could
/// not have searched a token for (`docs/remote.md`, "Pairing").
fn device_key_to_join(paths: &Paths) -> Result<(flottille_link::DeviceKey, bool)> {
    Ok(match kept_device_key(paths)? {
        Some(key) => (key, false),
        None => {
            let key = flottille_link::DeviceKey::make()?;
            let public = key.public();
            let pending = with_keyring(paths, |keyring, folder| key.keep_pending(keyring, folder))?;
            if pending.public() != public {
                return Err(ANOTHER_CLIENT.into());
            }
            (pending, true)
        }
    })
}

const ANOTHER_CLIENT: &str = "another client kept a device key at the same moment: run flottille \
                              remote members to see which one is a member";

/// The pending key a setup or a join left when its command was gone before it ended (`keep_new`
/// was never reached): kept when the list of the hub this machine pinned holds it as a member,
/// removed otherwise, so that a key the hub saw in a join or a setup that did not succeed is never
/// sent again. When the daemon holds a join accepted for it, that join's answer and the key, still
/// pending until its declaration is taken. A setup or a join the daemon still runs is waited for.
fn settle_pending(
    paths: &Paths,
    ask: &mut impl FnMut(&str, Value) -> Result<Value>,
) -> Result<Option<(Value, flottille_link::DeviceKey)>> {
    let Some(pending) = with_keyring(paths, |keyring, folder| {
        flottille_link::DeviceKey::load_pending(keyring, folder)
    })?
    else {
        return Ok(None);
    };
    let key = pending.public();
    // Also when pinned: a setup that replaces the hub is waited for the same way, and the join of
    // a machine pinned cannot hold an accepted device. A member only of a join the daemon
    // accepted has its declaration still to come.
    eprintln!(
        "A setup or a join of this device whose command was gone is looked for: a join \
         waits for the member who invited it to accept or refuse it, and goes on with the \
         code it was given, not the one typed now..."
    );
    let accepted = ask("remote.join", json!({"key": key, "resume": true}))?;
    if !accepted.is_null() {
        return Ok(Some((accepted, pending)));
    }
    // Pinned now, before or by a setup the daemon ended meanwhile: its list says.
    let listed = !ask("remote.machines", json!({}))?["hub"].is_null()
        && ask("remote.members", json!({}))?["members"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|member| member["key"] == json!(key));
    if listed {
        keep_new(paths, pending)?;
    } else {
        with_keyring(paths, |keyring, folder| {
            flottille_link::DeviceKey::discard_pending(keyring, folder)
        })?;
    }
    Ok(None)
}

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

/// Keeps `key`, the pending key a setup or a join succeeded with.
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
fn no_service(error: &oo7::Error) -> bool {
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
        zbus::Error::InputOutput(error) => matches!(
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

/// The host's name, as a device's default name.
fn host_name() -> Result<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is as long as said; gethostname writes within it.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return Err("the host has no name: give one with --name".into());
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    let name = String::from_utf8_lossy(&buffer[..end]).into_owned();
    if name.is_empty() {
        return Err("the host has no name: give one with --name".into());
    }
    Ok(name)
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
