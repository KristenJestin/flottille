//! Secrets masked out of what Flottille keeps or shows: the output of a command, a failure.

use std::collections::BTreeMap;
use std::ffi::OsString;

const MASK: &str = "[masked]";

/// Parts of a variable's name that say its value is a secret (`PASS` covers `PASSWORD` and
/// `PASSWD`, `KEY` covers `API_KEY` and `PRIVATE_KEY`).
const SECRET_NAMES: [&str; 6] = ["TOKEN", "SECRET", "PASS", "CREDENTIAL", "KEY", "AUTH"];

/// How the tokens of the usual forges, registries and providers start.
const TOKEN_PREFIXES: [&str; 14] = [
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "sk-",
    "AKIA",
    "npm_",
    "pypi-",
    "AIza",
];

/// `text` with the secrets it holds replaced by `[masked]`: the values of the variables of `env`
/// named like secrets, private key blocks, the password of a URL, and anything shaped like a known
/// kind of token or a JWT.
pub fn mask(text: &str, env: &BTreeMap<OsString, OsString>) -> String {
    mask_also(text, env, &[])
}

/// `mask`, and every one of `values` masked too: the values of the files holding secrets of the
/// place a command ran in (`file_values`).
pub fn mask_also(text: &str, env: &BTreeMap<OsString, OsString>, values: &[String]) -> String {
    Secrets::new(env, values).mask(text)
}

/// The secret values `mask_also` knows, found once for all the texts masked with them.
pub struct Secrets {
    /// The longest first: a secret holding a shorter one is masked whole.
    values: Vec<String>,
}

/// The most of a word (no whitespace) `settled` holds back when no secret crosses that bound: a
/// token, a JWT, a URL's password or a private key block that does is held whole, however long.
const HELD_WORD: usize = 4096;

impl Secrets {
    /// The values of the variables of `env` named like secrets, and `values`.
    pub fn new(env: &BTreeMap<OsString, OsString>, values: &[String]) -> Self {
        let mut values: Vec<String> = values
            .iter()
            .filter(|value| value.len() >= 6)
            .cloned()
            .chain(
                env.iter()
                    .filter_map(|(name, value)| Some((name.to_str()?, value.to_str()?)))
                    .filter(|(name, value)| {
                        let upper = name.to_ascii_uppercase();
                        // A short value is too likely to be an ordinary word; git's
                        // `GIT_CONFIG_KEY_<n>` names a setting, its value is in `GIT_CONFIG_VALUE_<n>`.
                        value.len() >= 6
                            && SECRET_NAMES.iter().any(|part| upper.contains(part))
                            && !upper.starts_with("GIT_CONFIG_KEY_")
                    })
                    .map(|(_, value)| value.to_owned()),
            )
            .collect();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        Secrets { values }
    }

    /// `text` with the secrets it holds replaced by `[masked]`, as `mask_also` does.
    pub fn mask(&self, text: &str) -> String {
        let mut masked = text.to_owned();
        for value in &self.values {
            masked = masked.replace(value.as_str(), MASK);
        }
        mask_tokens(&mask_private_keys(&masked))
    }

