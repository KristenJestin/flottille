//! A session is named by its id or by its title (`docs/tools.md`, "Sessions that read each
//! other"): the user talks of the title the window shows, which an agent is never told the id
//! behind. `session_read`, `session_send` and `agent_stop` are resolved here, once, before the
//! gate decides, so that the gate, the leave asked once per pair and the refusals only ever see
//! the id.

use flottille_core::agent::Agent;
use flottille_core::store::now_ms;
use serde_json::Value;

use crate::across::Away;
use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome};
use crate::tree::ago;

/// The sessions a refusal lists at most.
const LISTED: usize = 20;
/// What a line keeps of a title, or of the name the agent gave.
const LABEL_CHARS: usize = 80;

/// The field of the call that names a session, and what the refusals call the tool, for a call
/// that takes a title.
fn field(tool: &str, args: &Value) -> Option<(&'static str, &'static str)> {
    match tool {
        "session_read" => Some(("session", "session_read")),
        "session_send" => Some(("session", "session_send")),
        "agent_stop" => Some(("agent_id", "agent_stop")),
        "forge" if args["action"] == "unsubscribe" => Some(("session", "forge unsubscribe")),
        _ => None,
    }
}

/// Whether an ended session can take what the tool does to it: `session_send` and `agent_stop`
/// refuse one, `session_read` and `forge unsubscribe` do not.
fn needs_a_live_session(tool: &str) -> bool {
    matches!(tool, "session_send" | "agent_stop")
}

/// The other project a `session_send` or `session_read` names (`project`), when it is not the
/// caller's: a title is then matched among that project's root sessions. A project that is not
/// registered is refused, naming the registered ones.
fn named_project(
    daemon: &Daemon,
    tool: &str,
    args: &Value,
    caller_project: &str,
) -> Result<Option<String>, Outcome> {
    if !matches!(tool, "session_send" | "session_read") {
        return Ok(None);
    }
    let Some(project) = args["project"]
        .as_str()
        .map(str::trim)
        .filter(|project| !project.is_empty())
    else {
        return Ok(None);
    };
    if !crate::projects::exists(daemon, project) {
        let mut names: Vec<String> = crate::projects::all(daemon)
            .into_iter()
            .map(|project| project.name)
            .collect();
        names.sort();
        return Err(Outcome::Refused(format!(
            "refused: project {} is not registered: the registered projects are {}",
            label(project),
            names.join(", ")
        )));
    }
    Ok((project != caller_project).then(|| project.to_owned()))
}

/// Where the session a call names runs: here, the arguments naming it by its id, or on another
/// machine of the user's, which the call is sent to as it is (`across::forward`).
pub enum Resolved {
    Here(Value),
    Away(Value, Away),
}

/// The arguments of the call with the session it names given by its id, or the refusal when a
/// title picks none or several. An id that names an agent is left to the tool, which refuses a
/// session of another project or a child as it always did. With `project`, a title is matched
/// among that project's root sessions, as it is among the caller's own. The sessions other
/// machines list are found the same way, by their id or their title: a session is the same
/// wherever it runs (`docs/remote.md`, "A tree across machines").
pub fn resolve(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    mut args: Value,
) -> Result<Resolved, Outcome> {
    // Any tool of the tree that names, by its id, a session another machine runs and no agent
    // here: its machine answers it.
    if crate::tree::TOOLS.contains(&tool) {
        for field in ["session", "agent_id"] {
            let Some(id) = args[field].as_str().map(str::trim).map(str::to_owned) else {
                continue;
            };
            if daemon
                .read(|store| store.agent(&id))
                .ok()
                .flatten()
                .is_some()
            {
                break;
            }
            if let Some(away) = crate::across::sessions_elsewhere(daemon, None)
                .into_iter()
                .find(|away| away.session.id == id)
            {
                args[field] = Value::String(id);
                return Ok(Resolved::Away(args, away));
            }
        }
    }
    let Some((field, what)) = field(tool, &args) else {
        return Ok(Resolved::Here(args));
    };
    let Some(given) = args[field].as_str() else {
        return Ok(Resolved::Here(args));
    };
    let name = given.trim().to_owned();
    let failed = |error: String| Outcome::Failed(error);
    let caller = daemon
        .read(|store| store.agent(&grant.agent))
        .map_err(|error| failed(error.to_string()))?
        .ok_or_else(|| failed(format!("there is no agent {}", grant.agent)))?;
    let elsewhere = named_project(daemon, tool, &args, &caller.project)?;
    let found = daemon
        .read(|store| store.agent(&name))
        .map_err(|error| failed(error.to_string()))?;
    if found.is_some() {
        args[field] = Value::String(name);
        return Ok(Resolved::Here(args));
    }
    let among = elsewhere.as_deref().unwrap_or(&caller.project);
    let mut roots: Vec<Agent> = daemon
        .read(|store| store.agents())
        .map_err(|error| failed(error.to_string()))?
        .into_iter()
        .filter(|agent| agent.parent.is_none() && agent.project == among)
        .collect();
    roots.sort_by_key(|agent| std::cmp::Reverse((last_sign(agent), agent.created_at)));
    // Only a tool of the tree reaches another machine's sessions.
    let far = match crate::tree::TOOLS.contains(&tool) {
        true => crate::across::sessions_elsewhere(daemon, Some(among)),
        false => Vec::new(),
    };
    let wanted = name.to_lowercase();
    let titled = |title: &str| !wanted.is_empty() && title.trim().to_lowercase() == wanted;
    let mut same: Vec<&Agent> = roots
        .iter()
        .filter(|agent| agent.title.as_deref().is_some_and(titled))
        .collect();
    // Another machine lists its live sessions only.
    let far_same: Vec<&Away> = far
        .iter()
        .filter(|away| titled(&away.session.title))
        .collect();
    // A session that ended cannot take a message or be stopped: the live one is meant.
    if needs_a_live_session(tool)
        && (!far_same.is_empty() || same.iter().any(|agent| !agent.state.is_final()))
    {
        same.retain(|agent| !agent.state.is_final());
    }
    let whose = match &elsewhere {
        Some(project) => format!("project {}", label(project)),
        None => "your project".to_owned(),
    };
    let now = now_ms();
    // Every session listed, here and elsewhere, the most recently active first.
    let lines = |here: &[&Agent], away: &[&Away]| -> Vec<String> {
        let mut lines: Vec<(u64, String)> = here
            .iter()
            .map(|agent| (last_sign(agent), line(agent, now)))
            .chain(
                away.iter()
                    .map(|away| (away.session.exchanged_at, away_line(daemon, away, now))),
            )
            .collect();
        lines.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        lines.into_iter().map(|(_, line)| line).collect()
    };
    let (header, listed) = match (same.as_slice(), far_same.as_slice()) {
        ([one], []) => {
            args[field] = Value::String(one.id.clone());
            return Ok(Resolved::Here(args));
        }
        ([], [one]) => {
            args[field] = Value::String(one.session.id.clone());
            return Ok(Resolved::Away(args, (*one).clone()));
        }
        ([], []) => (
            format!(
                "refused: no session of {whose} has the id or the title \"{}\": {what} \
                 names a session by its agent id or by its title. The sessions of this project, \
                 the most recently active first:",
                label(&name)
            ),
            lines(
                &roots.iter().collect::<Vec<_>>(),
                &far.iter().collect::<Vec<_>>(),
            ),
        ),
        (here, away) => (
            format!(
                "refused: {} sessions of {whose} are titled \"{}\": name the one you mean \
                 by its agent id:",
                here.len() + away.len(),
                label(&name)
            ),
            lines(here, away),
        ),
    };
    Err(Outcome::Refused(listing(grant, &header, &listed)))
}

