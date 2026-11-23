//! An agent's request to write a connection in the window (#854, `docs/extensions.md`,
//! "Connections"): the held request waits in the rail as every request does, and opens the
//! validation dialog: the fields of "New server", filled with what it would write and read-only,
//! an edit's changed fields with their old value, then Accept, Edit and Refuse. On GPUI's test
//! platform in front of a real daemon with the fake ACP agent and the fake MCP server: no real
//! server, the in-memory keyring, temporary folders, no window on a screen.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::session::SessionPage;
use flottille_window::shell::Selection;
use flottille_window::vault::VaultSection;
use gpui_design_system::motion::PANEL;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
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
        if let Some(why) = label(cx, "connection-request-unread") {
            panic!("the request was not read: {why}");
        }
        visible(cx, "connection-request-name")
            || (present(cx, "connection-request-name") && {
                panic!("the dialog is drawn but not visible")
            })
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
    // The window reads what the daemon answers in its poll alone: without one, the dialog opens
    // on its skeleton, which nothing of the daemon's can have replaced yet.
    settle(cx);
    assert!(present(cx, "connection-request"), "the dialog opens");
    assert!(
        !present(cx, "connection-request-name"),
        "read without a poll"
    );
    assert!(height(cx, "connection-request-fields") > 0., "the skeleton");
    // What the daemon answers crossfades from its skeleton.
    until(&shell, cx, "the declaration begins to show", |cx| {
        present(cx, "connection-request-name")
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
            .any(|t| t == "Request accepted: the server almanac was added")
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

/// The secret the faulty URL of `design` holds, which the dialog never shows.
const IN_A_URL: &str = "Zq7Wx3Vp9";

#[gpui_kit::test]
fn the_dialog_marks_the_old_url_of_an_edit_served_cleaned(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let path = user.path("config/flottille/extensions.toml");
    let mut file = std::fs::read_to_string(&path).unwrap_or_default();
    file.push_str(&format!(
        "\n[[connections.server]]\nname = \"design\"\nabout = \"Designs.\"\n\
         url = \"https://design.example.com/mcp?project=p&access_token={IN_A_URL}\"\n"
    ));
    std::fs::write(&path, file).unwrap();
    let number = held(
        &mcp,
        json!({"action": "edit", "name": "design",
            "url": "https://design.example.com/mcp?project=p"}),
    );

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);

    assert_eq!(
        label(cx, "connection-request-url").as_deref(),
        Some("https://design.example.com/mcp?project=p")
    );
    // What was there is said cleaned, never as if it were the whole URL.
    assert_eq!(
        label(cx, "connection-request-was-url").as_deref(),
        Some("https://design.example.com/mcp (cleaned: its query is dropped)")
    );
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
fn the_card_shows_where_each_secret_goes(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    for (name, url) in [
        ("github-token", Some("https://api.github.com")),
        ("notes-token", Some("https://notes.example.com")),
        ("plain-token", None),
    ] {
        let mut params = json!({"name": name, "secret": "s3cret-t0ken-value"});
        if let Some(url) = url {
            params["url"] = json!(url);
        }
        user.call("vault.set", params).unwrap();
    }
    let line = line_of(&user, "almanac");
    let command = held(
        &mcp,
        json!({"action": "add", "name": "almanac", "about": "Notes.", "command": line,
            "env": {"GITHUB_TOKEN": "github-token", "PLAIN": "plain-token"}}),
    );
    let url = held(
        &mcp,
        json!({"action": "add", "name": "notes", "about": "Notes.",
            "url": "https://notes.example.com/mcp", "headers": {"Authorization": "notes-token"}}),
    );

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, command);
    open_dialog(&shell, cx, command);
    // A command's variables are its program's: each is said loudly, with the entry's address.
    let github = label(cx, "connection-request-secret-env-GITHUB_TOKEN").unwrap_or_default();
    assert!(
        github.contains("github-token") && github.contains("api.github.com"),
        "{github}"
    );
    assert!(github.contains("environment"), "{github}");
    let plain = label(cx, "connection-request-secret-env-PLAIN").unwrap_or_default();
    assert!(plain.contains("no address"), "{plain}");
    assert!(visible(
        cx,
        "connection-request-secret-env-GITHUB_TOKEN-elsewhere"
    ));
    assert!(visible(cx, "connection-request-secret-env-PLAIN-elsewhere"));
    assert!(visible(cx, "connection-request-secrets-field-warning"));

    shell.update(cx, |shell, cx| shell.open_connection_request(url, cx));
    until(&shell, cx, "the other request is read", |cx| {
        label(cx, "connection-request-name").as_deref() == Some("notes")
    });
    settle(cx);
    // A header of the entry's own host is said plainly.
    let header = label(cx, "connection-request-secret-headers-Authorization").unwrap_or_default();
    assert!(
        header.contains("notes-token") && header.contains("sent to notes.example.com"),
        "{header}"
    );
    assert!(!present(
        cx,
        "connection-request-secret-headers-Authorization-elsewhere"
    ));
    assert!(!visible(cx, "connection-request-secrets-field-warning"));
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
    let section = shell.read_with(cx, |s, cx| s.connections(cx));
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

// ── The secrets of a request, and the entry it names (#1015) ──────────────────────────────────

/// The secret of the vault entries below, which no window ever shows.
const SECRET: &str = "s3cret-notes-w1ndow";

fn connections_section(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::extensions::connections::ConnectionsSection> {
    shell.read_with(cx, |s, cx| s.connections(cx))
}

/// The agent's request to add `notes`, whose requests carry a bearer secret from `notes-key`.
fn a_bearer_request(mcp: &Mcp) -> u64 {
    held(
        mcp,
        json!({"action": "add", "name": "notes", "about": "Notes elsewhere.",
            "url": "https://notes.example.com/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "notes-key"}]}),
    )
}

#[gpui_kit::test]
fn editing_an_agent_s_request_keeps_its_secrets(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    user.call(
        "vault.set",
        json!({"name": "notes-key", "secret": SECRET, "url": "https://notes.example.com"}),
    )
    .unwrap();
    let number = a_bearer_request(&mcp);

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    click(cx, "connection-request-edit");
    until(&shell, cx, "the form opens with its secret", |cx| {
        visible(cx, "connection-modal") && visible(cx, "connection-secret-0-name")
    });
    settle(cx);
    assert_eq!(
        label(cx, "connection-secret-0-place").as_deref(),
        Some("Header")
    );
    assert_eq!(
        label(cx, "connection-secret-0-entry").as_deref(),
        Some("notes-key")
    );
    let section = connections_section(&shell, cx);
    let (name, prefix) = section.read_with(cx, |s, _| s.secret_fields(0).unwrap());
    assert_eq!(
        name.read_with(cx, |f, _| f.value().to_string()),
        "Authorization"
    );
    assert_eq!(
        prefix.read_with(cx, |f, _| f.value().to_string()),
        "Bearer "
    );

    click(cx, "save-connection");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(pending(&user), 0, "the request is answered");
    let written = connection(&user, "notes").unwrap();
    assert_eq!(
        written["secrets"],
        json!([{"in": "header", "name": "Authorization", "prefix": "Bearer ",
            "entry": "notes-key"}]),
        "{written}"
    );
    assert!(written.get("headers").is_none(), "{written}");
}

#[gpui_kit::test]
fn the_form_opened_from_a_request_says_it_answers_it_and_cancel_leaves_it_waiting(
    cx: &mut TestAppContext,
) {
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
    let title = label(cx, "connection-modal-title").unwrap_or_default();
    assert!(title.contains(&format!("#{number}")), "{title}");
    let said = label(cx, "connection-modal-sentence").unwrap_or_default();
    assert!(
        said.contains(&format!("request #{number}"))
            && said.contains("Save writes your values and answers it")
            && said.contains("Cancel leaves it waiting"),
        "{said}"
    );

    click(cx, "connection-modal-cancel");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(pending(&user), 1, "the request is still waiting");
    assert!(connection(&user, "almanac").is_none());
    // The form stayed over the session; the page is where it was, and a form opened on its own,
    // from Connections' page, is not said to answer anything.
    click(cx, "settings");
    until(&shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-features")
    });
    click(cx, "settings-tabs-features");
    until(&shell, cx, "Connections is a row", |cx| {
        present(cx, "features-connections")
    });
    click(cx, "features-connections");
    until(&shell, cx, "its page is in", |cx| {
        present(cx, "new-connection")
    });
    click(cx, "new-connection");
    until(&shell, cx, "the form opens", |cx| {
        visible(cx, "connection-modal")
    });
    let own = label(cx, "connection-modal-title").unwrap_or_default();
    assert!(!own.contains('#'), "{own}");
    click(cx, "connection-modal-cancel");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });

    // Back on the session, the request still waits under "Waiting for you", to be opened again.
    open_card(&shell, cx, &lead, number);
    assert_eq!(
        label(cx, "rail-waiting-title").as_deref(),
        Some("Waiting for you")
    );
    assert!(visible(cx, &format!("open-request-{number}")));
}

#[gpui_kit::test]
fn an_entry_without_an_address_is_not_said_to_go_to_another_host(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    // The entry the agent names is not made yet: it is bound to no address.
    let number = a_bearer_request(&mcp);

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    let line = label(cx, "connection-request-secret-headers-Authorization").unwrap_or_default();
    assert_eq!(
        line,
        "Authorization: the vault entry notes-key (bound to no address) will be sent to \
         notes.example.com"
    );
    assert!(
        !present(
            cx,
            "connection-request-secret-headers-Authorization-elsewhere"
        ),
        "not another host"
    );
    assert!(visible(
        cx,
        "connection-request-secret-headers-Authorization-unbound"
    ));
    let warning = label(cx, "connection-request-secrets-field-warning").unwrap_or_default();
    assert!(warning.contains("bound to no address"), "{warning}");
    assert!(!warning.contains("elsewhere than"), "{warning}");
}

#[gpui_kit::test]
fn a_missing_entry_is_made_from_the_request_s_dialog_then_accept_goes_through(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = a_bearer_request(&mcp);

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    assert!(!present(cx, "connection-request-new-entry-notes-key"));
    click(cx, "connection-request-accept");
    until(&shell, cx, "Accept says it waits for the entry", |cx| {
        shell.read_with(cx, |s, cx| {
            s.connection_request()
                .read(cx)
                .error()
                .is_some_and(|said| said.contains("the vault entry notes-key is not saved yet"))
        })
    });
    assert_eq!(pending(&user), 1, "nothing was answered");
    until(&shell, cx, "the button for the entry", |cx| {
        visible(cx, "connection-request-new-entry-notes-key")
    });
    settle(cx);

    // The vault's own dialog, on the entry's name and the host the request sends it to.
    click(cx, "connection-request-new-entry-notes-key");
    let vault = shell.read_with(cx, |s, cx| s.vault(cx));
    until(&shell, cx, "the vault's dialog opens", |cx| {
        vault.read_with(cx, |v, _| v.is_open())
    });
    settle(cx);
    assert!(
        visible(cx, "connection-request"),
        "the request's dialog stays, under the entry's"
    );
    let (name, url, secret) = vault.read_with(cx, |v, _| {
        (
            v.name_field().unwrap().clone(),
            v.url_field().unwrap().clone(),
            v.secret_field().unwrap().clone(),
        )
    });
    assert_eq!(
        name.read_with(cx, |f, _| f.value().to_string()),
        "notes-key"
    );
    assert_eq!(
        url.read_with(cx, |f, _| f.value().to_string()),
        "notes.example.com"
    );
    assert_eq!(
        secret.read_with(cx, |f, _| f.value().to_string()),
        "",
        "the secret is left for the user to type"
    );
    cx.update(|window, cx| secret.update(cx, |f, cx| f.set_value(SECRET, window, cx)));
    click(cx, "save-vault-entry");
    until(&shell, cx, "the request's dialog is back", |cx| {
        visible(cx, "connection-request") && !vault.read_with(cx, |v, _| v.is_open())
    });
    until(&shell, cx, "the declaration is read again", |cx| {
        visible(cx, "connection-request-accept")
            && !present(cx, "connection-request-new-entry-notes-key")
    });
    settle(cx);

    click(cx, "connection-request-accept");
    until(&shell, cx, "the dialog closes", |cx| {
        !visible(cx, "connection-request")
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while connection(&user, "notes").is_none() {
        assert!(Instant::now() < deadline, "never written");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(pending(&user), 0);
    // The secret was typed in the vault's dialog alone.
    let said = cx.update(|window, _| {
        window
            .elements()
            .into_iter()
            .filter_map(|found| found.label().map(str::to_owned))
            .collect::<Vec<_>>()
            .join("\n")
    });
    assert!(!said.contains(SECRET));
}

/// The dialog of a request whose entry is not made yet, Accept answered that it waits for it, the
/// button to make it shown.
fn waiting_for_its_entry(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    lead: &str,
    number: u64,
) {
    open_card(shell, cx, lead, number);
    open_dialog(shell, cx, number);
    click(cx, "connection-request-accept");
    until(shell, cx, "the button for the entry", |cx| {
        visible(cx, "connection-request-new-entry-notes-key")
    });
    settle(cx);
}

#[gpui_kit::test]
fn the_new_entry_buttons_fold_away_with_their_block(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = a_bearer_request(&mcp);
    let (shell, cx) = window(&user, cx);
    waiting_for_its_entry(&shell, cx, &lead, number);

    // Accept again: the answer is cleared, and the block folds away with its button in it.
    press(cx, "connection-request-accept");
    a_third_later(cx);
    assert!(
        visible(cx, "connection-request-new-entry-notes-key"),
        "the button folds away with its block, it does not vanish first"
    );
}

#[gpui_kit::test]
fn the_new_entry_buttons_fold_away_when_the_request_is_no_longer_current(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = a_bearer_request(&mcp);
    let (shell, cx) = window(&user, cx);
    waiting_for_its_entry(&shell, cx, &lead, number);

    // The user adds a server of that name meanwhile: the agent's add would no longer be done.
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Mine.", "url": "https://mine.example.com/mcp"}),
    )
    .unwrap();
    let reading = user
        .call("connection.proposed", json!({"number": number}))
        .unwrap();
    assert_eq!(reading["current"], false, "{reading}");
    // Read again, as the dialog does after a Share or a Bind: it says so, and no entry is
    // offered to make for it.
    let request = shell.read_with(cx, |s, _| s.connection_request().clone());
    request.update(cx, |r, cx| {
        r.answered(
            flottille_window::extensions::connection_request::Then::Proposed { number },
            Ok(reading),
            cx,
        )
    });
    a_third_later(cx);
    assert!(
        visible(cx, "connection-request-new-entry-notes-key"),
        "the button folds away with its block, it does not vanish first"
    );
    until(&shell, cx, "the block folds away", |cx| {
        !present(cx, "connection-request-new-entry-notes-key")
    });
    assert!(!present(cx, "connection-request-accept"));
}

#[gpui_kit::test]
fn the_new_entry_buttons_answer_only_an_accept_that_waits(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = a_bearer_request(&mcp);
    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);

    // A Refuse that failed names no entry to make, though the request names a missing one.
    let request = shell.read_with(cx, |s, _| s.connection_request().clone());
    request.update(cx, |r, cx| {
        r.answered(
            flottille_window::extensions::connection_request::Then::Decided {
                number,
                accepted: false,
            },
            Err(format!(
                "request #{number} is already decided: it was denied"
            )),
            cx,
        )
    });
    settle(cx);
    assert!(visible(cx, "connection-request-error"));
    assert!(!present(cx, "connection-request-new-entry-notes-key"));
}

// ── #1019: an agent's connection goes through without dead ends ─────────────────────────────

const KEY: &str = "hippocampe-k3y-from-the-form";

/// An agent's request to add the URL connection `hippocampe`, a server on this machine over
/// plain http, its bearer key the vault entry `hippocampe-key`.
fn hippocampe(mcp: &Mcp) -> u64 {
    held(
        mcp,
        json!({"action": "add", "name": "hippocampe", "about": "The user's memory.",
            "url": "http://127.0.0.1:7468/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "hippocampe-key"}]}),
    )
}

fn vault_entry(user: &User, name: &str) -> Option<Value> {
    user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .cloned()
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<SessionPage>> {
    shell.read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
}

/// The number of the request for a vault entry the session shown records: its card is in the
/// rail's "Waiting for you", named by it.
fn ask_row(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<u64> {
    let page = session_page(shell, cx)?;
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .find_map(|block| match block {
                Block::VaultAsk(ask) => Some(ask.seq),
                _ => None,
            })
    })
}

fn vault_form(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<VaultSection> {
    // Over the page shown, whichever it is: the form is a dialog of the window.
    shell.read_with(cx, |s, cx| s.vault(cx))
}

fn url_of(vault: &Entity<VaultSection>, cx: &mut VisualTestContext) -> String {
    let field = vault.read_with(cx, |v, _| v.url_field().cloned()).unwrap();
    field.read_with(cx, |f, _| f.value().to_string())
}

/// The agent asked for `hippocampe-key` after its request: the session's card of that ask, its
/// New entry clicked, the vault's form open.
fn new_entry_from_the_ask(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    lead: &str,
) -> (u64, Entity<VaultSection>) {
    until(shell, cx, "the session is in the tree", |cx| {
        present(cx, &format!("session-{lead}"))
    });
    click(cx, &format!("session-{lead}"));
    until(shell, cx, "the card of the ask", |cx| {
        ask_row(shell, cx).is_some_and(|at| present(cx, &format!("vault-ask-{at}-add")))
    });
    settle(cx);
    let at = ask_row(shell, cx).unwrap();
    let said = label(cx, &format!("vault-ask-{at}-project")).unwrap_or_default();
    assert!(
        said.to_lowercase()
            .contains("for the connection hippocampe (every project)"),
        "{said}"
    );
    click(cx, &format!("vault-ask-{at}-add"));
    let vault = vault_form(shell, cx);
    assert!(vault.read_with(cx, |v, _| v.is_open()), "the form is open");
    (at, vault)
}

/// Types the key in the open form and saves it.
fn save_the_key(
    user: &User,
    shell: &Entity<Shell>,
    vault: &Entity<VaultSection>,
    cx: &mut VisualTestContext,
) {
    let secret = vault
        .read_with(cx, |v, _| v.secret_field().cloned())
        .unwrap();
    cx.update(|window, cx| secret.update(cx, |f, cx| f.set_value(KEY, window, cx)));
    click(cx, "save-vault-entry");
    until(shell, cx, "the entry is saved", |cx| {
        if let Some(why) = vault.read_with(cx, |v, _| v.form_error().map(str::to_owned)) {
            panic!("the form refused: {why}");
        }
        vault_entry(user, "hippocampe-key").is_some()
    });
    settle(cx);
}

/// Accept of the open dialog: the same request goes through, the connection is written.
fn accept_goes_through(user: &User, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    assert!(
        visible(cx, "connection-request-accept"),
        "no Accept: {:?}",
        label(cx, "connection-request-stale")
    );
    click(cx, "connection-request-accept");
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        settle(cx);
        let said = toasts(shell, cx);
        if said
            .iter()
            .any(|t| t == "Request accepted: the server hippocampe was added")
        {
            break;
        }
        assert!(
            !present(cx, "connection-request-error"),
            "Accept was refused in the dialog"
        );
        assert!(
            Instant::now() < deadline,
            "no toast of the Accept: toasts {said:?}, pending {:?}",
            user.call("approval.list", json!({})).unwrap()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while connection(user, "hippocampe").is_none() {
        assert!(Instant::now() < deadline, "never written");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(pending(user), 0);
}

#[gpui_kit::test]
fn the_entry_dialog_from_a_connection_s_ask_chooses_every_project(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = hippocampe(&mcp);
    let asked = mcp.call(
        "vault",
        json!({"action": "ask", "name": "hippocampe-key", "why": "the memory server's key"}),
    );
    assert!(!asked.error, "{}", asked.text);

    let (shell, cx) = window(&user, cx);
    let (_, vault) = new_entry_from_the_ask(&shell, cx, &lead);
    assert_eq!(
        vault.read_with(cx, |v, _| v.project().map(str::to_owned)),
        None,
        "Seen by agents of: every project"
    );
    let why = label(cx, "vault-connection-note").unwrap_or_default();
    assert!(
        why.contains("hippocampe") && why.contains("every project"),
        "the dialog says why: {why}"
    );
    save_the_key(&user, &shell, &vault, cx);
    let entry = vault_entry(&user, "hippocampe-key").unwrap();
    assert_eq!(entry["project"], Value::Null, "{entry}");

    // From the request's own dialog, Accept goes through: the agent asks nothing again. Opened
    // as its card does: the toast of the save lies over the card's button.
    open_card(&shell, cx, &lead, number);
    shell.update(cx, |shell, cx| shell.open_connection_request(number, cx));
    until(&shell, cx, "the declaration is read", |cx| {
        visible(cx, "connection-request-name")
    });
    settle(cx);
    accept_goes_through(&user, &shell, cx);
}

#[gpui_kit::test]
fn an_entry_made_for_a_connection_gets_its_address(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    hippocampe(&mcp);
    let asked = mcp.call(
        "vault",
        json!({"action": "ask", "name": "hippocampe-key", "why": "the memory server's key",
            "connection": "hippocampe"}),
    );
    assert!(!asked.error, "{}", asked.text);

    let (shell, cx) = window(&user, cx);
    let (_, vault) = new_entry_from_the_ask(&shell, cx, &lead);
    // Shown and editable before Save: the connection's origin, and where it sends the secret.
    assert_eq!(url_of(&vault, cx), "http://127.0.0.1:7468");
    assert_eq!(
        label(cx, "vault-goes-to").as_deref(),
        Some("Goes to: 127.0.0.1")
    );
    save_the_key(&user, &shell, &vault, cx);
    assert_eq!(
        vault_entry(&user, "hippocampe-key").unwrap()["url"],
        "http://127.0.0.1:7468"
    );
}

#[gpui_kit::test]
fn a_project_s_entry_named_by_a_connection_request_offers_to_share_it(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = hippocampe(&mcp);
    // Saved from the card of an ask as it once offered: for the project the session runs in.
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": KEY, "project": "app",
            "url": "http://127.0.0.1:7468"}),
    )
    .unwrap();

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    // Not dead: said as it is, with the way out, and Accept still offered.
    assert!(present(cx, "connection-request-accept"));
    assert!(
        !label(cx, "connection-request-stale")
            .unwrap_or_default()
            .contains("can no longer be done")
    );
    assert_eq!(
        label(cx, "connection-request-share-hippocampe-key").as_deref(),
        Some(
            "the vault entry hippocampe-key is app's alone; a connection needs it for every \
             project"
        )
    );
    assert_eq!(
        label(cx, "connection-request-share-hippocampe-key-action").as_deref(),
        Some("Share with every project")
    );

    // The user's, behind the one confirmation, which says what it gives and to whom.
    click(cx, "connection-request-share-hippocampe-key-action");
    assert!(visible(cx, "connection-request-widen"));
    assert_eq!(
        label(cx, "connection-request-widen-title").as_deref(),
        Some("Share the entry hippocampe-key with every project?")
    );
    let sentence = label(cx, "connection-request-widen-sentence").unwrap_or_default();
    assert!(sentence.contains("every project"), "{sentence}");
    // Cancelled, nothing changes.
    click(cx, "connection-request-widen-cancel");
    assert!(!visible(cx, "connection-request-widen"));
    assert_eq!(
        vault_entry(&user, "hippocampe-key").unwrap()["project"],
        "app"
    );
    assert!(visible(cx, "connection-request"));
}

#[gpui_kit::test]
fn sharing_the_entry_lets_the_same_request_be_accepted(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = hippocampe(&mcp);
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": KEY, "project": "app",
            "url": "http://127.0.0.1:7468"}),
    )
    .unwrap();

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    click(cx, "connection-request-share-hippocampe-key-action");
    press(cx, "connection-request-widen-action");
    until(&shell, cx, "the entry is shared", |_| {
        vault_entry(&user, "hippocampe-key").unwrap()["project"].is_null()
    });
    // The line folds away, over a panel's time.
    let full = height(cx, "connection-request-widenings");
    assert!(full > 0., "the line is drawn");
    until(&shell, cx, "the line begins to fold", |cx| {
        height(cx, "connection-request-widenings") < full
    });
    let folding = height(cx, "connection-request-widenings");
    settle(cx);
    assert!(
        0. < folding && height(cx, "connection-request-widenings") < folding,
        "it folds over a panel's time: {folding} of {full}"
    );
    assert!(!present(cx, "connection-request-share-hippocampe-key"));
    accept_goes_through(&user, &shell, cx);
}

#[gpui_kit::test]
fn an_entry_without_an_address_is_bound_from_the_request_then_accepted(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": KEY, "description": "The memory's key"}),
    )
    .unwrap();
    let number = hippocampe(&mcp);

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    assert_eq!(
        label(cx, "connection-request-bind-hippocampe-key").as_deref(),
        Some(
            "the vault entry hippocampe-key has no address; this connection would send it to 127.0.0.1"
        )
    );
    assert_eq!(
        label(cx, "connection-request-bind-hippocampe-key-action").as_deref(),
        Some("Bind it to 127.0.0.1")
    );
    click(cx, "connection-request-bind-hippocampe-key-action");
    assert_eq!(
        label(cx, "connection-request-widen-title").as_deref(),
        Some("Bind the entry hippocampe-key to 127.0.0.1?")
    );
    let sentence = label(cx, "connection-request-widen-sentence").unwrap_or_default();
    assert!(
        sentence.contains("sent to 127.0.0.1"),
        "it says where the secret goes: {sentence}"
    );
    press(cx, "connection-request-widen-action");
    until(&shell, cx, "the entry is bound", |_| {
        vault_entry(&user, "hippocampe-key").unwrap()["url"] == "http://127.0.0.1:7468"
    });
    until(&shell, cx, "the line folds away", |cx| {
        !present(cx, "connection-request-bind-hippocampe-key")
    });
    let entry = vault_entry(&user, "hippocampe-key").unwrap();
    assert_eq!(entry["description"], "The memory's key", "{entry}");
    accept_goes_through(&user, &shell, cx);

    // Another address keeps the warning, and needs the same action.
    user.call(
        "vault.set",
        json!({"name": "other-key", "secret": KEY, "url": "https://memory.example.com"}),
    )
    .unwrap();
    let other = held(
        &mcp,
        json!({"action": "add", "name": "recall", "about": "Another memory.",
            "url": "http://127.0.0.1:7469/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "entry": "other-key"}]}),
    );
    shell.update(cx, |shell, cx| shell.open_connection_request(other, cx));
    until(&shell, cx, "the other request is read", |cx| {
        present(cx, "connection-request-bind-other-key")
    });
    settle(cx);
    assert_eq!(
        label(cx, "connection-request-bind-other-key").as_deref(),
        Some(
            "the vault entry other-key is for memory.example.com; this connection would send it to 127.0.0.1"
        )
    );
    assert!(visible(cx, "connection-request-bind-other-key-action"));
}

// ── #1019, round 2: Share and Bind act only on what the dialog shows ─────────────────────────

/// The connection `notes` the user declared at `https://old.example.com/mcp`, and an agent's
/// edit that only gives its server the vault entry `notes-key` (every project's, no address).
fn edit_adding_a_key(user: &User, mcp: &Mcp) -> u64 {
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Notes.", "url": "https://old.example.com/mcp"}),
    )
    .unwrap();
    user.call("vault.set", json!({"name": "notes-key", "secret": KEY}))
        .unwrap();
    held(
        mcp,
        json!({"action": "edit", "name": "notes",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "notes-key"}]}),
    )
}

