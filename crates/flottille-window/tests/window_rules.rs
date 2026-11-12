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

/// The window's own components, which were parts of `flottille-ui` and keep the exact sizes of
/// the design as the kit's do: only the spacing scan leaves them out, every other scan reads them.
const COMPONENTS: [&str; 2] = ["state.rs", "steps.rs"];

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
    offenders_in(window(), find)
}

fn offenders_in(
    files: Vec<(PathBuf, String)>,
    find: impl Fn(&str) -> Vec<(usize, String)>,
) -> Vec<String> {
    files
        .into_iter()
        .flat_map(|(file, text)| {
            find(&text)
                .into_iter()
                .map(move |(at, line)| format!("{}:{at}: {line}", file.display()))
        })
        .collect()
}

#[test]
fn the_scans_read_the_components_the_spacing_scan_leaves_out() {
    let files = window();
    for own in COMPONENTS {
        assert!(
            files.iter().any(|(file, _)| file.ends_with(own)),
            "{own} is read by the scans"
        );
    }
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
        "an action's result is a toast (`gpui_design_system::toast`), never a line under a form:\n{}",
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
    let screens = window()
        .into_iter()
        .filter(|(file, _)| !COMPONENTS.iter().any(|own| file.ends_with(own)))
        .collect();
    let found = offenders_in(screens, spacing_literals);
    assert!(
        found.is_empty(),
        "a gap or a padding is a step of `gpui_design_system::space` (4, 8, 12, 16, 24, 32, 48):\n{}",
        found.join("\n")
    );
}

// ── The conversation's Markdown is drawn in the palette ────────────────────────────────────────

