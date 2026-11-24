//! The test binary `group_window_shell`: the window: its shell (settings, connections, projects, the composer).
//!
//! Each test file stays in `tests/` and is a module of this binary, so a test is named
//! `<file>::<test>` and `cargo test -p flottille --test group_window_shell <file>::` runs one file
//! (`AGENTS.md`, "Adding a test file", says how a new one joins a group).

#[path = "../common/mod.rs"]
mod common;
#[path = "../guard/mod.rs"]
mod guard;
#[path = "../remote/mod.rs"]
mod remote;

#[path = "../window_allow_files.rs"]
mod window_allow_files;
#[path = "../window_any_machine.rs"]
mod window_any_machine;
#[path = "../window_appearance.rs"]
mod window_appearance;
#[path = "../window_centre.rs"]
mod window_centre;
#[path = "../window_clicks.rs"]
mod window_clicks;
#[path = "../window_code_copy.rs"]
mod window_code_copy;
#[path = "../window_composer_tokens.rs"]
mod window_composer_tokens;
#[path = "../window_connection_requests.rs"]
mod window_connection_requests;
#[path = "../window_connection_share.rs"]
mod window_connection_share;
#[path = "../window_connections.rs"]
mod window_connections;
#[path = "../window_developer_reports.rs"]
mod window_developer_reports;
#[path = "../window_drafts.rs"]
mod window_drafts;
#[path = "../window_every_machine.rs"]
mod window_every_machine;
#[path = "../window_first_frame.rs"]
mod window_first_frame;
#[path = "../window_folds.rs"]
mod window_folds;
#[path = "../window_health_check.rs"]
mod window_health_check;
#[path = "../window_instance.rs"]
mod window_instance;
#[path = "../window_link.rs"]
mod window_link;
#[path = "../window_machines.rs"]
mod window_machines;
#[path = "../window_menus.rs"]
mod window_menus;
#[path = "../window_notifications.rs"]
mod window_notifications;
#[path = "../window_one_list.rs"]
mod window_one_list;
#[path = "../window_pasted_text.rs"]
mod window_pasted_text;
#[path = "../window_pictures.rs"]
mod window_pictures;
#[path = "../window_programs.rs"]
mod window_programs;
#[path = "../window_project_images.rs"]
mod window_project_images;
#[path = "../window_project_machines.rs"]
mod window_project_machines;
#[path = "../window_project_order.rs"]
mod window_project_order;
#[path = "../window_project_page.rs"]
mod window_project_page;
#[path = "../window_project_sessions.rs"]
mod window_project_sessions;
#[path = "../window_project_settings.rs"]
mod window_project_settings;
#[path = "../window_project_vault.rs"]
mod window_project_vault;
#[path = "../window_raise.rs"]
mod window_raise;
#[path = "../window_remote_tree.rs"]
mod window_remote_tree;
#[path = "../window_repositories.rs"]
mod window_repositories;
#[path = "../window_runners.rs"]
mod window_runners;
#[path = "../window_selection.rs"]
mod window_selection;
#[path = "../window_services.rs"]
mod window_services;
#[path = "../window_settings.rs"]
mod window_settings;
#[path = "../window_setup_output.rs"]
mod window_setup_output;
#[path = "../window_sidebar.rs"]
mod window_sidebar;
#[path = "../window_sign_in.rs"]
mod window_sign_in;
#[path = "../window_switcher.rs"]
mod window_switcher;
#[path = "../window_tunnels.rs"]
mod window_tunnels;
#[path = "../window_update.rs"]
mod window_update;
#[path = "../window_vault.rs"]
mod window_vault;
#[path = "../window_vault_ask.rs"]
mod window_vault_ask;
#[path = "../window_vault_share.rs"]
mod window_vault_share;
#[path = "../window_whats_new.rs"]
mod window_whats_new;
#[path = "../window_words_whole.rs"]
mod window_words_whole;
#[path = "../windows.rs"]
mod windows;