#[gpui_kit::test]
fn a_bind_confirmed_after_the_request_changed_binds_nothing(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = edit_adding_a_key(&user, &mcp);

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    assert_eq!(
        label(cx, "connection-request-bind-notes-key-action").as_deref(),
        Some("Bind it to old.example.com")
    );
    click(cx, "connection-request-bind-notes-key-action");
    assert_eq!(
        label(cx, "connection-request-widen-title").as_deref(),
        Some("Bind the entry notes-key to old.example.com?")
    );
    // The user moves the connection meanwhile: the request would not be done any more.
    user.call(
        "connection.edit",
        json!({"name": "notes", "url": "https://new.example.com/mcp"}),
    )
    .unwrap();
    press(cx, "connection-request-widen-action");
    until(&shell, cx, "the refusal is said", |cx| {
        present(cx, "connection-request-error")
    });
    // Read again: stale, its lines fold away and no Accept is offered.
    until(&shell, cx, "the line folds away", |cx| {
        !present(cx, "connection-request-bind-notes-key")
    });
    assert!(!present(cx, "connection-request-accept"));
    assert_eq!(vault_entry(&user, "notes-key").unwrap()["url"], Value::Null);
}

#[gpui_kit::test]
fn a_request_changed_since_it_was_asked_offers_no_share_or_bind(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = edit_adding_a_key(&user, &mcp);
    user.call(
        "connection.edit",
        json!({"name": "notes", "url": "https://new.example.com/mcp"}),
    )
    .unwrap();

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    assert!(!present(cx, "connection-request-accept"));
    assert!(!present(cx, "connection-request-bind-notes-key"));
    assert!(!present(cx, "connection-request-bind-notes-key-action"));
}

