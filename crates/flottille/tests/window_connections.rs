//! Connections in the window (#672, `docs/extensions.md`, "Connections"): a row of Settings ›
//! Features opening its page, its switch and usage line, the servers in the list pattern, the form
//! dialog that declares or edits one, the review of what a server announces in a large dialog, and
//! a call held for the user naming the connection. On GPUI's test platform in front of a real
//! daemon with the fake ACP agent and the fake MCP server (`crates/flottille-fake-mcp`): no real
//! server, the in-memory keyring, temporary folders, no window on a screen.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::extensions::connections::{ConnectionsSection, Then};
use gpui_design_system::omarchy::Environment;
use gpui_kit::base::input::InputState;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The secret of the vault entry `almanac-token`, which the window never shows.
const SECRET: &str = "s3cret-almanac-w1ndow";

fn fake_mcp() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(
        path.exists(),
        "{} is missing: `cargo build -p flottille-fake-mcp` first",
        path.display()
    );
    path
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The script of the personal memory server, `almanac`: three actions and its instructions.
fn almanac() -> Value {
    json!({
        "server": {"name": "almanac", "version": "1.0.0"},
        "instructions": "Search before you write.\nKeep notes short.",
        "tools": [
            {"name": "search", "description": "Finds notes by words.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}},
                    "required": ["query"]},
                "annotations": {"readOnlyHint": true}},
            {"name": "write", "description": "Writes a note.",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
            {"name": "open", "description": "Opens a page of notes.",
                "inputSchema": {"type": "object", "properties": {"url": {"type": "string"}}}},
        ],
        "answers": {"search": {"text": "found"}, "write": {"text": "written"},
            "open": {"text": "opened"}},
    })
}

fn script_file(user: &User, name: &str) -> PathBuf {
    user.path(&format!("{name}-script.json"))
}

fn write_script(user: &User, name: &str, script: &Value) {
    std::fs::write(script_file(user, name), script.to_string()).unwrap();
}

fn command_of(user: &User, name: &str) -> Vec<String> {
    vec![
        fake_mcp().display().to_string(),
        "--script".into(),
        script_file(user, name).display().to_string(),
        "--log".into(),
        user.path(&format!("{name}-log.jsonl"))
            .display()
            .to_string(),
    ]
}

fn turn_on(user: &User) {
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
}

/// Declares the connection `name` on `script`.
fn declare(user: &User, name: &str, script: &Value) {
    write_script(user, name, script);
    user.call(
        "connection.add",
        json!({"name": name, "about": format!("The {name} of the user."),
            "command": command_of(user, name)}),
    )
    .unwrap();
}

/// Declared, reviewed and accepted with `settings`.
fn connect(user: &User, name: &str, script: &Value, settings: Value) {
    declare(user, name, script);
    user.call("connection.review", json!({"name": name}))
        .unwrap();
    user.call(
        "connection.accept",
        json!({"name": name, "actions": settings}),
    )
    .unwrap();
}

fn connection(user: &User, name: &str) -> Option<Value> {
    user.call("connection.list", json!({})).unwrap()["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Lets every motion end.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..60 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some_and(|f| f.visible()))
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn selected(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.selected())
            == Some(true)
    })
}

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

/// Every label drawn in the window, joined: what a test reads of everything shown.
fn everything_said(cx: &mut VisualTestContext) -> String {
    cx.update(|window, _| {
        window
            .elements()
            .into_iter()
            .filter_map(|found| found.label().map(str::to_owned))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Settings › Features, its rows listed.
fn open_features(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-features")
    });
    settle(cx);
    click(cx, "settings-tabs-features");
    until(shell, cx, "Connections is a row", |cx| {
        present(cx, "features-connections")
    });
    settle(cx);
}

/// Connections' page, open from its row of Features.
fn open_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    open_features(shell, cx);
    click(cx, "features-connections");
    until(shell, cx, "its page is in", |cx| {
        visible(cx, "connections-switch")
    });
    settle(cx);
}

