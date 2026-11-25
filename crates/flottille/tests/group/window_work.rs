//! The test binary `group_window_work`: the window: items, specs, reviews, commands and the states at rest.
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_window_work <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;
#[path = "../guard/mod.rs"]
mod guard;
#[path = "../kit_common/mod.rs"]
mod kit_common;
#[path = "../motion_gate/mod.rs"]
mod motion_gate;

#[path = "../window_at_rest.rs"]
mod window_at_rest;
#[path = "../window_commands.rs"]
mod window_commands;
#[path = "../window_compaction.rs"]
mod window_compaction;
#[path = "../window_exceptions.rs"]
mod window_exceptions;
#[path = "../window_expired_login.rs"]
mod window_expired_login;
#[path = "../window_items.rs"]
mod window_items;
#[path = "../window_kept_state.rs"]
mod window_kept_state;
#[path = "../window_leaves.rs"]
mod window_leaves;
#[path = "../window_led.rs"]
mod window_led;
#[path = "../window_motion_gate.rs"]
mod window_motion_gate;
#[path = "../window_new_session.rs"]
mod window_new_session;
#[path = "../window_polish.rs"]
mod window_polish;
#[path = "../window_progress.rs"]
mod window_progress;
#[path = "../window_quota.rs"]
mod window_quota;
#[path = "../window_review.rs"]
mod window_review;
#[path = "../window_skeletons.rs"]
mod window_skeletons;
#[path = "../window_specs.rs"]
mod window_specs;
#[path = "../window_start_mode.rs"]
mod window_start_mode;
#[path = "../window_states.rs"]
mod window_states;
#[path = "../window_subscriptions.rs"]
mod window_subscriptions;
#[path = "../window_wake.rs"]
mod window_wake;
#[path = "../window_worklog_entry.rs"]
mod window_worklog_entry;
#[path = "../window_workspace_facts.rs"]
mod window_workspace_facts;
