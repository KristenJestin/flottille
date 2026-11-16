//! The scenarios of the connections an agent proposes (`docs/tools.md`, "Connections"): the
//! `connections` family reads the declarations without a secret, and every write is held for the
//! user in every mode, Full included, written once the user accepts it, by the path of the user's
//! own Save, exactly as the card showed it. Against the real daemon with the fake ACP agent and
//! the fake MCP server: no real server, the in-memory keyring, temporary folders.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

/// The secret of the vault entry `almanac-token`, which nothing may give back.
const SECRET: &str = "s3cret-almanac-t0ken";

fn fake_mcp() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(
        path.exists(),
        "{} is missing: `cargo build -p flottille-fake-mcp` first",
        path.display()
    );
    path
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    user
}

fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

/// A root of `app`, in its main checkout: its id and its MCP client.
fn root(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "start"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

fn almanac() -> Value {
    json!({
        "server": {"name": "almanac", "version": "1.0.0"},
        "instructions": "Search before you write.",
        "tools": [
            {"name": "search", "description": "Finds notes by words.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}},
            {"name": "write", "description": "Writes a note.",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
        ],
        "answers": {"search": {"text": "found"}, "write": {"text": "written"}},
    })
}

/// The fake server's line for `name`, as an agent writes it: one line, no shell.
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

fn turn_on(user: &User) {
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
}

/// The connection `name`, declared by the user on the fake server.
fn declare(user: &User, name: &str, more: Value) {
    let mut params = json!({"name": name, "about": "The notes of the user.",
        "command": flottille_core::command::split_line(&line_of(user, name)).unwrap()});
    for (key, value) in more.as_object().into_iter().flatten() {
        params[key] = value.clone();
    }
    user.call("connection.add", params).unwrap();
}

fn connection(user: &User, name: &str) -> Option<Value> {
    user.call("connection.list", json!({})).unwrap()["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
}

fn file(user: &User) -> String {
    std::fs::read_to_string(user.path("config/flottille/extensions.toml")).unwrap_or_default()
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "an error: {}", answer.text);
    let rest = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    rest.split('.').next().unwrap().parse().unwrap()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    answer.text
}

fn request(user: &User, number: u64) -> Value {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap_or_else(|| panic!("no pending request #{number}"))
}

fn pending(user: &User) -> usize {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .len()
}

/// The text the agent is handed once the request `number` is answered, read from the store
/// opened read-only: `Store::open` writes it (its migrations), and a write of the test's that
/// lands between the read and the write of one of the daemon's transactions fails that one, the
/// very write the user accepted among them.
fn delivered(user: &User, number: u64) -> String {
    let store = rusqlite::Connection::open_with_flags(
        user.path("state/flottille/flottille.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    store.busy_timeout(Duration::from_secs(10)).unwrap();
    let body = || {
        store
            .query_row(
                "SELECT body FROM deliveries WHERE id = ?1",
                [format!("approval-{number}")],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .unwrap()
    };
    eventually("the answer", || body().is_some());
    body().unwrap()
}

fn approve(user: &User, number: u64) -> String {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    delivered(user, number)
}

#[test]
fn an_agent_adds_a_connection_once_the_user_accepts() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (agent, mcp) = root(&user);
    // Held in Full too: the command runs with the user's rights, its tool reaches every agent.
    set_mode(&user, &agent, "full");
    let line = line_of(&user, "almanac");

    let number = held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "almanac", "about": "The notes of the user.",
            "command": line, "only": {"query": ["login", "logout"]}}),
    ));

    let asked = request(&user, number);
    assert_eq!(asked["class"], "base", "{asked}");
    assert_eq!(asked["kind"], "action", "{asked}");
    let says = asked["says"].to_string();
    assert!(says.contains("Add the connection almanac"), "{says}");
    assert!(says.contains("--script"), "the command, whole: {says}");
    assert!(says.contains("login, logout"), "the limit: {says}");
    assert!(
        !file(&user).contains("almanac"),
        "written before the user said"
    );
    assert!(connection(&user, "almanac").is_none());

    // The card shows the declaration as it will be written.
    let proposed = user
        .call("connection.proposed", json!({"number": number}))
        .unwrap();
    assert_eq!(proposed["before"], Value::Null, "{proposed}");
    assert_eq!(proposed["after"]["name"], "almanac", "{proposed}");
    assert_eq!(proposed["current"], true, "{proposed}");

    let body = approve(&user, number);
    assert!(body.contains("and Flottille has now done it"), "{body}");
    assert!(body.contains("given to agents"), "{body}");
    let written = connection(&user, "almanac").expect("written");
    assert_eq!(written["command"], proposed["after"]["command"]);
    assert_eq!(written["only"], json!({"query": ["login", "logout"]}));
    // By the path of the user's own Save: given at once, every action judged.
    assert_eq!(written["state"], "ready", "{written}");
    assert!(
        written["actions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["setting"] == "judged"),
        "{written}"
    );
}

#[test]
fn a_refused_request_writes_nothing() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    declare(&user, "almanac", json!({}));
    let before = file(&user);
    let (_, mcp) = root(&user);
    let line = line_of(&user, "jotter");

    let calls = [
        json!({"action": "add", "name": "jotter", "about": "Notes.", "command": line}),
        json!({"action": "edit", "name": "almanac", "about": "Something else."}),
        json!({"action": "remove", "name": "almanac"}),
        json!({"action": "off", "name": "almanac"}),
        json!({"action": "set", "name": "almanac", "settings": {"write": "passes"}}),
    ];
    for call in calls {
        let number = held(&mcp.call("connections", call.clone()));
        user.call("approval.deny", json!({"number": number}))
            .unwrap();
        let body = delivered(&user, number);
        assert!(body.contains("was refused by the user"), "{call}: {body}");
        assert_eq!(file(&user), before, "{call}");
    }
    let almanac = connection(&user, "almanac").unwrap();
    assert_eq!(almanac["state"], "ready");
    assert!(
        almanac["actions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["setting"] == "judged")
    );
}

#[test]
fn an_edit_shows_its_differences() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    declare(&user, "almanac", json!({}));
    let (_, mcp) = root(&user);

    let number = held(&mcp.call(
        "connections",
        json!({"action": "edit", "name": "almanac", "about": "The user's decisions.",
            "only": {"query": ["login"]}}),
    ));

    let says = request(&user, number)["says"].to_string();
    assert!(says.contains("Edit the connection almanac"), "{says}");
    assert!(
        says.contains("The notes of the user.") && says.contains("The user's decisions."),
        "the old sentence and the new: {says}"
    );
    let proposed = user
        .call("connection.proposed", json!({"number": number}))
        .unwrap();
    assert_eq!(proposed["before"]["about"], "The notes of the user.");
    assert_eq!(proposed["after"]["about"], "The user's decisions.");
    assert_eq!(proposed["after"]["only"], json!({"query": ["login"]}));
    assert_eq!(
        proposed["after"]["command"], proposed["before"]["command"],
        "what it does not change stays"
    );
    assert_eq!(proposed["changed"], json!(["about", "only"]), "{proposed}");

    let body = approve(&user, number);
    assert!(body.contains("and Flottille has now done it"), "{body}");
    assert_eq!(
        connection(&user, "almanac").unwrap()["about"],
        "The user's decisions."
    );
}

#[test]
fn an_edit_changed_since_its_card_is_not_written() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    declare(&user, "almanac", json!({}));
    let (_, mcp) = root(&user);
    let number = held(&mcp.call(
        "connections",
        json!({"action": "edit", "name": "almanac", "about": "The user's decisions."}),
    ));

    // The user changes the declaration the card showed: what the agent's edit would leave is not
    // what the user read any more.
    user.call(
        "connection.edit",
        json!({"name": "almanac", "only": {"query": ["login"]}}),
    )
    .unwrap();
    let proposed = user
        .call("connection.proposed", json!({"number": number}))
        .unwrap();
    assert_eq!(proposed["current"], false, "{proposed}");

    let body = approve(&user, number);
    assert!(body.contains("was not executed"), "{body}");
    assert_eq!(
        connection(&user, "almanac").unwrap()["about"],
        "The notes of the user."
    );
}

