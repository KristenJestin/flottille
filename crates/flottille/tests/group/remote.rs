//! The test binary `group_remote`: connections, the vault, MCP and everything that reaches another machine.
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_remote <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;
#[path = "../remote/mod.rs"]
mod remote;

#[path = "../connection_requests.rs"]
mod connection_requests;
#[path = "../connection_secrets.rs"]
mod connection_secrets;
#[path = "../connection_share.rs"]
mod connection_share;
#[path = "../connections.rs"]
mod connections;
#[path = "../mcp.rs"]
mod mcp;
#[path = "../remote_asks.rs"]
mod remote_asks;
#[path = "../remote_child.rs"]
mod remote_child;
#[path = "../remote_code.rs"]
mod remote_code;
#[path = "../remote_deliveries.rs"]
mod remote_deliveries;
#[path = "../remote_files.rs"]
mod remote_files;
#[path = "../remote_hub.rs"]
mod remote_hub;
#[path = "../remote_hub_only.rs"]
mod remote_hub_only;
#[path = "../remote_keys.rs"]
mod remote_keys;
#[path = "../remote_launch.rs"]
mod remote_launch;
#[path = "../remote_link.rs"]
mod remote_link;
#[path = "../remote_project_travels.rs"]
mod remote_project_travels;
#[path = "../remote_relay.rs"]
mod remote_relay;
#[path = "../remote_requests.rs"]
mod remote_requests;
#[path = "../remote_session_tools.rs"]
mod remote_session_tools;
#[path = "../remote_sessions.rs"]
mod remote_sessions;
#[path = "../remote_settings.rs"]
mod remote_settings;
#[path = "../remote_tunnels.rs"]
mod remote_tunnels;
#[path = "../remote_uploads.rs"]
mod remote_uploads;
#[path = "../ssh_agent.rs"]
mod ssh_agent;
#[path = "../ssh_files.rs"]
mod ssh_files;
#[path = "../vault.rs"]
mod vault;
#[path = "../vault_ask.rs"]
mod vault_ask;
#[path = "../vault_share.rs"]
mod vault_share;
#[path = "../web.rs"]
mod web;
#[path = "../web_entries.rs"]
mod web_entries;