/// The refusal: `header`, then as many of the sessions' lines, whole, as fit under the limit of
/// a refusal once masked and escaped (`LISTED` at most), then how many are left out.
fn listing(grant: &Grant, header: &str, sessions: &[String]) -> String {
    let lines: Vec<String> = sessions.iter().take(LISTED).cloned().collect();
    let text = |kept: usize| {
        let mut text = vec![header.to_owned()];
        text.extend(lines[..kept].iter().cloned());
        if kept < sessions.len() {
            text.push(format!("(and {} more)", sessions.len() - kept));
        }
        crate::mcp::masked(&text.join("\n"), &grant.place)
    };
    let fits = |kept: usize| {
        crate::deliveries::escape(&text(kept)).chars().count() <= crate::mcp::ERROR_LIMIT
    };
    let kept = (0..=lines.len())
        .rev()
        .find(|kept| fits(*kept))
        .unwrap_or(0);
    text(kept)
}

/// `<title> · <id> · on <machine> · <state>, active <when> ago`, the machine's own state after
/// its name while it is off (`on server, offline since 22:14`).
fn away_line(daemon: &Daemon, away: &Away, now: u64) -> String {
    let title = Some(label(&away.session.title))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "(untitled)".to_owned());
    let machine = match crate::across::unreachable(daemon, away) {
        Some(state) => format!("on {}, {state}", away.name),
        None => format!("on {}", away.name),
    };
    format!(
        "{title} · {} · {machine} · {}, active {} ago",
        away.session.id,
        away.session.state,
        ago(now.saturating_sub(away.session.exchanged_at))
    )
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

/// What names a session on a line: its title. What it was started with is the user's opening
/// message, which no other session is given.
fn name_of(agent: &Agent) -> String {
    agent
        .title
        .as_deref()
        .map(label)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "(untitled)".to_owned())
}

/// `<title> · <id> · <place> · <state>, active <when> ago`.
fn line(agent: &Agent, now: u64) -> String {
    format!(
        "{} · {} · {} · {}, active {} ago",
        name_of(agent),
        agent.id,
        agent.workspace.as_deref().unwrap_or("main"),
        agent.state.as_str(),
        ago(now.saturating_sub(last_sign(agent)))
    )
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
    fn a_line_says_the_title_the_id_the_place_and_the_state() {
        let session = a_session("0123456789ab", Some("Rework the login"));

        let text = line(&session, 1_000);

        assert!(
            text.starts_with("Rework the login · 0123456789ab · main · idle, active "),
            "{text}"
        );
    }

    #[test]
    fn a_title_holding_a_line_break_stays_on_one_line() {
        let session = a_session("0123456789ab", Some("One\nTwo\u{7}"));

        assert_eq!(name_of(&session), "One Two");
    }

    #[test]
    fn a_session_without_a_title_is_never_named_by_what_it_was_started_with() {
        let mut session = a_session("0123456789ab", None);
        session.brief = Some("Fix the login with the token s3cret".into());

        assert_eq!(name_of(&session), "(untitled)");
    }
}
