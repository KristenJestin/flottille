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
use flottille_core::store::Store;
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

/// The text the agent is handed once the request `number` is answered.
fn delivered(user: &User, number: u64) -> String {
    let store = || Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || store().delivery(&id).unwrap().is_some());
    store().delivery(&id).unwrap().unwrap().body
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
