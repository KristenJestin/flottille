//! Settings for all machines (#1330, `docs/remote.md`, "Settings for all machines"), About you
//! first: the user's device signs `about.share` on one machine, whose daemon seals the values for
//! every machine of the user's and gives the hub the sealed copy; the hub keeps the latest of each
//! value without reading it and hands it to every machine, which keeps the last copy it checked.
//! A machine's own value (`about.toml`) wins on that machine only. A hub and three daemons served
//! in the test, each with its own temporary folders and the in-memory keyring; invented people.

use crate::common;
use crate::remote;

use common::User;
use flottille_link::DeviceKey;
use remote::{Machine, TestHub, error_of, eventually, pair, pc_and_server, signed};
use serde_json::{Value, json};

const SAM: &str = "Sam";
const NOTES: &str = "Backend developer, new to Rust. Answer short and direct.";
/// A vault entry's secret, which never leaves its machine.
const SECRET: &str = "vau1t-s3cret-Qm7Z";

fn about(machine: &Machine) -> Value {
    machine.user.call("about.show", json!({})).unwrap()
}

/// `fields` shared from `machine` as the user's device signs them there.
fn share(machine: &Machine, name: &str, fields: Value) -> Value {
    let answer = machine.raw(&signed(
        &machine.device(),
        &format!("share-{}", rand_id()),
        name,
        "about.share",
        fields,
    ));
    assert!(answer["error"].is_null(), "{answer}");
    answer["result"].clone()
}

fn rand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Waits until `machine`'s About you says `check`.
fn until(machine: &Machine, what: &str, check: impl Fn(&Value) -> bool) {
    eventually(what, || check(&about(machine)));
}

/// What the hub keeps of the values, as it would write it.
fn hub_copy(hub: &TestHub) -> String {
    hub.with_hub(|hub| serde_json::to_string(hub.shared()).unwrap())
}

#[test]
fn a_value_of_all_machines_reaches_every_machine() {
    let (_hub, pc, server) = pc_and_server(User::new());
    // Without a value of all machines yet, each says it has a hub to share with.
    assert!(about(&server)["all"].is_object(), "{}", about(&server));

    let shown = share(&pc, "pc", json!({"name": SAM, "notes": NOTES}));

    assert_eq!(shown["all"]["name"], SAM, "{shown}");
    assert_eq!(shown["name"], SAM, "{shown}");
    until(
        &server,
        "the server has the name of all machines",
        |about| about["name"] == SAM && about["all"]["notes"] == NOTES,
    );
    let server_about = about(&server);
    assert_eq!(server_about["own"], json!([]), "{server_about}");
    // Nothing of it is written in the server's own file.
    assert!(!server.user.path("config/flottille/about.toml").exists());
}

#[test]
fn a_machine_that_was_off_gets_it_at_its_connect() {
    let (_hub, pc, server) = pc_and_server(User::new());
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");

    share(&pc, "pc", json!({"name": SAM}));

    // Started again, it reads the hub's copy as it connects.
    until(
        &server,
        "the server got the value at its connect",
        |about| about["name"] == SAM,
    );
}

#[test]
fn an_override_wins_on_its_machine_only() {
    let (_hub, pc, server) = pc_and_server(User::new());
    share(&pc, "pc", json!({"name": SAM, "pronouns": "he/him"}));
    // Each value is a copy of its own, which may come apart.
    until(&server, "the server has both values", |about| {
        about["name"] == SAM && about["pronouns"] == "he/him"
    });

    let own = server
        .user
        .call("about.set", json!({"name": "Samuel"}))
        .unwrap();

    assert_eq!(own["name"], "Samuel", "{own}");
    assert_eq!(own["own"], json!(["name"]), "{own}");
    assert_eq!(own["all"]["name"], SAM, "{own}");
    // The field not changed there still follows all machines.
    assert_eq!(own["pronouns"], "he/him", "{own}");
    // pc keeps the value of all machines.
    let pc_about = about(&pc);
    assert_eq!(pc_about["name"], SAM, "{pc_about}");
    assert_eq!(pc_about["own"], json!([]), "{pc_about}");
}

#[test]
fn use_all_machines_value_restores_the_hub_s() {
    let (_hub, pc, server) = pc_and_server(User::new());
    share(&pc, "pc", json!({"name": SAM}));
    until(&server, "the server has the value", |about| {
        about["name"] == SAM
    });
    server
        .user
        .call("about.set", json!({"name": "Samuel"}))
        .unwrap();

    // Use all machines' value: the machine's own is dropped.
    let back = server.user.call("about.set", json!({"name": ""})).unwrap();

    assert_eq!(back["name"], SAM, "{back}");
    assert_eq!(back["own"], json!([]), "{back}");
}

