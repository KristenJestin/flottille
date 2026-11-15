//! Every clickable element of the window shows the pointing hand, like a link (text fields show
//! the text cursor). GPUI's test platform keeps the cursor an element asked for to itself, so no
//! test can hover a view and read it. What can be held is the way in, as `the_rule` does for
//! hovers and the kit's `tests/pointer_cursor.rs` does for its components: the kit's `Button`,
//! `ListRow`, `Switch`, ... set their own hand (held there), so a click wired on one of them needs
//! nothing here; an element the window draws itself (`div()`, a `.id(..)` of its own) sets
//! `cursor_pointer()` in the same chain, and only while it can act. An element that is no
//! control (a scrim, the mouse's back button) is listed below, with its reason.
//! A press or release (`.on_mouse_down`, `.on_mouse_up`) is found on any element, with an id or
//! without one; GPUI takes it on a bare `div()` too, and a kit component has none.

use std::fs;
use std::path::{Path, PathBuf};

/// How many lines before a click are read at most: a chain of `.child(..)` can run for a few hundred.
const BEFORE: usize = 400;
/// How many lines after a click the cursor may still come.
const AFTER: usize = 3;

/// A click that is no control: the file, the click's line and the one after it (trimmed, joined
/// by a space; one line alone does not tell two `.on_mouse_down(` apart), and why it needs no
/// hand. Each names exactly one place of its file.
const NOT_CONTROLS: &[(&str, &str, &str)] = &[
    (
        "src/session.rs",
        ".on_mouse_down(MouseButton::Left, move |_, window, cx| { TextSelection::activate_scope(scope, window, cx)",
        "begins a text selection on a message: the text view over it sets the I-beam",
    ),
    (
        "src/shell.rs",
        ".on_mouse_down( MouseButton::Navigate(NavigationDirection::Back),",
        "the mouse's back button: no pointer to show",
    ),
    (
        "src/shell.rs",
        ".on_mouse_down( MouseButton::Navigate(NavigationDirection::Forward),",
        "the mouse's forward button: no pointer to show",
    ),
    (
        "src/shell.rs",
        ".on_click(cx.listener(|this, _, _, cx| this.toggle_bell(cx))); d.when(self.bell_open, |d| {",
        "the scrim behind the bell's list: a click outside closes it, it is no control",
    ),
];

/// A source line without its trailing comment.
fn code(line: &str) -> &str {
    line.find("//").map_or(line, |at| &line[..at])
}

/// `text` with the inside of its bracket groups dropped, but the call's own `.when(..)` and
/// `.when_some(..)` kept: a pointer set there is the element's. What a `.child(..)` holds is
/// another element's.
fn flat(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    // The depth a kept `.when(..)` group opened at, while inside it.
    let mut kept: Option<usize> = None;
    for c in text.chars() {
        match c {
            '(' | '[' | '{' => {
                if kept.is_none()
                    && depth == 0
                    && (out.ends_with(".when") || out.ends_with(".when_some"))
                {
                    kept = Some(depth);
                }
                if depth == 0 || kept.is_some() {
                    out.push(c);
                }
                depth += 1;
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 || kept.is_some() {
                    out.push(c);
                }
                if kept == Some(depth) {
                    kept = None;
                }
            }
            _ if depth == 0 || kept.is_some() => out.push(c),
            _ => {}
        }
    }
    out
}

/// The chain a click is wired on: from the start of the expression (after the last `;`, `,`, `=`
/// or `=>` that is not inside brackets) to the click.
fn chain(lines: &[&str], at: usize, column: usize) -> String {
    let from = at.saturating_sub(BEFORE);
    let mut text = lines[from..at]
        .iter()
        .map(|l| code(l))
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    text.push_str(&code(lines[at])[..column]);
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut start = 0;
    for k in (0..bytes.len()).rev() {
        match bytes[k] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' if depth == 0 => {
                start = k + 1;
                break;
            }
            b'(' | b'[' | b'{' => depth -= 1,
            b';' | b',' if depth == 0 => {
                start = k + 1;
                break;
            }
            b'=' if depth == 0 => {
                let before = k.checked_sub(1).map(|b| bytes[b]);
                let after = bytes.get(k + 1).copied();
                if !matches!(before, Some(b'=' | b'!' | b'<' | b'>')) && after != Some(b'=') {
                    start = k + 1;
                    break;
                }
            }
            b'>' if depth == 0 && k > 0 && bytes[k - 1] == b'=' => {
                start = k + 1;
                break;
            }
            _ => {}
        }
    }
    text[start..].to_owned()
}

