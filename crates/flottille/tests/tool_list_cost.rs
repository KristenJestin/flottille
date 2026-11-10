//! The tool list of each kind of agent, as the real MCP server serves it (#676, the measure of
//! `docs/research/tool-list-cost.md`): a root, a led session, a worker, a child that is given the
//! role `lead`, and the agents the extensions launch, in a project with nothing declared, in one
//! with a forge and a tracker, and in one with Specs and Review on. And what the server answers to
//! a call that gets its arguments wrong, which is what a model meets when it calls a tool whose
//! schema it did not have in view. Against the real daemon with the fake ACP agent, throwaway
//! repositories and temporary XDG folders; no real agent runs.
//!
//! When `FLOTTILLE_TOOL_LISTS_OUT` names a file, the lists and the answers are written to it as
//! JSON, for `tools/tool-list-cost` to count.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// The review's agents hang in their turn, so that the test is the one to call their tools.
fn script() -> Value {
    json!({"turns": [
        {"when": "of the change of workspace", "do": [{"hang": true}]},
        {"when": "The change, in each repository", "do": [{"hang": true}]},
        {"when": "HANG", "do": [{"hang": true}]}
    ]})
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A project `name` over a throwaway repository, nothing declared.
fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    // A remote on a forge, which nothing ever reaches: the forge is declared, never called.
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
    folder
}

/// The project's repository declares a GitHub forge, and the project a Jira tracker.
fn declare_forge_and_tracker(user: &User, name: &str) {
    user.call(
        "project.set_settings",
        json!({"name": name,
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]}}),
    )
    .unwrap();
}

/// A root of `project`, its MCP session and the place of its `session/new` among the requests;
/// in Full, so that nothing the test launches is held.
fn root(user: &User, project: &str, workspace: Option<&str>) -> (String, Mcp, usize) {
    let opened = user.received_requests("session/new").len();
    let mut params = json!({"project": project, "message": "hello"});
    if let Some(workspace) = workspace {
        params["workspace"] = workspace.into();
    }
    let answer = user.call("agent.run", params).unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    (id, Mcp::initialize(&url, &authorization), opened)
}

/// What `agent_launch` with `arguments` started: its MCP session and the place of its
/// `session/new` among the requests.
fn launched(user: &User, parent: &Mcp, arguments: Value) -> (Mcp, usize) {
    let opened = user.received_requests("session/new").len();
    let answer = parent.call("agent_launch", arguments);
    assert!(!answer.error, "{}", answer.text);
    let (url, authorization) = server_of(user, opened);
    (Mcp::initialize(&url, &authorization), opened)
}

