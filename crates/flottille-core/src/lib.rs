//! Flottille's domain and store, shared by the daemon and its clients.

pub mod agent;
pub mod ansi;
pub mod approval;
pub mod claude_folder;
pub mod command;
pub mod commands;
pub mod declared;
pub mod detect;
pub mod error;
pub mod glob;
pub mod images;
pub mod item;
pub mod paths;
pub mod place;
pub mod project;
pub mod prompt;
pub mod protocol;
pub mod provider;
pub mod question;
pub mod registry;
pub mod run;
pub mod secrets;
pub mod service;
pub mod shown;
pub mod store;
pub mod subscription;
pub mod ticket;
pub mod variables;
pub mod vault;
pub mod workspace;

pub use error::{Error, Result};