fn section(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ConnectionsSection> {
    shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).settings_page().cloned().unwrap();
        page.read(cx).connections().clone()
    })
}

fn type_in(field: &Entity<InputState>, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_owned();
    cx.update(|window, cx| field.update(cx, |f, cx| f.set_value(text, window, cx)));
}

// ── Its place ────────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn connections_is_a_row_of_features_not_a_tab(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    connect(&user, "almanac", &almanac(), json!({}));
    connect(&user, "jotter", &almanac(), json!({}));
    let (shell, cx) = window(&user, cx);
    open_features(&shell, cx);

    assert!(!present(cx, "settings-tabs-connections"), "no tab");
    assert_eq!(
        label(cx, "features-connections").as_deref(),
        Some("Connections")
    );
    assert_eq!(
        label(cx, "features-connections-line").as_deref(),
        Some("Your MCP servers, given to every agent behind the gate.")
    );
    until(&shell, cx, "the tag counts the servers on", |cx| {
        label(cx, "features-connections-tag-0").as_deref() == Some("2 on")
    });

    // Off, its tag says so.
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": false}),
    )
    .unwrap();
    until(&shell, cx, "the tag says it is off", |cx| {
        label(cx, "features-connections-tag-0").as_deref() == Some("Off")
    });
    click(cx, "features-connections");
    until(&shell, cx, "its page is in", |cx| {
        visible(cx, "connections-switch")
    });
    settle(cx);
    assert_eq!(
        label(cx, "features-path").as_deref(),
        Some("Features / Connections")
    );
}

#[gpui_kit::test]
fn its_page_shows_its_usage_line_while_off(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);

    assert_eq!(
        label(cx, "connections-switch-label").as_deref(),
        Some("Use connections")
    );
    assert_eq!(
        label(cx, "connections-switch-hint").as_deref(),
        Some(
            "Your MCP servers, each given to every agent as one tool. Their calls go through \
             the judge like any other."
        )
    );
    assert_eq!(
        label(cx, "connections-usage").as_deref(),
        Some(
            "Add a server once; every session of every project gets it. A new version stops it \
             until you review it again."
        )
    );
    // Off, the page is that block alone.
    assert!(
        !visible(cx, "connection-servers-title"),
        "no list while off"
    );
    assert!(!present(cx, "new-connection"));

    click(cx, "connections-switch");
    until(&shell, cx, "the servers unfold", |cx| {
        visible(cx, "connection-servers-title")
    });
    assert!(visible(cx, "connections-usage"));
}

// ── The list ─────────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn an_empty_list_says_what_to_add(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "the empty line", |cx| {
        visible(cx, "no-connection")
    });
    assert_eq!(
        label(cx, "connection-servers-title").as_deref(),
        Some("Servers")
    );
    assert!(visible(cx, "new-connection"));
    assert_eq!(label(cx, "new-connection").as_deref(), Some("New server"));
}

