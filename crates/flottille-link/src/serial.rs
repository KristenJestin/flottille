//! A key as bytes and as text: what the member list and the invitation carry. A key travels in
//! hexadecimal, and one that is not a point of the curve, or a weak one, is refused when it is
//! read. Also the text a person reads in them: the characters that show nothing.

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{KeyError, PublicKey};

impl PublicKey {
    /// The key these bytes are, when they are one. A point of small order is refused: no device
    /// makes one, any signature checks against some of them, and a transport that turns the key
    /// into an X25519 one would agree on a secret everyone knows.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<PublicKey, KeyError> {
        let key = VerifyingKey::from_bytes(bytes)
            .map_err(|_| KeyError("these bytes are not a key".to_owned()))?;
        if key.is_weak() {
            return Err(KeyError(
                "this is a weak key (a point of small order), which no device makes".to_owned(),
            ));
        }
        Ok(PublicKey(key))
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
        PublicKey::from_bytes(&bytes).map_err(serde::de::Error::custom)
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

/// Whether `c` is a format character (Unicode's category `Cf`, as of Unicode 15.1): it shows
/// nothing, or turns the text around it (zero-width spaces and joiners, bidirectional overrides
/// and isolates, the byte order mark, tags), so that two texts that look the same are not.
pub(crate) fn invisible(c: char) -> bool {
    const FORMAT: &[(u32, u32)] = &[
        (0x00AD, 0x00AD),
        (0x0600, 0x0605),
        (0x061C, 0x061C),
        (0x06DD, 0x06DD),
        (0x070F, 0x070F),
        (0x0890, 0x0891),
        (0x08E2, 0x08E2),
        (0x180E, 0x180E),
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x2064),
        (0x2066, 0x206F),
        (0xFEFF, 0xFEFF),
        (0xFFF9, 0xFFFB),
        (0x110BD, 0x110BD),
        (0x110CD, 0x110CD),
        (0x13430, 0x1343F),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0001, 0xE0001),
        (0xE0020, 0xE007F),
    ];
    let c = u32::from(c);
    FORMAT
        .iter()
        .any(|&(first, last)| (first..=last).contains(&c))
}
