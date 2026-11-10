//! The matching of a quick switcher: the words typed against a title and the words around it,
//! with no regard for case or accents, and where in the title they were found.
//!
//! Every word typed must be found, in the title or in its context (the folder an item is in).
//! A word is found as a piece of the title, best at the start of a word of it; failing that, as
//! its letters in order, the closer together the better and never spread over more than twice
//! its length. In the context a word is found only as a piece of it. It is a plain scorer: a row is shown if
//! it matches, and the better matches come first.

use std::ops::Range;

use unicode_normalization::UnicodeNormalization as _;
use unicode_normalization::char::is_combining_mark;

/// What `query` found in a title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// The higher, the better.
    pub score: u32,
    /// The letters of the title that matched, as ranges of its characters (not bytes), in order,
    /// never touching: what a row draws in the accent.
    pub marks: Vec<Range<usize>>,
}

impl Match {
    /// The title cut at its marks: each piece with whether it matched.
    pub fn pieces(&self, title: &str) -> Vec<(String, bool)> {
        let chars: Vec<char> = title.chars().collect();
        let mut pieces = Vec::new();
        let mut at = 0;
        for mark in &self.marks {
            let (start, end) = (mark.start.min(chars.len()), mark.end.min(chars.len()));
            if start > at {
                pieces.push((chars[at..start].iter().collect(), false));
            }
            if end > start {
                pieces.push((chars[start..end].iter().collect(), true));
            }
            at = at.max(end);
        }
        if at < chars.len() {
            pieces.push((chars[at..].iter().collect(), false));
        }
        pieces
    }
}

/// The text a search compares: without case, accents or ligatures (`É` is `e`, `œ` is `oe`), each
/// letter with the position, in characters, of the one of the text it comes from. An accent
/// written as a mark of its own folds into nothing.
fn folded(text: &str) -> Vec<(char, usize)> {
    let mut found = Vec::with_capacity(text.len());
    for (at, c) in text.chars().enumerate() {
        for c in c.to_lowercase().nfd() {
            if is_combining_mark(c) {
                continue;
            }
            // The letters that are no letter and an accent, so that no decomposition reaches them.
            let spelled: &[char] = match c {
                'œ' => &['o', 'e'],
                'æ' => &['a', 'e'],
                'ß' => &['s', 's'],
                'ø' => &['o'],
                'đ' => &['d'],
                'ł' => &['l'],
                'ı' => &['i'],
                _ => {
                    found.push((c, at));
                    continue;
                }
            };
            found.extend(spelled.iter().map(|&c| (c, at)));
        }
    }
    found
}

/// What `query` makes of `title`, with `context` (more words that may hold some of the query's)
/// beside it. `None` when a word typed is in neither. A query with no word matches everything
/// with nothing marked.
pub fn find(query: &str, title: &str, context: &str) -> Option<Match> {
    let words: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|word| folded(word).into_iter().map(|(c, _)| c).collect())
        .collect();
    let title_folded = folded(title);
    let title_chars: Vec<char> = title_folded.iter().map(|(c, _)| *c).collect();
    let context_chars: Vec<char> = folded(context).into_iter().map(|(c, _)| c).collect();
    let length = title.chars().count();
    let mut score = 0;
    let mut marked = vec![false; length];
    for word in &words {
        if let Some((worth, at)) = locate(word, &title_chars, true) {
            score += worth;
            for position in at {
                marked[title_folded[position].1] = true;
            }
        } else if let Some((worth, _)) = locate(word, &context_chars, false) {
            // What is said around the title counts for less and is not marked.
            score += worth / 3;
        } else {
            return None;
        }
    }
    // An accent written after its letter goes with it.
    let kept: Vec<bool> = {
        let mut kept = vec![false; length];
        for (_, at) in &title_folded {
            kept[*at] = true;
        }
        kept
    };
    for at in 1..length {
        if !kept[at] && marked[at - 1] {
            marked[at] = true;
        }
    }
    let mut marks: Vec<Range<usize>> = Vec::new();
    for (at, on) in marked.iter().enumerate() {
        if !on {
            continue;
        }
        match marks.last_mut() {
            Some(last) if last.end == at => last.end += 1,
            _ => marks.push(at..at + 1),
        }
    }
    Some(Match { score, marks })
}

