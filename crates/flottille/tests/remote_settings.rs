//! Turning remote on and saying where this machine is reached, without a file (#1144,
//! `docs/remote.md`, "Identity and keys"): `remote.settings` and `remote.configure`, which write
//! `daemon.toml` themselves and start or stop the link live, and `flottille remote on | off |
//! listen`. Temporary HOME and XDG folders, a hub served on 127.0.0.1, the machines' interfaces
//! said by `FLOTTILLE_TEST_INTERFACES` (a debug build reads it).

use crate::common::User;
use crate::remote::{Machine, TestHub, pc_and_server_on, set_up};

use serde_json::{Value, json};

/// The interfaces the daemon finds: a tailnet one among two ordinary ones, and a WireGuard one.
const INTERFACES: &str = r#"[["eth0","192.168.1.20"],["docker0","172.17.0.1"],["wg0","10.8.0.2"],["tailscale0","100.64.0.7"]]"#;

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
    assert_eq!(on["on"], true);
    // Nowhere until it is said: it dials the others, nobody dials it.
    assert_eq!(on["listen"], Value::Null);
    assert_eq!(on["listening"], Value::Null);
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

    configure(&user, json!({"on": true, "listen": "127.0.0.1:0"}));
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
    assert_eq!(parsed["remote"]["listen"].as_str(), Some("127.0.0.1:0"));

    // Off keeps where it was reached, and everything else; nowhere takes the key away.
    configure(&user, json!({"on": false}));
    let written = toml_of(&user);
    assert!(written.contains("minutes = 10 # a comment"), "{written}");
    assert!(written.contains("on = false"), "{written}");
    assert!(written.contains("listen = \"127.0.0.1:0\""), "{written}");
    configure(&user, json!({"listen": "nowhere"}));
    let written = toml_of(&user);
    assert!(!written.contains("listen"), "{written}");
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
fn a_tailnet_address_is_found_first_and_marked() {
    let user = User::new().with_env("FLOTTILLE_TEST_INTERFACES", INTERFACES);
    let found = settings(&user)["interfaces"].clone();
    let found: Vec<(String, String, bool)> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["name"].as_str().unwrap().to_owned(),
                i["address"].as_str().unwrap().to_owned(),
                i["tailnet"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        found,
        vec![
            ("tailscale0".into(), "100.64.0.7".into(), true),
            ("wg0".into(), "10.8.0.2".into(), true),
            ("eth0".into(), "192.168.1.20".into(), false),
            ("docker0".into(), "172.17.0.1".into(), false),
        ]
    );
    // "tailnet" is the first of them, on the port of the link.
    configure(&user, json!({"on": false, "listen": "tailnet"}));
    assert_eq!(settings(&user)["listen"], "100.64.0.7:7447");
    assert!(
        toml_of(&user).contains("listen = \"100.64.0.7:7447\""),
        "{}",
        toml_of(&user)
    );
}

