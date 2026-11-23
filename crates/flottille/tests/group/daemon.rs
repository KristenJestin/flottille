//! The test binary `group_daemon`: the daemon, its protocol, the commands and the release and CI files.
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_daemon <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;

#[path = "../android_apk.rs"]
mod android_apk;
#[path = "../appearance.rs"]
mod appearance;
#[path = "../commands.rs"]
mod commands;
#[path = "../commit_message.rs"]
mod commit_message;
#[path = "../daemon.rs"]
mod daemon;
#[path = "../daemon_at_rest.rs"]
mod daemon_at_rest;
#[path = "../desktop_entry.rs"]
mod desktop_entry;
#[path = "../developer.rs"]
mod developer;
#[path = "../developer_reports.rs"]
mod developer_reports;
#[path = "../fake_records.rs"]
mod fake_records;
#[path = "../families.rs"]
mod families;
#[path = "../images.rs"]
mod images;
#[path = "../no_leftovers.rs"]
mod no_leftovers;
#[path = "../notifications.rs"]
mod notifications;
#[path = "../programs.rs"]
mod programs;
#[path = "../protocol.rs"]
mod protocol;
#[path = "../read_commands.rs"]
mod read_commands;
#[path = "../read_images.rs"]
mod read_images;
#[path = "../release.rs"]
mod release;
#[path = "../show_link.rs"]
mod show_link;
#[path = "../stream_load.rs"]
mod stream_load;
#[path = "../subscriptions.rs"]
mod subscriptions;
#[path = "../tool_list_cost.rs"]
mod tool_list_cost;
#[path = "../update_staged.rs"]
mod update_staged;
#[path = "../whats_new.rs"]
mod whats_new;
#[path = "../window_parity.rs"]
mod window_parity;
