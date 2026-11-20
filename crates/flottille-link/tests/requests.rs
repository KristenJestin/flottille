//! A request for another machine (`docs/remote.md`, "Requests from another machine's clients",
//! "Roles"): signed by a member's device key, checked by the receiving machine.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flottille_link::{
    Action, DeviceKey, Keyring, KeyringError, MemberList, Members, NONCES_PER_DEVICE, Request,
    Requests, Role, Signature, full_switch, runs_agents,
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
    let mut list = MemberList::start("pc", &pc).unwrap();
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

/// The list's members, its hub's key that of no member: the list was made before pair mode.
fn members(list: &MemberList) -> Members {
    list.check(&key(1).public(), &key(9).public()).unwrap()
}

/// A fixed moment, the time of the tests.
fn time(milliseconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_800_000_000_000 + milliseconds)
}

const MINUTE: u64 = 60_000;

/// The one refusal before the signature passes: it names nothing of the list.
const NOT_SIGNED: &str = "refused: this request is not signed by a member";

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
    let mut requests = Requests::new(time(0));
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
            .unwrap()
            .starts_with(b"flottille-request-v1\0")
    );
}

#[test]
fn an_unsigned_or_badly_signed_request_is_refused() {
    let (_, pixel, tablet, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
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
    let not_pixels = NOT_SIGNED;

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
    let mut requests = Requests::new(time(0));
    let stranger = key(9);
    let text = signed(&stranger, "agent.list", Value::Null, 0);
    assert_eq!(
        refused(&mut requests, &text, &members(&list), 0),
        NOT_SIGNED
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
        NOT_SIGNED
    );
}

#[test]
fn a_request_older_than_five_minutes_is_refused() {
    let (_, pixel, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
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
    let mut requests = Requests::new(time(0));
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
    let mut requests = Requests::new(time(0));
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

    // A clock set back by less than the window does not bring it back.
    let mut requests = Requests::new(time(0));
    let text = signed(&pixel, "agent.send", json!({"text": "go on"}), 0);
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());
    let later = signed(&pixel, "agent.list", Value::Null, 4 * MINUTE);
    assert!(
        requests
            .check(&later, "pc", &members, time(4 * MINUTE))
            .is_ok()
    );
    assert_eq!(refused(&mut requests, &text, &members, MINUTE), replayed);
}

#[test]
fn the_nonces_of_a_device_are_bounded_and_forgotten_after_the_window() {
    let (_, pixel, tablet, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
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
    let mut requests = Requests::new(time(0));

    let text = signed(&tablet, "agent.list", Value::Null, 0);
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());
    let text = signed(&tablet, "approval.approve", json!({"request": 3}), 0);
    assert_eq!(
        refused(&mut requests, &text, &members, 0),
        "refused: tablet may only view: a controller or a runner may answer a request"
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

    // The role is checked after the signature: a request in a viewer's name that it did not sign
    // learns nothing of its name or its role.
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
        NOT_SIGNED
    );
}

#[test]
fn every_method_of_the_catalogue_has_a_class() {
    let (_, pixel, tablet, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
    for (method, class) in METHODS {
        let text = signed(&pixel, method, Value::Null, 0);
        let controlled = requests.check(&text, "pc", &members, time(0));
        let text = signed(&tablet, method, Value::Null, 0);
        let viewed = requests.check(&text, "pc", &members, time(0));
        if *class == Class::Local {
            let local =
                format!("refused: {method} is answered only on its own machine, never over a link");
            assert_eq!(controlled.unwrap_err().to_string(), local);
            assert_eq!(viewed.unwrap_err().to_string(), local);
            continue;
        }
        assert!(controlled.is_ok(), "a controller may call {method}");
        match class {
            Class::Read => assert!(viewed.is_ok(), "a viewer may call {method}"),
            Class::Local => unreachable!("checked above"),
            Class::Drive => {
                let refusal = viewed.unwrap_err().to_string();
                assert!(
                    refusal.starts_with(
                        "refused: tablet may only view: a controller or a runner may "
                    ),
                    "{method}: {refusal}"
                );
            }
        }
    }
}

#[test]
fn a_request_signed_before_the_receiver_started_is_refused() {
    let (_, pixel, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
    let text = signed(&pixel, "agent.send", json!({"text": "go on"}), 0);
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());

    // The receiver started again, its memory empty: what it may have served is refused.
    let mut restarted = Requests::new(time(MINUTE));
    assert_eq!(
        refused(&mut restarted, &text, &members, MINUTE),
        "refused: this request was signed before this machine started: send it again"
    );
    let again = signed(&pixel, "agent.send", json!({"text": "go on"}), MINUTE);
    assert!(
        restarted
            .check(&again, "pc", &members, time(MINUTE))
            .is_ok()
    );
}

#[test]
fn a_clock_read_far_ahead_once_locks_no_device_out() {
    let (_, pixel, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
    let hour = 60 * MINUTE;
    let ahead = signed(&pixel, "agent.list", Value::Null, hour);
    assert!(requests.check(&ahead, "pc", &members, time(hour)).is_ok());

    // The clock is right again: requests at the right time are heard.
    let text = signed(&pixel, "agent.list", Value::Null, MINUTE);
    assert!(requests.check(&text, "pc", &members, time(MINUTE)).is_ok());
}

#[test]
fn a_refusal_shows_no_control_character_nor_a_long_text() {
    let (pc, _, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
    let shown = |sentence: String| {
        assert!(
            !sentence.chars().any(|c| c.is_control() || c == '\u{202e}'),
            "{sentence:?}"
        );
        assert!(
            sentence.chars().count() < 200,
            "{} characters",
            sentence.len()
        );
        sentence
    };

    // A method with a newline and an ANSI escape, and one of a megabyte.
    let text = signed(&pc, "agent.\n\u{1b}[31mlist\u{202e}", Value::Null, 0);
    let sentence = shown(refused(&mut requests, &text, &members, 0));
    assert!(
        sentence.contains("is not a method of Flottille's"),
        "{sentence}"
    );
    let text = signed(&pc, &"a".repeat(1 << 20), Value::Null, 0);
    shown(refused(&mut requests, &text, &members, 0));

    // A machine so named, refused before any check.
    let request = Request::sign(
        &pc,
        "7",
        "pc\n\u{1b}[2J",
        "agent.list",
        &Value::Null,
        time(0),
    )
    .unwrap();
    let text = serde_json::to_string(&request).unwrap();
    let sentence = shown(refused(&mut requests, &text, &members, 0));
    assert!(sentence.contains("not for pc"), "{sentence}");
    let request = Request::sign(
        &pc,
        "7",
        &"b".repeat(1 << 20),
        "agent.list",
        &Value::Null,
        time(0),
    )
    .unwrap();
    shown(refused(
        &mut requests,
        &serde_json::to_string(&request).unwrap(),
        &members,
        0,
    ));

    // A text that is not a request, whose error would echo it.
    let mut odd: Value = serde_json::from_str(&signed(&pc, "agent.list", Value::Null, 0)).unwrap();
    odd.as_object_mut()
        .unwrap()
        .insert(format!("x\n\u{1b}[31m{}", "c".repeat(1 << 20)), Value::Null);
    let sentence = shown(refused(&mut requests, &odd.to_string(), &members, 0));
    assert!(
        sentence.starts_with("refused: this is not a signed request"),
        "{sentence}"
    );
}

#[test]
fn remote_setup_is_answered_only_on_its_own_machine() {
    let (_, pixel, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
    let text = signed(
        &pixel,
        "remote.setup",
        json!({"url": "http://hub.example.net"}),
        0,
    );
    let refused = requests
        .check(&text, "pc", &members, time(0))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("remote.setup is answered only on its own machine"),
        "{refused}"
    );
}

#[test]
fn a_refusal_says_which_role_allows_it() {
    let (_, _, tablet, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
    for (method, deed) in [
        ("approval.approve", "answer a request"),
        ("approval.deny", "answer a request"),
        ("agent.answer", "answer a question"),
        ("agent.send", "send a message"),
        ("agent.start", "start a session"),
        ("agent.run", "start a session"),
        ("agent.stop", "stop a session"),
        ("agent.cancel", "stop a turn"),
        ("agent.mode", "switch the mode"),
        ("remote.change", "remove, rename or restyle a device"),
        // A method with no words of its own is named.
        ("project.add", "call project.add"),
    ] {
        let text = signed(&tablet, method, Value::Null, 0);
        assert_eq!(
            refused(&mut requests, &text, &members, 0),
            format!("refused: tablet may only view: a controller or a runner may {deed}"),
            "{method}"
        );
    }
}

#[test]
fn a_launch_on_a_machine_that_is_no_runner_is_refused() {
    let (pc, pixel, _, list) = three();
    let members = members(&list);
    let mut requests = Requests::new(time(0));
    // `pixel` and `tablet` checking a launch of the user's: their member runs no agent.
    for (machine, role) in [("pixel", "controller"), ("tablet", "viewer")] {
        let request = Request::sign(
            &pc,
            "7",
            machine,
            "agent.start",
            &json!({"project": "app"}),
            time(0),
        )
        .unwrap();
        let text = serde_json::to_string(&request).unwrap();
        let why = format!("refused: {machine} is a {role}: it runs no agent, a runner does");
        assert_eq!(
            requests
                .check(&text, machine, &members, time(0))
                .unwrap_err()
                .to_string(),
            why
        );
        // The same sentence for the machine that carries it, before it is sent.
        assert_eq!(
            runs_agents(&members, machine, "agent.start")
                .unwrap_err()
                .to_string(),
            why
        );
        // What starts no session is served there.
        assert!(runs_agents(&members, machine, "agent.list").is_ok());
    }
    // A runner takes it, and a name that is no member's is the carrier's to say.
    assert!(runs_agents(&members, "pc", "agent.start").is_ok());
    assert!(runs_agents(&members, "nowhere", "agent.start").is_ok());
    let text = signed(&pixel, "agent.start", json!({"project": "app"}), 0);
    assert!(requests.check(&text, "pc", &members, time(0)).is_ok());
}

#[test]
fn every_method_that_starts_an_agent_is_refused_on_a_machine_that_is_no_runner() {
    let (_, _, _, list) = three();
    let members = members(&list);
    for method in [
        "agent.resume",
        "agent.quota_resume",
        "spec.new",
        "spec.from_ticket",
        "spec.develop",
        "review.start",
    ] {
        assert_eq!(
            runs_agents(&members, "pixel", method)
                .unwrap_err()
                .to_string(),
            "refused: pixel is a controller: it runs no agent, a runner does",
            "{method}"
        );
        assert!(runs_agents(&members, "pc", method).is_ok(), "{method}");
    }
    // A command of the project's runs no agent.
    assert!(runs_agents(&members, "pixel", "task.run").is_ok());
}

#[test]
fn a_switch_to_full_is_taken_only_as_a_member_that_may_switch_signed_it() {
    let (pc, pixel, tablet, list) = three();
    let members = members(&list);
    let switch = |key: &DeviceKey, params: Value| signed(key, "agent.mode", params, 0);
    let full = json!({"agent": "a1", "mode": "full"});
    let now = time(MINUTE);

    let taken = full_switch(&switch(&pixel, full.clone()), "pc", "a1", &members, now).unwrap();
    assert_eq!(taken.name, "pixel");
    assert!(full_switch(&switch(&pc, full.clone()), "pc", "a1", &members, now).is_ok());

    let refusal = |text: &str, machine: &str, tree: &str| {
        full_switch(text, machine, tree, &members, now)
            .unwrap_err()
            .to_string()
    };
    assert!(refusal(&switch(&tablet, full.clone()), "pc", "a1").contains("may only view"));
    assert!(refusal(&switch(&pixel, full.clone()), "server", "a1").contains("not one of server"));
    let auto = json!({"agent": "a1", "mode": "auto"});
    assert!(refusal(&switch(&pixel, auto), "pc", "a1").contains("another mode"));
    let other = signed(
        &pixel,
        "agent.stop",
        json!({"agent": "a1", "mode": "full"}),
        0,
    );
    assert!(refusal(&other, "pc", "a1").contains("not one of pc"));
    // A key that is no member's, and a signature over other bytes.
    let stranger = signed(&key(9), "agent.mode", full.clone(), 0);
    assert!(refusal(&stranger, "pc", "a1").contains("not signed by a member"));
    let forged = switch(&pixel, full).replace(r#"\"a1\""#, r#"\"a2\""#);
    assert!(refusal(&forged, "pc", "a2").contains("not signed by a member"));
    assert!(refusal("{}", "pc", "a1").contains("not a signed request"));
}

#[test]
fn a_switch_kept_for_another_tree_is_refused() {
    let (_, pixel, _, list) = three();
    let members = members(&list);
    let switch = signed(
        &pixel,
        "agent.mode",
        json!({"agent": "a1", "mode": "full"}),
        0,
    );

    let refused = full_switch(&switch, "pc", "b2", &members, time(MINUTE))
        .unwrap_err()
        .to_string();

    assert!(refused.contains("another tree"), "{refused}");
}

#[test]
fn a_switch_to_full_counts_for_twelve_hours_from_its_signing() {
    let (_, pixel, _, list) = three();
    let members = members(&list);
    let switch = signed(
        &pixel,
        "agent.mode",
        json!({"agent": "a1", "mode": "full"}),
        0,
    );
    let hour = 60 * MINUTE;

    // One night's work: still the user's switch.
    assert!(full_switch(&switch, "pc", "a1", &members, time(12 * hour)).is_ok());
    let refusal = |at: u64| {
        full_switch(&switch, "pc", "a1", &members, time(at))
            .unwrap_err()
            .to_string()
    };
    assert!(
        refusal(12 * hour + MINUTE).contains("is older than twelve hours"),
        "{}",
        refusal(12 * hour + MINUTE)
    );
    // Dated ahead of the clock beyond the window a request is given.
    let ahead = signed(
        &pixel,
        "agent.mode",
        json!({"agent": "a1", "mode": "full"}),
        10 * MINUTE,
    );
    let refused = full_switch(&ahead, "pc", "a1", &members, time(0))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("ahead of this machine's clock"),
        "{refused}"
    );
}
