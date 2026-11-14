//! Connections in the window (#672, `docs/extensions.md`, "Connections"): a row of Settings ›
//! Features opening its page, its switch and usage line, the servers in the list pattern, the form
//! dialog that declares or edits one, the review of what a server announces in a dialog of a readable width, and
//! a call held for the user naming the connection. On GPUI's test platform in front of a real
//! daemon with the fake ACP agent and the fake MCP server (`crates/flottille-fake-mcp`): no real
//! server, the in-memory keyring, temporary folders, no window on a screen.

mod common;
mod guard;

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
    // A URL's requests carry headers, and say so.
    click(cx, "connection-reach-url");
    until(&shell, cx, "the URL's field unfolds", |cx| {
        visible(cx, "connection-url")
    });
    assert_eq!(
        label(cx, "connection-variable-0-name-field-label").as_deref(),
        Some("Header")
    );
    assert_eq!(
        label(cx, "connection-url-field-label").as_deref(),
        Some("URL")
    );
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
