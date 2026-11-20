//! Pairing (`docs/remote.md`, "Pairing"): the pair code `flottille-hub pair` prints, which holds
//! the hub's address, its key and the pairing's secret.

use flottille_link::{PairCode, PairCodeError, PairSecret, PublicKey};

/// RFC 8032's second test key, as a hub's.
fn hub_key() -> PublicKey {
    let hex = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";
    let bytes: Vec<u8> = (0..32)
        .map(|at| u8::from_str_radix(&hex[2 * at..2 * at + 2], 16).unwrap())
        .collect();
    PublicKey::from_bytes(&bytes.try_into().unwrap()).unwrap()
}

fn code() -> PairCode {
    PairCode {
        hub_url: "https://hub.example.net".to_owned(),
        hub_key: hub_key(),
        secret: PairSecret(std::array::from_fn(|at| at as u8)),
    }
}

const DAMAGED: &str = "this is not a pair code, or it is damaged: copy it again, whole, from what \
                       flottille-hub pair printed";
const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[test]
fn a_pair_code_round_trips_and_a_damaged_one_is_refused() {
    let sent = code();
    let text = sent.code().unwrap();
    assert!(text.chars().all(|c| ALPHABET.contains(c)), "{text}");
    assert_eq!(PairCode::from_code(&text).unwrap(), sent);

    // Copied as a person copies it: in small letters, cut in groups or lines, with the letters
    // that look like digits.
    assert_eq!(PairCode::from_code(&text.to_lowercase()).unwrap(), sent);
    let grouped: Vec<String> = text
        .as_bytes()
        .chunks(4)
        .map(|group| String::from_utf8(group.to_vec()).unwrap())
        .collect();
    assert_eq!(PairCode::from_code(&grouped.join("-")).unwrap(), sent);
    assert_eq!(
        PairCode::from_code(&format!("  {}\n", grouped.join("\n"))).unwrap(),
        sent
    );
    let alike = text.replace('0', "O").replace('1', "l");
    assert_eq!(PairCode::from_code(&alike).unwrap(), sent);

    // Every character changed into every other one.
    for (at, was) in text.char_indices() {
        for other in ALPHABET.chars().filter(|&other| other != was) {
            let mut damaged = text.clone();
            damaged.replace_range(at..at + 1, &other.to_string());
            let error = PairCode::from_code(&damaged).unwrap_err();
            assert_eq!(error.to_string(), DAMAGED, "{at}: {was} → {other}");
        }
    }

    for damaged in [
        &text[..text.len() - 1],
        &text[1..],
        &format!("{text}0"),
        &format!("{}U{}", &text[..10], &text[11..]),
        "",
        "flottille",
        // The setup code of a hub of before pair mode is no pair code.
        "7KQ2-M4XP-9TRW-H3CD",
    ] {
        let error = PairCode::from_code(damaged).unwrap_err();
        assert_eq!(error.to_string(), DAMAGED, "{damaged}");
    }
}

#[test]
fn a_hub_url_that_is_not_one_is_refused() {
    for url in [
        "",
        "hub.example.net",
        "ftp://hub.example.net",
        "https://",
        "https:///path",
        "https://hub.example.net/a b",
        "https://hub.example.net\n",
        "https://hub\u{200B}.example.net",
        "https://\u{202E}ten.elpmaxe.buh",
        &format!("https://hub.example.net/{}", "a".repeat(2048)),
    ] {
        let mut sent = code();
        sent.hub_url = url.to_owned();
        let error = sent.code().unwrap_err();
        assert_eq!(error, PairCodeError::HubUrl, "{url:?}");
    }
    for url in ["http://192.0.2.7:7448", "https://hub.example.net/flottille"] {
        let mut sent = code();
        sent.hub_url = url.to_owned();
        assert_eq!(PairCode::from_code(&sent.code().unwrap()).unwrap(), sent);
    }
}
