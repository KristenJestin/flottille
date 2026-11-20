//! The hub's state (`docs/remote.md`, "The hub", "Pairing"): its key, the member list it stores,
//! its pair mode, what it knows of the devices, presence and addresses, in a temporary state
//! folder, with a clock the tests move.

use std::cell::Cell;
use std::fmt::Debug;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use flottille_hub::{Clock, Hub, HubError, PairEnd, PairedDevice, Presence, reset};
use flottille_link::{
    Action, Change, ChangeHash, Colour, DeviceKey, Icon, Keyring, KeyringError, ListError,
    MachineKey, MemberList, PairCode, Proposed, Role, Signature,
};
use zeroize::Zeroizing;

/// 2027-01-15, in seconds since 1970.
const START: u64 = 1_800_000_000;

/// A clock the tests move.
#[derive(Clone)]
struct Time(Rc<Cell<u64>>);

impl Time {
    fn new() -> Time {
        Time(Rc::new(Cell::new(START)))
    }

    fn pass(&self, seconds: u64) {
        self.0.set(self.0.get() + seconds);
    }
}

impl Clock for Time {
    fn now(&self) -> u64 {
        self.0.get()
    }
}

/// A keyring that holds one device key, made from a fixed seed.
struct Seeded([u8; 32]);

impl Keyring for Seeded {
    fn read(
        &self,
        _attributes: &[(&str, &str)],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        Ok(Some(Zeroizing::new(self.0.to_vec())))
    }

    fn store(
        &self,
        _label: &str,
        _attributes: &[(&str, &str)],
        _secret: &[u8],
    ) -> Result<(), KeyringError> {
        unreachable!("the key is there")
    }
}

/// The device key number `n`.
fn key(n: u8) -> DeviceKey {
    DeviceKey::load_or_make(&Seeded([n; 32]), Path::new("/nonexistent")).unwrap()
}

/// A temporary state folder for the hub, not made yet.
fn folder() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("state/flottille-hub");
    (dir, folder)
}

fn open(folder: &Path, time: &Time) -> Hub<Time> {
    Hub::open(folder, time.clone()).unwrap()
}