/// The instructions `session/new` carried to the agent: what its system prompt holds, whole.
fn instructions_of(opened: &Value) -> String {
    let prompt = &opened["params"]["_meta"]["claudeCode"]["options"]["systemPrompt"]["prompt"];
    prompt
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The MCP session of the last agent session opened whose instructions `which` accepts, and the
/// place of its `session/new` among the requests.
fn session_of(user: &User, which: impl Fn(&str) -> bool) -> (Mcp, usize) {
    let mut found = None;
    eventually("the session", || {
        found = user
            .received_requests("session/new")
            .into_iter()
            .enumerate()
            .rev()
            .find(|(_, opened)| {
                let prompt =
                    opened["params"]["_meta"]["claudeCode"]["options"]["systemPrompt"]["prompt"][2]
                        .as_str()
                        .unwrap_or_default();
                which(prompt)
            });
        found.is_some()
    });
    let (at, opened) = found.unwrap();
    let server = &opened["params"]["mcpServers"][0];
    (
        Mcp::initialize(
            server["url"].as_str().unwrap(),
            server["headers"][0]["value"].as_str().unwrap(),
        ),
        at,
    )
}

/// One kind of agent: what it is, where it works, and the tools it is served.
struct Kind {
    kind: &'static str,
    project: &'static str,
    mcp: Mcp,
    /// The instructions the agent was given, which ride in every request beside the list.
    instructions: String,
}

impl Kind {
    fn names(&self) -> Vec<String> {
        self.mcp.names()
    }
}

/// Every kind of agent, in the three projects: `plain` (nothing declared), `full` (a forge and a
/// tracker) and `app` (a forge, a tracker, Specs and Review), the only one with a workspace.
fn kinds(user: &User) -> Vec<Kind> {
    project(user, "plain");
    project(user, "full");
    declare_forge_and_tracker(user, "full");

    let folder = project(user, "app");
    declare_forge_and_tracker(user, "app");
    // On for every project, then off for the two that are measured without them.
    for name in ["specs", "review"] {
        user.call("extension.set", json!({"name": name, "enabled": true}))
            .unwrap();
        for off in ["plain", "full"] {
            user.call(
                "extension.set",
                json!({"name": name, "enabled": false, "project": off}),
            )
            .unwrap();
        }
    }
    let workspace = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let workspace = PathBuf::from(workspace["folder"].as_str().unwrap());
    std::fs::write(workspace.join("login.rs"), "pub fn login() {}\n").unwrap();
    user.git(&workspace, &["add", "."]);
    user.git(&workspace, &["commit", "--quiet", "-m", "login"]);
    drop(folder);

    let child = |parent: &Mcp, title: &str, role: Option<&str>| {
        let mut arguments = json!({"kind": "child", "title": title, "brief": "Do the work."});
        if let Some(role) = role {
            arguments["role"] = role.into();
        }
        launched(user, parent, arguments)
    };

    let (_, plain, plain_at) = root(user, "plain", None);
    let (plain_worker, plain_worker_at) = child(&plain, "plain worker", None);
    let (_, full, full_at) = root(user, "full", None);
    let (worker, worker_at) = child(&full, "worker", None);
    let (lead_child, lead_child_at) = child(&full, "lead child", Some("lead"));
    let (led, led_at) = launched(
        user,
        &full,
        json!({"kind": "led", "title": "led", "brief": "Do the work.", "new_workspace": "l"}),
    );
    let (session, extended, extended_at) = root(user, "app", Some("w"));
    let (extended_worker, extended_worker_at) = child(&extended, "extended worker", None);
    user.call("review.start", json!({"agent": session}))
        .unwrap();
    let (pilot, pilot_at) = session_of(user, |prompt| {
        prompt.contains("You are the pilot of a review")
    });
    let text = pilot.call(
        "review",
        json!({"action": "run_pass", "pass": "bugs", "brief": "the login"}),
    );
    assert!(!text.error, "{}", text.text);
    let (pass, pass_at) = session_of(user, |prompt| {
        prompt.contains("You are the bugs pass of a review")
    });

    let opened = user.received_requests("session/new");
    let kind = |kind: &'static str, project: &'static str, mcp: Mcp, at: usize| Kind {
        kind,
        project,
        mcp,
        instructions: instructions_of(&opened[at]),
    };
    vec![
        kind("root", "plain", plain, plain_at),
        kind("worker", "plain", plain_worker, plain_worker_at),
        kind("root", "full", full, full_at),
        kind("worker", "full", worker, worker_at),
        kind(
            "worker with the role lead",
            "full",
            lead_child,
            lead_child_at,
        ),
        kind("led session", "full", led, led_at),
        kind("root with Specs and Review", "app", extended, extended_at),
        kind(
            "worker with Specs and Review",
            "app",
            extended_worker,
            extended_worker_at,
        ),
        kind("review pilot", "app", pilot, pilot_at),
        kind("review pass", "app", pass, pass_at),
    ]
}

/// The server of a connection: one tool per action, a dozen of them, with the words a real
/// server uses.
fn almanac() -> Value {
    let action = |name: &str, says: &str| {
        json!({"name": name, "description": says, "inputSchema": {"type": "object",
            "properties": {"query": {"type": "string"}}}})
    };
    json!({
        "server": {"name": "almanac", "version": "1.0.0"},
        "tools": [
            action("search", "Finds notes by words."),
            action("read", "Reads a note."),
            action("write", "Writes a note."),
            action("edit", "Changes a note."),
            action("delete", "Deletes a note."),
            action("list", "Lists the notes of a folder."),
            action("tag", "Tags a note."),
            action("untag", "Removes a tag."),
            action("link", "Links two notes."),
            action("recent", "Lists the notes changed lately."),
            action("export", "Exports a folder."),
            action("import", "Imports a folder."),
        ],
        "answers": {},
    })
}

/// A root of a project that sees an entry of the vault and, with `connection`, has one connection:
/// what the user's own entries and servers add to a root's list. Each is for every project, so
/// each is measured in a daemon of its own; a session's list follows what changes after it.
fn the_users_own(user: &User, connection: bool) -> Kind {
    project(user, "plain");
    user.call(
        "vault.set",
        json!({"name": "notes-token", "secret": "not-a-real-secret-0123456789",
            "description": "The token of the notes server"}),
    )
    .unwrap();
    if connection {
        let script = user.path("almanac-script.json");
        std::fs::write(&script, almanac().to_string()).unwrap();
        let fake = std::path::Path::new(env!("CARGO_BIN_EXE_flottille"))
            .with_file_name("flottille-fake-mcp");
        assert!(
            fake.exists(),
            "{} is missing: `cargo build --workspace --bins` first",
            fake.display()
        );
        user.call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
        user.call(
            "connection.add",
            json!({"name": "almanac", "about": "The user's notes.",
                "command": [fake, "--script", script]}),
        )
        .unwrap();
        user.call("connection.review", json!({"name": "almanac"}))
            .unwrap();
        user.call("connection.accept", json!({"name": "almanac"}))
            .unwrap();
    }
    let (_, mcp, at) = root(user, "plain", None);
    Kind {
        kind: if connection {
            "root with a vault entry and a connection"
        } else {
            "root with a vault entry"
        },
        project: "plain",
        mcp,
        instructions: instructions_of(&user.received_requests("session/new")[at]),
    }
}

fn kind<'a>(kinds: &'a [Kind], kind: &str, project: &str) -> &'a Kind {
    kinds
        .iter()
        .find(|k| k.kind == kind && k.project == project)
        .unwrap_or_else(|| panic!("no {kind} in {project}"))
}

