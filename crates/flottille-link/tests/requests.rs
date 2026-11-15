//! A request for another machine (`docs/remote.md`, "Requests from another machine's clients",
//! "Roles"): signed by a member's device key, checked by the receiving machine.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flottille_link::{
    Action, DeviceKey, Keyring, KeyringError, MemberList, Members, NONCES_PER_DEVICE, Request,
    Requests, Role, Signature,
};
use flottille_protocol::{Class, METHODS};
use serde_json::{Value, json};
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

/// The device key number `n`; the folder is never reached, the keyring holding the key.
fn key(n: u8) -> DeviceKey {
    DeviceKey::load_or_make(&Seeded([n; 32]), Path::new("/nonexistent")).unwrap()
}

/// The root `pc`, a runner; a controller `pixel`; a viewer `tablet`.
fn three() -> (DeviceKey, DeviceKey, DeviceKey, MemberList) {
    let (pc, pixel, tablet) = (key(1), key(2), key(3));
    let mut list = MemberList::start("pc", &pc);
    for (device, name, role) in [
        (&pixel, "pixel", Role::Controller),
        (&tablet, "tablet", Role::Viewer),
    ] {
        let entry = Action::Add {
            key: device.public(),
            name: name.to_owned(),
            role,
        };
        list.sign(entry, &pc);
    }
    (pc, pixel, tablet, list)
}

fn members(list: &MemberList) -> Members {
    list.check(&key(1).public()).unwrap()
}

/// A fixed moment, the time of the tests.
fn time(milliseconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_800_000_000_000 + milliseconds)
}

const MINUTE: u64 = 60_000;

/// `method` with `params`, for the machine `pc`, signed by `key` at `at`, as the link carries it.
fn signed(key: &DeviceKey, method: &str, params: Value, at: u64) -> String {
    let request = Request::sign(key, "7", "pc", method, &params, time(at)).unwrap();
    serde_json::to_string(&request).unwrap()
}