fn error<T: Debug>(result: Result<T, HubError>) -> String {
    result.unwrap_err().to_string()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn add(key: &DeviceKey, name: &str, role: Role) -> Action {
    Action::Add {
        key: key.public(),
        name: name.to_owned(),
        role,
    }
}

/// The card `key` proposes when it pairs.
fn proposed(key: &DeviceKey, name: &str, role: Role) -> Proposed {
    Proposed {
        key: key.public(),
        name: name.to_owned(),
        role,
        icon: Icon::Laptop,
        colour: Colour::of(&key.public()),
    }
}

/// Pair mode opened, as `flottille-hub pair` opens it: its number and its code.
fn open_pairing(hub: &mut Hub<Time>) -> (u64, PairCode) {
    hub.open_pairing("https://hub.example.net", None).unwrap()
}

/// `key` pairs as `name` with `role`, with the code it was given: its proof checks against the
/// code's secret, as the session's does.
fn pair_with(
    hub: &mut Hub<Time>,
    code: &PairCode,
    key: &DeviceKey,
    name: &str,
    role: Role,
) -> Result<PairedDevice, HubError> {
    let secret = code.secret;
    hub.pair(&proposed(key, name, role), |held| *held == secret, "0.4.0")
}

/// Pair mode opened, and `key` paired with its code.
fn pair(hub: &mut Hub<Time>, key: &DeviceKey, name: &str, role: Role) {
    let (_, code) = open_pairing(hub);
    pair_with(hub, &code, key, name, role).unwrap();
}

/// `signer`'s change after the hub's last one.
fn next(hub: &Hub<Time>, action: Action, signer: &DeviceKey) -> Change {
    Change::sign(hub.list().head(), action, signer)
}

/// The members the hub's list leaves, checked back to `root` and the hub's key.
fn members(hub: &Hub<Time>, root: &flottille_link::PublicKey) -> Vec<(String, Role)> {
    hub.list()
        .check(root, &hub.key())
        .unwrap()
        .members()
        .iter()
        .map(|member| (member.name.clone(), member.role))
        .collect()
}

/// A state folder holding `list`, as a hub of before pair mode wrote it.
fn written(folder: &Path, list: &MemberList) {
    let state = serde_json::json!({"version": 1, "members": list});
    std::fs::write(folder.join("hub.json"), state.to_string()).unwrap();
    std::fs::set_permissions(folder.join("hub.json"), PermissionsExt::from_mode(0o600)).unwrap();
}

#[test]
fn pair_mode_pairs_one_device_with_its_code_once() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    assert!(hub.members().is_none());
    let (pc, laptop) = (key(1), key(2));

    let (number, code) = open_pairing(&mut hub);
    assert_eq!(code.hub_key, hub.key());
    assert_eq!(code.hub_url, "https://hub.example.net");
    assert_eq!(hub.pairing_ended(number), None);
    let paired = pair_with(&mut hub, &code, &pc, "pc", Role::Runner).unwrap();
    assert_eq!(paired.name, "pc");
    assert!(!paired.again);
    assert_eq!(hub.pairing_ended(number), Some(PairEnd::Paired(paired)));

    // The list is the hub's: its root names the hub's key, and the entry is signed with it.
    assert_eq!(hub.list().root(), Some(hub.key()));
    assert_eq!(members(&hub, &hub.key()), [("pc".to_owned(), Role::Runner)]);
    let entry = hub.list().changes()[1].clone();
    assert_eq!(entry.signer, hub.key());

    // The code served once: another device holding it pairs nothing.
    let used = error(pair_with(&mut hub, &code, &laptop, "laptop", Role::Runner));
    assert!(
        used.starts_with("pair mode is not open on this hub"),
        "{used}"
    );
    assert_eq!(members(&hub, &hub.key()).len(), 1);

    // What the hub knows of the device: when it was paired and seen, and its version.
    let facts = &hub.facts()[0];
    assert_eq!(facts.key, pc.public());
    assert_eq!(facts.paired_at, Some(START));
    assert_eq!(facts.version.as_deref(), Some("0.4.0"));

    // Kept: the hub started again holds the list and the facts.
    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(members(&hub, &hub.key()), [("pc".to_owned(), Role::Runner)]);
    assert_eq!(hub.facts().len(), 1);
}

#[test]
fn pair_mode_ends_when_its_minutes_pass_or_its_command_stops() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let pc = key(1);

    let (number, code) = hub
        .open_pairing("https://hub.example.net", Some(2))
        .unwrap();
    // One pair mode at a time.
    let again = error(hub.open_pairing("https://hub.example.net", None));
    assert!(
        again.starts_with("pair mode is open already, 120 s more"),
        "{again}"
    );
    time.pass(2 * 60);
    assert_eq!(
        hub.pairing_ended(number),
        Some(PairEnd::Over(
            "pair mode's minutes passed with no device paired".to_owned()
        ))
    );
    assert!(
        error(pair_with(&mut hub, &code, &pc, "pc", Role::Runner))
            .starts_with("pair mode is not open")
    );

    let (number, code) = open_pairing(&mut hub);
    hub.close_pairing(number);
    assert_eq!(
        hub.pairing_ended(number),
        Some(PairEnd::Over("pair mode was closed".to_owned()))
    );
    assert!(pair_with(&mut hub, &code, &pc, "pc", Role::Runner).is_err());
    assert!(hub.members().is_none());

    // Minutes out of bounds, and a URL that is none, are refused before anything opens.
    assert!(
        error(hub.open_pairing("https://hub.example.net", Some(16))).contains("1 to 15 minutes")
    );
    assert!(hub.open_pairing("hub.example.net", None).is_err());
    assert_eq!(hub.pairing_left(), None);
}

