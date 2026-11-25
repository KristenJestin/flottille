//! The values of all machines (`docs/remote.md`, "Settings for all machines"): a value the user's
//! device signed, sealed by a machine for every machine of the user's, and kept by the hub without
//! being read.
//!
//! **What is sealed** is the request the device signed (`about.share`, `request.rs`), whole, so
//! that each machine checks the user's signature itself, whichever machine sealed it and whatever
//! the hub says. **The seal** is end to end between the machines: a random key encrypts the value
//! (ChaCha20-Poly1305), and that key is encrypted again for each machine's key, by an X25519
//! agreement between a key made for this seal alone and the machine's key turned into its X25519
//! form (the libsodium conversion), through HKDF-SHA256. The value's name and time are outside, for
//! the hub to keep the latest of each, and bound to the encrypted bytes as their associated data,
//! so that the hub cannot give one value the name or the time of another. The sealing machine
//! signs the whole with its machine key: the hub takes a copy from a member's machine only.
//!
//! What the signature covers, its **signed bytes**:
//!
//! ```text
//! "flottille-shared-v1" 0x00
//! name       length (4 bytes, big-endian), UTF-8
//! at         8 bytes, big-endian (milliseconds since the Unix epoch, the device's)
//! from       32 bytes (the machine that sealed it)
//! ephemeral  32 bytes (the X25519 key made for this seal)
//! boxes      count (4 bytes, big-endian), then each: to (32 bytes), key length (4), key
//! data       length (4 bytes, big-endian), the encrypted value
//! ```

use std::fmt;

use base64::Engine as _;
use curve25519_dalek::MontgomeryPoint;
use ed25519_dalek::Signer;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

use crate::request::{Request, RequestError};
use crate::{KeyError, MachineKey, Member, Members, PublicKey, Role, Signature};

const DOMAIN: &[u8] = b"flottille-shared-v1\0";
/// What the key of each machine is derived under.
const KEY_INFO: &[u8] = b"flottille-shared-key-v1";

/// The longest name of a value.
pub const NAME_LIMIT: usize = 64;
/// The most a sealed copy holds, its value and its boxes together: the hub refuses a larger one.
pub const SEALED_LIMIT: usize = 40 * 1024;
/// The most values the hub keeps.
pub const MOST_VALUES: usize = 32;
/// The names of the values of all machines: the hub keeps no other, so that a member's machine
/// cannot take its places with names of its own. A setting shared later adds its name here.
pub const SHARED_NAMES: &[&str] = &[
    "about.name",
    "about.pronouns",
    "about.notes",
    // The settings a machine often overrides.
    "new_session",
    "judge",
    "compactor",
    "review.spec_model",
    "review.bugs_model",
    "review.standards_model",
    "review.security_model",
    "review.verifier_model",
    "sessions.end_inactive_days",
    "sessions.resume_at_reset",
    "workspaces_folder",
    "update.check",
    "connections",
];

/// Bytes, in base64 in JSON.
#[derive(Clone, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Bytes, D::Error> {
        let text = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map(Bytes)
            .map_err(serde::de::Error::custom)
    }
}

/// The key of the value, encrypted for one machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedKey {
    pub to: PublicKey,
    pub key: Bytes,
}

/// A value of all machines as the hub keeps it: opaque but its name and its time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sealed {
    /// The value's name: `about.name`.
    pub name: String,
    /// When the user changed it, in milliseconds since the Unix epoch: the latest wins.
    pub at: u64,
    /// The machine that sealed it.
    pub from: PublicKey,
    pub ephemeral: Bytes,
    pub boxes: Vec<SealedKey>,
    pub data: Bytes,
    pub sig: Signature,
}

fn random<const N: usize>() -> Result<[u8; N], KeyError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes)
        .map_err(|error| KeyError(format!("no random bytes for a seal: {error}")))?;
    Ok(bytes)
}

fn prefix(length: usize) -> Option<[u8; 4]> {
    u32::try_from(length).ok().map(u32::to_be_bytes)
}

/// What the value's encryption is bound to: its name and its time.
fn associated(name: &str, at: u64) -> Vec<u8> {
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(&at.to_be_bytes());
    bytes
}

/// ChaCha20-Poly1305 under `key`: each key encrypts one text only, so its nonce is zero.
fn aead(key: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).expect("a 32-byte key"))
}

fn encrypt(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut bytes = plain.to_vec();
    aead(key)
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key([0; 12]),
            Aad::from(aad),
            &mut bytes,
        )
        .expect("a text under 256 GB");
    bytes
}

fn decrypt(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let mut bytes = sealed.to_vec();
    let plain = aead(key)
        .open_in_place(
            Nonce::assume_unique_for_key([0; 12]),
            Aad::from(aad),
            &mut bytes,
        )
        .ok()?
        .to_vec();
    Some(plain)
}

