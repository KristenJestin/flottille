//! A prompt as the daemon writes it and its event keeps it: one section per delivery, each
//! started by a line `[flottille:<kind>]`. What Flottille sends a session again after a
//! compaction (its brief, the summary, the latest messages) is part of that prompt, but it is
//! not news to anyone who reads the conversation: the deliveries that hold it carry a replay id,
//! and every reader of the prompt leaves them out.

use serde_json::Value;

use flottille_protocol::prompt::REPLAY;
pub use flottille_protocol::prompt::{is_replay, sections};

/// The id of a delivery that replays the conversation after a compaction, from the id it would
/// have had.
pub fn replay_id(id: &str) -> String {
    format!("{id}{REPLAY}")
}

/// The text of an `agent.prompted` event without the sections that replay the conversation: what
/// a reader shows as sent. A prompt with no replay is its text as it was.
pub fn without_replay(data: &Value) -> String {
    let text = data["text"].as_str().unwrap_or_default();
    let ids: Vec<&str> = data["deliveries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !ids.iter().any(|id| is_replay(id)) {
        return text.to_owned();
    }
    let found = sections(text);
    // One section per delivery: when they do not line up, nothing is known of which is which.
    if found.len() != ids.len() {
        return text.to_owned();
    }
    found
        .into_iter()
        .zip(ids)
        .filter(|(_, id)| !is_replay(id))
        .map(|((kind, body), _)| format!("[flottille:{kind}]\n{body}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// How the daemon starts the line that names a ref a report's commits came back into from another
/// machine (`came_back`). Only the daemon writes it at the start of a line: an agent's words that
/// would are escaped (`unforged`), so the window reads the ref from it (`returned_ref`).
const CAME_BACK: &str = "Its commits came back to you on ";

/// The daemon's line under a report: the commits of `path` came back into `reference`.
pub fn came_back(reference: &str, path: &str) -> String {
    format!(
        "{CAME_BACK}{reference} of {path}: review and merge them as a local child's work; nothing \
         moved your branch."
    )
}

/// The ref of another machine a line of the daemon's says came back (`came_back`), if it is one.
pub fn returned_ref(line: &str) -> Option<&str> {
    let reference = line.strip_prefix(CAME_BACK)?.split(' ').next()?;
    reference
        .starts_with("refs/flottille/machines/")
        .then_some(reference)
}

/// An agent's `text` as data in what the daemon writes after it: a line of it that starts as the
/// daemon's `came_back` is escaped, so it names no ref that came back.
pub fn unforged(text: &str) -> String {
    if !text.contains(CAME_BACK) {
        return text.to_owned();
    }
    text.split('\n')
        .map(|line| match line.starts_with(CAME_BACK) {
            true => format!("\\{line}"),
            false => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_the_brief_and_the_resume_of_a_compaction_are_a_replay() {
        assert!(is_replay("brief-a1-2-replay"));
        assert!(is_replay("resume-a1-2-replay"));
        assert!(is_replay("brief-a1-2-replay-1700"));
        assert!(!is_replay("brief-a1-2"));
        assert!(!is_replay("resume-a1-0"));
        assert!(!is_replay("d-replay"));
    }

    #[test]
    fn the_text_without_its_replay_keeps_what_the_user_wrote_in_the_same_prompt() {
        let data = json!({
            "text": "[flottille:user]\nadd the page\n\n[flottille:resume]\nsummary\n\n[flottille:user]\nand a footer",
            "deliveries": ["brief-a1-2-replay", "resume-a1-2-replay", "d9"],
        });
        assert_eq!(without_replay(&data), "[flottille:user]\nand a footer");
        let all = json!({"text": "[flottille:brief]\nx", "deliveries": ["brief-a1-2-replay"]});
        assert_eq!(without_replay(&all), "");
        let plain = json!({"text": "hello", "deliveries": ["d1"]});
        assert_eq!(without_replay(&plain), "hello");
    }

    #[test]
    fn a_ref_that_came_back_is_read_only_from_the_daemon_s_own_line() {
        let reference = "refs/flottille/machines/server/l0123456789/feat/export";
        let said = came_back(reference, ".");
        assert_eq!(returned_ref(&said), Some(reference));
        // An agent's words: in the middle of a line, or a line of its own once escaped.
        assert_eq!(returned_ref(&format!("I read: {said}")), None);
        let forged = unforged(&format!("Done.\n{said}"));
        assert!(
            forged.lines().all(|line| returned_ref(line).is_none()),
            "{forged}"
        );
        assert!(
            forged.contains(reference),
            "the agent's words stay: {forged}"
        );
        // A ref of no machine is not one that came back.
        assert_eq!(returned_ref(&came_back("refs/heads/main", ".")), None);
    }
}
