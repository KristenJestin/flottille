//! Two machines linked to each other (`docs/remote.md`, "Architecture", "The transport", "The
//! version"): two daemons, each with its own temporary HOME and XDG folders and in-memory keyring,
//! linked over 127.0.0.1, with no hub, no relay and no discovery. Until the hub brings the member
//! list, the test makes them trust each other's key and gives the addresses they reach each other
//! by, in the file the hub will keep (`remote/peers.json`).

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// How long a link may take to come up or go down: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(30);

struct Machine {
    user: User,
}

impl Machine {
    /// A machine with remote on, its link on loopback only, on a port of the system's choosing.
    fn new() -> Machine {
        Machine::with(User::new(), "127.0.0.1:0")
    }

    fn with(user: User, listen: &str) -> Machine {
        Machine::configured(
            user,
            &format!("[remote]\non = true\nlisten = \"{listen}\"\n"),
        )
    }

    /// A machine with remote on and no address to listen on: it only dials.
    fn dialling_only() -> Machine {
        Machine::configured(User::new(), "[remote]\non = true\n")
    }

    fn configured(user: User, daemon_toml: &str) -> Machine {
        std::fs::write(user.path("config/flottille/daemon.toml"), daemon_toml).unwrap();
        // Started at once: its key and its key folder are made.
        user.status();
        Machine { user }
    }

    /// This machine as `remote.machines` says it.
    fn this(&self) -> Value {
        self.user.call("remote.machines", json!({})).unwrap()["this"].clone()
    }

    fn key(&self) -> String {
        self.this()["key"].as_str().unwrap().to_owned()
    }

    fn fingerprint(&self) -> String {
        self.this()["fingerprint"].as_str().unwrap().to_owned()
    }

