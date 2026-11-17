//! The member list (`docs/remote.md`, "The hub", "The first member", "Pairing", "Roles"): an
//! ordered log of signed changes that every device checks back to the root it pinned.

use std::path::Path;

use flottille_link::{
    Action, Change, ChangeHash, DeviceKey, Keyring, KeyringError, ListError, MANAGING_HEADROOM,
    MOST_CHANGES, MachineKey, MemberList, Role,
};
use zeroize::Zeroizing;

/// A keyring that holds one device key, made from a fixed seed: the same key at every run.
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

/// The device key number `n`; the folder is never reached, the keyring holding the key.
fn key(n: u8) -> DeviceKey {
    DeviceKey::load_or_make(&Seeded([n; 32]), Path::new("/nonexistent")).unwrap()
}

fn add(key: &DeviceKey, name: &str, role: Role) -> Action {
    Action::Add {
        key: key.public(),
        name: name.to_owned(),
        role,
    }
}

fn remove(key: &DeviceKey) -> Action {
    Action::Remove { key: key.public() }
}

/// The root `pc`, a controller `pixel` it added, and a viewer `tablet` the controller added.
fn three() -> (DeviceKey, DeviceKey, DeviceKey, MemberList) {
    let (pc, pixel, tablet) = (key(1), key(2), key(3));
    let mut list = MemberList::start("pc", &pc).unwrap();
    list.sign(add(&pixel, "pixel", Role::Controller), &pc);
    list.sign(add(&tablet, "tablet", Role::Viewer), &pixel);
    (pc, pixel, tablet, list)
}

/// The refusal's sentence, for a list that must be refused.
fn refused(list: &MemberList, root: &DeviceKey) -> String {
    match list.check(&root.public()) {
        Ok(members) => panic!("accepted: {:?}", members.members()),
        Err(error) => error.to_string(),
    }
}

/// Points of small order, which decompress but are no device's key: the identity, and the point of
/// order two.
const IDENTITY: &str = "0100000000000000000000000000000000000000000000000000000000000000";
const ORDER_TWO: &str = "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f";

#[test]
fn a_weak_key_is_refused_in_an_entry() {
    let (pc, _, _, list) = three();
    for weak in [IDENTITY, ORDER_TWO] {
        let mut json = serde_json::to_value(&list).unwrap();
        json[1]["action"]["key"] = serde_json::Value::String(weak.to_owned());
        let error = serde_json::from_value::<MemberList>(json).unwrap_err();
        assert!(error.to_string().contains("weak key"), "{error}");
    }
    assert!(list.check(&pc.public()).is_ok());
}

