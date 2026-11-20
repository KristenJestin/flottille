//! The test binary `group_tickets`: tickets, specs and the projects they belong to.
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_tickets <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;

#[path = "../project_command.rs"]
mod project_command;
#[path = "../projects.rs"]
mod projects;
#[path = "../skills.rs"]
mod skills;
#[path = "../spec_amendments.rs"]
mod spec_amendments;
#[path = "../spec_development.rs"]
mod spec_development;
#[path = "../spec_tickets.rs"]
mod spec_tickets;
#[path = "../spec_writing.rs"]
mod spec_writing;
#[path = "../specs.rs"]
mod specs;
#[path = "../ticket_edit_rest.rs"]
mod ticket_edit_rest;
#[path = "../ticket_fields.rs"]
mod ticket_fields;
#[path = "../ticket_jira_replies.rs"]
mod ticket_jira_replies;
#[path = "../ticket_links.rs"]
mod ticket_links;
#[path = "../ticket_milestones.rs"]
mod ticket_milestones;
#[path = "../ticket_versions.rs"]
mod ticket_versions;
#[path = "../ticket_worklog.rs"]
mod ticket_worklog;
#[path = "../tickets.rs"]
mod tickets;
