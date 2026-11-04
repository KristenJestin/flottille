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
/// The sentence says what to write instead and, when the words meant are certain, gives the line
/// written so that it passes.
pub fn split_line(line: &str) -> Result<Vec<String>, String> {
    split(line).map_err(|refused| match refused {
        Refused::Syntax(token) => with_rewrite(&token, line),
        Refused::Malformed(sentence) => sentence,
    })
}

enum Refused {
    /// The shell syntax the line holds.
    Syntax(String),
    /// Why the line cannot be read at all.
    Malformed(String),
}

fn split(line: &str) -> Result<Vec<String>, Refused> {
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
            return Err(Refused::Syntax(format!("\\{q}")));
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
            return Err(Refused::Syntax(token));
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
        return Err(Refused::Malformed(format!(
            "the line has a {open} quote that is never closed: close it"
        )));
    }
    if in_word {
        words.push(word);
    }
    if words.is_empty() {
        return Err(Refused::Malformed(
            "the line is empty: name a program to run".into(),
        ));
    }
    Ok(words)
}

/// The sentence refusing a line that holds shell syntax: what to write instead.
pub fn refusal(token: &str) -> String {
    const SCRIPT: &str = "or put the line in a script of the repository and run the script";
    let instead = match token {
        "\\\"" => "put the word in single quotes, where a double quote needs no backslash".into(),
        "\\'" => "put the word in double quotes, where a single quote needs no backslash".into(),
        "*" | "?" | "~" => QUOTED.into(),
        t if t.starts_with('[') => QUOTED.into(),
        t if t.starts_with('%') => {
            "put the word in single quotes, where nothing is expanded".into()
        }
        "$(" | "`" => format!(
            "a shell would run a command there and none does here: run that command with its \
             own call to run and write its result in the line, {SCRIPT}"
        ),
        t if t.starts_with('$') => format!(
            "a shell would put the value of a variable there and none does here: write the value \
             itself, {SCRIPT}"
        ),
        "|" => format!(
            "run starts one program and hands its output to no other: read a file with fs_read, \
             search the project with search, {SCRIPT}"
        ),
        t if t.contains(['<', '>']) => format!(
            "run starts one program and keeps its output for you to read with run_status: write a \
             file with fs_write, read one with fs_read, {SCRIPT}"
        ),
        _ => format!("run starts one program: run each one with its own call, {SCRIPT}"),
    };
    format!("Flottille runs a command without a shell, and “{token}” is shell syntax: {instead}.")
}

const QUOTED: &str = "put the word in single quotes, so that the program receives it as it is: no shell expands \
     it here";

/// The refusal of `line` for `token`, with the line that passes when it is certain.
fn with_rewrite(token: &str, line: &str) -> String {
    let sentence = refusal(token);
    match rewritten(line) {
        Some(Rewrite::Line(passes)) => format!("{sentence} This line passes: {passes}"),
        Some(Rewrite::SingleQuote(word)) => format!(
            "{sentence} “{word}” holds a single quote, which no single-quoted word can hold: it \
             cannot be one word, split it."
        ),
        None => sentence,
    }
}

enum Rewrite {
    /// The same line, each refused word in single quotes.
    Line(String),
    /// A word that would need single quotes but holds one.
    SingleQuote(String),
}

/// `line` with each refused word written in single quotes, only where the words meant are
/// certain: a shell would read them as plain text but for a glob, a `~` or `[…]` meant for the
/// program. Syntax meant as syntax (a pipe, a substitution…) is never quoted around.
fn rewritten(line: &str) -> Option<Rewrite> {
    let chars: Vec<char> = line.chars().collect();
    let mut passes = String::new();
    let mut meant = Vec::new();
    let mut single_quote = None;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            passes.push(chars[i]);
            i += 1;
            continue;
        }
        let end = word_end(&chars, i)?;
        let word: String = chars[i..end].iter().collect();
        match split(&word) {
            Ok(words) => {
                passes.push_str(&word);
                meant.extend(words);
            }
            Err(_) => {
                let text = plain_text(&chars[i..end])?;
                if text.contains('\'') {
                    single_quote.get_or_insert(word);
                }
                passes.push_str(&format!("'{text}'"));
                meant.push(text);
            }
        }
        i = end;
    }
    if let Some(word) = single_quote {
        return Some(Rewrite::SingleQuote(word));
    }
    // What a shell and the check would read differently elsewhere in the line.
    (split(&passes).ok()? == meant).then_some(Rewrite::Line(passes))
}