#[test]
fn a_wrong_proof_pairs_nothing_and_five_close_pair_mode() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, stranger) = (key(1), key(9));
    let (number, code) = open_pairing(&mut hub);
    for _ in 0..4 {
        let wrong = error(hub.pair(&proposed(&stranger, "x", Role::Runner), |_| false, "0.4.0"));
        assert!(
            wrong.starts_with("this device did not prove it holds the pair code"),
            "{wrong}"
        );
    }
    // Four wrong proofs: the right one still pairs.
    let paired = pair_with(&mut hub, &code, &pc, "pc", Role::Runner).unwrap();
    assert_eq!(paired.name, "pc");

    let (number_2, _) = open_pairing(&mut hub);
    assert_ne!(number, number_2);
    for _ in 0..5 {
        let _ = hub.pair(&proposed(&stranger, "x", Role::Runner), |_| false, "0.4.0");
    }
    let Some(PairEnd::Over(why)) = hub.pairing_ended(number_2) else {
        panic!("pair mode is still open");
    };
    assert!(why.contains("after 5 wrong proofs"), "{why}");
    assert_eq!(members(&hub, &hub.key()).len(), 1);
}

#[test]
fn a_device_paired_again_keeps_its_card() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let pc = key(1);
    pair(&mut hub, &pc, "pc", Role::Runner);
    let changes = hub.list().changes().len();

    // Paired again with a new code, proposing another name and role: nothing changes.
    time.pass(60);
    let (_, code) = open_pairing(&mut hub);
    let again = pair_with(&mut hub, &code, &pc, "renamed", Role::Viewer).unwrap();
    assert!(again.again);
    assert_eq!((again.name.as_str(), again.role), ("pc", Role::Runner));
    assert_eq!(hub.list().changes().len(), changes);
    assert_eq!(hub.facts()[0].paired_at, Some(START + 60));

    // Removed, then paired again with the same key: it is a member again.
    hub.remove("pc").unwrap();
    assert!(
        hub.list()
            .check(&hub.key(), &hub.key())
            .unwrap()
            .was_removed(&pc.public())
    );
    pair(&mut hub, &pc, "pc", Role::Controller);
    assert_eq!(
        members(&hub, &hub.key()),
        [("pc".to_owned(), Role::Controller)]
    );
}

#[test]
fn only_the_hub_changes_a_role_and_no_member_adds_a_device() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, pixel) = (key(1), key(2));
    pair(&mut hub, &pc, "pc", Role::Runner);
    pair(&mut hub, &pixel, "pixel", Role::Controller);

    // A member's addition is refused, whatever it adds.
    let added = error(hub.submit(next(&hub, add(&key(3), "tablet", Role::Viewer), &pc)));
    assert_eq!(
        added,
        "the hub refuses this change: a device is paired on the hub (flottille-hub pair), no \
         member adds one"
    );
    // A role change a member signed is refused by the list's check.
    let raised = next(
        &hub,
        Action::Role {
            key: pixel.public(),
            role: Role::Runner,
        },
        &pc,
    );
    assert!(error(hub.submit(raised)).contains("only the hub may change the role of a device"));

    // The hub's host changes it, by name or fingerprint.
    hub.change_role("pixel", Role::Viewer).unwrap();
    hub.change_role(&pc.public().fingerprint(), Role::Controller)
        .unwrap();
    assert_eq!(
        members(&hub, &hub.key()),
        [
            ("pc".to_owned(), Role::Controller),
            ("pixel".to_owned(), Role::Viewer)
        ]
    );
    assert_eq!(
        error(hub.change_role("pixel", Role::Viewer)),
        "pixel is a viewer already"
    );
    assert!(error(hub.change_role("nobody", Role::Viewer)).starts_with("nobody is no member"));

    // A role changed by hand in the state folder: the hub refuses to start on it.
    drop(hub);
    let file = folder.join("hub.json");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replacen("\"controller\"", "\"runner\"", 1)).unwrap();
    let refused = error(Hub::open(&folder, time.clone()));
    assert!(
        refused.contains("does not carry its signer's signature")
            && refused.ends_with("run flottille-hub reset on this machine to start again"),
        "{refused}"
    );
}