#[test]
fn list_never_returns_a_secret() {
    let user = user();
    project(&user, "app");
    user.call(
        "vault.set",
        json!({"name": "almanac-token", "secret": SECRET}),
    )
    .unwrap();
    turn_on(&user);
    declare(
        &user,
        "almanac",
        json!({"env": {"ALMANAC_TOKEN": {"vault": "almanac-token"}}}),
    );
    let (_, mcp) = root(&user);

    let listed = done(mcp.call("connections", json!({"action": "list"})));
    let shown = done(mcp.call("connections", json!({"action": "show", "name": "almanac"})));

    for (what, said) in [("list", &listed), ("show", &shown)] {
        assert!(!said.contains(SECRET), "{what}: {said}");
        assert!(said.contains("almanac"), "{what}: {said}");
    }
    assert!(listed.contains("ready"), "{listed}");
    assert!(
        shown.contains("ALMANAC_TOKEN") && shown.contains("almanac-token"),
        "a variable by its vault entry's name: {shown}"
    );
    assert!(shown.contains("--script"), "its command: {shown}");
    assert!(
        shown.contains("search") && shown.contains("judged"),
        "{shown}"
    );
}

#[test]
fn a_childs_write_is_refused() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    declare(&user, "almanac", json!({}));
    let (_, lead) = root(&user);
    let opened = user.received_requests("session/new").len();
    let launched = lead.call(
        "agent_launch",
        json!({"kind": "child", "title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);

    let listed = done(child.call("connections", json!({"action": "list"})));
    assert!(listed.contains("almanac"), "{listed}");
    let write = refused(child.call(
        "connections",
        json!({"action": "remove", "name": "almanac"}),
    ));

    assert!(write.contains("ask your parent (ask)"), "{write}");
    assert_eq!(pending(&user), 0);
    assert!(connection(&user, "almanac").is_some());
}

#[test]
fn a_declaration_with_shell_syntax_is_refused_before_the_user_is_asked() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    declare(&user, "almanac", json!({}));
    let (_, mcp) = root(&user);

    let piped = refused(mcp.call(
        "connections",
        json!({"action": "add", "name": "jotter", "about": "Notes.",
            "command": "jotter mcp | tee /tmp/out"}),
    ));
    assert!(piped.contains('|'), "{piped}");
    let far = refused(mcp.call(
        "connections",
        json!({"action": "add", "name": "jotter", "about": "Notes.",
            "url": "http://notes.example.com/mcp"}),
    ));
    assert!(far.contains("https://"), "{far}");
    let secret = refused(mcp.call(
        "connections",
        json!({"action": "edit", "name": "almanac", "env": {"TOKEN": SECRET}}),
    ));
    assert!(secret.contains("vault entry"), "{secret}");
    assert!(!secret.contains(SECRET), "{secret}");
    assert_eq!(pending(&user), 0);
}

#[test]
fn an_agent_turns_a_connection_off_and_sets_an_action_once_the_user_accepts() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    declare(&user, "almanac", json!({}));
    let (_, mcp) = root(&user);

    let number = held(&mcp.call(
        "connections",
        json!({"action": "set", "name": "almanac", "settings": {"search": "passes"}}),
    ));
    let says = request(&user, number)["says"].to_string();
    assert!(says.contains("search") && says.contains("passes"), "{says}");
    approve(&user, number);
    let setting = |name: &str| {
        connection(&user, "almanac").unwrap()["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap()["setting"]
            .clone()
    };
    assert_eq!(setting("search"), "passes");
    assert_eq!(setting("write"), "judged");

    let number = held(&mcp.call("connections", json!({"action": "off", "name": "almanac"})));
    approve(&user, number);
    assert_eq!(connection(&user, "almanac").unwrap()["state"], "off");
}

#[test]
fn the_user_saves_their_own_values_for_a_request() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let line = line_of(&user, "almanac");
    let number = held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "almanac", "about": "Notes.", "command": line}),
    ));

    // "Edit" on the card: the form, filled with the agent's values, saved with the user's own.
    user.call(
        "connection.add",
        json!({"name": "almanac", "about": "The notes of the user.",
            "command": flottille_core::command::split_line(&line).unwrap(),
            "request": number}),
    )
    .unwrap();

    assert_eq!(pending(&user), 0, "the request is answered");
    let body = delivered(&user, number);
    assert!(body.contains("with changes of their own"), "{body}");
    assert!(body.contains("The notes of the user."), "{body}");
    assert_eq!(
        connection(&user, "almanac").unwrap()["about"],
        "The notes of the user."
    );
}

