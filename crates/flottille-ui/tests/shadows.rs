//! A shadow is a role of the palette (`Palette::shadow`), lighter in Light: no overlay draws a
//! black of its own, which would be as heavy on a light page as on a dark one.

use std::fs;
use std::path::{Path, PathBuf};

/// The lines that draw a black of their own.
fn own_blacks(source: &str) -> Vec<(usize, String)> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.trim_start();
            !code.starts_with("//") && code.contains("hsla(0., 0., 0.")
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
fn a_black_of_its_own_is_found() {
    let found = own_blacks("color: hsla(0., 0., 0., 0.5),\n// hsla(0., 0., 0., 1.)\n");
    assert_eq!(found, vec![(1, "color: hsla(0., 0., 0., 0.5),".to_owned())]);
}

#[test]
fn no_overlay_draws_a_shadow_in_a_black_of_its_own() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    sources(&root.join("../flottille-window/src"), &mut files);
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    let mut offenders = Vec::new();
    for file in files {
        for (at, line) in own_blacks(&fs::read_to_string(&file).unwrap()) {
            offenders.push(format!("{}:{at}: {line}", file.display()));
        }
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}
