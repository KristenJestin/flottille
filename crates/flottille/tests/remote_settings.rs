//! Turning remote on and off without a file (#1144, `docs/remote.md`, "Identity and keys"):
//! `remote.settings` and `remote.configure`, which write `daemon.toml` themselves and start or
//! stop the link live, and `flottille remote on | off`. Temporary HOME and XDG folders, a hub
//! served on 127.0.0.1.

use crate::common::User;
use crate::remote::{Machine, TestHub, pc_and_server_on, set_up};

use serde_json::{Value, json};

fn toml_of(user: &User) -> String {
    std::fs::read_to_string(user.path("config/flottille/daemon.toml")).unwrap_or_default()
}

fn settings(user: &User) -> Value {
    user.call("remote.settings", json!({})).unwrap()
}

fn configure(user: &User, params: Value) -> Value {
    user.call("remote.configure", params).unwrap()
}

#[test]
fn turning_remote_on_starts_the_link_without_a_restart() {
    let user = User::new();
    let pid = user.status()["pid"].clone();
    let (code, message) = user.call("remote.machines", json!({})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("remote is off"), "{message}");
    assert_eq!(settings(&user)["on"], false);

    let on = configure(&user, json!({"on": true}));
    assert_eq!(on, json!({"on": true}));
    let seen = user.call("remote.machines", json!({})).unwrap();
    assert!(seen["this"]["key"].is_string(), "{seen:#}");
    assert_eq!(user.status()["pid"], pid, "the daemon was not restarted");
    assert!(user.path("state/flottille/remote/machine.key").exists());
    assert!(toml_of(&user).contains("on = true"), "{}", toml_of(&user));
}

#[test]
fn the_daemon_toml_keeps_the_users_other_keys_and_comments() {
    let user = User::new();
    let mine = "# my daemon, my words\n[items]\n# every so often\nminutes = 10 # a comment\n\n\
                [remote]\n# turned on later\non = false\n";
    std::fs::write(user.path("config/flottille/daemon.toml"), mine).unwrap();

    configure(&user, json!({"on": true}));
    let written = toml_of(&user);
    for kept in [
        "# my daemon, my words",
        "[items]",
        "# every so often",
        "minutes = 10 # a comment",
        "# turned on later",
    ] {
        assert!(written.contains(kept), "{kept:?} is lost:\n{written}");
    }
    let parsed: toml_edit::DocumentMut = written.parse().unwrap();
    assert_eq!(parsed["remote"]["on"].as_bool(), Some(true));
    assert!(!written.contains("listen"), "{written}");

    // Off keeps everything else.
    configure(&user, json!({"on": false}));
    let written = toml_of(&user);
    assert!(written.contains("minutes = 10 # a comment"), "{written}");
    assert!(written.contains("on = false"), "{written}");
    assert!(written.contains("# every so often"), "{written}");
}

#[test]
fn a_file_that_is_not_toml_is_left_alone() {
    let user = User::new();
    std::fs::write(
        user.path("config/flottille/daemon.toml"),
        "[remote\non = \n",
    )
    .unwrap();
    let (code, message) = user
        .call("remote.configure", json!({"on": true}))
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("daemon.toml"), "{message}");
    assert_eq!(toml_of(&user), "[remote\non = \n");
}

#[test]
fn turning_remote_off_closes_the_links_and_keeps_the_hub() {
    let (_hub, pc, server) = pc_and_server_on(Machine::new(), User::new());
    pc.wait_for("server", "online");
    server.wait_for("pc", "online");
    let hub = pc.remote()["hub"]["url"].clone();
    assert!(hub.is_string());

    let off = configure(&pc.user, json!({"on": false}));
    assert_eq!(off["on"], false);
    let (code, message) = pc.user.call("remote.machines", json!({})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("remote is off"), "{message}");
    // The other machine sees the link closed.
    server.wait_for("pc", "offline");
    // The pairing with the hub is kept: on again, the same hub and the same list.
    configure(&pc.user, json!({"on": true}));
    let again = pc.remote();
    assert_eq!(again["hub"]["url"], hub);
    let members = pc.user.call("remote.members", json!({})).unwrap();
    assert_eq!(
        members["members"].as_array().unwrap().len(),
        2,
        "{members:#}"
    );
    pc.wait_for("server", "online");
}

#[test]
fn remote_on_and_off_from_the_cli() {
    let user = User::new();
    let json_of = |args: &[&str]| -> Value {
        let output = user.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };

    let on = json_of(&["remote", "on", "--json"]);
    assert_eq!(on["on"], true);
    assert!(user.call("remote.machines", json!({})).is_ok());

    let off = json_of(&["remote", "off", "--json"]);
    assert_eq!(off["on"], false);
    assert!(user.call("remote.machines", json!({})).is_err());

    // The words a person reads: what it did.
    let said = user.run(&["remote", "on"]);
    assert!(said.status.success());
    let said = String::from_utf8_lossy(&said.stdout).into_owned();
    assert!(said.contains("Remote is on"), "{said}");
    assert!(said.contains("through the hub"), "{said}");
    let off = user.run(&["remote", "off"]);
    assert!(String::from_utf8_lossy(&off.stdout).contains("Remote is off"));
}

#[test]
fn what_says_remote_is_off_points_to_the_switch_and_the_command() {
    let user = User::new();
    for method in ["remote.machines", "remote.pair", "remote.members"] {
        let (code, message) = user.call(method, json!({})).unwrap_err();
        assert_eq!(code, "refused", "{method}");
        assert!(message.contains("remote is off"), "{message}");
        assert!(message.contains("Settings"), "{message}");
        assert!(message.contains("flottille remote on"), "{message}");
        assert!(!message.contains("daemon.toml"), "{message}");
    }
    // The commands that start with it off say it the same way.
    let output = user.run(&["remote", "members"]);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(said.contains("flottille remote on"), "{said}");
    assert!(!said.contains("daemon.toml"), "{said}");
}

#[test]
fn a_machine_on_the_hub_is_set_up_with_remote_turned_on_from_nothing() {
    let hub = TestHub::new();
    let user = User::new();
    configure(&user, json!({"on": true}));
    let pc = Machine { user };
    set_up(&hub, &pc);
    assert_eq!(
        pc.user.call("remote.members", json!({})).unwrap()["members"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
