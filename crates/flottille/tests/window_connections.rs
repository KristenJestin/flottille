//! Connections in the window (#672, `docs/extensions.md`, "Connections"): a row of Settings ›
//! Features opening its page, its switch and usage line, the servers in the list pattern, the form
//! dialog that declares or edits one, the review of what a server announces in a dialog of a readable width, and
//! a call held for the user naming the connection. On GPUI's test platform in front of a real
//! daemon with the fake ACP agent and the fake MCP server (`crates/flottille-fake-mcp`): no real
//! server, the in-memory keyring, temporary folders, no window on a screen.

use crate::common;
use crate::guard;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::extensions::connections::{ConnectionsSection, Then};
use gpui_design_system::motion::PANEL;
use gpui_design_system::omarchy::Environment;
use gpui_design_system::space;
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

/// Declared with the extension on while its server cannot start (its script is not written yet):
/// nothing was listed, so it waits for its first review. `write_script` lets it start.
fn declare_waiting(user: &User, name: &str) {
    user.call(
        "connection.add",
        json!({"name": name, "about": format!("The {name} of the user."),
            "command": command_of(user, name)}),
    )
    .unwrap();
    assert_eq!(connection(user, name).unwrap()["state"], "awaiting review");
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

/// The height drawn now of the element `id`, 0 where it is not there: what a fold or an unfold
/// measures. A motion that is instant is either 0 or its whole height, never between.
fn height(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
}

/// A click that leaves the clock where it is, so that a motion it starts is measured as it goes.
fn press(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    cx.run_until_parked();
}

/// A third of a panel's motion later: a motion that moves is neither at its start nor at its end.
fn a_third_later(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.executor().advance_clock(PANEL / 3);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
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

/// A browser's server: fourteen actions, one with a description of two lines and arguments, two
/// that speak of cookies, and instructions of two lines.
fn driftbrowser() -> Value {
    let plain = |name: &str, description: &str| {
        json!({"name": name, "description": description,
            "inputSchema": {"type": "object", "properties": {}}})
    };
    let mut tools = vec![
        plain("browser_open", "Open a URL in the active tab."),
        plain("browser_back", "Go back one page in the active tab."),
        plain("browser_click", "Click an element."),
        plain("browser_type", "Type text into an element, key by key."),
        plain("browser_fill", "Replace the value of a field."),
        plain("browser_press", "Press a key or a chord."),
        plain("browser_snapshot", "Describe the page as a tree of roles."),
        json!({"name": "browser_eval",
            "description": "Evaluate JavaScript in the page.\nThe script can read anything the page can read.",
            "inputSchema": {"type": "object",
                "properties": {"script": {"type": "string", "description": "A function body."}},
                "required": ["script"]},
            "annotations": {"destructiveHint": true}}),
        plain("browser_screenshot", "Save a screenshot as a PNG file."),
        plain("browser_upload", "Attach a file to a file input."),
        plain("browser_cookies", "Read and set the cookies of the page."),
        plain(
            "browser_storage",
            "Read the local storage and the cookie jar of the page.",
        ),
        plain("browser_pdf", "Save the page as a PDF file."),
        plain("browser_download", "Download a file the page links to."),
    ];
    let answers: serde_json::Map<String, Value> = tools
        .iter_mut()
        .map(|tool| {
            (
                tool["name"].as_str().unwrap().to_owned(),
                json!({"text": "ok"}),
            )
        })
        .collect();
    json!({
        "server": {"name": "driftbrowser", "version": "0.9.2"},
        "instructions": "Drives one real browser window.\nCall a snapshot after a page changes.",
        "tools": tools,
        "answers": answers,
    })
}

/// The groups of the review, in the order they are drawn: each setting's word and its index.
const GROUPS: [&str; 4] = ["asked", "passes", "off", "judged"];

/// The ids of the row of the action `name` as drawn now: one, or two while it moves between
/// groups, or the row of "What changed".
fn rows_of(cx: &mut VisualTestContext, name: &str) -> Vec<String> {
    let mut ids: Vec<String> = (0..GROUPS.len())
        .map(|at| format!("connection-review-row-{at}-{name}"))
        .collect();
    ids.push(format!("connection-review-change-{name}"));
    ids.into_iter().filter(|id| present(cx, id)).collect()
}

/// The id of the row of `name`, once it stands in one place.
fn row_of(cx: &mut VisualTestContext, name: &str) -> String {
    let mut ids = rows_of(cx, name);
    assert_eq!(ids.len(), 1, "{name} has one row: {ids:?}");
    ids.remove(0)
}

/// The setting a row sits in: the group it is drawn in, "changes" for "What changed".
fn group_of(cx: &mut VisualTestContext, name: &str) -> Option<&'static str> {
    (0..GROUPS.len())
        .find(|at| present(cx, &format!("connection-review-row-{at}-{name}")))
        .map(|at| GROUPS[at])
        .or_else(|| present(cx, &format!("connection-review-change-{name}")).then_some("changes"))
}

/// The names of the rows drawn now, the ones folding away left out.
fn names_drawn(cx: &mut VisualTestContext, of: &[&str]) -> Vec<String> {
    of.iter()
        .filter(|name| !rows_of(cx, name).is_empty())
        .map(|name| (*name).to_owned())
        .collect()
}

/// What a row's pill says it is set to.
fn pill(cx: &mut VisualTestContext, name: &str) -> Option<String> {
    let id = row_of(cx, name);
    label(cx, &format!("{id}-setting"))
}

/// Sets a row through its pill: opens it, picks the option of the daemon's word `setting`.
fn choose(cx: &mut VisualTestContext, name: &str, setting: &str) {
    let id = row_of(cx, name);
    click(cx, &format!("{id}-setting"));
    click(cx, &format!("{id}-setting-{setting}"));
}

fn open_the_review(shell: &Entity<Shell>, cx: &mut VisualTestContext, name: &str, ready: &str) {
    open_page(shell, cx);
    until(shell, cx, "its row", |cx| {
        visible(cx, &format!("connection-{name}"))
    });
    click(cx, &format!("connection-more-{name}"));
    click(cx, &format!("review-connection-{name}"));
    until(shell, cx, "what the server announces", |cx| {
        present(cx, ready)
    });
    settle(cx);
}

fn search_field(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<InputState> {
    section(shell, cx)
        .read_with(cx, |s, _| s.review_search())
        .expect("the review has a search field")
}

/// What the daemon pinned for the action `name` of `connection`.
fn pinned(user: &User, connection_name: &str, name: &str) -> Value {
    connection(user, connection_name).unwrap()["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not pinned"))["setting"]
        .clone()
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
            "Add a server once; every session of every project gets it. A change in what it \
             gives agents stops it until you review it."
        )
    );
    // Off, the page is that block alone.
    assert!(
        !visible(cx, "connection-servers-title"),
        "no list while off"
    );
    assert!(!present(cx, "new-connection"));

    assert_eq!(height(cx, "connection-servers-block"), 0.);
    press(cx, "connections-switch");
    until(&shell, cx, "the servers begin to unfold", |cx| {
        height(cx, "connection-servers-block") > 0.
    });
    a_third_later(cx);
    let unfolding = height(cx, "connection-servers-block");
    settle(cx);
    let open = height(cx, "connection-servers-block");
    assert!(
        0. < unfolding && unfolding < open,
        "they unfold, over a panel's time: {unfolding} of {open}"
    );
    assert!(visible(cx, "connection-servers-title"));
    assert!(visible(cx, "connections-usage"));
}

#[gpui_kit::test]
fn the_servers_section_keeps_its_gap_under_the_card(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "the servers are in", |cx| {
        visible(cx, "connection-servers-title")
    });
    settle(cx);

    // The section is a block of the page like the others: `space::XXL` under the card. Its
    // heading sits inside the section, centred in the row of the heading, so the block is measured.
    let card = cx.update(|window, _| window.find("connections").bounds());
    let block = cx.update(|window, _| window.find("connection-servers-block").bounds());
    let title = cx.update(|window, _| window.find("connection-servers-title").bounds());
    assert_eq!(block.top() - card.bottom(), space::XXL);
    assert!(title.top() >= block.top(), "the heading is in the block");
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
    // Its server cannot start, so nothing was listed: it waits for its first review.
    turn_on(&user);
    declare_waiting(&user, "jotter");
    connect(&user, "almanac", &almanac(), json!({"open": "off"}));
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
    // Added, it is pinned and its server stopped: no review has left one running.
    declare(&user, "almanac", &almanac());
    user.call(
        "connection.accept",
        json!({"name": "almanac", "actions": {"search": "passes"}}),
    )
    .unwrap();
    let mut changed = almanac();
    changed["server"]["version"] = json!("2.0.0");
    changed["tools"][0]["description"] = json!("Finds notes, and sends them away.");
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
        label(cx, "connection-almanac-line").as_deref()
            == Some("Stopped: new version, the action search changed (its description), review it")
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
    // The daemon gave it to agents at once: the toast says so, every action judged, and its
    // button is the review.
    until(&shell, cx, "the toast says it is given", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "Server almanac is given to agents")
    });
    let said = toasts(&shell, cx);
    assert!(
        said.iter()
            .any(|t| t == "Server almanac is given to agents"),
        "{said:?}"
    );
    assert_eq!(added["state"], "ready");
    assert_eq!(
        label(cx, "connection-almanac-line").as_deref(),
        Some("On · 3 actions")
    );
    let second: Vec<Option<String>> = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|t| t.second_line().map(str::to_owned))
            .collect()
    });
    assert!(
        second.contains(&Some("Every action is judged.".to_owned())),
        "{second:?}"
    );
    assert_eq!(label(cx, "toast-1-action").as_deref(), Some("Review"));
    click(cx, "toast-1-action");
    until(&shell, cx, "the review opens", |cx| {
        present(cx, "connection-review-server")
    });
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
fn a_variable_is_picked_from_the_vault_and_the_dialog_names_its_vault_entry_only(
    cx: &mut TestAppContext,
) {
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
fn every_control_of_the_form_has_a_visible_label(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    click(cx, "new-connection");
    click(cx, "connection-new-variable");
    click(cx, "connection-new-limit");
    until(&shell, cx, "the rows unfold", |cx| {
        visible(cx, "connection-variable-0-name") && visible(cx, "connection-limit-0-argument")
    });
    settle(cx);
    // A control and the label that names it, visible with it.
    let named = [
        ("connection-name", "connection-name-field", "Name"),
        (
            "connection-about",
            "connection-about-field",
            "What it is for",
        ),
        ("connection-command", "connection-command-field", "Command"),
        (
            "connection-variable-0-name",
            "connection-variable-0-name-field",
            "Variable",
        ),
        (
            "connection-variable-0-entry",
            "connection-variable-0-entry-field",
            "Vault entry",
        ),
        (
            "connection-limit-0-argument",
            "connection-limit-0-argument-field",
            "Argument",
        ),
        (
            "connection-limit-0-values",
            "connection-limit-0-values-field",
            "Allowed values",
        ),
    ];
    for (control, field, said) in named {
        assert!(visible(cx, control), "{control} is drawn");
        assert_eq!(
            label(cx, &format!("{field}-label")).as_deref(),
            Some(said),
            "{control}"
        );
        assert!(visible(cx, &format!("{field}-label")), "{field}-label");
    }
    // A URL's requests carry secrets, and say so.
    click(cx, "connection-reach-url");
    until(&shell, cx, "the URL's field unfolds", |cx| {
        visible(cx, "connection-url")
    });
    click(cx, "connection-new-secret");
    until(&shell, cx, "a secret's line unfolds", |cx| {
        visible(cx, "connection-secret-0-name")
    });
    settle(cx);
    assert_eq!(
        label(cx, "connection-secrets-field-label").as_deref(),
        Some("Secrets of its requests")
    );
    for (control, said) in [
        ("place", "Place"),
        ("name", "Name"),
        ("prefix", "Prefix"),
        ("entry", "Vault entry"),
    ] {
        assert!(visible(cx, &format!("connection-secret-0-{control}")));
        assert_eq!(
            label(cx, &format!("connection-secret-0-{control}-field-label")).as_deref(),
            Some(said)
        );
    }
    assert_eq!(
        label(cx, "connection-url-field-label").as_deref(),
        Some("URL")
    );
}

#[gpui_kit::test]
fn the_form_scrolls_once_in_the_dialog_and_its_footer_stays_in_view(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    click(cx, "new-connection");
    click(cx, "connection-new-variable");
    cx.simulate_resize(size(px(1280.), px(480.)));
    settle(cx);

    let bounds = |cx: &mut VisualTestContext, id: &str| {
        let id = id.to_owned();
        cx.update(|window, _| window.find(id).bounds())
    };
    let viewport = cx.update(|window, _| window.viewport_size());
    let (panel, title, form, save) = (
        bounds(cx, "connection-modal"),
        bounds(cx, "connection-modal-title"),
        bounds(cx, "connection-form"),
        bounds(cx, "save-connection"),
    );
    assert!(
        panel.top() >= px(0.) && panel.bottom() <= viewport.height,
        "the dialog fits the window: {panel:?} in {viewport:?}"
    );
    assert!(
        save.bottom() <= panel.bottom() && save.top() >= title.bottom(),
        "the buttons are in it: {save:?}"
    );
    // One scroll, the dialog's: the form is as tall as what it holds, and goes on under the
    // buttons instead of scrolling inside a box of its own.
    assert!(
        form.bottom() > save.top(),
        "the form is cut by the dialog's scroll: {form:?} against {save:?}"
    );

    cx.update(|window, cx| {
        window.scroll(
            "connection-name-field-label",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-4000.))),
            cx,
        )
    });
    settle(cx);
    assert_eq!(bounds(cx, "save-connection"), save, "Save stays");
    let last = bounds(cx, "connection-headed");
    assert!(
        last.bottom() <= save.top() && last.top() >= title.bottom(),
        "the last switch is now between the title and the buttons: {last:?}"
    );
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing is redrawn at rest"
    );
}

