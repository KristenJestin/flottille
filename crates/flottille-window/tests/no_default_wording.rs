//! The window never says "default model" or "default effort" (issue #280), nor that a provider
//! chooses (issue #329): the model and the effort are said by their names, the ones Flottille
//! applies. The window scenarios hold what the pickers and the lines say; what can be held here is
//! the way in, the window's sources. Nor does it show the adapter's "Default (recommended)" as a
//! model's label (issue #583): the daemon names that row by its model, and the window never
//! shows the adapter's wording for it, stored or not.

use std::fs;
use std::path::{Path, PathBuf};

/// The lines whose code says one of the words; a comment may talk about them.
fn said(source: &str) -> Vec<(usize, String)> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.trim_start().to_lowercase();
            !code.starts_with("//")
                && ["default model", "default effort", "chosen by"]
                    .iter()
                    .any(|words| code.contains(words))
        })
        .map(|(at, line)| (at + 1, line.trim().to_owned()))
        .collect()
}

/// The lines of the window's code, test modules left out (their fixtures are the adapter's own
/// words), that say what the adapter words its own choice by.
fn adapter_wording(source: &str) -> Vec<(usize, String)> {
    source
        .lines()
        .enumerate()
        .take_while(|(_, line)| line.trim() != "#[cfg(test)]")
        .filter(|(_, line)| {
            let code = line.trim_start().to_lowercase();
            !code.starts_with("//") && code.contains("(recommended)")
        })
        .map(|(at, line)| (at + 1, line.trim().to_owned()))
        .collect()
}

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

#[test]
fn a_string_that_says_default_model_is_found_and_a_comment_is_not() {
    assert_eq!(said("let a = \"default model\";").len(), 1);
    assert_eq!(said("\"Default effort\".into()").len(), 1);
    assert!(said("/// the default model's name\n// default effort").is_empty());
}

#[test]
fn a_string_that_shows_the_adapter_s_default_wording_is_found_outside_the_tests() {
    assert_eq!(
        adapter_wording("let a = \"Default (recommended)\";").len(),
        1
    );
    assert!(adapter_wording("// \"Default (recommended)\"").is_empty());
    assert!(
        adapter_wording("fn a() {}\n#[cfg(test)]\nmod t { \"Default (recommended)\" }").is_empty()
    );
}

#[test]
fn a_string_that_says_chosen_by_is_found() {
    assert_eq!(said("format!(\"Chosen by {title}\")").len(), 1);
    assert_eq!(said("let a = \"model chosen by Claude\";").len(), 1);
}

#[test]
fn the_window_s_sources_say_no_default_model_and_no_default_effort() {
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(!files.is_empty());
    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        let found = said(&source);
        assert!(found.is_empty(), "{}: {found:?}", file.display());
        let found = adapter_wording(&source);
        assert!(found.is_empty(), "{}: {found:?}", file.display());
    }
}
