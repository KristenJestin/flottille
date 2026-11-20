//! Flottille's mobile core (`docs/remote.md`, "The Android app"): what the phone holds and does in
//! Rust, behind a coarse boundary the Kotlin app reaches through UniFFI.
//!
//! - the device key, sealed by a key the app holds in the Android Keystore (`Sealer`), and the
//!   key of the phone's endpoint;
//! - pairing with the hub's pair code (`Core::join`), and the member list checked back to the
//!   hub's key and its root;
//! - the control session to the hub (presence), a link to each runner, requests signed with the
//!   device key, and each runner's events resumed after the last one seen;
//! - a view state computed here (`ViewState`), handed with each batch of events (`Listener`);
//! - a panic hook that keeps the message for the app's next start (`keep_panics`).

uniffi::setup_scaffolding!();

mod join;
mod keys;
mod link;
mod mobile;
mod panic;
mod view;

pub use keys::Sealer;
pub use mobile::{Batch, Core, Event, Listener};
pub use panic::{PANIC_FILE, keep_panics, last_panic};
pub use view::{
    AskView, ConversationView, Entry, MachineView, ProjectView, RequestView, SessionView, ViewState,
};

/// Why the core could not do what it was asked: a sentence for the user.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
pub enum MobileError {
    Refused { why: String },
}

impl MobileError {
    pub(crate) fn new(why: impl Into<String>) -> MobileError {
        MobileError::Refused { why: why.into() }
    }
}

impl std::fmt::Display for MobileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MobileError::Refused { why } => f.write_str(why),
        }
    }
}

impl std::error::Error for MobileError {}

/// What the core says of itself to the hub and to each runner: Flottille's version, the protocols
/// it speaks.
pub(crate) fn hello() -> flottille_link::Hello {
    flottille_link::Hello::ours(env!("CARGO_PKG_VERSION"))
}