#[gpui_kit::test]
fn the_form_says_what_is_required_and_what_is_optional_once(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    click(cx, "new-connection");
    settle(cx);

    assert_eq!(
        label(cx, "connection-optional").as_deref(),
        Some("Optional: everything below can stay empty")
    );
    let bounds = |cx: &mut VisualTestContext, id: &str| {
        let id = id.to_owned();
        cx.update(|window, _| window.find(id).bounds())
    };
    let line = bounds(cx, "connection-optional");
    for required in [
        "connection-name-field",
        "connection-reach",
        "connection-command-field",
    ] {
        assert!(
            bounds(cx, required).bottom() <= line.top(),
            "{required} is above the line"
        );
    }
    for optional in [
        "connection-variables-field",
        "connection-limits-field",
        "connection-headed",
    ] {
        assert!(
            bounds(cx, optional).top() >= line.bottom(),
            "{optional} is below the line"
        );
    }
    // The first part says it is required, once.
    assert_eq!(
        label(cx, "connection-section-what-tag").as_deref(),
        Some("required")
    );
    assert!(!visible(cx, "connection-section-reach-tag"));
}

// ── The form of a browser ────────────────────────────────────────────────────────────────────

/// The form of the connection `name`, open on its declaration.
fn open_the_form_of(shell: &Entity<Shell>, cx: &mut VisualTestContext, name: &str) {
    open_page(shell, cx);
    until(shell, cx, "its row", |cx| {
        visible(cx, &format!("connection-{name}"))
    });
    click(cx, &format!("connection-more-{name}"));
    click(cx, &format!("edit-connection-{name}"));
    until(shell, cx, "the form opens on it", |cx| {
        visible(cx, "connection-modal")
    });
    settle(cx);
}

#[gpui_kit::test]
fn the_form_has_no_sites_field(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    agent_browser(&user, &["agent_browser_click"], false);
    let (shell, cx) = window(&user, cx);
    open_the_form_of(&shell, cx, "browser");
    // The browser goes wherever the agent takes it: no host is added to a sign-in.
    assert!(visible(cx, "connection-headed"));
    assert!(!present(cx, "connection-sites-field-label"));
    assert!(!present(cx, "connection-new-site"));
}

#[gpui_kit::test]
fn editing_the_command_pins_the_server_again_with_no_review(cx: &mut TestAppContext) {
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
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
    // The daemon listed what the edited server announces and pinned it: no review is asked.
    let edited = connection(&user, "almanac").unwrap();
    assert_eq!(edited["state"], "ready");
    assert!(edited["command"].to_string().contains("--quiet"));
    assert_eq!(
        label(cx, "connection-almanac-line").as_deref(),
        Some("On · 3 actions")
    );
}

/// Opens the form of the connection `name`, removes the entry `gone` (a variable or a limit),
/// saves, and waits for the dialog to close.
fn save_without(shell: &Entity<Shell>, cx: &mut VisualTestContext, name: &str, gone: &str) {
    until(shell, cx, "its row", |cx| {
        visible(cx, &format!("connection-{name}"))
    });
    click(cx, &format!("connection-more-{name}"));
    click(cx, &format!("edit-connection-{name}"));
    until(shell, cx, "the entry is in the form", |cx| {
        visible(cx, gone)
    });
    click(cx, gone);
    click(cx, "save-connection");
    until(shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
}

#[gpui_kit::test]
fn removing_the_last_variable_of_a_command_clears_it_on_the_daemon(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    user.call(
        "vault.set",
        json!({"name": "almanac-token", "secret": SECRET, "description": "Almanac's token"}),
    )
    .unwrap();
    write_script(&user, "almanac", &almanac());
    user.call(
        "connection.add",
        json!({"name": "almanac", "about": "The almanac of the user.",
            "command": command_of(&user, "almanac"),
            "env": {"ALMANAC_TOKEN": {"vault": "almanac-token"}}}),
    )
    .unwrap();
    assert!(connection(&user, "almanac").unwrap()["env"].is_object());
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    save_without(&shell, cx, "almanac", "connection-variable-0-remove");
    assert!(
        connection(&user, "almanac").unwrap().get("env").is_none(),
        "{:?}",
        connection(&user, "almanac")
    );
}

#[gpui_kit::test]
fn removing_the_last_header_of_a_url_clears_it_on_the_daemon(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    user.call("vault.set", json!({"name": "notes-key", "secret": SECRET}))
        .unwrap();
    user.call(
        "connection.add",
        json!({"name": "remote", "about": "A notes server elsewhere.",
            "url": "https://notes.example.com/mcp",
            "headers": {"Authorization": {"vault": "notes-key"}}}),
    )
    .unwrap();
    assert!(connection(&user, "remote").unwrap()["headers"].is_object());
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    save_without(&shell, cx, "remote", "connection-secret-0-remove");
    assert!(
        connection(&user, "remote")
            .unwrap()
            .get("headers")
            .is_none(),
        "{:?}",
        connection(&user, "remote")
    );
}

#[gpui_kit::test]
fn removing_the_last_limit_clears_it_on_the_daemon(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    write_script(&user, "almanac", &almanac());
    user.call(
        "connection.add",
        json!({"name": "almanac", "about": "The almanac of the user.",
            "command": command_of(&user, "almanac"), "only": {"app": ["gedit"]}}),
    )
    .unwrap();
    assert!(connection(&user, "almanac").unwrap()["only"].is_object());
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    save_without(&shell, cx, "almanac", "connection-limit-0-remove");
    assert!(
        connection(&user, "almanac").unwrap().get("only").is_none(),
        "{:?}",
        connection(&user, "almanac")
    );
}

/// The second lines of the toasts shown, in the order of `toasts`.
fn second_lines(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<Option<String>> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|t| t.second_line().map(str::to_owned))
            .collect()
    })
}

#[gpui_kit::test]
fn an_edit_that_announces_a_change_says_a_review_is_needed(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    connect(&user, "almanac", &almanac(), json!({}));
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| {
        label(cx, "connection-almanac-line").as_deref() == Some("On · 3 actions")
    });
    // The server announces another description now: nobody read it, so it is not pinned.
    let mut newer = almanac();
    newer["tools"][1]["description"] = json!("Deletes every note.");
    write_script(&user, "almanac", &newer);
    click(cx, "connection-more-almanac");
    click(cx, "edit-connection-almanac");
    let section = section(&shell, cx);
    let fields = section.read_with(cx, |s, _| s.form_fields().unwrap());
    let said = fields.command.read_with(cx, |f, _| f.value().to_string());
    type_in(&fields.command, &format!("{said} --quiet"), cx);
    click(cx, "save-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(
        connection(&user, "almanac").unwrap()["state"],
        "awaiting review"
    );
    until(&shell, cx, "the toast says it is saved", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "Server almanac saved")
    });
    let said = toasts(&shell, cx);
    assert!(said.iter().any(|t| t == "Server almanac saved"), "{said:?}");
    let second = second_lines(&shell, cx);
    assert!(
        second.iter().flatten().any(|line| line.contains("review")),
        "{second:?}"
    );
    assert_eq!(label(cx, "toast-1-action").as_deref(), Some("Review"));
}

#[gpui_kit::test]
fn an_edit_whose_server_does_not_start_says_why_in_its_toast(cx: &mut TestAppContext) {
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
    type_in(&fields.command, "/nonexistent/flottille-no-such-server", cx);
    click(cx, "save-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
    let second = second_lines(&shell, cx);
    assert!(
        second
            .iter()
            .flatten()
            .any(|line| line.starts_with("It waits for its review:") && line.contains("start")),
        "{second:?}"
    );
}

#[gpui_kit::test]
fn the_form_says_it_is_saving_and_cannot_be_submitted_twice(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    write_script(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    click(cx, "new-connection");
    let section = section(&shell, cx);
    let fields = section.read_with(cx, |s, _| s.form_fields().unwrap());
    type_in(&fields.name, "almanac", cx);
    type_in(&fields.about, "The user's notes.", cx);
    let line = command_of(&user, "almanac")
        .iter()
        .map(|word| format!("'{word}'"))
        .collect::<Vec<_>>()
        .join(" ");
    type_in(&fields.command, &line, cx);
    assert_eq!(label(cx, "save-connection").as_deref(), Some("Add"));
    click(cx, "save-connection");
    // The daemon lists the server's tools before it answers: the dialog says it waits, and a
    // second press sends nothing.
    assert_eq!(label(cx, "save-connection").as_deref(), Some("Adding…"));
    click(cx, "save-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
    until(&shell, cx, "the toast says it is given", |cx| {
        toasts(&shell, cx) == ["Server almanac is given to agents"]
    });
    let said = toasts(&shell, cx);
    assert_eq!(said, ["Server almanac is given to agents"], "{said:?}");
}

#[gpui_kit::test]
fn a_server_added_while_the_extension_was_off_is_given_when_its_switch_is_turned_on(
    cx: &mut TestAppContext,
) {
    let user = user();
    declare(&user, "almanac", &almanac());
    assert_eq!(connection(&user, "almanac").unwrap()["state"], "off");
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    click(cx, "connections-switch");
    until(&shell, cx, "the toast says it", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "Server almanac is given to agents")
    });
    assert_eq!(connection(&user, "almanac").unwrap()["state"], "ready");
    // It does not claim every action is judged: a server pinned again keeps its settings.
    let second = second_lines(&shell, cx);
    assert!(
        second.contains(&Some(
            "Review it to change what each action does.".to_owned()
        )),
        "{second:?}"
    );
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
    let whole = height(cx, "connection-item-almanac");
    assert!(whole > 0.);
    press(cx, "connection-removal-action");
    until(&shell, cx, "its row begins to fold", |cx| {
        height(cx, "connection-item-almanac") < whole
    });
    a_third_later(cx);
    let folding = height(cx, "connection-item-almanac");
    assert!(
        0. < folding && folding < whole,
        "its row folds away, over a panel's time: {folding} of {whole}"
    );
    until(&shell, cx, "its row folds away", |cx| {
        !present(cx, "connection-almanac")
    });
    assert!(connection(&user, "almanac").is_none());
    assert!(visible(cx, "no-connection"));
}

// ── The review ───────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn a_refused_accept_is_said_under_the_review(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "almanac");
    write_script(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    open_the_review(&shell, cx, "almanac", "connection-review-row-3-search");
    // The server is removed under the open review: the daemon refuses what is accepted.
    user.call("connection.remove", json!({"name": "almanac"}))
        .unwrap();
    click(cx, "accept-connection");
    until(&shell, cx, "the refusal is said", |cx| {
        visible(cx, "connection-accept-error")
    });
    settle(cx);

    // The line is `space::MD` under what the review shows: the body is a column that gaps it.
    let (content, line) = cx.update(|window, _| {
        (
            window.find("connection-review-waiting").bounds(),
            window.find("connection-accept-error-block").bounds(),
        )
    });
    assert_eq!(line.top() - content.bottom(), space::MD);
}

#[gpui_kit::test]
fn the_review_is_a_dialog_where_each_action_is_judged_by_default(cx: &mut TestAppContext) {
    let user = user();
    // A first review: the connection was added while its server could not start.
    turn_on(&user);
    declare_waiting(&user, "almanac");
    write_script(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    open_the_review(&shell, cx, "almanac", "connection-review-row-3-search");
    assert!(visible(cx, "connection-review"));
    assert!(
        visible(cx, "connection-review-cancel"),
        "a dialog with a Cancel"
    );
    assert_eq!(
        label(cx, "connection-review-server").as_deref(),
        Some("almanac 1.0.0")
    );
    // Its instructions are folded to their first line; they unfold on a click, as plain lines.
    assert_eq!(
        label(cx, "connection-review-instructions-about").as_deref(),
        Some("Search before you write.")
    );
    assert!(!present(cx, "connection-review-instructions-text-line-0"));
    click(cx, "connection-review-instructions");
    assert_eq!(
        label(cx, "connection-review-instructions-text-line-0")
            .as_deref()
            .map(str::trim),
        Some("Search before you write.")
    );
    assert_eq!(
        label(cx, "connection-review-instructions-text-line-1")
            .as_deref()
            .map(str::trim),
        Some("Keep notes short.")
    );
    for action in ["search", "write", "open"] {
        assert_eq!(group_of(cx, action), Some("judged"), "{action}");
        assert_eq!(pill(cx, action).as_deref(), Some("Judged"), "{action}");
    }
    let said = everything_said(cx);
    assert!(said.contains("Finds notes by words."));
    assert!(!said.contains("passes"), "the word passes is not shown");

    // Search is allowed from its unfolded row, open is turned off by its pill, write stays judged.
    let search = row_of(cx, "search");
    click(cx, &format!("{search}-row"));
    for key in ["off", "passes", "judged", "asked"] {
        assert!(visible(cx, &format!("{search}-choose-{key}")), "{key}");
    }
    assert!(selected(cx, &format!("{search}-choose-judged")));
    click(cx, &format!("{search}-choose-passes"));
    assert_eq!(group_of(cx, "search"), Some("passes"));
    choose(cx, "open", "off");
    assert_eq!(group_of(cx, "open"), Some("off"));
    click(cx, "accept-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-review")
    });
    let accepted = connection(&user, "almanac").unwrap();
    assert_eq!(accepted["state"], "ready");
    assert_eq!(pinned(&user, "almanac", "search"), "passes");
    assert_eq!(pinned(&user, "almanac", "write"), "judged");
    assert_eq!(pinned(&user, "almanac", "open"), "off");
    until(&shell, cx, "its row says it is on", |cx| {
        label(cx, "connection-almanac-line").as_deref() == Some("On · 2 actions")
    });
}

