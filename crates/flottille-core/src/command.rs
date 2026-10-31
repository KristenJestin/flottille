//! A command line split into words without a shell (`docs/reference/hemera.md` §3, "Commands").
//!
//! Flottille never hands a line to a shell: it splits it here and spawns the first word with the
//! others as its arguments. Anything a shell would read differently (a pipe, a redirection, a
//! variable, a glob) is refused rather than quoted around, so that a line means what it says.
//! The setup's `run` steps use it, and so does the agents' `run`.

/// The words of `line`, or the sentence refusing it.
///
/// Words are separated by whitespace. A quoted part (`'…'` or `"…"`) is kept whole and its quotes
/// are dropped; a backslash is an ordinary character, but refused before a quote, which a shell
/// reads as escaped. Refused outside single quotes, since a shell expands them inside double
/// quotes too: a newline, a backtick, `$(`, `${`, `$VAR`, `%VAR%`. Refused outside any quotes:
/// `|`, `&`, `;`, a redirection (`>`, `2>&1`, `<<`…), `*`, `?`, a leading `~`, a `[…]` pattern.
pub fn split_line(line: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    // A word exists once something was read for it, even an empty quoted part: `''` is a word.
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // The gate splits here the line a `sh -c` runs, so both must see the same quotes.
        if c == '\\'
            && let Some(&q) = chars.get(i + 1).filter(|q| matches!(q, '"' | '\''))
        {
            return Err(refusal(&format!("\\{q}")));
        }
        if quote == Some('\'') {
            if c == '\'' {
                quote = None;
            } else {
                word.push(c);
            }
            i += 1;
            continue;
        }
        if let Some(token) = shell_syntax(&chars, i, quote.is_some(), !in_word) {
            return Err(refusal(&token));
        }
        match c {
            '"' if quote == Some('"') => quote = None,
            '"' | '\'' if quote.is_none() => {
                quote = Some(c);
                in_word = true;
            }
            c if c.is_whitespace() && quote.is_none() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
        i += 1;
    }
    if let Some(open) = quote {
        return Err(format!(
            "the line has a {open} quote that is never closed: close it"
        ));
    }
    if in_word {
        words.push(word);
    }
    if words.is_empty() {
        return Err("the line is empty: name a program to run".into());
    }
    Ok(words)
}

/// The sentence refusing a line that holds shell syntax.
pub fn refusal(token: &str) -> String {
    format!(
        "Flottille runs a command without a shell, and “{token}” is shell syntax: put it in a \
         script of the repository and run the script."
    )
}

/// The shell syntax starting at `chars[at]`, outside single quotes. `quoted` is whether a double
/// quote is open, `word_start` whether nothing of the current word was read yet.
fn shell_syntax(chars: &[char], at: usize, quoted: bool, word_start: bool) -> Option<String> {
    let c = chars[at];
    let next = chars.get(at + 1).copied();
    let taken = |end: usize| chars[at..end].iter().collect::<String>();
    match c {
        '\n' | '\r' => return Some("a new line".into()),
        '`' => return Some(c.into()),
        '$' => match next {
            Some('(') | Some('{') => return Some(taken(at + 2)),
            Some(n) if n.is_ascii_alphabetic() || n == '_' => {
                let end = name_end(chars, at + 1);
                return Some(taken(end));
            }
            _ => {}
        },
        '%' if next.is_some_and(|n| n.is_ascii_alphabetic() || n == '_') => {
            let end = name_end(chars, at + 1);
            if chars.get(end) == Some(&'%') {
                return Some(taken(end + 1));
            }
        }
        _ => {}
    }
    if quoted {
        return None;
    }
    match c {
        ';' | '*' | '?' => Some(c.into()),
        '|' | '&' => Some(taken(if next == Some(c) { at + 2 } else { at + 1 })),
        '<' | '>' => {
            // Digits right before it are the redirected descriptor (`2>`).
            let mut start = at;
            while start > 0 && chars[start - 1].is_ascii_digit() {
                start -= 1;
            }
            let mut end = at + 1;
            if chars.get(end) == Some(&c) {
                end += 1;
            }
            if chars.get(end) == Some(&'&') {
                end += 1;
                while chars.get(end).is_some_and(char::is_ascii_digit) {
                    end += 1;
                }
                if chars.get(end) == Some(&'-') {
                    end += 1;
                }
            }
            Some(chars[start..end].iter().collect())
        }
        '~' if word_start => Some(c.into()),
        '[' => chars[at + 1..]
            .iter()
            .position(|&n| n == ']')
            .map(|close| taken(at + close + 2)),
        _ => None,
    }
}