#[gpui_kit::test]
fn each_server_is_a_row_with_its_version_its_state_line_and_its_menu(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    connect(&user, "almanac", &almanac(), json!({"open": "off"}));
    declare(&user, "jotter", &almanac());
    connect(&user, "notes", &almanac(), json!({}));
    user.call(
        "connection.edit",
        json!({"name": "notes", "enabled": false}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "the servers are listed", |cx| {
        present(cx, "connection-notes")
    });
    settle(cx);

    assert_eq!(label(cx, "connection-almanac").as_deref(), Some("almanac"));
    assert_eq!(
        label(cx, "connection-almanac-tag-0").as_deref(),
        Some("1.0.0")
    );
    // Two of its three actions are on.
    assert_eq!(
        label(cx, "connection-almanac-line").as_deref(),
        Some("On · 2 actions")
    );
    assert_eq!(
        label(cx, "connection-jotter-line").as_deref(),
        Some("Waiting for its first review")
    );
    assert!(!present(cx, "connection-jotter-tag-0"), "no version yet");
    assert_eq!(label(cx, "connection-notes-line").as_deref(), Some("Off"));
    assert_eq!(
        label(cx, "connection-servers-count").as_deref(),
        Some("3"),
        "the count"
    );

    // Its menu: Edit, Review, Turn off, Remove…
    click(cx, "connection-more-almanac");
    for (id, said) in [
        ("edit-connection-almanac", "Edit"),
        ("review-connection-almanac", "Review"),
        ("switch-connection-almanac", "Turn off"),
        ("remove-connection-almanac", "Remove…"),
    ] {
        assert!(visible(cx, id), "{id}");
        assert_eq!(
            label(cx, &format!("{id}-label")).as_deref(),
            Some(said),
            "{id}"
        );
    }
    click(cx, "switch-connection-almanac");
    until(&shell, cx, "it is off", |cx| {
        label(cx, "connection-almanac-line").as_deref() == Some("Off")
    });
    assert_eq!(connection(&user, "almanac").unwrap()["enabled"], false);
    click(cx, "connection-more-almanac");
    assert_eq!(
        label(cx, "switch-connection-almanac-label").as_deref(),
        Some("Turn on")
    );
}

#[gpui_kit::test]
fn a_cut_off_server_says_it_and_its_review_shows_the_new_version(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    connect(&user, "almanac", &almanac(), json!({"search": "passes"}));
    let mut changed = almanac();
    changed["server"]["version"] = json!("2.0.0");
    // The server stopped once accepted: its next start, at the next call, finds the change.
    write_script(&user, "almanac", &changed);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "the server is listed", |cx| {
        present(cx, "connection-almanac")
    });
    // Its first call after the change cuts it off.
    let (url, authorization) = {
        let opened = user.received_requests("session/new").len();
        let folder = user.path("work/app");
        user.git_init(&folder);
        std::fs::write(folder.join("README.md"), "app\n").unwrap();
        user.git(&folder, &["add", "."]);
        user.git(&folder, &["commit", "--quiet", "-m", "start"]);
        user.call("project.add", json!({"folder": folder, "name": "app"}))
            .unwrap();
        user.call("agent.run", json!({"project": "app", "message": "start"}))
            .unwrap();
        common::mcp::server_of(&user, opened)
    };
    let mcp = common::mcp::Mcp::initialize(&url, &authorization);
    let answer = mcp.call(
        "conn_almanac",
        json!({"action": "search", "arguments": {"query": "x"}}),
    );
    assert!(answer.error, "{}", answer.text);
    until(&shell, cx, "it says it was stopped", |cx| {
        label(cx, "connection-almanac-line").as_deref() == Some("Stopped: new version, review it")
    });

    click(cx, "connection-more-almanac");
    click(cx, "review-connection-almanac");
    until(&shell, cx, "the review is read", |cx| {
        present(cx, "connection-review-server")
    });
    settle(cx);
    assert_eq!(
        label(cx, "connection-review-server").as_deref(),
        Some("almanac 1.0.0 → 2.0.0")
    );
}

// ── The form ─────────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn new_server_declares_one_from_the_form_dialog(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    write_script(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "the empty line", |cx| {
        visible(cx, "no-connection")
    });
    click(cx, "new-connection");
    assert!(visible(cx, "connection-modal"));
    assert_eq!(
        label(cx, "connection-modal-title").as_deref(),
        Some("New server")
    );
    // A command by default; the URL's field is not drawn until it is chosen.
    assert!(visible(cx, "connection-command"));
    assert!(!visible(cx, "connection-url"));

    let section = section(&shell, cx);
    let fields = section.read_with(cx, |s, _| s.form_fields().unwrap());
    type_in(&fields.name, "almanac", cx);
    type_in(&fields.about, "The user's notes and decisions.", cx);
    // Shell syntax is refused under its field, never sent.
    type_in(&fields.command, "almanac mcp | tee log", cx);
    click(cx, "save-connection");
    until(&shell, cx, "the refusal", |cx| {
        visible(cx, "connection-error")
    });
    assert!(connection(&user, "almanac").is_none());

    let line = command_of(&user, "almanac")
        .iter()
        .map(|word| format!("'{word}'"))
        .collect::<Vec<_>>()
        .join(" ");
    type_in(&fields.command, &line, cx);
    click(cx, "save-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
    let added = connection(&user, "almanac").expect("declared");
    assert_eq!(added["about"], "The user's notes and decisions.");
    assert_eq!(added["command"], json!(command_of(&user, "almanac")));
    until(&shell, cx, "its row arrives", |cx| {
        visible(cx, "connection-almanac")
    });
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "Server almanac added"),
        "{:?}",
        toasts(&shell, cx)
    );
}