#[test]
fn a_list_checks_back_to_the_pinned_root() {
    let (pc, pixel, tablet, mut list) = three();
    list.sign(
        Action::Rename {
            key: tablet.public(),
            name: "kitchen".to_owned(),
        },
        &pc,
    );

    let members = list.check(&pc.public()).unwrap();

    assert_eq!(members.root(), pc.public());
    assert_eq!(members.head(), list.head());
    let rows: Vec<_> = members
        .members()
        .iter()
        .map(|member| {
            (
                member.key,
                member.name.as_str(),
                member.role,
                member.added_by,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (pc.public(), "pc", Role::Runner, pc.public()),
            (pixel.public(), "pixel", Role::Controller, pc.public()),
            (tablet.public(), "kitchen", Role::Viewer, pixel.public()),
        ]
    );
    assert_eq!(members.get(&pixel.public()).unwrap().name, "pixel");
    assert!(members.get(&key(9).public()).is_none());
}

#[test]
fn a_change_no_member_signed_is_refused() {
    let (pc, _, _, list) = three();
    let stranger = key(9);

    let mut signed_by_a_stranger = list.clone();
    signed_by_a_stranger.sign(add(&key(4), "intruder", Role::Runner), &stranger);
    let why = refused(&signed_by_a_stranger, &pc);
    assert!(
        why.contains("change 4 is signed by a key that is not a member"),
        "{why}"
    );

    // A stranger's signature put under a member's name.
    let mut changes = list.changes().to_vec();
    let mut forged = Change::sign(
        list.head(),
        add(&key(4), "intruder", Role::Runner),
        &stranger,
    );
    forged.signer = pc.public();
    changes.push(forged);
    let why = refused(&MemberList::from_changes(changes), &pc);
    assert!(
        why.contains("change 4 does not carry its signer's signature"),
        "{why}"
    );
}

#[test]
fn a_viewer_cannot_add_or_remove_a_member() {
    let (pc, pixel, tablet, list) = three();

    let mut adds = list.clone();
    adds.sign(add(&key(4), "laptop", Role::Viewer), &tablet);
    let why = refused(&adds, &pc);
    assert!(
        why.contains("change 4 is signed by tablet, a viewer: a controller or a runner may add"),
        "{why}"
    );

    let mut removes = list.clone();
    removes.sign(remove(&pixel), &tablet);
    let why = refused(&removes, &pc);
    assert!(
        why.contains("change 4 is signed by tablet, a viewer: a controller or a runner may remove"),
        "{why}"
    );

    let mut renames = list.clone();
    renames.sign(
        Action::Rename {
            key: tablet.public(),
            name: "mine".to_owned(),
        },
        &tablet,
    );
    let why = refused(&renames, &pc);
    assert!(
        why.contains("change 4 is signed by tablet, a viewer: a controller or a runner may rename"),
        "{why}"
    );

    // A device that wants to leave removes itself, whatever its role.
    let mut leaves = list.clone();
    leaves.sign(remove(&tablet), &tablet);
    let members = leaves.check(&pc.public()).unwrap();
    assert!(members.get(&tablet.public()).is_none());
}

#[test]
fn a_removed_key_cannot_be_added_again() {
    let (pc, pixel, _, mut list) = three();
    list.sign(remove(&pixel), &pc);
    assert!(list.check(&pc.public()).is_ok());

    list.sign(add(&pixel, "pixel", Role::Controller), &pc);
    let why = refused(&list, &pc);
    assert!(
        why.contains("change 5 adds a key that was removed"),
        "{why}"
    );
}

#[test]
fn a_removed_member_cannot_sign() {
    let (pc, pixel, _, mut list) = three();
    list.sign(remove(&pixel), &pc);
    list.sign(add(&key(4), "laptop", Role::Runner), &pixel);

    let why = refused(&list, &pc);
    assert!(
        why.contains("change 5 is signed by a key that is not a member"),
        "{why}"
    );
}

#[test]
fn a_list_with_another_root_is_refused_saying_it_was_reset() {
    let (pc, _, _, _) = three();
    let other = key(7);
    let mut reset = MemberList::start("pc", &other).unwrap();
    reset.sign(add(&pc, "pc", Role::Runner), &other);

    let error = reset.check(&pc.public()).unwrap_err();
    assert!(matches!(error, ListError::Reset));
    assert_eq!(
        error.to_string(),
        "the hub's member list was reset: set this device up again"
    );

    // An emptied list, before a first member joins again, is a reset too.
    let error = MemberList::from_changes(Vec::new())
        .check(&pc.public())
        .unwrap_err();
    assert!(matches!(error, ListError::Reset));
}

#[test]
fn the_root_entry_is_a_runner_that_signed_itself() {
    let (pc, pixel) = (key(1), key(2));

    let viewer = MemberList::from_changes(vec![Change::sign(
        ChangeHash::default(),
        add(&pc, "pc", Role::Viewer),
        &pc,
    )]);
    let why = refused(&viewer, &pc);
    assert!(
        why.contains("change 1 is not the root's own entry as a runner"),
        "{why}"
    );

    let signed_by_another = MemberList::from_changes(vec![Change::sign(
        ChangeHash::default(),
        add(&pc, "pc", Role::Runner),
        &pixel,
    )]);
    let why = refused(&signed_by_another, &pc);
    assert!(
        why.contains("change 1 is not the root's own entry as a runner"),
        "{why}"
    );
}

#[test]
fn the_role_is_in_the_signed_entry_and_cannot_change_without_a_new_entry() {
    let (pc, _, tablet, list) = three();

    // The hub turns the viewer into a runner: the signature no longer matches.
    let mut changes = list.changes().to_vec();
    let Action::Add { role, .. } = &mut changes[2].action else {
        panic!("the third change adds the tablet");
    };
    *role = Role::Runner;
    let why = refused(&MemberList::from_changes(changes), &pc);
    assert!(
        why.contains("change 3 does not carry its signer's signature"),
        "{why}"
    );

    // A rename keeps the role.
    let mut renamed = list.clone();
    renamed.sign(
        Action::Rename {
            key: tablet.public(),
            name: "kitchen".to_owned(),
        },
        &pc,
    );
    let members = renamed.check(&pc.public()).unwrap();
    assert_eq!(members.get(&tablet.public()).unwrap().role, Role::Viewer);

    // Adding the key again with another role, while it is a member, is refused: a role changes by
    // removing the device, which then joins again with a new key.
    let mut again = list.clone();
    again.sign(add(&tablet, "tablet", Role::Controller), &pc);
    let why = refused(&again, &pc);
    assert!(
        why.contains("change 4 adds a key that is a member"),
        "{why}"
    );
}

#[test]
fn a_dropped_change_is_refused() {
    let (pc, _, _, mut list) = three();
    list.sign(add(&key(4), "laptop", Role::Runner), &pc);
    let mut changes = list.changes().to_vec();
    changes.remove(2);

    let why = refused(&MemberList::from_changes(changes), &pc);
    assert!(
        why.contains("change 3 does not follow the change before it"),
        "{why}"
    );
}

#[test]
fn a_reordered_change_is_refused() {
    let (pc, _, _, mut list) = three();
    list.sign(add(&key(4), "laptop", Role::Runner), &pc);
    let mut changes = list.changes().to_vec();
    changes.swap(2, 3);

    let why = refused(&MemberList::from_changes(changes), &pc);
    assert!(
        why.contains("change 3 does not follow the change before it"),
        "{why}"
    );
}

#[test]
fn a_replayed_change_is_refused() {
    let (pc, pixel, _, mut list) = three();
    list.sign(remove(&pixel), &pc);
    // The hub plays the tablet's addition again after the removal, signed as it was.
    let mut changes = list.changes().to_vec();
    changes.push(changes[2].clone());

    let why = refused(&MemberList::from_changes(changes), &pc);
    assert!(
        why.contains("change 5 does not follow the change before it"),
        "{why}"
    );
}

#[test]
fn a_list_that_lost_changes_this_device_has_is_refused() {
    let (pc, _, _, list) = three();
    let mut known = list.clone();
    known.sign(add(&key(4), "laptop", Role::Runner), &pc);

    // The hub sends back the list as it was before the laptop: whole, but behind.
    let error = list.check_update(&pc.public(), &known).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("it lacks changes this device has already checked"),
        "{error}"
    );

    let mut newer = known.clone();
    newer.sign(add(&key(5), "server", Role::Runner), &pc);
    let members = newer.check_update(&pc.public(), &known).unwrap();
    assert_eq!(members.members().len(), 5);
}

