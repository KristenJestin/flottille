//! The rule "everything that moves is animated" (`docs/design.md` §10), as far as a test can hold
//! it. GPUI's own `.hover(|d| ..)` style switches at once, and gpui-kit's test support does not
//! say which listeners an element has, so no test can walk a view and ask each interactive
//! element for its hover transition. What can be held is the way in: the window's sources never
//! use the instant style; a hover is `hoverable`, `selectable` or a component's own (`Button`,
//! `ListRow`, `TreeRow`, `MenuItem`), which fade over `HOVER`. The rest of the rule is the
//! checklist of `docs/reference/gpui.md`, which each window pull request ticks.

use std::fs;
use std::path::{Path, PathBuf};

/// The lines that use a style GPUI switches at once on hover.
fn instant_hovers(source: &str) -> Vec<(usize, String)> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.trim_start();
            !code.starts_with("//") && (code.contains(".hover(") || code.contains(".group_hover("))
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
fn a_hover_style_that_switches_at_once_is_found() {
    let found = instant_hovers("div()\n    .hover(|d| d.bg(p.hover))\n    .child(x)");
    assert_eq!(found, vec![(2, ".hover(|d| d.bg(p.hover))".to_owned())]);
    let group = instant_hovers("d.group_hover(\"row\", |d| d)");
    assert_eq!(group.len(), 1);
}

#[test]
fn a_comment_about_it_and_a_fading_hover_are_not() {
    let source =
        "// GPUI's `.hover(..)` switches at once\n/// never `.hover(`\nhoverable(el, |d, t| d)";
    assert!(instant_hovers(source).is_empty());
}

#[test]
fn no_view_of_the_window_switches_a_hover_at_once() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    sources(&root.join("../flottille-ui/src"), &mut files);
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    let mut offenders = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        for (at, line) in instant_hovers(&source) {
            offenders.push(format!("{}:{at}: {line}", file.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "a hover is animated: use `hoverable` or `selectable` (docs/design.md §10):\n{}",
        offenders.join("\n")
    );
}
