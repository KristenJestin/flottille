//! What a project that travels with a launch still needs on the other machine (#1214,
//! `docs/remote.md`, "Which project", "What travels"): where that machine makes it, a setup that
//! copies files the machine lacks, held until the user shares them through the vault, and the
//! refusals that name the window's action. A hub served in the test on 127.0.0.1 and daemons, each
//! with its own temporary HOME and XDG folders, in-memory keyring and fake agents; throwaway
//! repositories.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::remote_launch::{
    Machine, allowing, done, eventually, folder_of, launched, pc_and_server, prompted, refused,
    runner_user, server_user, titled,
};

/// The text of the caller's ignored `.env`: a secret, which nothing of Flottille's may show.
const ENV: &str = "DATABASE_URL=postgres://robin:s3cret-pass@db.example.test/ledger\n";

/// Waits until `pc` sees the server offer Claude, signed in.
fn signed_in_there(pc: &Machine) {
    eventually("server's agents", || {
        pc.seen("server")["offers"]["agents"]
            .as_array()
            .is_some_and(|agents| agents.iter().any(|agent| agent == "claude"))
    });
}

fn project(machine: &Machine, name: &str) -> Value {
    machine
        .user
        .call("project.show", json!({"name": name}))
        .unwrap()
}

/// The project `name` of `machine` registered by hand in `folder`, a repository of its own.
fn register_other(machine: &Machine, name: &str, folder: &Path) {
    machine.user.git_init(folder);
    let text = format!(
        "folder = \"{}\"\nworkspaces_root = \"{}\"\n[[repositories]]\npath = \".\"\nbase = \
         [\"main\"]\n",
        folder.display(),
        machine.user.path(&format!("work/{name}-work")).display()
    );
    machine.user.write_project_file(name, &text);
    machine.user.call("project.list", json!({})).unwrap();
}

/// A led session started on the server by a root of `pc` in `mode`.
fn led_there(pc: &Machine, title: &str, mode: &str) -> String {
    let (_root, mcp) = pc.root(mode);
    done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": title,
            "brief": format!("{title}, whole.")}),
    ))
    .to_owned()
}

#[test]
fn the_folder_for_projects_from_other_machines_is_a_setting_of_that_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    signed_in_there(&pc);
    let shown = server
        .user
        .call("travelled_folder.show", json!({}))
        .unwrap();
    assert_eq!(
        shown["folder"],
        server
            .user
            .path("data/flottille/projects")
            .to_str()
            .unwrap(),
        "{shown:#}"
    );
    assert!(shown["own"].is_null(), "{shown:#}");

    // A folder inside another project's, or one that leads out through a link, is refused.
    let other = server.user.path("work/ledger");
    register_other(&server, "ledger", &other);
    let (code, inside) = server
        .user
        .call(
            "travelled_folder.set",
            json!({"folder": other.join("from-others")}),
        )
        .unwrap_err();
    assert_eq!(code, "refused", "{inside}");
    assert!(inside.contains("the project ledger"), "{inside}");
    let link = server.user.path("work/through");
    std::os::unix::fs::symlink(&other, &link).unwrap();
    let (_, linked) = server
        .user
        .call("travelled_folder.set", json!({"folder": link}))
        .unwrap_err();
    assert!(linked.contains("link"), "{linked}");
    let (_, relative) = server
        .user
        .call("travelled_folder.set", json!({"folder": "from-others"}))
        .unwrap_err();
    assert!(relative.contains("absolute"), "{relative}");

    let chosen = server.user.path("elsewhere/from-others");
    let set = server
        .user
        .call("travelled_folder.set", json!({"folder": chosen}))
        .unwrap();
    assert_eq!(set["folder"], chosen.to_str().unwrap(), "{set:#}");
    assert_eq!(set["own"], chosen.to_str().unwrap(), "{set:#}");

    led_there(&pc, "Elsewhere", "auto");

    // The project is made in the folder chosen, from now on.
    let made = project(&server, "app");
    assert_eq!(
        made["folder"],
        chosen.join("app").to_str().unwrap(),
        "{made:#}"
    );
    assert!(!server.user.path("data/flottille/projects/app").exists());

    // Back to the default.
    let back = server
        .user
        .call("travelled_folder.set", json!({"folder": null}))
        .unwrap();
    assert!(back["own"].is_null(), "{back:#}");
}

/// `pc`'s project `app` with a setup that copies its ignored `.env`, which `pc` has.
fn needing_env(pc: &Machine) {
    pc.register("[[setup]]\ncopy = [\".env\"]\n");
    std::fs::write(pc.folder().join(".env"), ENV).unwrap();
}

/// The one request of `server` that waits for the user, once there.
fn the_request(server: &Machine) -> Value {
    eventually("a request on server", || !server.requests().is_empty());
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:#?}");
    requests[0].clone()
}

