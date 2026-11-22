//! The test binary `group_sessions`: sessions and their tree: starting, waking, leading, following and ending them.
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_sessions <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;

#[path = "../cli_agent.rs"]
mod cli_agent;
#[path = "../cli_follow.rs"]
mod cli_follow;
#[path = "../cli_name.rs"]
mod cli_name;
#[path = "../compaction.rs"]
mod compaction;
#[path = "../conversation.rs"]
mod conversation;
#[path = "../cross_project.rs"]
mod cross_project;
#[path = "../inactive.rs"]
mod inactive;
#[path = "../items.rs"]
mod items;
#[path = "../led_sessions.rs"]
mod led_sessions;
#[path = "../new_session.rs"]
mod new_session;
#[path = "../pilot.rs"]
mod pilot;
#[path = "../questions.rs"]
mod questions;
#[path = "../restart.rs"]
mod restart;
#[path = "../review.rs"]
mod review;
#[path = "../routine_events.rs"]
mod routine_events;
#[path = "../routine_items.rs"]
mod routine_items;
#[path = "../routines.rs"]
mod routines;
#[path = "../runs.rs"]
mod runs;
#[path = "../session_end.rs"]
mod session_end;
#[path = "../session_follow.rs"]
mod session_follow;
#[path = "../session_leave.rs"]
mod session_leave;
#[path = "../session_options.rs"]
mod session_options;
#[path = "../session_read.rs"]
mod session_read;
#[path = "../session_title.rs"]
mod session_title;
#[path = "../session_titles.rs"]
mod session_titles;
#[path = "../task_copy.rs"]
mod task_copy;
#[path = "../tasks.rs"]
mod tasks;
#[path = "../tree.rs"]
mod tree;
#[path = "../unqueue.rs"]
mod unqueue;
#[path = "../wake.rs"]
mod wake;