#[gpui_kit::test]
fn no_share_or_bind_is_offered_for_an_entry_the_request_cannot_take(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    // Named before it exists, then made for another use: a browser login of another site.
    let number = hippocampe(&mcp);
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": KEY, "url": "https://site.example",
            "for": "connection:browser"}),
    )
    .unwrap();
    // The project's own entry, then moved to another project after the call.
    let folder = user.path("work/other");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "other\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "other"}))
        .unwrap();
    user.call(
        "vault.set",
        json!({"name": "recall-key", "secret": KEY, "project": "app",
            "url": "https://recall.example.com"}),
    )
    .unwrap();
    let recall = held(
        &mcp,
        json!({"action": "add", "name": "recall", "about": "Another memory.",
            "url": "https://recall.example.com/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "entry": "recall-key"}]}),
    );
    user.call(
        "vault.set",
        json!({"name": "recall-key", "project": "other"}),
    )
    .unwrap();

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    assert!(!present(cx, "connection-request-bind-hippocampe-key"));
    assert!(!present(cx, "connection-request-share-hippocampe-key"));
    shell.update(cx, |shell, cx| shell.open_connection_request(recall, cx));
    until(&shell, cx, "the other request is read", |cx| {
        label(cx, "connection-request-name").is_some_and(|name| name.contains("recall"))
    });
    settle(cx);
    assert!(!present(cx, "connection-request-share-recall-key"));
    assert!(!present(cx, "connection-request-bind-recall-key"));
}

#[gpui_kit::test]
fn the_confirmation_says_the_change_stays_and_the_address_it_replaces(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": KEY, "project": "app"}),
    )
    .unwrap();
    let number = hippocampe(&mcp);

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    click(cx, "connection-request-share-hippocampe-key-action");
    let sentence = label(cx, "connection-request-widen-sentence").unwrap_or_default();
    assert!(
        sentence.contains("even if you then refuse the request"),
        "{sentence}"
    );
    click(cx, "connection-request-widen-cancel");
    click(cx, "connection-request-bind-hippocampe-key-action");
    let sentence = label(cx, "connection-request-widen-sentence").unwrap_or_default();
    assert!(
        sentence.contains("now no address") && sentence.contains("http://127.0.0.1:7468"),
        "{sentence}"
    );
    assert!(
        sentence.contains("even if you then refuse the request"),
        "{sentence}"
    );
    click(cx, "connection-request-widen-cancel");

    // An entry with an address of its own: the Confirm names the one it replaces.
    user.call(
        "vault.set",
        json!({"name": "other-key", "secret": KEY, "url": "https://memory.example.com/keys"}),
    )
    .unwrap();
    let other = held(
        &mcp,
        json!({"action": "add", "name": "recall", "about": "Another memory.",
            "url": "http://127.0.0.1:7469/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "entry": "other-key"}]}),
    );
    shell.update(cx, |shell, cx| shell.open_connection_request(other, cx));
    until(&shell, cx, "the other request is read", |cx| {
        present(cx, "connection-request-bind-other-key-action")
    });
    settle(cx);
    click(cx, "connection-request-bind-other-key-action");
    let sentence = label(cx, "connection-request-widen-sentence").unwrap_or_default();
    assert!(
        sentence.contains("now https://memory.example.com/keys")
            && sentence.contains("http://127.0.0.1:7469"),
        "{sentence}"
    );
}

// ── Accepting a request that gives new actions leads into their review (#1031) ───────────────

/// `almanac` given to agents, its server then announcing one more action: the agent's edit (an
/// argument added to its command) is held, and what it gives needs the user's review.
fn edit_giving_an_action(user: &User, mcp: &Mcp) -> u64 {
    let line = line_of(user, "almanac");
    user.call(
        "connection.add",
        json!({"name": "almanac", "about": "The notes of the user.",
            "command": flottille_core::command::split_line(&line).unwrap()}),
    )
    .unwrap();
    let mut newer = almanac();
    newer["tools"].as_array_mut().unwrap().push(json!({
        "name": "link", "description": "Links two notes.", "inputSchema": {"type": "object"}}));
    std::fs::write(user.path("almanac-script.json"), newer.to_string()).unwrap();
    held(
        mcp,
        json!({"action": "edit", "name": "almanac", "command": format!("{line} --verbose")}),
    )
}

/// The toast whose action reads `Review`, if one is shown: the id of its action.
fn review_toast(cx: &mut VisualTestContext) -> Option<String> {
    (1..16)
        .map(|n| format!("toast-{n}-action"))
        .find(|id| label(cx, id).as_deref() == Some("Review"))
}

#[gpui_kit::test]
fn accepting_a_request_that_adds_actions_opens_their_review(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = edit_giving_an_action(&user, &mcp);

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    click(cx, "connection-request-accept");

    // The review opens at once, over the page the user is on: what the server announces is
    // there, the new action first among what is read.
    until(&shell, cx, "the review opens", |cx| {
        present(cx, "connection-review-server")
    });
    settle(cx);
    assert!(visible(cx, "connection-review"));
    assert_eq!(
        label(cx, "connection-review-server").as_deref(),
        Some("almanac 1.0.0")
    );
    assert_eq!(
        connection(&user, "almanac").unwrap()["state"],
        "awaiting review"
    );
    let said = toasts(&shell, cx);
    assert!(
        said.iter()
            .any(|t| t == "Request accepted: the server almanac was saved"),
        "{said:?}"
    );

    // Accept in the review gives the agent its actions.
    click(cx, "accept-connection");
    until(&shell, cx, "the review closes", |cx| {
        !visible(cx, "connection-review")
    });
    assert_eq!(connection(&user, "almanac").unwrap()["state"], "ready");
}

#[gpui_kit::test]
fn closing_the_review_of_an_accepted_request_says_the_connection_still_waits(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = edit_giving_an_action(&user, &mcp);

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    click(cx, "connection-request-accept");
    until(&shell, cx, "the review opens", |cx| {
        present(cx, "connection-review-server")
    });
    settle(cx);

    click(cx, "connection-review-cancel");
    until(&shell, cx, "the review closes", |cx| {
        !visible(cx, "connection-review")
    });
    // Left awaiting review, and the toast says so, with the kit's button to come back to it.
    until(&shell, cx, "the toast", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "The server almanac still waits for your review")
    });
    let again = review_toast(cx).expect("the toast has a Review button");
    assert_eq!(
        connection(&user, "almanac").unwrap()["state"],
        "awaiting review"
    );
    click(cx, &again);
    until(&shell, cx, "the review opens again", |cx| {
        visible(cx, "connection-review") && present(cx, "connection-review-server")
    });
}