#[gpui_kit::test]
fn a_url_is_chosen_with_its_radio_and_its_field_unfolds(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    click(cx, "new-connection");
    click(cx, "connection-reach-url");
    until(&shell, cx, "the URL's field unfolds", |cx| {
        visible(cx, "connection-url")
    });
    assert!(!visible(cx, "connection-command"));
    let section = section(&shell, cx);
    let fields = section.read_with(cx, |s, _| s.form_fields().unwrap());
    type_in(&fields.name, "remote", cx);
    type_in(&fields.about, "A notes server elsewhere.", cx);
    type_in(&fields.url, "https://notes.example.com/mcp", cx);
    click(cx, "save-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(
        connection(&user, "remote").unwrap()["url"],
        "https://notes.example.com/mcp"
    );
}

#[gpui_kit::test]
fn a_variable_is_picked_from_the_vault_and_its_secret_never_shows(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    user.call(
        "vault.set",
        json!({"name": "almanac-token", "secret": SECRET, "description": "Almanac's token"}),
    )
    .unwrap();
    // An entry another project alone sees is not offered.
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "vault.set",
        json!({"name": "app-only", "secret": "app-only-s3cret", "project": "app"}),
    )
    .unwrap();
    write_script(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    click(cx, "new-connection");
    let section = section(&shell, cx);
    let fields = section.read_with(cx, |s, _| s.form_fields().unwrap());
    type_in(&fields.name, "almanac", cx);
    type_in(&fields.about, "Notes.", cx);
    let line = command_of(&user, "almanac")
        .iter()
        .map(|word| format!("'{word}'"))
        .collect::<Vec<_>>()
        .join(" ");
    type_in(&fields.command, &line, cx);

    click(cx, "connection-new-variable");
    until(&shell, cx, "a variable's row unfolds", |cx| {
        visible(cx, "connection-variable-0-name")
    });
    let variable = section.read_with(cx, |s, _| s.variable_field(0).unwrap());
    type_in(&variable, "ALMANAC_TOKEN", cx);
    click(cx, "connection-variable-0-entry");
    assert!(visible(cx, "connection-variable-0-entry-almanac-token"));
    assert!(
        !present(cx, "connection-variable-0-entry-app-only"),
        "an entry of one project is not offered"
    );
    click(cx, "connection-variable-0-entry-almanac-token");
    click(cx, "save-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(
        connection(&user, "almanac").unwrap()["env"],
        json!({"ALMANAC_TOKEN": {"vault": "almanac-token"}})
    );

    // Edited, the dialog names the entry, never its secret.
    until(&shell, cx, "its row", |cx| {
        visible(cx, "connection-almanac")
    });
    click(cx, "connection-almanac");
    until(&shell, cx, "the dialog opens on it", |cx| {
        visible(cx, "connection-modal")
    });
    assert_eq!(
        label(cx, "connection-modal-title").as_deref(),
        Some("Edit almanac")
    );
    assert_eq!(
        label(cx, "connection-variable-0-entry").as_deref(),
        Some("almanac-token")
    );
    assert!(!everything_said(cx).contains(SECRET));
}