#[test]
fn a_list_that_forked_from_the_one_this_device_has_is_refused() {
    let (pc, _, _, list) = three();
    let mut known = list.clone();
    known.sign(add(&key(4), "laptop", Role::Runner), &pc);

    // As long as the known one, each change signed, but another fourth change.
    let mut fork = list.clone();
    fork.sign(add(&key(5), "server", Role::Runner), &pc);
    assert!(fork.check(&pc.public()).is_ok());
    let error = fork.check_update(&pc.public(), &known).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("it lacks changes this device has already checked"),
        "{error}"
    );
}

#[test]
fn a_name_with_an_invisible_character_is_refused() {
    let (pc, _, _, list) = three();
    for name in [
        "p\u{200B}xel",
        "pixel\u{200D}",
        "\u{202E}lexip",
        "pi\u{2066}xel",
        "\u{FEFF}pc",
    ] {
        let mut invisible = list.clone();
        invisible.sign(add(&key(4), name, Role::Viewer), &pc);
        let why = refused(&invisible, &pc);
        assert!(
            why.contains("change 4 names a device") && why.contains("no invisible character"),
            "{name:?}: {why}"
        );
    }
}

#[test]
fn a_name_is_unique_among_the_members() {
    let (pc, pixel, _, list) = three();

    let mut taken = list.clone();
    taken.sign(add(&key(4), "Pixel", Role::Viewer), &pc);
    let why = refused(&taken, &pc);
    assert!(
        why.contains("change 4 names a device Pixel, a name a member has"),
        "{why}"
    );

    let mut renamed = list.clone();
    renamed.sign(
        Action::Rename {
            key: pixel.public(),
            name: "tablet".to_owned(),
        },
        &pc,
    );
    let why = refused(&renamed, &pc);
    assert!(
        why.contains("change 4 names a device tablet, a name a member has"),
        "{why}"
    );

    let mut empty = list.clone();
    empty.sign(add(&key(4), " ", Role::Viewer), &pc);
    let why = refused(&empty, &pc);
    assert!(why.contains("change 4 names a device"), "{why}");

    // A removed member's name is free again.
    let mut freed = list.clone();
    freed.sign(remove(&pixel), &pc);
    freed.sign(add(&key(4), "pixel", Role::Controller), &pc);
    assert!(freed.check(&pc.public()).is_ok());
}