    /// How much of `text`, a message still streaming, is settled: masked alone, it reads as it will
    /// once the message is whole, so it can be stored and shown now. The rest may still be part of
    /// a secret the next pieces complete: the last word, as long as the longest value known at
    /// least, and a private key block until its end line.
    pub fn settled(&self, text: &str) -> usize {
        let longest = self.values.first().map_or(0, String::len);
        let floor = |at: usize| {
            (0..=at)
                .rev()
                .find(|&i| text.is_char_boundary(i))
                .unwrap_or(0)
        };
        let mut cut = floor(text.len().saturating_sub(longest));
        // Tokens, JWTs and a URL's password end at a whitespace: after the last one, unless the
        // word it starts is too long to hold.
        let word = text[..cut].rfind(char::is_whitespace).map_or(0, |at| {
            at + text[at..].chars().next().map_or(1, char::len_utf8)
        });
        cut = cut.min(word.max(floor(text.len().saturating_sub(longest.max(HELD_WORD)))));
        // Cut inside a word too long to hold, the cut must not split a token: a token is held or
        // settled whole.
        let tokens = tokens(text);
        loop {
            let before = cut;
            for value in &self.values {
                for (at, _) in text.match_indices(value.as_str()) {
                    if at < cut && cut < at + value.len() {
                        cut = at;
                    }
                }
            }
            for &(at, end) in &tokens {
                if at < cut && cut < end {
                    cut = at;
                }
            }
            // Nor start one inside a word: alone, what follows the cut would read as a token.
            if let Some(last) = text[..cut].chars().next_back()
                && token_char(last)
                && token_start(&text[cut..])
            {
                cut -= last.len_utf8();
            }
            cut = cut.min(private_key(text, cut));
            if cut == before {
                return cut;
            }
        }
    }
}

/// Where the private key block `text` may hold before `cut` starts, a block masked to its end
/// line (`mask_private_keys`), or `cut`. A `-----BEGIN ` whose label is still coming may start
/// one; a label of another kind does not.
fn private_key(text: &str, cut: usize) -> usize {
    if let Some((at, _)) = text
        .match_indices("-----BEGIN ")
        .find(|&(at, begin)| at < cut && cut < at + begin.len())
    {
        return at;
    }
    let mut from = 0;
    while let Some(begin) = text[from..cut].find("-----BEGIN ").map(|at| from + at) {
        let header = &text[begin + "-----BEGIN ".len()..];
        let Some(close) = header.find("-----") else {
            let label = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || c == ' ';
            // Its closing dashes may be coming too.
            let label = header.trim_end_matches('-').chars().all(label);
            return if label { begin } else { cut };
        };
        from = begin + "-----BEGIN ".len();
        if !header[..close].ends_with("PRIVATE KEY") {
            continue;
        }
        let end = header.find("-----END ").and_then(|end| {
            let footer = &header[end + "-----END ".len()..];
            footer
                .find("-----")
                .map(|close| text.len() - footer.len() + close + 5)
        });
        match end {
            Some(end) if end <= cut => from = end,
            _ => return begin,
        }
    }
    cut
}

/// Where each token, JWT or URL's password of `text` is, or may be once the text goes on, from
/// its first byte to its last: what `mask_tokens` would mask, and its start before it is whole.
fn tokens(text: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut previous = None;
    for (at, c) in text.char_indices() {
        let rest = &text[at..];
        if let Some(after) = rest.strip_prefix("://") {
            // To the character after the user's part, which says whether it is one.
            let end = after
                .find(|c: char| matches!(c, '@' | '/' | '?' | '#') || c.is_whitespace())
                .map_or(text.len(), |end| at + 3 + end + 1);
            found.push((at, end));
        } else if previous.is_none_or(|c| !token_char(c)) && token_start(rest) {
            let end = rest
                .find(|c| !token_char(c) && c != '.')
                .map_or(text.len(), |end| at + end);
            found.push((at, end));
        }
        previous = Some(c);
    }
    found
}

/// Whether `text` starts like a known kind of token or a JWT, at the start of a word.
fn token_start(text: &str) -> bool {
    text.starts_with("eyJ") || TOKEN_PREFIXES.iter().any(|prefix| text.starts_with(prefix))
}

/// Whether a file of that name holds secrets as `KEY=VALUE` lines: `.env`, `.env.local`…
pub fn secret_file(name: &str) -> bool {
    name == ".env" || name.starts_with(".env.")
}

/// The values of the `KEY=VALUE` lines of a file holding secrets, every one of them: whatever its
/// key, a value of such a file is not for a command's output to show. Comments, an `export` in
/// front and the quotes around a value are left out.
pub fn file_values(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            if line.starts_with('#') {
                return None;
            }
            let (_, value) = line.split_once('=')?;
            let value = value.trim();
            let value = [('"', '"'), ('\'', '\'')]
                .iter()
                .find_map(|(open, close)| value.strip_prefix(*open)?.strip_suffix(*close))
                .unwrap_or(value);
            (!value.is_empty()).then(|| value.to_owned())
        })
        .collect()
}

