//! Settings for all machines that a machine often overrides (#1462, `docs/remote.md`, "Settings
//! for all machines"): new sessions' provider, model and effort, the judge's, compactor's and
//! review's models, Sessions, the workspaces folder when it is a `~/…` template, the check for
//! updates and the `https://` connections. Each is a value of all machines as About you is: signed
//! by the user's device on one machine, kept by the hub without reading it, read on every machine
//! under that machine's own choice. A hub and two daemons served in the test, each with its own
//! temporary folders and the in-memory keyring; invented people and servers.

use crate::common;
use crate::remote;

use std::path::PathBuf;

use common::User;
use remote::{Machine, TestHub, error_of, eventually, pc_and_server_on, signed};
use serde_json::{Value, json};

/// A vault entry's secret, which never leaves its machine.
const SECRET: &str = "vau1t-s3cret-Qm7Z";
/// A project variable's value, which never leaves its machine.
const VARIABLE: &str = "pr0ject-var-Lk2";
const TEMPLATE: &str = "~/work/<project>-ws";
/// A server that answers nowhere (`.invalid` is no host).
const ALMANAC: &str = "https://almanac.invalid/mcp";

/// A machine whose agents are the fake agent offering `fake-small` and `fake-large`, whose daemon
/// would check for updates against a server that is not there, were its settings to say so.
fn user() -> User {
    let user = User::with_fake_agent()
        .with_update_check()
        .with_env("FLOTTILLE_RELEASES", "http://127.0.0.1:9/repos/o/r");
    user.fake_script(
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"],
        "turns": [{"when": "", "do": [{"say": "ready"}]}]}),
    );
    user
}

fn two() -> (TestHub, Machine, Machine) {
    pc_and_server_on(Machine::of(user()), user())
}

/// The line `pc`'s client writes to change values of all machines: `values` by name, signed by
/// its device.
fn share_line(machine: &Machine, name: &str, values: Value) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    signed(
        &machine.device(),
        &format!("often-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        name,
        "settings.share",
        json!({"values": values}),
    )
}

fn share(machine: &Machine, name: &str, values: Value) -> Value {
    let answer = machine.raw(&share_line(machine, name, values));
    assert!(answer["error"].is_null(), "{answer}");
    answer["result"].clone()
}

fn refused(machine: &Machine, values: Value) -> String {
    error_of(&machine.raw(&share_line(machine, "pc", values)))
}

fn shared(machine: &Machine) -> Value {
    machine.user.call("settings.shared", json!({})).unwrap()
}

fn call(machine: &Machine, method: &str, params: Value) -> Value {
    machine
        .user
        .call(method, params)
        .unwrap_or_else(|error| panic!("{method}: {error:?}"))
}

fn hub_copy(hub: &TestHub) -> String {
    hub.with_hub(|hub| serde_json::to_string(hub.shared()).unwrap())
}

/// Every value of the part, as `settings.share` takes them.
fn every_value() -> Value {
    json!({
        "new_session": r#"{"provider":"claude","model":"fake-large","effort":"high"}"#,
        "judge": r#"{"provider":"claude","model":"fake-small"}"#,
        "compactor": r#"{"provider":"claude","model":"fake-small"}"#,
        "review.spec_model": r#"{"provider":"claude","model":"fake-large"}"#,
        "review.bugs_model": r#"{"provider":"claude","model":"fake-large"}"#,
        "review.standards_model": r#"{"provider":"claude","model":"fake-small"}"#,
        "review.security_model": r#"{"provider":"claude","model":"fake-large"}"#,
        "review.verifier_model": r#"{"provider":"claude","model":"fake-small"}"#,
        "sessions.end_inactive_days": "30",
        "sessions.resume_at_reset": "true",
        "workspaces_folder": TEMPLATE,
        "update.check": "never",
        "connections": serde_json::to_string(&json!([
            {"name": "almanac", "about": "Notes of the team.", "url": ALMANAC}
        ])).unwrap(),
    })
}

