//! The member list (`docs/remote.md`, "The hub", "The first member", "Pairing", "Roles"): an
//! ordered log of changes, each signed by the device key of a member that may make it, each naming
//! the hash of the change before it. The first change is the root's own entry, signed by itself;
//! every device pins that root, and accepts a list only if every change checks back to it. The hub
//! stores and sends the list, and can make no change of it: dropping, reordering or replaying a
//! change breaks the chain, and changing an entry breaks its signature.
//!
//! What a change's signature covers, its **signed bytes**, is a fixed encoding, never a JSON text
//! whose key order or spacing could vary:
//!
//! ```text
//! "flottille-member-change-v1" 0x00    the domain: nothing else a device key signs starts so
//! previous   32 bytes                  the hash of the change before it; for the first, the
//!                                      list's id: 32 random bytes made at setup
//! signer     32 bytes                  the signing member's public key
//! kind        1 byte                   1 add, 2 remove, 3 rename, 4 machine
//! key        32 bytes                  the member the change is about
//! add:     name length (4 bytes, big-endian), name (UTF-8), role (1 runner, 2 controller, 3 viewer)
//! remove:  nothing more
//! rename:  name length (4 bytes, big-endian), name (UTF-8)
//! machine: the machine key (32 bytes)
//! ```
//!
//! The list's id makes every setup a list of its own: a device that sets a hub up again after a
//! reset, with the same key, the same name and the same machine, signs another first change, so
//! that no change of the old list follows on the new one.
//!
//! A `machine` change is a member's own declaration of the key its machine's daemon holds: the
//! list holds the devices' keys, while the link and the control session to the hub run on the
//! machines' keys, and that tie must be one the hub cannot make either. The machine signs it too,
//! so that no member declares a machine whose key it does not hold: its **claim** is the machine
//! key's signature over
//!
//! ```text
//! "flottille-machine-claim-v1" 0x00
//! member     32 bytes                  the member that declares it
//! previous   32 bytes                  the hash of the change before the declaration
//! ```
//!
//! A change's hash, which the next change names, is the SHA-256 of its signed bytes followed by its
//! 64-byte signature, then, for a `machine` change, its claim's 64 bytes.

use std::fmt;

use ed25519_dalek::Signer;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::serial::{from_hex_text, invisible, to_hex};
use crate::{DeviceKey, KeyError, MachineKey, PublicKey};

/// The start of every change's signed bytes.
const DOMAIN: &[u8] = b"flottille-member-change-v1\0";
/// The start of what a machine signs when a member declares it.
const CLAIM: &[u8] = b"flottille-machine-claim-v1\0";

/// The most changes a list holds of any member's (`MANAGING_HEADROOM` more of a controller's or
/// a runner's): a device refuses a longer one before it checks a change of it, and the hub refuses
/// a change past it, so that the list always fits the line it travels in (`control::LINE_LIMIT`,
/// a change being at most some 1.3 KB of JSON).
pub const MOST_CHANGES: usize = 1000;
/// The changes a list takes past `MOST_CHANGES`, each a controller's or a runner's change of
/// another member (a removal, mostly): a list a member filled with its own changes can still be
/// cleaned. A device takes a list of `MOST_CHANGES + MANAGING_HEADROOM` changes at most.
pub const MANAGING_HEADROOM: usize = 100;

/// The longest name a member may have, in characters.
const NAME_LIMIT: usize = 64;

/// What a member may do (`docs/remote.md`, "Roles"), signed in its entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Runner,
    Controller,
    Viewer,
}

impl Role {
    fn byte(self) -> u8 {
        match self {
            Role::Runner => 1,
            Role::Controller => 2,
            Role::Viewer => 3,
        }
    }

    /// Whether a member of this role may add, remove and rename members.
    fn manages(self) -> bool {
        self != Role::Viewer
    }

    /// The role as the user reads it: `runner`, `controller`, `viewer`.
    pub fn word(self) -> &'static str {
        match self {
            Role::Runner => "runner",
            Role::Controller => "controller",
            Role::Viewer => "viewer",
        }
    }
}

