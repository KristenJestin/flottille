//! The hub's state (`docs/remote.md`, "The hub", "The first member", "Pairing"): its key, the
//! setup code, the member list it stores, the invitations, presence and addresses, in a temporary
//! state folder, with a clock the tests move.

use std::cell::Cell;
use std::fmt::Debug;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use flottille_hub::{Clock, Hub, HubError, JoinRequest, Presence, reset};
use flottille_link::{
    Action, Change, ChangeHash, DeviceKey, InvitationAsk, Keyring, KeyringError, ListError,
    MemberList, Role, Signature, Token,
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

/// The first member's own entry, as `MemberList::start` signs it.
fn root_entry(key: &DeviceKey, name: &str) -> Change {
    MemberList::start(name, key).unwrap().changes()[0].clone()
}

fn add(key: &DeviceKey, name: &str, role: Role) -> Action {
    Action::Add {
        key: key.public(),
        name: name.to_owned(),
        role,
    }
}

/// `pc` sets the hub up with its code, and is the root.
fn set_up(hub: &mut Hub<Time>, pc: &DeviceKey) {
    let code = hub.setup_code().unwrap().to_owned();
    hub.setup(&code, vec![root_entry(pc, "pc")]).unwrap();
}

/// The token `inviter` is given for an invitation asked now.
fn invite(hub: &mut Hub<Time>, time: &Time, inviter: &DeviceKey) -> Token {
    let ask = InvitationAsk::sign(&hub.key(), time.now(), inviter).unwrap();
    hub.invite(&ask).unwrap()
}

/// `signer`'s change after the hub's last one.
fn next(hub: &Hub<Time>, action: Action, signer: &DeviceKey) -> Change {
    Change::sign(hub.list().head(), action, signer)
}

/// The members the hub's list leaves, checked back to `root`.
fn members(hub: &Hub<Time>, root: &DeviceKey) -> Vec<(String, Role)> {
    hub.list()
        .check(&root.public())
        .unwrap()
        .members()
        .iter()
        .map(|member| (member.name.clone(), member.role))
        .collect()
}

#[test]
fn the_setup_code_is_printed_once_and_makes_the_first_member() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);

    let code = hub.setup_code().unwrap().to_owned();
    let groups: Vec<&str> = code.split('-').collect();
    assert_eq!(groups.len(), 4, "{code}");
    for group in groups {
        assert_eq!(group.len(), 4, "{code}");
        assert!(
            group
                .chars()
                .all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)),
            "{code}"
        );
    }
    assert_eq!(
        hub.take_announcement(),
        Some(format!(
            "setup code: {code}, valid until a first member joins"
        ))
    );
    assert_eq!(hub.take_announcement(), None);
    assert_eq!(mode(&folder.join("hub.json")), 0o600);

    // Started again: the same code, kept, and not printed again.
    drop(hub);
    let mut hub = open(&folder, &time);
    assert_eq!(hub.take_announcement(), None);
    assert_eq!(hub.setup_code().unwrap(), code);
    assert!(hub.list().changes().is_empty());

    // Typed as a person may (Crockford's base32 is read in any case, hyphens or spaces between).
    let pc = key(1);
    let typed = code.to_lowercase().replace('-', " ");
    hub.setup(&typed, vec![root_entry(&pc, "pc")]).unwrap();
    assert_eq!(members(&hub, &pc), [("pc".to_owned(), Role::Runner)]);
    assert_eq!(hub.list().root(), Some(pc.public()));

    // Stored: a hub started again holds the same list.
    let list = hub.list().clone();
    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(hub.list(), &list);
    assert_eq!(members(&hub, &pc), [("pc".to_owned(), Role::Runner)]);
}

