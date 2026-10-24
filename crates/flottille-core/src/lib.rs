//! Flottille's domain and store, shared by the daemon and its clients.

pub mod error;
pub mod paths;
pub mod project;
pub mod protocol;
pub mod registry;
pub mod store;

pub use error::{Error, Result};