/// What a change does. There is no change of a role: a role is changed by removing the device,
/// which then joins again with a new key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Add {
        key: PublicKey,
        name: String,
        role: Role,
    },
    Remove {
        key: PublicKey,
    },
    Rename {
        key: PublicKey,
        name: String,
    },
    /// The member `key` says its machine's daemon holds `machine`: signed by that member only.
    Machine {
        key: PublicKey,
        machine: PublicKey,
    },
}

impl Action {
    fn key(&self) -> &PublicKey {
        match self {
            Action::Add { key, .. }
            | Action::Remove { key }
            | Action::Rename { key, .. }
            | Action::Machine { key, .. } => key,
        }
    }

    fn verb(&self) -> &'static str {
        match self {
            Action::Add { .. } => "add",
            Action::Remove { .. } => "remove",
            Action::Rename { .. } => "rename",
            Action::Machine { .. } => "declare the machine of",
        }
    }
}

/// The hash of a change, which the change after it names. The first change names zeros.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangeHash(pub [u8; 32]);

/// An Ed25519 signature, over a change's signed bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Signature").field(&to_hex(&self.0)).finish()
    }
}

/// One change of the list, as its signer signed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub previous: ChangeHash,
    pub signer: PublicKey,
    pub action: Action,
    pub signature: Signature,
    /// For a `machine` change, the machine's own signature (`MachineKey::claim`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<Signature>,
}

impl Change {
    /// `action`, after the change whose hash is `previous`, signed with `key`. A `machine` change
    /// is claimed by its machine after (`claimed_by`).
    pub fn sign(previous: ChangeHash, action: Action, key: &DeviceKey) -> Change {
        let mut change = Change {
            previous,
            signer: key.public(),
            action,
            signature: Signature([0; 64]),
            claim: None,
        };
        change.signature = Signature(key.0.sign(&change.signed_bytes()).to_bytes());
        change
    }

    /// A `machine` change, the machine's claim added: the machine's daemon adds it, for its own
    /// key only. Any other change is left as it is.
    pub fn claimed_by(mut self, machine: &MachineKey) -> Change {
        if let Action::Machine { key, .. } = &self.action {
            self.claim = Some(machine.claim(key, &self.previous));
        }
        self
    }

    /// What the signature covers: the encoding of the module's documentation.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut bytes = DOMAIN.to_vec();
        bytes.extend_from_slice(&self.previous.0);
        bytes.extend_from_slice(self.signer.as_bytes());
        let name = |bytes: &mut Vec<u8>, name: &str| {
            // A name longer than 4 GB cannot be signed; it would be refused as a name anyway.
            bytes.extend_from_slice(&(name.len() as u32).to_be_bytes());
            bytes.extend_from_slice(name.as_bytes());
        };
        match &self.action {
            Action::Add {
                key,
                name: added,
                role,
            } => {
                bytes.push(1);
                bytes.extend_from_slice(key.as_bytes());
                name(&mut bytes, added);
                bytes.push(role.byte());
            }
            Action::Remove { key } => {
                bytes.push(2);
                bytes.extend_from_slice(key.as_bytes());
            }
            Action::Rename { key, name: new } => {
                bytes.push(3);
                bytes.extend_from_slice(key.as_bytes());
                name(&mut bytes, new);
            }
            Action::Machine { key, machine } => {
                bytes.push(4);
                bytes.extend_from_slice(key.as_bytes());
                bytes.extend_from_slice(machine.as_bytes());
            }
        }
        bytes
    }

    pub fn hash(&self) -> ChangeHash {
        let mut hasher = Sha256::new();
        hasher.update(self.signed_bytes());
        hasher.update(self.signature.0);
        if let Some(claim) = &self.claim {
            hasher.update(claim.0);
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&hasher.finalize());
        ChangeHash(hash)
    }

    /// Whether the signature is the signer's over the signed bytes. Strict: a signature that could
    /// be rewritten into another valid one is refused, so that a change has one hash.
    fn signed(&self) -> bool {
        let signature = ed25519_dalek::Signature::from_bytes(&self.signature.0);
        self.signer
            .0
            .verify_strict(&self.signed_bytes(), &signature)
            .is_ok()
    }

    /// Whether a `machine` change carries its machine's claim.
    fn claimed(&self) -> bool {
        let (Action::Machine { key, machine }, Some(claim)) = (&self.action, &self.claim) else {
            return false;
        };
        let claim = ed25519_dalek::Signature::from_bytes(&claim.0);
        machine
            .0
            .verify_strict(&claim_bytes(key, &self.previous), &claim)
            .is_ok()
    }
}

