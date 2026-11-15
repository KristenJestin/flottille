//! The tokens of a message, one mechanism for both kinds (#773, #846): a picture where it was put
//! in the text (`Image 1`, its name and size) and a command `/name`. In a composer they are the
//! chips of the kit's `TokenField`; in the conversation, the same chips in a `TokenText`.
//!
//! A pasted text (#848) is a third: a chip named `pasted-text.txt` with its size, sent as its
//! marker (`[Pasted text 1: pasted-text.txt]`, `flottille_core::pasted`) with the text itself as a
//! file the agent reads.
//!
//! A command's chip is its `/name` in the text, so the daemon reads the message as typed. A
//! picture's is sent as its marker (`[Image 1: photo.png]`, `flottille_core::images::marker`),
//! which the daemon cuts the prompt at, so that the agent reads each picture where it was put,
//! and the conversation finds it again.

use std::ops::Range;

use gpui_design_system::{Piece, Token, TokenField, TokenLook};
use gpui_kit::{Context, Entity, SharedString, Window};

use crate::glyphs;

/// The id of a picture's chip in a composer: the picture it stands for.
pub fn picture_id(picture: u64) -> SharedString {
    format!("picture-{picture}").into()
}

/// The picture a chip's id stands for, when it is a picture's.
pub fn picture_of(id: &str) -> Option<u64> {
    id.strip_prefix("picture-")?.parse().ok()
}

/// How a picture's chip is drawn: its glyph, `Image <number>`, its name and its size.
pub fn picture_look(number: usize, name: &str, bytes: Option<usize>) -> TokenLook {
    let detail = match bytes {
        Some(bytes) => format!("{name} · {}", size(bytes)),
        None => name.to_owned(),
    };
    TokenLook::new(format!("Image {number}"))
        .glyph(glyphs::PICTURE)
        .detail(detail)
}

/// The id of a pasted text's chip in a composer: the text it stands for (#848).
pub fn pasted_id(text: u64) -> SharedString {
    format!("pasted-{text}").into()
}

/// The pasted text a chip's id stands for, when it is one's.
pub fn pasted_of(id: &str) -> Option<u64> {
    id.strip_prefix("pasted-")?.parse().ok()
}

/// How a pasted text's chip is drawn: its file glyph, its name and its size (none yet for a
/// message the daemon did not record).
pub fn pasted_look(name: &str, bytes: Option<usize>) -> TokenLook {
    let look = TokenLook::new(name.to_owned()).glyph(glyphs::TEXT_FILE);
    match bytes {
        Some(bytes) => look.detail(size(bytes)),
        None => look,
    }
}

/// The id of the chip of a sent message's pasted text, by the file the daemon wrote: what opens it.
pub fn sent_text_id(file: &str) -> String {
    format!("text-{file}")
}

/// The file a sent message's chip stands for, when it is a pasted text's.
pub fn sent_text_of(id: &str) -> Option<&str> {
    id.strip_prefix("text-").filter(|file| !file.is_empty())
}

/// The pasted-text markers of a message's pieces, each made a chip by `chip` (given the marker),
/// the others left as they are. A marker `chip` makes none of stays text.
pub fn text_chips(
    pieces: Vec<Piece>,
    mut chip: impl FnMut(&flottille_core::pasted::Marker) -> Option<Token>,
) -> Vec<Piece> {
    let mut out = Vec::new();
    for piece in pieces {
        let Piece::Text(text) = piece else {
            out.push(piece);
            continue;
        };
        let mut at = 0;
        for found in flottille_core::pasted::markers(&text) {
            let Some(token) = chip(&found) else {
                continue;
            };
            if found.range.start > at {
                out.push(Piece::Text(text[at..found.range.start].to_owned().into()));
            }
            out.push(Piece::Token(token));
            at = found.range.end;
        }
        if at < text.len() {
            out.push(Piece::Text(text[at..].to_owned().into()));
        }
    }
    out
}

/// The chip of a pasted text's marker in a message shown: one the daemon wrote its file for is
/// opened by that file, one still being sent is a chip with no size and nothing to open.
pub fn shown_chip(found: &flottille_core::pasted::Marker) -> Option<Token> {
    Some(match &found.file {
        Some(file) => Token::new(
            sent_text_id(file.file_name()?),
            pasted_look(&found.name, Some(file.bytes)),
        ),
        None => Token::new(
            format!("pending-text-{}", found.number),
            pasted_look(&found.name, None),
        ),
    })
}

/// A command's chip: its `/name`, which is also its text.
pub fn command(name: &str) -> Token {
    Token::new(
        format!("command-{name}"),
        TokenLook::new(format!("/{name}")),
    )
}

/// A picture's weight as a chip says it.
pub fn size(bytes: usize) -> String {
    const KIB: usize = 1024;
    match bytes {
        b if b < KIB => format!("{b} B"),
        b if b < KIB * KIB => format!("{} KiB", (b + KIB / 2) / KIB),
        b => format!("{:.1} MiB", b as f64 / (KIB * KIB) as f64),
    }
}

/// The text of `pieces` as it is sent: a picture's chip as its marker (`marker` gives it, by id,
/// with its number), any other chip as its label.
pub fn sent_text(pieces: &[Piece], mut marker: impl FnMut(&str) -> Option<String>) -> String {
    let mut text = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(part) => text.push_str(part),
            Piece::Token(token) => match marker(&token.id) {
                Some(marker) => text.push_str(&marker),
                None => text.push_str(&token.look.label),
            },
        }
    }
    text
}