#[gpui_kit::test]
fn the_review_lists_one_row_per_action(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );

    let names: Vec<String> = driftbrowser()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names.len(), 14);
    let mut tops = Vec::new();
    for name in &names {
        let row = row_of(cx, name);
        assert_eq!(group_of(cx, name), Some("judged"), "{name}");
        assert_eq!(
            label(cx, &format!("{row}-name")).as_deref(),
            Some(name.as_str())
        );
        let h = height(cx, &row);
        assert!(
            0. < h && h <= 40.,
            "{name} is one compact row, not a card: {h}"
        );
        tops.push(cx.update(|window, _| f32::from(window.find(row).bounds().origin.y)));
    }
    tops.sort_by(f32::total_cmp);
    assert!(
        tops.windows(2).all(|w| w[1] - w[0] >= 28.),
        "one under the other, none over another: {tops:?}"
    );
    // Its group says how many it holds, and the empty groups are not drawn.
    assert_eq!(
        label(cx, "connection-review-group-judged-count").as_deref(),
        Some("14")
    );
    for empty in ["asked", "passes", "off"] {
        assert!(
            !present(cx, &format!("connection-review-group-{empty}")),
            "{empty}"
        );
    }
    // The row says the first line of the description and the full text waits for a click.
    let eval = row_of(cx, "browser_eval");
    assert_eq!(
        label(cx, &format!("{eval}-about")).as_deref(),
        Some("Evaluate JavaScript in the page.")
    );
    assert_eq!(height(cx, &format!("{eval}-row-body")), 0.);
    let said = everything_said(cx);
    assert!(!said.contains("The script can read anything"), "{said}");
    let folded = height(cx, &eval);
    click(cx, &format!("{eval}-row"));
    let said = everything_said(cx);
    assert!(
        said.contains("The script can read anything the page can read."),
        "{said}"
    );
    assert!(
        said.contains("script (string, required): A function body."),
        "{said}"
    );
    assert!(said.contains("The server says it may destroy."), "{said}");
    assert!(height(cx, &eval) > folded, "the row grew to hold its text");
    // The four choices, each with its sentence, the one it has chosen.
    for (key, sentence) in [
        ("off", "Agents never see it."),
        ("passes", "It runs in every mode, without asking."),
        ("judged", "The judge decides in Auto; you are asked in Ask."),
        ("asked", "It always comes to you, never to the judge."),
    ] {
        assert!(visible(cx, &format!("{eval}-choose-{key}")), "{key}");
        assert!(said.contains(sentence), "{sentence}");
    }
    assert!(selected(cx, &format!("{eval}-choose-judged")));
    // A click on it again folds it.
    click(cx, &format!("{eval}-row"));
    assert_eq!(height(cx, &format!("{eval}-row-body")), 0.);
    assert!(height(cx, &eval) <= folded + 0.5);
}

#[gpui_kit::test]
fn the_review_search_filters_actions(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    let all = [
        "browser_open",
        "browser_cookies",
        "browser_storage",
        "browser_upload",
        "browser_eval",
    ];
    assert_eq!(names_drawn(cx, &all).len(), all.len());
    assert_eq!(
        label(cx, "connection-review-filter-all").as_deref(),
        Some("All 14")
    );
    let field = search_field(&shell, cx);

    // By name and by description: "cookie" is in two names and in one description.
    type_in(&field, "cookie", cx);
    settle(cx);
    assert_eq!(
        names_drawn(cx, &all),
        ["browser_cookies", "browser_storage"],
        "the others are gone"
    );
    assert_eq!(
        label(cx, "connection-review-filter-all").as_deref(),
        Some("All 2")
    );
    assert_eq!(
        label(cx, "connection-review-filter-asked").as_deref(),
        Some("Asked 0")
    );
    assert_eq!(
        label(cx, "connection-review-group-judged-count").as_deref(),
        Some("2")
    );
    type_in(&field, "cookie jar", cx);
    settle(cx);
    assert_eq!(names_drawn(cx, &all), ["browser_storage"]);
    type_in(&field, "zzz", cx);
    settle(cx);
    assert!(names_drawn(cx, &all).is_empty());
    assert!(
        label(cx, "connection-review-none").is_some_and(|said| said.contains("zzz")),
        "a line says nothing matches"
    );
    // The field's clear button brings every row back.
    click(cx, "connection-review-search-clear");
    assert_eq!(field.read_with(cx, |f, _| f.value().to_string()), "");
    assert_eq!(names_drawn(cx, &all).len(), all.len());
    assert!(!visible(cx, "connection-review-none"));

    // By setting: the chips filter, and the chosen one clicked again goes back to all.
    choose(cx, "browser_upload", "asked");
    assert_eq!(
        label(cx, "connection-review-filter-asked").as_deref(),
        Some("Asked 1")
    );
    assert_eq!(
        label(cx, "connection-review-filter-judged").as_deref(),
        Some("Judged 13")
    );
    click(cx, "connection-review-filter-asked");
    assert!(selected(cx, "connection-review-filter-asked"));
    assert_eq!(names_drawn(cx, &all), ["browser_upload"]);
    click(cx, "connection-review-filter-asked");
    assert!(selected(cx, "connection-review-filter-all"));
    assert_eq!(names_drawn(cx, &all).len(), all.len());
}

#[gpui_kit::test]
fn several_actions_are_set_at_once(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    assert!(
        !present(cx, "connection-review-bulk"),
        "no bar before a row is ticked"
    );

    // Two rows ticked: a bar unfolds saying so, and sets them both.
    for name in ["browser_click", "browser_type"] {
        let row = row_of(cx, name);
        click(cx, &format!("{row}-pick"));
        assert!(selected(cx, &format!("{row}-pick")), "{name}");
    }
    assert_eq!(
        label(cx, "connection-review-bulk-count").as_deref(),
        Some("2 selected")
    );
    for key in ["off", "passes", "judged", "asked"] {
        assert!(
            visible(cx, &format!("connection-review-bulk-{key}")),
            "{key}"
        );
    }
    click(cx, "connection-review-bulk-asked");
    for name in ["browser_click", "browser_type"] {
        assert_eq!(group_of(cx, name), Some("asked"), "{name}");
        assert_eq!(pill(cx, name).as_deref(), Some("Asked"));
    }
    assert_eq!(
        group_of(cx, "browser_open"),
        Some("judged"),
        "what was not ticked stays"
    );
    assert!(
        !present(cx, "connection-review-bulk"),
        "the bar folds away once it has done"
    );

    // The pill of a ticked row sets every ticked row; the pill of one that is not ticked, itself.
    for name in ["browser_fill", "browser_press"] {
        let row = row_of(cx, name);
        click(cx, &format!("{row}-pick"));
    }
    choose(cx, "browser_fill", "off");
    assert_eq!(group_of(cx, "browser_fill"), Some("off"));
    assert_eq!(group_of(cx, "browser_press"), Some("off"));
    choose(cx, "browser_open", "passes");
    assert_eq!(group_of(cx, "browser_open"), Some("passes"));
    assert_eq!(group_of(cx, "browser_back"), Some("judged"));
    // Clear empties a selection without setting anything.
    let row = row_of(cx, "browser_back");
    click(cx, &format!("{row}-pick"));
    assert_eq!(
        label(cx, "connection-review-bulk-count").as_deref(),
        Some("1 selected")
    );
    click(cx, "connection-review-bulk-clear");
    assert!(!present(cx, "connection-review-bulk"));
    assert_eq!(group_of(cx, "browser_back"), Some("judged"));

    click(cx, "accept-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-review")
    });
    for (name, setting) in [
        ("browser_click", "asked"),
        ("browser_type", "asked"),
        ("browser_fill", "off"),
        ("browser_press", "off"),
        ("browser_open", "passes"),
        ("browser_back", "judged"),
        ("browser_eval", "judged"),
    ] {
        assert_eq!(pinned(&user, "driftbrowser", name), setting, "{name}");
    }
}

#[gpui_kit::test]
fn a_row_set_to_another_setting_moves_to_its_group(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "almanac");
    write_script(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    open_the_review(&shell, cx, "almanac", "connection-review-row-3-search");
    let whole = height(cx, "connection-review-row-3-search");
    assert!(whole > 0.);

    // It folds away in the group it leaves and unfolds in the one it reaches.
    let id = "connection-review-row-3-search";
    click_pill(cx, id, "passes");
    a_third_later(cx);
    let (leaving, arriving) = (height(cx, id), height(cx, "connection-review-row-1-search"));
    assert!(
        0. < leaving && leaving < whole,
        "it folds away over a panel's time: {leaving} of {whole}"
    );
    assert!(
        0. < arriving && arriving < whole,
        "it unfolds over a panel's time: {arriving} of {whole}"
    );
    assert!(
        present(cx, "connection-review-group-passes"),
        "its new group unfolds"
    );
    settle(cx);
    assert_eq!(rows_of(cx, "search"), ["connection-review-row-1-search"]);
    assert_eq!(
        label(cx, "connection-review-group-passes-count").as_deref(),
        Some("1")
    );
    assert_eq!(
        label(cx, "connection-review-group-judged-count").as_deref(),
        Some("2")
    );

    // The last row to leave a group takes the group away with it, folding.
    choose(cx, "write", "passes");
    click_pill(cx, "connection-review-row-3-open", "off");
    a_third_later(cx);
    assert!(
        present(cx, "connection-review-group-judged"),
        "it folds, it does not vanish"
    );
    assert!(height(cx, "connection-review-group-judged-group") > 0.);
    settle(cx);
    assert!(!present(cx, "connection-review-group-judged"));
    assert_eq!(
        label(cx, "connection-review-group-passes-count").as_deref(),
        Some("2")
    );
    assert_eq!(
        label(cx, "connection-review-group-off-count").as_deref(),
        Some("1")
    );
    // Its group folds on a click on its header, and its rows with it.
    let open = height(cx, "connection-review-group-passes-body");
    assert!(open > 0.);
    click(cx, "connection-review-group-passes");
    assert_eq!(height(cx, "connection-review-group-passes-body"), 0.);
}

/// The width drawn now of the element `id`, 0 where it is not there.
fn width(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map_or(0., |found| f32::from(found.bounds().size.width))
    })
}

#[gpui_kit::test]
fn the_review_says_how_many_are_left_to_the_judge_and_the_exceptions(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    // With no exception, all of them are left to the judge, and the bar is one piece.
    assert_eq!(
        label(cx, "connection-review-left").as_deref(),
        Some("All 14 left to the judge")
    );
    let whole = width(cx, "connection-review-bar");
    assert!(whole > 0.);
    assert!((width(cx, "connection-review-bar-judged") - whole).abs() < 1.);
    assert_eq!(width(cx, "connection-review-bar-passes"), 0.);

    // A pill changes it: the number at once, the split over a panel's time.
    let id = "connection-review-row-3-browser_click";
    click_pill(cx, id, "passes");
    a_third_later(cx);
    assert_eq!(
        label(cx, "connection-review-left").as_deref(),
        Some("13 left to the judge \u{b7} your exceptions: 1 allowed")
    );
    let (going, rest) = (
        width(cx, "connection-review-bar-passes"),
        width(cx, "connection-review-bar-judged"),
    );
    assert!(
        0. < going && going < whole / 14. - 1.,
        "the split moves: {going} of {}",
        whole / 14.
    );
    assert!(rest < whole, "{rest}");
    settle(cx);
    assert!((width(cx, "connection-review-bar-passes") - whole / 14.).abs() < 1.);
    assert!((width(cx, "connection-review-bar-judged") - whole * 13. / 14.).abs() < 1.);

    // Each setting counts, also what a bulk sets; what is searched does not change it.
    choose(cx, "browser_open", "asked");
    choose(cx, "browser_back", "off");
    assert_eq!(
        label(cx, "connection-review-left").as_deref(),
        Some("11 left to the judge \u{b7} your exceptions: 1 allowed, 1 asked, 1 off")
    );
    for name in ["browser_type", "browser_fill"] {
        let row = row_of(cx, name);
        click(cx, &format!("{row}-pick"));
    }
    click(cx, "connection-review-bulk-off");
    assert_eq!(
        label(cx, "connection-review-left").as_deref(),
        Some("9 left to the judge \u{b7} your exceptions: 1 allowed, 1 asked, 3 off")
    );
    let field = search_field(&shell, cx);
    type_in(&field, "cookie", cx);
    settle(cx);
    assert_eq!(
        label(cx, "connection-review-left").as_deref(),
        Some("9 left to the judge \u{b7} your exceptions: 1 allowed, 1 asked, 3 off"),
        "all the actions, not the ones shown"
    );
}