/// What a machine signs when `member` declares it after the change `previous`.
fn claim_bytes(member: &PublicKey, previous: &ChangeHash) -> Vec<u8> {
    let mut bytes = CLAIM.to_vec();
    bytes.extend_from_slice(member.as_bytes());
    bytes.extend_from_slice(&previous.0);
    bytes
}

impl MachineKey {
    /// This machine's claim, for `member`'s declaration of it after the change `previous`.
    pub fn claim(&self, member: &PublicKey, previous: &ChangeHash) -> Signature {
        Signature(self.0.sign(&claim_bytes(member, previous)).to_bytes())
    }
}

/// The member list as the hub keeps and sends it: every change, the root's first. It travels as
/// the JSON array of its changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemberList(Vec<Change>);

impl MemberList {
    /// A new list: the root, the first member, a `runner`, its entry signed by itself after a
    /// new list id.
    pub fn start(name: &str, root: &DeviceKey) -> Result<MemberList, KeyError> {
        let mut id = [0u8; 32];
        getrandom::fill(&mut id).map_err(|error| KeyError(format!("no random bytes: {error}")))?;
        let entry = Action::Add {
            key: root.public(),
            name: name.to_owned(),
            role: Role::Runner,
        };
        Ok(MemberList(vec![Change::sign(ChangeHash(id), entry, root)]))
    }

    pub fn from_changes(changes: Vec<Change>) -> MemberList {
        MemberList(changes)
    }

    pub fn changes(&self) -> &[Change] {
        &self.0
    }

    /// The hash of the last change, which the next one names; zeros for an empty list.
    pub fn head(&self) -> ChangeHash {
        self.0.last().map(Change::hash).unwrap_or_default()
    }

    /// The key the list says is its root, which only a device that has none pinned yet takes.
    pub fn root(&self) -> Option<PublicKey> {
        self.0.first().map(|first| *first.action.key())
    }

    /// Signs `action` with `key` after the last change, and appends it. Nothing is checked: the
    /// caller checks the list before sending the change.
    pub fn sign(&mut self, action: Action, key: &DeviceKey) -> &Change {
        let change = Change::sign(self.head(), action, key);
        self.0.push(change);
        self.0.last().expect("just pushed")
    }

    /// `member` declares `machine` after the last change, claimed by the machine; appended.
    pub fn declare(&mut self, member: &DeviceKey, machine: &MachineKey) -> &Change {
        let action = Action::Machine {
            key: member.public(),
            machine: machine.public(),
        };
        let change = Change::sign(self.head(), action, member).claimed_by(machine);
        self.0.push(change);
        self.0.last().expect("just pushed")
    }

    /// The members, when every change checks back to `root`, the root this device pinned.
    pub fn check(&self, root: &PublicKey) -> Result<Members, ListError> {
        let Some(first) = self.0.first() else {
            // Emptied on the hub's machine, before a first member joins again.
            return Err(ListError::Reset);
        };
        if first.action.key() != root {
            return Err(ListError::Reset);
        }
        let most = MOST_CHANGES + MANAGING_HEADROOM;
        if self.0.len() > most {
            return Err(ListError::Refused(format!(
                "it holds {} changes, more than {most} changes, the most a device takes",
                self.0.len()
            )));
        }
        let mut members = Members {
            root: *root,
            members: Vec::new(),
            removed: Vec::new(),
            // The first change names the list's id, whatever it is.
            head: first.previous,
        };
        for (at, change) in self.0.iter().enumerate() {
            members
                .apply(change, at == 0, at >= MOST_CHANGES)
                .map_err(|why| ListError::Refused(format!("change {} {why}", at + 1)))?;
        }
        Ok(members)
    }