#[test]
fn the_setup_code_works_only_while_no_member_exists() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let code = hub.setup_code().unwrap().to_owned();
    let (pc, pixel) = (key(1), key(2));

    // The same code but its first symbol.
    let first = if code.starts_with('0') { "1" } else { "0" };
    let not_it = format!("{first}{}", &code[1..]);
    let not_the_hub_s = "this setup code is not the hub's: copy it again from the hub's log, or \
                         from flottille-hub setup-code on the hub's machine";
    for wrong in [not_it.as_str(), "7KQ2-M4XP", "not a code", ""] {
        assert_eq!(
            error(hub.setup(wrong, vec![root_entry(&pc, "pc")])),
            not_the_hub_s,
            "{wrong:?}"
        );
    }
    // The entry must be its signer's own, and a runner's.
    let not_own = Change::sign(
        ChangeHash::default(),
        add(&pixel, "pixel", Role::Runner),
        &pc,
    );
    assert_eq!(
        error(hub.setup(&code, vec![not_own])),
        "the hub refuses this change: its root is not the first member's"
    );
    let as_viewer = Change::sign(ChangeHash::default(), add(&pc, "pc", Role::Viewer), &pc);
    assert_eq!(
        error(hub.setup(&code, vec![as_viewer])),
        "the hub refuses this change: change 1 is not the root's own entry as a runner"
    );
    assert!(hub.list().changes().is_empty());

    hub.setup(&code, vec![root_entry(&pc, "pc")]).unwrap();
    let has_members = "the hub has a member already: a setup code serves only before the first \
                       member joins (flottille-hub setup-code --reset empties the list and makes a \
                       new one)";
    assert_eq!(
        error(hub.setup(&code, vec![root_entry(&pixel, "pixel")])),
        has_members
    );
    assert_eq!(error(hub.setup_code()), has_members);
    assert_eq!(members(&hub, &pc), [("pc".to_owned(), Role::Runner)]);

    // The code is gone from the state folder: a hub started again has none.
    drop(hub);
    let mut hub = open(&folder, &time);
    assert_eq!(hub.take_announcement(), None);
    assert_eq!(error(hub.setup_code()), has_members);
    assert!(
        !std::fs::read_to_string(folder.join("hub.json"))
            .unwrap()
            .contains(&code)
    );
}

#[test]
fn a_reset_empties_the_list_and_makes_a_new_code() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let old = hub.setup_code().unwrap().to_owned();
    let (pc, pixel) = (key(1), key(2));
    set_up(&mut hub, &pc);
    let made = hub.key();

    // A reset is `flottille-hub setup-code --reset`, a process of its own: the hub is stopped.
    drop(hub);
    let new = reset(&folder).unwrap();
    assert_ne!(new, old);

    // Started again, the hub has no member, the new code, printed by the reset already, and its
    // key.
    let mut hub = open(&folder, &time);
    assert_eq!(hub.key(), made);
    assert!(hub.list().changes().is_empty());
    assert_eq!(hub.setup_code().unwrap(), new);
    assert_eq!(hub.take_announcement(), None);

    assert_eq!(
        error(hub.setup(&old, vec![root_entry(&pixel, "pixel")])),
        "this setup code is not the hub's: copy it again from the hub's log, or from \
         flottille-hub setup-code on the hub's machine"
    );
    hub.setup(&new, vec![root_entry(&pixel, "pixel")]).unwrap();
    assert_eq!(members(&hub, &pixel), [("pixel".to_owned(), Role::Runner)]);
    // A device that pinned the old root refuses the new list, saying the hub was reset.
    assert!(matches!(
        hub.list().check(&pc.public()),
        Err(ListError::Reset)
    ));
}

#[test]
fn a_list_change_no_member_signed_is_refused_by_the_hub() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, stranger) = (key(1), key(9));

    // Before the first member, no change is taken but the setup code's.
    let early = Change::sign(
        ChangeHash::default(),
        Action::Remove { key: pc.public() },
        &pc,
    );
    assert_eq!(
        error(hub.submit(early)),
        "the hub has no member yet: the first one joins with the setup code"
    );
    set_up(&mut hub, &pc);
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
        (by_a_stranger, "is signed by a key that is not a member"),
        (broken, "does not carry its signer's signature"),
        (claimed, "does not carry its signer's signature"),
        (off_the_chain, "does not follow the change before it"),
    ] {
        assert_eq!(
            error(hub.submit(change)),
            format!("the hub refuses this change: change 2 {why}")
        );
    }
    assert_eq!(hub.list().changes().len(), 1);

    // The member's own change is stored, and kept.
    hub.submit(next(&hub, rename, &pc)).unwrap();
    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(members(&hub, &pc), [("laptop".to_owned(), Role::Runner)]);
}