    /// Its addresses, found by the transport or declared by hand (`how`).
    fn addresses(&self, how: &str) -> Vec<String> {
        self.this()["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|address| address["how"] == how)
            .map(|address| address["address"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The other machines, as this one sees them.
    fn machines(&self) -> Vec<Value> {
        self.user.call("remote.machines", json!({})).unwrap()["machines"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn machine(&self, name: &str) -> Option<Value> {
        self.machines()
            .into_iter()
            .find(|machine| machine["name"] == name)
    }

    /// Waits until this machine sees `name` in `state`; its entry then.
    fn wait_for(&self, name: &str, state: &str) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let seen = self.machine(name);
            if let Some(seen) = &seen
                && seen["state"] == state
            {
                return seen.clone();
            }
            assert!(
                Instant::now() < deadline,
                "{name} is not {state}: {seen:?}\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Waits until `check` holds of what this machine sees of `name`.
    fn wait_until(&self, name: &str, what: &str, check: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let seen = self.machine(name);
            if let Some(seen) = &seen
                && check(seen)
            {
                return seen.clone();
            }
            assert!(
                Instant::now() < deadline,
                "{what}: {seen:?}\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stands in for the hub: this machine trusts `other`'s key, named `name`, and reaches it at
    /// `addresses`.
    fn trust(&self, name: &str, other: &Machine, addresses: &[String]) {
        self.trust_key(name, &other.key(), addresses);
    }

    fn trust_key(&self, name: &str, key: &str, addresses: &[String]) {
        let mut peers = self.peers();
        let machines = peers["machines"].as_array_mut().unwrap();
        machines.retain(|machine| machine["key"] != key);
        machines.push(json!({"name": name, "key": key, "addresses": addresses}));
        std::fs::write(self.peers_file(), peers.to_string()).unwrap();
    }

    fn peers_file(&self) -> std::path::PathBuf {
        self.user.path("state/flottille/remote/peers.json")
    }

    /// What this machine keeps of the others, read from its file.
    fn peers(&self) -> Value {
        match std::fs::read_to_string(self.peers_file()) {
            Ok(text) => serde_json::from_str(&text).unwrap(),
            Err(_) => json!({"machines": []}),
        }
    }

    /// What this machine keeps of `key` in its file.
    fn peer(&self, key: &str) -> Value {
        self.peers()["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|machine| machine["key"] == key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The addresses it knows for `key`: those it was given, then those `key` told it.
    fn known_addresses(&self, key: &str) -> Vec<String> {
        let peer = self.peer(key);
        let mut known = Vec::new();
        for field in ["addresses", "told"] {
            for address in peer[field].as_array().into_iter().flatten() {
                let address = address.as_str().unwrap().to_owned();
                if !known.contains(&address) {
                    known.push(address);
                }
            }
        }
        known
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.user.path("state/flottille/daemon.log")).unwrap_or_default()
    }

    fn stop(&self) {
        assert!(self.user.run(&["daemon", "stop"]).status.success());
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

/// The UDP sockets the user's daemon holds open, read from `/proc`.
fn udp_sockets(user: &User) -> usize {
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
    ["udp", "udp6"]
        .into_iter()
        .filter_map(|table| std::fs::read_to_string(format!("/proc/{pid}/net/{table}")).ok())
        .flat_map(|text| {
            text.lines()
                .skip(1)
                .filter_map(|line| line.split_whitespace().nth(9).map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .filter(|inode| held.contains(inode))
        .count()
}

/// A port nobody uses now on loopback: the system's choice, let go at once.
fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn two_machines_that_trust_each_other_link_over_loopback() {
    let (laptop, server) = (Machine::new(), Machine::new());
    // What the transport finds is the loopback it was told to listen on: no other path, no relay.
    let found = server.addresses("found");
    assert!(!found.is_empty());
    assert!(
        found
            .iter()
            .all(|address| address.starts_with("127.0.0.1:")),
        "{found:?}"
    );

    laptop.trust("server", &server, &server.addresses("found"));
    server.trust("laptop", &laptop, &laptop.addresses("found"));

    let seen = laptop.wait_for("server", "online");
    assert_eq!(seen["key"], server.key());
    assert_eq!(seen["fingerprint"], server.fingerprint());
    assert_eq!(seen["reason"], Value::Null);
    let seen = server.wait_for("laptop", "online");
    assert_eq!(seen["key"], laptop.key());
    assert!(
        laptop.log().contains("linked to server") || server.log().contains("linked to laptop"),
        "{}\n{}",
        laptop.log(),
        server.log()
    );

    // A machine that stops is offline, and links again once it is back.
    server.stop();
    let seen = laptop.wait_for("server", "offline");
    assert!(seen["since"].is_u64(), "{seen}");
    server.user.status();
    laptop.wait_for("server", "online");
}

#[test]
fn a_linked_machine_says_the_address_it_is_reached_at() {
    let (laptop, server) = (Machine::new(), Machine::new());
    // Only the laptop knows where the server is: it is the one that dials.
    laptop.trust("server", &server, &server.addresses("found"));
    server.trust("laptop", &laptop, &[]);

    let seen = laptop.wait_for("server", "online");
    assert_eq!(
        seen["reached"],
        json!({"address": server.addresses("found")[0], "dialled": true}),
        "{seen:#}"
    );

    // The one that was dialled knows where the link came from, and that it did not dial it.
    let seen = server.wait_for("laptop", "online");
    assert_eq!(seen["reached"]["dialled"], false, "{seen:#}");
    assert!(
        seen["reached"]["address"]
            .as_str()
            .is_some_and(|address| address.starts_with("127.0.0.1:")),
        "{seen:#}"
    );

    // A machine that is off is reached at no address.
    server.stop();
    let seen = laptop.wait_for("server", "offline");
    assert_eq!(seen["reached"], Value::Null, "{seen:#}");
}

#[test]
fn a_key_that_is_not_trusted_is_refused() {
    let (laptop, server) = (Machine::new(), Machine::new());
    // The laptop trusts the server; the server knows nothing of the laptop.
    laptop.trust("server", &server, &server.addresses("found"));

    let seen = laptop.wait_until("server", "the server refuses the laptop", |seen| {
        seen["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("refused the link"))
    });
    assert_eq!(seen["state"], "offline");
    assert_eq!(
        seen["reason"],
        "server refused the link: refused: this machine's key is not one of the user's"
    );
    assert!(server.machines().is_empty());
    let log = server.log();
    assert!(
        log.contains(&format!(
            "refused a link from {}: not a machine this one trusts",
            laptop.fingerprint()
        )),
        "{log}"
    );

    // Trusted, it links.
    server.trust("laptop", &laptop, &[]);
    laptop.wait_for("server", "online");
    server.wait_for("laptop", "online");
}

#[test]
fn an_untrusted_key_that_dials_again_is_said_once() {
    let (laptop, server) = (Machine::new(), Machine::new());
    laptop.trust("server", &server, &server.addresses("found"));
    laptop.wait_until("server", "the server refuses the laptop", |seen| {
        seen["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("refused the link"))
    });
    // The laptop dials again after 1 s, then 2 s: refused each time, said once.
    std::thread::sleep(Duration::from_secs(4));
    let line = format!("refused a link from {}", laptop.fingerprint());
    let log = server.log();
    assert_eq!(log.matches(&line).count(), 1, "{log}");
}

#[test]
fn a_flood_of_fresh_keys_is_said_within_a_budget() {
    use flottille_link::{Endpoint, Hello, MachineKey, PublicKey, Traffic};
    let server = Machine::new();
    let addresses: Vec<std::net::SocketAddr> = server
        .addresses("found")
        .iter()
        .map(|address| address.parse().unwrap())
        .collect();
    let server_key: PublicKey = serde_json::from_value(json!(server.key())).unwrap();
    let folder = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Each dial with a key of its own, as a key costs a stranger nothing.
    const DIALS: usize = 60;
    runtime.block_on(async {
        for at in 0..DIALS {
            let key = MachineKey::load_or_make(&folder.path().join(format!("{at}"))).unwrap();
            let stranger = Endpoint::dialler(&key).unwrap();
            let refused = tokio::time::timeout(
                WAIT,
                stranger.connect(
                    server_key,
                    &addresses,
                    Traffic::Machine,
                    &Hello::ours("0.4.1"),
                    "server",
                ),
            )
            .await
            .unwrap();
            assert!(refused.is_err());
        }
    });
    let deadline = Instant::now() + WAIT;
    while !server.log().contains("are counted, not said") {
        assert!(Instant::now() < deadline, "{}", server.log());
        std::thread::sleep(Duration::from_millis(100));
    }
    let log = server.log();
    let said = log.matches("refused a link from ").count();
    assert!(said <= 16, "{said} refusals said:\n{log}");
    assert_eq!(log.matches("are counted, not said").count(), 1, "{log}");
}

#[test]
fn an_address_declared_by_hand_is_used_to_link() {
    let (laptop, server) = (Machine::new(), Machine::new());
    let port = server.addresses("found")[0]
        .rsplit_once(':')
        .unwrap()
        .1
        .to_owned();

    // A name the transport cannot find, declared by hand; its port is the link's own.
    let added = server.user.run(&["remote", "address", "add", "localhost"]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let listed = server.user.run(&["remote", "address", "list"]);
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert!(
        listed.contains(&format!("localhost:{port}  declared")),
        "{listed}"
    );
    assert!(listed.contains("  found"), "{listed}");
    let declared = server.addresses("declared");
    assert_eq!(declared, vec![format!("localhost:{port}")]);

    // The laptop knows the server by its declared address only, and the server knows no address
    // of the laptop: the link is the laptop's, through the name.
    laptop.trust("server", &server, &declared);
    server.trust("laptop", &laptop, &[]);
    laptop.wait_for("server", "online");
    server.wait_for("laptop", "online");

    // Refused: an address with what no address holds; removed, it is gone.
    let refused = server.user.run(&["remote", "address", "add", "local host"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("not an address"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let removed = server
        .user
        .run(&["remote", "address", "remove", "localhost"]);
    assert!(removed.status.success());
    assert!(server.addresses("declared").is_empty());
    let missing = server
        .user
        .run(&["remote", "address", "remove", "localhost"]);
    assert!(!missing.status.success());
}

#[test]
fn a_machine_tells_the_other_its_new_addresses() {
    // The laptop listens on a port of its own, kept across its restart.
    let laptop = Machine::with(User::new(), &format!("127.0.0.1:{}", free_port()));
    let server = Machine::new();
    let (laptop_key, server_key) = (laptop.key(), server.key());
    let given = server.addresses("found");
    // Only the laptop is given an address: the server learns the laptop's from the laptop.
    laptop.trust("server", &server, &given);
    server.trust("laptop", &laptop, &[]);
    laptop.wait_for("server", "online");
    server.wait_for("laptop", "online");

    let told = server.wait_until("laptop", "the laptop told its addresses", |seen| {
        !seen["addresses"].as_array().unwrap().is_empty()
    });
    assert_eq!(strings(&told["addresses"]), laptop.addresses("found"));
    assert_eq!(
        server.known_addresses(&laptop_key),
        laptop.addresses("found")
    );

    // A new address of the server's, declared by hand, is told to the laptop at once and kept.
    assert!(
        server
            .user
            .run(&["remote", "address", "add", "192.0.2.7:4000"])
            .status
            .success()
    );
    laptop.wait_until("server", "the server told its new address", |seen| {
        strings(&seen["addresses"]).contains(&"192.0.2.7:4000".to_owned())
    });
    assert!(
        laptop
            .known_addresses(&server_key)
            .contains(&"192.0.2.7:4000".to_owned())
    );
    // Told addresses add to those the laptop was given, which stay as they were.
    let kept = laptop.peer(&server_key);
    assert_eq!(strings(&kept["addresses"]), given, "{kept}");
    assert!(
        strings(&kept["told"]).contains(&"192.0.2.7:4000".to_owned()),
        "{kept}"
    );
    // Removed, it is told too.
    assert!(
        server
            .user
            .run(&["remote", "address", "remove", "192.0.2.7:4000"])
            .status
            .success()
    );
    laptop.wait_until("server", "the server told the address is gone", |seen| {
        !strings(&seen["addresses"]).contains(&"192.0.2.7:4000".to_owned())
    });

    // Without the hub, they link again by what they told each other: the laptop starts again
    // knowing no address of the server, and the server reaches it at the address it was told.
    laptop.stop();
    server.wait_for("laptop", "offline");
    laptop.trust_key("server", &server_key, &[]);
    laptop.user.status();
    laptop.wait_for("server", "online");
    server.wait_for("laptop", "online");
}

#[test]
fn no_common_protocol_refuses_the_link_saying_which_to_update() {
    let laptop = Machine::new();
    let server = Machine::with(
        User::new().with_env(
            "FLOTTILLE_TEST_LINK_HELLO",
            r#"{"flottille": "0.9.0", "protocols": [99]}"#,
        ),
        "127.0.0.1:0",
    );
    let status = laptop.user.status();
    let (version, protocol) = (
        status["version"].as_str().unwrap().to_owned(),
        status["protocol"].as_u64().unwrap(),
    );
    laptop.trust("server", &server, &server.addresses("found"));
    server.trust("laptop", &laptop, &laptop.addresses("found"));

    let seen = laptop.wait_for("server", "needs an update");
    let sentence = format!(
        "server runs Flottille 0.9.0 (protocol 99), this machine {version} (protocol \
         {protocol}): update one of them (flottille update)"
    );
    assert_eq!(seen["reason"], sentence);
    assert!(laptop.log().contains(&sentence), "{}", laptop.log());
    // Refused on the other side too, with its own point of view.
    let seen = server.wait_for("laptop", "needs an update");
    let sentence = format!(
        "laptop runs Flottille {version} (protocol {protocol}), this machine 0.9.0 (protocol \
         99): update one of them (flottille update)"
    );
    assert_eq!(seen["reason"], sentence);
    assert!(server.log().contains(&sentence), "{}", server.log());
}

#[test]
fn remote_off_has_no_link() {
    let user = User::new();
    let (code, message) = user.call("remote.machines", json!({})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("remote is off"), "{message}");
    assert!(!user.path("state/flottille/remote").exists());
    assert_eq!(udp_sockets(&user), 0);
}

#[test]
fn no_listener_until_an_address_is_set() {
    let laptop = Machine::dialling_only();
    // Remote on, no `[remote] listen`: nothing listens, not even a socket is open.
    assert_eq!(udp_sockets(&laptop.user), 0);
    assert_eq!(laptop.this()["listening"], Value::Null);
    assert!(laptop.addresses("found").is_empty());

    // It still links to a machine that listens, which reaches it over that link.
    let server = Machine::new();
    laptop.trust("server", &server, &server.addresses("found"));
    server.trust("laptop", &laptop, &[]);
    laptop.wait_for("server", "online");
    server.wait_for("laptop", "online");
    assert_eq!(laptop.this()["listening"], Value::Null);
}

#[test]
fn a_half_written_peers_file_keeps_the_links() {
    let (laptop, server) = (Machine::new(), Machine::new());
    laptop.trust("server", &server, &server.addresses("found"));
    server.trust("laptop", &laptop, &laptop.addresses("found"));
    laptop.wait_for("server", "online");
    server.wait_for("laptop", "online");

    // Caught half written (a hand edit, a writer that stopped): the last file read is kept.
    std::fs::write(laptop.peers_file(), r#"{"machines": [{"name": "ser"#).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    let seen = laptop
        .machine("server")
        .expect("the server is still trusted");
    assert_eq!(seen["state"], "online", "{seen}");
    assert_eq!(server.machine("laptop").unwrap()["state"], "online");
    // Said once, not at every round.
    let log = laptop.log();
    assert_eq!(log.matches("peers.json is not read").count(), 1, "{log}");
    assert!(!log.contains("the link to server ended"), "{log}");
}

#[test]
fn two_machines_dialing_at_once_keep_one_link() {
    // Each takes a link it is dialled on only after it dialled the other: two links, one each
    // way, on both sides.
    let late = || User::new().with_env("FLOTTILLE_TEST_LINK_ACCEPT_DELAY_MS", "2500");
    let laptop = Machine::with(late(), "127.0.0.1:0");
    let server = Machine::with(late(), "127.0.0.1:0");
    laptop.trust("server", &server, &server.addresses("found"));
    server.trust("laptop", &laptop, &laptop.addresses("found"));
    laptop.wait_for("server", "online");
    server.wait_for("laptop", "online");

    // Both keep the same one, by a rule both compute: the link the lower key dialled. The side
    // that takes the other's link first holds two and says so; the other side may see the link
    // it dialled closed before it takes the second one, and holds one only.
    let (lower, higher) = if laptop.key() < server.key() {
        (laptop.fingerprint(), server.fingerprint())
    } else {
        (server.fingerprint(), laptop.fingerprint())
    };
    let kept = format!("kept the link dialled by {lower}");
    let deadline = Instant::now() + WAIT;
    while !(laptop.log().contains(&kept) || server.log().contains(&kept)) {
        assert!(
            Instant::now() < deadline,
            "{kept}:\n{}\n{}",
            laptop.log(),
            server.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_secs(3));
    let linked = |log: &str| log.matches(" (protocol ").count();
    let before = [linked(&laptop.log()), linked(&server.log())];
    std::thread::sleep(Duration::from_secs(3));
    // One link, kept: nobody links again.
    assert_eq!(before, [linked(&laptop.log()), linked(&server.log())]);
    assert_eq!(laptop.machine("server").unwrap()["state"], "online");
    assert_eq!(server.machine("laptop").unwrap()["state"], "online");
    for log in [laptop.log(), server.log()] {
        assert!(
            !log.contains(&format!("kept the link dialled by {higher}")),
            "{log}"
        );
        for line in log.lines().filter(|line| line.contains("ended")) {
            assert!(line.ends_with(": linked already"), "{log}");
        }
    }
}

#[test]
fn a_client_link_s_requests_are_refused_on_a_machine_with_no_member_list() {
    use flottille_link::{Endpoint, Hello, MachineKey, Message, PublicKey, Traffic};
    let server = Machine::new();
    let addresses: Vec<std::net::SocketAddr> = server
        .addresses("found")
        .iter()
        .map(|address| address.parse().unwrap())
        .collect();
    let server_key: PublicKey = serde_json::from_value(json!(server.key())).unwrap();
    let folder = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // A client of another machine, whose key the server trusts.
        let key = MachineKey::load_or_make(&folder.path().join("remote")).unwrap();
        let client = Endpoint::bind(&key, "127.0.0.1:0".parse().unwrap()).unwrap();
        let client_key = serde_json::to_value(client.key()).unwrap();
        server.trust_key("laptop", client_key.as_str().unwrap(), &[]);

        let link = tokio::time::timeout(
            WAIT,
            client.connect(
                server_key,
                &addresses,
                Traffic::Client,
                &Hello::ours("0.4.1"),
                "server",
            ),
        )
        .await
        .unwrap()
        .unwrap();
        // No hub, so no member to have signed it: nothing is served.
        let mut answers = link
            .ask(&Message::Client(
                json!({"id": "1", "method": "daemon.status"}),
            ))
            .await
            .unwrap();
        let refused = answers.next().await.unwrap().unwrap();
        assert_eq!(refused["id"], "1");
        assert_eq!(refused["error"]["code"], "refused");
        assert_eq!(
            refused["error"]["message"],
            "refused: this machine is not set up with a hub: flottille remote setup, or \
             flottille remote join"
        );
        assert!(answers.next().await.is_none());
        client.close().await;
    });
}
