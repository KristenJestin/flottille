//! A text field takes its style from `flottille_ui::field_style`, and keeps it with the theme
//! through `follow_theme`. A source scan holds it: a field made with a style of its own would
//! keep the colours of the theme it was made in.

use std::fs;
use std::path::{Path, PathBuf};

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

fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or_default()
}

/// What is wrong in `source`, a file that is not the helper itself: a style built by hand, a
/// style set by hand, a field made and never given `follow_theme` within its statement.
fn faults(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let line = code(line);
        let by_hand = line.contains("InputEditorStyle {") || line.contains("set_editor_style(");
        let made = line.contains("InputState::new(") || line.contains("TextareaState::new(");
        let followed = lines[at..lines.len().min(at + 12)]
            .iter()
            .any(|next| code(next).contains("follow_theme("));
        if by_hand || (made && !followed) {
            found.push((at + 1, line.trim().to_owned()));
        }
    }
    found
}

#[test]
fn a_field_with_a_style_of_its_own_is_found() {
    let by_hand =
        "field.set_editor_style(InputEditorStyle { caret: p.text, ..Default::default() });";
    assert_eq!(faults(by_hand).len(), 1);
    let never_followed = "let field = InputState::new(window, cx);\nfield";
    assert_eq!(faults(never_followed).len(), 1);
}

#[test]
fn a_field_that_follows_the_theme_and_a_comment_about_it_are_not() {
    let followed = "let mut field = InputState::new(window, cx);\nfollow_theme(&mut field, cx);";
    assert!(faults(followed).is_empty());
    assert!(faults("// never `set_editor_style(` by hand").is_empty());
}

#[test]
fn every_text_field_takes_its_style_from_the_helper() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    sources(&root.join("../flottille-ui/src"), &mut files);
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    let mut offenders = Vec::new();
    for file in files {
        // The helper is where a style is built.
        if file.ends_with("flottille-ui/src/field.rs")
            || file.ends_with("../flottille-ui/src/field.rs")
        {
            continue;
        }
        let source = fs::read_to_string(&file).unwrap();
        for (at, line) in faults(&source) {
            offenders.push(format!("{}:{at}: {line}", file.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "a text field takes its style from flottille_ui::field_style, through follow_theme:\n{}",
        offenders.join("\n")
    );
}
