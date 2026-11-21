//! The window does everything the command line does (#1179): the maintainer does nothing from a
//! terminal. Every command of `docs/cli.md` has an entry of `flottille_window::parity::COMMANDS`:
//! the daemon methods the window calls for it, each one a string literal of the window's sources,
//! or a reason it is for scripts only. A command added to the command line without its place in
//! the window fails here until it has one, or until its entry says why it has none.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use flottille_window::parity::{COMMANDS, Place};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The commands of `docs/cli.md`: the words of each `### \`flottille …\`` heading before its first
/// argument or option.
fn documented() -> BTreeSet<String> {
    let text = std::fs::read_to_string(root().join("docs/cli.md")).unwrap();
    text.lines()
        .filter_map(|line| line.strip_prefix("### `flottille "))
        .filter_map(|line| line.split('`').next())
        .map(|heading| {
            heading
                .split_whitespace()
                .take_while(|word| {
                    !word.starts_with(['<', '[', '(', '-']) && !word.contains(['<', '['])
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Everything the window's sources say, to find a method called by its string literal.
fn window_sources() -> String {
    fn walk(folder: &Path, into: &mut String) {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !path.ends_with("parity.rs")
            {
                into.push_str(&std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut sources = String::new();
    walk(&root().join("crates/flottille-window/src"), &mut sources);
    sources
}

#[test]
fn every_cli_command_has_its_place_in_the_window() {
    let documented = documented();
    let listed: BTreeSet<String> = COMMANDS
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    assert_eq!(
        listed.len(),
        COMMANDS.len(),
        "a command is listed twice in flottille_window::parity::COMMANDS"
    );
    let missing: Vec<_> = documented.difference(&listed).collect();
    assert!(
        missing.is_empty(),
        "a command of docs/cli.md has no place in the window: give it the daemon methods the \
         window calls for it in crates/flottille-window/src/parity.rs, or say why it is for \
         scripts only: {missing:?}"
    );
    let stale: Vec<_> = listed.difference(&documented).collect();
    assert!(
        stale.is_empty(),
        "parity.rs lists commands docs/cli.md does not have: {stale:?}"
    );

    let sources = window_sources();
    for (name, place) in COMMANDS {
        match place {
            Place::Window(methods) => {
                assert!(
                    !methods.is_empty(),
                    "{name}: no method says where it is done"
                );
                for method in *methods {
                    assert!(
                        sources.contains(&format!("\"{method}\"")),
                        "{name}: the window never calls {method}"
                    );
                }
            }
            Place::Script(why) => assert!(
                why.len() > 12,
                "{name}: a script-only command says why, in a sentence"
            ),
        }
    }

    // The script-only list stays short: a command is for scripts when it is not a user's act.
    let scripts = COMMANDS
        .iter()
        .filter(|(_, place)| matches!(place, Place::Script(_)))
        .count();
    assert!(
        scripts <= 12,
        "{scripts} commands are for scripts only: is each one not a user's act?"
    );
}
