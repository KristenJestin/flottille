//! The test binary `group_window_session`: the window: a session's conversation, its cards, questions and sub-agents.
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_window_session <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;
#[path = "../guard/mod.rs"]
mod guard;
#[path = "../kit_common/mod.rs"]
mod kit_common;

#[path = "../window.rs"]
mod window;
#[path = "../window_cards.rs"]
mod window_cards;
#[path = "../window_commit_checks.rs"]
mod window_commit_checks;
#[path = "../window_conversation.rs"]
mod window_conversation;
#[path = "../window_conversation_loading.rs"]
mod window_conversation_loading;
#[path = "../window_ended_section.rs"]
mod window_ended_section;
#[path = "../window_message_checks.rs"]
mod window_message_checks;
#[path = "../window_models.rs"]
mod window_models;
#[path = "../window_new_session_page.rs"]
mod window_new_session_page;
#[path = "../window_questions.rs"]
mod window_questions;
#[path = "../window_runs_on.rs"]
mod window_runs_on;
#[path = "../window_session_options.rs"]
mod window_session_options;
#[path = "../window_session_subject.rs"]
mod window_session_subject;
#[path = "../window_session_titles.rs"]
mod window_session_titles;
#[path = "../window_sessions.rs"]
mod window_sessions;
#[path = "../window_shell_session.rs"]
mod window_shell_session;
#[path = "../window_started_by.rs"]
mod window_started_by;
#[path = "../window_steps.rs"]
mod window_steps;
#[path = "../window_sub_agents.rs"]
mod window_sub_agents;
#[path = "../window_tie.rs"]
mod window_tie;
#[path = "../window_tools.rs"]
mod window_tools;
#[path = "../window_write_spec.rs"]
mod window_write_spec;
