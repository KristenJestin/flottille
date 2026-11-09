//! A session's title (`docs/tools.md`, "The session's title"): its root's agent names it with
//! `session_title` during its first turn and may correct it while the user has not named it; the
//! user renames it (`agent.rename`), and the user's title always wins. Until either, the session
//! keeps the first line of its first message. A child's title is its `agent_launch` title.

use std::sync::Arc;

use flottille_core::agent::Agent;
use flottille_core::secrets;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome, arguments};
use crate::projects::Failure;

/// What an agent's title holds at most, in characters.
pub const LONGEST: usize = 60;
/// How many words an agent's title holds.
pub const WORDS: std::ops::RangeInclusive<usize> = 3..=8;

/// Who named a session.
pub const BY_AGENT: &str = "agent";
pub const BY_USER: &str = "user";

/// A title the agent gave, trimmed, or why it is refused.
pub fn checked(title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.contains(['\n', '\r']) {
        return Err("a title is one line: give it without a line break".to_owned());
    }
    let chars = title.chars().count();
    if chars > LONGEST {
        return Err(format!(
            "a title is at most {LONGEST} characters, this one has {chars}: say the work in fewer \
             words"
        ));
    }
    let words = title.split_whitespace().count();
    if !WORDS.contains(&words) {
        return Err(format!(
            "a title is {} to {} words, this one has {words}: say the work, as \"Rework the \
             login page\"",
            WORDS.start(),
            WORDS.end()
        ));
    }
    Ok(title.to_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    title: String,
}

/// `session_title`: the root's agent names its session, unless the user named it.
pub fn session_title(daemon: &Arc<Daemon>, grant: &Grant, args: Value) -> Outcome {
    let asked: Asked = match arguments("session_title", args) {
        Ok(asked) => asked,
        Err(outcome) => return outcome,
    };
    let agent = match daemon.read(|store| store.agent(&grant.agent)) {
        Ok(Some(agent)) => agent,
        Ok(None) => return Outcome::Failed(format!("there is no agent {}", grant.agent)),
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    if agent.parent.is_some() {
        return Outcome::Refused(
            "refused: a child's title is the one its parent gave it with agent_launch".to_owned(),
        );
    }
    if let Some(refused) = user_named(&agent) {
        return refused;
    }
    let title = match checked(&asked.title) {
        Ok(title) => crate::mcp::masked(&title, &grant.place),
        Err(why) => return Outcome::Refused(format!("refused: {why}")),
    };
    let titled = daemon.write(|store| {
        let event = store.retitle_agent(&agent.id, &title, BY_AGENT)?;
        Ok((event.is_some(), event))
    });
    match titled {
        Ok(true) => Outcome::Done(format!("The session is titled \"{title}\".")),
        // The user named it between the read and the write: their title holds.
        Ok(false) => match daemon.read(|store| store.agent(&agent.id)) {
            Ok(Some(now)) => {
                user_named(&now).unwrap_or_else(|| Outcome::Failed("not titled".into()))
            }
            _ => Outcome::Failed(format!("there is no agent {}", agent.id)),
        },
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// The refusal of an agent's title once the user named the session.
fn user_named(agent: &Agent) -> Option<Outcome> {
    (agent.titled_by.as_deref() == Some(BY_USER)).then(|| {
        Outcome::Refused(format!(
            "refused: the user named this session \"{}\", and the user's title stays: do not \
             title it again",
            agent.title.as_deref().unwrap_or_default()
        ))
    })
}

/// `agent.rename`: the user names a session; their title always wins over its agent's.
pub fn rename(daemon: &Arc<Daemon>, agent: &Agent, title: &str) -> Result<Value, Failure> {
    let title = title.trim();
    if title.is_empty() {
        return Err(Failure::new("invalid-params", "a title cannot be empty"));
    }
    if title.contains(['\n', '\r']) {
        return Err(Failure::new(
            "invalid-params",
            "a title is one line: give it without a line break",
        ));
    }
    let title = secrets::mask(title, &std::env::vars_os().collect());
    daemon.write(|store| Ok(((), store.retitle_agent(&agent.id, &title, BY_USER)?)))?;
    Ok(json!({"agent": agent.id, "title": title, "by": BY_USER}))
}
