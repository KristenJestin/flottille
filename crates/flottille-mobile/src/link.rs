//! The route to a runner: the one function that decides where the phone dials.
//!
//! Every link goes through the hub, over the session the endpoint was given
//! (`Endpoint::through`), as a daemon's links do (`docs/remote.md`, "The transport"). The link
//! checks the runner's key against the one the signed member list holds for it, so the hub is
//! carried through, never believed.

use flottille_link::{Endpoint, Link, LinkError, PublicKey, Traffic};

/// A client link to the runner whose machine key is `machine`, named `name`, through the hub if
/// the runner is online there.
pub(crate) async fn dial(
    endpoint: &Endpoint,
    machine: PublicKey,
    name: &str,
) -> Result<Link, LinkError> {
    endpoint
        .connect(machine, Traffic::Client, &crate::hello(), name)
        .await
}