#[gpui_kit::test]
fn editing_the_command_asks_for_a_new_review(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    connect(&user, "almanac", &almanac(), json!({}));
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| {
        label(cx, "connection-almanac-line").as_deref() == Some("On · 3 actions")
    });
    click(cx, "connection-more-almanac");
    click(cx, "edit-connection-almanac");
    let section = section(&shell, cx);
    let fields = section.read_with(cx, |s, _| s.form_fields().unwrap());
    let said = fields.command.read_with(cx, |f, _| f.value().to_string());
    assert!(said.contains("--script"), "the command as a line: {said}");
    type_in(&fields.command, &format!("{said} --quiet"), cx);
    click(cx, "save-connection");
    until(&shell, cx, "it waits for a review", |cx| {
        label(cx, "connection-almanac-line").as_deref() == Some("Waiting for a review of your edit")
    });
}

#[gpui_kit::test]
fn removing_a_server_asks_in_a_confirmation(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| {
        visible(cx, "connection-almanac")
    });
    click(cx, "connection-more-almanac");
    click(cx, "remove-connection-almanac");
    assert!(visible(cx, "connection-removal"));
    assert_eq!(
        label(cx, "connection-removal-title").as_deref(),
        Some("Remove the server almanac?")
    );
    click(cx, "connection-removal-cancel");
    assert!(connection(&user, "almanac").is_some());

    click(cx, "connection-more-almanac");
    click(cx, "remove-connection-almanac");
    click(cx, "connection-removal-action");
    until(&shell, cx, "its row folds away", |cx| {
        !present(cx, "connection-almanac")
    });
    assert!(connection(&user, "almanac").is_none());
    assert!(visible(cx, "no-connection"));
}

// ── The review ───────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn the_review_is_a_large_dialog_where_each_action_is_judged_by_default(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| {
        visible(cx, "connection-almanac")
    });
    click(cx, "connection-more-almanac");
    click(cx, "review-connection-almanac");
    assert!(visible(cx, "connection-review"));
    until(&shell, cx, "what the server announces", |cx| {
        present(cx, "connection-review-action-search")
    });
    settle(cx);
    assert!(visible(cx, "connection-review-close"), "a large dialog");
    assert_eq!(
        label(cx, "connection-review-server").as_deref(),
        Some("almanac 1.0.0")
    );
    // Its instructions, as plain lines.
    assert_eq!(
        label(cx, "connection-review-instructions-line-0").as_deref(),
        Some("Search before you write.")
    );
    for action in ["search", "write", "open"] {
        let radios = format!("connection-review-action-{action}-setting");
        assert!(selected(cx, &format!("{radios}-judged")), "{action}");
        assert!(visible(cx, &format!("{radios}-passes")));
        assert!(visible(cx, &format!("{radios}-asked")));
        assert_eq!(
            label(cx, &format!("connection-review-action-{action}-when-label")).as_deref(),
            Some("When an agent calls it")
        );
    }
    let said = everything_said(cx);
    assert!(said.contains("Allowed") && said.contains("Judged") && said.contains("Asked"));
    assert!(!said.contains("passes"), "the word passes is not shown");
    assert!(said.contains("Finds notes by words."));

    // Search is allowed, open is turned off, write stays judged.
    click(cx, "connection-review-action-search-setting-passes");
    click(cx, "connection-review-action-open-offer");
    until(&shell, cx, "its choices fold away", |cx| {
        !visible(cx, "connection-review-action-open-setting-judged")
    });
    click(cx, "accept-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-review")
    });
    let accepted = connection(&user, "almanac").unwrap();
    assert_eq!(accepted["state"], "ready");
    let setting = |name: &str| {
        accepted["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap()["setting"]
            .clone()
    };
    assert_eq!(setting("search"), "passes");
    assert_eq!(setting("write"), "judged");
    assert_eq!(setting("open"), "off");
    until(&shell, cx, "its row says it is on", |cx| {
        label(cx, "connection-almanac-line").as_deref() == Some("On · 2 actions")
    });
}