/// The vault entry `name`, every project's, for the address `url` (none when `None`).
fn entry(user: &User, name: &str, url: Option<&str>) {
    let mut params = json!({"name": name, "secret": SECRET});
    if let Some(url) = url {
        params["url"] = json!(url);
    }
    user.call("vault.set", params).unwrap();
}

/// Whether the user's Accept of `number` waits, and why: the request stays held.
fn waits(user: &User, number: u64) -> String {
    let (_, why) = user
        .call("approval.approve", json!({"number": number}))
        .unwrap_err();
    request(user, number);
    why
}

#[test]
fn an_agent_cannot_send_an_entry_to_another_host() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    entry(&user, "github-token", Some("https://api.github.com"));
    let (_, mcp) = root(&user);

    // Held for the user, who alone may bind the entry elsewhere: Accept waits until then.
    let sent = held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "notes", "about": "Notes.",
            "url": "https://notes.example.com/mcp",
            "headers": {"Authorization": "github-token"}}),
    ));
    let why = waits(&user, sent);
    assert!(
        why.contains("api.github.com") && why.contains("notes.example.com"),
        "both hosts: {why}"
    );
    assert!(connection(&user, "notes").is_none());

    // Nor by moving a connection the user bound it to: the entry would follow the URL.
    entry(&user, "notes-token", Some("https://notes.example.com"));
    user.call("approval.deny", json!({"number": sent})).unwrap();
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Notes.", "url": "https://notes.example.com/mcp",
            "headers": {"Authorization": {"vault": "notes-token"}}}),
    )
    .unwrap();
    let moved = held(&mcp.call(
        "connections",
        json!({"action": "edit", "name": "notes", "url": "https://elsewhere.example.com/mcp"}),
    ));
    let why = waits(&user, moved);
    assert!(
        why.contains("notes.example.com") && why.contains("elsewhere.example.com"),
        "{why}"
    );
    assert_eq!(
        connection(&user, "notes").unwrap()["url"],
        "https://notes.example.com/mcp"
    );
    // The entries are as the user left them.
    let listed = user.call("vault.list", json!({})).unwrap();
    for (name, url) in [
        ("github-token", "https://api.github.com"),
        ("notes-token", "https://notes.example.com"),
    ] {
        let entry = listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap();
        assert_eq!(entry["url"], url, "{entry}");
    }
}

#[test]
fn an_agent_cannot_bind_an_entry_without_an_address_to_a_header() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    entry(&user, "github-token", None);
    let (_, mcp) = root(&user);

    let sent = held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "notes", "about": "Notes.",
            "url": "https://notes.example.com/mcp",
            "headers": {"Authorization": "github-token"}}),
    ));
    let why = waits(&user, sent);
    assert!(
        why.contains("no address") && why.contains("notes.example.com"),
        "{why}"
    );
    assert!(connection(&user, "notes").is_none());
    assert_eq!(
        user.call("vault.list", json!({})).unwrap()["entries"][0]["url"],
        Value::Null
    );
}

