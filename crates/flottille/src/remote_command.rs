//! `flottille remote`: Flottille on the user's other machines (`docs/remote.md`). Here, the hub's
//! first member (`setup`), and the addresses this machine may be reached at: those its link
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
    /// Make this machine the first member of a new hub, with the setup code it printed
    Setup {
        /// The hub's URL: http:// or https://, its host and its port
        url: String,
        /// The setup code, from the hub's log or `flottille-hub setup-code`
        code: String,
        /// This device's name among the user's [default: the host's name]
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// This machine's addresses: those its link listens on, and those declared by hand
    Address {
        #[command(subcommand)]
        action: AddressAction,
    },
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
        RemoteAction::Setup {
            url,
            code,
            name,
            json,
        } => {
            let name = match name {
                Some(name) => name,
                None => host_name()?,
            };
            let device = device_key(paths)?;
            let machine: flottille_link::PublicKey =
                serde_json::from_value(ask("remote.machines", json!({}))?["this"]["key"].clone())?;
            let mut list = flottille_link::MemberList::start(&name, &device);
            list.sign(
                flottille_link::Action::Machine {
                    key: device.public(),
                    machine,
                },
                &device,
            );
            let set = ask(
                "remote.setup",
                json!({"url": url, "code": code, "changes": list.changes()}),
            )?;
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

/// The device key of this user session's clients: in the session's keyring, or, with none,
/// `remote/device.key`, made there the first time.
fn device_key(paths: &Paths) -> Result<flottille_link::DeviceKey> {
    let folder = crate::remote::folder(paths);
    let key = if crate::vault::memory_chosen(paths) {
        flottille_link::DeviceKey::load_or_make(&NoKeyring, &folder)
    } else {
        flottille_link::DeviceKey::load_or_make(&SessionKeyring, &folder)
    };
    Ok(key?)
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
            let keyring = oo7::Keyring::new()
                .await
                .map_err(|_| flottille_link::KeyringError::Absent)?;
            act(keyring).await
        })
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
            let Some(item) = items.first() else {
                return Ok(None);
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