#[gpui_kit::test]
fn the_session_of_the_agent_shows_the_review_its_request_led_to(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = edit_giving_an_action(&user, &mcp);
    user.call("approval.approve", json!({"number": number}))
        .unwrap();

    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        present(cx, &format!("session-{lead}"))
    });
    click(cx, &format!("session-{lead}"));
    until(&shell, cx, "the card of the review", |cx| {
        review_row(&shell, cx).is_some_and(|at| present(cx, &format!("row-{at}-review-card")))
    });
    settle(cx);
    let at = review_row(&shell, cx).unwrap();
    assert_eq!(
        label(cx, &format!("row-{at}-review-said")).as_deref(),
        Some("almanac waits for your review of its new actions")
    );
    click(cx, &format!("row-{at}-review-open"));
    until(&shell, cx, "the review opens", |cx| {
        present(cx, "connection-review-server")
    });

    // Reviewed: the daemon says so to the session, and the card is no longer open once the user
    // is back in it.
    click(cx, "accept-connection");
    until(&shell, cx, "the review closes", |cx| {
        !visible(cx, "connection-review")
    });
    click(cx, &format!("session-{lead}"));
    until(&shell, cx, "the card is done", |cx| {
        session_page(&shell, cx).is_some_and(|page| {
            page.read_with(cx, |page, cx| {
                page.conversation()
                    .read(cx)
                    .blocks()
                    .iter()
                    .any(|block| matches!(block, Block::ConnectionReview(review) if !review.open))
            })
        })
    });
    assert_eq!(connection(&user, "almanac").unwrap()["state"], "ready");
}