#[test]
fn an_agent_binds_an_entry_of_the_same_host() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    entry(&user, "notes-token", Some("https://NOTES.example.com/api"));
    entry(&user, "github-token", None);
    let (_, mcp) = root(&user);

    let number = held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "notes", "about": "Notes.",
            "url": "https://notes.example.com/mcp",
            "headers": {"Authorization": "notes-token"}}),
    ));
    let proposed = user
        .call("connection.proposed", json!({"number": number}))
        .unwrap();
    assert_eq!(
        proposed["secrets"],
        json!([{"key": "headers", "name": "Authorization", "entry": "notes-token",
            "entry_host": "notes.example.com", "entry_url": "https://NOTES.example.com/api",
            "host": "notes.example.com", "differs": false, "missing": false, "project": null,
            "share": false, "to_bind": false, "origin": "https://notes.example.com"}]),
        "{proposed}"
    );

    // A command's variables may name any entry: the card says where each goes.
    let line = line_of(&user, "almanac");
    let number = held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "almanac", "about": "Notes.", "command": line,
            "env": {"GITHUB_TOKEN": "github-token"}}),
    ));
    let proposed = user
        .call("connection.proposed", json!({"number": number}))
        .unwrap();
    assert_eq!(
        proposed["secrets"],
        json!([{"key": "env", "name": "GITHUB_TOKEN", "entry": "github-token",
            "entry_host": null, "entry_url": null, "host": null, "differs": true,
            "missing": false, "project": null, "share": false, "to_bind": false,
            "origin": null}]),
        "{proposed}"
    );
}

#[test]
fn a_failing_server_never_shows_its_secret() {
    let user = user();
    project(&user, "app");
    user.call(
        "vault.set",
        json!({"name": "almanac-token", "secret": SECRET}),
    )
    .unwrap();
    turn_on(&user);
    let line = line_of(&user, "almanac");
    let mut script = almanac();
    script["stderr"] = json!(format!("no login for {SECRET}"));
    script["exit"] = json!(3);
    std::fs::write(user.path("almanac-script.json"), script.to_string()).unwrap();
    user.call(
        "connection.add",
        json!({"name": "almanac", "about": "The notes of the user.",
            "command": flottille_core::command::split_line(&line).unwrap(),
            "env": {"ALMANAC_TOKEN": {"vault": "almanac-token"}}}),
    )
    .unwrap();
    let (_, mcp) = root(&user);

    let listed = done(mcp.call("connections", json!({"action": "list"})));
    let shown = done(mcp.call("connections", json!({"action": "show", "name": "almanac"})));

    for (what, said) in [("list", &listed), ("show", &shown)] {
        assert!(said.contains("Failing:"), "{what}: {said}");
        assert!(!said.contains(SECRET), "{what}: {said}");
    }
}

#[test]
fn a_request_accepted_is_not_answered_again_by_the_users_save() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let line = line_of(&user, "almanac");
    let number = held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "almanac", "about": "Notes.", "command": line.clone()}),
    ));
    approve(&user, number);

    let (_, again) = user
        .call(
            "connection.add",
            json!({"name": "jotter", "about": "The notes of the user.",
                "command": flottille_core::command::split_line(&line).unwrap(),
                "request": number}),
        )
        .unwrap_err();
    assert!(again.contains("already decided"), "{again}");
    assert!(connection(&user, "jotter").is_none());
}

/// An agent's proposal of a design server that takes its token in a query parameter.
#[test]
fn a_split_or_uuid_shaped_entry_name_is_refused() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    for written in [
        "0123456789-abcdef0123-456789abcd-ef01234567",
        "3f2a9c1e-7b4d-4e8a-9c2f-1a2b3c4d5e6f",
        "3F2A9C1E.7B4D.4E8A.9C2F.1A2B3C4D5E6F",
        "design_0123456789abcdef-0123456789abcdef",
    ] {
        let sent = refused(mcp.call(
            "connections",
            json!({"action": "add", "name": "design", "about": "Designs.",
                "url": "https://design.example.com/mcp",
                "secrets": [{"in": "query", "name": "userToken", "entry": written}]}),
        ));
        assert!(!sent.contains(written), "{sent}");
    }
    assert_eq!(pending(&user), 0);
    // Names of words, a short number among them, are still ones the user is to make.
    for (n, entry) in [
        "design-usertoken",
        "design-api-key-2026",
        "acme.design_v2-token",
    ]
    .into_iter()
    .enumerate()
    {
        held(&mcp.call(
            "connections",
            json!({"action": "add", "name": format!("design{n}"), "about": "Designs.",
                "url": "https://design.example.com/mcp",
                "secrets": [{"in": "query", "name": "userToken", "entry": entry}]}),
        ));
    }
}

#[test]
fn an_entry_name_mixing_letters_and_digits_in_a_long_word_is_refused() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    for written in ["Zq7Wx3Vp9Kd2-Lm8Nb4Rt6Yh1-Pc5Fg0Jk3Sx7", "notes-Zq7Wx3Vp9"] {
        let sent = refused(mcp.call(
            "connections",
            json!({"action": "add", "name": "design", "about": "Designs.",
                "url": "https://design.example.com/mcp",
                "secrets": [{"in": "query", "name": "userToken", "entry": written}]}),
        ));
        assert!(!sent.contains(written), "{sent}");
    }
    assert_eq!(pending(&user), 0);
    for (n, entry) in ["github-token-2", "staging-db-password", "notion-api-v2"]
        .into_iter()
        .enumerate()
    {
        held(&mcp.call(
            "connections",
            json!({"action": "add", "name": format!("design{n}"), "about": "Designs.",
                "url": "https://design.example.com/mcp",
                "secrets": [{"in": "query", "name": "userToken", "entry": entry}]}),
        ));
    }
}

