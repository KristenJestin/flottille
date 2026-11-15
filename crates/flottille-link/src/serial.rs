//! A key as bytes and as text: what the member list and the invitation carry. A key travels in
//! hexadecimal, and one that is not a point of the curve is refused when it is read.

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::PublicKey;

impl PublicKey {
    /// The key these bytes are, when they are one.
    pub fn from_bytes(bytes: &[u8; 32]) -> Option<PublicKey> {
        VerifyingKey::from_bytes(bytes).ok().map(PublicKey)
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(self.as_bytes()))
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<PublicKey, D::Error> {
        let bytes = from_hex_text::<_, 32>(deserializer, "a key")?;
        PublicKey::from_bytes(&bytes)
            .ok_or_else(|| serde::de::Error::custom("not a key of Flottille's"))
    }
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The `N` bytes written in `text` in hexadecimal, when it is exactly that.
pub(crate) fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N * 2 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; N];
    for (at, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[at * 2..at * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

/// The `N` bytes of a hexadecimal string, as serde reads it; `what` names it in the error.
pub(crate) fn from_hex_text<'de, D: Deserializer<'de>, const N: usize>(
    deserializer: D,
    what: &str,
) -> Result<[u8; N], D::Error> {
    let text = String::deserialize(deserializer)?;
    from_hex(&text)
        .ok_or_else(|| serde::de::Error::custom(format!("{what} is {N} bytes in hexadecimal")))
}
