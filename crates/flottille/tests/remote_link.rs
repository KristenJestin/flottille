//! Two machines linked to each other (`docs/remote.md`, "Architecture", "The transport", "The
//! version"): two daemons, each with its own temporary HOME and XDG folders, paired with a hub
//! served in the test on 127.0.0.1, which their links go through. No address, no discovery.

use crate::common;
use crate::remote::{self, Machine, TestHub};

use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// How long a link may take to come up or go down: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(30);

/// What `machine` sees of `name`, as `remote.machines` says it.
fn seen(machine: &Machine, name: &str) -> Value {
    machine.remote()["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|seen| seen["name"] == name)
        .cloned()
        .unwrap_or(Value::Null)
}

/// `pc` and `laptop`, paired with one hub, both runners.
fn paired(pc: Machine, laptop: Machine) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    remote::set_up(&hub, &pc);
    remote::pair(&hub, &laptop, "laptop", "runner");
    (hub, pc, laptop)
}

fn this(machine: &Machine) -> Value {
    machine.remote()["this"].clone()
}

#[test]
fn no_common_protocol_refuses_the_link_saying_which_to_update() {
    let (_hub, pc, laptop) = paired(
        Machine::new(),
        Machine::of(User::new().with_env(
            "FLOTTILLE_TEST_LINK_HELLO",
            r#"{"flottille": "0.9.0", "protocols": [99]}"#,
        )),
    );
    let status = pc.user.status();
    let (version, protocol) = (
        status["version"].as_str().unwrap().to_owned(),
        status["protocol"].as_u64().unwrap(),
    );

    pc.wait_for("laptop", "needs an update");
    let sentence = format!(
        "laptop runs Flottille 0.9.0 (protocol 99), this machine {version} (protocol \
         {protocol}): update one of them (flottille update)"
    );
    assert_eq!(seen(&pc, "laptop")["reason"], sentence);
    assert!(pc.log().contains(&sentence), "{}", pc.log());
    // Refused on the other side too, with its own point of view.
    laptop.wait_for("pc", "needs an update");
    let sentence = format!(
        "pc runs Flottille {version} (protocol {protocol}), this machine 0.9.0 (protocol 99): \
         update one of them (flottille update)"
    );
    assert_eq!(seen(&laptop, "pc")["reason"], sentence);
    assert!(laptop.log().contains(&sentence), "{}", laptop.log());
}

#[test]
fn remote_off_has_no_link() {
    let user = User::new();
    let (code, message) = user.call("remote.machines", json!({})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("remote is off"), "{message}");
    assert!(!user.path("state/flottille/remote").exists());
}

#[test]
fn a_half_written_peers_file_keeps_the_links() {
    let (_hub, pc, laptop) = paired(Machine::new(), Machine::new());
    pc.wait_for("laptop", "online");
    laptop.wait_for("pc", "online");

    // Caught half written (a hand edit, a writer that stopped): the last file read is kept.
    std::fs::write(
        pc.folder().join("peers.json"),
        r#"{"machines": [{"name": "lap"#,
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(seen(&pc, "laptop")["state"], "online");
    assert_eq!(seen(&laptop, "pc")["state"], "online");
    // Said once, not at every round.
    let log = pc.log();
    assert_eq!(log.matches("peers.json is not read").count(), 1, "{log}");
    assert!(!log.contains("the link to laptop ended"), "{log}");
}

#[test]
fn two_machines_dialing_at_once_keep_one_link() {
    // Each takes a link it is dialled on only after it dialled the other: two links, one each
    // way, on both sides.
    let late = || User::new().with_env("FLOTTILLE_TEST_LINK_ACCEPT_DELAY_MS", "2500");
    let (_hub, pc, laptop) = paired(Machine::of(late()), Machine::of(late()));
    pc.wait_for("laptop", "online");
    laptop.wait_for("pc", "online");

    // Both keep the same one, by a rule both compute: the link the lower key dialled. The side
    // that takes the other's link first holds two and says so; the other side may see the link
    // it dialled closed before it takes the second one, and holds one only.
    let key = |machine: &Machine| this(machine)["key"].as_str().unwrap().to_owned();
    let fingerprint = |machine: &Machine| this(machine)["fingerprint"].as_str().unwrap().to_owned();
    let (lower, higher) = if key(&pc) < key(&laptop) {
        (fingerprint(&pc), fingerprint(&laptop))
    } else {
        (fingerprint(&laptop), fingerprint(&pc))
    };
    let kept = format!("kept the link dialled by {lower}");
    let deadline = Instant::now() + WAIT;
    while !(pc.log().contains(&kept) || laptop.log().contains(&kept)) {
        assert!(
            Instant::now() < deadline,
            "{kept}:\n{}\n{}",
            pc.log(),
            laptop.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_secs(3));
    let linked = |log: &str| log.matches(" (protocol ").count();
    let before = [linked(&pc.log()), linked(&laptop.log())];
    std::thread::sleep(Duration::from_secs(3));
    // One link, kept: nobody links again.
    assert_eq!(before, [linked(&pc.log()), linked(&laptop.log())]);
    assert_eq!(seen(&pc, "laptop")["state"], "online");
    assert_eq!(seen(&laptop, "pc")["state"], "online");
    for log in [pc.log(), laptop.log()] {
        assert!(
            !log.contains(&format!("kept the link dialled by {higher}")),
            "{log}"
        );
        for line in log.lines().filter(|line| line.contains("ended")) {
            assert!(line.ends_with(": linked already"), "{log}");
        }
    }
}