#[test]
fn an_agent_s_add_proposal_with_a_token_like_url_is_refused() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    for url in [
        "https://design.example.com/mcp?usertoken=abc123",
        "https://design.example.com/mcp?X-Amz-Signature=abc123",
        "https://design.example.com/mcp#access_token=abc123",
        "http://design.example.com/mcp?page=abc123",
        "ftp://design.example.com/mcp?userToken=abc123",
    ] {
        let sent = refused(mcp.call(
            "connections",
            json!({"action": "add", "name": "design", "about": "Designs.", "url": url}),
        ));
        assert!(!sent.contains("abc123"), "{url}: {sent}");
    }
    assert_eq!(pending(&user), 0);
}

fn propose_design(mcp: &Mcp) -> u64 {
    held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "design", "about": "Designs.",
            "url": "https://design.example.com/mcp",
            "secrets": [{"in": "query", "name": "userToken", "entry": "design-usertoken"}]}),
    ))
}

#[test]
fn an_agent_proposes_a_connection_whose_entry_does_not_exist_yet() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let number = propose_design(&mcp);
    let proposed = user
        .call("connection.proposed", json!({"number": number}))
        .unwrap();
    assert_eq!(proposed["secrets"][0]["missing"], true, "{proposed}");
    assert_eq!(
        proposed["secrets"][0]["entry"], "design-usertoken",
        "{proposed}"
    );
    assert!(
        proposed["waits"]
            .as_str()
            .is_some_and(|why| why.contains("design-usertoken")),
        "{proposed}"
    );
    // A value that is no entry's name, or that looks like a secret, is still refused, unsaid.
    for written in ["ghp_abcdefghijklmnopqrstuvwxyz0123456789", "not a name"] {
        let sent = refused(mcp.call(
            "connections",
            json!({"action": "add", "name": "other", "about": "Other.",
                "url": "https://other.example.com/mcp",
                "secrets": [{"in": "header", "name": "X-Key", "entry": written}]}),
        ));
        assert!(!sent.contains(written), "{sent}");
    }
    assert_eq!(pending(&user), 1);
}

#[test]
fn accept_waits_for_the_missing_entry_then_takes_it() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let number = propose_design(&mcp);
    let waited = user
        .call("approval.approve", json!({"number": number}))
        .unwrap_err();
    assert!(waited.1.contains("design-usertoken"), "{waited:?}");
    // Still held: nothing was decided.
    request(&user, number);
    assert!(connection(&user, "design").is_none());

    // Saved, on the connection's host: Accept takes it.
    entry(
        &user,
        "design-usertoken",
        Some("https://design.example.com"),
    );
    let proposed = user
        .call("connection.proposed", json!({"number": number}))
        .unwrap();
    assert_eq!(proposed["secrets"][0]["missing"], false, "{proposed}");
    assert!(proposed["waits"].is_null(), "{proposed}");
    assert_eq!(proposed["current"], true, "{proposed}");
    let answer = approve(&user, number);
    assert!(answer.contains("and Flottille has now done it"), "{answer}");
    assert_eq!(
        connection(&user, "design").unwrap()["secrets"],
        json!([{"in": "query", "name": "userToken", "entry": "design-usertoken"}])
    );
}

#[test]
fn query_cookie_and_basic_lines_bound_to_another_host_wait_for_the_user() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    entry(&user, "github-token", Some("https://api.github.com"));
    let (_, mcp) = root(&user);
    for (n, line) in [
        json!({"in": "query", "name": "userToken", "entry": "github-token"}),
        json!({"in": "cookie", "name": "session", "entry": "github-token"}),
        json!({"in": "basic", "entry": "github-token"}),
    ]
    .into_iter()
    .enumerate()
    {
        let number = held(&mcp.call(
            "connections",
            json!({"action": "add", "name": format!("notes{n}"), "about": "Notes.",
                "url": "https://notes.example.com/mcp", "secrets": [line]}),
        ));
        let why = waits(&user, number);
        assert!(
            why.contains("api.github.com") && why.contains("notes.example.com"),
            "{line}: {why}"
        );
    }
}

/// Accept of `number` once its missing entry exists: what the agent is told, or why the Accept
/// was refused.
fn accepted(user: &User, number: u64) -> String {
    match user.call("approval.approve", json!({"number": number})) {
        Ok(_) => delivered(user, number),
        Err((_, why)) => why,
    }
}

#[test]
fn a_missing_entry_created_later_on_another_host_is_refused_at_accept() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let number = propose_design(&mcp);
    entry(&user, "design-usertoken", Some("https://other.example.com"));
    let answer = accepted(&user, number);
    assert!(
        answer.contains("other.example.com") && answer.contains("design.example.com"),
        "{answer}"
    );
    assert!(connection(&user, "design").is_none());
}

#[test]
fn a_missing_entry_created_later_with_no_url_is_refused_at_accept() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let number = propose_design(&mcp);
    entry(&user, "design-usertoken", None);
    let answer = accepted(&user, number);
    assert!(answer.contains("no address"), "{answer}");
    assert!(connection(&user, "design").is_none());
}

