//! The test binary `group_agents`: the agents Flottille runs: ACP, the providers, the gate and the tools an agent is given.
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_agents <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;

#[path = "../about.rs"]
mod about;
#[path = "../agent_actions.rs"]
mod agent_actions;
#[path = "../agent_runs_on.rs"]
mod agent_runs_on;
#[path = "../agents.rs"]
mod agents;
#[path = "../approvals.rs"]
mod approvals;
#[path = "../bridge.rs"]
mod bridge;
#[path = "../claude.rs"]
mod claude;
#[path = "../claude_import.rs"]
mod claude_import;
#[path = "../codex.rs"]
mod codex;
#[path = "../codex_bridge.rs"]
mod codex_bridge;
#[path = "../expired_login.rs"]
mod expired_login;
#[path = "../full_mode.rs"]
mod full_mode;
#[path = "../judge.rs"]
mod judge;
#[path = "../models_left.rs"]
mod models_left;
#[path = "../models_listing.rs"]
mod models_listing;
#[path = "../providers.rs"]
mod providers;
#[path = "../quota.rs"]
mod quota;
#[path = "../reload_tools.rs"]
mod reload_tools;
#[path = "../rules.rs"]
mod rules;
#[path = "../tool_results.rs"]
mod tool_results;
#[path = "../turn_usage.rs"]
mod turn_usage;
#[path = "../unchanged_file.rs"]
mod unchanged_file;