#[gpui_kit::test]
fn a_ticked_row_hidden_by_the_search_is_not_set(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    for name in ["browser_cookies", "browser_storage", "browser_pdf"] {
        let row = row_of(cx, name);
        click(cx, &format!("{row}-pick"));
    }
    let field = search_field(&shell, cx);
    type_in(&field, "cookie", cx);
    settle(cx);
    // Only what is shown is selected: "browser_pdf" is hidden, and stays ticked.
    assert_eq!(
        label(cx, "connection-review-bulk-count").as_deref(),
        Some("2 selected")
    );
    click(cx, "connection-review-bulk-asked");
    assert_eq!(group_of(cx, "browser_cookies"), Some("asked"));
    assert_eq!(group_of(cx, "browser_storage"), Some("asked"));

    type_in(&field, "", cx);
    settle(cx);
    assert_eq!(
        group_of(cx, "browser_pdf"),
        Some("judged"),
        "the hidden row was not set"
    );
    assert_eq!(
        label(cx, "connection-review-bulk-count").as_deref(),
        Some("1 selected"),
        "it counts again once it is shown"
    );

    // The pill of a ticked row sets the ticked rows that are shown, and no other.
    for name in ["browser_open", "browser_back"] {
        let row = row_of(cx, name);
        click(cx, &format!("{row}-pick"));
    }
    type_in(&field, "browser_open", cx);
    settle(cx);
    choose(cx, "browser_open", "off");
    type_in(&field, "", cx);
    settle(cx);
    assert_eq!(group_of(cx, "browser_open"), Some("off"));
    assert_eq!(group_of(cx, "browser_back"), Some("judged"));
    assert_eq!(group_of(cx, "browser_pdf"), Some("judged"));
    assert_eq!(
        label(cx, "connection-review-bulk-count").as_deref(),
        Some("2 selected")
    );
}

/// The names of the actions of `driftbrowser` whose row is drawn now.
fn shown_rows(cx: &mut VisualTestContext) -> Vec<String> {
    let names: Vec<String> = driftbrowser()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    names
        .into_iter()
        .filter(|name| !rows_of(cx, name).is_empty())
        .collect()
}

/// The names of the actions of `driftbrowser` whose row shows a ticked box now.
fn ticked_rows(cx: &mut VisualTestContext) -> Vec<String> {
    shown_rows(cx)
        .into_iter()
        .filter(|name| match rows_of(cx, name).first() {
            Some(row) => selected(cx, &format!("{row}-pick")),
            None => false,
        })
        .collect()
}

/// What a tick says of its rows: its value, "mixed" when only part of them is ticked.
fn tick_value(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.value().map(str::to_owned))
    })
}

#[gpui_kit::test]
fn a_group_s_tick_selects_every_shown_row_of_the_group(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    // Ask one row first: the Judged group is then every other action.
    choose(cx, "browser_click", "asked");
    let tick = "connection-review-group-judged-pick";
    assert!(visible(cx, tick), "the header has a tick");
    assert!(ticked_rows(cx).is_empty());

    click(cx, tick);
    let ticked = ticked_rows(cx);
    assert!(!ticked.is_empty());
    assert!(
        !ticked.iter().any(|name| name == "browser_click"),
        "a row of another group is not ticked: {ticked:?}"
    );
    for name in &ticked {
        assert_eq!(group_of(cx, name), Some("judged"), "{name}");
    }
    assert!(selected(cx, tick), "the tick says all");
    assert_eq!(
        label(cx, "connection-review-bulk-count"),
        Some(format!("{} selected", ticked.len()))
    );

    // A second click unticks them all.
    click(cx, tick);
    assert!(ticked_rows(cx).is_empty());
    assert!(!selected(cx, tick));
    assert!(!present(cx, "connection-review-bulk"));
}

#[gpui_kit::test]
fn a_group_s_tick_says_some_when_part_of_it_is_ticked(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    let tick = "connection-review-group-judged-pick";
    assert_eq!(tick_value(cx, tick), None, "none ticked says none");
    let row = row_of(cx, "browser_open");
    click(cx, &format!("{row}-pick"));
    assert_eq!(tick_value(cx, tick).as_deref(), Some("mixed"));
    assert!(!selected(cx, tick));

    // A click on a tick that says some ticks them all.
    click(cx, tick);
    assert!(selected(cx, tick));
    assert_eq!(tick_value(cx, tick), None);

    // Unticking one row says some again.
    click(cx, &format!("{row}-pick"));
    assert_eq!(tick_value(cx, tick).as_deref(), Some("mixed"));
}

#[gpui_kit::test]
fn select_all_shown_ticks_what_the_search_shows(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    let row = row_of(cx, "browser_back");
    click(cx, &format!("{row}-pick"));
    let field = search_field(&shell, cx);
    type_in(&field, "cookie", cx);
    settle(cx);
    // The bar shows with a ticked row that shows: tick one of the found rows to bring it.
    let first = shown_rows(cx).remove(0);
    let first = row_of(cx, &first);
    click(cx, &format!("{first}-pick"));
    assert_eq!(
        label(cx, "connection-review-bulk-select-all").as_deref(),
        Some("Select all shown")
    );
    click(cx, "connection-review-bulk-select-all");
    // What the search shows is ticked, and what it hides is left as it was.
    let shown = shown_rows(cx);
    assert!(1 < shown.len() && shown.len() < 14, "{shown:?}");
    assert_eq!(ticked_rows(cx), shown);
    assert_eq!(
        label(cx, "connection-review-bulk-count"),
        Some(format!("{} selected", shown.len()))
    );

    type_in(&field, "", cx);
    settle(cx);
    let (back, press) = (row_of(cx, "browser_back"), row_of(cx, "browser_press"));
    assert!(
        selected(cx, &format!("{back}-pick")),
        "the tick of a hidden row stays"
    );
    assert!(
        !selected(cx, &format!("{press}-pick")),
        "a hidden row was not ticked by it"
    );
    assert_eq!(
        label(cx, "connection-review-bulk-count"),
        Some(format!("{} selected", shown.len() + 1))
    );
}

#[gpui_kit::test]
fn ctrl_a_in_the_review_selects_every_shown_row(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    let field = search_field(&shell, cx);
    type_in(&field, "browser_c", cx);
    settle(cx);
    // The list has the focus once it is clicked in: a click on a row's name opens it, and the
    // focus stays with the list.
    let row = row_of(cx, "browser_click");
    click(cx, &row);
    cx.simulate_keystrokes("ctrl-a");
    settle(cx);
    let shown = shown_rows(cx);
    assert!(1 < shown.len() && shown.len() < 14, "{shown:?}");
    assert_eq!(ticked_rows(cx), shown);
    assert_eq!(
        label(cx, "connection-review-bulk-count"),
        Some(format!("{} selected", shown.len()))
    );
}

#[gpui_kit::test]
fn ctrl_a_on_a_failed_review_keeps_its_message(cx: &mut TestAppContext) {
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
    let said = label(cx, "connection-review-error");
    assert!(said.is_some(), "it says why");

    // The list has the focus once it is clicked in; Ctrl+A on it has nothing to tick.
    click(cx, "connection-review-error");
    cx.simulate_keystrokes("ctrl-a");
    settle(cx);
    assert!(
        visible(cx, "connection-review-error"),
        "the message stays after Ctrl+A"
    );
    assert_eq!(label(cx, "connection-review-error"), said);
}

#[gpui_kit::test]
fn ctrl_a_in_the_search_ticks_no_row(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    let field = search_field(&shell, cx);
    type_in(&field, "cookie", cx);
    settle(cx);
    field.update_in(cx, |field, window, cx| field.focus(window, cx));
    cx.simulate_keystrokes("ctrl-a");
    settle(cx);
    assert!(
        ticked_rows(cx).is_empty(),
        "Ctrl+A in the search selects its text, no row"
    );
    assert!(!present(cx, "connection-review-bulk"));

    // With nothing focused it is nobody's.
    cx.update(|window, cx| window.blur(cx));
    cx.simulate_keystrokes("ctrl-a");
    settle(cx);
    assert!(ticked_rows(cx).is_empty(), "nothing focused ticks nothing");
    assert!(!present(cx, "connection-review-bulk"));
}

#[gpui_kit::test]
fn a_group_s_tick_under_a_search_leaves_hidden_rows_as_they_were(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "driftbrowser");
    write_script(&user, "driftbrowser", &driftbrowser());
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-row-3-browser_open",
    );
    choose(cx, "browser_click", "asked");
    // One hidden row ticked before the search, the others not.
    let back = row_of(cx, "browser_back");
    click(cx, &format!("{back}-pick"));

    let field = search_field(&shell, cx);
    type_in(&field, "cookie", cx);
    settle(cx);
    let found = shown_rows(cx);
    assert_eq!(found.len(), 2, "{found:?}");
    let tick = "connection-review-group-judged-pick";
    click(cx, tick);
    assert_eq!(ticked_rows(cx), found, "the tick ticks the shown rows");

    type_in(&field, "", cx);
    settle(cx);
    let mut ticked = ticked_rows(cx);
    ticked.sort();
    let mut expected = found.clone();
    expected.push("browser_back".to_owned());
    expected.sort();
    assert_eq!(ticked, expected, "the hidden rows kept their state");

    // Unticking under the search leaves the hidden ticked row ticked.
    type_in(&field, "cookie", cx);
    settle(cx);
    click(cx, tick);
    assert!(ticked_rows(cx).is_empty());
    type_in(&field, "", cx);
    settle(cx);
    assert_eq!(ticked_rows(cx), ["browser_back"]);
}

#[gpui_kit::test]
fn a_group_title_does_not_move_when_a_search_leaves_it_only_locked_rows(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    agent_browser(
        &user,
        &[
            "agent_browser_click",
            "agent_browser_fill",
            "agent_browser_auth_save",
            "agent_browser_eval",
        ],
        true,
    );
    let (shell, cx) = window(&user, cx);
    open_the_review(&shell, cx, "browser", "connection-review-adapter");
    // The Off group holds a row with a box and the locked login.
    choose(cx, "agent_browser_eval", "off");
    let title = "connection-review-group-off-title";
    let left = |cx: &mut VisualTestContext| {
        cx.update(|window, _| f32::from(window.find(title).bounds().origin.x))
    };
    let before = left(cx);

    let field = search_field(&shell, cx);
    type_in(&field, "auth_save", cx);
    settle(cx);
    assert!(
        rows_of(cx, "agent_browser_eval").is_empty(),
        "the search leaves the Off group the locked row only"
    );
    assert!(!rows_of(cx, "agent_browser_auth_save").is_empty());
    assert_eq!(left(cx), before, "the title stays where it was");

    type_in(&field, "", cx);
    settle(cx);
    assert_eq!(left(cx), before);
}

/// A server with `n` actions, `tool_000` to `tool_{n-1}`, each with a sentence of its own.
fn many_tools(n: usize) -> Value {
    let tools: Vec<Value> = (0..n)
        .map(|at| {
            json!({"name": format!("tool_{at:03}"),
                "description": format!("Does the job number {at} on the page."),
                "inputSchema": {"type": "object", "properties": {}}})
        })
        .collect();
    let answers: serde_json::Map<String, Value> = (0..n)
        .map(|at| (format!("tool_{at:03}"), json!({"text": "ok"})))
        .collect();
    json!({"server": {"name": "sprawl", "version": "1.0.0"},
        "instructions": "Many small things.", "tools": tools, "answers": answers})
}

#[gpui_kit::test]
fn a_frame_of_a_long_review_does_not_match_every_action_again(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "sprawl");
    write_script(&user, "sprawl", &many_tools(160));
    let (shell, cx) = window(&user, cx);
    open_the_review(&shell, cx, "sprawl", "connection-review-row-3-tool_000");
    let review = section(&shell, cx);
    let matched = |cx: &mut VisualTestContext| review.read_with(cx, |s, _| s.review_matches());
    assert_eq!(matched(cx), 0, "no word typed is nothing to match");

    // A text is matched with the 160 actions once, however many frames follow.
    let field = search_field(&shell, cx);
    type_in(&field, "tool_01", cx);
    settle(cx);
    assert_eq!(matched(cx), 160);
    settle(cx);
    click_pill(cx, "connection-review-row-3-tool_010", "asked");
    settle(cx);
    click(cx, "connection-review-filter-asked");
    click(cx, "connection-review-filter-all");
    assert_eq!(
        matched(cx),
        160,
        "a pill, a chip and every frame of their motion use what was found"
    );
    type_in(&field, "tool_02", cx);
    settle(cx);
    assert_eq!(matched(cx), 320, "another text is matched again, once");

    // A row draws its text and its choices while it is open and while it folds away only.
    let row = row_of(cx, "tool_020");
    assert!(!present(cx, &format!("{row}-choose-judged")), "folded");
    click(cx, &format!("{row}-row"));
    assert!(visible(cx, &format!("{row}-choose-judged")), "open");
    press(cx, &format!("{row}-row"));
    a_third_later(cx);
    assert!(
        height(cx, &format!("{row}-row-body")) > 0.,
        "it still shows while it folds away"
    );
    settle(cx);
    assert!(
        !present(cx, &format!("{row}-choose-judged")),
        "folded again"
    );
}