/// The key a machine's box is encrypted under: HKDF-SHA256 of the X25519 agreement, salted with
/// both public keys. None when the agreement is the zero point, which a key of small order gives.
fn box_key(agreed: MontgomeryPoint, ephemeral: &[u8; 32], to: &PublicKey) -> Option<[u8; 32]> {
    if agreed.to_bytes() == [0; 32] {
        return None;
    }
    let mut salt = ephemeral.to_vec();
    salt.extend_from_slice(to.as_bytes());
    let mut agreed = agreed.to_bytes();
    let prk = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, &salt).extract(&agreed);
    agreed.zeroize();
    let mut key = [0u8; 32];
    prk.expand(&[KEY_INFO], ring::hkdf::HKDF_SHA256)
        .ok()?
        .fill(&mut key)
        .ok()?;
    Some(key)
}

impl Sealed {
    /// `plain`, the value `name` changed by the user at `at`, sealed for each machine of `to` and
    /// signed with `machine`.
    pub fn seal(
        name: &str,
        at: u64,
        plain: &[u8],
        to: &[PublicKey],
        machine: &MachineKey,
    ) -> Result<Sealed, KeyError> {
        let aad = associated(name, at);
        let mut value_key = random::<32>()?;
        let data = encrypt(&value_key, &aad, plain);
        let mut secret = random::<32>()?;
        let ephemeral = MontgomeryPoint::mul_base_clamped(secret).to_bytes();
        let mut boxes = Vec::new();
        for key in to {
            let agreed = key.0.to_montgomery().mul_clamped(secret);
            let Some(mut wrapping) = box_key(agreed, &ephemeral, key) else {
                continue;
            };
            boxes.push(SealedKey {
                to: *key,
                key: Bytes(encrypt(&wrapping, &aad, &value_key)),
            });
            wrapping.zeroize();
        }
        secret.zeroize();
        value_key.zeroize();
        let mut sealed = Sealed {
            name: name.to_owned(),
            at,
            from: machine.public(),
            ephemeral: Bytes(ephemeral.to_vec()),
            boxes,
            data: Bytes(data),
            sig: Signature([0; 64]),
        };
        let bytes = sealed
            .signed_bytes()
            .ok_or_else(|| KeyError("this value is too large to seal".to_owned()))?;
        sealed.sig = Signature(machine.0.sign(&bytes).to_bytes());
        Ok(sealed)
    }

    /// What the signature covers (the module's documentation); none for a field past its length
    /// prefix.
    pub fn signed_bytes(&self) -> Option<Vec<u8>> {
        let mut bytes = DOMAIN.to_vec();
        bytes.extend_from_slice(&prefix(self.name.len())?);
        bytes.extend_from_slice(self.name.as_bytes());
        bytes.extend_from_slice(&self.at.to_be_bytes());
        bytes.extend_from_slice(self.from.as_bytes());
        bytes.extend_from_slice(&prefix(self.ephemeral.0.len())?);
        bytes.extend_from_slice(&self.ephemeral.0);
        bytes.extend_from_slice(&prefix(self.boxes.len())?);
        for sealed in &self.boxes {
            bytes.extend_from_slice(sealed.to.as_bytes());
            bytes.extend_from_slice(&prefix(sealed.key.0.len())?);
            bytes.extend_from_slice(&sealed.key.0);
        }
        bytes.extend_from_slice(&prefix(self.data.0.len())?);
        bytes.extend_from_slice(&self.data.0);
        Some(bytes)
    }

    /// How many bytes it holds: what the hub bounds.
    pub fn size(&self) -> usize {
        self.name.len()
            + self.data.0.len()
            + self
                .boxes
                .iter()
                .map(|sealed| 32 + sealed.key.0.len())
                .sum::<usize>()
    }

    /// Whether the signature is its machine's.
    pub fn is_signed(&self) -> bool {
        let Some(bytes) = self.signed_bytes() else {
            return false;
        };
        self.from
            .0
            .verify_strict(&bytes, &ed25519_dalek::Signature::from_bytes(&self.sig.0))
            .is_ok()
    }

    /// The machines it is sealed for.
    pub fn recipients(&self) -> impl Iterator<Item = &PublicKey> {
        self.boxes.iter().map(|sealed| &sealed.to)
    }