#[test]
fn an_agent_s_env_cannot_name_a_missing_entry() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let line = line_of(&user, "almanac");
    let sent = refused(mcp.call(
        "connections",
        json!({"action": "add", "name": "almanac", "about": "Notes.", "command": line,
            "env": {"ALMANAC_TOKEN": "almanac-token"}}),
    ));
    assert!(sent.contains("names no vault entry"), "{sent}");
    assert_eq!(pending(&user), 0);
}

#[test]
fn a_missing_entry_name_that_looks_like_a_secret_is_refused() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    for written in [
        "0123456789abcdef0123456789abcdef01234567",
        "design_0123456789abcdef0123456789abcdef01234567",
    ] {
        let sent = refused(mcp.call(
            "connections",
            json!({"action": "add", "name": "design", "about": "Designs.",
                "url": "https://design.example.com/mcp",
                "secrets": [{"in": "query", "name": "userToken", "entry": written}]}),
        ));
        assert!(!sent.contains(written), "{sent}");
    }
    assert_eq!(pending(&user), 0);
    // A name of words is still one the user is to make.
    propose_design(&mcp);
}

#[test]
fn an_agent_s_request_cannot_show_the_browser() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    declare(&user, "almanac", json!({}));
    let (_, mcp) = root(&user);

    let added = refused(mcp.call(
        "connections",
        json!({"action": "add", "name": "jotter", "about": "Notes.",
            "command": "jotter mcp", "headed": true}),
    ));
    assert!(added.contains("not a key an agent sets"), "{added}");
    let edited = refused(mcp.call(
        "connections",
        json!({"action": "edit", "name": "almanac", "headed": true}),
    ));
    assert!(edited.contains("not a key an agent sets"), "{edited}");
    assert_eq!(pending(&user), 0);
    assert_eq!(connection(&user, "almanac").unwrap()["headed"], Value::Null);
}

/// The URL connection `hippocampe` an agent proposes, a server on this machine over plain http,
/// its bearer key the vault entry `hippocampe-key`: the request's number.
fn propose_hippocampe(mcp: &Mcp) -> u64 {
    held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "hippocampe", "about": "The user's memory.",
            "url": "http://127.0.0.1:7468/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "hippocampe-key"}]}),
    ))
}

fn proposed(user: &User, number: u64) -> Value {
    user.call("connection.proposed", json!({"number": number}))
        .unwrap()
}

fn vault_entry(user: &User, name: &str) -> Value {
    user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no entry {name}"))
}

#[test]
fn an_entry_problem_does_not_make_the_request_stale() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let number = propose_hippocampe(&mcp);

    // Saved for the project the session runs in: one setting away from going through.
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": SECRET, "project": "app",
            "url": "http://127.0.0.1:7468"}),
    )
    .unwrap();
    let card = proposed(&user, number);
    assert_eq!(card["current"], true, "{card}");
    assert!(card["why"].is_null(), "{card}");
    assert_eq!(card["secrets"][0]["project"], "app", "{card}");
    assert!(
        card["waits"]
            .as_str()
            .is_some_and(|why| why.contains("the vault entry hippocampe-key is app's alone")),
        "{card}"
    );
    assert!(waits(&user, number).contains("app's alone"));

    // Every project's but bound to no address: the same request, waiting for its binding.
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "project": null, "url": null}),
    )
    .unwrap();
    let card = proposed(&user, number);
    assert_eq!(card["current"], true, "{card}");
    assert!(card["why"].is_null(), "{card}");
    assert_eq!(card["secrets"][0]["project"], Value::Null, "{card}");
    assert_eq!(card["secrets"][0]["to_bind"], true, "{card}");
    assert_eq!(
        card["secrets"][0]["origin"], "http://127.0.0.1:7468",
        "{card}"
    );
    assert!(waits(&user, number).contains("no address"));

    // Bound to the connection's address: Accept goes through, the agent asking nothing again.
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "url": "http://127.0.0.1:7468"}),
    )
    .unwrap();
    let card = proposed(&user, number);
    assert!(card["waits"].is_null(), "{card}");
    assert_eq!(card["secrets"][0]["to_bind"], false, "{card}");
    let answer = approve(&user, number);
    assert!(answer.contains("and Flottille has now done it"), "{answer}");
    assert!(connection(&user, "hippocampe").is_some());
}

#[test]
fn the_refusal_says_http_on_this_machine_is_an_address() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    entry(&user, "hippocampe-key", None);
    let (_, mcp) = root(&user);
    let number = propose_hippocampe(&mcp);

    let why = waits(&user, number);
    assert!(
        why.contains("its url: https://…, or http:// on this machine"),
        "{why}"
    );
    assert!(why.contains("127.0.0.1"), "{why}");
    assert!(!why.contains("(its url, https://)"), "{why}");
}