/// Opens the pill of the row `id` and picks `setting`, leaving the clock where it is.
fn click_pill(cx: &mut VisualTestContext, id: &str, setting: &str) {
    click(cx, &format!("{id}-setting"));
    press(cx, &format!("{id}-setting-{setting}"));
}

#[gpui_kit::test]
fn a_rereview_shows_the_changes_first(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    connect(
        &user,
        "driftbrowser",
        &driftbrowser(),
        json!({"browser_snapshot": "passes", "browser_upload": "asked",
            "browser_cookies": "off", "browser_eval": "off"}),
    );
    let mut now = driftbrowser();
    now["server"]["version"] = json!("0.10.0");
    now["instructions"] =
        json!("Drives one real browser window.\nAlways call a snapshot after a page changes.");
    let tools = now["tools"].as_array_mut().unwrap();
    tools.retain(|t| t["name"] != "browser_pdf");
    for tool in tools.iter_mut() {
        match tool["name"].as_str().unwrap() {
            "browser_click" => tool["description"] = json!("Click an element, then snapshot."),
            "browser_eval" => tool["description"] = json!("Evaluate JavaScript in the main world."),
            "browser_download" => tool["description"] = json!("Download a file to a path."),
            _ => {}
        }
    }
    tools.push(
        json!({"name": "browser_wait_for_text", "description": "Wait until a text appears.",
        "inputSchema": {"type": "object", "properties": {}}}),
    );
    write_script(&user, "driftbrowser", &now);
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-change-browser_click",
    );

    // What changed comes first, saying how many and since when; the removed one is in it.
    assert_eq!(
        label(cx, "connection-review-changes-title").as_deref(),
        Some("What changed")
    );
    assert_eq!(
        label(cx, "connection-review-changes-count").as_deref(),
        Some("5")
    );
    let y = |cx: &mut VisualTestContext, id: &str| {
        let id = id.to_owned();
        cx.update(|window, _| f32::from(window.find(id).bounds().origin.y))
    };
    for (name, tag) in [
        ("browser_wait_for_text", "New"),
        ("browser_click", "Changed"),
        ("browser_eval", "Changed"),
        ("browser_download", "Changed"),
        ("browser_pdf", "Removed"),
    ] {
        let row = format!("connection-review-change-{name}");
        assert!(present(cx, &row), "{name}");
        assert_eq!(
            label(cx, &format!("{row}-change")).as_deref(),
            Some(tag),
            "{name}"
        );
    }
    // A changed action is judged again, a removed one has no setting to give.
    assert_eq!(pill(cx, "browser_eval").as_deref(), Some("Judged"));
    assert!(!present(cx, "connection-review-change-browser_pdf-setting"));
    // The unchanged ones are one line, saying what they hold, and their settings are kept.
    assert_eq!(
        label(cx, "connection-review-unchanged-title").as_deref(),
        Some("10 unchanged")
    );
    assert_eq!(
        label(cx, "connection-review-unchanged-detail").as_deref(),
        Some("1 allowed · 1 asked · 1 off · 7 judged")
    );
    assert!(
        y(cx, "connection-review-changes") < y(cx, "connection-review-unchanged"),
        "the changes first"
    );
    assert!(!visible(cx, "connection-review-row-1-browser_snapshot"));
    // The instructions changed too: a line says so, and the diff unfolds under it.
    assert_eq!(
        label(cx, "connection-review-instructions-change").as_deref(),
        Some("Changed")
    );
    assert!(!present(cx, "connection-review-instructions-text-line-0"));
    click(cx, "connection-review-instructions");
    let line = |cx: &mut VisualTestContext, at: usize| {
        label(
            cx,
            &format!("connection-review-instructions-text-line-{at}"),
        )
    };
    assert_eq!(
        line(cx, 0).as_deref().map(str::trim),
        Some("Drives one real browser window.")
    );
    assert_eq!(
        line(cx, 1).as_deref(),
        Some("− Call a snapshot after a page changes.")
    );
    assert_eq!(
        line(cx, 2).as_deref(),
        Some("+ Always call a snapshot after a page changes.")
    );
    // A changed action shows its old text and its new one.
    let row = "connection-review-change-browser_eval";
    click(cx, &format!("{row}-row"));
    let said = everything_said(cx);
    assert!(
        said.contains("− Evaluate JavaScript in the page."),
        "{said}"
    );
    assert!(
        said.contains("+ Evaluate JavaScript in the main world."),
        "{said}"
    );

    // The unchanged unfold into the same groups.
    click(cx, "connection-review-unchanged");
    assert_eq!(group_of(cx, "browser_snapshot"), Some("passes"));
    assert_eq!(group_of(cx, "browser_upload"), Some("asked"));
    assert_eq!(group_of(cx, "browser_cookies"), Some("off"));
    assert_eq!(group_of(cx, "browser_open"), Some("judged"));
    // A search that finds one of them unfolds the line by itself, and folds it back.
    click(cx, "connection-review-unchanged");
    assert!(!visible(cx, "connection-review-row-1-browser_snapshot"));
    let field = search_field(&shell, cx);
    type_in(&field, "snapshot", cx);
    settle(cx);
    assert!(visible(cx, "connection-review-row-1-browser_snapshot"));
    type_in(&field, "", cx);
    settle(cx);
    assert!(!visible(cx, "connection-review-row-1-browser_snapshot"));
    // The changed filter is a chip of a re-review.
    click(cx, "connection-review-filter-changed");
    assert!(selected(cx, "connection-review-filter-changed"));
    assert!(present(cx, "connection-review-change-browser_click"));
    assert!(!present(
        cx,
        "connection-review-change-browser_wait_for_text"
    ));
    click(cx, "connection-review-filter-changed");
    assert!(selected(cx, "connection-review-filter-all"));
}

#[gpui_kit::test]
fn the_review_dialog_keeps_a_readable_width(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "almanac");
    let mut script = almanac();
    script["tools"][0]["description"] = json!(format!("{}\n{}", guard::UNBROKEN, guard::SPACED));
    write_script(&user, "almanac", &script);
    let (shell, cx) = window(&user, cx);
    open_the_review(&shell, cx, "almanac", "connection-review-row-3-search");
    let frame = cx.update(|window, _| window.find("connection-review").bounds());
    let (width, height) = (f32::from(frame.size.width), f32::from(frame.size.height));
    assert!(
        width <= 640.,
        "a dialog of a readable width, not the window's: {width}"
    );
    assert!(width >= 480., "and not a strip: {width}");
    assert!(height < 2400., "it fits its window");
    // Its header and its footer stay in sight: Accept and Cancel are inside it.
    for id in ["accept-connection", "connection-review-cancel"] {
        let at = cx.update(|window, _| window.find(id).bounds());
        assert!(
            frame.contains(&at.origin) && frame.contains(&at.bottom_right()),
            "{id}"
        );
    }
    // A long word and a long sentence wrap inside the rows and the unfolded text.
    let row = row_of(cx, "search");
    click(cx, &format!("{row}-row"));
    guard::assert_stays_inside(&shell, cx, "connection-review");
    guard::narrow(cx);
    settle(cx);
    guard::assert_stays_inside(&shell, cx, "connection-review");
}

/// A connection `browser` to a server that says it is agent-browser 0.43.0 and serves `names`:
/// pinned on its first start, or, with `waiting`, added before its script is written, so that it
/// waits for its first review.
fn agent_browser(user: &User, names: &[&str], waiting: bool) {
    let tool = |name: &str| {
        json!({"name": name, "description": format!("{name} of the browser."),
            "inputSchema": {"type": "object", "properties": {}}})
    };
    let script = json!({"server": {"name": "agent-browser", "version": "0.43.0"},
        "tools": names.iter().map(|name| tool(name)).collect::<Vec<_>>()});
    if !waiting {
        write_script(user, "browser", &script);
    }
    let mut command = command_of(user, "browser");
    command.push("mcp".into());
    user.call(
        "connection.add",
        json!({"name": "browser", "about": "A browser.", "command": command}),
    )
    .unwrap();
    if waiting {
        write_script(user, "browser", &script);
    }
}