/// The row of the card of a connection waiting for its review, in the session shown.
fn review_row(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<usize> {
    let page = session_page(shell, cx)?;
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .position(|block| matches!(block, Block::ConnectionReview(_)))
    })
}

#[gpui_kit::test]
fn a_request_on_a_connection_awaiting_review_offers_its_review_first(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let first = edit_giving_an_action(&user, &mcp);
    user.call("approval.approve", json!({"number": first}))
        .unwrap();
    assert_eq!(
        connection(&user, "almanac").unwrap()["state"],
        "awaiting review"
    );
    // Held, not refused: the agent has nothing to relay.
    let number = held(
        &mcp,
        json!({"action": "set", "name": "almanac", "settings": {"search": "passes"}}),
    );

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    assert_eq!(
        label(cx, "connection-request-review-said").as_deref(),
        Some("almanac waits for your review of its actions first")
    );
    click(cx, "connection-request-review");
    until(&shell, cx, "the review opens", |cx| {
        present(cx, "connection-review-server")
    });
    assert_eq!(pending(&user), 1, "the request stays held");
}

#[gpui_kit::test]
fn a_failed_write_of_an_accepted_request_says_so_and_opens_no_review(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let user = user();
    let (lead, mcp) = a_root(&user);
    // The connection already waits for its review, and the agent asks for one more edit.
    let first = edit_giving_an_action(&user, &mcp);
    user.call("approval.approve", json!({"number": first}))
        .unwrap();
    let number = held(
        &mcp,
        json!({"action": "edit", "name": "almanac", "about": "The notes of the user, again."}),
    );

    let (shell, cx) = window(&user, cx);
    open_card(&shell, cx, &lead, number);
    open_dialog(&shell, cx, number);
    // The write cannot happen: the folder of the declarations is read-only.
    let folder = user.path("config/flottille");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    click(cx, "connection-request-accept");

    until(&shell, cx, "the failure is said", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "The server almanac was not written")
    });
    settle(cx);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    let said = toasts(&shell, cx);
    assert!(
        !said.iter().any(|t| t.contains("was saved")),
        "never said saved: {said:?}"
    );
    // Nothing was written, so no review is opened for it: the connection is as it was.
    assert!(!present(cx, "connection-review-server"));
    assert!(!visible(cx, "connection-review"));
}

