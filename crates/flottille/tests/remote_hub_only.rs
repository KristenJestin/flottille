//! Every link through the hub, and nothing of the direct link left (`docs/remote.md`, decision
//! 3, "The transport"): no listening, no address, and what a user had before the update still
//! starts and links: an old `daemon.toml` with `listen`, a key folder with addresses, a peer of the
//! previous version that still tells its addresses.

use crate::common::User;
use crate::remote::{self, Machine, TestHub};

use std::time::{Duration, Instant};

use flottille_link::{Device, Endpoint, Hello, MachineKey, Tls, Traffic};
use serde_json::{Value, json};

/// A machine whose `daemon.toml` is `toml`, started.
fn machine_with(toml: &str) -> Machine {
    started(User::new(), toml)
}

/// `user`'s machine, its `daemon.toml` `toml`, started.
fn started(user: User, toml: &str) -> Machine {
    std::fs::write(user.path("config/flottille/daemon.toml"), toml).unwrap();
    user.status();
    Machine { user }
}

/// The sockets the user's daemon holds, by kind, read from `/proc`: its UDP sockets, and its TCP
/// sockets that listen.
fn sockets(user: &User) -> (usize, usize) {
    let pid = user.status()["pid"].as_u64().unwrap();
    let held: Vec<String> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
        .filter_map(|target| {
            let target = target.to_string_lossy().into_owned();
            Some(
                target
                    .strip_prefix("socket:[")?
                    .strip_suffix(']')?
                    .to_owned(),
            )
        })
        .collect();
    // Each line of a table: its state is the 4th field, its inode the 10th.
    let count = |tables: &[&str], listening_only: bool| {
        tables
            .iter()
            .filter_map(|table| std::fs::read_to_string(format!("/proc/{pid}/net/{table}")).ok())
            .flat_map(|text| {
                text.lines()
                    .skip(1)
                    .filter_map(|line| {
                        let fields: Vec<&str> = line.split_whitespace().collect();
                        let listens = fields.get(3).is_some_and(|state| *state == "0A");
                        (!listening_only || listens).then(|| fields.get(9).map(|i| i.to_string()))
                    })
                    .flatten()
                    .collect::<Vec<_>>()
            })
            .filter(|inode| held.contains(inode))
            .count()
    };
    (
        count(&["udp", "udp6"], false),
        count(&["tcp", "tcp6"], true),
    )
}