#[gpui_kit::test]
fn the_review_says_an_adapter_applies_and_why_an_action_stays_off(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    agent_browser(
        &user,
        &[
            "agent_browser_open",
            "agent_browser_eval",
            "agent_browser_auth_save",
            "agent_browser_get_attr",
        ],
        false,
    );
    // An action that reads the page, turned on: login stays offered beside it.
    user.call(
        "connection.accept",
        json!({"name": "browser", "actions": {"agent_browser_get_attr": "judged"}}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_the_review(&shell, cx, "browser", "connection-review-adapter");

    // That it applies: said above the bar, before Accept, and login is refused for no action.
    let said = label(cx, "connection-review-adapter").unwrap();
    assert!(
        said.contains("Flottille's adapter for agent-browser 0.43.x applies"),
        "{said}"
    );
    assert!(!said.contains("login is refused while"), "{said}");
    assert!(
        visible(cx, "connection-review-left"),
        "the bar is under the adapter's sentence"
    );
    // What it changed unfolds under the sentence.
    assert_eq!(height(cx, "connection-review-adapter-changes-body"), 0.);
    click(cx, "connection-review-adapter-changes");
    assert!(height(cx, "connection-review-adapter-changes-body") > 0.);
    let changed = everything_said(cx);
    assert!(
        changed.contains("login is Flottille's own action"),
        "{changed}"
    );
    assert!(changed.contains("--config"), "{changed}");

    // Its own logins cannot be turned on: the pill is disabled, and the unfolded row says why.
    click(cx, "connection-review-unchanged");
    let locked = row_of(cx, "agent_browser_auth_save");
    assert_eq!(pill(cx, "agent_browser_auth_save").as_deref(), Some("Off"));
    assert!(
        !present(cx, &format!("{locked}-locked")),
        "the reason waits for the row to unfold"
    );
    click(cx, &format!("{locked}-row"));
    let why = label(cx, &format!("{locked}-locked")).unwrap();
    assert!(why.contains("vault"), "{why}");
    assert!(
        !present(cx, &format!("{locked}-choose-passes")),
        "no choice turns it on"
    );
    click(cx, &format!("{locked}-setting"));
    assert!(
        !present(cx, &format!("{locked}-setting-passes")),
        "its list does not open"
    );
    assert!(
        !present(cx, &format!("{locked}-pick")),
        "no box to tick it with"
    );
    // Eval is off at its first pin, and the user's to turn on: its row has all four choices.
    let eval = row_of(cx, "agent_browser_eval");
    click(cx, &format!("{eval}-row"));
    assert!(present(cx, &format!("{eval}-choose-passes")));
    assert!(!present(cx, &format!("{eval}-locked")));
    assert!(present(cx, &format!("{eval}-pick")));
}

#[gpui_kit::test]
fn a_locked_action_is_not_set_by_the_bulk_bar(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    agent_browser(
        &user,
        &[
            "agent_browser_click",
            "agent_browser_fill",
            "agent_browser_auth_save",
            "agent_browser_eval",
        ],
        true,
    );
    let (shell, cx) = window(&user, cx);
    open_the_review(&shell, cx, "browser", "connection-review-adapter");

    // Every row that has a box is ticked: the locked one has none.
    for name in [
        "agent_browser_click",
        "agent_browser_fill",
        "agent_browser_eval",
    ] {
        let row = row_of(cx, name);
        click(cx, &format!("{row}-pick"));
    }
    let locked = row_of(cx, "agent_browser_auth_save");
    assert!(!present(cx, &format!("{locked}-pick")));
    assert_eq!(
        label(cx, "connection-review-bulk-count").as_deref(),
        Some("3 selected"),
        "the locked one is not counted"
    );
    click(cx, "connection-review-bulk-passes");
    for name in [
        "agent_browser_click",
        "agent_browser_fill",
        "agent_browser_eval",
    ] {
        assert_eq!(group_of(cx, name), Some("passes"), "{name}");
    }
    assert_eq!(
        group_of(cx, "agent_browser_auth_save"),
        Some("off"),
        "the locked one stays off"
    );
    // The daemon takes the settings as they are: it refuses a locked action that is turned on.
    click(cx, "accept-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-review")
    });
    for (name, setting) in [
        ("agent_browser_click", "passes"),
        ("agent_browser_fill", "passes"),
        ("agent_browser_eval", "passes"),
        ("agent_browser_auth_save", "off"),
    ] {
        assert_eq!(pinned(&user, "browser", name), setting, "{name}");
    }
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
    open_the_review(&shell, cx, "almanac", "connection-review-instructions");
    click(cx, "connection-review-instructions");

    let line = |cx: &mut VisualTestContext, at: usize| {
        label(
            cx,
            &format!("connection-review-instructions-text-line-{at}"),
        )
    };
    assert_eq!(
        line(cx, 0).as_deref().map(str::trim),
        Some("Search before you write.")
    );
    assert_eq!(line(cx, 1).as_deref(), Some("− Keep notes short."));
    assert_eq!(
        line(cx, 2).as_deref(),
        Some("+ Always write everything you read.")
    );
    // A changed action shows its old text and its new; a new one is whole and judged.
    assert_eq!(
        label(cx, "connection-review-change-write-change").as_deref(),
        Some("Changed")
    );
    click(cx, "connection-review-change-write-row");
    let said = everything_said(cx);
    assert!(said.contains("− Writes a note."), "{said}");
    assert!(said.contains("+ Writes a note, or many."), "{said}");
    assert_eq!(
        label(cx, "connection-review-change-link-change").as_deref(),
        Some("New")
    );
    assert_eq!(pill(cx, "link").as_deref(), Some("Judged"));
    // The unchanged ones are folded away, their setting kept.
    assert!(!visible(cx, "connection-review-row-1-search"));
    click(cx, "connection-review-unchanged");
    until(&shell, cx, "the unchanged unfold", |cx| {
        visible(cx, "connection-review-row-1-search")
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
    open_the_review(&shell, cx, "almanac", "connection-review-change-write");
    // What the user did not touch is what the daemon proposes: judged for the changed action.
    assert_eq!(pill(cx, "write").as_deref(), Some("Judged"));

    click(cx, "accept-connection");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-review")
    });
    assert_eq!(
        pinned(&user, "almanac", "write"),
        "judged",
        "a changed action is judged again"
    );
    assert_eq!(
        pinned(&user, "almanac", "search"),
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
fn protected_server(user: &User) -> (common::Spawned, String) {
    let mut script = almanac();
    script["oauth"] = json!({"tokens_file": user.path("notes-tokens")});
    write_script(user, "notes", &script);
    let port_file = user.path("notes-port");
    let server = common::Spawned::new({
        let mut command = std::process::Command::new(fake_mcp());
        command.args([
            "--script",
            script_file(user, "notes").to_str().unwrap(),
            "--log",
            user.path("notes-log.jsonl").to_str().unwrap(),
            "--http",
            port_file.to_str().unwrap(),
        ]);
        command
    });
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
    // Signed in, it is given at once (`docs/extensions.md`, "Connections"): its row ends on, and
    // the moment it waits for its review between the two is one the window may never draw.
    until(&shell, cx, "the row says it is on", |cx| {
        label(cx, "connection-notes-line").as_deref() == Some("On · 3 actions")
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
    let mut server = common::Spawned::new({
        let mut command = std::process::Command::new(fake_mcp());
        command.args([
            "--script",
            script_file(&user, "plain").to_str().unwrap(),
            "--http",
            port_file.to_str().unwrap(),
        ]);
        command
    });
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
        label(cx, "connection-plain-line").as_deref() == Some("On · 3 actions")
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

#[gpui_kit::test]
fn the_form_shows_the_browser_when_its_toggle_is_on(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    agent_browser(&user, &["agent_browser_click"], false);
    let (shell, cx) = window(&user, cx);
    open_the_form_of(&shell, cx, "browser");
    // A labelled toggle, off by default.
    assert_eq!(
        label(cx, "connection-headed-label").as_deref(),
        Some("Show the browser")
    );
    assert_eq!(connection(&user, "browser").unwrap()["headed"], Value::Null);
    click(cx, "connection-headed");
    settle(cx);
    click(cx, "save-connection");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(connection(&user, "browser").unwrap()["headed"], json!(true));
    // Open again: it is on, and saving another field keeps it.
    click(cx, "connection-more-browser");
    click(cx, "edit-connection-browser");
    until(&shell, cx, "the form opens on it", |cx| {
        visible(cx, "connection-modal")
    });
    settle(cx);
    let fields = section(&shell, cx).read_with(cx, |s, _| s.form_fields().unwrap());
    type_in(&fields.about, "The browser of the user.", cx);
    click(cx, "save-connection");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(connection(&user, "browser").unwrap()["headed"], json!(true));
}

#[gpui_kit::test]
fn a_browser_that_could_not_be_shown_says_why_under_its_switch(cx: &mut TestAppContext) {
    // The session names a Wayland socket that is not there: the browser runs headless.
    let user = user().with_env("WAYLAND_DISPLAY", "wayland-none");
    turn_on(&user);
    agent_browser(&user, &["agent_browser_click"], false);
    user.call(
        "connection.edit",
        json!({"name": "browser", "headed": true}),
    )
    .unwrap();
    user.call("connection.review", json!({"name": "browser"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    open_the_form_of(&shell, cx, "browser");
    settle(cx);
    assert!(visible(cx, "connection-headed-why"));
    let said = section(&shell, cx)
        .read_with(cx, |s, _| s.headless_said())
        .expect("it says why");
    assert!(said.contains("not shown"), "{said}");
    assert!(said.contains("Wayland socket"), "{said}");
    // Turned off in the form, there is nothing to say: it folds away.
    click(cx, "connection-headed");
    settle(cx);
    assert!(!visible(cx, "connection-headed-why"));
}

// ── A review that needs a sign-in ────────────────────────────────────────────────────────────

/// The review of `notes`, behind its authorization server and not signed in, opened from its row.
fn review_of_unsigned<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    &'a mut VisualTestContext,
    common::Spawned,
    Arc<std::sync::Mutex<Vec<String>>>,
) {
    turn_on(user);
    let (server, url) = protected_server(user);
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "A notes server.", "url": url}),
    )
    .unwrap();
    let (shell, cx) = window(user, cx);
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
    until(&shell, cx, "its row", |cx| visible(cx, "connection-notes"));
    click(cx, "connection-more-notes");
    click(cx, "review-connection-notes");
    until(&shell, cx, "what the review says", |cx| {
        present(cx, "connection-review-sign-in") || present(cx, "connection-review-error")
    });
    settle(cx);
    (shell, cx, server, opened)
}

#[gpui_kit::test]
fn a_review_that_needs_a_sign_in_offers_the_button(cx: &mut TestAppContext) {
    let user = user();
    let (_shell, cx, mut server, _) = review_of_unsigned(&user, cx);
    assert!(
        present(cx, "connection-review-sign-in"),
        "the review offers Sign in"
    );
    assert_eq!(
        label(cx, "connection-review-sign-in").as_deref(),
        Some("Sign in")
    );
    // A state, not an error: the error line is not drawn, and there is nothing to accept yet.
    assert!(!present(cx, "connection-review-error"));
    assert!(present(cx, "connection-review-needs-sign-in"));
    assert!(!present(cx, "accept-connection"));
    let _ = server.kill();
    let _ = server.wait();
}

#[gpui_kit::test]
fn signing_in_from_the_review_reads_the_server_and_shows_its_actions(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx, mut server, opened) = review_of_unsigned(&user, cx);
    click(cx, "connection-review-sign-in");
    until(&shell, cx, "the browser is opened on the sign-in", |_| {
        !opened.lock().unwrap().is_empty()
    });
    let address = opened.lock().unwrap()[0].clone();
    assert!(address.contains("/as/authorize"), "{address}");
    let (status, location) = browse(&address);
    assert_eq!(status, 302);
    let (status, _) = browse(&location.unwrap());
    assert_eq!(status, 200);
    // Nobody presses anything more: the review reads the server again and shows what it announces.
    // The server is given at once once signed in, so its actions are the pinned ones: the review
    // is ready when it can be accepted, whatever group each row is in.
    until(&shell, cx, "the actions of the server", |cx| {
        present(cx, "accept-connection")
    });
    settle(cx);
    assert!(present(cx, "connection-review-server"));
    assert!(!present(cx, "connection-review-sign-in"));
    assert!(!present(cx, "connection-review-needs-sign-in"));
    assert!(present(cx, "accept-connection"));
    let _ = server.kill();
    let _ = server.wait();
}

#[gpui_kit::test]
fn no_window_message_tells_the_user_to_run_connection_login(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx, mut server, _) = review_of_unsigned(&user, cx);
    let section = section(&shell, cx);
    for said in ["signed_out", "sign_in_failed", "tokens_unsaved"] {
        cx.update(|_, cx| {
            section.update(cx, |s, cx| {
                s.sign_in_said(said, "notes", Some("the vault said no".into()), cx)
            })
        });
        settle(cx);
    }
    let toasts: Vec<String> = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|t| format!("{} {:?}", t.title(), t.second_line()))
            .collect()
    });
    let said = format!("{}\n{}", everything_said(cx), toasts.join("\n"));
    assert!(!said.contains("connection login"), "{said}");
    assert!(!said.contains("flottille connection"), "{said}");
    let _ = server.kill();
    let _ = server.wait();
}

#[gpui_kit::test]
fn a_401_with_a_header_from_the_vault_names_the_entry_not_a_sign_in(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    user.call(
        "vault.set",
        json!({"name": "hippo-token", "secret": SECRET, "description": "Hippo's token"}),
    )
    .unwrap();
    let (mut server, url) = protected_server(&user);
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "A notes server.", "url": url,
            "headers": {"Authorization": {"vault": "hippo-token"}}}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| visible(cx, "connection-notes"));
    click(cx, "connection-more-notes");
    click(cx, "review-connection-notes");
    until(&shell, cx, "what the review says", |cx| {
        present(cx, "connection-review-edit-entry") || present(cx, "connection-review-sign-in")
    });
    settle(cx);
    // The server refused what it was given: the entry is named, never its value, and the way out
    // is the entry, not a sign-in.
    assert!(!present(cx, "connection-review-sign-in"));
    assert!(!present(cx, "connection-review-error"));
    assert_eq!(
        label(cx, "connection-review-edit-entry").as_deref(),
        Some("Edit entry")
    );
    let said = label(cx, "connection-review-needs-entry").unwrap_or_default();
    assert!(
        said.contains("Authorization") && said.contains("hippo-token"),
        "{said}"
    );
    let all = everything_said(cx);
    assert!(!all.contains(SECRET), "{all}");
    assert!(!all.contains("connection login"), "{all}");
    assert!(!all.contains("sign in"), "{all}");
    // Edit entry opens the entry's dialog in the vault, its secret to type again.
    click(cx, "connection-review-edit-entry");
    let vault = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .settings_page()
            .map(|page| page.read(cx).vault().clone())
            .unwrap()
    });
    until(&shell, cx, "the entry's dialog", |cx| {
        vault.read_with(cx, |v, _| v.is_open())
    });
    let _ = server.kill();
    let _ = server.wait();
}

#[gpui_kit::test]
fn a_401_with_query_and_cookie_secrets_offers_to_edit_each_entry(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    for name in ["notes-query", "notes-cookie", "notes-login"] {
        user.call(
            "vault.set",
            json!({"name": name, "secret": SECRET, "description": "Notes' secret",
                "username": "ada@example.com"}),
        )
        .unwrap();
    }
    let (mut server, url) = protected_server(&user);
    // The cookie's entry is used twice (a cookie and a query parameter): offered once.
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "A notes server.", "url": url,
            "secrets": [{"in": "query", "name": "userToken", "entry": "notes-query"},
                {"in": "cookie", "name": "session", "entry": "notes-cookie"},
                {"in": "query", "name": "again", "entry": "notes-cookie"},
                {"in": "basic", "entry": "notes-login"}]}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| visible(cx, "connection-notes"));
    click(cx, "connection-more-notes");
    click(cx, "review-connection-notes");
    until(&shell, cx, "what the review says", |cx| {
        present(cx, "connection-review-needs-entry")
            || present(cx, "connection-review-sign-in")
            || present(cx, "connection-review-error")
    });
    settle(cx);
    assert!(present(cx, "connection-review-needs-entry"));
    // Each entry the server refused is offered, not only a header's.
    for entry in ["notes-query", "notes-cookie", "notes-login"] {
        assert_eq!(
            label(cx, &format!("connection-review-edit-{entry}")),
            Some(format!("Edit {entry}")),
            "{}",
            everything_said(cx)
        );
    }
    let all = everything_said(cx);
    assert_eq!(
        all.matches("Edit notes-cookie").count(),
        1,
        "an entry used twice is offered once: {all}"
    );
    assert!(!all.contains(SECRET), "{all}");
    let _ = server.kill();
    let _ = server.wait();
}

/// The secret the faulty URL of `design` holds, which the window never shows nor writes back.
const IN_A_URL: &str = "Zq7Wx3Vp9";

