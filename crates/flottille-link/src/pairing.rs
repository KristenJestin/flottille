//! The pair code (`docs/remote.md`, "Pairing"): what `flottille-hub pair` prints, and its QR, for
//! one device to pair with the hub. It holds everything the device needs: where the hub is, its key,
//! which the device pins, and the secret the device proves it knows, never sends.
//!
//! The code, which is also the QR's text, is in Crockford's base32 (digits and capital letters,
//! which a QR holds in its compact alphanumeric mode; read in any case, `O` as `0`, `I` and `L` as
//! `1`, hyphens and spaces skipped), of these bytes:
//!
//! ```text
//! version     1 byte      1
//! hub key    32 bytes     the hub's public key, which the device pins
//! secret     16 bytes     random, made for this pairing: valid a few minutes, once
//! hub URL    the rest     UTF-8
//! checksum    4 bytes     the first 4 bytes of the SHA-256 of "flottille-pair-v1" 0x00 and every
//!                         byte above
//! ```
//!
//! A code that does not decode, or whose checksum is not its own, is refused as damaged.
//!
//! The device proves it holds the secret in its session's handshake (`control.rs`): an
//! HMAC-SHA256 keyed by the secret, over the session's keys and nonces and the card it proposes,
//! so that the proof is good for that session and that card only.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::serial::invisible;
use crate::{Colour, Icon, PublicKey, Role, check_name, random};

const VERSION: u8 = 1;
const CHECKSUM_DOMAIN: &[u8] = b"flottille-pair-v1\0";
const CHECKSUM: usize = 4;
/// The bytes before the hub URL.
const FIXED: usize = 1 + 32 + 16;
/// The longest hub URL, in bytes.
const URL_LIMIT: usize = 2048;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The secret of one pairing. It lets a device pair, so `Debug` does not print it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PairSecret(pub [u8; 16]);

impl PairSecret {
    /// A new secret, from the system's random generator.
    pub fn new() -> Result<PairSecret, String> {
        random().map(PairSecret).map_err(|error| error.to_string())
    }
}

impl fmt::Debug for PairSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairSecret(..)")
    }
}

/// What a pair code holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairCode {
    pub hub_url: String,
    pub hub_key: PublicKey,
    pub secret: PairSecret,
}

impl PairCode {
    /// The code to copy, and the QR's text; refused when the hub URL is not one (`is_hub_url`).
    pub fn code(&self) -> Result<String, PairCodeError> {
        if !is_hub_url(&self.hub_url) {
            return Err(PairCodeError::HubUrl);
        }
        let mut bytes = vec![VERSION];
        bytes.extend_from_slice(self.hub_key.as_bytes());
        bytes.extend_from_slice(&self.secret.0);
        bytes.extend_from_slice(self.hub_url.as_bytes());
        let checksum = checksum(&bytes);
        bytes.extend_from_slice(&checksum);
        Ok(encode(&bytes))
    }

    pub fn from_code(code: &str) -> Result<PairCode, PairCodeError> {
        let bytes = decode(code).ok_or(PairCodeError::Damaged)?;
        if bytes.len() <= FIXED + CHECKSUM {
            return Err(PairCodeError::Damaged);
        }
        let (payload, sum) = bytes.split_at(bytes.len() - CHECKSUM);
        if checksum(payload) != sum {
            return Err(PairCodeError::Damaged);
        }
        if payload[0] != VERSION {
            return Err(PairCodeError::OtherVersion);
        }
        let key: [u8; 32] = payload[1..33].try_into().expect("32 bytes");
        let hub_key = PublicKey::from_bytes(&key).map_err(|_| PairCodeError::BadKey)?;
        let hub_url = std::str::from_utf8(&payload[FIXED..])
            .ok()
            .ok_or(PairCodeError::Damaged)?;
        if !is_hub_url(hub_url) {
            return Err(PairCodeError::HubUrl);
        }
        Ok(PairCode {
            hub_url: hub_url.to_owned(),
            hub_key,
            secret: PairSecret(payload[33..FIXED].try_into().expect("16 bytes")),
        })
    }
}

/// Why a pair code is refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PairCodeError {
    Damaged,
    OtherVersion,
    /// The hub's key in it is not one, or is a weak one: a code made by hand, not a hub's.
    BadKey,
    HubUrl,
}