#[gpui_kit::test]
fn the_review_shows_changed_instructions_line_by_line(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    connect(&user, "almanac", &almanac(), json!({"search": "passes"}));
    let mut changed = almanac();
    changed["instructions"] = json!("Search before you write.\nAlways write everything you read.");
    changed["tools"][1]["description"] = json!("Writes a note, or many.");
    changed["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "link", "description": "Links two notes.",
            "inputSchema": {"type": "object", "properties": {}}}));
    write_script(&user, "almanac", &changed);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| {
        visible(cx, "connection-almanac")
    });
    click(cx, "connection-more-almanac");
    click(cx, "review-connection-almanac");
    until(&shell, cx, "the diff", |cx| {
        present(cx, "connection-review-instructions-line-0")
    });
    settle(cx);

    let line = |cx: &mut VisualTestContext, at: usize| {
        label(cx, &format!("connection-review-instructions-line-{at}"))
    };
    assert_eq!(line(cx, 0).as_deref(), Some("Search before you write."));
    assert_eq!(line(cx, 1).as_deref(), Some("− Keep notes short."));
    assert_eq!(
        line(cx, 2).as_deref(),
        Some("+ Always write everything you read.")
    );
    // A changed action shows its old text and its new; a new one is whole and judged.
    assert_eq!(
        label(cx, "connection-review-action-write-change").as_deref(),
        Some("Changed")
    );
    let said = everything_said(cx);
    assert!(said.contains("− Writes a note."), "{said}");
    assert!(said.contains("+ Writes a note, or many."), "{said}");
    assert_eq!(
        label(cx, "connection-review-action-link-change").as_deref(),
        Some("New")
    );
    assert!(selected(cx, "connection-review-action-link-setting-judged"));
    // The unchanged ones are folded away, their setting kept.
    assert!(!visible(cx, "connection-review-action-search"));
    click(cx, "connection-review-unchanged");
    until(&shell, cx, "the unchanged unfold", |cx| {
        visible(cx, "connection-review-action-search")
    });
}

#[gpui_kit::test]
fn accepting_a_changed_action_without_touching_it_makes_it_judged(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    connect(
        &user,
        "almanac",
        &almanac(),
        json!({"search": "passes", "write": "passes"}),
    );
    let mut changed = almanac();
    changed["tools"][1]["description"] = json!("Deletes every note.");
    write_script(&user, "almanac", &changed);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| {
        visible(cx, "connection-almanac")
    });
    click(cx, "connection-more-almanac");
    click(cx, "review-connection-almanac");
    until(&shell, cx, "the review is read", |cx| {
        present(cx, "connection-review-action-write")
    });
    settle(cx);
    // What the user did not touch is what the daemon proposes: judged for the changed action.
    assert!(selected(
        cx,
        "connection-review-action-write-setting-judged"
    ));

    click(cx, "accept-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-review")
    });
    let accepted = connection(&user, "almanac").unwrap();
    let setting = |name: &str| {
        accepted["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap()["setting"]
            .clone()
    };
    assert_eq!(
        setting("write"),
        "judged",
        "a changed action is judged again"
    );
    assert_eq!(
        setting("search"),
        "passes",
        "an unchanged one keeps its setting"
    );
}