#[test]
fn a_root_is_served_the_tools_its_project_declares() {
    let user = User::with_fake_agent();
    user.fake_script(script());
    let kinds = kinds(&user);

    // The agent's list is its role's and its project's: a forge and a tracker add one tool each,
    // Specs and Review one each (`docs/design.md` §4.3).
    let plain = kind(&kinds, "root", "plain").names();
    let full = kind(&kinds, "root", "full").names();
    let extended = kind(&kinds, "root with Specs and Review", "app").names();
    assert_eq!(full.len(), plain.len() + 2, "{plain:?} {full:?}");
    assert_eq!(extended.len(), full.len() + 2, "{full:?} {extended:?}");
    for name in ["forge", "tickets"] {
        assert!(full.contains(&name.to_owned()) && !plain.contains(&name.to_owned()));
    }
    for name in ["spec", "review"] {
        assert!(extended.contains(&name.to_owned()) && !full.contains(&name.to_owned()));
    }
}

#[test]
fn a_vault_entry_adds_one_tool_and_a_connection_one_more_whatever_its_actions() {
    let alone = User::with_fake_agent();
    alone.fake_script(script());
    let vault = the_users_own(&alone, false).names();
    let linked = User::with_fake_agent();
    linked.fake_script(script());
    let both = the_users_own(&linked, true).names();

    assert!(vault.contains(&"vault".to_owned()), "{vault:?}");
    assert_eq!(both.len(), vault.len() + 1, "{vault:?} {both:?}");
    assert!(both.contains(&"conn_almanac".to_owned()), "{both:?}");
}

#[test]
fn a_worker_whatever_its_role_and_a_led_session_are_served_the_list_of_their_place_in_the_tree() {
    let user = User::with_fake_agent();
    user.fake_script(script());
    let kinds = kinds(&user);

    // The role is instructions; the tools are the place in the tree's (`docs/design.md` §4.4).
    let worker = kind(&kinds, "worker", "full").names();
    assert_eq!(
        kind(&kinds, "worker with the role lead", "full").names(),
        worker
    );
    assert_eq!(
        kind(&kinds, "led session", "full").names(),
        kind(&kinds, "root", "full").names()
    );
    assert!(worker.len() < kind(&kinds, "root", "full").names().len());
    for none in [
        "agent_launch",
        "agent_send",
        "agent_status",
        "approval_answer",
    ] {
        assert!(!worker.contains(&none.to_owned()), "{none}");
    }
}

#[test]
fn the_agents_an_extension_launches_are_served_a_worker_list_without_ask() {
    let user = User::with_fake_agent();
    user.fake_script(script());
    let kinds = kinds(&user);

    let worker = kind(&kinds, "worker with Specs and Review", "app").names();
    let pilot = kind(&kinds, "review pilot", "app").names();
    let pass = kind(&kinds, "review pass", "app").names();
    let without_ask: Vec<&String> = worker.iter().filter(|name| *name != "ask").collect();
    // The pass has the worker's tools less `ask`; the pilot those and `review`.
    assert_eq!(pass.iter().collect::<Vec<_>>(), without_ask);
    assert!(pilot.contains(&"review".to_owned()), "{pilot:?}");
    assert!(!pilot.contains(&"ask".to_owned()), "{pilot:?}");
}