#[test]
fn a_device_is_removed_from_the_hub_or_by_a_member() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, pixel, tablet) = (key(1), key(2), key(3));
    pair(&mut hub, &pc, "pc", Role::Runner);
    pair(&mut hub, &pixel, "pixel", Role::Controller);
    pair(&mut hub, &tablet, "tablet", Role::Viewer);

    hub.remove("tablet").unwrap();
    hub.submit(next(
        &hub,
        Action::Remove {
            key: pixel.public(),
        },
        &pc,
    ))
    .unwrap();
    assert_eq!(members(&hub, &hub.key()), [("pc".to_owned(), Role::Runner)]);
    // What the hub knew of them goes with them.
    assert_eq!(hub.facts().len(), 1);
}

#[test]
fn a_hub_set_up_before_pair_mode_keeps_its_members_and_pairs_after_them() {
    let (_dir, folder) = folder();
    let time = Time::new();
    drop(open(&folder, &time));
    // The state of a hub set up with a setup code: its first member and its machine.
    let pc = key(1);
    let machine_dir = tempfile::tempdir().unwrap();
    let machine = MachineKey::load_or_make(&machine_dir.path().join("pc")).unwrap();
    let mut list = MemberList::start("pc", &pc).unwrap();
    list.declare(&pc, &machine);
    let state = serde_json::json!({"version": 1, "setup_code": null, "members": list});
    std::fs::write(folder.join("hub.json"), state.to_string()).unwrap();
    std::fs::set_permissions(folder.join("hub.json"), PermissionsExt::from_mode(0o600)).unwrap();

    let mut hub = open(&folder, &time);
    assert_eq!(
        members(&hub, &pc.public()),
        [("pc".to_owned(), Role::Runner)]
    );
    assert!(
        hub.welcome(&machine.public(), Some(list.changes()[0].hash()))
            .is_ok()
    );
    assert!(hub.welcome(&pc.public(), None).is_ok());

    // The next device pairs after them, its entry the hub's; the old member's own changes go on.
    pair(&mut hub, &key(2), "phone", Role::Controller);
    hub.submit(next(
        &hub,
        Action::Rename {
            key: pc.public(),
            name: "desktop".to_owned(),
        },
        &pc,
    ))
    .unwrap();
    assert_eq!(
        members(&hub, &pc.public()),
        [
            ("desktop".to_owned(), Role::Runner),
            ("phone".to_owned(), Role::Controller)
        ]
    );
    // A device that pinned the old list takes the new one.
    assert!(
        hub.list()
            .check_update(&pc.public(), &hub.key(), &list)
            .is_ok()
    );
}

#[test]
fn a_reset_empties_the_list_and_the_next_pairing_makes_a_new_one() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, pixel) = (key(1), key(2));
    pair(&mut hub, &pc, "pc", Role::Runner);
    let made = hub.key();
    let old = hub.list().clone();

    // A reset is `flottille-hub reset`, a process of its own: the hub is stopped.
    drop(hub);
    reset(&folder).unwrap();
    let mut hub = open(&folder, &time);
    assert_eq!(hub.key(), made);
    assert!(hub.list().changes().is_empty());
    assert!(hub.facts().is_empty());
    assert!(hub.welcome(&pc.public(), None).is_err());

    pair(&mut hub, &pixel, "pixel", Role::Runner);
    // A device that pinned the old list refuses the new one, saying the hub was reset: its first
    // change names another list's id.
    assert!(matches!(
        hub.list().check_update(&made, &made, &old),
        Err(ListError::Reset)
    ));
}

