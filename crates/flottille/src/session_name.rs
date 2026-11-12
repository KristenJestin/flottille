//! A session is named by its id or by its title (`docs/tools.md`, "Sessions that read each
//! other"): the user talks of the title the window shows, which an agent is never told the id
//! behind. `session_read`, `session_send` and `agent_stop` are resolved here, once, before the
//! gate decides, so that the gate, the leave asked once per pair and the refusals only ever see
//! the id.

use flottille_core::agent::Agent;
use flottille_core::store::now_ms;
use serde_json::Value;

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome};
use crate::tree::ago;

/// The sessions a refusal lists at most.
const LISTED: usize = 20;
/// What a line keeps of a title, or of the name the agent gave.
const LABEL_CHARS: usize = 80;

/// The field of the tool's arguments that names a session, for a tool that takes a title.
fn field(tool: &str) -> Option<&'static str> {
    match tool {
        "session_read" | "session_send" => Some("session"),
        "agent_stop" => Some("agent_id"),
        _ => None,
    }
}

/// The arguments of the call with the session it names given by its id, or the refusal when a
/// title picks none or several. An id that names an agent is left to the tool, which refuses a
/// session of another project or a child as it always did.
pub fn resolve(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    mut args: Value,
) -> Result<Value, Outcome> {
    let Some(field) = field(tool) else {
        return Ok(args);
    };
    let Some(name) = args[field].as_str().map(str::to_owned) else {
        return Ok(args);
    };
    let failed = |error: String| Outcome::Failed(error);
    let found = daemon
        .read(|store| store.agent(&name))
        .map_err(|error| failed(error.to_string()))?;
    if found.is_some() {
        return Ok(args);
    }
    let caller = daemon
        .read(|store| store.agent(&grant.agent))
        .map_err(|error| failed(error.to_string()))?
        .ok_or_else(|| failed(format!("there is no agent {}", grant.agent)))?;
    let mut roots: Vec<Agent> = daemon
        .read(|store| store.agents())
        .map_err(|error| failed(error.to_string()))?
        .into_iter()
        .filter(|agent| agent.parent.is_none() && agent.project == caller.project)
        .collect();
    roots.sort_by_key(|agent| std::cmp::Reverse((last_sign(agent), agent.created_at)));
    let wanted = name.trim().to_lowercase();
    let same: Vec<&Agent> = roots
        .iter()
        .filter(|agent| {
            !wanted.is_empty()
                && agent
                    .title
                    .as_deref()
                    .is_some_and(|title| title.trim().to_lowercase() == wanted)
        })
        .collect();
    let refusal = match same.as_slice() {
        [one] => {
            args[field] = Value::String(one.id.clone());
            return Ok(args);
        }
        [] => format!(
            "refused: no session of your project has the id or the title \"{}\": {tool} names a \
             session by its agent id or by its title. The sessions of this project, the most \
             recently active first:\n{}",
            label(&name),
            lines(&roots.iter().collect::<Vec<_>>(), roots.len())
        ),
        several => format!(
            "refused: {} sessions of your project are titled \"{}\": name the one you mean by \
             its agent id:\n{}",
            several.len(),
            label(&name),
            lines(several, several.len())
        ),
    };
    Err(Outcome::Refused(crate::mcp::masked(&refusal, &grant.place)))
}

/// When the session last showed a sign of life.
fn last_sign(agent: &Agent) -> u64 {
    agent
        .active_at
        .or(agent.exchanged_at)
        .unwrap_or(agent.created_at)
}

/// A text on one line, whatever it holds, and no longer than a line should be.
fn label(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    match flat.char_indices().nth(LABEL_CHARS) {
        Some((end, _)) => format!("{}…", &flat[..end]),
        None => flat.to_owned(),
    }
}

/// What names a session on a line: its title, else the first line of the message that started it.
fn name_of(agent: &Agent) -> String {
    let named = agent.title.as_deref().map(label).filter(|t| !t.is_empty());
    let first = || {
        agent
            .brief
            .as_deref()
            .and_then(|brief| brief.lines().map(label).find(|line| !line.is_empty()))
    };
    named
        .or_else(first)
        .unwrap_or_else(|| "(untitled)".to_owned())
}

/// A line each, `<title> · <id> · <place> · <state>, active <when>`, the first `LISTED` of
/// `total` sessions.
fn lines(sessions: &[&Agent], total: usize) -> String {
    let now = now_ms();
    let mut text: Vec<String> = sessions
        .iter()
        .take(LISTED)
        .map(|agent| {
            format!(
                "{} · {} · {} · {}, active {} ago",
                name_of(agent),
                agent.id,
                agent.workspace.as_deref().unwrap_or("main"),
                agent.state.as_str(),
                ago(now.saturating_sub(last_sign(agent)))
            )
        })
        .collect();
    if total > LISTED {
        text.push(format!("(and {} more)", total - LISTED));
    }
    text.join("\n")
}

#[cfg(test)]
mod tests {
    use flottille_core::agent::{AgentState, Choices, GateMode};

    use super::*;

    fn a_session(id: &str, title: Option<&str>) -> Agent {
        Agent {
            id: id.into(),
            provider: "claude".into(),
            role: "lead".into(),
            project: "app".into(),
            workspace: None,
            folder: "/app".into(),
            state: AgentState::Idle,
            state_reason: None,
            created_at: 1,
            updated_at: 1,
            active_at: None,
            exchanged_at: None,
            ended_at: None,
            chosen: Choices::default(),
            taken: Choices::default(),
            pending: Choices::default(),
            native_id: None,
            agent_version: None,
            instructions: String::new(),
            tools: Vec::new(),
            parent: None,
            depth: 0,
            epoch: 0,
            title: title.map(str::to_owned),
            brief: None,
            reported_at: None,
            quiet_turns: 0,
            waiting_on_answer: false,
            reopens: false,
            images: None,
            gate_mode: GateMode::Auto,
            launched_by: None,
            led: false,
            detached_from: None,
            titled_by: None,
        }
    }

    #[test]
    fn a_refusal_lists_twenty_sessions_and_counts_the_others() {
        let sessions: Vec<Agent> = (0..23)
            .map(|n| a_session(&format!("{n:012}"), Some("Rework the login")))
            .collect();
        let all: Vec<&Agent> = sessions.iter().collect();

        let text = lines(&all, all.len());

        assert_eq!(text.lines().count(), 21, "{text}");
        assert!(text.ends_with("(and 3 more)"), "{text}");
        assert!(text.contains("Rework the login · 000000000000 · main · idle, active "));
    }

    #[test]
    fn a_title_holding_a_line_break_stays_on_one_line() {
        let session = a_session("0123456789ab", Some("One\nTwo\u{7}"));

        assert_eq!(name_of(&session), "One Two");
    }

    #[test]
    fn a_session_without_a_title_is_named_by_the_first_line_of_its_brief() {
        let mut session = a_session("0123456789ab", None);
        session.brief = Some("\nFix the login\nand the logout".into());

        assert_eq!(name_of(&session), "Fix the login");
        assert_eq!(name_of(&a_session("0123456789ab", None)), "(untitled)");
    }
}
