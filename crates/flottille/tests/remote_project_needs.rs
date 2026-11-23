//! What a project that travels with a launch still needs on the other machine (#1214,
//! `docs/remote.md`, "Which project", "What travels"): where that machine makes it, a setup that
//! copies files the machine lacks, held until the user shares them through the vault, and the
//! refusals that name the window's action. A hub served in the test on 127.0.0.1 and daemons, each
//! with its own temporary HOME and XDG folders, in-memory keyring and fake agents; throwaway
//! repositories.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::common;

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

/// A `.env` of two lines: an agent may read one alone.
const TWO_LINES: &str = "DATABASE_URL=postgres://robin:s3cret-pass@db.example.test/ledger\n\
                         API_TOKEN=t0ken-Value-77\n";

/// A root of `pc` whose mode the user switched to Full from a device, signed: its MCP session.
fn full_root(pc: &Machine, server: &Machine) -> common::mcp::Mcp {
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("auto");
    let switched = server
        .user
        .run(&["--machine", "pc", "agent", "mode", &root, "full"]);
    assert!(switched.status.success(), "{}", pc.log());
    mcp
}

/// `pc`'s `.env` made the vault entry `app-.env` and shared with the server.
fn shared_env(pc: &Machine, server: &Machine) {
    pc.user
        .call("vault.from_file", json!({"project": "app", "path": ".env"}))
        .unwrap();
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
    eventually("the entry on server", || {
        server.user.call("vault.list", json!({})).unwrap()["entries"]
            .to_string()
            .contains("app-.env")
    });
}

fn approve(server: &Machine, request: &Value) {
    server
        .user
        .call(
            "approval.approve",
            json!({"number": request["number"].as_u64().unwrap()}),
        )
        .unwrap();
}

#[test]
fn full_holds_a_launch_whose_setup_lays_a_file_and_names_its_entry() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    pc.register("[[setup]]\ncopy = [\".env\"]\n");
    std::fs::write(pc.folder().join(".env"), TWO_LINES).unwrap();
    // The agent there reads the file whole, then its second line alone.
    let second = TWO_LINES.find("API_TOKEN").unwrap();
    server.user.fake_script(json!({"turns": [
        {"when": "Ledger, whole.", "do": [
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}},
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env",
                "range": {"offset": second}, "again": true}}}
        ]}
    ]}));
    signed_in_there(&pc);
    shared_env(&pc, &server);
    let mcp = full_root(&pc, &server);

    let answer = done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Ledger",
            "brief": "Ledger, whole."}),
    ))
    .to_owned();

    // Full: the launch would start at once, but a file is laid from the vault, which the user
    // allows, the entry named.
    assert!(answer.contains("waiting for the user"), "{answer}");
    let request = the_request(&server);
    let card = request.to_string();
    assert!(
        card.contains("app lays .env from the vault entry app-.env shared by pc"),
        "{card}"
    );
    let session = titled(&server, "Ledger");
    let made = std::path::PathBuf::from(project(&server, "app")["folder"].as_str().unwrap());
    assert!(!made.join(".env").exists());

    approve(&server, &request);

    eventually("the .env copied into the workspace", || {
        std::fs::read_to_string(folder_of(&session).join(".env")).is_ok_and(|t| t == TWO_LINES)
    });
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(made.join(".env"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    // What the agent read is masked, the whole file and its line alone.
    let id = session["id"].as_str().unwrap().to_owned();
    eventually("both reads answered", || {
        server
            .user
            .call("agent.read", json!({"agent": id, "after": 0}))
            .unwrap()
            .to_string()
            .matches("[masked]")
            .count()
            >= 2
    });
    let page = server
        .user
        .call("agent.read", json!({"agent": id, "after": 0}))
        .unwrap()
        .to_string();
    for secret in ["s3cret-pass", "t0ken-Value-77"] {
        assert!(!page.contains(secret), "{page}");
    }
}

#[test]
fn in_full_an_entry_of_the_server_s_own_named_as_the_file_is_never_laid() {
    let (_hub, pc, server) = pc_and_server(server_user(None), None);
    needing_env(&pc);
    signed_in_there(&pc);
    // The server's own entry, unrelated, which a launch's names happen to (or are made to) name.
    let own = "own-t0ken-Zr81";
    server
        .user
        .call("vault.set", json!({"name": "app-.env", "secret": own}))
        .unwrap();
    let mcp = full_root(&pc, &server);

    let answer = done(&launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Ledger",
            "brief": "Ledger, whole."}),
    ))
    .to_owned();

    assert!(answer.contains("waiting for the user"), "{answer}");
    let request = the_request(&server);
    let card = request.to_string();
    assert!(
        card.contains("app needs .env on server: share it?"),
        "{card}"
    );
    let session = titled(&server, "Ledger");
    let made = std::path::PathBuf::from(project(&server, "app")["folder"].as_str().unwrap());

    approve(&server, &request);

    eventually("the session ended", || {
        server.agent(session["id"].as_str().unwrap())["state"] == "ended"
    });
    assert!(
        !made.join(".env").exists(),
        "the server's own entry was laid"
    );
    assert!(!folder_of(&session).join(".env").exists());
    assert!(!server.log().contains(own));
}