#[test]
fn the_latest_of_two_changes_wins() {
    let (_hub, pc, server) = pc_and_server(User::new());
    share(&pc, "pc", json!({"name": "First"}));
    until(&server, "the first reached the server", |about| {
        about["name"] == "First"
    });

    share(&server, "server", json!({"name": "Second"}));

    until(&pc, "the latest reached pc", |about| {
        about["name"] == "Second"
    });
    // A change of one value leaves the others as they were.
    share(&pc, "pc", json!({"notes": NOTES}));
    until(&server, "the notes reached the server", |about| {
        about["notes"] == NOTES && about["name"] == "Second"
    });
}

#[test]
fn the_hub_cannot_read_a_value() {
    let (hub, pc, server) = pc_and_server(User::new());
    share(&pc, "pc", json!({"name": SAM, "notes": NOTES}));
    until(&server, "the server has the value", |about| {
        about["name"] == SAM
    });

    let kept = hub_copy(&hub);
    let file = std::fs::read_to_string(hub.state_file()).unwrap();

    assert!(kept.contains("about.name"), "the hub keeps it: {kept}");
    for text in [SAM, NOTES, "Backend"] {
        assert!(!kept.contains(text), "the hub reads {text}: {kept}");
        assert!(!file.contains(text), "hub.json holds {text}: {file}");
    }
}

#[test]
fn a_machine_with_the_hub_out_of_reach_keeps_its_last_values() {
    let (mut hub, pc, server) = pc_and_server(User::new());
    share(&pc, "pc", json!({"name": SAM}));
    until(&server, "the server has the value", |about| {
        about["name"] == SAM
    });

    hub.stop();
    // Started again with the hub out of reach: what it last checked is what its agents get.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    let kept = about(&server);

    assert_eq!(kept["name"], SAM, "{kept}");
    assert_eq!(kept["all"]["name"], SAM, "{kept}");
    // A change made meanwhile on pc waits for the hub, then reaches the server.
    share(&pc, "pc", json!({"name": "Later"}));
    hub.start_again();
    until(
        &server,
        "the change reached the server once the hub was back",
        |about| about["name"] == "Later",
    );
}

#[test]
fn no_secret_is_ever_in_what_the_hub_stores() {
    let (hub, pc, server) = pc_and_server(User::new());
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    share(&pc, "pc", json!({"name": SAM, "notes": NOTES}));
    until(&server, "the server has the value", |about| {
        about["name"] == SAM
    });

    let kept = hub_copy(&hub);
    let file = std::fs::read_to_string(hub.state_file()).unwrap();

    for text in [&kept, &file] {
        assert!(!text.contains(SECRET), "a vault secret: {text}");
    }
    // Only the values of About you are there, by their names: no vault value, token, key or
    // project variable has one.
    let names: Vec<String> = hub.with_hub(|hub| {
        hub.shared()
            .iter()
            .map(|sealed| sealed.name.clone())
            .collect()
    });
    assert!(!names.is_empty());
    for name in &names {
        assert!(
            ["about.name", "about.pronouns", "about.notes"].contains(&name.as_str()),
            "{name} is not a value of all machines"
        );
    }
    // A name that is no value of all machines is refused before anything is sealed.
    let answer = pc.raw(&signed(
        &pc.device(),
        "s",
        "pc",
        "about.share",
        json!({"secret": SECRET}),
    ));
    assert!(!error_of(&answer).is_empty(), "{answer}");
    assert!(!hub_copy(&hub).contains("secret"));
}

#[test]
fn a_value_needs_the_device_s_signature() {
    let (_hub, pc, _server) = pc_and_server(User::new());

    let (_, unsigned) = pc
        .user
        .call("about.share", json!({"name": SAM}))
        .unwrap_err();
    assert!(
        unsigned.contains("signed by the user's device"),
        "{unsigned}"
    );
    let stranger = DeviceKey::make().unwrap();
    let answer = pc.raw(&signed(
        &stranger,
        "1",
        "pc",
        "about.share",
        json!({"name": SAM}),
    ));
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );
    assert!(about(&pc)["all"]["name"] == "", "{}", about(&pc));
}

#[test]
fn a_viewer_cannot_change_the_values_of_all_machines() {
    let (hub, pc, server) = pc_and_server(User::new());
    let phone = Machine::new();
    pair(&hub, &phone, "phone", "viewer");
    pc.wait_for_member("phone", true);

    let answer = pc.raw(&signed(
        &phone.device(),
        "1",
        "pc",
        "about.share",
        json!({"name": SAM}),
    ));

    assert!(error_of(&answer).contains("may only view"), "{answer}");
    assert_eq!(about(&server)["all"]["name"], "");
}

#[test]
fn without_a_hub_about_you_is_as_today() {
    let user = User::new();

    let shown = user.call("about.set", json!({"name": SAM})).unwrap();

    assert_eq!(shown["name"], SAM, "{shown}");
    assert!(shown["all"].is_null(), "{shown}");
    let (_, refused) = user.call("about.share", json!({"name": SAM})).unwrap_err();
    assert!(!refused.is_empty());
}