/// The sentence a request is refused with, on the machine `pc`, at `now`.
fn refused(requests: &mut Requests, text: &str, members: &Members, now: u64) -> String {
    match requests.check(text, "pc", members, time(now)) {
        Ok(accepted) => panic!("accepted: {accepted:?}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_signed_request_of_a_member_is_accepted() {
    let (pc, pixel, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new();
    let params = json!({"session": "s1", "text": "go on"});
    let text = signed(&pixel, "agent.send", params.clone(), 0);

    let accepted = requests.check(&text, "pc", &members, time(1_000)).unwrap();
    assert_eq!(accepted.device.name, "pixel");
    assert_eq!(accepted.device.key, pixel.public());
    assert_eq!(accepted.id, "7");
    assert_eq!(accepted.method, "agent.send");
    assert_eq!(accepted.params, params);

    // The runner's own clients, the root's key, too.
    let text = signed(&pc, "agent.send", params, 0);
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());

    // What is signed starts with its own domain: never taken for a member change.
    let request: Request = serde_json::from_str(&text).unwrap();
    assert!(
        request
            .signed_bytes()
            .starts_with(b"flottille-request-v1\0")
    );
}

#[test]
fn an_unsigned_or_badly_signed_request_is_refused() {
    let (_, pixel, tablet, list) = three();
    let members = members(&list);
    let mut requests = Requests::new();
    let text = signed(&pixel, "agent.send", json!({"text": "go on"}), 0);
    let request: Request = serde_json::from_str(&text).unwrap();

    // No signature at all.
    let mut unsigned: Value = serde_json::from_str(&text).unwrap();
    unsigned.as_object_mut().unwrap().remove("sig");
    let sentence = refused(&mut requests, &unsigned.to_string(), &members, 0);
    assert!(
        sentence.starts_with("refused: this is not a signed request"),
        "{sentence}"
    );

    let bad = |request: Request| serde_json::to_string(&request).unwrap();
    let not_pixels = "refused: this request does not carry the signature of pixel";

    // A signature of zeros.
    let mut zeros = request.clone();
    zeros.sig = Signature([0; 64]);
    assert_eq!(refused(&mut requests, &bad(zeros), &members, 0), not_pixels);

    // Each signed field changed after the signature.
    let mut params = request.clone();
    params.signed.params = r#"{"text":"stop"}"#.to_owned();
    assert_eq!(
        refused(&mut requests, &bad(params), &members, 0),
        not_pixels
    );
    let mut method = request.clone();
    method.signed.method = "agent.stop".to_owned();
    assert_eq!(
        refused(&mut requests, &bad(method), &members, 0),
        not_pixels
    );
    let mut at = request.clone();
    at.signed.at += 1;
    assert_eq!(refused(&mut requests, &bad(at), &members, 0), not_pixels);
    let mut nonce = request.clone();
    nonce.signed.nonce.0[0] ^= 1;
    assert_eq!(refused(&mut requests, &bad(nonce), &members, 0), not_pixels);
    let mut id = request.clone();
    id.id = "8".to_owned();
    assert_eq!(refused(&mut requests, &bad(id), &members, 0), not_pixels);

    // Signed by the viewer, claimed as pixel's.
    let tablets: Request =
        serde_json::from_str(&signed(&tablet, "agent.send", json!({"text": "go on"}), 0)).unwrap();
    let mut claimed = tablets;
    claimed.signed.device = pixel.public();
    assert_eq!(
        refused(&mut requests, &bad(claimed), &members, 0),
        not_pixels
    );

    // For another machine: signed for it, so its name cannot be changed either.
    assert_eq!(
        requests
            .check(&text, "laptop", &members, time(0))
            .unwrap_err()
            .to_string(),
        "refused: this request is for pc, not for laptop"
    );
    let mut elsewhere = request.clone();
    elsewhere.machine = "laptop".to_owned();
    assert_eq!(
        requests
            .check(&bad(elsewhere), "laptop", &members, time(0))
            .unwrap_err()
            .to_string(),
        not_pixels
    );

    // A signature whose `s` is not reduced (raised by the group's order): the same signature
    // written another way, which the strict check refuses.
    let mut malleable = request.clone();
    let order: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde,
        0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    let mut carry = 0u16;
    for (byte, add) in malleable.sig.0[32..].iter_mut().zip(order) {
        let sum = *byte as u16 + add as u16 + carry;
        *byte = sum as u8;
        carry = sum >> 8;
    }
    assert_eq!(
        refused(&mut requests, &bad(malleable), &members, 0),
        not_pixels
    );

    // The untouched request still passes: none of the refusals took its nonce.
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());
}

#[test]
fn a_request_of_a_key_not_in_the_list_or_removed_is_refused() {
    let (pc, _, tablet, mut list) = three();
    let mut requests = Requests::new();
    let stranger = key(9);
    let text = signed(&stranger, "agent.list", Value::Null, 0);
    assert_eq!(
        refused(&mut requests, &text, &members(&list), 0),
        format!(
            "refused: the device {} is not a member",
            stranger.public().fingerprint()
        )
    );

    let text = signed(&tablet, "agent.list", Value::Null, 0);
    assert!(
        requests
            .check(&text, "pc", &members(&list), time(0))
            .is_ok()
    );
    list.sign(
        Action::Remove {
            key: tablet.public(),
        },
        &pc,
    );
    let text = signed(&tablet, "agent.list", Value::Null, 0);
    assert_eq!(
        refused(&mut requests, &text, &members(&list), 0),
        format!(
            "refused: the device {} is not a member",
            tablet.public().fingerprint()
        )
    );
}

#[test]
fn a_request_older_than_five_minutes_is_refused() {
    let (_, pixel, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new();
    let at = 10 * MINUTE;

    let old = signed(&pixel, "agent.list", Value::Null, at);
    assert_eq!(
        refused(&mut requests, &old, &members, at + 5 * MINUTE + 1),
        "refused: this request is older than five minutes"
    );
    let ahead = signed(&pixel, "agent.list", Value::Null, at + 5 * MINUTE + 1);
    assert_eq!(
        refused(&mut requests, &ahead, &members, at),
        "refused: this request is dated more than five minutes ahead of this machine's clock"
    );

    // Five minutes either way are within the window; on a memory whose clock was not moved past
    // it already.
    let mut requests = Requests::new();
    let late = signed(&pixel, "agent.list", Value::Null, at);
    assert!(
        requests
            .check(&late, "pc", &members, time(at + 5 * MINUTE))
            .is_ok()
    );
    let early = signed(&pixel, "agent.list", Value::Null, at + 10 * MINUTE);
    assert!(
        requests
            .check(&early, "pc", &members, time(at + 5 * MINUTE))
            .is_ok()
    );
}

#[test]
fn a_replayed_nonce_is_refused() {
    let (_, pixel, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new();
    let text = signed(&pixel, "agent.send", json!({"text": "go on"}), 0);
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());

    let replayed = "refused: this request was received already";
    assert_eq!(refused(&mut requests, &text, &members, 0), replayed);
    assert_eq!(
        refused(&mut requests, &text, &members, 5 * MINUTE),
        replayed
    );
    assert_eq!(
        refused(&mut requests, &text, &members, 5 * MINUTE + 1),
        "refused: this request is older than five minutes"
    );

    // A clock set back does not bring it back: the memory forgot it once it was out of the window.
    assert_eq!(
        refused(&mut requests, &text, &members, MINUTE),
        "refused: this request is older than five minutes"
    );
}

#[test]
fn the_nonces_of_a_device_are_bounded_and_forgotten_after_the_window() {
    let (_, pixel, tablet, list) = three();
    let members = members(&list);
    let mut requests = Requests::new();
    for _ in 0..NONCES_PER_DEVICE {
        let text = signed(&pixel, "agent.list", Value::Null, 0);
        requests.check(&text, "pc", &members, time(0)).unwrap();
    }
    let text = signed(&pixel, "agent.list", Value::Null, 0);
    assert_eq!(
        refused(&mut requests, &text, &members, 0),
        format!("refused: pixel sent more than {NONCES_PER_DEVICE} requests in five minutes")
    );

    // Another device is not held up by it.
    let text = signed(&tablet, "agent.list", Value::Null, 0);
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());

    // Once those are out of the window, the device is heard again.
    let at = 5 * MINUTE + 1;
    let text = signed(&pixel, "agent.list", Value::Null, at);
    assert!(requests.check(&text, "pc", &members, time(at)).is_ok());
}

#[test]
fn a_viewer_may_read_but_not_drive_and_the_refusal_says_which_role_may() {
    let (pc, pixel, tablet, list) = three();
    let members = members(&list);
    let mut requests = Requests::new();

    let text = signed(&tablet, "agent.list", Value::Null, 0);
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());
    let text = signed(&tablet, "approval.approve", json!({"request": 3}), 0);
    assert_eq!(
        refused(&mut requests, &text, &members, 0),
        "refused: tablet may only view: a controller or a runner may call approval.approve"
    );

    for driver in [&pc, &pixel] {
        let text = signed(driver, "approval.approve", json!({"request": 3}), 0);
        assert!(requests.check(&text, "pc", &members, time(0)).is_ok());
    }

    // A method the daemon does not answer has no class: refused, whoever asks.
    let text = signed(&pc, "agent.nope", Value::Null, 0);
    assert_eq!(
        refused(&mut requests, &text, &members, 0),
        "refused: agent.nope is not a method of Flottille's"
    );

    // The role is checked before the signature: a viewer's request to drive is refused for its
    // role, signed or not.
    let mut forged: Request =
        serde_json::from_str(&signed(&tablet, "agent.send", Value::Null, 0)).unwrap();
    forged.sig = Signature([0; 64]);
    assert_eq!(
        refused(
            &mut requests,
            &serde_json::to_string(&forged).unwrap(),
            &members,
            0
        ),
        "refused: tablet may only view: a controller or a runner may call agent.send"
    );
}

#[test]
fn every_method_of_the_catalogue_has_a_class() {
    let (_, pixel, tablet, list) = three();
    let members = members(&list);
    let mut requests = Requests::new();
    for (method, class) in METHODS {
        let text = signed(&pixel, method, Value::Null, 0);
        assert!(
            requests.check(&text, "pc", &members, time(0)).is_ok(),
            "a controller may call {method}"
        );
        let text = signed(&tablet, method, Value::Null, 0);
        let viewed = requests.check(&text, "pc", &members, time(0));
        match class {
            Class::Read => assert!(viewed.is_ok(), "a viewer may call {method}"),
            Class::Drive => assert_eq!(
                viewed.unwrap_err().to_string(),
                format!(
                    "refused: tablet may only view: a controller or a runner may call {method}"
                )
            ),
        }
    }
}
