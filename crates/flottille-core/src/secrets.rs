//! Secrets masked out of what Flottille keeps or shows: the output of a command, a failure.

use std::collections::BTreeMap;
use std::ffi::OsString;

const MASK: &str = "[masked]";

/// Parts of a variable's name that say its value is a secret.
const SECRET_NAMES: [&str; 7] = [
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "API_KEY",
    "PRIVATE_KEY",
];

/// How the tokens of the usual forges and providers start.
const TOKEN_PREFIXES: [&str; 11] = [
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
];

/// `text` with the secrets it holds replaced by `[masked]`: the values of the variables of `env`
/// named like secrets, and anything shaped like a known kind of token.
pub fn mask(text: &str, env: &BTreeMap<OsString, OsString>) -> String {
    let mut masked = text.to_owned();
    for (name, value) in env {
        let (Some(name), Some(value)) = (name.to_str(), value.to_str()) else {
            continue;
        };
        let upper = name.to_ascii_uppercase();
        // A short value is too likely to be an ordinary word.
        if value.len() >= 6 && SECRET_NAMES.iter().any(|part| upper.contains(part)) {
            masked = masked.replace(value, MASK);
        }
    }
    mask_tokens(&masked)
}

fn mask_tokens(text: &str) -> String {
    let token_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while !rest.is_empty() {
        let at_start = out.chars().last().is_none_or(|c| !token_char(c));
        if at_start {
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
    fn a_token_of_a_known_shape_is_masked() {
        let env = BTreeMap::new();

        assert_eq!(
            mask("token=ghp_abcdefghijklmnopqrstuvwxyz0123 ok", &env),
            "token=[masked] ok"
        );
        assert_eq!(mask("risk-taking sk-short", &env), "risk-taking sk-short");
    }
}
