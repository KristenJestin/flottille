//! The test binary `group_forges`: forges, pull requests, workspaces and the services a project runs.
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_forges <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;

#[path = "../commit_checks.rs"]
mod commit_checks;
#[path = "../doctor.rs"]
mod doctor;
#[path = "../doctor_forges.rs"]
mod doctor_forges;
#[path = "../forge_ci.rs"]
mod forge_ci;
#[path = "../forge_follow.rs"]
mod forge_follow;
#[path = "../forge_pipelines.rs"]
mod forge_pipelines;
#[path = "../forge_review.rs"]
mod forge_review;
#[path = "../forge_threads.rs"]
mod forge_threads;
#[path = "../forges.rs"]
mod forges;
#[path = "../message_checks.rs"]
mod message_checks;
#[path = "../pr_edit.rs"]
mod pr_edit;
#[path = "../services.rs"]
mod services;
#[path = "../workspace_actions.rs"]
mod workspace_actions;
#[path = "../workspaces.rs"]
mod workspaces;
