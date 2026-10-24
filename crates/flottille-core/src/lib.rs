//! Flottille's domain and store, shared by the daemon and its clients.

pub mod agent;
pub mod approval;
pub mod command;
pub mod detect;
pub mod error;
pub mod glob;
pub mod paths;
pub mod project;
pub mod protocol;
pub mod registry;
pub mod run;
pub mod secrets;
pub mod store;
pub mod variables;
pub mod workspace;

pub use error::{Error, Result};