// ── A dialog opens over the page it is asked from (#1018) ─────────────────────────────────────

fn selected(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Selection> {
    shell.read_with(cx, |s, _| s.selected().cloned())
}

fn stack(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<&'static str> {
    shell.read_with(cx, |s, cx| s.modal_stack(cx))
}

#[gpui_kit::test]
fn edit_on_a_connection_request_opens_the_form_over_the_session(cx: &mut TestAppContext) {
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

    // The user is where they were: the session, with the request's card in its rail.
    assert_eq!(selected(&shell, cx), Some(Selection::Session(lead.clone())));
    assert!(
        session_page(&shell, cx).is_some(),
        "the session's page is shown"
    );
    assert!(
        present(cx, &format!("open-request-{number}")),
        "its card is behind the form"
    );
    assert_eq!(stack(&shell, cx), ["connection-modal"]);
    let fields = connections_section(&shell, cx).read_with(cx, |s, _| s.form_fields().unwrap());
    assert_eq!(
        fields.about.read_with(cx, |f, _| f.value().to_string()),
        "Notes."
    );

    // Cancel leaves the request waiting, on the same page.
    click(cx, "connection-modal-cancel");
    until(&shell, cx, "the form closes", |cx| {
        !visible(cx, "connection-modal")
    });
    assert_eq!(selected(&shell, cx), Some(Selection::Session(lead)));
    assert_eq!(pending(&user), 1, "the request is still waiting");
}

#[gpui_kit::test]
fn a_dialog_opened_from_a_dialog_stacks_and_escape_returns_to_the_first(cx: &mut TestAppContext) {
    let user = user();
    let (lead, mcp) = a_root(&user);
    let number = a_bearer_request(&mcp);

    let (shell, cx) = window(&user, cx);
    waiting_for_its_entry(&shell, cx, &lead, number);
    click(cx, "connection-request-new-entry-notes-key");
    until(&shell, cx, "the vault's dialog opens", |cx| {
        visible(cx, "vault-modal")
    });
    settle(cx);

    // The request's dialog stays, under the entry's.
    assert_eq!(stack(&shell, cx), ["connection-request", "vault-modal"]);
    assert!(
        visible(cx, "connection-request"),
        "the first is still drawn"
    );
    assert_eq!(selected(&shell, cx), Some(Selection::Session(lead.clone())));

    // Escape closes the top one and returns to the one below.
    cx.update(|window, cx| window.press("escape", cx));
    settle(cx);
    assert_eq!(stack(&shell, cx), ["connection-request"]);
    assert!(!visible(cx, "vault-modal"), "the top one is closed");
    assert!(
        visible(cx, "connection-request"),
        "the first is back, alone"
    );

    // Escape again closes it: the request stays waiting, the page is as it was.
    cx.update(|window, cx| window.press("escape", cx));
    settle(cx);
    assert!(stack(&shell, cx).is_empty());
    assert!(!visible(cx, "connection-request"));
    assert_eq!(selected(&shell, cx), Some(Selection::Session(lead)));
    assert_eq!(pending(&user), 1);
}