/// Whether a chain is an element the window draws itself, not a call on one of the kit's. Only
/// a click is told apart so: a mouse handler is GPUI's own, wired on whatever element the chain
/// starts with (a bare `flex_col()` or a variable too), so `uncursored` takes it as drawn here.
fn drawn_here(chain: &str) -> bool {
    let chain = flat(chain);
    ["div()", ".id()", "text_region()", "Stateful"]
        .iter()
        .any(|mark| chain.contains(mark))
}

fn sets_a_cursor(text: &str) -> bool {
    ["cursor_pointer()", "cursor_text()", "text_region()"]
        .iter()
        .any(|mark| text.contains(mark))
}

/// The clicks and presses on an element the window draws itself that set no cursor, as
/// `(line, the click's line and the next, trimmed)`.
fn without_a_cursor(file: &str, source: &str) -> Vec<(usize, String)> {
    unexcused(file, &uncursored(source))
}

fn unexcused(file: &str, found: &[(usize, String)]) -> Vec<(usize, String)> {
    found
        .iter()
        .filter(|(_, key)| {
            !NOT_CONTROLS
                .iter()
                .any(|(path, listed, _)| file.ends_with(path) && key == listed)
        })
        .cloned()
        .collect()
}

/// Every such click, listed or not.
fn uncursored(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let line = code(line);
        let Some((column, mouse)) = [".on_click(", ".on_mouse_down(", ".on_mouse_up("]
            .iter()
            .filter_map(|call| line.find(call).map(|at| (at, *call != ".on_click(")))
            .min()
        else {
            continue;
        };
        if line.contains("stop_propagation()") {
            continue;
        }
        let chain = chain(&lines, at, column);
        if !mouse && !drawn_here(&chain) {
            continue;
        }
        let after = std::iter::once(&line[column..])
            .chain(
                lines[at + 1..(at + 1 + AFTER).min(lines.len())]
                    .iter()
                    .map(|l| code(l)),
            )
            .collect::<Vec<_>>()
            .join("\n");
        if sets_a_cursor(&flat(&chain)) || sets_a_cursor(&after) {
            continue;
        }
        let key = format!(
            "{} {}",
            line[column..].trim(),
            lines.get(at + 1).map_or("", |next| next.trim())
        );
        found.push((at + 1, key.trim_end().to_owned()));
    }
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

fn window_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    assert!(files.len() > 20, "the sources were found: {}", files.len());
    files
        .into_iter()
        .map(|file| {
            let name = file.strip_prefix(root).unwrap().display().to_string();
            (name, fs::read_to_string(&file).unwrap())
        })
        .collect()
}

#[test]
fn a_clickable_element_drawn_here_without_a_cursor_is_found() {
    let source = "let row = div()\n    .id(\"row\")\n    .child(x)\n    .on_click(f);";
    assert_eq!(without_a_cursor("src/a.rs", source).len(), 1);
    let press = "div().on_mouse_down(MouseButton::Left, f)";
    assert_eq!(without_a_cursor("src/a.rs", press).len(), 1);
}

/// A mouse handler is GPUI's own: a kit component only takes `.on_click(..)`. So the element it is
/// wired on is drawn here whatever starts its chain, with an id or without one.
#[test]
fn a_mouse_handler_on_an_element_without_an_id_is_found() {
    let bare = "let row = flex_col().on_mouse_down(MouseButton::Left, f);";
    assert_eq!(without_a_cursor("src/a.rs", bare).len(), 1);
    let up = "let row = flex_col()\n    .child(x)\n    .on_mouse_up(MouseButton::Left, f);";
    assert_eq!(without_a_cursor("src/a.rs", up).len(), 1);
    let variable = "row.on_mouse_down(MouseButton::Left, f);";
    assert_eq!(without_a_cursor("src/a.rs", variable).len(), 1);
    let pointer = "let row = flex_col().cursor_pointer().on_mouse_down(MouseButton::Left, f);";
    assert!(without_a_cursor("src/a.rs", pointer).is_empty());
}

