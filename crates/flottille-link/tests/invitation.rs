//! Pairing (`docs/remote.md`, "Pairing"): the invitation a member shows, the four words both
//! screens show, and the new device's check of the list it is sent.

use std::path::Path;

use flottille_link::{
    Action, DeviceKey, Invitation, Keyring, KeyringError, MemberList, Role, Token,
    verification_words,
};
use zeroize::Zeroizing;

/// A keyring that holds one device key, made from a fixed seed: the same key at every run.
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

fn key_from(seed: [u8; 32]) -> DeviceKey {
    DeviceKey::load_or_make(&Seeded(seed), Path::new("/nonexistent")).unwrap()
}

fn key(n: u8) -> DeviceKey {
    key_from([n; 32])
}

fn bytes<const N: usize>(hex: &str) -> [u8; N] {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

fn token() -> Token {
    Token(std::array::from_fn(|at| at as u8))
}

fn invitation(inviter: &DeviceKey) -> Invitation {
    Invitation {
        hub_url: "https://hub.example.net:7443".to_owned(),
        hub_key: key(8).public(),
        token: token(),
        inviter: inviter.public(),
    }
}

const DAMAGED: &str = "this invitation code is damaged: copy it again, whole";
const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[test]
fn an_invitation_code_round_trips_and_a_damaged_one_is_refused() {
    let sent = invitation(&key(2));
    let code = sent.code();
    assert!(code.chars().all(|c| ALPHABET.contains(c)), "{code}");
    assert_eq!(Invitation::from_code(&code).unwrap(), sent);

    // Copied as a person copies it: in small letters, cut in groups or lines, with the letters
    // that look like digits.
    let lower = code.to_lowercase();
    assert_eq!(Invitation::from_code(&lower).unwrap(), sent);
    let grouped: Vec<String> = code
        .as_bytes()
        .chunks(4)
        .map(|group| String::from_utf8(group.to_vec()).unwrap())
        .collect();
    assert_eq!(Invitation::from_code(&grouped.join("-")).unwrap(), sent);
    assert_eq!(
        Invitation::from_code(&format!("  {}\n", grouped.join("\n"))).unwrap(),
        sent
    );
    let alike = code.replace('0', "O").replace('1', "l");
    assert_eq!(Invitation::from_code(&alike).unwrap(), sent);

    // Every character changed into every other one.
    for (at, was) in code.char_indices() {
        for other in ALPHABET.chars().filter(|&other| other != was) {
            let mut damaged = code.clone();
            damaged.replace_range(at..at + 1, &other.to_string());
            let error = Invitation::from_code(&damaged).unwrap_err();
            assert_eq!(error.to_string(), DAMAGED, "{at}: {was} → {other}");
        }
    }

    for damaged in [
        &code[..code.len() - 1],
        &code[1..],
        &format!("{code}0"),
        &format!("{}U{}", &code[..10], &code[11..]),
        "",
        "flottille",
    ] {
        let error = Invitation::from_code(damaged).unwrap_err();
        assert_eq!(error.to_string(), DAMAGED, "{damaged}");
    }
}

#[test]
fn both_sides_draw_the_same_four_words() {
    // The keys of RFC 8032's first two test vectors.
    let new_device = key_from(bytes(
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    ));
    let inviter = key_from(bytes(
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
    ));
    assert_eq!(
        new_device.public().as_bytes(),
        &bytes::<32>("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    );
    assert_eq!(
        inviter.public().as_bytes(),
        &bytes::<32>("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c")
    );

    // The inviting member: the join request's key, its own, the token the hub gave it.
    let on_the_member = verification_words(&new_device.public(), &inviter.public(), &token());
    // The new device: its own key and what it read in the code.
    let read = Invitation::from_code(&invitation(&inviter).code()).unwrap();
    let on_the_new_device = read.words(&new_device.public());

    assert_eq!(on_the_member, on_the_new_device);
    assert_eq!(on_the_member, ["guard", "crazy", "gravity", "nasty"]);

    let mut other_token = token();
    other_token.0[0] ^= 1;
    assert_ne!(
        verification_words(&new_device.public(), &inviter.public(), &other_token),
        on_the_member
    );
    assert_ne!(
        verification_words(&key(5).public(), &inviter.public(), &token()),
        on_the_member
    );
}

#[test]
fn an_entry_not_signed_by_the_invitation_s_fingerprint_is_refused_by_the_new_device() {
    let (pc, pixel, laptop) = (key(1), key(2), key(4));
    let mut list = MemberList::start("pc", &pc);
    list.sign(
        Action::Add {
            key: pixel.public(),
            name: "pixel".to_owned(),
            role: Role::Controller,
        },
        &pc,
    );
    // The phone invited the laptop.
    let read = Invitation::from_code(&invitation(&pixel).code()).unwrap();
    let laptop_entry = Action::Add {
        key: laptop.public(),
        name: "laptop".to_owned(),
        role: Role::Runner,
    };

    let error = read.check_list(&list, &laptop.public()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the hub's member list was refused: this device is not in it"
    );

    // Added by a member, but not the one that invited it.
    let mut by_another = list.clone();
    by_another.sign(laptop_entry.clone(), &pc);
    assert!(by_another.check(&pc.public()).is_ok());
    let error = read.check_list(&by_another, &laptop.public()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "the hub's member list was refused: this device's entry is not signed by the member \
             that invited it ({})",
            pixel.public().fingerprint()
        )
    );

    let mut by_the_inviter = list.clone();
    by_the_inviter.sign(laptop_entry, &pixel);
    let members = read.check_list(&by_the_inviter, &laptop.public()).unwrap();
    // The root the new device pins from now on.
    assert_eq!(members.root(), pc.public());
    assert_eq!(members.get(&laptop.public()).unwrap().role, Role::Runner);
}