#[test]
fn a_setup_needing_a_secret_holds_the_launch_and_asks_to_share_it() {
    // In Auto, the server's judge would allow the launch: what it lacks is the user's to give.
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    needing_env(&pc);
    signed_in_there(&pc);

    let answer = led_there(&pc, "Ledger", "auto");

    assert!(answer.contains("waiting for the user"), "{answer}");
    let request = the_request(&server);
    let card = request.to_string();
    assert!(
        card.contains("app needs .env on server: share it?"),
        "{card}"
    );
    assert!(card.contains("app-.env"), "the entry to share: {card}");
    let session = titled(&server, "Ledger");
    assert!(!folder_of(&session).join(".env").exists());

    // On pc, the file is offered as a vault entry, made from it there, then shared.
    let made = pc
        .user
        .call("vault.from_file", json!({"project": "app", "path": ".env"}))
        .unwrap();
    assert_eq!(made["name"], "app-.env", "{made:#}");
    let shared = pc.user.run(&[
        "vault",
        "share",
        "app-.env",
        "--machine",
        "server",
        "--json",
    ]);
    assert!(
        shared.status.success(),
        "{}",
        String::from_utf8_lossy(&shared.stderr)
    );
    server
        .user
        .call(
            "approval.approve",
            json!({"number": request["number"].as_u64().unwrap()}),
        )
        .unwrap();

    // Allowed, the file is made from the server's vault and the setup copies it.
    eventually("the .env copied into the workspace", || {
        std::fs::read_to_string(folder_of(&session).join(".env")).is_ok_and(|text| text == ENV)
    });
    prompted(&server, "Ledger, whole.");
}

#[test]
fn refusing_the_share_refuses_the_launch_saying_what_was_missing() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    needing_env(&pc);
    signed_in_there(&pc);
    led_there(&pc, "Ledger", "auto");
    let request = the_request(&server);
    let session = titled(&server, "Ledger");

    server
        .user
        .call(
            "approval.deny",
            json!({"number": request["number"].as_u64().unwrap()}),
        )
        .unwrap();

    // The launch is refused there, and the agent that asked is told what was missing.
    eventually("the session ended", || {
        server.agent(session["id"].as_str().unwrap())["state"] == "ended"
    });
    eventually("pc's root told", || {
        pc.prompts()
            .iter()
            .any(|prompt| prompt.contains("refused") && prompt.contains(".env"))
    });
    assert!(
        !server
            .prompts()
            .iter()
            .any(|prompt| prompt.contains("Ledger, whole.")),
        "a refused launch started"
    );
}

#[test]
fn a_file_made_a_vault_entry_is_in_no_event_log_or_answer() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    needing_env(&pc);
    signed_in_there(&pc);
    led_there(&pc, "Ledger", "auto");
    let request = the_request(&server);

    let made = pc
        .user
        .call("vault.from_file", json!({"project": "app", "path": ".env"}))
        .unwrap();
    // A file outside the project, or none, is refused without a word of what it holds.
    let (_, outside) = pc
        .user
        .call(
            "vault.from_file",
            json!({"project": "app", "path": "../app-work/x"}),
        )
        .unwrap_err();
    pc.user.run(&[
        "vault",
        "share",
        "app-.env",
        "--machine",
        "server",
        "--json",
    ]);
    server
        .user
        .call(
            "approval.approve",
            json!({"number": request["number"].as_u64().unwrap()}),
        )
        .unwrap();
    eventually("the session prompted", || {
        server
            .prompts()
            .iter()
            .any(|prompt| prompt.contains("Ledger, whole."))
    });

    let secret = "s3cret-pass";
    for said in [made.to_string(), outside, request.to_string()] {
        assert!(!said.contains(secret), "{said}");
    }
    for machine in [&pc, &server] {
        assert!(!machine.log().contains(secret), "{}", machine.log());
        let events = machine
            .user
            .watch(&["--after", "0"])
            .within(Duration::from_secs(1));
        let events = serde_json::to_string(&events).unwrap();
        assert!(!events.contains(secret), "{events}");
        let listed = machine.user.call("vault.list", json!({})).unwrap();
        assert!(!listed.to_string().contains(secret), "{listed}");
        let requests = machine
            .user
            .call("approval.list", json!({"all": true}))
            .map(|list| list.to_string())
            .unwrap_or_default();
        assert!(!requests.contains(secret), "{requests}");
    }
}

#[test]
fn a_refused_launch_names_the_window_s_action() {
    // A runner with Claude enabled and not signed in.
    let (_hub, pc, server) = pc_and_server(runner_user(), Some(""));
    eventually("server's offer", || {
        pc.seen("server")["offers"]["projects"]
            .as_array()
            .is_some_and(|projects| !projects.is_empty())
    });
    let (_root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Night", "brief": "Work."}),
    );

    let text = refused(&answer);
    assert!(
        text.contains("Sign in to Claude on server in Settings › Agents"),
        "{text}"
    );
    assert!(!text.contains("flottille "), "a command line: {text}");
    assert!(server.agents().is_empty());

    // Offline: where the window shows it.
    let stopped = server.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success());
    eventually("server offline", || pc.seen("server")["state"] == "offline");
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Night", "brief": "Work."}),
    );
    let text = refused(&answer);
    assert!(text.contains("server is offline since"), "{text}");
    assert!(text.contains("Settings › Machines"), "{text}");
    assert!(!text.contains("flottille "), "a command line: {text}");
}

#[test]
fn the_caller_is_told_when_a_hand_registered_project_keeps_its_own_settings() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.register("rules = \"Theirs.\"");
    signed_in_there(&pc);

    let answer = led_there(&pc, "Kept", "auto");

    assert!(
        answer.contains(
            "It runs in the project app there, which the user registered on server: it keeps \
             its own settings, not yours"
        ),
        "{answer}"
    );
    assert!(
        project(&server, "app")
            .get("rules")
            .is_none_or(|r| r != "Theirs.")
    );
}