/// Each `-----BEGIN … PRIVATE KEY-----` block, to its end line or, cut short, to the end.
fn mask_private_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(begin) = rest.find("-----BEGIN ") {
        let header = &rest[begin..];
        let Some(close) = header["-----BEGIN ".len()..].find("-----") else {
            break;
        };
        let label = &header[..close + "-----BEGIN ".len()];
        out.push_str(&rest[..begin]);
        if !label.ends_with("PRIVATE KEY") {
            out.push_str("-----BEGIN ");
            rest = &header["-----BEGIN ".len()..];
            continue;
        }
        out.push_str(MASK);
        rest = match header.find("-----END ") {
            Some(end) => {
                let footer = &header[end + "-----END ".len()..];
                footer
                    .find("-----")
                    .map_or("", |close| &footer[close + 5..])
            }
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// The length of the JWT `text` starts with: three parts of base64url joined by dots, the first
/// two not empty.
fn jwt(text: &str) -> Option<usize> {
    if !text.starts_with("eyJ") {
        return None;
    }
    let part = |from: usize| {
        text[from..]
            .find(|c| !token_char(c))
            .map_or(text.len(), |n| from + n)
    };
    let header = part(0);
    let payload = text[header..].strip_prefix('.').map(|_| part(header + 1))?;
    let signature = text[payload..]
        .strip_prefix('.')
        .map(|_| part(payload + 1))?;
    (payload > header + 1).then_some(signature)
}

/// The length of the `user:password` part of the URL whose `://` `text` follows.
fn userinfo(text: &str) -> Option<usize> {
    let end = text.find(|c: char| matches!(c, '@' | '/' | '?' | '#') || c.is_whitespace())?;
    (end > 0 && text[end..].starts_with('@') && text[..end].contains(':')).then_some(end)
}

fn mask_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("://")
            && let Some(end) = userinfo(after)
        {
            out.push_str("://");
            out.push_str(MASK);
            rest = &after[end..];
            continue;
        }
        let at_start = out.chars().last().is_none_or(|c| !token_char(c));
        if at_start {
            if let Some(end) = jwt(rest) {
                out.push_str(MASK);
                rest = &rest[end..];
                continue;
            }
            for prefix in TOKEN_PREFIXES {
                if let Some(after) = rest.strip_prefix(prefix) {
                    let body = after.find(|c| !token_char(c)).unwrap_or(after.len());
                    if body >= 16 {
                        out.push_str(MASK);
                        rest = &after[body..];
                        continue 'scan;
                    }
                }
            }
        }
        let c = rest.chars().next().expect("rest is not empty");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_value_of_a_variable_named_like_a_secret_is_masked() {
        let env = BTreeMap::from([
            ("NPM_TOKEN".into(), "s3cr3t-value".into()),
            ("COMPOSE_PROJECT_NAME".into(), "app-login".into()),
        ]);

        assert_eq!(
            mask("auth s3cr3t-value for app-login", &env),
            "auth [masked] for app-login"
        );
    }

    #[test]
    fn a_variable_named_with_key_auth_or_pass_is_a_secret() {
        for name in [
            "STRIPE_KEY",
            "NPM_AUTH",
            "DB_PASS",
            "api_key",
            "PRIVATE_KEY",
        ] {
            let env = BTreeMap::from([(name.into(), "0123456789abcdef".into())]);

            assert_eq!(
                mask("value 0123456789abcdef.", &env),
                "value [masked].",
                "{name}"
            );
        }
    }

    #[test]
    fn a_git_setting_s_name_is_not_a_secret() {
        let env = BTreeMap::from([
            (
                "GIT_CONFIG_KEY_0".into(),
                "url.nowhere.pushInsteadOf".into(),
            ),
            ("GIT_CONFIG_VALUE_0".into(), "".into()),
        ]);

        assert_eq!(
            mask("GIT_CONFIG_KEY_0=url.nowhere.pushInsteadOf", &env),
            "GIT_CONFIG_KEY_0=url.nowhere.pushInsteadOf"
        );
    }

    #[test]
    fn a_longer_secret_holding_a_shorter_one_is_masked_whole() {
        let env = BTreeMap::from([
            ("A_TOKEN".into(), "abcdef".into()),
            ("B_TOKEN".into(), "abcdefghijkl".into()),
        ]);

        assert_eq!(
            mask("abcdefghijkl and abcdef", &env),
            "[masked] and [masked]"
        );
    }

    #[test]
    fn the_tokens_of_known_shapes_are_masked() {
        let env = BTreeMap::new();
        let table = [
            (
                "npm npm_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789 ok",
                "npm [masked] ok",
            ),
            ("pypi-AgEIcHlwaS5vcmcCJDAwMDAwMDAw-fake ok", "[masked] ok"),
            (
                "key=AIzaSyFAKEfakeFAKEfakeFAKEfakeFAKE012 ok",
                "key=[masked] ok",
            ),
            (
                "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmYWtlIn0.c2lnbmF0dXJl-x_y done",
                "Bearer [masked] done",
            ),
            (
                "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAA\nZmFrZQ==\n\
                 -----END OPENSSH PRIVATE KEY-----\nafter",
                "before\n[masked]\nafter",
            ),
            ("-----BEGIN RSA PRIVATE KEY-----\ncut short", "[masked]"),
            (
                "fetch https://someone:hunter2-fake@forge.example/app.git failed",
                "fetch https://[masked]@forge.example/app.git failed",
            ),
            // Neither a password nor a JWT nor a token: kept.
            (
                "ssh://git@forge.example/app.git",
                "ssh://git@forge.example/app.git",
            ),
            ("eyJ.short", "eyJ.short"),
            ("risk-taking sk-short npm_x", "risk-taking sk-short npm_x"),
        ];
        for (text, masked) in table {
            assert_eq!(mask(text, &env), masked, "{text}");
        }
    }

    #[test]
    fn the_values_of_a_secret_file_are_masked_whatever_their_key() {
        let file = "# the database\nDATABASE_URL=postgres://app@db/app\nexport \
                    SIGNING=\"0123456789abcdef\"\nSHORT=ab\nEMPTY=\n";
        let values = file_values(file);
        assert_eq!(values, ["postgres://app@db/app", "0123456789abcdef", "ab"]);

        assert_eq!(
            mask_also(
                "url postgres://app@db/app key 0123456789abcdef ab",
                &BTreeMap::new(),
                &values
            ),
            "url [masked] key [masked] ab"
        );
        assert!(secret_file(".env") && secret_file(".env.local"));
        assert!(!secret_file("env") && !secret_file(".envrc"));
    }

    /// `text` streamed in the pieces `pieces` cuts it into, each settled part masked as it comes and
    /// the rest at the end, as the recorder does.
    fn streamed(text: &str, pieces: &[usize], env: &BTreeMap<OsString, OsString>) -> String {
        let secrets = Secrets::new(env, &[]);
        let (mut shown, mut held, mut from) = (String::new(), String::new(), 0);
        for &to in pieces.iter().chain([&text.len()]) {
            held.push_str(&text[from..to]);
            from = to;
            let cut = secrets.settled(&held);
            shown.push_str(&secrets.mask(&held[..cut]));
            held.drain(..cut);
        }
        shown + &secrets.mask(&held)
    }

    /// A reply holding every kind of secret `mask` knows, all invented.
    const REPLY: &str = "Done: the value s3cr3t-token-value is set, \
        token=ghp_abcdefghijklmnopqrstuvwxyz0123 ok, \
        Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmYWtlIn0.c2lnbmF0dXJl-x_y done, \
        fetch https://someone:hunter2-fake@forge.example/app.git, a key\n\
        -----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAA\nZmFrZQ==\n\
        -----END OPENSSH PRIVATE KEY-----\nand é, ok.";

    /// A reply with no whitespace, longer than `HELD_WORD`, holding secrets: minified JSON.
    fn long_word() -> String {
        format!(
            "{{\"a\":\"{}\",\"token\":\"ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\",\
             \"jwt\":\"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmYWtlIn0.c2lnbmF0dXJl-x_y\",\
             \"url\":\"https://someone:hunter2-fake@forge.example/app.git\",\
             \"value\":\"s3cr3t-token-value\",\"b\":\"{}\"}}",
            "x".repeat(5000),
            "y".repeat(5000)
        )
    }

    #[test]
    fn a_message_streamed_in_any_pieces_reads_as_it_would_whole() {
        let env = BTreeMap::from([("API_TOKEN".into(), "s3cr3t-token-value".into())]);
        for reply in [REPLY.to_owned(), long_word()] {
            let whole = mask(&reply, &env);
            assert!(
                ["s3cr3t", "hunter2", "ghp_", "eyJ"]
                    .iter()
                    .all(|secret| !whole.contains(secret)),
                "a secret is left whole"
            );
            let bounds: Vec<usize> = (1..reply.len())
                .filter(|&i| reply.is_char_boundary(i))
                .collect();
            // Cut in two anywhere (every few bytes of the long one, for time), and in pieces of
            // every size up to 10. No secret in a failure message.
            let step = if reply.len() > HELD_WORD { 13 } else { 1 };
            for &at in bounds.iter().step_by(step) {
                assert!(streamed(&reply, &[at], &env) == whole, "cut at {at}");
            }
            for size in 1..=10 {
                let pieces: Vec<usize> = bounds.iter().copied().filter(|i| i % size == 0).collect();
                assert!(streamed(&reply, &pieces, &env) == whole, "pieces of {size}");
            }
        }
    }

    #[test]
    fn a_reply_explaining_pem_streams_on() {
        let secrets = Secrets::new(&BTreeMap::new(), &[]);
        let reply = "A key file starts with `-----BEGIN ` and a label, then the key. \
                     It goes on and on without one, ";

        assert_eq!(secrets.settled(reply), reply.len());
    }

    #[test]
    fn an_unclosed_pgp_signed_message_streams_on() {
        let secrets = Secrets::new(&BTreeMap::new(), &[]);
        let reply = "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA256\n\nThe text, ";

        assert_eq!(secrets.settled(reply), reply.len());
    }

    #[test]
    fn what_is_settled_holds_no_part_of_a_secret_still_coming() {
        let env = BTreeMap::from([("API_TOKEN".into(), "s3cr3t-token-value".into())]);
        // Each is the start of a secret its next piece would complete.
        for (start, settles) in [
            (
                "the value is set, the value s3cr3t-tok",
                "the value is set, ",
            ),
            ("token=ghp_abcdefghij", ""),
            (
                "a key\n-----BEGIN RSA PRIVATE KEY-----\nMIIfake\n",
                "a key\n",
            ),
        ] {
            assert_eq!(&start[..Secrets::new(&env, &[]).settled(start)], settles);
        }
    }

    #[test]
    fn a_long_word_is_held_back_no_more_than_a_bound() {
        let word = "x".repeat(10_000);

        let secrets = Secrets::new(&BTreeMap::new(), &[]);

        assert_eq!(secrets.settled(&word), 10_000 - HELD_WORD);
        assert_eq!(
            secrets.settled("plain words and more"),
            "plain words and ".len()
        );
    }

    #[test]
    fn a_token_of_a_known_shape_is_masked() {
        let env = BTreeMap::new();

        assert_eq!(
            mask("token=ghp_abcdefghijklmnopqrstuvwxyz0123 ok", &env),
            "token=[masked] ok"
        );
        assert_eq!(mask("risk-taking sk-short", &env), "risk-taking sk-short");
    }
}