#[test]
fn the_hub_cannot_add_a_member_or_change_a_role() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, pixel, stranger) = (key(1), key(2), key(9));
    set_up(&mut hub, &pc);

    // The hub's key is no device key: it signs no change, and the hub has no method that adds.
    // An addition answers a join request, signed by the member that invited.
    let no_request = "the hub refuses this change: it adds a key no join request to its signer \
                      holds (a device joins through an invitation)";
    assert_eq!(
        error(hub.submit(next(&hub, add(&pixel, "pixel", Role::Viewer), &pc))),
        no_request
    );
    let token = invite(&mut hub, &time, &pc);
    hub.join(&token, pixel.public()).unwrap();
    assert_eq!(
        error(hub.submit(next(&hub, add(&pixel, "pixel", Role::Runner), &stranger))),
        no_request
    );

    // The role is in the signed entry: one changed after signing is refused.
    let signed = next(&hub, add(&pixel, "pixel", Role::Viewer), &pc);
    let mut raised = signed.clone();
    raised.action = add(&pixel, "pixel", Role::Runner);
    assert_eq!(
        error(hub.submit(raised)),
        "the hub refuses this change: change 2 does not carry its signer's signature"
    );
    hub.submit(signed).unwrap();
    assert_eq!(
        members(&hub, &pc),
        [
            ("pc".to_owned(), Role::Runner),
            ("pixel".to_owned(), Role::Viewer)
        ]
    );

    // A viewer may not invite.
    let ask = InvitationAsk::sign(&hub.key(), time.now(), &pixel).unwrap();
    assert_eq!(
        error(hub.invite(&ask)),
        "pixel may only view: a controller or a runner may invite a device"
    );

    // A role changed by hand in the state folder: the hub refuses to start on it.
    drop(hub);
    let file = folder.join("hub.json");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"viewer\""), "{text}");
    std::fs::write(&file, text.replace("\"viewer\"", "\"runner\"")).unwrap();
    let refused = error(Hub::open(&folder, time.clone()));
    assert_eq!(
        refused,
        format!(
            "the member list in {} does not check: change 2 does not carry its signer's \
             signature; the hub stores only what members signed, so the file was changed or \
             damaged: run flottille-hub setup-code --reset on this machine to start again",
            file.display()
        )
    );
}

#[test]
fn a_reset_works_on_a_state_that_does_not_check() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let made = hub.key();
    set_up(&mut hub, &key(1));
    drop(hub);

    let file = folder.join("hub.json");
    // A role changed by hand, which does not check, and a file that is not the hub's state.
    let tampered = std::fs::read_to_string(&file)
        .unwrap()
        .replace("\"runner\"", "\"viewer\"");
    for broken in [tampered.as_str(), "not the hub's state"] {
        std::fs::write(&file, broken).unwrap();
        assert!(Hub::open(&folder, time.clone()).is_err());

        let code = reset(&folder).unwrap();
        let hub = open(&folder, &time);
        assert_eq!(hub.setup_code().unwrap(), code);
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
    let code = hub.setup_code().unwrap().to_owned();

    // Another process, or `setup-code` and `setup-code --reset`, while the hub runs: refused, so
    // that a reset is never undone by the running hub's next write, nor its old code left valid.
    let another = format!(
        "another flottille-hub uses {}: stop it first",
        folder.display()
    );
    assert_eq!(error(Hub::open(&folder, time.clone())), another);
    assert_eq!(error(reset(&folder)), another);
    assert_eq!(hub.setup_code().unwrap(), code);
    hub.setup(&code, vec![root_entry(&key(1), "pc")]).unwrap();

    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(hub.list().changes().len(), 1);
    drop(hub);
    reset(&folder).unwrap();
}

#[test]
fn an_invitation_works_once_and_expires_after_five_minutes() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, pixel, tablet, stranger) = (key(1), key(2), key(3), key(9));
    set_up(&mut hub, &pc);

    // Once.
    let token = invite(&mut hub, &time, &pc);
    hub.join(&token, pixel.public()).unwrap();
    assert_eq!(
        error(hub.join(&token, tablet.public())),
        "this invitation was answered already: ask for a new one"
    );
    assert_eq!(
        hub.requests(&pc.public()),
        [JoinRequest {
            token,
            key: pixel.public(),
            inviter: pc.public(),
        }]
    );
    assert!(hub.requests(&pixel.public()).is_empty());

    // Valid 5 minutes: at 4:59 a device still joins; at 5:00 it does not.
    let token = invite(&mut hub, &time, &pc);
    time.pass(299);
    hub.join(&token, tablet.public()).unwrap();
    let token = invite(&mut hub, &time, &pc);
    time.pass(300);
    let expired = "this invitation is unknown or expired: ask for a new one";
    assert_eq!(error(hub.join(&token, tablet.public())), expired);
    assert_eq!(error(hub.join(&Token([5; 16]), tablet.public())), expired);
    assert!(hub.requests(&pc.public()).is_empty());

    // The ask: signed by a member, for this hub, within 5 minutes, taken once.
    let ask = InvitationAsk::sign(&hub.key(), time.now(), &pc).unwrap();
    hub.invite(&ask).unwrap();
    assert_eq!(error(hub.invite(&ask)), "this ask was taken already");
    let old = InvitationAsk::sign(&hub.key(), time.now() - 300, &pc).unwrap();
    let ahead = InvitationAsk::sign(&hub.key(), time.now() + 300, &pc).unwrap();
    for ask in [old, ahead] {
        assert_eq!(
            error(hub.invite(&ask)),
            "this ask was not signed within 5 minutes of the hub's time: check the device's \
             clock"
        );
    }
    let mut forged = InvitationAsk::sign(&hub.key(), time.now(), &stranger).unwrap();
    forged.member = pc.public();
    assert_eq!(
        error(hub.invite(&forged)),
        "this ask does not carry its member's signature"
    );
    let elsewhere = InvitationAsk::sign(&stranger.public(), time.now(), &pc).unwrap();
    assert_eq!(error(hub.invite(&elsewhere)), "this ask is for another hub");
    let not_member = InvitationAsk::sign(&hub.key(), time.now(), &stranger).unwrap();
    assert_eq!(
        error(hub.invite(&not_member)),
        format!("{} is not a member", stranger.public().fingerprint())
    );
}

