//! A turn whose prompt the agent failed because the provider's login is gone: the turn fails,
//! the session stays (`docs/agents.md`, "A failed turn"). Any other reason still fails the
//! agent. A provider's service that is overloaded, unreachable or failing never fails a prompt:
//! the bridges end that turn with a failure the session survives, so it has no entry here.

/// The provider's code and words for a failed prompt: what the session's state line says when
/// its login is gone, `None` for any other reason.
type Recogniser = fn(code: Option<&str>, text: &str) -> Option<&'static str>;

/// The recognisers, by provider: another provider's wording is one more entry.
const RECOGNISERS: &[(&str, Recogniser)] = &[("claude", claude), ("codex", codex)];

pub const CLAUDE_LOGIN: &str =
    "Claude's login expired: sign in again (run claude and /login), then send your message again";

/// Claude Code's kinds of error for a sign-in gone or refused, which the bridge passes as the
/// failed prompt's code (`bridge.rs`, signed out); its words only when it passed none, as for a
/// result that said `/login` without a kind.
fn claude(code: Option<&str>, text: &str) -> Option<&'static str> {
    const WORDS: [&str; 5] = [
        "Failed to authenticate",
        "Please run /login",
        "OAuth session expired",
        "OAuth token has expired",
        "Invalid API key",
    ];
    let login = match code {
        Some(code) => matches!(code, "authentication_failed" | "oauth_org_not_allowed"),
        None => WORDS.iter().any(|words| text.contains(words)),
    };
    login.then_some(CLAUDE_LOGIN)
}

const CODEX_LOGIN: &str =
    "Codex's login expired: sign in again (run codex login), then send your message again";

/// Codex's account signed out, as the bridge tags it (`codex_bridge.rs`, `unauthorized`): its
/// text can hold any number.
fn codex(code: Option<&str>, _text: &str) -> Option<&'static str> {
    (code == Some("unauthorized")).then_some(CODEX_LOGIN)
}

/// The state line of a session whose prompt `provider`'s agent failed with `code` and `text`,
/// when its login is gone.
pub fn recognise(provider: &str, code: Option<&str>, text: &str) -> Option<&'static str> {
    let (_, recogniser) = RECOGNISERS.iter().find(|(name, _)| *name == provider)?;
    recogniser(code, text)
}

/// Whether an agent's state line says its login is gone: it waits for the user then
/// (`agents::waits_for_the_user`).
pub fn is_login(reason: &str) -> bool {
    [CLAUDE_LOGIN, CODEX_LOGIN].contains(&reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_s_expired_login_leaves_the_session_usable() {
        assert_eq!(
            recognise(
                "claude",
                None,
                "Internal error: Failed to authenticate: OAuth session expired and could not be \
                 refreshed"
            ),
            Some(CLAUDE_LOGIN)
        );
        assert_eq!(
            recognise("claude", None, "Not logged in · Please run /login"),
            Some(CLAUDE_LOGIN)
        );
    }

    #[test]
    fn any_other_reason_or_provider_is_not_recognised() {
        assert_eq!(recognise("claude", None, "claude ended: exit code 1"), None);
        assert_eq!(recognise("claude", None, "API Error: 529 Overloaded"), None);
        assert_eq!(recognise("opencode", None, "Failed to authenticate"), None);
        assert_eq!(recognise("codex", None, "the turn failed"), None);
    }

    #[test]
    fn a_codex_error_mentioning_401_in_its_text_is_not_a_login_failure() {
        assert_eq!(
            recognise("codex", None, "the context holds 140123 tokens: 401 over"),
            None
        );
        assert_eq!(recognise("codex", None, "401 Unauthorized"), None);
        assert_eq!(
            recognise("codex", Some("unauthorized"), "whatever it says"),
            Some(CODEX_LOGIN)
        );
    }

    #[test]
    fn a_claude_authentication_failed_code_is_recognised_whatever_its_text() {
        for code in ["authentication_failed", "oauth_org_not_allowed"] {
            assert_eq!(
                recognise("claude", Some(code), "something else entirely"),
                Some(CLAUDE_LOGIN)
            );
        }
        // Another code is not a login, whatever its text says.
        assert_eq!(
            recognise("claude", Some("billing_error"), "Failed to authenticate"),
            None
        );
    }
}
