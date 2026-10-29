//! The colours a program writes in its output (`ESC [ … m`), read for the window: a command's
//! output is kept as the program wrote it, masked, and only shown turned into coloured spans or
//! stripped. Every other escape sequence (moving the cursor, clearing, a title, a link) is
//! dropped, never shown.

/// One of the eight colours of the terminal's palette, or its bright variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    /// 0 black, 1 red, 2 green, 3 yellow, 4 blue, 5 magenta, 6 cyan, 7 white.
    pub index: u8,
    pub bright: bool,
}

/// A piece of text with the style it was written in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Option<Color>,
    pub bold: bool,
}

#[derive(Clone, Copy, Default)]
struct Style {
    color: Option<Color>,
    bold: bool,
}

/// The text without any escape sequence.
pub fn strip(text: &str) -> String {
    spans(text).into_iter().map(|span| span.text).collect()
}

/// The text as spans, a new span at each change of style, with the sequences gone. A style
/// carries over lines, as it does on a terminal, and a line redrawn in place with `\r` shows
/// its last state.
pub fn spans(text: &str) -> Vec<Span> {
    let text = redrawn(text);
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let push = |current: &mut String, style: Style, spans: &mut Vec<Span>| {
        if !current.is_empty() {
            spans.push(Span {
                text: std::mem::take(current),
                color: style.color,
                bold: style.bold,
            });
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                // A control sequence: parameters, then one final byte.
                Some('[') => {
                    chars.next();
                    let mut parameters = String::new();
                    let mut last = None;
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            last = Some(c);
                            break;
                        }
                        parameters.push(c);
                    }
                    if last == Some('m') {
                        push(&mut current, style, &mut spans);
                        style = restyled(style, &parameters);
                    }
                }
                // An operating system command (a title, a link): up to BEL or ESC \.
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                // Any other: the byte after it goes with it.
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => current.push(c),
        }
    }
    push(&mut current, style, &mut spans);
    spans
}

/// Each line as the terminal ends up showing it: what follows the last `\r` of a line, which a
/// progress bar redrawn in place leaves as its last state.
fn redrawn(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(|line| match line.rsplit_once('\r') {
            Some((_, last)) if !last.is_empty() => last,
            Some((first, _)) => first.rsplit('\r').next().unwrap_or(first),
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn restyled(mut style: Style, parameters: &str) -> Style {
    let codes: Vec<u32> = if parameters.is_empty() {
        vec![0]
    } else {
        parameters
            .split(';')
            .map(|code| code.parse().unwrap_or(0))
            .collect()
    };
    let mut at = 0;
    while at < codes.len() {
        match codes[at] {
            0 => style = Style::default(),
            1 => style.bold = true,
            22 => style.bold = false,
            30..=37 => {
                style.color = Some(Color {
                    index: (codes[at] - 30) as u8,
                    bright: false,
                })
            }
            90..=97 => {
                style.color = Some(Color {
                    index: (codes[at] - 90) as u8,
                    bright: true,
                })
            }
            39 => style.color = None,
            // An extended colour (256 or true colour) is read past, not shown: the palette's
            // colours are the window's to choose.
            38 | 48 => match codes.get(at + 1) {
                Some(5) => at += 2,
                Some(2) => at += 4,
                _ => {}
            },
            _ => {}
        }
        at += 1;
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_become_spans_and_nothing_else_is_shown() {
        let found = spans("ok \u{1b}[1;31mfailed\u{1b}[0m done\u{1b}[2K\u{1b}]0;title\u{7}!");

        assert_eq!(
            found,
            [
                Span {
                    text: "ok ".into(),
                    ..Span::default()
                },
                Span {
                    text: "failed".into(),
                    color: Some(Color {
                        index: 1,
                        bright: false
                    }),
                    bold: true
                },
                Span {
                    text: " done!".into(),
                    ..Span::default()
                },
            ]
        );
    }

    #[test]
    fn stripping_leaves_the_words_and_the_lines() {
        assert_eq!(
            strip("\u{1b}[32mPASS\u{1b}[0m a\n\u{1b}[91mFAIL\u{1b}[39m b\n"),
            "PASS a\nFAIL b\n"
        );
        assert_eq!(strip("plain"), "plain");
        assert_eq!(strip("\u{1b}[38;5;196mred\u{1b}[0m"), "red");
        assert_eq!(strip("\u{1b}[38;2;1;2;3mx"), "x");
    }

    #[test]
    fn a_line_redrawn_in_place_shows_its_last_state() {
        assert_eq!(strip("10%\r50%\r100%\nnext"), "100%\nnext");
        assert_eq!(strip("a\r\nb"), "a\nb");
    }

    #[test]
    fn an_unfinished_sequence_at_the_end_shows_nothing() {
        assert_eq!(strip("text\u{1b}[3"), "text");
        assert_eq!(strip("text\u{1b}"), "text");
    }
}