#[test]
fn no_tailnet_address_is_refused_by_name() {
    let user = User::new().with_env("FLOTTILLE_TEST_INTERFACES", r#"[["eth0","192.168.1.20"]]"#);
    let (code, message) = user
        .call("remote.configure", json!({"listen": "tailnet"}))
        .unwrap_err();
    assert_eq!(code, "invalid-params");
    assert!(message.contains("tailnet"), "{message}");
}

#[test]
fn choosing_where_this_machine_is_reached_listens_there() {
    let user = User::new();
    let pid = user.status()["pid"].clone();
    configure(&user, json!({"on": true}));
    assert_eq!(settings(&user)["listening"], Value::Null);

    let chosen = configure(&user, json!({"listen": "127.0.0.1:0"}));
    let listening = chosen["listening"].as_str().expect("it listens now");
    assert!(listening.starts_with("127.0.0.1:"), "{chosen:#}");
    assert_eq!(chosen["waits"], false);
    // Said as found by the link, with no restart by hand.
    let seen = user.call("remote.machines", json!({})).unwrap();
    assert!(
        seen["this"]["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|address| address["address"] == listening && address["how"] == "found"),
        "{seen:#}"
    );
    assert_eq!(user.status()["pid"], pid, "the daemon was not restarted");

    // Nowhere again: it closes its port and keeps dialling.
    let nowhere = configure(&user, json!({"listen": "nowhere"}));
    assert_eq!(nowhere["listening"], Value::Null);
    assert_eq!(nowhere["listen"], Value::Null);
}

#[test]
fn an_address_that_cannot_be_listened_on_is_refused_and_changes_nothing() {
    let user = User::new();
    configure(&user, json!({"on": true}));
    // A documentation address (RFC 5737): no interface of the machine holds it.
    let (code, message) = user
        .call("remote.configure", json!({"listen": "192.0.2.1:7447"}))
        .unwrap_err();
    assert_eq!(code, "refused", "{message}");
    assert!(!toml_of(&user).contains("192.0.2.1"), "{}", toml_of(&user));
    assert_eq!(settings(&user)["listening"], Value::Null);
    let (code, _) = user
        .call("remote.configure", json!({"listen": "not an address"}))
        .unwrap_err();
    assert_eq!(code, "invalid-params");
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
fn a_change_that_waits_for_the_next_start_says_so() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    let machine = Machine::of(user);
    let before = settings(&machine.user);
    let listening = before["listening"].clone();
    assert!(listening.is_string());

    // An agent runs: the link is not cut under it.
    let folder = machine.user.path("work/harbor");
    machine.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "harbor\n").unwrap();
    machine.user.git(&folder, &["add", "README.md"]);
    machine
        .user
        .git(&folder, &["commit", "--quiet", "-m", "start"]);
    machine
        .user
        .call("project.add", json!({"folder": folder, "name": "harbor"}))
        .unwrap();
    machine
        .user
        .call(
            "agent.start",
            json!({"project": "harbor", "message": "Write the release notes"}),
        )
        .unwrap();

    let waiting = configure(&machine.user, json!({"listen": "127.0.0.2:0"}));
    assert_eq!(waiting["waits"], true, "{waiting:#}");
    assert_eq!(
        waiting["listening"], listening,
        "it still listens where it did"
    );
    assert_eq!(waiting["listen"], "127.0.0.2:0");
    assert!(toml_of(&machine.user).contains("127.0.0.2:0"));
    assert_eq!(settings(&machine.user)["waits"], true);

    // Said back to where it is, the change no longer waits.
    let back = configure(&machine.user, json!({"listen": "127.0.0.1:0"}));
    assert_eq!(back["waits"], false, "{back:#}");
}

#[test]
fn remote_on_and_off_from_the_cli() {
    let user = User::new().with_env("FLOTTILLE_TEST_INTERFACES", INTERFACES);
    let json_of = |args: &[&str]| -> Value {
        let output = user.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };

    let on = json_of(&["remote", "on", "--listen", "127.0.0.1:0", "--json"]);
    assert_eq!(on["on"], true);
    assert!(on["listening"].as_str().unwrap().starts_with("127.0.0.1:"));
    assert!(user.call("remote.machines", json!({})).is_ok());

    let moved = json_of(&["remote", "listen", "nowhere", "--json"]);
    assert_eq!(moved["listening"], Value::Null);
    let moved = json_of(&["remote", "listen", "127.0.0.1:0", "--json"]);
    assert!(moved["listening"].is_string());

    let off = json_of(&["remote", "off", "--json"]);
    assert_eq!(off["on"], false);
    assert!(user.call("remote.machines", json!({})).is_err());

    // The words a person reads: what it did, and where it is reached.
    let said = user.run(&["remote", "on", "--listen", "nowhere"]);
    assert!(said.status.success());
    let said = String::from_utf8_lossy(&said.stdout).into_owned();
    assert!(said.contains("Remote is on"), "{said}");
    assert!(said.contains("nowhere"), "{said}");
    let off = user.run(&["remote", "off"]);
    assert!(String::from_utf8_lossy(&off.stdout).contains("Remote is off"));

    // `tailnet` is the first tailnet address of this machine, on the link's port.
    let tailnet = json_of(&["remote", "listen", "tailnet", "--json"]);
    assert_eq!(tailnet["listen"], "100.64.0.7:7447");
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
