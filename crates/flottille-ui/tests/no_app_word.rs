//! The kit is shared by more than one application: its sources, examples and tests, comments
//! included, name nothing of what one of them is about. A scan holds it, shown a sample of the
//! wrong text first.

use std::fs;
use std::path::{Path, PathBuf};

/// What belongs to an application and not to the kit: the app's name, and what it is made of.
const APP_WORDS: [&str; 5] = ["flottille", "agent", "session", "project", "workspace"];

/// The kit's own crate name, which a `use` in an example or a test cannot avoid: it is the kit's
/// and is not counted.
const OWN_NAME: [&str; 2] = ["flottille_ui", "flottille-ui"];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// Whether `line` has `word` at the start of a word of its own (`sub-agents` has `agent`, a
/// colour called `magenta` has not).
fn names(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(at, _)| {
        !line[..at]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_alphabetic())
    })
}

/// The lines of `source` that name an application word, numbered from 1, with the word.
fn app_words(source: &str) -> Vec<(usize, &'static str)> {
    source
        .lines()
        .enumerate()
        .flat_map(|(at, line)| {
            let line = OWN_NAME
                .into_iter()
                .fold(line.to_lowercase(), |line, own| line.replace(own, ""));
            APP_WORDS
                .into_iter()
                .filter(move |word| names(&line, word))
                .map(move |word| (at + 1, word))
        })
        .collect()
}

#[test]
fn an_application_word_is_found() {
    let sample = "/// A row of a session.\nlet x = 1;\nfn flottille() {}\n// The Agents tab";
    assert_eq!(
        app_words(sample),
        [(1, "session"), (3, "flottille"), (4, "agent")]
    );
    assert!(app_words("/// A row of a list.\nlet x = 1;").is_empty());
    assert!(app_words("\"magenta\", \"cyan\"").is_empty());
}

#[test]
fn the_name_of_the_kit_itself_is_not_an_application_word() {
    assert!(app_words("use flottille_ui::Button;\n//! cargo run -p flottille-ui").is_empty());
    assert_eq!(
        app_words("flottille_ui::Button::new(\"New session\")"),
        [(1, "session")]
    );
}

#[test]
fn the_kit_names_no_app_word() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for folder in ["src", "examples", "tests"] {
        sources(&root.join(folder), &mut files);
    }
    // This file holds the words, as the samples of its own scan.
    files.retain(|file| !file.ends_with("tests/no_app_word.rs"));
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    let mut found = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for (at, word) in app_words(&text) {
            let line = text.lines().nth(at - 1).unwrap().trim();
            found.push(format!("{}:{at}: `{word}`: {line}", file.display()));
        }
    }
    assert!(
        found.is_empty(),
        "the kit names what an application is about:\n{}",
        found.join("\n")
    );
}