/// The lines that build a Markdown view of the kit's own, which draws in the kit's light theme
/// (dark ink on a dark window, white chips): Markdown is drawn by `MarkdownPiece`, which takes
/// `markdown_style`.
fn markdown_in_the_kit_theme(source: &str) -> Vec<(usize, String)> {
    code(source)
        .filter(|(_, line)| line.contains("TextView::markdown("))
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_markdown_view_in_the_kit_theme_is_found() {
    let sample = "TextView::markdown(id(\"report-markdown\"), text.clone())";
    assert_eq!(markdown_in_the_kit_theme(sample).len(), 1);
    assert!(markdown_in_the_kit_theme("TextView::new(&self.state).style(style)").is_empty());
}

#[test]
fn every_markdown_of_the_window_is_drawn_in_the_palette() {
    let found = offenders(markdown_in_the_kit_theme);
    assert!(
        found.is_empty(),
        "Markdown is a `MarkdownPiece` in `markdown_view` (`markdown_style`: the palette's inks and surfaces):\n{}",
        found.join("\n")
    );
}

// ── A state mark is an icon, never a text character ────────────────────────────────────────────

/// The lines that draw a state mark with a character (`✓ ✗ ● ■ ×`, or `–` alone in a string),
/// which looks like ASCII next to the stroked icons: a mark is an `Icon` of `gpui-design-system`.
fn text_marks(source: &str) -> Vec<(usize, String)> {
    code(source)
        .filter(|(_, line)| line.chars().any(|c| "✓✗●■×".contains(c)) || line.contains("\"–\""))
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_state_mark_drawn_with_a_character_is_found() {
    for sample in [
        "Ending::Exited(0) => (\"✓\", p.ok),",
        "let mark = if ready { \"✓\" } else { \"✗\" };",
        "Some(_) => (\"●\", p.run, \"checks pending\"),",
        "Ending::Stopped => (\"■\", p.faint),",
        "Ending::Interrupted => (\"–\", p.faint),",
        ".child(\"×\")",
    ] {
        assert_eq!(text_marks(sample).len(), 1, "{sample}");
    }
    assert!(text_marks("// a comment about ✓ and ✗").is_empty());
    assert!(text_marks(".child(icon(Icon::Check))").is_empty());
}

#[test]
fn every_state_mark_of_the_window_is_an_icon() {
    let found = offenders(text_marks);
    assert!(
        found.is_empty(),
        "a state mark is an `Icon` drawn with `icon(..)`, not a text character:\n{}",
        found.join("\n")
    );
}

// ── Words: the window never says "extension" ───────────────────────────────────────────────────

/// The string literals of the code that say "extension" in words: a text the window shows. A
/// method or a key the daemon names (`"extension.list"`, `"extensions"`) has no space, and is no
/// sentence.
fn says_extension(source: &str) -> Vec<(usize, String)> {
    code(source)
        .filter(|(_, line)| {
            line.split('"')
                .skip(1)
                .step_by(2)
                .any(|text| text.contains(' ') && text.to_lowercase().contains("extension"))
        })
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_text_that_says_extension_is_found() {
    let sample = "Section::new(\"specs\", \"Specs\").sentence(\"An extension of Flottille.\")";
    assert_eq!(says_extension(sample).len(), 1);
    assert_eq!(
        says_extension("toast(Toast::failure(\"Extensions not read\"), cx)").len(),
        1
    );
    assert!(says_extension("ask(\"extension.list\", json!({}))").is_empty());
    assert!(says_extension("let all = listed[\"extensions\"].as_array();").is_empty());
}

#[test]
fn the_window_never_says_extension() {
    let found = offenders(says_extension);
    assert!(
        found.is_empty(),
        "the word \"extension\" never appears in the window (`docs/extensions.md`):\n{}",
        found.join("\n")
    );
}

// ── A one-line text has a way to shrink ────────────────────────────────────────────────────────

/// The lines that set a text on one line (`whitespace_nowrap`) with nothing near them that lets
/// it give way: `overflow_hidden` (it is clipped, and cut with an ellipsis), `text_ellipsis`, or
/// `flex_none` (a short mark that keeps its room, which its siblings give way to). Such a text
/// leaves its frame as soon as it is long. "Near" is the same builder chain: nine lines around.
fn unshrinkable_lines(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    code(source)
        .filter(|(_, line)| line.contains(".whitespace_nowrap()"))
        .filter(|(at, _)| {
            let around = &lines[at.saturating_sub(10)..(*at + 9).min(lines.len())];
            !around.iter().any(|line| {
                [".overflow_hidden()", ".text_ellipsis()", ".flex_none()"]
                    .iter()
                    .any(|way| line.contains(way))
            })
        })
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_one_line_text_that_cannot_shrink_is_found() {
    let wrong = "div()\n    .mono(12.)\n    .whitespace_nowrap()\n    .child(path)";
    assert_eq!(unshrinkable_lines(wrong).len(), 1);
    for right in [
        "div()\n    .overflow_hidden()\n    .whitespace_nowrap()\n    .child(path)",
        "div()\n    .whitespace_nowrap()\n    .text_ellipsis()\n    .child(path)",
        "div()\n    .flex_none()\n    .whitespace_nowrap()\n    .child(mark)",
    ] {
        assert!(unshrinkable_lines(right).is_empty(), "{right}");
    }
    assert!(unshrinkable_lines("// .whitespace_nowrap() in a comment").is_empty());
}

#[test]
fn every_one_line_text_of_the_window_can_shrink() {
    let found = offenders(unshrinkable_lines);
    assert!(
        found.is_empty(),
        "a text on one line is clipped (`overflow_hidden`, with `text_ellipsis`) so that it never \
         leaves its frame; a path is a `path_line`:\n{}",
        found.join("\n")
    );
}

// ── The composer's toolbar: Send is its only primary ───────────────────────────────────────────

/// The lines of a `reply-` toggle of the composer's toolbar (`Button::choice`) with no `.ghost()`
/// in its builder chain: a choice fills with the accent when it is the chosen one, and in the
/// composer only Send is filled. "In the chain" is the five lines after it.
fn filled_toolbar_choices(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    code(source)
        .filter(|(_, line)| line.contains("Button::choice(\"reply-"))
        .filter(|(at, _)| {
            let chain = &lines[at - 1..(*at + 5).min(lines.len())];
            !chain.iter().any(|line| line.contains(".ghost()"))
        })
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_toolbar_toggle_filled_with_the_accent_is_found() {
    let wrong = "Button::choice(\"reply-compact\", COMPACT_CHIP, self.compact)\n    .tooltip(HINT)";
    assert_eq!(filled_toolbar_choices(wrong).len(), 1);
    let right =
        "Button::choice(\"reply-compact\", COMPACT_CHIP, on)\n    .ghost()\n    .tooltip(HINT)";
    assert!(filled_toolbar_choices(right).is_empty());
}

#[test]
fn the_composer_s_toggles_are_never_filled_with_the_accent() {
    let found = offenders(filled_toolbar_choices);
    assert!(
        found.is_empty(),
        "Send is the only primary of the composer: a toggle of its toolbar is `.ghost()`:\n{}",
        found.join("\n")
    );
}

// ── A Save beside a field is a secondary button ────────────────────────────────────────────────

/// The `Save` of a section's own field (`id("save")`) that is a primary button: a primary is the
/// one action of a view, and a tab of the project's settings holds several sections that each
/// save apart, so they are secondary (rule 1). The rules editor is both the global one, whose
/// Save may be primary, and a project's, drawn by the `Some(_)` arm of its match: only that arm
/// is held.
fn primary_saves(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<(usize, &str)> = code(source).collect();
    // The `Some(_)` arm that follows each `match self.project {`: eight lines of it.
    let project_arms: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, (_, line))| line.contains("match self.project {"))
        .filter_map(|(at, _)| {
            (at + 1..lines.len()).find(|next| lines[*next].1.starts_with("Some(_) =>"))
        })
        .collect();
    lines
        .iter()
        .enumerate()
        .filter(|(at, (_, line))| {
            line.contains("Button::primary(id(\"save\")")
                && (!source.contains("fn save_shown")
                    || project_arms.iter().any(|arm| (*arm..arm + 8).contains(at)))
        })
        .map(|(_, (at, line))| (*at, (*line).to_owned()))
        .collect()
}

#[test]
fn a_primary_save_beside_a_field_is_found() {
    assert_eq!(
        primary_saves("let save = Button::primary(id(\"save\"), \"Save\")").len(),
        1
    );
    assert!(primary_saves("Button::secondary(id(\"save\"), \"Save\")").is_empty());
}

#[test]
fn a_primary_save_in_a_project_s_rules_editor_is_found_and_the_global_one_is_not() {
    let editor = "fn save_shown() {}\nmatch self.project {\n    None => Button::primary(id(\"save\"), \"Save\"),\n    Some(_) => Button::primary(id(\"save\"), \"Save\"),\n}";
    assert_eq!(primary_saves(editor).len(), 1);
    assert_eq!(primary_saves(editor)[0].0, 4);
    let right = editor.replace("Some(_) => Button::primary", "Some(_) => Button::secondary");
    assert!(primary_saves(&right).is_empty());
}

#[test]
fn the_saves_of_a_project_s_settings_are_secondary() {
    let sections = [
        "conventions.rs",
        "message_checks.rs",
        "project_settings.rs",
        "rules.rs",
        "workspaces_folder.rs",
    ];
    let found = offenders_in(
        window()
            .into_iter()
            .filter(|(file, _)| sections.iter().any(|name| file.ends_with(name)))
            .collect(),
        primary_saves,
    );
    assert!(
        found.is_empty(),
        "a Save beside a field is `Button::secondary`:\n{}",
        found.join("\n")
    );
}

// ── The kit's rules, held on the window's sources ──────────────────────────────────────────────

/// The lines that draw a black of their own: a shadow is a role of the palette
/// (`Palette::shadow`), lighter in Light.
fn own_blacks(source: &str) -> Vec<(usize, String)> {
    code(source)
        .filter(|(_, line)| line.contains("hsla(0., 0., 0."))
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_black_of_its_own_is_found() {
    let found = own_blacks("color: hsla(0., 0., 0., 0.5),\n// hsla(0., 0., 0., 1.)\n");
    assert_eq!(found, vec![(1, "color: hsla(0., 0., 0., 0.5),".to_owned())]);
}

#[test]
fn no_overlay_of_the_window_draws_a_shadow_in_a_black_of_its_own() {
    let found = offenders(own_blacks);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The lines that open GPUI's own tooltip, which follows the pointer: a tooltip is anchored to its
/// element (`Button::tooltip`, `gpui_design_system::tipped`).
fn pointer_tooltips(source: &str) -> Vec<(usize, String)> {
    code(source)
        .filter(|(_, line)| line.contains(".tooltip(|") || line.contains(".tooltip(move |"))
        .map(|(at, line)| (at, line.to_owned()))
        .collect()
}

#[test]
fn a_tooltip_that_follows_the_pointer_is_found() {
    let found =
        pointer_tooltips(".tooltip(|w, cx| view)\n.tooltip(move |w, cx| view)\n.tooltip(\"x\")");
    assert_eq!(found.len(), 2);
}

#[test]
fn no_source_of_the_window_uses_the_tooltip_that_follows_the_pointer() {
    let found = offenders(pointer_tooltips);
    assert!(
        found.is_empty(),
        "GPUI's tooltip opens under the pointer:\n{}",
        found.join("\n")
    );
}

// ── 15. One way to choose a model ──────────────────────────────────────────────────────────────

/// A `Select` whose block (back to its function, its match arm or its closure) speaks of models,
/// a judge or a compactor: a choice of a model built outside `ModelChoice`.
fn model_selects(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<(usize, &str)> = code(source).collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, (_, line))| line.contains("Select::new("))
        .filter(|(at, _)| {
            let mut block = Vec::new();
            for (_, line) in lines[..=*at].iter().rev() {
                block.push(*line);
                if line.starts_with("fn ")
                    || line.starts_with("pub fn ")
                    || line.starts_with("pub(crate) fn ")
                    || line.contains("=> {")
                    || line.contains("= |")
                {
                    break;
                }
            }
            block.iter().any(|line| {
                let line = line.to_lowercase();
                ["model", "judge", "compactor"]
                    .iter()
                    .any(|word| line.contains(word))
            })
        })
        .map(|(_, (at, line))| (*at, (*line).to_owned()))
        .collect()
}

#[test]
fn a_select_of_models_outside_model_choice_is_refused() {
    let sample = "fn judge_section(&self) {\n\
                  let entries = self.judge_entries();\n\
                  let options = entries.iter().map(|e| SelectOption::new(e.0, e.1));\n\
                  Select::new(\"judge-model\", options, chosen)\n\
                  }\n\
                  fn sessions_section(&self) {\n\
                  Select::new(\"sessions-days\", options, chosen)\n\
                  }";
    assert_eq!(model_selects(sample).len(), 1);
    let found = offenders(model_selects);
    assert!(
        found.is_empty(),
        "a model is chosen with `ModelChoice` (rule 15), never a `Select` of its own:\n{}",
        found.join("\n")
    );
}

// ── 4. A switch's label names what it turns on ─────────────────────────────────────────────────

/// The literals of a source, as a person reads them: comments and the tests at the end of a file
/// left out, a continued line (`\` at its end) joined into one, each with the line it starts on.
fn literals(source: &str) -> Vec<(usize, String)> {
    let source = source.split("#[cfg(test)]").next().unwrap_or(source);
    let chars: Vec<char> = source.chars().collect();
    let (mut at, mut line) = (0, 1);
    let mut found = Vec::new();
    while at < chars.len() {
        match chars[at] {
            '\n' => {
                line += 1;
                at += 1;
            }
            '/' if chars.get(at + 1) == Some(&'/') => {
                while at < chars.len() && chars[at] != '\n' {
                    at += 1;
                }
            }
            // A character or a lifetime: never a text.
            '\'' => {
                at += match (chars.get(at + 1), chars.get(at + 2)) {
                    (Some('\\'), _) => 4,
                    (_, Some('\'')) => 3,
                    _ => 1,
                };
            }
            '"' => {
                let (start, mut text) = (line, String::new());
                at += 1;
                while at < chars.len() && chars[at] != '"' {
                    if chars[at] == '\\' {
                        at += 1;
                        if chars.get(at) == Some(&'\n') {
                            while chars.get(at).is_some_and(|c| c.is_whitespace()) {
                                line += usize::from(chars[at] == '\n');
                                at += 1;
                            }
                            continue;
                        }
                        text.push(' ');
                    } else {
                        line += usize::from(chars[at] == '\n');
                        text.push(chars[at]);
                    }
                    at += 1;
                }
                at += 1;
                found.push((start, text));
            }
            _ => at += 1,
        }
    }
    found
}

/// The calls that open with `marker` (`Switch::new(`), each as its own text up to the closing
/// parenthesis, with the line it starts on: what a neighbouring call says is not its own.
fn calls(source: &str, marker: &str) -> Vec<(usize, String)> {
    let source = source.split("#[cfg(test)]").next().unwrap_or(source);
    let mut found = Vec::new();
    for (start, _) in source.match_indices(marker) {
        let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
        if source[line_start..start].trim_start().starts_with("//") {
            continue;
        }
        let (mut depth, mut quoted, mut escaped, mut end) = (0, false, false, source.len());
        for (at, c) in source[start..].char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + at + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let line = source[..start].matches('\n').count() + 1;
        found.push((line, source[start..end].to_owned()));
    }
    found
}

/// A switch whose label says its state instead of what it turns on: the switch already draws
/// whether it is on.
fn state_labelled_switches(source: &str) -> Vec<(usize, String)> {
    const STATES: [&str; 5] = ["On", "Off", "Enabled", "Disabled", "Use"];
    calls(source, "Switch::new(")
        .into_iter()
        .flat_map(|(at, call)| {
            literals(&call)
                .into_iter()
                .filter(|(_, text)| STATES.contains(&text.as_str()))
                .map(move |(_, text)| (at, format!("Switch labelled {text:?}")))
        })
        .collect()
}

#[test]
fn a_switch_labelled_with_its_state_is_found() {
    let sample = "Switch::new(\n\"task-flag-serve\",\non,\n\"Enabled\",\n)\n\
                  Switch::new(id, on, \"Use\")";
    assert_eq!(state_labelled_switches(sample).len(), 2);
    let right = "Switch::new(\"x\", on, \"Review changes\").hint(\"On a diff\")";
    assert!(state_labelled_switches(right).is_empty());
}

#[test]
fn no_switch_is_labelled_on_enabled_or_use() {
    let found = offenders(state_labelled_switches);
    assert!(
        found.is_empty(),
        "a switch's label names what it turns on, never its state (rule 4):\n{}",
        found.join("\n")
    );
}

// ── 8. Words ───────────────────────────────────────────────────────────────────────────────────

/// The words of how Flottille is built, which the window never says (rule 8).
const INTERNAL: [&str; 9] = [
    "pilot",
    "pass",
    "verifier",
    "compactor",
    "daemon",
    "worktree",
    "slot",
    "handoff",
    "providers.toml",
];

/// The texts of a source that read as words (a sentence, or a capitalised label) and hold an
/// internal word, in any case, plural included. Element ids and keys of a message are not words.
fn internal_words(source: &str) -> Vec<(usize, String)> {
    literals(source)
        .into_iter()
        .filter(|(_, text)| text.contains(' ') || text.starts_with(|c: char| c.is_uppercase()))
        .filter(|(_, text)| {
            let lower = text.to_lowercase();
            lower.contains("providers.toml")
                || lower.split(|c: char| !c.is_alphanumeric()).any(|word| {
                    INTERNAL.iter().any(|internal| {
                        word == *internal
                            || word == format!("{internal}s")
                            || word == format!("{internal}es")
                    })
                })
        })
        .collect()
}

#[test]
fn an_internal_word_in_a_text_is_found() {
    let sample = "f(\"Its pilot answers at the end of its turn.\");\n\
                  f(\"cannot reach the daemon\");\n\
                  f(\"A workspace takes a worktree \\\n of it.\");\n\
                  f(\"Reads providers.toml\");\n\
                  f(\"Run the Compactors\");";
    let found = internal_words(sample);
    assert_eq!(found.len(), 5, "{found:?}");
    // Not the words: a key, a longer word, a comment, the tests of the file.
    let right = "f(\"pilot\");\nf(\"The password is kept\");\nf(\"Its lead answers\");\n\
                 // Its pilot answers\n#[cfg(test)]\nmod tests { f(\"the daemon\"); }";
    assert!(
        internal_words(right).is_empty(),
        "{:?}",
        internal_words(right)
    );
}

#[test]
fn no_internal_word_reaches_the_window() {
    let found = offenders(|source| {
        internal_words(source)
            .into_iter()
            .map(|(at, text)| (at, text.chars().take(100).collect()))
            .collect()
    });
    assert!(
        found.is_empty(),
        "the window speaks of what the user sees and does, never of how Flottille is built \
         (rule 8):\n{}",
        found.join("\n")
    );
}

// ── 8. The same gesture has the same verb ──────────────────────────────────────────────────────

/// What the window registers rather than creates: the project, a folder that exists already.
const ADDS: [&str; 3] = ["Add a project", "Add project", "Add this folder"];

/// A button or a dialog that creates and says "Add …": it says "New …" (a dialog's own button
/// only says "Add", its title says what is new).
fn adding_creators(source: &str) -> Vec<(usize, String)> {
    ["Button::", "Modal::new(", ".action("]
        .into_iter()
        .flat_map(|marker| calls(source, marker))
        .flat_map(|(at, call)| {
            literals(&call)
                .into_iter()
                .filter(|(_, text)| text.starts_with("Add ") && !ADDS.contains(&text.as_str()))
                .map(move |(_, text)| (at, text))
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[test]
fn a_creating_button_that_says_add_is_found() {
    let sample = "Button::secondary(\"add-task\", \"Add task\")\n\
                  Modal::new(\"skill-modal\", \"Add a skill\")\n\
                  Button::primary(\n\"save-entry\",\nif edit { \"Save\" } else { \"Add entry\" },\n)";
    assert_eq!(
        adding_creators(sample).len(),
        3,
        "{:?}",
        adding_creators(sample)
    );
    let right = "Button::secondary(\"add-task\", \"New task\")\n\
                 Button::primary(\"save-task\", \"Add\")\n\
                 Button::primary(\"add-project-next\", \"Add project\")";
    assert!(adding_creators(right).is_empty());
}

#[test]
fn creating_buttons_say_new() {
    let found = offenders(adding_creators);
    assert!(
        found.is_empty(),
        "the same gesture has the same verb: New step, skill, command, task, entry (rule 8):\n{}",
        found.join("\n")
    );
}
