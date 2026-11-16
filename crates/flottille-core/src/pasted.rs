//! The long texts a message carries as files (#848): a paste above the threshold is a text file
//! the agent reads with its own read tool, named in the message where the paste was. The daemon
//! and the window both read it, so there is one answer.
//!
//! The window sends `[Pasted text 1: pasted-text.txt]` where the chip was and the text as an
//! upload; the daemon writes the file and says in the message where to read it
//! (`[Pasted text 1: pasted-text.txt · 4092 bytes · read it at /path]`), which the conversation
//! reads back as the chip.

use std::ops::Range;

/// A paste with more characters than the first, or more lines than the second, is attached as a
/// file instead of filling the field.
pub const LARGE_PASTE: (usize, usize) = (4_000, 80);
/// Most texts one message carries, and the biggest one, in bytes.
pub const MAX_TEXTS: usize = 20;
pub const MAX_TEXT_BYTES: usize = 5 * 1024 * 1024;
/// The name of a pasted text, in its chip and in its marker.
pub const NAME: &str = "pasted-text.txt";

const OPEN: &str = "[Pasted text ";
const SEPARATOR: &str = " · ";
const WHERE: &str = "read it at ";

/// Whether `name` is shaped as the file of a pasted text the daemon writes: `<stamp>-text-<n>.txt`,
/// the stamp twelve lowercase hexadecimal digits and `n` from 1.
pub fn is_file_name(name: &str) -> bool {
    let Some((stamp, rest)) = name.split_once("-text-") else {
        return false;
    };
    let Some(number) = rest.strip_suffix(".txt") else {
        return false;
    };
    stamp.len() == 12
        && stamp
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && !number.is_empty()
        && !number.starts_with('0')
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// Whether a paste is large enough to be attached as a file.
pub fn is_large(text: &str) -> bool {
    text.chars().count() > LARGE_PASTE.0 || text.lines().count() > LARGE_PASTE.1
}

fn clean(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| if c == ']' || c.is_control() { ' ' } else { c })
        .collect();
    name.trim().to_owned()
}

/// What the window sends where a pasted text's chip was: `[Pasted text 1: pasted-text.txt]`,
/// numbered from 1 in the message's order.
pub fn marker(number: usize, name: &str) -> String {
    format!("{OPEN}{number}: {}]", clean(name))
}

/// What the agent reads in its place once the file is written: the name, the size and the path.
pub fn placed(number: usize, name: &str, bytes: usize, path: &str) -> String {
    format!(
        "{OPEN}{number}: {}{SEPARATOR}{bytes} bytes{SEPARATOR}{WHERE}{}]",
        clean(name),
        path.replace(']', " ")
    )
}

/// A marker found in a text by [`markers`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub range: Range<usize>,
    pub number: usize,
    pub name: String,
    /// The size and the path, once the daemon placed the file.
    pub file: Option<Placed>,
}

/// Where the daemon wrote a pasted text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub bytes: usize,
    pub path: String,
}

impl Placed {
    /// The file's name in the agent's folder: what `agent.image` asks for.
    pub fn file_name(&self) -> Option<&str> {
        self.path.rsplit('/').next().filter(|name| !name.is_empty())
    }
}

/// The markers of `text` ([`marker`] and [`placed`]), in their order.
pub fn markers(text: &str) -> Vec<Marker> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(OPEN).map(|at| from + at) {
        from = at + OPEN.len();
        let rest = &text[from..];
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let Some(after) = rest[digits..].strip_prefix(": ") else {
            continue;
        };
        let Some(close) = after
            .find([']', '\n'])
            .filter(|&close| after[close..].starts_with(']'))
        else {
            continue;
        };
        let Ok(number) = rest[..digits].parse() else {
            continue;
        };
        let inner = &after[..close];
        let end = from + digits + 2 + close + 1;
        let mut parts = inner.splitn(3, SEPARATOR);
        let (name, bytes, path) = (parts.next(), parts.next(), parts.next());
        let (name, file) = match (name, bytes, path) {
            (Some(name), Some(bytes), Some(path)) => {
                let file = bytes
                    .strip_suffix(" bytes")
                    .and_then(|bytes| bytes.parse().ok())
                    .zip(path.strip_prefix(WHERE))
                    .map(|(bytes, path)| Placed {
                        bytes,
                        path: path.to_owned(),
                    });
                match file {
                    Some(file) => (name, Some(file)),
                    None => (inner, None),
                }
            }
            _ => (inner, None),
        };
        found.push(Marker {
            range: at..end,
            number,
            name: name.to_owned(),
            file,
        });
        from = end;
    }
    found
}

