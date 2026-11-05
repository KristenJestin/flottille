//! A `run` line that only prints a part of a file, answered as the `fs_read` it is
//! (`docs/tools.md`, "Running"): `sed -n <range>p <file>`, `head [-n N] <file>`,
//! `tail [-n N] <file>` and `cat <file>`.
//!
//! Only these exact shapes are read, from the words the no-shell splitter makes of the line, and
//! only when the gate lets the `fs_read` of that file through at once: a path outside the
//! workspace, a sensitive place or a link leading out is left to what a `run` is today.

use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::files::{self, Lines};
use crate::gate::Verdict;
use crate::mcp::{self, Grant};

/// The `fs_read` a `run` call is, with the line that says so to the agent; `None` for any call
/// that is not such a read.
pub fn as_read(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    args: &Value,
) -> Option<(Value, String)> {
    if tool != "run" || !grant.tools.contains(&"fs_read") {
        return None;
    }
    // A task, a timeout or a notification makes it a run, not a read.
    let object = args.as_object()?;
    if object
        .keys()
        .any(|key| !["line", "repository", "why"].contains(&key.as_str()))
    {
        return None;
    }
    let words = flottille_core::command::split_line(object.get("line")?.as_str()?).ok()?;
    let (path, lines) = read_of(&words)?;
    let mut read = json!({"path": path});
    if let Some(repository) = object.get("repository") {
        read["repository"] = repository.clone();
    }
    // The file is opened only once the gate lets a read of it through.
    if !matches!(
        mcp::classify(daemon, grant, "fs_read", &read),
        Verdict::Allow
    ) {
        return None;
    }
    let resolved = files::reaches(&grant.place, "fs_read", &read)
        .into_iter()
        .next()?;
    let (offset, length) = files::line_span(&resolved, lines).ok()??;
    let repository = match &read["repository"] {
        Value::Null => String::new(),
        repository => format!(r#", "repository": {repository}"#),
    };
    let said = format!(
        r#"read with fs_read {{"path": {}{repository}, "range": {{"offset": {offset}, "length": {length}}}}}: use it directly"#,
        Value::from(path)
    );
    read["range"] = json!({"offset": offset, "length": length});
    Some((read, said))
}

/// The file and the lines a command line prints, when it is one of the shapes and nothing else.
fn read_of(words: &[String]) -> Option<(&str, Lines)> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let (path, lines) = match words[..] {
        ["cat", path] => (
            path,
            Lines::From {
                first: 1,
                last: None,
            },
        ),
        ["sed", "-n", range, path] => (path, sed_range(range)?),
        ["head", path] => (
            path,
            Lines::From {
                first: 1,
                last: Some(10),
            },
        ),
        ["head", "-n", count, path] => (
            path,
            Lines::From {
                first: 1,
                last: Some(number(count)?),
            },
        ),
        ["tail", path] => (path, Lines::Last(10)),
        ["tail", "-n", count, path] => (path, Lines::Last(number(count)?)),
        _ => return None,
    };
    // A word that starts with `-` is an option, and `~` is the home folder to a file tool.
    (!path.starts_with(['-', '~'])).then_some((path, lines))
}

/// `5p`, `5,20p` or `5,$p`: from a first line to a last, or to the end.
fn sed_range(range: &str) -> Option<Lines> {
    let range = range.strip_suffix('p')?;
    let (first, last) = match range.split_once(',') {
        Some((first, "$")) => (number(first)?, None),
        Some((first, last)) => (number(first)?, Some(number(last)?)),
        None => (number(range)?, Some(number(range)?)),
    };
    (last.is_none_or(|last| first <= last)).then_some(Lines::From { first, last })
}

/// A count of lines or a line's number: digits alone, one at least.
fn number(text: &str) -> Option<u64> {
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(line: &str) -> Option<(String, Vec<u64>)> {
        let words = flottille_core::command::split_line(line).unwrap();
        read_of(&words).map(|(path, lines)| {
            let numbers = match lines {
                Lines::From { first, last } => vec![first, last.unwrap_or(0)],
                Lines::Last(n) => vec![n],
            };
            (path.to_owned(), numbers)
        })
    }

    #[test]
    fn the_shapes_that_only_print_a_part_of_a_file_are_read() {
        let shapes = [
            (
                "sed -n 295,316p docs/README.md",
                "docs/README.md",
                vec![295, 316],
            ),
            ("sed -n 5p a", "a", vec![5, 5]),
            ("sed -n '5,$p' a", "a", vec![5, 0]),
            ("head a", "a", vec![1, 10]),
            ("head -n 3 a", "a", vec![1, 3]),
            ("tail a", "a", vec![10]),
            ("tail -n 3 a", "a", vec![3]),
            ("cat a", "a", vec![1, 0]),
        ];
        for (line, path, lines) in shapes {
            assert_eq!(read(line), Some((path.to_owned(), lines)), "{line}");
        }
    }

    #[test]
    fn anything_else_is_not_a_read() {
        for line in [
            "sed -i s/a/b/ a",
            "sed -n 5p",
            "sed -n 5,3p a",
            "sed -n 0p a",
            "sed -n '5,$p' a b",
            "sed -n /x/p a",
            "sed -n 5d a",
            "sed -n +5p a",
            "sed 5p a",
            "head -c 5 a",
            "head -n -3 a",
            "head -n 0 a",
            "head -n 3 a b",
            "tail -n +3 a",
            "tail -f a",
            "cat -n a",
            "cat a b",
            "cat -- a",
            "cat '~/a'",
            "cat '-'",
            "/usr/bin/cat a",
            "cat",
        ] {
            assert_eq!(read(line), None, "{line}");
        }
    }
}
