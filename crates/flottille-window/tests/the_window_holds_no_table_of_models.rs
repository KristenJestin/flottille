//! The window holds no table of models (issue #713): what a default slot gives is the daemon's
//! answer (`extension.list`), never words of the window's own. The tests of a source, after its
//! `#[cfg(test)]`, are invented data and left out.

use std::fs;
use std::path::{Path, PathBuf};

/// The names of models and of their families, as a provider words them.
const MODELS: &[&str] = &[
    "opus", "sonnet", "haiku", "fable", "gpt-", "codex-", "gemini",
];

/// The lines of the code that say a model, comments and the tests after `#[cfg(test)]` left out.
fn said(source: &str) -> Vec<(usize, String)> {
    source
        .lines()
        .enumerate()
        .take_while(|(_, line)| line.trim() != "#[cfg(test)]")
        .filter(|(_, line)| {
            let code = line.trim_start().to_lowercase();
            !code.starts_with("//") && MODELS.iter().any(|name| code.contains(name))
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
fn a_string_that_names_a_model_is_found_and_a_comment_or_a_test_is_not() {
    assert_eq!(
        said("(\"One size smaller\", \"Sonnet for Claude\")").len(),
        1
    );
    assert_eq!(said("let a = \"gpt-5\";").len(), 1);
    assert!(said("/// Opus for Claude\n// sonnet").is_empty());
    assert!(said("fn a() {}\n#[cfg(test)]\nmod t { \"opus\" }").is_empty());
}

#[test]
fn the_window_holds_no_table_of_models() {
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(!files.is_empty());
    for file in files {
        let found = said(&fs::read_to_string(&file).unwrap());
        assert!(found.is_empty(), "{}: {found:?}", file.display());
    }
}