#[test]
fn the_signed_bytes_are_the_documented_encoding() {
    let (pc, pixel) = (key(1), key(2));
    let list = MemberList::start("pc", &pc).unwrap();
    let change = Change::sign(list.head(), add(&pixel, "pixel", Role::Controller), &pc);

    let mut expected = b"flottille-member-change-v1\0".to_vec();
    expected.extend_from_slice(&list.head().0);
    expected.extend_from_slice(pc.public().as_bytes());
    expected.push(1);
    expected.extend_from_slice(pixel.public().as_bytes());
    expected.extend_from_slice(&5u32.to_be_bytes());
    expected.extend_from_slice(b"pixel");
    expected.push(2);
    assert_eq!(change.signed_bytes(), expected);

    let removal = Change::sign(list.head(), remove(&pixel), &pc);
    let mut expected = b"flottille-member-change-v1\0".to_vec();
    expected.extend_from_slice(&list.head().0);
    expected.extend_from_slice(pc.public().as_bytes());
    expected.push(2);
    expected.extend_from_slice(pixel.public().as_bytes());
    assert_eq!(removal.signed_bytes(), expected);

    // The first change names the list's id, random, where the others name the change before.
    let genesis = &list.changes()[0];
    assert_ne!(genesis.previous, ChangeHash::default());
    assert_eq!(&genesis.signed_bytes()[27..59], &genesis.previous.0);
}

#[test]
fn a_list_travels_as_json_and_still_checks() {
    let (pc, _, _, list) = three();
    let json = serde_json::to_value(&list).unwrap();

    assert_eq!(json[0]["action"]["kind"], "add");
    assert_eq!(json[0]["action"]["role"], "runner");
    assert_eq!(json[0]["action"]["name"], "pc");
    assert_eq!(json[1]["signer"].as_str().unwrap().len(), 64);
    assert_eq!(json[1]["signature"].as_str().unwrap().len(), 128);

    let back: MemberList = serde_json::from_value(json).unwrap();
    assert_eq!(back, list);
    assert!(back.check(&pc.public()).is_ok());

    // A key that is not one is refused when read.
    for not_a_key in [
        "zz".repeat(32),
        "ab".repeat(31),
        IDENTITY.to_owned(),
        ORDER_TWO.to_owned(),
    ] {
        let mut broken = serde_json::to_value(&list).unwrap();
        broken[1]["signer"] = serde_json::Value::String(not_a_key);
        assert!(serde_json::from_value::<MemberList>(broken).is_err());
    }
}

/// A machine's key, made in `dir`.
fn machine(dir: &tempfile::TempDir, name: &str) -> MachineKey {
    MachineKey::load_or_make(&dir.path().join(name)).unwrap()
}

/// `member`'s declaration of `machine`, after the change `previous`, as the member and the
/// machine sign it.
fn declared(previous: ChangeHash, member: &DeviceKey, machine: &MachineKey) -> Change {
    let action = Action::Machine {
        key: member.public(),
        machine: machine.public(),
    };
    Change::sign(previous, action, member).claimed_by(machine)
}

