//! An agent's request to write a connection in the window (#854, `docs/extensions.md`,
//! "Connections"): the held request waits in the rail as every request does, and opens the
//! validation dialog: the fields of "New server", filled with what it would write and read-only,
//! an edit's changed fields with their old value, then Accept, Edit and Refuse. On GPUI's test
//! platform in front of a real daemon with the fake ACP agent and the fake MCP server: no real
//! server, the in-memory keyring, temporary folders, no window on a screen.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_window::Shell;
use gpui_design_system::motion::PANEL;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn fake_mcp() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(path.exists(), "{} is missing", path.display());
    path
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn almanac() -> Value {
    json!({
        "server": {"name": "almanac", "version": "1.0.0"},
        "instructions": "Search before you write.",
        "tools": [
            {"name": "search", "description": "Finds notes by words.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}},
        ],
        "answers": {"search": {"text": "found"}},
    })
}

/// The fake server's command line for `name`, its script written.
fn line_of(user: &User, name: &str) -> String {
    let script = user.path(&format!("{name}-script.json"));
    std::fs::write(&script, almanac().to_string()).unwrap();
    format!(
        "{} --script {} --log {}",
        fake_mcp().display(),
        script.display(),
        user.path(&format!("{name}-log.jsonl")).display()
    )
}

fn connection(user: &User, name: &str) -> Option<Value> {
    user.call("connection.list", json!({})).unwrap()["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
}

fn pending(user: &User) -> usize {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .len()
}

/// Connections on, a project and its root's session: the root's id and its MCP client.
fn a_root(user: &User) -> (String, Mcp) {
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
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
    let (url, authorization) = server_of(user, opened);
    (lead, Mcp::initialize(&url, &authorization))
}

fn held(mcp: &Mcp, args: Value) -> u64 {
    let answer = mcp.call("connections", args);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..60 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn a_third_later(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.executor().advance_clock(PANEL / 3);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn height(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
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

fn press(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, id: &str) {
    press(cx, id);
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

/// The session of `lead` open, its request `number`'s card in the rail.
fn open_card(shell: &Entity<Shell>, cx: &mut VisualTestContext, lead: &str, number: u64) {
    until(shell, cx, "the session is in the tree", |cx| {
        present(cx, &format!("session-{lead}"))
    });
    click(cx, &format!("session-{lead}"));
    until(shell, cx, "the card", |cx| {
        present(cx, &format!("approval-title-{number}"))
    });
    settle(cx);
}

/// The card's Open: the validation dialog, its declaration read.
fn open_dialog(shell: &Entity<Shell>, cx: &mut VisualTestContext, number: u64) {
    click(cx, &format!("open-request-{number}"));
    until(shell, cx, "the declaration is read", |cx| {
        visible(cx, "connection-request-name")
    });
    settle(cx);
}

#[gpui_kit::test]
fn an_agents_request_opens_as_the_form_filled_and_read_only_and_accept_writes_it(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let line = line_of(&user, "almanac");
    let number = held(
        &mcp,
        json!({"action": "add", "name": "almanac", "about": "The notes of the user.",
            "command": line, "only": {"query": ["login"]}}),
    );

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    // A connection's request is never allowed from the rail: its card opens the dialog.
    assert!(!present(cx, &format!("allow-{number}")));
    assert!(present(cx, &format!("deny-{number}")));
    press(cx, &format!("open-request-{number}"));
    until(&shell, cx, "the dialog opens", |cx| {
        present(cx, "connection-request")
    });
    // What the daemon answers crossfades from its skeleton.
    until(&shell, cx, "the declaration begins to show", |cx| {
        height(cx, "connection-request-fields") > 0.
    });
    a_third_later(cx);
    let showing = height(cx, "connection-request-fields");
    settle(cx);
    let shown = height(cx, "connection-request-fields");
    assert!(
        0. < showing && showing < shown,
        "the fields come over a panel's time: {showing} of {shown}"
    );

    assert_eq!(
        label(cx, "connection-request-title").as_deref(),
        Some("Add the server almanac")
    );
    assert_eq!(
        label(cx, "connection-request-name").as_deref(),
        Some("almanac")
    );
    assert_eq!(
        label(cx, "connection-request-about").as_deref(),
        Some("The notes of the user.")
    );
    assert_eq!(
        label(cx, "connection-request-reach").as_deref(),
        Some("A program")
    );
    let command = label(cx, "connection-request-command").unwrap_or_default();
    assert!(command.contains("--script"), "the command whole: {command}");
    assert_eq!(
        label(cx, "connection-request-limit-query").as_deref(),
        Some("query: login")
    );
    // Read-only: no field of the form is there to type in.
    assert!(!present(cx, "connection-name"));
    assert!(!present(cx, "connection-command"));
    assert!(visible(cx, "connection-request-edit"));
    assert!(visible(cx, "connection-request-refuse"));

    click(cx, "connection-request-accept");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-request")
    });
    until(&shell, cx, "the toast", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "Request accepted: the server almanac is being added")
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while connection(&user, "almanac").is_none_or(|c| c["state"] != "ready") {
        assert!(Instant::now() < deadline, "never written");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[gpui_kit::test]
fn an_edit_shows_its_differences_in_the_dialog(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let line = line_of(&user, "almanac");
    user.call(
        "connection.add",
        json!({"name": "almanac", "about": "The notes of the user.",
            "command": flottille_core::command::split_line(&line).unwrap()}),
    )
    .unwrap();
    let number = held(
        &mcp,
        json!({"action": "edit", "name": "almanac", "about": "The user's decisions."}),
    );

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);

    assert_eq!(
        label(cx, "connection-request-title").as_deref(),
        Some("Edit the server almanac")
    );
    assert_eq!(
        label(cx, "connection-request-about").as_deref(),
        Some("The user's decisions.")
    );
    assert_eq!(
        label(cx, "connection-request-was-about").as_deref(),
        Some("The notes of the user.")
    );
    assert!(visible(cx, "connection-request-changed-about"));
    // What it does not change has no old value and no tag.
    assert!(!present(cx, "connection-request-was-command"));
    assert!(!present(cx, "connection-request-changed-command"));
}

#[gpui_kit::test]
fn refuse_writes_nothing_without_a_confirmation(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let line = line_of(&user, "almanac");
    let number = held(
        &mcp,
        json!({"action": "add", "name": "almanac", "about": "Notes.", "command": line}),
    );

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    // Refusing writes nothing, as the rail's Deny: it is not confirmed.
    click(cx, "connection-request-refuse");
    assert!(!present(cx, "connection-request-refusal"));
    until(&shell, cx, "the toast", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "Request refused: nothing was written")
    });
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-request")
    });
    assert_eq!(pending(&user), 0);
    assert!(connection(&user, "almanac").is_none());
}

#[gpui_kit::test]
fn edit_opens_the_servers_form_with_the_agents_values(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let line = line_of(&user, "almanac");
    let number = held(
        &mcp,
        json!({"action": "add", "name": "almanac", "about": "Notes.", "command": line}),
    );

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    click(cx, "connection-request-edit");
    until(&shell, cx, "the form opens", |cx| {
        visible(cx, "connection-modal")
    });
    settle(cx);
    assert!(!visible(cx, "connection-request"));
    let section = shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).settings_page().cloned().unwrap();
        page.read(cx).connections().clone()
    });
    let fields = section.read_with(cx, |s, _| s.form_fields().unwrap());
    let about = fields.about.read_with(cx, |f, _| f.value().to_string());
    assert_eq!(about, "Notes.", "the agent's values");
    let command = fields.command.read_with(cx, |f, _| f.value().to_string());
    assert!(command.contains("--script"), "{command}");

    cx.update(|window, cx| {
        fields.about.update(cx, |f, cx| {
            f.set_value("The notes of the user.", window, cx)
        })
    });
    click(cx, "save-connection");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(pending(&user), 0, "the request is answered");
    assert_eq!(
        connection(&user, "almanac").unwrap()["about"],
        "The notes of the user."
    );
}

#[gpui_kit::test]
fn a_request_changed_since_it_was_asked_offers_no_accept(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let line = line_of(&user, "almanac");
    user.call(
        "connection.add",
        json!({"name": "almanac", "about": "The notes of the user.",
            "command": flottille_core::command::split_line(&line).unwrap()}),
    )
    .unwrap();
    let number = held(
        &mcp,
        json!({"action": "edit", "name": "almanac", "about": "The user's decisions."}),
    );
    user.call(
        "connection.edit",
        json!({"name": "almanac", "only": {"query": ["login"]}}),
    )
    .unwrap();

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    let said = label(cx, "connection-request-stale").unwrap_or_default();
    assert!(said.contains("changed since"), "{said}");
    assert!(!present(cx, "connection-request-accept"));
}