    /// As `check`, for a list sent to a device that has already checked `known`: the new one must
    /// hold every change of it, in order, so that a hub cannot take back a change it sent.
    pub fn check_update(&self, root: &PublicKey, known: &MemberList) -> Result<Members, ListError> {
        // Another first change: another list, set up again after a reset, by this root or not.
        if let (Some(ours), Some(theirs)) = (known.0.first(), self.0.first())
            && ours != theirs
        {
            return Err(ListError::Reset);
        }
        let members = self.check(root)?;
        if !self.0.starts_with(&known.0) {
            return Err(ListError::Refused(
                "it lacks changes this device has already checked".to_owned(),
            ));
        }
        Ok(members)
    }
}

/// A member of a checked list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub key: PublicKey,
    pub name: String,
    pub role: Role,
    /// The member whose signature added it; the root's own key for the root.
    pub added_by: PublicKey,
    /// The key its machine's daemon holds, as the member itself declared it.
    pub machine: Option<PublicKey>,
}

/// The members a checked list leaves, in the order they were added.
#[derive(Clone, Debug)]
pub struct Members {
    root: PublicKey,
    members: Vec<Member>,
    /// Every key ever removed: none comes back.
    removed: Vec<PublicKey>,
    head: ChangeHash,
}

impl Members {
    pub fn root(&self) -> PublicKey {
        self.root
    }

    /// The hash of the list's last change.
    pub fn head(&self) -> ChangeHash {
        self.head
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }

    pub fn get(&self, key: &PublicKey) -> Option<&Member> {
        self.members.iter().find(|member| member.key == *key)
    }

    /// Whether `key` was a member once and was removed: it is never added again.
    pub fn was_removed(&self, key: &PublicKey) -> bool {
        self.removed.contains(key)
    }

    /// The member whose machine holds `machine`.
    pub fn by_machine(&self, machine: &PublicKey) -> Option<&Member> {
        self.members
            .iter()
            .find(|member| member.machine.as_ref() == Some(machine))
    }

    /// Applies one change, or says why it is refused, after its number. Past `MOST_CHANGES`, only
    /// a managing member's change of another member is taken.
    fn apply(&mut self, change: &Change, first: bool, past: bool) -> Result<(), String> {
        if change.previous != self.head {
            return Err("does not follow the change before it".to_owned());
        }
        if !change.signed() {
            return Err("does not carry its signer's signature".to_owned());
        }
        if first {
            let Action::Add { key, name, role } = &change.action else {
                return Err("is not the root's own entry as a runner".to_owned());
            };
            if *role != Role::Runner || change.signer != *key {
                return Err("is not the root's own entry as a runner".to_owned());
            }
            if change.claim.is_some() {
                return Err(
                    "carries a machine's signature, which only a machine change has".to_owned(),
                );
            }
            self.add(*key, name, Role::Runner, *key)?;
            self.head = change.hash();
            return Ok(());
        }
        let Some(signer) = self.get(&change.signer) else {
            return Err("is signed by a key that is not a member".to_owned());
        };
        let own = match &change.action {
            Action::Remove { key } | Action::Machine { key, .. } => *key == signer.key,
            Action::Add { .. } | Action::Rename { .. } => false,
        };
        if matches!(change.action, Action::Machine { .. }) && !own {
            return Err(
                "declares another member's machine: a member declares its own machine \
                        only"
                    .to_owned(),
            );
        }
        if past && (!signer.role.manages() || change.action.key() == &signer.key) {
            return Err(format!(
                "would make more than {MOST_CHANGES} changes: past them, a list takes only a \
                 controller's or a runner's change of another member"
            ));
        }
        if !signer.role.manages() && !own {
            return Err(format!(
                "is signed by {}, a {}: a controller or a runner may {} a member",
                signer.name,
                signer.role.word(),
                change.action.verb()
            ));
        }
        match &change.action {
            Action::Machine { .. } if !change.claimed() => {
                return Err(
                    "does not carry its machine's signature: a member declares only a \
                            machine whose key it holds"
                        .to_owned(),
                );
            }
            Action::Machine { .. } => {}
            _ if change.claim.is_some() => {
                return Err(
                    "carries a machine's signature, which only a machine change has".to_owned(),
                );
            }
            _ => {}
        }
        match &change.action {
            Action::Add { key, name, role } => self.add(*key, name, *role, change.signer)?,
            Action::Remove { key } => {
                let at = self.position(key)?;
                self.members.remove(at);
                self.removed.push(*key);
            }
            Action::Rename { key, name } => {
                let at = self.position(key)?;
                self.free(name, Some(key))?;
                self.members[at].name = name.clone();
            }
            Action::Machine { key, machine } => {
                if self.members.iter().any(|member| member.key == *machine)
                    || self.removed.contains(machine)
                {
                    return Err("declares a member's key as a machine".to_owned());
                }
                if self
                    .by_machine(machine)
                    .is_some_and(|member| member.key != *key)
                {
                    return Err("declares a machine another member declared".to_owned());
                }
                let at = self.position(key)?;
                self.members[at].machine = Some(*machine);
            }
        }
        self.head = change.hash();
        Ok(())
    }