#[test]
fn a_launch_naming_an_entry_of_the_server_s_own_is_held_whatever_the_judge_says() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), None);
    signed_in_there(&pc);
    // The server's project github, and its own entry github-token, of no file.
    let folder = server.user.path("work/github");
    register_other(&server, "github", &folder);
    server
        .user
        .call(
            "vault.set",
            json!({"name": "github-token", "secret": "ghp-0wn-t0ken-Qz7"}),
        )
        .unwrap();
    let key = pc.stand_in();
    let mut asked = crate::remote_launch::launch_ask("l-github", "root1");
    asked.mode = "auto".into();
    asked.project = "github".into();
    asked.copied = vec!["token".into()];

    let answer = crate::remote_launch::sent_with(
        &key,
        &server,
        vec![flottille_link::MachineMessage::Launch(Box::new(asked))],
    )
    .remove(0)
    .unwrap();

    // The judge would allow it: what it lays from the vault is the user's to decide.
    assert_eq!(answer["state"], "held", "{answer:#}");
    assert!(!folder.join("token").exists());
}

#[test]
fn copied_paths_that_are_not_inside_or_too_many_are_refused_on_arrival() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    signed_in_there(&pc);
    let key = pc.stand_in();
    let with = |launch: &str, copied: Vec<String>| {
        let mut asked = crate::remote_launch::launch_ask(launch, "root1");
        asked.mode = "auto".into();
        asked.copied = copied;
        flottille_link::MachineMessage::Launch(Box::new(asked))
    };
    let many: Vec<String> = (0..33).map(|n| format!("f{n}")).collect();

    let answers = crate::remote_launch::sent_with(
        &key,
        &server,
        vec![
            with("l-up", vec!["../outside".into()]),
            with("l-root", vec!["/etc/passwd".into()]),
            with("l-git", vec![".git/config".into()]),
            with("l-many", many),
        ],
    );

    for answer in &answers {
        let refused = answer.as_ref().unwrap_err();
        assert!(
            refused.contains("not a path inside the project") || refused.contains("32 files"),
            "{refused}"
        );
    }
    assert!(server.agents().is_empty());
}

#[test]
fn a_file_made_an_entry_is_refused_through_a_link_and_never_replaces_one_silently() {
    let pc = Machine::of(runner_user());
    pc.register("");
    let folder = pc.folder();
    std::fs::create_dir_all(&folder).unwrap();
    let outside = pc.user.path("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("key"), "0utside-s3cret\n").unwrap();
    std::os::unix::fs::symlink(outside.join("key"), folder.join("linked")).unwrap();
    std::os::unix::fs::symlink(&outside, folder.join("conf")).unwrap();
    let from_file = |path: &str, more: Value| {
        let mut params = json!({"project": "app", "path": path});
        params
            .as_object_mut()
            .unwrap()
            .extend(more.as_object().cloned().unwrap_or_default());
        pc.user.call("vault.from_file", params)
    };

    // A link, and a file reached through a linked folder.
    for path in ["linked", "conf/key"] {
        let (code, refused) = from_file(path, json!({})).unwrap_err();
        assert_eq!(code, "refused", "{refused}");
        assert!(!refused.contains("0utside-s3cret"), "{refused}");
    }

    // An entry of that name already: refused unless the call says it replaces it.
    std::fs::create_dir_all(folder.join("a")).unwrap();
    std::fs::write(folder.join("a/b"), "FIRST=f1rst-value\n").unwrap();
    std::fs::write(folder.join("a-b"), "SECOND=s3cond-value\n").unwrap();
    assert_eq!(from_file("a/b", json!({})).unwrap()["name"], "app-a-b");
    let (code, again) = from_file("a/b", json!({})).unwrap_err();
    assert_eq!(code, "refused", "{again}");
    assert!(again.contains("app-a-b"), "{again}");
    assert_eq!(
        from_file("a/b", json!({"replace": true})).unwrap()["name"],
        "app-a-b"
    );
    // Another file whose entry would have the same name: refused, even replacing.
    for more in [json!({}), json!({"replace": true})] {
        let (code, ambiguous) = from_file("a-b", more).unwrap_err();
        assert_eq!(code, "refused", "{ambiguous}");
        assert!(ambiguous.contains("a/b"), "{ambiguous}");
    }
    let listed = pc.user.call("vault.list", json!({})).unwrap();
    assert_eq!(listed["entries"][0]["for"], "file:app/a/b", "{listed:#}");
}

#[test]
fn the_folder_for_projects_from_other_machines_is_never_a_root_the_home_or_flottille_s() {
    let user = common::User::new();
    let home = user.path("home");
    std::fs::create_dir_all(&home).unwrap();
    for folder in [
        "/".to_owned(),
        "~".to_owned(),
        home.to_str().unwrap().to_owned(),
        user.path("data/flottille").to_str().unwrap().to_owned(),
        // It holds the configuration, the state and the data folders.
        user.path("")
            .to_str()
            .unwrap()
            .trim_end_matches('/')
            .to_owned(),
        user.path("data").to_str().unwrap().to_owned(),
    ] {
        let (code, refused) = user
            .call("travelled_folder.set", json!({"folder": folder}))
            .unwrap_err();
        assert_eq!(code, "refused", "{folder}: {refused}");
    }
    // A folder of its own in the home is fine.
    user.call(
        "travelled_folder.set",
        json!({"folder": home.join("from-others")}),
    )
    .unwrap();
}