fn setting(machine: &Machine, key: &str) -> Value {
    let listed = call(machine, "extension.list", json!({}));
    let review = listed["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|extension| extension["name"] == "review")
        .unwrap()
        .clone();
    review["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|setting| setting["key"] == key)
        .unwrap()["value"]
        .clone()
}

fn connection(machine: &Machine, name: &str) -> Option<Value> {
    call(machine, "connection.list", json!({}))["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|connection| connection["name"] == name)
        .cloned()
}

/// Whether the machine reads every value of `every_value`.
fn reads_every_value(machine: &Machine) -> bool {
    let providers = call(machine, "provider.list", json!({}));
    let compaction = call(machine, "compaction.show", json!({}));
    let sessions = call(machine, "sessions.show", json!({}));
    let folder = call(machine, "workspaces_folder.show", json!({}));
    let update = call(machine, "update.show", json!({}));
    providers["new_session"]
        == json!({"provider": "claude", "model": "fake-large", "effort": "high"})
        && providers["judge"] == json!({"provider": "claude", "model": "fake-small"})
        && compaction["compactor"] == json!({"provider": "claude", "model": "fake-small"})
        && setting(machine, "spec_model")["model"] == "fake-large"
        && setting(machine, "bugs_model")["model"] == "fake-large"
        && setting(machine, "standards_model")["model"] == "fake-small"
        && setting(machine, "security_model")["model"] == "fake-large"
        && setting(machine, "verifier_model")["model"] == "fake-small"
        && sessions == json!({"days": 30, "resume": true})
        && folder["place"] == "own"
        && folder["path"] == TEMPLATE
        && update["check"] == "never"
        && connection(machine, "almanac").is_some_and(|c| c["url"] == ALMANAC)
}

/// A project `app` over a throwaway repository with one commit.
fn project(machine: &Machine) -> PathBuf {
    let user = &machine.user;
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    call(
        machine,
        "project.add",
        json!({"folder": folder, "name": "app"}),
    );
    folder
}

/// A session of `app` started with nothing asked: what it was started on.
fn started(machine: &Machine) -> Value {
    let started = call(
        machine,
        "agent.start",
        json!({"project": "app", "message": "hello"}),
    );
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    call(machine, "agent.show", json!({"agent": id}))["agent"].clone()
}

#[test]
fn each_value_reaches_every_machine() {
    let (_hub, pc, server) = two();
    // Without one yet, a machine says it has a hub to share with.
    assert!(shared(&server)["all"].is_object(), "{}", shared(&server));

    let answer = share(&pc, "pc", every_value());

    assert_eq!(answer["all"]["update.check"], "never", "{answer}");
    assert!(reads_every_value(&pc), "{}", shared(&pc));
    eventually("the server reads every value", || {
        reads_every_value(&server)
    });
    assert_eq!(shared(&server)["own"], json!([]), "{}", shared(&server));
    // Nothing of them is written in the server's own files.
    for file in [
        "config/flottille/compaction.toml",
        "config/flottille/sessions.toml",
        "config/flottille/workspaces.toml",
        "config/flottille/updates.toml",
        "config/flottille/extensions.toml",
    ] {
        let text = std::fs::read_to_string(server.user.path(file)).unwrap_or_default();
        assert!(!text.contains("fake-large"), "{file}: {text}");
        assert!(!text.contains("almanac"), "{file}: {text}");
        assert!(!text.contains(TEMPLATE), "{file}: {text}");
    }
}

#[test]
fn a_machine_that_was_off_gets_them_at_its_connect() {
    let (_hub, pc, server) = two();
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");

    share(&pc, "pc", every_value());

    eventually("the server reads every value at its connect", || {
        reads_every_value(&server)
    });
}

#[test]
fn an_override_wins_on_its_machine_only() {
    let (_hub, pc, server) = two();
    share(&pc, "pc", every_value());
    eventually("the server reads every value", || {
        reads_every_value(&server)
    });

    call(
        &server,
        "provider.set_new_session",
        json!({"provider": "claude", "model": "fake-small"}),
    );
    call(&server, "provider.set_judge", json!({}));
    call(
        &server,
        "compaction.set",
        json!({"compactor": {"provider": "claude", "model": "fake-large"}}),
    );
    call(
        &server,
        "extension.set",
        json!({"name": "review", "settings": {"spec_model": {"provider": "claude", "model": "fake-small"}}}),
    );
    // Off, said: a choice of its own, which all machines' "true" does not undo.
    call(&server, "sessions.set", json!({"resume": false}));
    call(&server, "workspaces_folder.set", json!({"place": "beside"}));
    call(&server, "update.set", json!({"check": "automatically"}));

    let providers = call(&server, "provider.list", json!({}));
    assert_eq!(providers["new_session"]["model"], "fake-small");
    assert_eq!(providers["judge"], Value::Null);
    assert_eq!(
        call(&server, "compaction.show", json!({}))["compactor"]["model"],
        "fake-large"
    );
    assert_eq!(setting(&server, "spec_model")["model"], "fake-small");
    // The slot not changed there still follows all machines, as do the days not changed.
    assert_eq!(setting(&server, "bugs_model")["model"], "fake-large");
    assert_eq!(
        call(&server, "sessions.show", json!({})),
        json!({"days": 30, "resume": false})
    );
    assert_eq!(
        call(&server, "workspaces_folder.show", json!({}))["place"],
        "beside"
    );
    assert_eq!(
        call(&server, "update.show", json!({}))["check"],
        "automatically"
    );
    let own = shared(&server)["own"].clone();
    for name in [
        "new_session",
        "judge",
        "compactor",
        "review.spec_model",
        "sessions.resume_at_reset",
        "workspaces_folder",
        "update.check",
    ] {
        assert!(own.as_array().unwrap().contains(&json!(name)), "{own}");
    }
    assert!(
        !own.as_array()
            .unwrap()
            .contains(&json!("sessions.end_inactive_days")),
        "{own}"
    );
    // pc keeps the values of all machines.
    assert!(reads_every_value(&pc));
    assert_eq!(shared(&pc)["own"], json!([]));
}

#[test]
fn use_all_machines_value_restores_it() {
    let (_hub, pc, server) = two();
    share(&pc, "pc", every_value());
    eventually("the server reads every value", || {
        reads_every_value(&server)
    });
    call(
        &server,
        "provider.set_new_session",
        json!({"provider": "claude", "model": "fake-small"}),
    );
    call(&server, "provider.set_judge", json!({}));
    call(&server, "compaction.set", json!({"compactor": "none"}));
    call(
        &server,
        "extension.set",
        json!({"name": "review", "settings": {"spec_model": {"provider": "claude", "model": "fake-small"}}}),
    );
    call(&server, "sessions.set", json!({"days": 3, "resume": false}));
    call(&server, "workspaces_folder.set", json!({"place": "beside"}));
    call(&server, "update.set", json!({"check": "automatically"}));
    // A connection of all machines edited here is this machine's own.
    call(
        &server,
        "connection.edit",
        json!({"name": "almanac", "url": "https://notes.invalid/mcp"}),
    );
    assert_eq!(
        connection(&server, "almanac").unwrap()["url"],
        "https://notes.invalid/mcp"
    );
    assert_eq!(connection(&pc, "almanac").unwrap()["url"], ALMANAC);
    let own = shared(&server)["own"].clone();
    assert!(
        own.as_array()
            .unwrap()
            .contains(&json!("connections.almanac")),
        "{own}"
    );

    let back = call(&server, "settings.use_all", json!({"names": own}));

    assert_eq!(back["own"], json!([]), "{back}");
    assert!(reads_every_value(&server), "{back}");
}

#[test]
fn a_choice_flottille_picked_follows_all_machines() {
    let (_hub, pc, server) = two();
    // A judge and a compactor Flottille picked are no choice of the user's.
    server
        .user
        .write_providers("[judge]\nprovider = \"claude\"\nmodel = \"haiku\"\npicked = true\n");
    std::fs::write(
        server.user.path("config/flottille/compaction.toml"),
        "[compactor]\nprovider = \"claude\"\nmodel = \"haiku\"\npicked = true\n",
    )
    .unwrap();

    share(
        &pc,
        "pc",
        json!({"judge": "none", "compactor": r#"{"provider":"claude","model":"fake-large"}"#}),
    );

    eventually("all machines' choices win over the picked ones", || {
        call(&server, "provider.list", json!({}))["judge"].is_null()
            && call(&server, "compaction.show", json!({}))["compactor"]["model"] == "fake-large"
    });
    assert_eq!(shared(&server)["own"], json!([]));
}

#[test]
fn a_model_a_machine_lacks_is_ignored_there_and_said() {
    let (_hub, pc, server) = two();
    project(&server);
    // A session of the server tells which models it offers.
    let first = started(&server);
    eventually("the server knows its models", || {
        call(&server, "provider.list", json!({}))["providers"][0]["models"].is_array()
    });

    share(
        &pc,
        "pc",
        json!({
            "new_session": r#"{"provider":"claude","model":"elsewhere-huge","effort":"high"}"#,
            "review.spec_model": r#"{"provider":"claude","model":"elsewhere-huge"}"#,
        }),
    );

    eventually("the server says the model it lacks", || {
        shared(&server)["ignored"]["new_session"].is_string()
    });
    let ignored = shared(&server)["ignored"].clone();
    for name in ["new_session", "review.spec_model"] {
        let why = ignored[name].as_str().unwrap_or_default();
        assert!(why.contains("elsewhere-huge"), "{name}: {ignored}");
    }
    // A session starts there all the same, on what the provider proposes.
    let next = started(&server);
    assert_ne!(next["chosen"]["model"], "elsewhere-huge", "{next}");
    assert_eq!(next["provider"], first["provider"]);
}

#[test]
fn an_https_connection_is_declared_on_each_machine_and_no_token_travels() {
    let (hub, pc, server) = two();
    call(
        &pc,
        "vault.set",
        json!({"name": "almanac-token", "secret": SECRET}),
    );
    let value = serde_json::to_string(&json!([{"name": "almanac", "about": "Notes of the team.",
        "url": ALMANAC, "client_id": "flottille-notes", "only": {"space": ["TEAM"]}}]))
    .unwrap();

    share(&pc, "pc", json!({"connections": value}));

    eventually("the server declares the connection", || {
        connection(&server, "almanac").is_some()
    });
    let declared = connection(&server, "almanac").unwrap();
    assert_eq!(declared["url"], ALMANAC, "{declared}");
    assert_eq!(declared["about"], "Notes of the team.", "{declared}");
    // Each machine signs in for itself: nothing of a sign-in is there.
    let kept = hub_copy(&hub);
    let file = std::fs::read_to_string(hub.state_file()).unwrap();
    for text in [&kept, &file] {
        assert!(!text.contains(SECRET), "{text}");
        assert!(!text.contains("almanac.invalid"), "the hub reads: {text}");
    }
    let copied = std::fs::read_to_string(server.folder().join("shared.json")).unwrap();
    assert!(!copied.contains(SECRET), "{copied}");

    // A connection that names a vault entry, a program or a plain address is not shared.
    for wrong in [
        json!([{"name": "tracker", "about": "T.", "url": ALMANAC,
            "secrets": [{"in": "header", "name": "Authorization", "entry": "almanac-token"}]}]),
        json!([{"name": "tracker", "about": "T.", "url": ALMANAC,
            "headers": {"Authorization": {"vault": "almanac-token"}}}]),
        json!([{"name": "tracker", "about": "T.", "command": ["tracker", "mcp"]}]),
        json!([{"name": "tracker", "about": "T.", "url": "http://127.0.0.1:1/mcp"}]),
        json!([{"name": "tracker", "about": "T.", "url": "https://t.invalid/mcp?token=abc"}]),
    ] {
        let why = refused(
            &pc,
            json!({"connections": serde_json::to_string(&wrong).unwrap()}),
        );
        assert!(!why.is_empty(), "{wrong}");
    }
}

#[test]
fn a_connection_taken_away_from_all_machines_leaves_each_machine() {
    let (_hub, pc, server) = two();
    let value = serde_json::to_string(
        &json!([{"name": "almanac", "about": "Notes of the team.", "url": ALMANAC}]),
    )
    .unwrap();
    share(&pc, "pc", json!({"connections": value}));
    eventually("the server declares the connection", || {
        connection(&server, "almanac").is_some()
    });

    share(&pc, "pc", json!({"connections": ""}));

    eventually("the server declares it no more", || {
        connection(&server, "almanac").is_none()
    });
}

#[test]
fn no_secret_is_in_any_value_of_all_machines() {
    let (hub, pc, server) = two();
    call(
        &pc,
        "vault.set",
        json!({"name": "tracker", "secret": SECRET}),
    );
    project(&pc);
    let file = pc.user.path("config/flottille/projects/app.toml");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&format!("\n[variables]\nTOKEN = \"{VARIABLE}\"\n"));
    std::fs::write(&file, text).unwrap();

    share(&pc, "pc", every_value());
    eventually("the server reads every value", || {
        reads_every_value(&server)
    });

    let kept = hub_copy(&hub);
    let file = std::fs::read_to_string(hub.state_file()).unwrap();
    let copied = std::fs::read_to_string(server.folder().join("shared.json")).unwrap();
    for text in [&kept, &file, &copied] {
        assert!(!text.contains(SECRET), "a vault secret: {text}");
        assert!(!text.contains(VARIABLE), "a project variable: {text}");
    }
    // The hub reads no value.
    for value in ["fake-large", TEMPLATE, "almanac.invalid"] {
        assert!(!kept.contains(value), "the hub reads {value}: {kept}");
    }
    // Each value is under its own name, and no other name is one.
    let names: Vec<String> = hub.with_hub(|hub| {
        hub.shared()
            .iter()
            .map(|sealed| sealed.name.clone())
            .collect()
    });
    for name in every_value().as_object().unwrap().keys() {
        assert!(names.contains(name), "{name} is not among {names:?}");
    }
    for wrong in [
        json!({"vault": "x"}),
        json!({"about.name": "x"}),
        json!({"variables": "x"}),
    ] {
        assert!(!refused(&pc, wrong).is_empty());
    }
}

#[test]
fn a_value_a_machine_could_not_set_is_refused_before_it_is_sealed() {
    let (hub, pc, _server) = two();
    for wrong in [
        json!({"new_session": r#"{"provider":"elsewhere"}"#}),
        json!({"new_session": "fake-large"}),
        json!({"judge": r#"{"provider":"claude","picked":true}"#}),
        json!({"compactor": r#"{"model":"fake-large"}"#}),
        json!({"review.spec_model": r#"{"model":"fake-large"}"#}),
        json!({"sessions.end_inactive_days": "0"}),
        json!({"sessions.end_inactive_days": "91"}),
        json!({"sessions.resume_at_reset": "yes"}),
        // Only a template under the home folder: any other folder is one machine's.
        json!({"workspaces_folder": "/srv/work/<project>"}),
        json!({"workspaces_folder": "~/work/all"}),
        json!({"update.check": "daily"}),
        json!({}),
    ] {
        let why = refused(&pc, wrong.clone());
        assert!(!why.is_empty(), "{wrong}");
    }
    assert!(hub_copy(&hub) == "[]", "{}", hub_copy(&hub));
}

#[test]
fn a_change_of_these_values_needs_the_device_s_signature() {
    let (_hub, pc, _server) = two();

    let (_, unsigned) = pc
        .user
        .call(
            "settings.share",
            json!({"values": {"update.check": "never"}}),
        )
        .unwrap_err();

    assert!(
        unsigned.contains("signed by the user's device"),
        "{unsigned}"
    );
    assert_eq!(shared(&pc)["all"], json!({}), "{}", shared(&pc));
}

#[test]
fn without_a_hub_these_settings_are_as_today() {
    let user = user();

    let shown = user.call("settings.shared", json!({})).unwrap();

    assert!(shown["all"].is_null(), "{shown}");
    assert_eq!(shown["own"], json!([]), "{shown}");
    let (_, why) = user
        .call(
            "settings.share",
            json!({"values": {"update.check": "never"}}),
        )
        .unwrap_err();
    assert!(!why.is_empty());
}

#[test]
fn turning_remote_off_leaves_a_machine_its_own_settings() {
    let (_hub, pc, server) = two();
    share(&pc, "pc", every_value());
    eventually("the server reads every value", || {
        reads_every_value(&server)
    });

    call(&server, "remote.configure", json!({"on": false}));

    assert_eq!(
        call(&server, "sessions.show", json!({})),
        json!({"days": "off", "resume": false})
    );
    assert!(shared(&server)["all"].is_null(), "{}", shared(&server));
}