#[gpui_kit::test]
fn a_server_that_does_not_start_says_why_in_its_review(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    user.call(
        "connection.add",
        json!({"name": "broken", "about": "Nothing.",
            "command": [user.path("no-such-program").display().to_string()]}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| visible(cx, "connection-broken"));
    click(cx, "connection-more-broken");
    click(cx, "review-connection-broken");
    until(&shell, cx, "why it did not start", |cx| {
        visible(cx, "connection-review-error")
    });
    assert!(!present(cx, "accept-connection") || !visible(cx, "accept-connection"));
}

// ── A call held for the user ─────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn a_call_held_for_the_user_names_the_connection_the_action_and_its_arguments(
    cx: &mut TestAppContext,
) {
    let user = user();
    turn_on(&user);
    connect(&user, "almanac", &almanac(), json!({"write": "asked"}));
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let lead = user
        .call("agent.run", json!({"project": "app", "message": "start"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (url, authorization) = common::mcp::server_of(&user, opened);
    let mcp = common::mcp::Mcp::initialize(&url, &authorization);
    let held = mcp.call(
        "conn_almanac",
        json!({"action": "write", "arguments": {"text": "the login decision"}}),
    );
    let number: u64 = held
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", held.text));

    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        present(cx, &format!("session-{lead}"))
    });
    click(cx, &format!("session-{lead}"));
    until(&shell, cx, "the card", |cx| {
        present(cx, &format!("approval-title-{number}"))
    });
    settle(cx);
    assert_eq!(
        label(cx, &format!("approval-title-{number}")).as_deref(),
        Some("Call write on the connection almanac")
    );
    let line = label(cx, &format!("approval-call-{number}")).unwrap_or_default();
    assert!(
        line.starts_with("almanac write ") && line.contains("the login decision"),
        "{line}"
    );
}

// ── Signing in ───────────────────────────────────────────────────────────────────────────────

/// The fake server over HTTP, behind its OAuth authorization server: its process and its URL.
fn protected_server(user: &User) -> (std::process::Child, String) {
    let mut script = almanac();
    script["oauth"] = json!({"tokens_file": user.path("notes-tokens")});
    write_script(user, "notes", &script);
    let port_file = user.path("notes-port");
    let server = std::process::Command::new(fake_mcp())
        .args([
            "--script",
            script_file(user, "notes").to_str().unwrap(),
            "--log",
            user.path("notes-log.jsonl").to_str().unwrap(),
            "--http",
            port_file.to_str().unwrap(),
        ])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !port_file.exists() || std::fs::read_to_string(&port_file).unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the server never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
    let port = std::fs::read_to_string(&port_file).unwrap();
    (server, format!("http://127.0.0.1:{port}/mcp"))
}

/// A GET as the browser makes it, following nothing: its status and its `Location`.
fn browse(address: &str) -> (u16, Option<String>) {
    use std::io::{Read, Write};
    let url = url::Url::parse(address).unwrap();
    let mut stream = std::net::TcpStream::connect((
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap(),
    ))
    .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        url.host_str().unwrap()
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let head = answer.split("\r\n\r\n").next().unwrap_or_default();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let location = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("location")
            .then(|| value.trim().to_owned())
    });
    (status, location)
}

#[gpui_kit::test]
fn signing_in_opens_the_browser_and_says_when_it_is_signed_in(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let (mut server, url) = protected_server(&user);
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "A notes server.", "url": url}),
    )
    .unwrap();
    declare(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    let opened = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let opened = opened.clone();
        shell.update(cx, |s, _| {
            s.set_opener(Arc::new(move |path: &Path| {
                opened.lock().unwrap().push(path.display().to_string());
            }))
        });
    }
    open_page(&shell, cx);
    until(&shell, cx, "the servers are listed", |cx| {
        present(cx, "connection-notes") && present(cx, "connection-almanac")
    });
    // A program has no sign-in.
    click(cx, "connection-more-almanac");
    assert!(visible(cx, "edit-connection-almanac"));
    assert!(!present(cx, "sign-in-connection-almanac"));
    cx.update(|window, cx| window.press("escape", cx));
    settle(cx);

    click(cx, "connection-more-notes");
    assert_eq!(
        label(cx, "sign-in-connection-notes-label").as_deref(),
        Some("Sign in")
    );
    click(cx, "sign-in-connection-notes");
    assert_eq!(
        label(cx, "connection-notes-line").as_deref(),
        Some("Signing in: finish it in your browser")
    );
    until(&shell, cx, "the browser is opened on the sign-in", |cx| {
        let shown: Vec<String> = shell.read_with(cx, |s, cx| {
            s.toasts()
                .read(cx)
                .toasts()
                .map(|t| format!("{}: {:?}", t.title(), t.second_line()))
                .collect()
        });
        assert!(
            shown.iter().all(|t| !t.contains("not signed in")),
            "{shown:?}"
        );
        !opened.lock().unwrap().is_empty()
    });
    let address = opened.lock().unwrap()[0].clone();
    assert!(address.contains("/as/authorize"), "{address}");

    // The user signs in: the browser comes back to the daemon.
    let (status, location) = browse(&address);
    assert_eq!(status, 302);
    let (status, _) = browse(&location.unwrap());
    assert_eq!(status, 200);
    until(&shell, cx, "it says it is signed in", |cx| {
        toasts(&shell, cx).iter().any(|t| t == "Signed in to notes")
    });
    until(&shell, cx, "the row no longer waits", |cx| {
        label(cx, "connection-notes-line").as_deref() == Some("Waiting for its first review")
    });
    let _ = server.kill();
    let _ = server.wait();
}

