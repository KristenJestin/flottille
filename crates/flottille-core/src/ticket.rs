//! A ticket's reference: what a workspace is linked to (`docs/tickets.md`). `#123` is an issue of
//! the repository's GitHub, `ABC-123` a work item of Jira. The reference is parsed once, when it is
//! typed, so that what reaches a tracker's command line is always a number or a key and never an
//! option.

/// The tracker a ticket is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracker {
    GitHub,
    Jira,
}

impl Tracker {
    pub fn as_str(self) -> &'static str {
        match self {
            Tracker::GitHub => "github",
            Tracker::Jira => "jira",
        }
    }
}

/// A ticket's reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// An issue of a GitHub repository.
    Issue(u64),
    /// A Jira key, upper case: `ABC-123`.
    Jira(String),
}

impl Key {
    /// `#123`, `123` (a shell takes a word starting with `#` for a comment) or `abc-123`.
    pub fn parse(text: &str) -> Result<Key, String> {
        let text = text.trim();
        let number = text.strip_prefix('#').unwrap_or(text);
        if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
            return match number.parse::<u64>() {
                Ok(n) if n > 0 => Ok(Key::Issue(n)),
                _ => Err(format!("{text} is not an issue number")),
            };
        }
        if let Some((project, number)) = text.rsplit_once('-')
            && project.len() >= 2
            && project.starts_with(|c: char| c.is_ascii_alphabetic())
            && project
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
        {
            return Ok(Key::Jira(text.to_ascii_uppercase()));
        }
        Err(format!(
            "{text} is not a ticket: write #123 (an issue of the repository's GitHub) or a \
             Jira key such as ABC-123"
        ))
    }

    pub fn tracker(&self) -> Tracker {
        match self {
            Key::Issue(_) => Tracker::GitHub,
            Key::Jira(_) => Tracker::Jira,
        }
    }

    /// As it is kept and shown: `#123`, `ABC-123`.
    pub fn text(&self) -> String {
        match self {
            Key::Issue(number) => format!("#{number}"),
            Key::Jira(key) => key.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_number_with_or_without_its_hash_and_a_jira_key_are_tickets() {
        assert_eq!(Key::parse("#123"), Ok(Key::Issue(123)));
        assert_eq!(Key::parse(" 7 "), Ok(Key::Issue(7)));
        assert_eq!(Key::parse("ABC-123"), Ok(Key::Jira("ABC-123".into())));
        assert_eq!(Key::parse("ab_c2-9"), Ok(Key::Jira("AB_C2-9".into())));
        assert_eq!(Key::parse("#123").unwrap().text(), "#123");
        assert_eq!(Key::parse("abc-1").unwrap().tracker(), Tracker::Jira);
        assert_eq!(Key::parse("#1").unwrap().tracker(), Tracker::GitHub);
    }

    #[test]
    fn anything_else_is_refused_and_never_reaches_a_command_line() {
        for text in [
            "",
            "#",
            "#0",
            "0",
            "-1",
            "--web",
            "-h",
            "A-1",
            "1ABC-2",
            "ABC-",
            "ABC",
            "ABC-1 --web",
            "ABC-1;ls",
            "#12x",
            "ABC--1",
            "#99999999999999999999",
        ] {
            assert!(Key::parse(text).is_err(), "{text:?}");
        }
    }
}
