//! The protocol's version and its catalogue of methods (`docs/protocol.md`), against the real
//! binary.

mod common;

use common::User;
use flottille_protocol::{METHODS, PROTOCOL};
use serde_json::json;

#[test]
fn daemon_status_says_the_protocol_version() {
    let user = User::new();

    assert_eq!(user.status()["protocol"], PROTOCOL);
}

/// The other way, a method answered and not catalogued, cannot happen: the daemon calls a method
/// unknown before anything reads it when the catalogue does not name it.
#[test]
fn every_method_of_the_catalogue_is_answered() {
    // The `test.*` methods are answered only with their variables; the providers stand on the
    // fake agent, so that listing them runs no program of the machine's.
    let user = User::with_fake_providers()
        .with_env("FLOTTILLE_TEST_HOLD", "1")
        .with_env("FLOTTILLE_TEST_CLOCK", "1");

    // `daemon.stop` last: it ends the daemon the others ask.
    let mut methods: Vec<&str> = METHODS.iter().map(|(name, _)| *name).collect();
    methods.sort_by_key(|name| *name == "daemon.stop");
    let unknown: Vec<&str> = methods
        .into_iter()
        .filter(|method| {
            matches!(
                user.call(method, json!({})),
                Err((code, _)) if code == "unknown-method"
            )
        })
        .collect();

    assert!(unknown.is_empty(), "answered unknown: {unknown:?}");
}
