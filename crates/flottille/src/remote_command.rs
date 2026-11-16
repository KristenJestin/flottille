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
            name,
            replace,
            json,
        } => {
            let name = match name {
                Some(name) => name,
                None => host_name()?,
            };
            let code = setup_code()?;
            let device = device_key(paths)?;
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

/// The setup code, from the standard input: asked for when it is a terminal. Never an argument,
/// which other users of the machine and the shell's history would see.
fn setup_code() -> Result<zeroize::Zeroizing<String>> {
    use std::io::{BufRead, IsTerminal, Write};
    if std::io::stdin().is_terminal() {
        eprint!("The hub's setup code: ");
        std::io::stderr().flush()?;
    }
    let mut code = zeroize::Zeroizing::new(String::new());
    std::io::stdin().lock().read_line(&mut code)?;
    let trimmed = zeroize::Zeroizing::new(code.trim().to_owned());
    if trimmed.is_empty() {
        return Err("no setup code was given: type it, or give it on the standard input".into());
    }
    Ok(trimmed)
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
