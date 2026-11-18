//! A dialog opens over the page it is asked from (`docs/design.md` §10, #1018): the window never
//! goes to a page only to open a dialog there. The sweep is a scan of the window's sources: a
//! function that changes page (`open_settings_at`, `open_project_settings_at`) and opens a dialog
//! of that page (a form of the vault, of the connections, a review) in the same body is the
//! defect this issue removed. The dialogs are opened through the host (`Shell::open_modal`) from
//! wherever the user is; a function that changes page does only that.

use std::fs;
use std::path::{Path, PathBuf};

/// What changes the page shown.
const CHANGES_PAGE: [&str; 2] = ["open_settings_at(", "open_project_settings_at("];

/// What opens a dialog of the vault's or the connections' sections.
const OPENS_A_DIALOG: [&str; 9] = [
    ".add_prefilled(",
    ".add_asked(",
    ".add_for_request(",
    ".replace_in(",
    ".replace(",
    ".answer_request(",
    ".open_review",
    ".edit(",
    ".add(window",
];

/// The functions of `source` that do both: their name and line.
fn changing_page_to_open_a_dialog(source: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut current: Option<(usize, String, String)> = None;
    let mut finish = |current: Option<(usize, String, String)>| {
        if let Some((at, name, body)) = current
            && CHANGES_PAGE.iter().any(|call| body.contains(call))
            && OPENS_A_DIALOG.iter().any(|call| body.contains(call))
        {
            found.push((at, name));
        }
    };
    for (at, line) in source.lines().enumerate() {
        let code = line.trim_start();
        let starts = code.starts_with("pub fn ") || code.starts_with("fn ");
        // Only the methods and functions of the first two levels start a body.
        if starts && line.len() - code.len() <= 4 {
            finish(current.take());
            let name = code
                .trim_start_matches("pub ")
                .trim_start_matches("fn ")
                .split(['(', '<'])
                .next()
                .unwrap_or_default()
                .to_owned();
            current = Some((at + 1, name, String::new()));
        } else if let Some((_, _, body)) = current.as_mut()
            && !code.starts_with("//")
        {
            body.push_str(line);
            body.push('\n');
        }
    }
    finish(current);
    found
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
fn a_function_that_changes_page_and_opens_its_dialog_is_found() {
    let source = "impl Shell {\n    pub fn add_entry(&mut self) {\n        self.open_settings_at(Reveal::Vault, window, cx);\n        vault.update(cx, |v, cx| v.add_prefilled(p, window, cx));\n    }\n}";
    assert_eq!(
        changing_page_to_open_a_dialog(source),
        vec![(2, "add_entry".to_owned())]
    );
}

#[test]
fn a_function_that_only_changes_page_or_only_opens_a_dialog_is_not() {
    let navigation = "    fn go(&mut self) {\n        self.open_settings_at(Reveal::Rules, window, cx);\n    }\n    fn add(&mut self) {\n        self.open_modal(&vault, window, cx);\n        vault.update(cx, |v, cx| v.add_prefilled(p, window, cx));\n    }\n";
    assert!(changing_page_to_open_a_dialog(navigation).is_empty());
}

#[test]
fn no_dialog_is_opened_by_changing_page() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    let mut offenders = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        for (at, name) in changing_page_to_open_a_dialog(&source) {
            offenders.push(format!("{}:{at} `{name}`", file.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "these go to a page to open a dialog there; open it through the host, over the page the \
         user is on:\n{}",
        offenders.join("\n")
    );
}