#[test]
fn the_connections_tool_says_its_entries_are_every_project_s() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);

    let tools = mcp.tools();
    let description = tools
        .iter()
        .find(|tool| tool["name"] == "connections")
        .and_then(|tool| tool["description"].as_str())
        .unwrap()
        .to_owned();
    assert!(description.contains("every project's"), "{description}");
    assert!(
        description.contains("vault {action: \"ask\"") && description.contains("connection"),
        "{description}"
    );
    // The tool to ask with is there, though the project sees no entry yet.
    let vault = tools
        .iter()
        .find(|tool| tool["name"] == "vault")
        .unwrap_or_else(|| panic!("no vault tool: {:?}", mcp.names()));
    assert!(
        vault["inputSchema"]["properties"]["connection"].is_object(),
        "{vault}"
    );

    // The answer to an add naming an entry says it too.
    let answer = mcp.call(
        "connections",
        json!({"action": "add", "name": "hippocampe", "about": "The user's memory.",
            "url": "http://127.0.0.1:7468/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "hippocampe-key"}]}),
    );
    held(&answer);
    assert!(answer.text.contains("every project's"), "{}", answer.text);
    assert!(
        answer.text.contains("\"connection\": \"hippocampe\""),
        "{}",
        answer.text
    );
    // One naming none is not told.
    let line = line_of(&user, "almanac");
    let answer = mcp.call(
        "connections",
        json!({"action": "add", "name": "almanac", "about": "Notes.", "command": line}),
    );
    held(&answer);
    assert!(!answer.text.contains("every project's"), "{}", answer.text);
}

#[test]
fn an_agent_alone_cannot_share_or_bind_an_entry() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": SECRET, "project": "app"}),
    )
    .unwrap();
    let (_, mcp) = root(&user);

    // Held, not refused: the user decides, and Accept waits for what only the user does.
    let number = propose_hippocampe(&mcp);
    let why = waits(&user, number);
    assert!(
        why.contains("app's alone") && why.contains("no address"),
        "{why}"
    );
    // None of the agent's own calls changes the entry.
    for args in [
        json!({"action": "ask", "name": "hippocampe-key", "why": "the memory's key",
            "connection": "hippocampe"}),
        json!({"action": "ask", "name": "hippocampe-key", "why": "the memory's key",
            "project": "every", "url": "http://127.0.0.1:7468"}),
        json!({"action": "set", "name": "hippocampe-key", "project": "every"}),
    ] {
        let answer = mcp.call("vault", args.clone());
        assert!(answer.error, "{args}: {}", answer.text);
    }
    let edit = mcp.call(
        "connections",
        json!({"action": "edit", "name": "hippocampe", "about": "Another."}),
    );
    assert!(edit.error, "{}", edit.text);
    let entry = vault_entry(&user, "hippocampe-key");
    assert_eq!(entry["project"], "app", "{entry}");
    assert_eq!(entry["url"], Value::Null, "{entry}");
    assert!(connection(&user, "hippocampe").is_none());
    request(&user, number);
}

#[test]
fn the_user_shares_or_binds_only_what_a_pending_request_names() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": SECRET, "project": "app",
            "description": "The memory's key"}),
    )
    .unwrap();
    entry(&user, "github-token", Some("https://api.github.com"));
    let (_, mcp) = root(&user);
    let number = propose_hippocampe(&mcp);

    // Only an entry the request names, and only to where it would send it.
    for method in ["connection.share_entry", "connection.bind_entry"] {
        let (_, why) = user
            .call(method, json!({"number": number, "entry": "github-token"}))
            .unwrap_err();
        assert!(why.contains("github-token"), "{method}: {why}");
        let (_, why) = user
            .call(
                method,
                json!({"number": number + 100, "entry": "hippocampe-key"}),
            )
            .unwrap_err();
        assert!(why.contains("request"), "{method}: {why}");
    }
    assert_eq!(
        vault_entry(&user, "github-token")["url"],
        "https://api.github.com"
    );

    user.call(
        "connection.share_entry",
        json!({"number": number, "entry": "hippocampe-key"}),
    )
    .unwrap();
    user.call(
        "connection.bind_entry",
        json!({"number": number, "entry": "hippocampe-key", "host": "127.0.0.1"}),
    )
    .unwrap();
    // What else it says is kept.
    let entry = vault_entry(&user, "hippocampe-key");
    assert_eq!(entry["project"], Value::Null, "{entry}");
    assert_eq!(entry["url"], "http://127.0.0.1:7468", "{entry}");
    assert_eq!(entry["description"], "The memory's key", "{entry}");
    assert!(proposed(&user, number)["waits"].is_null());

    // A request decided is no longer a reason to widen anything.
    user.call("approval.deny", json!({"number": number}))
        .unwrap();
    let (_, why) = user
        .call(
            "connection.share_entry",
            json!({"number": number, "entry": "hippocampe-key"}),
        )
        .unwrap_err();
    assert!(why.contains("decided"), "{why}");
}

/// The connection `notes` the user declared at `https://old.example.com/mcp`, and an agent's
/// edit that only gives its server the vault entry `notes-key` (every project's, no address):
/// the request's number.
fn edit_adding_a_key(user: &User, mcp: &Mcp) -> u64 {
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Notes.", "url": "https://old.example.com/mcp"}),
    )
    .unwrap();
    entry(user, "notes-key", None);
    held(&mcp.call(
        "connections",
        json!({"action": "edit", "name": "notes",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "notes-key"}]}),
    ))
}