    fn add(
        &mut self,
        key: PublicKey,
        name: &str,
        role: Role,
        added_by: PublicKey,
    ) -> Result<(), String> {
        if self.removed.contains(&key) {
            return Err("adds a key that was removed".to_owned());
        }
        if self.get(&key).is_some() {
            return Err("adds a key that is a member already".to_owned());
        }
        self.free(name, None)?;
        if self.by_machine(&key).is_some() {
            return Err("adds a key that is a member's machine".to_owned());
        }
        self.members.push(Member {
            key,
            name: name.to_owned(),
            role,
            added_by,
            machine: None,
        });
        Ok(())
    }

    fn position(&self, key: &PublicKey) -> Result<usize, String> {
        self.members
            .iter()
            .position(|member| member.key == *key)
            .ok_or_else(|| format!("is about {}, which is not a member", key.fingerprint()))
    }

    /// Whether `name` is a name, and no member's but `renamed`'s; names differ in more than case.
    fn free(&self, name: &str, renamed: Option<&PublicKey>) -> Result<(), String> {
        let length = name.chars().count();
        if length == 0
            || length > NAME_LIMIT
            || name.chars().any(|c| c.is_control() || invisible(c))
            || name.trim() != name
        {
            return Err(format!(
                "names a device {name:?}: a name is 1 to {NAME_LIMIT} characters, with no control \
                 character, no invisible character and no space at either end"
            ));
        }
        let lower = name.to_lowercase();
        let taken = self
            .members
            .iter()
            .any(|member| Some(&member.key) != renamed && member.name.to_lowercase() == lower);
        if taken {
            return Err(format!("names a device {name}, a name a member has"));
        }
        Ok(())
    }
}

/// Why a device refuses a member list.
#[derive(Debug)]
pub enum ListError {
    /// Its root is not the one this device pinned: the hub was set up again.
    Reset,
    /// A change of it does not check, or the list is not what this device expects.
    Refused(String),
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListError::Reset => {
                f.write_str("the hub's member list was reset: set this device up again")
            }
            ListError::Refused(why) => write!(f, "the hub's member list was refused: {why}"),
        }
    }
}

impl std::error::Error for ListError {}

impl Serialize for ChangeHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for ChangeHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<ChangeHash, D::Error> {
        from_hex_text(deserializer, "a change's hash").map(ChangeHash)
    }
}

impl Serialize for Signature {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Signature, D::Error> {
        from_hex_text(deserializer, "a signature").map(Signature)
    }
}
