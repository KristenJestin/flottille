//! The matching of a quick switcher: the words typed against a title and the words around it,
//! with no regard for case or accents, and where in the title they were found.
//!
//! Every word typed must be found, in the title or in its context (the project a session is in).
//! A word is found as a piece of the title, best at the start of a word of it; failing that, as
//! its letters in order, the closer together the better. It is a plain scorer: a row is shown if
//! it matches, and the better matches come first.

use std::ops::Range;

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

/// A letter without its case and its accent: the one a search compares. One character for one
/// character, so that a position in the folded text is a position in the text.
pub fn fold(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match lower {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

fn folded(text: &str) -> Vec<char> {
    text.chars().map(fold).collect()
}

/// What `query` makes of `title`, with `context` (more words that may hold some of the query's)
/// beside it. `None` when a word typed is in neither. A query with no word matches everything
/// with nothing marked.
pub fn find(query: &str, title: &str, context: &str) -> Option<Match> {
    let words: Vec<Vec<char>> = query.split_whitespace().map(folded).collect();
    let (title_chars, context_chars) = (folded(title), folded(context));
    let mut score = 0;
    let mut marked = vec![false; title_chars.len()];
    for word in &words {
        if let Some((worth, at)) = locate(word, &title_chars) {
            score += worth;
            for position in at {
                marked[position] = true;
            }
        } else if let Some((worth, _)) = locate(word, &context_chars) {
            // What is said around the title counts for less and is not marked.
            score += worth / 3;
        } else {
            return None;
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

/// How well `word` is found in `text`, and the positions of its letters there.
fn locate(word: &[char], text: &[char]) -> Option<(u32, Vec<usize>)> {
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
    // Or its letters in order, each as soon as it can be: the nearer together, the better.
    let mut at = 0;
    let mut positions = Vec::with_capacity(word.len());
    for letter in word {
        let found = text[at..].iter().position(|c| c == letter)? + at;
        positions.push(found);
        at = found + 1;
    }
    let span = positions.last()? - positions.first()? + 1;
    let slack = (span - word.len()) as u32;
    Some((100u32.saturating_sub(slack.min(90)), positions))
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
        assert!(found("lgn", "login").score > found("lgn", "lots of garbage names").score);
        assert_eq!(marked("lgn", "login"), ["l", "g", "n"]);
        assert!(find("xyz", "login", "").is_none());
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
