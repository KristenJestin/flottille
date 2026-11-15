//! The invitation a member shows to add a device (`docs/remote.md`, "Pairing"), and the new
//! device's check of the first list the hub sends it.
//!
//! The code, which is also the QR's text, is in Crockford's base32 (digits and capital letters,
//! which a QR holds in its compact alphanumeric mode; read in any case, `O` as `0`, `I` and `L` as
//! `1`, hyphens and spaces skipped), of these bytes:
//!
//! ```text
//! version     1 byte      1
//! hub key    32 bytes     the hub's public key, which the new device pins
//! inviter    32 bytes     the inviting member's public key, whole
//! token      16 bytes     the hub's token for this invitation
//! hub URL    the rest     UTF-8
//! checksum    4 bytes     the first 4 bytes of the SHA-256 of "flottille-invitation-v1" 0x00
//!                         and every byte above
//! ```
//!
//! A code that does not decode, or whose checksum is not its own, is refused as damaged.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::serial::{from_hex_text, to_hex};
use crate::{ListError, MemberList, Members, PublicKey, verification_words};

const VERSION: u8 = 1;
const CHECKSUM_DOMAIN: &[u8] = b"flottille-invitation-v1\0";
const CHECKSUM: usize = 4;
/// The bytes before the hub URL.
const FIXED: usize = 1 + 32 + 32 + 16;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The hub's token for one invitation: valid 5 minutes, once. It lets a device ask to join, so it
/// is not printed by `Debug`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Token(pub [u8; 16]);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

impl Serialize for Token {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Token {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Token, D::Error> {
        from_hex_text(deserializer, "a token").map(Token)
    }
}

/// What the inviting member shows: where the hub is and which key it has, the token, and who
/// invites.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitation {
    pub hub_url: String,
    pub hub_key: PublicKey,
    pub token: Token,
    pub inviter: PublicKey,
}

impl Invitation {
    /// The code to copy, and the QR's text.
    pub fn code(&self) -> String {
        let mut bytes = vec![VERSION];
        bytes.extend_from_slice(self.hub_key.as_bytes());
        bytes.extend_from_slice(self.inviter.as_bytes());
        bytes.extend_from_slice(&self.token.0);
        bytes.extend_from_slice(self.hub_url.as_bytes());
        let checksum = checksum(&bytes);
        bytes.extend_from_slice(&checksum);
        encode(&bytes)
    }

    pub fn from_code(code: &str) -> Result<Invitation, InvitationError> {
        let bytes = decode(code).ok_or(InvitationError::Damaged)?;
        if bytes.len() <= FIXED + CHECKSUM {
            return Err(InvitationError::Damaged);
        }
        let (payload, sum) = bytes.split_at(bytes.len() - CHECKSUM);
        if checksum(payload) != sum {
            return Err(InvitationError::Damaged);
        }
        if payload[0] != VERSION {
            return Err(InvitationError::OtherVersion);
        }
        let key = |at: usize| {
            let bytes: [u8; 32] = payload[at..at + 32].try_into().expect("32 bytes");
            PublicKey::from_bytes(&bytes).ok_or(InvitationError::Damaged)
        };
        let hub_url = std::str::from_utf8(&payload[FIXED..])
            .ok()
            .filter(|url| !url.chars().any(|c| c.is_whitespace() || c.is_control()))
            .ok_or(InvitationError::Damaged)?;
        Ok(Invitation {
            hub_url: hub_url.to_owned(),
            hub_key: key(1)?,
            token: Token(payload[65..FIXED].try_into().expect("16 bytes")),
            inviter: key(33)?,
        })
    }

    /// The four words the new device shows, its key being `new_device`.
    pub fn words(&self, new_device: &PublicKey) -> [&'static str; 4] {
        verification_words(new_device, &self.inviter, &self.token)
    }

    /// The new device's check of the first list the hub sends it, `own` being its key: the list
    /// checks back to its own root, which the device pins from then on, and the device's entry is
    /// there, signed by the member that invited it. A hub cannot make such a list: the inviter
    /// signed the entry after the real list's last change.
    pub fn check_list(&self, list: &MemberList, own: &PublicKey) -> Result<Members, ListError> {
        let root = list
            .root()
            .ok_or_else(|| ListError::Refused("it is empty".to_owned()))?;
        let members = list.check(&root)?;
        let Some(entry) = members.get(own) else {
            return Err(ListError::Refused("this device is not in it".to_owned()));
        };
        if entry.added_by != self.inviter {
            return Err(ListError::Refused(format!(
                "this device's entry is not signed by the member that invited it ({})",
                self.inviter.fingerprint()
            )));
        }
        Ok(members)
    }
}

/// Why an invitation code is refused.
#[derive(Debug, PartialEq, Eq)]
pub enum InvitationError {
    Damaged,
    OtherVersion,
}

impl fmt::Display for InvitationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InvitationError::Damaged => "this invitation code is damaged: copy it again, whole",
            InvitationError::OtherVersion => {
                "this invitation code comes from another version of Flottille: update the older \
                 device (flottille update)"
            }
        })
    }
}

impl std::error::Error for InvitationError {}

fn checksum(payload: &[u8]) -> [u8; CHECKSUM] {
    let mut hasher = Sha256::new();
    hasher.update(CHECKSUM_DOMAIN);
    hasher.update(payload);
    let mut sum = [0u8; CHECKSUM];
    sum.copy_from_slice(&hasher.finalize()[..CHECKSUM]);
    sum
}

/// Crockford's base32 of `bytes`, five bits a character, the last one padded with zeros.
fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 8 / 5 + 1);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The bytes of a code, `None` when it holds another character or is not one `encode` makes:
/// padding of five bits or more, or padding that is not zeros.
fn decode(code: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(code.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in code.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let value = match c.to_ascii_uppercase() {
            'O' => 0,
            'I' | 'L' => 1,
            upper => ALPHABET
                .iter()
                .position(|&letter| letter as char == upper)? as u32,
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
        buffer &= (1 << bits) - 1;
    }
    (bits < 5 && buffer == 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_of_another_version_says_so() {
        let mut bytes = vec![2u8];
        bytes.extend_from_slice(&[7; FIXED - 1]);
        bytes.extend_from_slice(b"https://hub.example.net");
        let sum = checksum(&bytes);
        bytes.extend_from_slice(&sum);

        let error = Invitation::from_code(&encode(&bytes)).unwrap_err();
        assert_eq!(error, InvitationError::OtherVersion);
        assert_eq!(
            error.to_string(),
            "this invitation code comes from another version of Flottille: update the older \
             device (flottille update)"
        );
    }

    #[test]
    fn base32_round_trips_every_length() {
        let bytes: Vec<u8> = (0..=255).collect();
        for length in 0..40 {
            let code = encode(&bytes[..length]);
            assert_eq!(code.len(), (length * 8).div_ceil(5));
            assert_eq!(decode(&code).unwrap(), &bytes[..length]);
        }
        assert_eq!(encode(b"f"), "CR");
        assert_eq!(encode(&[0xff; 5]), "ZZZZZZZZ");
    }
}