#[test]
fn share_or_bind_on_a_request_that_changed_is_refused() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let number = edit_adding_a_key(&user, &mcp);
    let card = proposed(&user, number);
    assert_eq!(card["current"], true, "{card}");
    assert_eq!(card["secrets"][0]["host"], "old.example.com", "{card}");
    assert_eq!(card["secrets"][0]["to_bind"], true, "{card}");

    // The user moves the connection meanwhile: the request would not be done any more.
    user.call(
        "connection.edit",
        json!({"name": "notes", "url": "https://new.example.com/mcp"}),
    )
    .unwrap();
    assert_eq!(proposed(&user, number)["current"], false);
    for host in ["old.example.com", "new.example.com"] {
        let (_, why) = user
            .call(
                "connection.bind_entry",
                json!({"number": number, "entry": "notes-key", "host": host}),
            )
            .unwrap_err();
        assert!(why.contains("changed"), "{host}: {why}");
    }
    user.call("vault.set", json!({"name": "notes-key", "project": "app"}))
        .unwrap();
    let (_, why) = user
        .call(
            "connection.share_entry",
            json!({"number": number, "entry": "notes-key"}),
        )
        .unwrap_err();
    assert!(why.contains("changed"), "{why}");
    let entry = vault_entry(&user, "notes-key");
    assert_eq!(entry["url"], Value::Null, "{entry}");
    assert_eq!(entry["project"], "app", "{entry}");
}

#[test]
fn bind_is_refused_when_the_host_shown_is_not_the_one_the_daemon_reads() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    let number = edit_adding_a_key(&user, &mcp);

    // A Bind that names no host, or another one than the request sends it to, binds nothing.
    for params in [
        json!({"number": number, "entry": "notes-key"}),
        json!({"number": number, "entry": "notes-key", "host": "new.example.com"}),
    ] {
        let (_, why) = user
            .call("connection.bind_entry", params.clone())
            .unwrap_err();
        assert!(why.contains("old.example.com"), "{params}: {why}");
    }
    assert_eq!(vault_entry(&user, "notes-key")["url"], Value::Null);

    // The host the dialog showed is the one bound.
    user.call(
        "connection.bind_entry",
        json!({"number": number, "entry": "notes-key", "host": "old.example.com"}),
    )
    .unwrap();
    assert_eq!(
        vault_entry(&user, "notes-key")["url"],
        "https://old.example.com"
    );
}

#[test]
fn an_agent_cannot_get_another_projects_entry_offered_for_sharing() {
    let user = user();
    project(&user, "app");
    project(&user, "other");
    turn_on(&user);
    user.call(
        "vault.set",
        json!({"name": "prod-token", "secret": SECRET, "project": "other",
            "url": "https://api.example.com"}),
    )
    .unwrap();
    let (_, mcp) = root(&user);

    // The agent of app names other's private entry: refused at the call, nothing is held.
    let answer = mcp.call(
        "connections",
        json!({"action": "add", "name": "evil", "about": "Somewhere.",
            "url": "https://evil.example/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "prod-token"}]}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("prod-token"), "{}", answer.text);
    assert!(!answer.text.contains("other"), "{}", answer.text);
    let pending = user.call("approval.list", json!({})).unwrap();
    assert_eq!(pending["requests"], json!([]), "{pending}");

    // Its own project's entry is the issue's case: held, Share offered.
    user.call(
        "vault.set",
        json!({"name": "app-token", "secret": SECRET, "project": "app"}),
    )
    .unwrap();
    let number = held(&mcp.call(
        "connections",
        json!({"action": "add", "name": "mine", "about": "Mine.",
            "url": "https://mine.example.com/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "app-token"}]}),
    ));
    let card = proposed(&user, number);
    assert_eq!(card["secrets"][0]["share"], true, "{card}");

    // Moved to another project after the call: no Share is offered, and none is done.
    user.call(
        "vault.set",
        json!({"name": "app-token", "project": "other"}),
    )
    .unwrap();
    let card = proposed(&user, number);
    assert_eq!(card["secrets"][0]["share"], false, "{card}");
    for method in ["connection.share_entry", "connection.bind_entry"] {
        let (_, why) = user
            .call(
                method,
                json!({"number": number, "entry": "app-token", "host": "mine.example.com"}),
            )
            .unwrap_err();
        assert!(why.contains("app-token"), "{method}: {why}");
    }
    let entry = vault_entry(&user, "app-token");
    assert_eq!(entry["project"], "other", "{entry}");
    assert_eq!(entry["url"], Value::Null, "{entry}");
}

#[test]
fn share_or_bind_is_refused_for_an_entry_bound_to_another_use() {
    let user = user();
    project(&user, "app");
    turn_on(&user);
    let (_, mcp) = root(&user);
    // Named while it does not exist yet: held.
    let number = propose_hippocampe(&mcp);
    // Then made for another use, a browser login of another site.
    user.call(
        "vault.set",
        json!({"name": "hippocampe-key", "secret": SECRET, "url": "https://site.example",
            "for": "connection:browser"}),
    )
    .unwrap();
    let card = proposed(&user, number);
    assert_eq!(card["secrets"][0]["share"], false, "{card}");
    assert_eq!(card["secrets"][0]["to_bind"], false, "{card}");
    for method in ["connection.share_entry", "connection.bind_entry"] {
        let (_, why) = user
            .call(
                method,
                json!({"number": number, "entry": "hippocampe-key", "host": "127.0.0.1"}),
            )
            .unwrap_err();
        assert!(why.contains("hippocampe-key"), "{method}: {why}");
    }
    let entry = vault_entry(&user, "hippocampe-key");
    assert_eq!(entry["url"], "https://site.example", "{entry}");
    assert_eq!(entry["for"], "connection:browser", "{entry}");
}