/// What a command printed on its error output.
fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn remote_on_links_through_the_hub() {
    // Remote turned on as the user turns it on: nothing but `on` in its file.
    let (pc_user, server_user) = (User::new(), User::new());
    for user in [&pc_user, &server_user] {
        let on = user.run(&["remote", "on"]);
        assert!(on.status.success(), "{}", stderr(&on));
        let toml = std::fs::read_to_string(user.path("config/flottille/daemon.toml")).unwrap();
        assert!(!toml.contains("listen"), "{toml}");
    }
    let (pc, server) = (Machine { user: pc_user }, Machine { user: server_user });
    let hub = TestHub::new();
    remote::set_up(&hub, &pc);
    remote::pair(&hub, &server, "server", "runner");
    pc.wait_for("server", "online");
    server.wait_for("pc", "online");

    // The link goes through the hub, and the machines say no address of their own nor of the
    // other's.
    let seen = pc.remote();
    assert!(seen["this"].get("addresses").is_none(), "{seen:#}");
    assert!(seen["this"].get("listening").is_none(), "{seen:#}");
    let server_seen = seen["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|machine| machine["name"] == "server")
        .unwrap()
        .clone();
    assert_eq!(
        server_seen["reached"]["address"], "the hub",
        "{server_seen:#}"
    );
    assert!(server_seen.get("addresses").is_none(), "{server_seen:#}");

    // A machine that stops is offline, and links again through the hub once it is back.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    server.user.status();
    pc.wait_for("server", "online");
}

#[test]
fn there_is_no_listen_nor_address_left() {
    let (_hub, pc, server) =
        remote::pc_and_server_on(machine_with("[remote]\non = true\n"), User::new());

    // The command line offers neither.
    for args in [
        &["remote", "listen", "127.0.0.1:7447"][..],
        &["remote", "address", "list"],
        &["remote", "address", "add", "192.0.2.7"],
        &["remote", "on", "--listen", "nowhere"],
    ] {
        let output = pc.user.run(args);
        assert!(!output.status.success(), "{args:?} was taken");
    }
    let help = pc.user.run(&["remote", "--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(!help.contains("listen"), "{help}");
    assert!(!help.contains("address"), "{help}");

    // The protocol neither: no address method, no listen in the settings, and a listen asked is
    // not written.
    for method in ["remote.address.add", "remote.address.remove"] {
        let (code, message) = pc
            .user
            .call(method, json!({"address": "192.0.2.7"}))
            .unwrap_err();
        assert!(
            message.contains("unknown method"),
            "{method}: {code} {message}"
        );
    }
    let settings = pc.user.call("remote.settings", json!({})).unwrap();
    for gone in ["listen", "listening", "interfaces", "port"] {
        assert!(settings.get(gone).is_none(), "{gone}: {settings:#}");
    }
    let _ = pc
        .user
        .call("remote.configure", json!({"listen": "127.0.0.1:0"}));
    let toml = std::fs::read_to_string(pc.user.path("config/flottille/daemon.toml")).unwrap();
    assert!(!toml.contains("listen"), "{toml}");

    // With remote on and linked, neither daemon holds a socket that listens, nor any UDP socket.
    pc.wait_for("server", "online");
    for machine in [&pc, &server] {
        assert_eq!(sockets(&machine.user), (0, 0), "{}", machine.log());
    }
}

#[test]
fn an_old_daemon_toml_with_listen_starts_and_says_it_is_ignored() {
    let toml = "# mine\n[remote]\non = true\nlisten = \"127.0.0.1:0\"\n";
    let pc = machine_with(toml);
    let said = "[remote] listen in daemon.toml is no longer read";
    let log = pc.log();
    assert_eq!(log.matches(said).count(), 1, "{log}");
    // Nothing listens, and the user's file is left as they wrote it.
    assert_eq!(sockets(&pc.user), (0, 0), "{log}");
    assert_eq!(
        std::fs::read_to_string(pc.user.path("config/flottille/daemon.toml")).unwrap(),
        toml
    );

    // It links through the hub as any other.
    let (_hub, pc, _server) = remote::pc_and_server_on(pc, User::new());
    pc.wait_for("server", "online");
    let log = pc.log();
    assert_eq!(log.matches(said).count(), 1, "said at each round: {log}");
}

#[test]
fn a_store_with_addresses_migrates() {
    // A key folder as the previous version left it: addresses declared by hand, and the peers
    // with the addresses they were given and told.
    let user = User::new();
    let folder = user.path("state/flottille/remote");
    std::fs::create_dir_all(&folder).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    let other = MachineKey::load_or_make(&user.path("other"))
        .unwrap()
        .public();
    std::fs::write(
        folder.join("addresses.json"),
        r#"{"addresses": ["192.0.2.7", "server.example.net:7447"]}"#,
    )
    .unwrap();
    std::fs::write(
        folder.join("peers.json"),
        json!({"machines": [{
            "name": "server",
            "key": other,
            "addresses": ["192.0.2.8:7447"],
            "told": ["198.51.100.9:7447"],
        }]})
        .to_string(),
    )
    .unwrap();
    let pc = started(user, "[remote]\non = true\n");

    // At its start, the addresses are gone; the machines it trusts stay.
    remote::eventually("the addresses migrated away", || {
        !folder.join("addresses.json").exists()
    });
    let peers: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("peers.json")).unwrap()).unwrap();
    assert_eq!(
        peers,
        json!({"machines": [{"name": "server", "key": other}]}),
        "{}",
        pc.log()
    );
    assert_eq!(pc.remote()["machines"][0]["name"], "server");
}

#[test]
fn a_peer_that_still_sends_addresses_links() {
    let (_hub, pc, server) = remote::pc_and_server(User::new());

    // The server of the previous version: its machine key, through the hub, telling the pc its
    // addresses as it did on every link.
    let key = MachineKey::load_or_make(&server.folder()).unwrap();
    let pinned: Device =
        serde_json::from_str(&std::fs::read_to_string(server.folder().join("hub.json")).unwrap())
            .unwrap();
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    let pc_key = pc.remote()["this"]["key"].clone();
    let pc_key: flottille_link::PublicKey = serde_json::from_value(pc_key).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (endpoint, answered) = runtime.block_on(async {
        let hello = Hello::ours(env!("CARGO_PKG_VERSION"));
        let mut session = pinned.connect(&key, &hello, &Tls::System).await.unwrap();
        let endpoint = Endpoint::dialler(&key).unwrap();
        endpoint.through(session.relay().unwrap());
        tokio::spawn(async move { while session.next().await.is_some() {} });
        let deadline = Instant::now() + remote::WAIT;
        let link = loop {
            match endpoint
                .connect(pc_key, &[], Traffic::Machine, &hello, "pc")
                .await
            {
                Ok(link) => break link,
                Err(why) => {
                    assert!(Instant::now() < deadline, "{why}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        };
        let answered = link
            .send_raw(br#"{"machine": {"addresses": ["192.0.2.7:7447"]}}"#)
            .await
            .map_err(|error| error.to_string());
        (endpoint, answered)
    });
    // The pc sees the server online over this very link.
    pc.wait_for("server", "online");
    runtime.block_on(endpoint.close());
    assert_eq!(answered.unwrap(), json!({}));
    // Nothing of them is kept.
    let peers = std::fs::read_to_string(pc.folder().join("peers.json")).unwrap();
    assert!(!peers.contains("192.0.2.7"), "{peers}");
}
