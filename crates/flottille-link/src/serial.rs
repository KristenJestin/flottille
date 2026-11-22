//! A key as bytes and as text: what the member list and the pair code carry. A key travels in
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

/// `text` as Flottille shows what another machine or the hub said: its first `limit` characters,
/// a control or format character as `\u{fffd}`, so that it can neither forge lines of a log nor
/// drive a terminal.
pub(crate) fn shown(text: &str, limit: usize) -> String {
    let mut shown: String = text
        .chars()
        .take(limit)
        .map(|c| {
            if c.is_control() || invisible(c) {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect();
    if text.chars().nth(limit).is_some() {
        shown.push('…');
    }
    shown
}

/// Whether `c` hides or turns text a reader sees: a format character (`invisible`), a line or
/// paragraph separator (U+2028, U+2029), which no control check catches, or one of the characters
/// that are no format character yet show nothing: variation selectors, Hangul fillers, the
/// combining grapheme joiner and the blank braille pattern. The text and emoji presentation
/// selectors (U+FE0E, U+FE0F) are left out: they are part of the emoji people write (⚠️ ❤️).
pub fn hidden(c: char) -> bool {
    const BLANK: &[(u32, u32)] = &[
        (0x034F, 0x034F),
        (0x115F, 0x1160),
        (0x2028, 0x2029),
        (0x2800, 0x2800),
        (0x3164, 0x3164),
        (0xFE00, 0xFE0D),
        (0xFFA0, 0xFFA0),
        (0xE0100, 0xE01EF),
    ];
    let code = u32::from(c);
    invisible(c)
        || BLANK
            .iter()
            .any(|&(first, last)| (first..=last).contains(&code))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_that_shows_nothing_is_hidden_format_character_or_not() {
        for c in [
            '\u{202E}',
            '\u{2028}', // format, separator
            '\u{FE00}',
            '\u{FE0D}',
            '\u{E0100}',
            '\u{E01EF}', // variation selectors
            '\u{115F}',
            '\u{1160}',
            '\u{3164}',
            '\u{FFA0}', // Hangul fillers
            '\u{034F}',
            '\u{2800}', // combining grapheme joiner, blank braille
        ] {
            assert!(hidden(c), "U+{:04X} shows something", u32::from(c));
        }
        for c in [
            '\u{FE0E}',
            '\u{FE0F}', // the text and emoji presentation selectors: ⚠️ ❤️
            'a',
            ' ',
            'é',
            '\u{FDFF}',
            '\u{E01F0}',
            '\u{2801}',
            '한',
        ] {
            assert!(!hidden(c), "U+{:04X} was hidden", u32::from(c));
        }
    }
}