/// Where a variable's name starting at `from` ends.
fn name_end(chars: &[char], from: usize) -> usize {
    let mut end = from;
    while chars
        .get(end)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
    {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split_line(line).unwrap()
    }

    /// The token a refused line is refused for.
    fn refused_for(line: &str) -> String {
        let sentence = split_line(line).unwrap_err();
        let token = sentence
            .split('“')
            .nth(1)
            .and_then(|rest| rest.split('”').next())
            .unwrap_or_else(|| panic!("not a shell refusal: {sentence}"));
        assert_eq!(sentence, refusal(token));
        token.to_owned()
    }

    #[test]
    fn a_line_is_split_on_whitespace() {
        assert_eq!(words("  npm   run\tbuild "), ["npm", "run", "build"]);
    }

    #[test]
    fn a_quoted_part_is_kept_whole_and_its_quotes_dropped() {
        assert_eq!(
            words(r#"git commit -m "a message" 'and | this' x"y z"w"#),
            ["git", "commit", "-m", "a message", "and | this", "xy zw"]
        );
        assert_eq!(words("echo '' \"\""), ["echo", "", ""]);
    }

    #[test]
    fn a_backslash_is_literal() {
        assert_eq!(words(r"printf a\nb c\ d"), ["printf", r"a\nb", r"c\", "d"]);
    }

    #[test]
    fn the_refusal_reads_as_documented() {
        assert_eq!(
            split_line("a | b").unwrap_err(),
            "Flottille runs a command without a shell, and “|” is shell syntax: put it in a \
             script of the repository and run the script."
        );
    }

    #[test]
    fn pipes_lists_and_substitutions_are_refused_outside_quotes() {
        for (line, token) in [
            ("a | b", "|"),
            ("a || b", "||"),
            ("a & b", "&"),
            ("a&&b", "&&"),
            ("a; b", ";"),
            ("echo `id`", "`"),
            ("echo $(id)", "$("),
            ("echo ${HOME}", "${"),
            ("echo $HOME/x", "$HOME"),
            ("echo %PATH%", "%PATH%"),
            ("echo \"$HOME\"", "$HOME"),
            ("a\nb", "a new line"),
        ] {
            assert_eq!(refused_for(line), token, "{line}");
        }
    }

    #[test]
    fn redirections_are_refused_with_their_descriptors() {
        for (line, token) in [
            ("a > out", ">"),
            ("a >> out", ">>"),
            ("a < in", "<"),
            ("a <<EOF", "<<"),
            ("a 2>&1", "2>&1"),
            ("a 2>err", "2>"),
            ("a >&2", ">&2"),
            ("a 3>&-", "3>&-"),
        ] {
            assert_eq!(refused_for(line), token, "{line}");
        }
    }

    #[test]
    fn globs_and_a_leading_tilde_are_refused_outside_any_quotes() {
        for (line, token) in [
            ("ls *.rs", "*"),
            ("ls a?", "?"),
            ("cd ~/x", "~"),
            ("ls [ab].txt", "[ab]"),
        ] {
            assert_eq!(refused_for(line), token, "{line}");
        }
        assert_eq!(
            words("ls '*.rs' \"a?\" a~b \"~\""),
            ["ls", "*.rs", "a?", "a~b", "~"]
        );
        assert_eq!(words("test [ x"), ["test", "[", "x"]);
    }

    #[test]
    fn single_quotes_keep_everything_as_it_is() {
        assert_eq!(
            words("echo '$HOME | ; > `x` ${y} %Z%'"),
            ["echo", "$HOME | ; > `x` ${y} %Z%"]
        );
    }

    #[test]
    fn a_pipe_inside_double_quotes_is_passed_as_text() {
        assert_eq!(
            words(r#"echo "a | b && c > d" "x;y" "2>&1 <in""#),
            ["echo", "a | b && c > d", "x;y", "2>&1 <in"]
        );
        assert_eq!(
            words(r#"git commit -m "fix: a -> b""#),
            ["git", "commit", "-m", "fix: a -> b"]
        );
        assert_eq!(
            words(r#"git log --grep "a|b""#),
            ["git", "log", "--grep", "a|b"]
        );
    }

    #[test]
    fn and_and_a_redirection_inside_single_quotes_are_passed_as_text() {
        assert_eq!(words("echo 'a && b > c'"), ["echo", "a && b > c"]);
    }

    #[test]
    fn an_unquoted_pipe_is_refused() {
        assert_eq!(refused_for(r#"echo "a" | cat"#), "|");
        assert_eq!(refused_for(r#"echo "a"|"b""#), "|");
    }

    #[test]
    fn an_unquoted_substitution_is_refused() {
        assert_eq!(refused_for("echo $(id)"), "$(");
        assert_eq!(refused_for(r#"echo "a"$(id)"#), "$(");
    }

    #[test]
    fn what_a_shell_expands_inside_double_quotes_is_refused() {
        for (line, token) in [
            (r#"echo "$(id)""#, "$("),
            (r#"echo "a `id` b""#, "`"),
            (r#"echo "${HOME}""#, "${"),
            (r#"echo "$HOME""#, "$HOME"),
            (r#"echo "%PATH%""#, "%PATH%"),
            ("echo \"a\nb\"", "a new line"),
        ] {
            assert_eq!(refused_for(line), token, "{line}");
        }
        assert_eq!(
            words("echo '$(id) `id` $HOME'"),
            ["echo", "$(id) `id` $HOME"]
        );
    }

    #[test]
    fn a_quoted_glob_is_not_expanded() {
        assert_eq!(
            words(r#"ls "*.rs" '?' "[ab]" "~/x""#),
            ["ls", "*.rs", "?", "[ab]", "~/x"]
        );
    }

    #[test]
    fn a_backslash_before_a_quote_is_refused() {
        // A shell reads it as a quote escaped, so the line would not split as it does here.
        for (line, token) in [
            (r#"echo \" | rm x \""#, r#"\""#),
            (r#"echo "a\" | rm x \"""#, r#"\""#),
            (r"echo \' | rm x \'", r"\'"),
            (r"echo 'a\' '| rm x '\' '", r"\'"),
        ] {
            assert_eq!(refused_for(line), token, "{line}");
        }
    }

    #[test]
    fn a_dollar_or_a_percent_alone_is_text() {
        assert_eq!(
            words("echo $ 5% $1 100%"),
            ["echo", "$", "5%", "$1", "100%"]
        );
    }

    #[test]
    fn an_unclosed_quote_and_an_empty_line_are_refused() {
        assert!(split_line("echo 'a").unwrap_err().contains("never closed"));
        assert!(split_line("   ").unwrap_err().contains("empty"));
    }
}