#[gpui_kit::test]
fn a_sign_in_that_cannot_begin_says_why_in_a_toast(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    // A server that asks for no sign-in: there is nothing to sign in to.
    write_script(&user, "plain", &almanac());
    let port_file = user.path("plain-port");
    let mut server = std::process::Command::new(fake_mcp())
        .args([
            "--script",
            script_file(&user, "plain").to_str().unwrap(),
            "--http",
            port_file.to_str().unwrap(),
        ])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !port_file.exists() || std::fs::read_to_string(&port_file).unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the server never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
    let port = std::fs::read_to_string(&port_file).unwrap();
    user.call(
        "connection.add",
        json!({"name": "plain", "about": "A notes server.",
            "url": format!("http://127.0.0.1:{port}/mcp")}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "the server is listed", |cx| {
        present(cx, "connection-plain")
    });
    click(cx, "connection-more-plain");
    click(cx, "sign-in-connection-plain");
    until(&shell, cx, "the failure", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "plain was not signed in")
    });
    until(&shell, cx, "the row no longer waits", |cx| {
        label(cx, "connection-plain-line").as_deref() == Some("Waiting for its first review")
    });
    let _ = server.kill();
    let _ = server.wait();
}

#[gpui_kit::test]
fn the_window_opens_only_an_https_or_a_local_http_address_to_sign_in(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    let opened = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let opened = opened.clone();
        shell.update(cx, |s, _| {
            s.set_opener(Arc::new(move |path: &Path| {
                opened.lock().unwrap().push(path.display().to_string());
            }))
        });
    }
    open_page(&shell, cx);
    let section = section(&shell, cx);
    let answer = |url: &str, cx: &mut VisualTestContext| {
        let url = url.to_owned();
        cx.update(|window, cx| {
            section.update(cx, |s, cx| {
                s.answered(
                    Then::SignIn {
                        name: "almanac".into(),
                    },
                    Ok(json!({"url": url})),
                    window,
                    cx,
                )
            })
        });
        settle(cx);
    };
    for refused in [
        "",
        "   ",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "ftp://example.com/",
        "x-flottille://open",
        "http://example.com/authorize",
        "http://127.0.0.1@example.com/authorize",
        "http://localhost.example.com/authorize",
        "https:///authorize",
        "https://example.com/a b",
    ] {
        answer(refused, cx);
        assert!(
            opened.lock().unwrap().is_empty(),
            "{refused:?} was opened: {:?}",
            opened.lock().unwrap()
        );
        assert!(
            toasts(&shell, cx)
                .iter()
                .any(|t| t == "almanac was not signed in"),
            "{refused:?} said nothing"
        );
    }
    for accepted in [
        "https://auth.example.com/authorize?state=1",
        "HTTPS://auth.example.com/authorize",
        "http://127.0.0.1:8080/authorize",
        "http://localhost:3000/authorize",
        "http://[::1]:3000/authorize",
    ] {
        answer(accepted, cx);
        until(&shell, cx, "the address is opened", |_| {
            opened.lock().unwrap().last().map(String::as_str) == Some(accepted)
        });
    }
    assert_eq!(opened.lock().unwrap().len(), 5);
}