/// How well `word` is found in `text`, and the positions of its letters there. `scattered`
/// also takes its letters in order when it is not a piece of the text, the nearer together the
/// better, and never spread over more than twice its length: a long title holds the letters of
/// any short word somewhere.
fn locate(word: &[char], text: &[char], scattered: bool) -> Option<(u32, Vec<usize>)> {
    if word.is_empty() {
        return Some((0, Vec::new()));
    }
    let starts_a_word =
        |at: usize| at == 0 || !text[at - 1].is_alphanumeric() || !text[at].is_alphanumeric();
    // As a piece of the text: the earliest start of a word of it, else the earliest anywhere.
    let pieces = text
        .windows(word.len())
        .enumerate()
        .filter(|(_, w)| *w == word);
    let best = pieces
        .clone()
        .find(|(at, _)| starts_a_word(*at))
        .or_else(|| pieces.clone().next());
    if let Some((at, _)) = best {
        let worth = if at == 0 {
            400
        } else if starts_a_word(at) {
            300
        } else {
            200
        };
        return Some((worth, (at..at + word.len()).collect()));
    }
    if !scattered {
        return None;
    }
    // Each letter as soon as it can be, from each place the first one is: the nearest run wins.
    let mut best: Option<Vec<usize>> = None;
    for first in (0..text.len()).filter(|&at| text[at] == word[0]) {
        let mut positions = vec![first];
        let mut at = first + 1;
        for letter in &word[1..] {
            let Some(found) = text[at..].iter().position(|c| c == letter) else {
                break;
            };
            positions.push(found + at);
            at = found + at + 1;
        }
        if positions.len() < word.len() {
            break;
        }
        let span = |p: &[usize]| p[p.len() - 1] - p[0] + 1;
        if best.as_ref().is_none_or(|b| span(&positions) < span(b)) {
            best = Some(positions);
        }
    }
    let positions = best?;
    let span = positions[positions.len() - 1] - positions[0] + 1;
    if span > word.len() * 2 {
        return None;
    }
    let slack = (span - word.len()) as u32;
    Some((100u32.saturating_sub(slack * 10), positions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(query: &str, title: &str) -> Match {
        find(query, title, "").unwrap_or_else(|| panic!("{query:?} should find {title:?}"))
    }

    fn marked(query: &str, title: &str) -> Vec<String> {
        found(query, title)
            .pieces(title)
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(text, _)| text)
            .collect()
    }

    #[test]
    fn case_and_accents_do_not_matter() {
        assert_eq!(marked("ECRAN", "Écran de démarrage"), ["Écran"]);
        assert_eq!(marked("demarrage", "Écran de démarrage"), ["démarrage"]);
    }

    #[test]
    fn every_word_typed_must_be_found() {
        assert!(find("login fix", "Fix the login", "").is_some());
        assert!(find("login deploy", "Fix the login", "").is_none());
    }

    #[test]
    fn a_word_of_the_context_counts_but_is_not_marked() {
        let found = find("harbor login", "Fix the login", "harbor · main").unwrap();
        assert_eq!(found.pieces("Fix the login")[1], ("login".to_owned(), true));
        assert!(find("harbor login", "Fix the login", "").is_none());
    }

    #[test]
    fn the_start_of_a_word_beats_the_middle_of_one() {
        assert!(found("log", "login page").score > found("log", "catalog page").score);
        assert!(found("page", "login page").score > found("age", "login page").score);
    }

    #[test]
    fn letters_in_order_match_and_the_nearer_the_better() {
        assert!(found("lgn", "login").score > found("lgn", "logging in").score);
        assert_eq!(marked("lgn", "login"), ["l", "g", "n"]);
        assert!(find("xyz", "login", "").is_none());
    }

    #[test]
    fn letters_scattered_over_a_long_title_are_no_match() {
        let title = "Set up the staging tests, then investigate why the signup flow times out";
        assert!(find("settings", title, "").is_none());
        assert!(find("lgn", "lots of garbage names", "").is_none());
    }

    #[test]
    fn a_word_of_the_context_is_found_whole_not_as_scattered_letters() {
        assert!(find("hbr", "Fix the login", "harbor main").is_none());
        assert!(find("harb", "Fix the login", "harbor main").is_some());
    }

    #[test]
    fn combining_marks_and_ligatures_fold_too() {
        // `e` then a combining acute accent.
        assert_eq!(marked("cafe", "cafe\u{301} noir"), ["cafe\u{301}"]);
        assert_eq!(marked("oeuvre", "Chef-d'œuvre"), ["œuvre"]);
        assert_eq!(marked("aesir", "Æsir"), ["Æsir"]);
        assert_eq!(marked("strasse", "Hauptstraße"), ["straße"]);
        assert_eq!(marked("ss", "Straße"), ["ß"]);
    }

    #[test]
    fn marks_are_in_characters_not_bytes() {
        let found = found("ec", "Écran");
        assert_eq!(found.marks.len(), 1);
        assert_eq!(found.marks[0], 0..2);
    }

    #[test]
    fn nothing_typed_matches_everything_with_nothing_marked() {
        let found = find("  ", "Fix the login", "").unwrap();
        assert_eq!(found.score, 0);
        assert!(found.marks.is_empty());
    }
}