/// A file written for a message's text: its name, its size and where it is.
pub struct Written {
    pub name: String,
    pub bytes: usize,
    pub path: String,
}

/// `text` with each file said where its marker is: text `k` takes the place of the first marker
/// numbered `k` the window sent (a copy of a marker is text). A file without one is marked after
/// the text, as an image is.
pub fn place(text: &str, written: &[Written]) -> String {
    let found = markers(text);
    let mut replaced: Vec<(Range<usize>, String)> = Vec::new();
    let mut after = String::new();
    for (at, file) in written.iter().enumerate() {
        let said = placed(at + 1, &file.name, file.bytes, &file.path);
        match found
            .iter()
            .find(|found| found.number == at + 1 && found.file.is_none())
        {
            Some(found) => replaced.push((found.range.clone(), said)),
            None => {
                after.push_str(if after.is_empty() && !text.ends_with('\n') {
                    "\n\n"
                } else {
                    "\n"
                });
                after.push_str(&said);
            }
        }
    }
    let mut result = text.to_owned();
    replaced.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, said) in replaced {
        result.replace_range(range, &said);
    }
    result.push_str(&after);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paste_is_large_over_four_thousand_characters_or_eighty_lines() {
        assert!(!is_large(&"a".repeat(4_000)));
        assert!(is_large(&"a".repeat(4_001)));
        assert!(!is_large(&"x\n".repeat(80)));
        assert!(is_large(&"x\n".repeat(81)));
        assert!(!is_large("one\ntwo\nthree"));
    }

    #[test]
    fn a_pasted_text_s_file_is_named_by_a_stamp_and_its_number() {
        for name in ["0a1b2c3d4e5f-text-1.txt", "0a1b2c3d4e5f-text-20.txt"] {
            assert!(is_file_name(name), "{name}");
        }
        for name in [
            "pasted-text.txt",
            "x-text-1.txt",
            "0A1B2C3D4E5F-text-1.txt",
            "0a1b2c3d4e5f-text-0.txt",
            "0a1b2c3d4e5f-text-.txt",
            "0a1b2c3d4e5f-text-1.txt.bak",
            "0a1b2c3d4e5f-1.png",
            "../0a1b2c3d4e5f-text-1.txt",
        ] {
            assert!(!is_file_name(name), "{name}");
        }
    }

    #[test]
    fn a_marker_reads_back_where_it_was_put() {
        let text = format!(
            "see {} and {}",
            marker(1, NAME),
            placed(2, NAME, 4092, "/data/images/a/x-text-1.txt")
        );
        let found = markers(&text);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!((found[0].number, found[0].name.as_str()), (1, NAME));
        assert_eq!(found[0].file, None);
        assert_eq!(found[1].number, 2);
        let file = found[1].file.clone().unwrap();
        assert_eq!(file.bytes, 4092);
        assert_eq!(file.file_name(), Some("x-text-1.txt"));
    }

    #[test]
    fn each_file_is_said_where_its_marker_is_and_one_without_after_the_text() {
        let written: Vec<Written> = (1..=3)
            .map(|n| Written {
                name: NAME.into(),
                bytes: n * 10,
                path: format!("/f/{n}.txt"),
            })
            .collect();
        let text = format!("a {} b {} c", marker(1, NAME), marker(2, NAME));
        let placed = place(&text, &written);
        assert!(
            placed.starts_with(
                "a [Pasted text 1: pasted-text.txt · 10 bytes · read it at /f/1.txt] b "
            ),
            "{placed}"
        );
        assert!(placed.contains("2: pasted-text.txt · 20 bytes · read it at /f/2.txt] c"));
        assert!(
            placed
                .ends_with("\n\n[Pasted text 3: pasted-text.txt · 30 bytes · read it at /f/3.txt]"),
            "{placed}"
        );
    }
}
