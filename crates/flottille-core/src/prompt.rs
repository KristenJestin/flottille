//! A prompt as the daemon writes it and its event keeps it: one section per delivery, each
//! started by a line `[flottille:<kind>]`. What Flottille sends a session again after a
//! compaction (its brief, the summary, the latest messages) is part of that prompt, but it is
//! not news to anyone who reads the conversation: the deliveries that hold it carry a replay id,
//! and every reader of the prompt leaves them out.

use serde_json::Value;

/// What an id of a delivery that replays the conversation to a new session carries: its brief,
/// the summary and the latest messages (`brief-<agent>-<epoch>-replay`).
const REPLAY: &str = "-replay";

/// The id of a delivery that replays the conversation after a compaction, from the id it would
/// have had.
pub fn replay_id(id: &str) -> String {
    format!("{id}{REPLAY}")
}

/// Whether a delivery of that id only replays the conversation to a new session.
pub fn is_replay(id: &str) -> bool {
    (id.starts_with("brief-") || id.starts_with("resume-")) && id.contains(REPLAY)
}

/// The deliveries of a prompt, each `(kind, body)`: the daemon starts each with a line
/// `[flottille:<kind>]`, which no data can forge (it escapes the marker in every body). Text
/// before the first marker is of no kind.
pub fn sections(text: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        match marker(line) {
            Some(kind) => found.push((kind.to_owned(), Vec::new())),
            None => match found.last_mut() {
                Some((_, lines)) => lines.push(line),
                None => found.push((String::new(), vec![line])),
            },
        }
    }
    found
        .into_iter()
        .map(|(kind, lines)| (kind, lines.join("\n").trim().to_owned()))
        .filter(|(kind, body)| !kind.is_empty() || !body.is_empty())
        .collect()
}

fn marker(line: &str) -> Option<&str> {
    let kind = line.strip_prefix("[flottille:")?.strip_suffix(']')?;
    (!kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase() || c == '-')).then_some(kind)
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
}
