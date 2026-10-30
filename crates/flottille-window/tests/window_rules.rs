//! The rules of the window (`docs/reference/gpui.md`, "Rules of the window") that a scan of the
//! window's sources can hold: a confirmation is a dialog, never buttons that turn into others in
//! place; an action's result is a toast, never a line under a form; a dialog offers no segmented
//! control; a gap or a padding is a step of the spacing scale. Each scan is shown a sample of the
//! wrong code first.

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

/// Every source of the window, with its text.
fn window() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    files
        .into_iter()
        .map(|file| {
            let text = fs::read_to_string(&file).unwrap();
            (file, text)
        })
        .collect()
}

/// The lines of code, numbered from 1, comments left out.
fn code(source: &str) -> impl Iterator<Item = (usize, &str)> {
    source
        .lines()
        .enumerate()
        .map(|(at, line)| (at + 1, line.trim()))
        .filter(|(_, line)| !line.starts_with("//"))
}

fn offenders(find: impl Fn(&str) -> Vec<(usize, String)>) -> Vec<String> {
    window()
        .into_iter()
        .flat_map(|(file, text)| {
            find(&text)
                .into_iter()
                .map(move |(at, line)| format!("{}:{at}: {line}", file.display()))
        })
        .collect()
}

// ── 2. One confirmation: a modal dialog ────────────────────────────────────────────────────────

/// The element ids of a row's button that turned into Cancel and a second button in place.
fn inline_confirmations(source: &str) -> Vec<(usize, String)> {
    code(source)
        .filter(|(_, line)| {
            [
                "\"confirm-remove",
                "\"cancel-remove",
                "\"confirm-end",
                "\"cancel-end",
            ]
            .iter()
            .any(|id| line.contains(id))
        })
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_button_that_turns_into_others_in_place_is_found() {
    let sample = "Button::quiet(id(\"cancel-remove-task\"), \"Cancel\")\n\
                  Button::secondary(id(\"confirm-remove-task\"), \"Remove\")";
    assert_eq!(inline_confirmations(sample).len(), 2);
    assert!(inline_confirmations("Confirm::new(\"remove-task\", question)").is_empty());
}

#[test]
fn every_confirmation_of_the_window_is_a_dialog() {
    let found = offenders(inline_confirmations);
    assert!(
        found.is_empty(),
        "a confirmation is a `Confirm` dialog, never buttons that turn into others in place:\n{}",
        found.join("\n")
    );
}

// ── 3. An action's result is a toast ───────────────────────────────────────────────────────────

/// The words that say an action's result, written as a line of a page instead of a toast.
fn results_under_a_form(source: &str) -> Vec<(usize, String)> {
    code(source)
        .filter(|(_, line)| {
            (line.contains("\"Saved") || line.contains("\"Added:") || line.contains("\"Removed:"))
                && !line.contains("Toast::")
        })
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_result_written_under_a_form_is_found() {
    let sample = "Said::Fine(\n\"Saved: Flottille reads these forges from now on.\".into())";
    assert_eq!(results_under_a_form(sample).len(), 1);
    assert!(results_under_a_form("toast(Toast::success(\"Saved\"), cx)").is_empty());
    assert!(results_under_a_form("toast(Toast::success(\"Forges saved\"), cx)").is_empty());
}

#[test]
fn every_result_of_an_action_is_a_toast() {
    let found = offenders(results_under_a_form);
    assert!(
        found.is_empty(),
        "an action's result is a toast (`flottille_ui::toast`), never a line under a form:\n{}",
        found.join("\n")
    );
}

// ── 4. Every control has a visible label: no segmented control in a dialog ─────────────────────

/// The functions that build a dialog and put a segmented control in it.
fn segmented_in_a_dialog(source: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut function: Option<(usize, String)> = None;
    let (mut dialog, mut segmented) = (false, None);
    let mut close = |function: &Option<(usize, String)>, dialog: bool, segmented: Option<usize>| {
        if let (Some((_, name)), true, Some(at)) = (function, dialog, segmented) {
            found.push((at, name.clone()));
        }
    };
    for (at, line) in code(source) {
        if line.starts_with("fn ") || line.contains(" fn ") {
            close(&function, dialog, segmented);
            function = Some((at, line.to_owned()));
            (dialog, segmented) = (false, None);
        }
        if line.contains("Modal::new") || line.contains("Confirm::new") {
            dialog = true;
        }
        if line.contains("Segmented::new") {
            segmented.get_or_insert(at);
        }
    }
    close(&function, dialog, segmented);
    found
}

#[test]
fn a_segmented_control_in_a_dialog_is_found() {
    let sample = "fn modal(&self) -> Modal {\n    Modal::new(\"skill-modal\", title)\n\
                  .child(Segmented::new(\"skill-kind\", \"Kind\", [], 0))\n}\n\
                  fn bar(&self) {\n    Segmented::new(\"mode\", \"Mode\", [], 0)\n}";
    assert_eq!(segmented_in_a_dialog(sample).len(), 1);
}

#[test]
fn no_dialog_of_the_window_offers_a_segmented_control() {
    let found = offenders(segmented_in_a_dialog);
    assert!(
        found.is_empty(),
        "a form's choice of a kind is `RadioChoices` or a `Select`, never `Segmented`:\n{}",
        found.join("\n")
    );
}

// ── 9. Spacing: the scale ──────────────────────────────────────────────────────────────────────

/// The gaps and paddings given as a literal instead of a step of `space`.
fn spacing_literals(source: &str) -> Vec<(usize, String)> {
    const SPACING: [&str; 10] = [
        ".gap(", ".gap_x(", ".gap_y(", ".p(", ".px(", ".py(", ".pt(", ".pb(", ".pl(", ".pr(",
    ];
    code(source)
        .filter(|(_, line)| {
            SPACING.iter().any(|call| {
                line.match_indices(call)
                    .any(|(at, _)| line[at + call.len()..].starts_with("px("))
            })
        })
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_gap_or_a_padding_off_the_scale_is_found() {
    let sample = ".gap(px(10.))\n.px(px(if narrow { 12. } else { 28. }))\n.gap(space::SM)\n\
                  .py(space::XS)\n.w(px(10.))";
    assert_eq!(
        spacing_literals(sample)
            .into_iter()
            .map(|(at, _)| at)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn every_gap_and_padding_of_the_window_is_a_step_of_the_scale() {
    let found = offenders(spacing_literals);
    assert!(
        found.is_empty(),
        "a gap or a padding is a step of `flottille_ui::space` (4, 8, 12, 16, 24, 32, 48):\n{}",
        found.join("\n")
    );
}