#[test]
fn a_list_change_neither_the_hub_nor_a_member_signed_is_refused() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, stranger) = (key(1), key(9));

    // Before the first device, no change is taken.
    let early = Change::sign(
        ChangeHash::default(),
        Action::Remove { key: pc.public() },
        &pc,
    );
    assert_eq!(
        error(hub.submit(early)),
        "the hub has no member yet: pair a first device with flottille-hub pair"
    );
    pair(&mut hub, &pc, "pc", Role::Runner);
    let rename = Action::Rename {
        key: pc.public(),
        name: "laptop".to_owned(),
    };

    let by_a_stranger = next(&hub, rename.clone(), &stranger);
    let mut broken = next(&hub, rename.clone(), &pc);
    broken.signature = Signature([7; 64]);
    let mut claimed = next(&hub, rename.clone(), &stranger);
    claimed.signer = pc.public();
    let off_the_chain = Change::sign(ChangeHash([3; 32]), rename.clone(), &pc);
    for (change, why) in [
        (
            by_a_stranger,
            "is signed by a key that is neither a member nor the hub",
        ),
        (broken, "does not carry its signer's signature"),
        (claimed, "does not carry its signer's signature"),
        (off_the_chain, "does not follow the change before it"),
    ] {
        assert_eq!(
            error(hub.submit(change)),
            format!("the hub refuses this change: change 3 {why}")
        );
    }
    assert_eq!(hub.list().changes().len(), 2);

    // The member's own change is stored, and kept.
    hub.submit(next(&hub, rename, &pc)).unwrap();
    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(
        members(&hub, &hub.key()),
        [("laptop".to_owned(), Role::Runner)]
    );
}

#[test]
fn a_reset_works_on_a_state_that_does_not_check() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let made = hub.key();
    pair(&mut hub, &key(1), "pc", Role::Runner);
    drop(hub);

    let file = folder.join("hub.json");
    // A role changed by hand, which does not check, and a file that is not the hub's state.
    let tampered = std::fs::read_to_string(&file)
        .unwrap()
        .replace("\"runner\"", "\"viewer\"");
    for broken in [tampered.as_str(), "not the hub's state"] {
        std::fs::write(&file, broken).unwrap();
        assert!(Hub::open(&folder, time.clone()).is_err());

        reset(&folder).unwrap();
        let hub = open(&folder, &time);
        assert!(hub.list().changes().is_empty());
        assert_eq!(hub.key(), made);
    }
    assert_eq!(mode(&file), 0o600);
}

#[test]
fn a_second_hub_on_the_same_folder_is_refused_until_the_first_is_dropped() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);

    // Another process, or `reset`, while the hub runs: refused, so that a reset is never undone
    // by the running hub's next write.
    let another = format!(
        "another flottille-hub uses {}: stop it first",
        folder.display()
    );
    assert_eq!(error(Hub::open(&folder, time.clone())), another);
    assert_eq!(error(reset(&folder)), another);
    pair(&mut hub, &key(1), "pc", Role::Runner);

    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(hub.list().changes().len(), 2);
    drop(hub);
    reset(&folder).unwrap();
}

#[test]
fn the_hub_key_is_made_once() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let hub = open(&folder, &time);
    let made = hub.key();
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(mode(&folder.join("hub.key")), 0o600);
    drop(hub);

    assert_eq!(open(&folder, &time).key(), made);
    let (_other_dir, other) = self::folder();
    assert_ne!(open(&other, &time).key(), made);
}

#[test]
fn presence_and_addresses_are_kept_in_memory_for_members_only() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, stranger) = (key(1), key(9));
    pair(&mut hub, &pc, "pc", Role::Runner);

    let presence = Presence {
        active: true,
        last_input: START,
    };
    hub.tell_presence(&pc.public(), presence).unwrap();
    hub.tell_addresses(&pc.public(), vec!["192.0.2.7:4433".to_owned()])
        .unwrap();
    assert_eq!(hub.presence(&pc.public()), Some(presence));
    assert_eq!(
        hub.addresses(&pc.public()),
        Some(&["192.0.2.7:4433".to_owned()][..])
    );

    let not_member = format!("{} is not a member", stranger.public().fingerprint());
    assert_eq!(
        error(hub.tell_presence(&stranger.public(), presence)),
        not_member
    );
    assert_eq!(
        error(hub.tell_addresses(&stranger.public(), Vec::new())),
        not_member
    );
    assert_eq!(hub.presence(&stranger.public()), None);

    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(hub.presence(&pc.public()), None);
    assert_eq!(hub.addresses(&pc.public()), None);
}

