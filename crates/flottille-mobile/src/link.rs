//! The route to a runner: the one function that changes when links go through the hub (#1151).
//!
//! On `dev` today a link is direct: the phone dials the addresses the hub told for the runner's
//! machine (`docs/remote.md`, "Paths"), with the transport of `flottille-link`. Once the link's
//! packets travel inside the control session, this function takes that route instead; nothing
//! above it changes (the link, its checks, the requests, the events).

use std::net::SocketAddr;

use flottille_link::{Endpoint, Link, LinkError, PublicKey, Traffic};

/// A client link to the runner whose machine key is `machine`, named `name`, at the first of the
/// addresses `told` for it that answers.
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