/// A message's text as pieces: each picture marker a chip (`picture` gives it from the picture the
/// marker's number names, 0 first, and what it says), each command `known` a chip, the rest text.
/// Only the first marker of a number is its picture's, as the daemon places it: a copy stays text.
pub fn pieces_of(
    text: &str,
    known: impl Fn(&str) -> bool,
    mut picture: impl FnMut(usize, &flottille_core::images::Marker) -> Option<Token>,
) -> Vec<Piece> {
    let mut seen = Vec::new();
    let mut chips: Vec<(Range<usize>, Token)> = Vec::new();
    for found in flottille_core::images::markers(text) {
        if found.number == 0 || seen.contains(&found.number) {
            continue;
        }
        seen.push(found.number);
        if let Some(token) = picture(found.number - 1, &found) {
            chips.push((found.range.clone(), token));
        }
    }
    for (range, name) in flottille_core::commands::used(text, &known) {
        if !chips.iter().any(|(chip, _)| overlaps(chip, &range)) {
            chips.push((range, command(name)));
        }
    }
    chips.sort_by_key(|(range, _)| range.start);
    let mut pieces = Vec::new();
    let mut at = 0;
    for (range, token) in chips {
        if range.start > at {
            pieces.push(Piece::Text(text[at..range.start].to_owned().into()));
        }
        pieces.push(Piece::Token(token));
        at = range.end;
    }
    if at < text.len() {
        pieces.push(Piece::Text(text[at..].to_owned().into()));
    }
    pieces
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// Whether `pieces` hold a chip.
pub fn has_tokens(pieces: &[Piece]) -> bool {
    pieces.iter().any(|piece| matches!(piece, Piece::Token(_)))
}

/// Makes a chip of each command `known` the field's text names, as it is typed: a word the caret
/// is at the end of is still being typed (`/clo` may become `/close`), unless `all`. The caret
/// stays where it was; each chip unfolds as the kit inserts it.
pub fn chip_commands<T: 'static>(
    field: &Entity<TokenField>,
    known: impl Fn(&str) -> bool,
    all: bool,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let input = field.read(cx).input().clone();
    let (text, spans, caret) = {
        let input = input.read(cx);
        let spans: Vec<Range<usize>> = input.tokens().iter().map(|span| span.range()).collect();
        (input.value().to_string(), spans, input.cursor())
    };
    let words: Vec<(Range<usize>, String)> = flottille_core::commands::used(&text, &known)
        .into_iter()
        .filter(|(range, _)| all || range.end != caret)
        .filter(|(range, _)| !spans.iter().any(|span| overlaps(span, range)))
        .map(|(range, name)| (range, name.to_owned()))
        .collect();
    if words.is_empty() {
        return;
    }
    // From the last, so that the places of the others stay: a chip's text is its word.
    for (range, name) in words.into_iter().rev() {
        input.update(cx, |input, cx| input.set_selected_range(range, cx));
        field.update(cx, |field, cx| {
            field.insert(command(&name), window, cx).ok()
        });
    }
    input.update(cx, |input, cx| input.set_selected_range(caret..caret, cx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_s_markers_and_commands_are_its_chips_in_their_order() {
        let text = "see [Image 1: a.png] then /ship it, [Image 2: b.png]";
        let pieces = pieces_of(
            text,
            |name| name == "ship",
            |at, found| {
                Some(Token::new(
                    format!("p{at}"),
                    picture_look(found.number, &found.name, None),
                ))
            },
        );
        let ids: Vec<String> = pieces
            .iter()
            .map(|piece| match piece {
                Piece::Text(text) => format!("text:{text}"),
                Piece::Token(token) => format!("chip:{}", token.look.label),
            })
            .collect();
        assert_eq!(
            ids,
            [
                "text:see ",
                "chip:Image 1",
                "text: then ",
                "chip:/ship",
                "text: it, ",
                "chip:Image 2"
            ]
        );
        assert_eq!(
            sent_text(&pieces, |id| id
                .strip_prefix('p')
                .map(|at| format!("[Image {at}]"))),
            "see [Image 0] then /ship it, [Image 1]"
        );
    }

    #[test]
    fn only_the_first_marker_of_a_number_is_its_picture_s_chip() {
        let text = "[Image 2: b.png] [Image 1: a.png] [Image 1: a.png] [Image 3: c.png]";
        let pieces = pieces_of(
            text,
            |_| false,
            |picture, found| {
                (picture < 2).then(|| {
                    Token::new(
                        format!("p{picture}"),
                        picture_look(found.number, &found.name, None),
                    )
                })
            },
        );
        let ids: Vec<String> = pieces
            .iter()
            .map(|piece| match piece {
                Piece::Text(text) => format!("text:{text}"),
                Piece::Token(token) => format!("chip:{}", token.id),
            })
            .collect();
        assert_eq!(
            ids,
            [
                "chip:p1",
                "text: ",
                "chip:p0",
                "text: [Image 1: a.png] [Image 3: c.png]"
            ]
        );
    }

    #[test]
    fn a_size_is_said_in_bytes_kib_or_mib() {
        assert_eq!(size(900), "900 B");
        assert_eq!(size(340 * 1024), "340 KiB");
        assert_eq!(size(2 * 1024 * 1024 + 200 * 1024), "2.2 MiB");
    }
}