/// Where the word starting at `from` ends, read as a shell reads it; `None` for a quote never
/// closed.
fn word_end(chars: &[char], from: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() && !chars[i].is_whitespace() {
        match chars[i] {
            '\'' => i += chars[i + 1..].iter().position(|&c| c == '\'')? + 1,
            '"' => {
                i += 1;
                while *chars.get(i)? != '"' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
            }
            '\\' => i += 1,
            _ => {}
        }
        i += 1;
    }
    Some(i.min(chars.len()))
}

/// The text a shell gives the program for `word`, a glob, `~` or `[…]` taken as text; `None`
/// where a shell would read anything else as syntax.
fn plain_text(word: &[char]) -> Option<String> {
    let mut text = String::new();
    let mut i = 0;
    while i < word.len() {
        match word[i] {
            '\'' => {
                let close = i + 1 + word[i + 1..].iter().position(|&c| c == '\'')?;
                text.extend(&word[i + 1..close]);
                i = close;
            }
            '"' => {
                i += 1;
                while word[i] != '"' {
                    match (word[i], word.get(i + 1).copied()) {
                        ('\\', Some(c @ ('"' | '\\'))) => {
                            text.push(c);
                            i += 1;
                        }
                        ('\\', Some('$' | '`' | '\n')) | ('`', _) => return None,
                        ('$', Some(n)) if n.is_ascii_alphanumeric() || "_{(@*#?-$!".contains(n) => {
                            return None;
                        }
                        (c, _) => text.push(c),
                    }
                    i += 1;
                }
            }
            c if c.is_ascii_alphanumeric() || "-_./:=+,@^*?~[]".contains(c) => text.push(c),
            c if !c.is_ascii() && !c.is_whitespace() => text.push(c),
            _ => return None,
        }
        i += 1;
    }
    (!text.contains(['\n', '\r'])).then_some(text)
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
        assert!(sentence.starts_with(&refusal(token)), "{sentence}");
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
            "Flottille runs a command without a shell, and “|” is shell syntax: run starts one \
             program and hands its output to no other: read a file with fs_read, search the \
             project with search, or put the line in a script of the repository and run the \
             script."
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

    /// The line a refusal gives whole, if any.
    fn suggested(line: &str) -> Option<String> {
        let sentence = split_line(line).unwrap_err();
        sentence
            .split_once(" This line passes: ")
            .map(|(_, line)| line.to_owned())
    }

    #[test]
    fn a_backslash_quote_inside_double_quotes_is_refused_with_the_single_quoted_line() {
        let line = r#"git grep -E "\"key\": *[0-9]+" -- src"#;
        let sentence = split_line(line).unwrap_err();
        assert!(sentence.starts_with(&refusal(r#"\""#)), "{sentence}");
        assert!(sentence.contains("single quotes"), "{sentence}");
        assert_eq!(
            suggested(line).as_deref(),
            Some(r#"git grep -E '"key": *[0-9]+' -- src"#)
        );
        // A single quote too: no single-quoted word holds it.
        let line = r#"git grep "it's \"x\"""#;
        let sentence = split_line(line).unwrap_err();
        assert_eq!(suggested(line), None, "{sentence}");
        assert!(
            sentence.contains("cannot be one word") && sentence.contains("split it"),
            "{sentence}"
        );
    }

    #[test]
    fn a_git_format_inside_double_quotes_is_refused_with_the_single_quoted_line() {
        let line = r#"git log --format="%h%x20%s" -5"#;
        assert_eq!(refused_for(line), "%h%");
        assert!(split_line(line).unwrap_err().contains("single quotes"));
        assert_eq!(
            suggested(line).as_deref(),
            Some("git log '--format=%h%x20%s' -5")
        );
    }

    #[test]
    fn an_unquoted_glob_is_refused_with_the_quoted_word() {
        for (line, token, whole) in [
            (
                "grep -rn --include=*.ts foo src",
                "*",
                "grep -rn '--include=*.ts' foo src",
            ),
            (
                "ng test --include src/**/*.spec.ts",
                "*",
                "ng test --include 'src/**/*.spec.ts'",
            ),
            ("ls a?", "?", "ls 'a?'"),
            ("ls ~/notes", "~", "ls '~/notes'"),
            ("ls [ab].txt", "[ab]", "ls '[ab].txt'"),
        ] {
            assert_eq!(refused_for(line), token, "{line}");
            let sentence = split_line(line).unwrap_err();
            assert!(
                sentence.contains("the program receives it as it is: no shell expands it here"),
                "{sentence}"
            );
            assert_eq!(suggested(line).as_deref(), Some(whole), "{line}");
        }
    }

    #[test]
    fn the_suggested_line_passes_the_check_and_means_the_same_words() {
        for (line, meant) in [
            (
                r#"git grep -E "\"key\": *[0-9]+" -- src"#,
                &["git", "grep", "-E", r#""key": *[0-9]+"#, "--", "src"][..],
            ),
            (
                r#"rg "^\"name\"$" pkg/a.json"#,
                &["rg", r#"^"name"$"#, "pkg/a.json"],
            ),
            (r#"grep -E "a\\b\"c" x"#, &["grep", "-E", r#"a\b"c"#, "x"]),
            (
                r#"git log --format="%h%x20%s" -5"#,
                &["git", "log", "--format=%h%x20%s", "-5"],
            ),
            (
                r#"git log --format="%h %an%n" "a b""#,
                &["git", "log", "--format=%h %an%n", "a b"],
            ),
            (
                "grep -rn --include=*.ts foo src",
                &["grep", "-rn", "--include=*.ts", "foo", "src"],
            ),
            (
                "ng test --include src/**/*.spec.ts",
                &["ng", "test", "--include", "src/**/*.spec.ts"],
            ),
            (
                r#"find . -name *.rs -newer "a\"b""#,
                &["find", ".", "-name", "*.rs", "-newer", r#"a"b"#],
            ),
            ("ls  a?\t~/x", &["ls", "a?", "~/x"]),
        ] {
            let whole = suggested(line).unwrap_or_else(|| panic!("no line for {line}"));
            assert_eq!(words(&whole), meant, "{line} -> {whole}");
        }
        // Where the words meant are not certain, no line is given.
        for line in [
            r#"grep "\$x\"" f"#,
            r#"grep "$x\"" f"#,
            r"ls a\ b*",
            "ls *.{ts,js}",
            r#"ls *.rs "C:\dir\\""#,
            "test [ x ]",
            "ls #*",
        ] {
            assert_eq!(suggested(line), None, "{line}");
        }
    }

    #[test]
    fn a_pipe_is_refused_with_what_to_use_instead() {
        let sentence = split_line(r#"cat a.txt | grep "\"x\"""#).unwrap_err();
        assert!(sentence.starts_with(&refusal("|")), "{sentence}");
        for tool in ["fs_read", "search", "script"] {
            assert!(sentence.contains(tool), "{tool}: {sentence}");
        }
        for (line, said) in [
            ("make && make test", "its own call"),
            ("a; b", "its own call"),
            ("cargo test > out.txt", "fs_write"),
            ("cargo test 2>&1", "run_status"),
            ("echo $(date)", "its own call to run"),
            (r#"cd "$HOME""#, "the value itself"),
        ] {
            let sentence = split_line(line).unwrap_err();
            assert!(sentence.contains(said), "{line}: {sentence}");
            assert_eq!(suggested(line), None, "{line}");
            assert!(!sentence.contains('\''), "{line}: {sentence}");
        }
    }

    #[test]
    fn an_unclosed_quote_and_an_empty_line_are_refused() {
        assert!(split_line("echo 'a").unwrap_err().contains("never closed"));
        assert!(split_line("   ").unwrap_err().contains("empty"));
    }
}