    /// Whether the hub may keep it: a name of letters, digits, `.` and `_` among `SHARED_NAMES`, a
    /// seal's key of 32 bytes, the size bound, and the signature of a member's machine whose role
    /// may change settings (a runner's or a controller's). Why not, in a sentence.
    pub fn check_for_hub(&self, members: &Members) -> Result<(), String> {
        let named = (1..=NAME_LIMIT).contains(&self.name.len())
            && self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_');
        if !named {
            return Err("a value's name is 1 to 64 letters, digits, . or _".to_owned());
        }
        if !SHARED_NAMES.contains(&self.name.as_str()) {
            return Err(format!("{} is no value of all machines", self.name));
        }
        if self.ephemeral.0.len() != 32 {
            return Err("a value's seal holds a key of 32 bytes".to_owned());
        }
        if self.size() > SEALED_LIMIT {
            return Err(format!(
                "a value of all machines holds {SEALED_LIMIT} bytes at most"
            ));
        }
        let member = members
            .by_machine(&self.from)
            .ok_or("only a member's machine seals a value of all machines")?;
        if member.role == Role::Viewer {
            return Err(format!("{} may only view", member.name));
        }
        if !self.is_signed() {
            return Err("this value is not signed by the machine it names".to_owned());
        }
        Ok(())
    }

    /// The value, opened with this machine's key: none when it is not sealed for it, or was
    /// changed on the way.
    pub fn open(&self, machine: &MachineKey) -> Option<Vec<u8>> {
        let own = machine.public();
        let sealed = self.boxes.iter().find(|sealed| sealed.to == own)?;
        let ephemeral: [u8; 32] = self.ephemeral.0.as_slice().try_into().ok()?;
        let mut scalar = machine.0.to_scalar_bytes();
        let agreed = MontgomeryPoint(ephemeral).mul_clamped(scalar);
        scalar.zeroize();
        let mut wrapping = box_key(agreed, &ephemeral, &own)?;
        let aad = associated(&self.name, self.at);
        let value_key = decrypt(&wrapping, &aad, &sealed.key.0);
        wrapping.zeroize();
        let mut value_key: [u8; 32] = value_key?.as_slice().try_into().ok()?;
        let plain = decrypt(&value_key, &aad, &self.data.0);
        value_key.zeroize();
        plain
    }
}

/// The user's change of a value of all machines, as a machine checks it once opened: the request
/// `method` a member's device signed, by a member that may change settings (a runner or a
/// controller, never a viewer). Neither its time window nor its nonce is checked: a machine off
/// for a week takes the latest copy when it is back, and an older one never replaces a newer one.
pub fn check_shared(
    text: &[u8],
    method: &str,
    members: &Members,
) -> Result<(Member, Request, serde_json::Value), RequestError> {
    let refused = |why: &str| RequestError(why.to_owned());
    let request: Request =
        serde_json::from_slice(text).map_err(|_| refused("this is not a signed request"))?;
    let member = members
        .get(&request.signed.device)
        .filter(|_| request.is_signed())
        .ok_or_else(|| refused("this request is not signed by a member"))?;
    if member.role == Role::Viewer {
        return Err(RequestError(format!(
            "{} may only view: a controller or a runner may change the values of all machines",
            member.name
        )));
    }
    if request.signed.method != method {
        return Err(refused("this is not a change of a value of all machines"));
    }
    let params = serde_json::from_str(&request.signed.params)
        .map_err(|_| refused("its params are not JSON"))?;
    Ok((member.clone(), request, params))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> (tempfile::TempDir, MachineKey) {
        let folder = tempfile::tempdir().unwrap();
        // The key's folder is made by it, the user's only, whatever the temporary folder's mode.
        let key = MachineKey::load_or_make(&folder.path().join("remote")).unwrap();
        (folder, key)
    }

    #[test]
    fn each_machine_it_is_sealed_for_opens_it_and_no_other() {
        let ((_a, pc), (_b, server), (_c, other)) = (machine(), machine(), machine());
        let sealed = Sealed::seal(
            "about.name",
            7,
            b"Sam",
            &[pc.public(), server.public()],
            &pc,
        )
        .unwrap();

        assert_eq!(sealed.open(&pc).as_deref(), Some(&b"Sam"[..]));
        assert_eq!(sealed.open(&server).as_deref(), Some(&b"Sam"[..]));
        assert_eq!(sealed.open(&other), None);
        assert!(sealed.is_signed());
        let text = serde_json::to_string(&sealed).unwrap();
        assert!(!text.contains("Sam"), "{text}");
        let back: Sealed = serde_json::from_str(&text).unwrap();
        assert_eq!(back, sealed);
    }

    #[test]
    fn a_name_or_a_time_changed_on_the_way_opens_nothing() {
        let (_a, pc) = machine();
        let sealed = Sealed::seal("about.name", 7, b"Sam", &[pc.public()], &pc).unwrap();

        let renamed = Sealed {
            name: "about.notes".into(),
            ..sealed.clone()
        };
        let later = Sealed {
            at: 8,
            ..sealed.clone()
        };

        assert_eq!(renamed.open(&pc), None);
        assert!(!renamed.is_signed());
        assert_eq!(later.open(&pc), None);
        assert!(!later.is_signed());
    }
}
