//! The four verification words of a pairing (`docs/remote.md`, "Pairing"): both screens draw them
//! from the new device's key, the inviting member's key and the invitation's token, so that the user
//! sees the two devices talk to each other and not to a third one between them.
//!
//! The words are the SHA-256 of `"flottille-pairing-words-v1" 0x00`, the new device's key (32
//! bytes), the inviting member's (32) and the token (16); its first 44 bits, read big-endian, are
//! four numbers of 11 bits, each the place of a word in a fixed list of 2048.
//!
//! The list is BIP-39's English one (`words.txt`, from the bitcoin/bips repository,
//! `bip-0039/english.txt`), under the MIT licence (`words.txt.LICENSE`): short, common words, no
//! two of which start with the same four letters.

use sha2::{Digest, Sha256};

use crate::{PublicKey, Token};

const WORDS: &str = include_str!("words.txt");
const DOMAIN: &[u8] = b"flottille-pairing-words-v1\0";

/// The four words both screens show for this pairing.
pub fn verification_words(
    new_device: &PublicKey,
    inviter: &PublicKey,
    token: &Token,
) -> [&'static str; 4] {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(new_device.as_bytes());
    hasher.update(inviter.as_bytes());
    hasher.update(token.0);
    let digest = hasher.finalize();
    let mut first = [0u8; 8];
    first[2..].copy_from_slice(&digest[..6]);
    let bits = u64::from_be_bytes(first) >> 4;
    std::array::from_fn(|at| word(((bits >> (33 - 11 * at)) & 0x7ff) as usize))
}

fn word(place: usize) -> &'static str {
    WORDS.lines().nth(place).expect("the list has 2048 words")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_bip_39_s_english_one() {
        let words: Vec<&str> = WORDS.lines().collect();
        assert_eq!(words.len(), 2048);
        assert!(words.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            words
                .iter()
                .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
        );
        let digest = Sha256::digest(WORDS.as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda"
        );
        assert_eq!((word(0), word(2047)), ("abandon", "zoo"));
    }
}