impl fmt::Display for PairCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PairCodeError::Damaged => {
                "this is not a pair code, or it is damaged: copy it again, whole, from what \
                 flottille-hub pair printed"
            }
            PairCodeError::OtherVersion => {
                "this pair code comes from another version of Flottille: update the older side \
                 (flottille update)"
            }
            PairCodeError::BadKey => {
                "this pair code holds a key no hub makes: run flottille-hub pair again"
            }
            PairCodeError::HubUrl => {
                "the hub URL of this pair code is not an http:// or https:// address of at most \
                 2048 characters, with no space and no invisible character"
            }
        })
    }
}

impl std::error::Error for PairCodeError {}

/// What a device proposes for itself when it pairs, and the user confirmed: its key, its name, its
/// role, its icon and colour. The hub signs it as it is (`Action::Pair`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposed {
    pub key: PublicKey,
    pub name: String,
    pub role: Role,
    pub icon: Icon,
    pub colour: Colour,
}

impl Proposed {
    /// Whether the name may name a device; why not otherwise.
    pub fn check(&self) -> Result<(), String> {
        check_name(&self.name).map_err(|why| format!("this device {why}"))
    }

    /// The bytes a pairing's proof covers of it: its key, its name's length (4 bytes, big-endian)
    /// and its name, its role, icon and colour (1 byte each, as a `pair` change says them).
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let mut bytes = self.key.as_bytes().to_vec();
        bytes.extend_from_slice(&(self.name.len() as u32).to_be_bytes());
        bytes.extend_from_slice(self.name.as_bytes());
        bytes.extend_from_slice(&[self.role.byte(), self.icon.byte(), self.colour.byte()]);
        bytes
    }
}

/// Whether `url` is a hub URL: `http://` or `https://`, a host, at most `URL_LIMIT` bytes, and
/// no space, control or invisible character, which would make the address a person checks differ
/// from the one the device reaches.
pub(crate) fn is_hub_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    url.len() <= URL_LIMIT
        && !authority.is_empty()
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || invisible(c))
}

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

    /// RFC 8032's second test key.
    fn hub_key() -> PublicKey {
        PublicKey::from_bytes(
            &crate::serial::from_hex::<32>(
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_pair_code_round_trips_and_a_changed_one_is_damaged() {
        let code = PairCode {
            hub_url: "https://hub.example.net".to_owned(),
            hub_key: hub_key(),
            secret: PairSecret([9; 16]),
        };
        let text = code.code().unwrap();
        assert!(
            text.chars().all(|c| ALPHABET.contains(&(c as u8))),
            "{text}"
        );
        assert_eq!(PairCode::from_code(&text.to_lowercase()).unwrap(), code);
        let mut changed = text.into_bytes();
        changed[10] = if changed[10] == b'A' { b'B' } else { b'A' };
        let error = PairCode::from_code(&String::from_utf8(changed).unwrap()).unwrap_err();
        assert_eq!(error, PairCodeError::Damaged);
        // The secret is never printed.
        assert!(!format!("{code:?}").contains("9, 9"));
    }

    #[test]
    fn a_code_of_another_version_says_so() {
        let mut bytes = vec![2u8];
        bytes.extend_from_slice(&[7; FIXED - 1]);
        bytes.extend_from_slice(b"https://hub.example.net");
        let sum = checksum(&bytes);
        bytes.extend_from_slice(&sum);

        let error = PairCode::from_code(&encode(&bytes)).unwrap_err();
        assert_eq!(error, PairCodeError::OtherVersion);
    }

    #[test]
    fn a_weak_key_in_a_code_is_refused() {
        let mut identity = [0u8; 32];
        identity[0] = 1;
        let mut bytes = vec![VERSION];
        bytes.extend_from_slice(&identity);
        bytes.extend_from_slice(&[7; 16]);
        bytes.extend_from_slice(b"https://hub.example.net");
        let sum = checksum(&bytes);
        bytes.extend_from_slice(&sum);

        let error = PairCode::from_code(&encode(&bytes)).unwrap_err();
        assert_eq!(error, PairCodeError::BadKey);
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
