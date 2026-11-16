//! `flottille remote`: Flottille on the user's other machines (`docs/remote.md`). Here, the
//! addresses this machine may be reached at: those its link listens on, and those the user
//! declares by hand (a VPN address, a DNS name, a forwarded port), which it tells the machines it
//! is linked to.

use std::process::ExitCode;

use clap::Subcommand;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::client::Client;
use crate::project_command::in_full;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum RemoteAction {
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
    let RemoteAction::Address { action } = action;
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