#[gpui_kit::test]
fn editing_a_faulty_url_leaves_its_field_empty_and_says_why(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let path = user.path("config/flottille/extensions.toml");
    let mut file = std::fs::read_to_string(&path).unwrap_or_default();
    file.push_str(&format!(
        "\n[[connections.server]]\nname = \"design\"\nabout = \"Designs.\"\n\
         url = \"https://design.example.com/mcp?project=p&access_token={IN_A_URL}#k\"\n"
    ));
    std::fs::write(&path, file).unwrap();
    let (shell, cx) = window(&user, cx);
    open_the_form_of(&shell, cx, "design");
    let section = section(&shell, cx);
    let fields = section.read_with(cx, |s, _| s.form_fields().unwrap());
    // The field is left empty, its fault beside it: what was served cleaned is never saved back.
    assert_eq!(fields.url.read_with(cx, |f, _| f.value().to_string()), "");
    let fault = label(cx, "connection-url-field-error").unwrap_or_default();
    assert!(fault.contains("access_token"), "{fault}");
    assert!(!everything_said(cx).contains(IN_A_URL));
    type_in(&fields.about, "The designs.", cx);
    click(cx, "save-connection");
    assert!(visible(cx, "connection-modal"));
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains(&format!("project=p&access_token={IN_A_URL}#k")),
        "{written}"
    );
    // Typed again, the fault folds away, over a panel's time.
    let whole = height(cx, "connection-url-field-error-fold");
    type_in(&fields.url, "https://design.example.com/mcp?project=p", cx);
    a_third_later(cx);
    let folding = height(cx, "connection-url-field-error-fold");
    assert!(
        0. < folding && folding < whole,
        "the fault folds away: {folding} of {whole}"
    );
    settle(cx);
    assert!(!present(cx, "connection-url-field-error"));
    click(cx, "save-connection");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });
    let design = connection(&user, "design").unwrap();
    assert_eq!(design["url"], "https://design.example.com/mcp?project=p");
    assert_eq!(design["about"], "The designs.");
}

// ── The review opens at rest (#954) ──────────────────────────────────────────────────────────

/// The user of a re-review: `driftbrowser` pinned with settings, then announcing one more tool,
/// one fewer and a few changed, so that "What changed" and the unchanged line both show.
fn rereview_user() -> User {
    let user = user();
    turn_on(&user);
    connect(
        &user,
        "driftbrowser",
        &driftbrowser(),
        json!({"browser_snapshot": "passes", "browser_upload": "asked"}),
    );
    let mut now = driftbrowser();
    now["server"]["version"] = json!("0.10.0");
    let tools = now["tools"].as_array_mut().unwrap();
    tools.retain(|t| t["name"] != "browser_pdf");
    for tool in tools.iter_mut() {
        if tool["name"] == "browser_click" {
            tool["description"] = json!("Click an element, then snapshot.");
        }
    }
    tools.push(
        json!({"name": "browser_wait_for_text", "description": "Wait until a text appears.",
        "inputSchema": {"type": "object", "properties": {}}}),
    );
    write_script(&user, "driftbrowser", &now);
    user
}

/// What the window drew from the click on Review of `name` until the data was drawn and for two
/// panels after it, one frame (16 ms) at a time.
struct Opening {
    /// Per frame before the data: whether the loading state was drawn.
    loading: Vec<bool>,
    /// Per frame from the one with the data: whether the loading state was still drawn.
    skeleton: Vec<bool>,
    /// Per frame from the one with the data: the height drawn of each of the `watched` ids.
    heights: Vec<Vec<f32>>,
    /// Per frame from the one with the data: the height drawn of each of the `hidden` ids.
    hidden: Vec<Vec<f32>>,
}

fn open_frame_by_frame(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    name: &str,
    ready: &str,
    watched: &[&str],
    hidden: &[&str],
) -> Opening {
    open_frame_by_frame_while(shell, cx, name, ready, watched, hidden, |_| {})
}

/// `open_frame_by_frame`, with `while_asked` called after each frame drawn before the data, with
/// the number of those frames so far: a test that holds the server's answer releases it there.
fn open_frame_by_frame_while(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    name: &str,
    ready: &str,
    watched: &[&str],
    hidden: &[&str],
    mut while_asked: impl FnMut(usize),
) -> Opening {
    open_page(shell, cx);
    until(shell, cx, "its row", |cx| {
        visible(cx, &format!("connection-{name}"))
    });
    click(cx, &format!("connection-more-{name}"));
    click(cx, &format!("review-connection-{name}"));
    let mut opening = Opening {
        loading: Vec::new(),
        skeleton: Vec::new(),
        heights: Vec::new(),
        hidden: Vec::new(),
    };
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut frames_after = 0;
    while frames_after * 16 < 2 * PANEL.as_millis() as u64 {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let skeleton = present(cx, "connection-review-skeleton-0");
        if !present(cx, ready) {
            assert!(Instant::now() < deadline, "the data never came");
            opening.loading.push(skeleton);
            while_asked(opening.loading.len());
            std::thread::sleep(Duration::from_millis(25));
            continue;
        }
        frames_after += 1;
        opening.skeleton.push(skeleton);
        opening
            .heights
            .push(watched.iter().map(|id| height(cx, id)).collect());
        opening
            .hidden
            .push(hidden.iter().map(|id| height(cx, id)).collect());
    }
    opening
}

/// Whether every column of `frames` is the same height on each frame: nothing folds or unfolds.
fn steady(ids: &[&str], frames: &[Vec<f32>]) -> Result<(), String> {
    let last = frames.last().expect("a frame with the data");
    for (at, frame) in frames.iter().enumerate() {
        for (id, (seen, rest)) in ids.iter().zip(frame.iter().zip(last)) {
            if (seen - rest).abs() > 0.5 {
                return Err(format!(
                    "{id} is {seen}px high on frame {at} and {rest}px at rest"
                ));
            }
        }
    }
    Ok(())
}

/// Frames asked for over a second of the clock once everything settled.
fn frames_asked_at_rest(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> usize {
    let mut asked = 0;
    for _ in 0..60 {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        asked += cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    asked
}

#[gpui_kit::test]
fn the_review_opens_in_its_resting_state(cx: &mut TestAppContext) {
    let user = rereview_user();
    let (shell, cx) = window(&user, cx);
    let watched = [
        "connection-review-changes",
        "connection-review-change-browser_click",
        "connection-review-change-browser_wait_for_text",
        "connection-review-change-browser_pdf",
        "connection-review-unchanged",
    ];
    let hidden = [
        "connection-review-unchanged-body",
        "connection-review-none-block",
        "connection-review-row-1-browser_snapshot",
        "connection-review-row-2-browser_upload",
    ];
    let opening = open_frame_by_frame(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-change-browser_click",
        &watched,
        &hidden,
    );
    // The first frame with the data is the resting state: the unchanged group is closed on it and
    // on every frame after, and no block folds or unfolds.
    for (at, frame) in opening.hidden.iter().enumerate() {
        assert!(
            frame.iter().all(|h| *h == 0.),
            "frame {at}: the unchanged group and the empty blocks have no height: {frame:?}"
        );
    }
    steady(&watched, &opening.heights).unwrap();
    assert!(
        opening.heights[0].iter().all(|h| *h > 0.),
        "the first frame draws the rows: {:?}",
        opening.heights[0]
    );
    // Then the window draws no frame and asks for none.
    settle(cx);
    assert_eq!(frames_asked_at_rest(&shell, cx), 0);
}

#[gpui_kit::test]
fn a_first_review_opens_in_its_resting_state_too(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "almanac");
    write_script(&user, "almanac", &almanac());
    let (shell, cx) = window(&user, cx);
    let watched = [
        "connection-review-group-judged",
        "connection-review-row-3-search",
        "connection-review-row-3-write",
        "connection-review-row-3-open",
    ];
    let opening = open_frame_by_frame(
        &shell,
        cx,
        "almanac",
        "connection-review-row-3-search",
        &watched,
        &["connection-review-none-block"],
    );
    steady(&watched, &opening.heights).unwrap();
    assert!(opening.heights[0].iter().all(|h| *h > 0.));
    assert!(opening.hidden.iter().flatten().all(|h| *h == 0.));
    settle(cx);
    assert_eq!(frames_asked_at_rest(&shell, cx), 0);
}

#[gpui_kit::test]
fn opening_a_group_of_the_review_still_unfolds_it(cx: &mut TestAppContext) {
    let user = rereview_user();
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-change-browser_click",
    );
    assert_eq!(height(cx, "connection-review-unchanged-body"), 0.);
    press(cx, "connection-review-unchanged");
    a_third_later(cx);
    let moving = height(cx, "connection-review-unchanged-body");
    settle(cx);
    let open = height(cx, "connection-review-unchanged-body");
    assert!(open > 0., "the group is open");
    assert!(
        moving > 0. && moving < open,
        "it unfolds over a panel: {moving}px of {open}px a third in"
    );
    // And "What changed" still folds on its header: its body, which clips the rows, is neither at
    // its full height nor gone a third in (a row's own frame keeps its height while it clips).
    let whole = height(cx, "connection-review-changes-body");
    assert!(whole > 0., "the group is open");
    press(cx, "connection-review-changes");
    a_third_later(cx);
    let folding = height(cx, "connection-review-changes-body");
    assert!(
        0. < folding && folding < whole,
        "the group folds over a panel: {folding}px of {whole}px a third in"
    );
}

/// Frames drawn while the held server is asked, before the test lets its listing go.
const LOADING_FRAMES: usize = 10;

#[gpui_kit::test]
fn a_review_whose_server_is_asked_shows_one_loading_state_then_fades_to_its_content(
    cx: &mut TestAppContext,
) {
    let user = user();
    turn_on(&user);
    declare_waiting(&user, "almanac");
    // The server holds its listing until the test lets it go, so the frames drawn while it is
    // asked do not depend on how long the machine takes to draw them.
    let release = user.path("almanac-release");
    let mut held = almanac();
    held["list_when"] = json!(release.display().to_string());
    write_script(&user, "almanac", &held);
    let (shell, cx) = window(&user, cx);
    let watched = [
        "connection-review-group-judged",
        "connection-review-row-3-search",
    ];
    let opening = open_frame_by_frame_while(
        &shell,
        cx,
        "almanac",
        "connection-review-row-3-search",
        &watched,
        &[],
        |asked| {
            if asked == LOADING_FRAMES {
                std::fs::write(&release, "").unwrap();
            }
        },
    );
    // One loading state while the server is asked, from the first frame to the data.
    assert!(
        opening.loading.len() >= LOADING_FRAMES,
        "the server was asked: {} frames drawn before its answer",
        opening.loading.len()
    );
    assert!(
        opening.loading.iter().all(|shown| *shown),
        "{:?}",
        opening.loading
    );
    // Its replacement is one fade: the loading state leaves once, never comes back, and the
    // blocks of the content are at their size throughout.
    let gone = opening.skeleton.iter().position(|shown| !shown);
    assert!(gone.is_some(), "the loading state is gone after two panels");
    assert!(
        opening.skeleton[gone.unwrap()..].iter().all(|shown| !shown),
        "{:?}",
        opening.skeleton
    );
    steady(&watched, &opening.heights).unwrap();
}

/// The heights drawn now of the rows of the form's three lists (the elements that clip them as
/// they unfold), by the keys the form gave them: only those that are drawn.
fn form_rows(cx: &mut VisualTestContext) -> Vec<(String, f32)> {
    let mut rows = Vec::new();
    for kind in ["variable", "secret", "limit"] {
        for key in 1..=16 {
            let id = format!("connection-{kind}-item-{key}");
            if present(cx, &id) {
                rows.push((id.clone(), height(cx, &id)));
            }
        }
    }
    rows
}