#[test]
fn a_member_declares_its_own_machine_and_no_other() {
    let dir = tempfile::tempdir().unwrap();
    let (pc, pixel, tablet, mut list) = three();
    let (desk, other) = (machine(&dir, "desk"), machine(&dir, "other"));
    // The daemon's key of `pc`, declared by pc itself and claimed by the machine.
    list.declare(&pc, &desk);
    let members = list.check(&pc.public()).unwrap();
    assert_eq!(
        members.get(&pc.public()).unwrap().machine,
        Some(desk.public())
    );
    assert_eq!(members.by_machine(&desk.public()).unwrap().name, "pc");
    assert!(members.by_machine(&other.public()).is_none());

    // Not for another member, whatever its signer's role.
    let action = Action::Machine {
        key: tablet.public(),
        machine: other.public(),
    };
    let change = Change::sign(list.head(), action, &pixel).claimed_by(&other);
    let forged = MemberList::from_changes([list.changes(), &[change]].concat());
    assert!(
        refused(&forged, &pc).contains("a member declares its own machine only"),
        "{}",
        refused(&forged, &pc)
    );
    // Not a machine another member declared, nor a member's own key.
    let mut taken = list.clone();
    taken.declare(&pixel, &desk);
    assert!(refused(&taken, &pc).contains("a machine another member declared"));
    let mut member = list.clone();
    member.sign(
        Action::Machine {
            key: pixel.public(),
            machine: tablet.public(),
        },
        &pixel,
    );
    assert!(refused(&member, &pc).contains("does not carry its machine's signature"));

    // A viewer declares its own; a new declaration replaces the old one.
    list.declare(&tablet, &other);
    let moved = machine(&dir, "moved");
    list.declare(&pc, &moved);
    let members = list.check(&pc.public()).unwrap();
    assert!(members.by_machine(&desk.public()).is_none());
    assert_eq!(members.by_machine(&moved.public()).unwrap().name, "pc");
    assert_eq!(members.by_machine(&other.public()).unwrap().name, "tablet");

    // A member removed takes its machine with it.
    list.sign(remove(&tablet), &pc);
    assert!(
        list.check(&pc.public())
            .unwrap()
            .by_machine(&other.public())
            .is_none()
    );

    // The declaration travels as JSON and is signed as the documented encoding: kind 4, the
    // member, then the machine; the machine's claim beside it, in its hash.
    let json = serde_json::to_value(&list).unwrap();
    assert_eq!(json[3]["action"]["kind"], "machine");
    assert_eq!(json[3]["claim"].as_str().unwrap().len(), 128);
    assert!(json[2].get("claim").is_none());
    let change = &list.changes()[3];
    let bytes = change.signed_bytes();
    let tail = &bytes[bytes.len() - 65..];
    assert_eq!(tail[0], 4);
    assert_eq!(&tail[1..33], pc.public().as_bytes());
    assert_eq!(&tail[33..], desk.public().as_bytes());
    let mut unclaimed = change.clone();
    unclaimed.claim = None;
    assert_ne!(unclaimed.hash(), change.hash());
}

#[test]
fn two_setups_by_the_same_device_never_make_the_same_list() {
    let pc = key(1);
    let (first, second) = (
        MemberList::start("pc", &pc).unwrap(),
        MemberList::start("pc", &pc).unwrap(),
    );
    assert_ne!(first.changes()[0], second.changes()[0]);
    assert_ne!(first.head(), second.head());
    // A change of the first list does not follow on the second.
    let mut old = first.clone();
    old.sign(add(&key(2), "phone", Role::Controller), &pc);
    let mut mixed = second.changes().to_vec();
    mixed.push(old.changes()[1].clone());
    assert!(
        refused(&MemberList::from_changes(mixed), &pc)
            .contains("change 2 does not follow the change before it")
    );
}

#[test]
fn a_list_longer_than_a_device_takes_is_refused_before_it_is_checked() {
    let pc = key(1);
    let root = MemberList::start("pc", &pc).unwrap().changes()[0].clone();
    let long = MemberList::from_changes(vec![root; MOST_CHANGES + MANAGING_HEADROOM + 1]);
    let said = refused(&long, &pc);
    assert!(said.contains("more than 1100 changes"), "{said}");
}

#[test]
fn a_member_cannot_declare_a_machine_it_does_not_hold() {
    let dir = tempfile::tempdir().unwrap();
    let (pc, _, tablet, list) = three();
    // The machine of pc, not declared yet: a viewer declares it, without holding its key...
    let desk = machine(&dir, "desk");
    let mut bare = list.clone();
    bare.sign(
        Action::Machine {
            key: tablet.public(),
            machine: desk.public(),
        },
        &tablet,
    );
    let said = refused(&bare, &pc);
    assert!(
        said.contains("does not carry its machine's signature"),
        "{said}"
    );
    // ...nor with a claim of another machine, or one the machine made for another member.
    let mut changes = list.changes().to_vec();
    let mut lent = declared(list.head(), &tablet, &desk);
    lent.claim = Some(machine(&dir, "tablet's").claim(&tablet.public(), &list.head()));
    changes.push(lent);
    assert!(refused(&MemberList::from_changes(changes), &pc).contains("its machine's signature"));
    let mut changes = list.changes().to_vec();
    let mut lent = declared(list.head(), &tablet, &desk);
    lent.claim = Some(desk.claim(&pc.public(), &list.head()));
    changes.push(lent);
    assert!(refused(&MemberList::from_changes(changes), &pc).contains("its machine's signature"));
    // Held, it is taken.
    let mut held = list.clone();
    held.declare(&tablet, &desk);
    assert!(held.check(&pc.public()).is_ok());
}
