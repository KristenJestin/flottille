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
    let mut values: Vec<&str> = env
        .iter()
        .filter_map(|(name, value)| Some((name.to_str()?, value.to_str()?)))
        .filter(|(name, value)| {
            let upper = name.to_ascii_uppercase();
            // A short value is too likely to be an ordinary word.
            value.len() >= 6 && SECRET_NAMES.iter().any(|part| upper.contains(part))
        })
        .map(|(_, value)| value)
        .collect();
    // The longest first: a secret holding a shorter one is masked whole.
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let mut masked = text.to_owned();
    for value in values {
        masked = masked.replace(value, MASK);
    }
    mask_tokens(&mask_private_keys(&masked))
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
    fn a_token_of_a_known_shape_is_masked() {
        let env = BTreeMap::new();

        assert_eq!(
            mask("token=ghp_abcdefghijklmnopqrstuvwxyz0123 ok", &env),
            "token=[masked] ok"
        );
        assert_eq!(mask("risk-taking sk-short", &env), "risk-taking sk-short");
    }
}