#[gpui_kit::test]
fn the_connection_form_opens_in_its_resting_state(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    user.call(
        "vault.set",
        json!({"name": "browser-token", "secret": SECRET, "description": "The browser's token"}),
    )
    .unwrap();
    agent_browser(&user, &["agent_browser_click"], false);
    let token = json!({"vault": "browser-token"});
    user.call(
        "connection.edit",
        json!({"name": "browser",
            "env": {"BROWSER_TOKEN": token, "BROWSER_KEY": token, "BROWSER_SEAT": token},
            "only": {"url": ["https://shop.example.com"], "app": ["gedit"]}}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    until(&shell, cx, "its row", |cx| {
        visible(cx, "connection-browser")
    });
    click(cx, "connection-more-browser");
    press(cx, "edit-connection-browser");

    // What the window draws from the click on Edit, a frame (16 ms) at a time, until the dialog
    // and its fade are done: every row of the three lists is at its height whenever it is drawn.
    let mut frames = Vec::new();
    for _ in 0..(2 * PANEL.as_millis() as u64 / 16) {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        frames.push(form_rows(cx));
    }
    let rest = frames.last().unwrap().clone();
    assert_eq!(rest.len(), 5, "3 variables, 2 limits: {rest:?}");
    for (id, rest) in &rest {
        assert!(*rest > 0., "{id} is drawn at rest");
    }
    for (at, frame) in frames.iter().enumerate() {
        for (id, seen) in frame {
            let rest = rest.iter().find(|(other, _)| other == id).unwrap().1;
            assert!(
                (seen - rest).abs() < 0.5,
                "{id} is {seen}px high on frame {at} and {rest}px at rest"
            );
        }
    }

    // A row added once the form is open still unfolds.
    press(cx, "connection-new-variable");
    a_third_later(cx);
    let arriving = form_rows(cx)
        .into_iter()
        .filter(|(id, _)| id.starts_with("connection-variable-item-"))
        .filter(|(id, _)| !rest.iter().any(|(old, _)| old == id))
        .collect::<Vec<_>>();
    assert_eq!(arriving.len(), 1, "one new variable row: {arriving:?}");
    let moving = arriving[0].1;
    let whole = rest
        .iter()
        .find(|(id, _)| id.starts_with("connection-variable-item-"))
        .unwrap()
        .1;
    assert!(
        0. < moving && moving < whole,
        "a row added after the form opened unfolds: {moving}px of {whole}px a third in"
    );
}

#[gpui_kit::test]
fn a_row_that_moves_after_the_review_opened_still_unfolds(cx: &mut TestAppContext) {
    let user = rereview_user();
    let (shell, cx) = window(&user, cx);
    open_the_review(
        &shell,
        cx,
        "driftbrowser",
        "connection-review-change-browser_click",
    );
    // The unchanged group holds its rows (browser_snapshot is "passes", browser_upload "asked"):
    // open it, then give browser_open another setting, so that it reaches a group that has no
    // rest of its own: a slot that is new after the review opened.
    press(cx, "connection-review-unchanged");
    settle(cx);
    let row = row_of(cx, "browser_open");
    assert_eq!(group_of(cx, "browser_open"), Some("judged"));
    let whole = height(cx, &row);
    assert!(whole > 0.);
    click_pill(cx, &row, "off");
    a_third_later(cx);
    let arriving = height(cx, "connection-review-row-2-browser_open");
    assert!(
        0. < arriving && arriving < whole,
        "it unfolds in its new group over a panel's time: {arriving}px of {whole}px"
    );
    settle(cx);
    assert_eq!(group_of(cx, "browser_open"), Some("off"));
    let rest = height(cx, "connection-review-row-2-browser_open");
    assert!(
        (rest - whole).abs() < 0.5,
        "{rest}px at rest, {whole}px before"
    );
}

// ── The secrets of a URL's requests (#1015) ───────────────────────────────────────────────────

/// The vault entries the secrets below name, none of them bound to an address.
fn notes_entries(user: &User) {
    for name in ["notes-plain", "notes-key", "notes-cookie", "notes-query"] {
        user.call("vault.set", json!({"name": name, "secret": SECRET}))
            .unwrap();
    }
    user.call(
        "vault.set",
        json!({"name": "notes-basic", "secret": SECRET, "username": "ana"}),
    )
    .unwrap();
}

/// The connection `remote` of the user with a plain header and one secret of each place.
fn remote_with_every_secret(user: &User) -> Value {
    notes_entries(user);
    user.call(
        "connection.add",
        json!({"name": "remote", "about": "A notes server elsewhere.",
        "url": "https://notes.example.com/mcp",
        "headers": {"X-Plain": {"vault": "notes-plain"}},
        "secrets": [
            {"in": "header", "name": "X-Api-Auth", "prefix": "Bearer ", "entry": "notes-key"},
            {"in": "query", "name": "workspace", "entry": "notes-query"},
            {"in": "cookie", "name": "visit", "entry": "notes-cookie"},
            {"in": "basic", "entry": "notes-basic"},
        ]}),
    )
    .unwrap();
    connection(user, "remote").unwrap()
}

/// Opens the form of `remote` and waits for the `lines` of its secrets.
fn edit_remote(shell: &Entity<Shell>, cx: &mut VisualTestContext, lines: usize) {
    until(shell, cx, "its row", |cx| visible(cx, "connection-remote"));
    click(cx, "connection-more-remote");
    click(cx, "edit-connection-remote");
    until(shell, cx, "its secrets are in the form", |cx| {
        (0..lines).all(|at| visible(cx, &format!("connection-secret-{at}-entry")))
    });
    settle(cx);
}

fn save_and_close(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    click(cx, "save-connection");
    until(shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-modal")
    });
}

#[gpui_kit::test]
fn a_connection_s_secrets_are_shown_and_kept_by_an_edit(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    let before = remote_with_every_secret(&user);
    assert_eq!(before["secrets"].as_array().unwrap().len(), 4, "{before}");
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    edit_remote(&shell, cx, 5);

    // The old header line first, as a header with no prefix, then each secret in its order.
    let places: Vec<_> = (0..5)
        .map(|at| label(cx, &format!("connection-secret-{at}-place")).unwrap_or_default())
        .collect();
    assert_eq!(
        places,
        [
            "Header",
            "Header",
            "Query parameter",
            "Cookie",
            "Basic authentication"
        ]
    );
    let entries: Vec<_> = (0..5)
        .map(|at| label(cx, &format!("connection-secret-{at}-entry")).unwrap_or_default())
        .collect();
    assert_eq!(
        entries,
        [
            "notes-plain",
            "notes-key",
            "notes-query",
            "notes-cookie",
            "notes-basic"
        ]
    );
    let section = section(&shell, cx);
    let (name, prefix) = section.read_with(cx, |s, _| s.secret_fields(1).unwrap());
    assert_eq!(
        name.read_with(cx, |f, _| f.value().to_string()),
        "X-Api-Auth"
    );
    assert_eq!(
        prefix.read_with(cx, |f, _| f.value().to_string()),
        "Bearer "
    );
    assert!(!everything_said(cx).contains(SECRET));

    // A save that changes nothing leaves the declaration, and so its digest, as it was.
    save_and_close(&shell, cx);
    assert_eq!(connection(&user, "remote").unwrap(), before);
}

#[gpui_kit::test]
fn old_header_lines_are_written_back_as_they_were(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    user.call("vault.set", json!({"name": "notes-key", "secret": SECRET}))
        .unwrap();
    user.call(
        "connection.add",
        json!({"name": "remote", "about": "A notes server elsewhere.",
            "url": "https://notes.example.com/mcp",
            "headers": {"X-Notes-Key": {"vault": "notes-key"}}}),
    )
    .unwrap();
    let before = connection(&user, "remote").unwrap();
    assert!(before.get("secrets").is_none(), "{before}");
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    edit_remote(&shell, cx, 1);
    assert_eq!(
        label(cx, "connection-secret-0-place").as_deref(),
        Some("Header")
    );
    save_and_close(&shell, cx);
    let after = connection(&user, "remote").unwrap();
    // Still `headers`, and no `secrets` of its own: what a pin's digest is made of.
    assert_eq!(
        after["headers"],
        json!({"X-Notes-Key": {"vault": "notes-key"}})
    );
    assert!(after.get("secrets").is_none(), "{after}");
    assert_eq!(after, before);
}

#[gpui_kit::test]
fn a_secret_can_be_added_and_removed_in_the_form(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    remote_with_every_secret(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    edit_remote(&shell, cx, 5);
    let section = section(&shell, cx);

    // A new line unfolds, with every control labelled.
    press(cx, "connection-new-secret");
    a_third_later(cx);
    let arriving: Vec<_> = form_rows(cx)
        .into_iter()
        .filter(|(id, _)| id.starts_with("connection-secret-item-"))
        .collect();
    assert_eq!(arriving.len(), 6, "{arriving:?}");
    let (whole, moving) = (arriving[0].1, arriving[5].1);
    assert!(
        0. < moving && moving < whole,
        "a line added unfolds: {moving}px of {whole}px a third in"
    );
    settle(cx);
    for (control, field, said) in [
        ("place", "place-field", "Place"),
        ("name", "name-field", "Name"),
        ("prefix", "prefix-field", "Prefix"),
        ("entry", "entry-field", "Vault entry"),
    ] {
        let id = format!("connection-secret-5-{control}");
        assert!(visible(cx, &id), "{id} is drawn");
        assert_eq!(
            label(cx, &format!("connection-secret-5-{field}-label")).as_deref(),
            Some(said),
            "{id}"
        );
    }
    click(cx, "connection-secret-5-place");
    click(cx, "connection-secret-5-place-query");
    let (name, _) = section.read_with(cx, |s, _| s.secret_fields(5).unwrap());
    type_in(&name, "team", cx);
    click(cx, "connection-secret-5-entry");
    click(cx, "connection-secret-5-entry-notes-key");

    // A line removed folds away, and is not written.
    click(cx, "connection-secret-1-remove");
    until(&shell, cx, "the line is gone", |cx| {
        !present(cx, "connection-secret-5-name")
    });
    save_and_close(&shell, cx);
    let after = connection(&user, "remote").unwrap();
    assert_eq!(
        after["secrets"],
        json!([
            {"in": "query", "name": "workspace", "entry": "notes-query"},
            {"in": "cookie", "name": "visit", "entry": "notes-cookie"},
            {"in": "basic", "entry": "notes-basic"},
            {"in": "query", "name": "team", "entry": "notes-key"},
        ]),
        "{after}"
    );
    assert_eq!(
        after["headers"],
        json!({"X-Plain": {"vault": "notes-plain"}})
    );
}

#[gpui_kit::test]
fn a_secret_s_name_and_prefix_fold_away_where_they_do_not_apply(cx: &mut TestAppContext) {
    let user = user();
    turn_on(&user);
    remote_with_every_secret(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    edit_remote(&shell, cx, 5);
    let section = section(&shell, cx);
    // A basic line has no name; only a header has a prefix.
    assert!(!visible(cx, "connection-secret-4-name"));
    assert!(!visible(cx, "connection-secret-2-prefix"));
    assert!(visible(cx, "connection-secret-1-prefix"));

    // The bearer header moved to a query parameter: its prefix folds away, and is not kept.
    let whole = height(cx, "connection-secret-1-prefix-block");
    click(cx, "connection-secret-1-place");
    press(cx, "connection-secret-1-place-query");
    a_third_later(cx);
    let folding = height(cx, "connection-secret-1-prefix-block");
    assert!(
        0. < folding && folding < whole,
        "the prefix folds away: {folding}px of {whole}px a third in"
    );
    settle(cx);
    assert!(!visible(cx, "connection-secret-1-prefix"));
    assert!(visible(cx, "connection-secret-1-name"));
    let (_, prefix) = section.read_with(cx, |s, _| s.secret_fields(1).unwrap());
    assert_eq!(prefix.read_with(cx, |f, _| f.value().to_string()), "");

    // A cookie made basic: its name folds away and is not kept; the basic line made a cookie
    // unfolds a name to type.
    click(cx, "connection-secret-3-place");
    click(cx, "connection-secret-3-place-basic");
    assert!(!visible(cx, "connection-secret-3-name"));
    let (name, _) = section.read_with(cx, |s, _| s.secret_fields(3).unwrap());
    assert_eq!(name.read_with(cx, |f, _| f.value().to_string()), "");
    click(cx, "connection-secret-4-place");
    click(cx, "connection-secret-4-place-cookie");
    assert!(visible(cx, "connection-secret-4-name"));
    assert!(!visible(cx, "connection-secret-4-prefix"));
    let (name, _) = section.read_with(cx, |s, _| s.secret_fields(4).unwrap());
    type_in(&name, "session", cx);

    save_and_close(&shell, cx);
    let after = connection(&user, "remote").unwrap();
    assert_eq!(
        after["secrets"],
        json!([
            {"in": "query", "name": "X-Api-Auth", "entry": "notes-key"},
            {"in": "query", "name": "workspace", "entry": "notes-query"},
            {"in": "basic", "entry": "notes-cookie"},
            {"in": "cookie", "name": "session", "entry": "notes-basic"},
        ]),
        "{after}"
    );
}

// ── A dialog opens over the page it is asked from (#1018) ─────────────────────────────────────

fn stack(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<&'static str> {
    shell.read_with(cx, |s, cx| s.modal_stack(cx))
}

#[gpui_kit::test]
fn the_vault_page_and_the_connections_page_open_their_forms_through_the_host(
    cx: &mut TestAppContext,
) {
    let user = user();
    turn_on(&user);
    let (shell, cx) = window(&user, cx);
    open_page(&shell, cx);
    assert!(stack(&shell, cx).is_empty(), "nothing is open yet");

    // Connections' own "New server": the host holds its form.
    click(cx, "new-connection");
    until(&shell, cx, "the form opens", |cx| {
        visible(cx, "connection-modal")
    });
    assert_eq!(stack(&shell, cx), ["connection-modal"]);
    click(cx, "connection-modal-cancel");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert!(stack(&shell, cx).is_empty(), "closed, it leaves the host");

    // The vault page's "New entry", then a row's Edit: the host holds the entry's dialog.
    user.call(
        "vault.set",
        json!({"name": "jira", "secret": "pa55-for-the-test"}),
    )
    .unwrap();
    click(cx, "settings-tabs-vault");
    until(&shell, cx, "the vault page is shown", |cx| {
        present(cx, "add-vault-entry")
    });
    click(cx, "add-vault-entry");
    until(&shell, cx, "the dialog opens", |cx| {
        visible(cx, "vault-modal")
    });
    assert_eq!(stack(&shell, cx), ["vault-modal"]);
    click(cx, "vault-modal-cancel");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "vault-modal")
    });
    assert!(stack(&shell, cx).is_empty());
    until(&shell, cx, "the entry is listed", |cx| {
        present(cx, "vault-jira")
    });
    click(cx, "vault-jira");
    until(&shell, cx, "its dialog opens", |cx| {
        visible(cx, "vault-modal")
    });
    assert_eq!(stack(&shell, cx), ["vault-modal"]);
}
