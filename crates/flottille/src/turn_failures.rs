//! A turn whose prompt the agent failed, for a reason that leaves its session usable (a login
//! that expired, a network down, a provider overloaded or failing): the turn fails, the session
//! stays (`docs/agents.md`, "How an agent runs"). Any other reason still fails the agent.

/// The provider's words for a failed prompt: what the session's state line says when they leave
/// the session usable, `None` for any other.
type Recogniser = fn(text: &str) -> Option<&'static str>;

/// The recognisers, by provider: another provider's wording is one more entry.
const RECOGNISERS: &[(&str, Recogniser)] = &[("claude", claude), ("codex", codex)];

pub const CLAUDE_LOGIN: &str =
    "Claude's login expired: sign in again (run claude and /login), then send your message again";
const CLAUDE_UNREACHABLE: &str =
    "Claude's service could not be reached: send your message again once it is back";

/// What Claude Code says when its sign-in is gone or refused, as the bridge fails the prompt
/// with it (`bridge.rs`, signed out), and when its service could not answer.
fn claude(text: &str) -> Option<&'static str> {
    const LOGIN: [&str; 5] = [
        "Failed to authenticate",
        "Please run /login",
        "OAuth session expired",
        "OAuth token has expired",
        "Invalid API key",
    ];
    const UNREACHABLE: [&str; 6] = [
        "Connection error",
        "Request timed out",
        "Overloaded",
        "overloaded_error",
        "API Error: 5",
        "Internal server error",
    ];
    if LOGIN.iter().any(|words| text.contains(words)) {
        return Some(CLAUDE_LOGIN);
    }
    UNREACHABLE
        .iter()
        .any(|words| text.contains(words))
        .then_some(CLAUDE_UNREACHABLE)
}

const CODEX_LOGIN: &str =
    "Codex's login expired: sign in again (run codex login), then send your message again";

/// What Codex says when its account is signed out (`codex_bridge.rs`, `unauthorized`).
fn codex(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    ["unauthorized", "401", "codex login", "sign in again"]
        .iter()
        .any(|words| lower.contains(words))
        .then_some(CODEX_LOGIN)
}

/// The state line of a session whose prompt `provider`'s agent failed with `text`, when that
/// leaves its session usable.
pub fn recognise(provider: &str, text: &str) -> Option<&'static str> {
    let (_, recogniser) = RECOGNISERS.iter().find(|(name, _)| *name == provider)?;
    recogniser(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_s_expired_login_leaves_the_session_usable() {
        assert_eq!(
            recognise(
                "claude",
                "Internal error: Failed to authenticate: OAuth session expired and could not be \
                 refreshed"
            ),
            Some(CLAUDE_LOGIN)
        );
        assert_eq!(
            recognise("claude", "Not logged in · Please run /login"),
            Some(CLAUDE_LOGIN)
        );
        assert_eq!(
            recognise("claude", "API Error: 529 Overloaded"),
            Some(CLAUDE_UNREACHABLE)
        );
    }

    #[test]
    fn any_other_reason_or_provider_is_not_recognised() {
        assert_eq!(recognise("claude", "claude ended: exit code 1"), None);
        assert_eq!(recognise("opencode", "Failed to authenticate"), None);
        assert_eq!(recognise("codex", "the turn failed"), None);
        assert_eq!(recognise("codex", "401 Unauthorized"), Some(CODEX_LOGIN));
    }
}
