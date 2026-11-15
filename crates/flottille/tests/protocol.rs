//! The protocol's version and its catalogue of methods (`docs/protocol.md`), against the real
//! binary.

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use common::User;
use flottille_protocol::{METHODS, PROTOCOL, class};
use serde_json::json;

#[test]
fn daemon_status_says_the_protocol_version() {
    let user = User::new();

    assert_eq!(user.status()["protocol"], PROTOCOL);
}

/// The other way, a method answered and not catalogued, is the next test's.
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

/// A method the daemon routes, the window or the command line sends, or an extension's `call`
/// answers is written in the sources as a literal `<family>.<verb>`: every such literal is
/// catalogued or an event (`docs/events.md`), so that a method added without the catalogue fails
/// here, not at run time with `unknown-method`. The extensions' actions and guidance are their
/// data's (the unit test of `extensions`).
#[test]
fn every_method_the_sources_name_is_catalogued() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let events = fs::read_to_string(crates.join("../docs/events.md")).unwrap();
    // The names of the events are in the headings, one or several per heading.
    let events: HashSet<&str> = events
        .lines()
        .filter(|line| line.starts_with('#'))
        .flat_map(|line| line.split('`').skip(1).step_by(2))
        .collect();
    let daemon = fs::read_to_string(crates.join("flottille/src/daemon.rs")).unwrap();
    // The catalogue's families, and each one `daemon.rs` routes, by a prefix (`"agent."`) or a
    // method (`"daemon.status"`): a family left wholly out of the catalogue is still read.
    let mut families: HashSet<String> = METHODS
        .iter()
        .map(|(name, _)| name.split_once('.').unwrap().0.to_owned())
        .collect();
    families.extend(
        quoted(&daemon)
            .filter_map(|literal| literal.split_once('.'))
            .filter(|(family, verb)| is_word(family) && (verb.is_empty() || is_word(verb)))
            .map(|(family, _)| family.to_owned()),
    );
    // Not methods, and not in `docs/events.md`.
    let besides = [
        // Files: the daemon's sessions, and their next version before it takes their place.
        "daemon.sessions",
        "sessions.next",
        // An event, said in `docs/protocol.md` ("Rules").
        "rules.changed",
    ];
    let files = ["md", "toml", "log", "pdf", "sock"];

    let mut sources = Vec::new();
    for folder in [
        "flottille/src",
        "flottille-client/src",
        "flottille-window/src",
    ] {
        rust_files(&crates.join(folder), &mut sources);
    }
    let mut unknown = Vec::new();
    for path in &sources {
        let source = fs::read_to_string(path).unwrap();
        for literal in quoted(&source) {
            let Some((family, verb)) = literal.split_once('.') else {
                continue;
            };
            if families.contains(family)
                && is_word(verb)
                && class(literal).is_none()
                && !events.contains(literal)
                && !besides.contains(&literal)
                && !files.contains(&verb)
            {
                unknown.push(format!("{literal} ({})", path.display()));
            }
        }
    }

    // `skill import` and `command import` ask `<kind>.<verb>`, built at run time: the kinds are
    // the callers', the verbs `import_command`'s.
    let compact = |path: &Path| -> String {
        let text = fs::read_to_string(path).unwrap();
        text.split_whitespace().collect()
    };
    let kinds: Vec<String> = sources
        .iter()
        .flat_map(|path| after(&compact(path), "import_command::run(&mutclient,\""))
        .collect();
    let verbs = after(
        &compact(&crates.join("flottille/src/import_command.rs")),
        "ask(\"",
    );
    assert!(
        !kinds.is_empty() && !verbs.is_empty(),
        "{kinds:?} {verbs:?}"
    );
    for kind in &kinds {
        for verb in &verbs {
            let method = format!("{kind}.{verb}");
            if class(&method).is_none() {
                unknown.push(format!("{method} (import_command.rs)"));
            }
        }
    }

    assert!(unknown.is_empty(), "not catalogued: {unknown:?}");
}

/// Each text between two quotes of `source`, a literal or not: one that reads `family.verb` can
/// only be a literal.
fn quoted(source: &str) -> impl Iterator<Item = &str> {
    source.split('"')
}

fn is_word(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// Each text written between `opening` and the next quote in `text`.
fn after(text: &str, opening: &str) -> Vec<String> {
    text.split(opening)
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
        .map(|(word, _)| word.to_owned())
        .collect()
}

fn rust_files(folder: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}
