//! Review in the window (`docs/extensions.md`, "Review"): `/review` in a session's composer, which
//! Flottille runs and never sends to the agent. A review's agents and findings are one folded line
//! of the session's rail (`session.rs`, `RailRow::Review`).

use flottille_ui::{Toast, toast};
use gpui_kit::{App, WeakEntity};
use serde_json::{Value, json};

use super::Registry;
use crate::actions::Command;
use crate::shell::Shell;

/// Its name in `extension.list`.
pub const NAME: &str = "review";

/// The composer's own command that starts a review of the session.
pub const COMMAND: &str = "review";

/// What `/review` says in the composer's list.
pub const COMMAND_LINE: &str =
    "Review this session's change with fresh eyes: the findings come back to the session";

/// The default of a check's model, as its first option says it: the rule, and what it gives
/// today (the daemon's `STRONGEST` and `SMALLER` of Review).
pub fn model_default(key: &str) -> (&'static str, &'static str) {
    match key {
        "verifier_model" => ("One size smaller", "Sonnet for Claude"),
        _ => ("Strongest of the session's provider", "Opus for Claude"),
    }
}

/// What the card of the checks' models says once under it.
pub const MODELS_LINE: &str = "Models: the strongest of the session's provider unless you choose \
     one. Each check is a turn on your own quota.";

#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `review.start` of a session.
    Started,
}

/// `/review` in the composer of `session`: the daemon starts the review, or says why not.
pub fn start(shell: &WeakEntity<Shell>, session: &str, cx: &mut App) {
    let command = Command::Ask {
        method: "review.start".into(),
        params: json!({"agent": session}),
        then: super::Then::Review(Then::Started),
    };
    shell.update(cx, |shell, _| shell.perform(command)).ok();
}

/// What the daemon answered: a toast either way.
pub fn answered(_: &mut Registry, then: Then, answer: Result<Value, String>, cx: &mut App) {
    match (then, answer) {
        (Then::Started, Ok(started)) => toast(
            Toast::success(format!(
                "Review started, round {} of 2",
                started["round"].as_u64().unwrap_or(1)
            ))
            .detail("Its findings come back to the session."),
            cx,
        ),
        (Then::Started, Err(why)) => toast(Toast::failure("No review started").detail(why), cx),
    }
}