#[test]
fn a_refused_or_expired_join_request_adds_nothing() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, pixel, tablet) = (key(1), key(2), key(3));
    set_up(&mut hub, &pc);
    let no_request = "the hub refuses this change: it adds a key no join request to its signer \
                      holds (a device joins through an invitation)";

    // Refused, by the member that invited only.
    let token = invite(&mut hub, &time, &pc);
    hub.join(&token, pixel.public()).unwrap();
    assert_eq!(
        error(hub.refuse(&token, &tablet.public())),
        "only the member that invited may refuse its join request"
    );
    hub.refuse(&token, &pc.public()).unwrap();
    assert!(hub.requests(&pc.public()).is_empty());
    assert_eq!(
        error(hub.submit(next(&hub, add(&pixel, "pixel", Role::Controller), &pc))),
        no_request
    );
    assert_eq!(
        error(hub.join(&token, tablet.public())),
        "this invitation is unknown or expired: ask for a new one"
    );
    assert_eq!(
        error(hub.refuse(&token, &pc.public())),
        "no join request holds this invitation"
    );

    // Expired with its token, 5 minutes after the invitation, whenever the device joined.
    let token = invite(&mut hub, &time, &pc);
    time.pass(200);
    hub.join(&token, pixel.public()).unwrap();
    time.pass(100);
    assert!(hub.requests(&pc.public()).is_empty());
    assert_eq!(
        error(hub.submit(next(&hub, add(&pixel, "pixel", Role::Controller), &pc))),
        no_request
    );

    assert_eq!(hub.list().changes().len(), 1);
    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(members(&hub, &pc), [("pc".to_owned(), Role::Runner)]);
}

#[test]
fn an_accepted_join_request_adds_the_member_once() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let (pc, pixel) = (key(1), key(2));
    set_up(&mut hub, &pc);

    let token = invite(&mut hub, &time, &pc);
    hub.join(&token, pixel.public()).unwrap();
    let entry = next(&hub, add(&pixel, "pixel", Role::Controller), &pc);
    hub.submit(entry.clone()).unwrap();
    assert!(hub.requests(&pc.public()).is_empty());
    assert_eq!(
        error(hub.submit(entry)),
        "the hub refuses this change: it adds a key no join request to its signer holds (a \
         device joins through an invitation)"
    );

    drop(hub);
    let hub = open(&folder, &time);
    assert_eq!(
        members(&hub, &pc),
        [
            ("pc".to_owned(), Role::Runner),
            ("pixel".to_owned(), Role::Controller)
        ]
    );
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
    set_up(&mut hub, &pc);

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
    set_up(&mut hub, &pc);
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
    let state = serde_json::json!({"version": 1, "members": list});
    std::fs::write(folder.join("hub.json"), state.to_string()).unwrap();
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
    let state = serde_json::json!({"version": 1, "members": list});
    std::fs::write(folder.join("hub.json"), state.to_string()).unwrap();
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
    assert_eq!(members(&hub, &pc).len(), 1);
}

#[test]
fn a_setup_is_stored_whole_or_not_at_all() {
    let (_dir, folder) = folder();
    let time = Time::new();
    let mut hub = open(&folder, &time);
    let code = hub.setup_code().unwrap().to_owned();
    let pc = key(1);
    // Its second change does not check: nothing of the setup is kept, and the code still works.
    let mut list = MemberList::start("pc", &pc).unwrap();
    list.sign(
        Action::Machine {
            key: pc.public(),
            machine: key(10).public(),
        },
        &pc,
    );
    let refused = error(hub.setup(&code, list.changes().to_vec()));
    assert!(
        refused.contains("change 2 does not carry its machine's signature"),
        "{refused}"
    );
    assert!(hub.list().changes().is_empty());
    drop(hub);
    let mut hub = open(&folder, &time);
    assert_eq!(hub.setup_code().unwrap(), code);
    hub.setup(&code, vec![list.changes()[0].clone()]).unwrap();
}