#[test]
fn a_member_makes_at_most_30_changes_an_hour() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let pc = key(1);
    pair(&mut hub, &pc, "pc", Role::Runner);
    let rename = |hub: &Hub<Time>, n: usize| {
        next(
            hub,
            Action::Rename {
                key: pc.public(),
                name: format!("pc {n}"),
            },
            &pc,
        )
    };
    for n in 0..30 {
        let change = rename(&hub, n);
        hub.submit(change).unwrap();
    }
    let change = rename(&hub, 30);
    let refused = error(hub.submit(change.clone()));
    assert!(
        refused.contains("made 30 changes of the list within an hour"),
        "{refused}"
    );
    time.pass(60 * 60);
    hub.submit(change).unwrap();
}

#[test]
fn the_hub_refuses_a_change_past_the_most_a_device_takes() {
    let (_dir, folder) = folder();
    let time = Time::new();
    drop(open(&folder, &time));
    // A list of 1000 changes, as a member could make it over months.
    let pc = key(1);
    let mut list = MemberList::start("pc", &pc).unwrap();
    for n in 1..1000 {
        list.sign(
            Action::Rename {
                key: pc.public(),
                name: format!("pc {n}"),
            },
            &pc,
        );
    }
    written(&folder, &list);
    let mut hub = open(&folder, &time);
    let change = next(
        &hub,
        Action::Rename {
            key: pc.public(),
            name: "one more".to_owned(),
        },
        &pc,
    );
    let refused = error(hub.submit(change));
    assert!(refused.contains("more than 1000 changes"), "{refused}");
}

#[test]
fn a_full_list_still_removes_a_member() {
    let (_dir, folder) = folder();
    let time = Time::new();
    drop(open(&folder, &time));
    // A list filled to 1000 changes, as a viewer re-declaring its own machine 30 times an hour
    // could fill it in a day and a half.
    let (pc, phone, tablet) = (key(1), key(2), key(3));
    let mut list = MemberList::start("pc", &pc).unwrap();
    list.sign(add(&phone, "phone", Role::Controller), &pc);
    list.sign(add(&tablet, "tablet", Role::Viewer), &pc);
    while list.changes().len() < 1000 {
        let name = format!("pc {}", list.changes().len());
        list.sign(
            Action::Rename {
                key: pc.public(),
                name,
            },
            &pc,
        );
    }
    written(&folder, &list);
    let mut hub = open(&folder, &time);
    // The viewer's own change stays refused...
    let own = next(
        &hub,
        Action::Remove {
            key: tablet.public(),
        },
        &tablet,
    );
    let refused = error(hub.submit(own));
    assert!(refused.contains("more than 1000 changes"), "{refused}");
    // ...while a controller still removes it, and the root still removes the controller.
    let removal = next(
        &hub,
        Action::Remove {
            key: tablet.public(),
        },
        &phone,
    );
    hub.submit(removal).unwrap();
    let removal = next(
        &hub,
        Action::Remove {
            key: phone.public(),
        },
        &pc,
    );
    hub.submit(removal).unwrap();
    assert_eq!(members(&hub, &pc.public()).len(), 1);
}

#[test]
fn what_a_device_says_of_itself_is_kept_with_its_facts() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, stranger) = (key(1), key(9));
    pair(&mut hub, &pc, "pc", Role::Runner);
    time.pass(30);
    hub.about(&pc.public(), "computer", "linux").unwrap();
    hub.seen(&pc.public(), "0.4.1").unwrap();
    let facts = hub.facts()[0].clone();
    assert_eq!(facts.kind.as_deref(), Some("computer"));
    assert_eq!(facts.system.as_deref(), Some("linux"));
    assert_eq!(facts.version.as_deref(), Some("0.4.1"));
    assert_eq!(facts.last_seen, Some(START + 30));
    assert!(hub.about(&pc.public(), "a b", "linux").is_err());
    assert!(hub.about(&stranger.public(), "computer", "linux").is_err());
}