/// The answer the server gives to a call that is wrong in one way.
struct Misuse {
    tool: String,
    mistake: &'static str,
    arguments: Value,
    /// What the answer has to name for the model to correct the call without the schema.
    expect: Vec<String>,
    answer: Answer,
}

/// Each tool of `mcp` called with no arguments, and with an action that does not exist, which is
/// what a model that never saw the schema may send.
fn misuses(mcp: &Mcp) -> Vec<Misuse> {
    let mut all = Vec::new();
    for tool in mcp.tools() {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        let strings = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        };
        let mut calls = vec![("no arguments", json!({}), strings(&schema["required"]))];
        let actions = strings(&schema["properties"]["action"]["enum"]);
        if !actions.is_empty() {
            calls.push((
                "an action that does not exist",
                json!({"action": "frobnicate"}),
                actions,
            ));
        }
        for (mistake, arguments, expect) in calls {
            let answer = mcp.call(name, arguments.clone());
            all.push(Misuse {
                tool: name.to_owned(),
                mistake,
                arguments,
                expect,
                answer,
            });
        }
    }
    all
}

#[test]
fn a_call_with_wrong_arguments_is_answered_in_under_two_thousand_characters() {
    let user = User::with_fake_agent();
    user.fake_script(script());
    let kinds = kinds(&user);
    let root = kind(&kinds, "root", "full");

    for misuse in misuses(&root.mcp) {
        assert!(
            misuse.answer.text.chars().count() < 2_000,
            "{} ({}): {}",
            misuse.tool,
            misuse.mistake,
            misuse.answer.text
        );
    }
}

#[test]
fn the_events_say_which_agent_called_which_tool_and_how_the_call_ended() {
    // What `tool-list-cost usage` reads of `flottille watch --after 0`: the agents, and their calls.
    let user = User::with_fake_agent();
    user.fake_script(script());
    let kinds = kinds(&user);
    let root = kind(&kinds, "root", "full");
    assert!(root.mcp.call("fs_read", json!({})).error);

    let events = user.watch(&["--after", "0"]).within(Duration::from_secs(2));
    let created: Vec<&Value> = events
        .iter()
        .filter(|e| e["name"] == "agent.created")
        .collect();
    assert!(
        created.iter().any(|e| e["data"]["parent"].is_null()),
        "{created:?}"
    );
    assert!(
        created.iter().any(|e| e["data"]["parent"].is_string()),
        "{created:?}"
    );
    let refused = events
        .iter()
        .find(|e| e["name"] == "agent.tool_served" && e["data"]["tool"] == "fs_read")
        .expect("the refused call is recorded");
    assert_eq!(refused["data"]["outcome"], "refused");
    assert!(refused["data"]["agent"].is_string());
}

#[test]
fn the_lists_and_the_answers_to_misuse_are_written_for_the_measure() {
    let Some(out) = std::env::var_os("FLOTTILLE_TOOL_LISTS_OUT") else {
        return;
    };
    let user = User::with_fake_agent();
    user.fake_script(script());
    let kinds = kinds(&user);
    let root = kind(&kinds, "root with Specs and Review", "app");
    let (alone, linked) = (User::with_fake_agent(), User::with_fake_agent());
    alone.fake_script(script());
    linked.fake_script(script());
    let own_kinds = [the_users_own(&alone, false), the_users_own(&linked, true)];
    let dump = json!({
        "kinds": kinds.iter().chain(&own_kinds).map(|k| json!({
            "kind": k.kind,
            "project": k.project,
            "tools": k.mcp.tools(),
            "instructions": k.instructions,
        })).collect::<Vec<_>>(),
        "misuses": misuses(&root.mcp).into_iter().map(|m| json!({
            "tool": m.tool,
            "mistake": m.mistake,
            "arguments": m.arguments,
            "expect": m.expect,
            "error": m.answer.error,
            "text": m.answer.text,
        })).collect::<Vec<_>>(),
    });
    // The events of that daemon, a sample of what `tool-list-cost usage` reads in the user's own.
    let events: Vec<String> = user
        .watch(&["--after", "0"])
        .within(Duration::from_secs(2))
        .iter()
        .map(Value::to_string)
        .collect();
    let mut events_out = out.clone();
    events_out.push(".events.jsonl");
    std::fs::write(events_out, events.join("\n")).unwrap();
    std::fs::write(out, serde_json::to_string_pretty(&dump).unwrap()).unwrap();
}
