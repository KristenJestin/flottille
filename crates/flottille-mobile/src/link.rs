//! The route to a runner: the one function that decides where the phone dials.
//!
//! The phone listens nowhere and is rarely on the runners' network: a link goes to the addresses
//! the hub told for the runner's machine, in turn, then through the hub over the session the
//! endpoint was given (`Endpoint::through`, #1151), as a daemon's links do (`docs/remote.md`, "The
//! transport"). Either way the link checks the runner's key against the one the signed member
//! list holds for it, so the hub is carried through, never believed.

use std::net::SocketAddr;

use flottille_link::{Endpoint, Link, LinkError, PublicKey, Traffic};

/// A client link to the runner whose machine key is `machine`, named `name`, at the first of the
/// addresses `told` for it that answers, else through the hub if the runner is online there.
pub(crate) async fn dial(
    endpoint: &Endpoint,
    machine: PublicKey,
    told: &[String],
    name: &str,
) -> Result<Link, LinkError> {
    let mut addresses: Vec<SocketAddr> = Vec::new();
    for address in told {
        if let Ok(found) = tokio::net::lookup_host(address.as_str()).await {
            addresses.extend(found);
        }
    }
    endpoint
        .connect(machine, &addresses, Traffic::Client, &crate::hello(), name)
        .await
}