#[test]
fn a_pointer_with_its_click_a_kits_component_and_a_swallowed_click_are_not() {
    let pointer = "div()\n    .id(\"row\")\n    .cursor_pointer()\n    .child(x)\n    .on_click(f)";
    assert!(without_a_cursor("src/a.rs", pointer).is_empty());
    let after = "div().id(\"row\").on_click(f).cursor_pointer()";
    assert!(without_a_cursor("src/a.rs", after).is_empty());
    let only_with_a_handler =
        "div()\n    .when(url.is_some(), |d| d.cursor_pointer())\n    .on_click(f)";
    assert!(without_a_cursor("src/a.rs", only_with_a_handler).is_empty());
    let kit = "Button::icon(id, Icon::Copy, \"Copy\")\n    .on_click(f)";
    assert!(without_a_cursor("src/a.rs", kit).is_empty());
    let swallowed = "div().id(\"x\").on_click(|_, _, cx| cx.stop_propagation())";
    assert!(without_a_cursor("src/a.rs", swallowed).is_empty());
    let text = "text_region(id).on_mouse_down(MouseButton::Left, f)";
    assert!(without_a_cursor("src/a.rs", text).is_empty());
}

#[test]
fn a_cursor_on_a_child_is_not_the_elements() {
    let child = "div()\n    .id(\"row\")\n    .child(div().cursor_pointer())\n    .on_click(f)";
    assert_eq!(without_a_cursor("src/a.rs", child).len(), 1);
    let comment = "// div().on_click(f)\nlet x = 1;";
    assert!(without_a_cursor("src/a.rs", comment).is_empty());
}

#[test]
fn every_clickable_element_of_the_window_sets_the_pointer_cursor() {
    let mut offenders = Vec::new();
    for (name, source) in window_sources() {
        for (at, key) in without_a_cursor(&name, &source) {
            offenders.push(format!("{name}:{at}: {key}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "what takes a click shows the pointing hand (`cursor_pointer()`, only while it can act): \
         use a component of the kit, or set it; a scrim that is no control goes in NOT_CONTROLS \
         with its reason:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn every_exception_names_exactly_one_place_of_its_file() {
    let sources = window_sources();
    for (path, listed, reason) in NOT_CONTROLS {
        assert!(!reason.is_empty());
        let (_, source) = sources
            .iter()
            .find(|(name, _)| name == path)
            .unwrap_or_else(|| panic!("{path} is a source of the window"));
        let found = uncursored(source)
            .iter()
            .filter(|(_, key)| key == listed)
            .count();
        assert_eq!(
            found, 1,
            "{path} has {found} places `{listed}`: an exception names one, drop or narrow it"
        );
    }
}

/// The copy button of a code block (`copy-code-<at>`), of a message and of a path is a button of
/// the kit, which shows the pointer: none is drawn by hand from a bare element.
#[test]
fn the_copy_button_of_a_code_block_shows_the_pointer() {
    let mut copies = 0;
    for (name, source) in window_sources() {
        let lines: Vec<&str> = source.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            if !code(line).contains("Icon::Copy") {
                continue;
            }
            copies += 1;
            let around = lines[at.saturating_sub(4)..=at].join("\n");
            assert!(
                around.contains("Button::icon(") || around.contains("Button::quiet("),
                "{name}:{}: a copy button is a kit `Button`, which sets the pointer",
                at + 1
            );
        }
    }
    assert!(copies >= 3, "the copy buttons were found: {copies}");
    let session =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session.rs")).unwrap();
    assert!(
        session.contains("format!(\"copy-code-{at}\")"),
        "the code block's copy button is still named copy-code-<at>"
    );
    // And what is drawn by hand around them is held by the scan above.
    let mut offenders = Vec::new();
    for (name, source) in window_sources() {
        for (at, key) in without_a_cursor(&name, &source) {
            offenders.push(format!("{name}:{at}: {key}"));
        }
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}
