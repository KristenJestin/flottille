//! Long tool results, cut at the source (`docs/tools.md`, "Long results"): a result over the
//! user's limit goes to the agent as its beginning and its end with one line between them, and
//! its whole text, masked, is kept for the agent's session and read by ranges with `run_status`
//! and the id of the cut. No model is involved. The limit is `tool-results.toml` of Flottille's
//! configuration folder, read at every call so that a change applies at once
//! (`docs/protocol.md`, "The tool results methods").

use std::path::PathBuf;

use flottille_core::agent::new_id;
use flottille_core::paths::Paths;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome, masked};
use crate::projects::Failure;
use crate::providers;

/// The limit until the user chooses another, in characters.
pub const DEFAULT_LIMIT: usize = 8_000;
/// The smallest limit: a page of the whole text and the line that describes it fit under it.
pub const MIN_LIMIT: usize = 4_000;
/// The largest: a result never goes past it anyway (`mcp::RESULT_LIMIT`).
pub const MAX_LIMIT: usize = crate::mcp::RESULT_LIMIT;
/// A line longer than this shows as several, in a cut as in the pages of the whole text, so that
/// every line of either fits the room a cut keeps for its end.
const LINE_CHARS: usize = 1_000;
/// What a page of the whole text leaves under the limit for the line that describes it and for
/// what escaping adds.
const ABOUT_CHARS: usize = 400;
/// How an id of a kept result begins: a run's id is hexadecimal, so none is taken for the other.
const PREFIX: &str = "res_";

/// Whether `run_status`'s `run` names a kept result rather than a run.
pub fn is_id(run: &str) -> bool {
    run.starts_with(PREFIX)
}

/// `$XDG_CONFIG_HOME/flottille/tool-results.toml`.
fn config(paths: &Paths) -> PathBuf {
    paths.config.join("tool-results.toml")
}

/// What `tool-results.toml` says: `Some(limit)`, `None` when results are returned whole. The
/// default when there is no file.
fn read(paths: &Paths) -> Result<Option<usize>, String> {
    let text = match std::fs::read_to_string(config(paths)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(DEFAULT_LIMIT));
        }
        Err(error) => return Err(format!("tool-results.toml: {error}")),
    };
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| format!("tool-results.toml: {}", e.message()))?;
    for (key, _) in document.iter() {
        if key != "limit" {
            return Err(format!("tool-results.toml: unknown key `{key}`"));
        }
    }
    let Some(item) = document.get("limit") else {
        return Ok(Some(DEFAULT_LIMIT));
    };
    if item.as_str() == Some("off") {
        return Ok(None);
    }
    match item.as_integer() {
        Some(limit) if (MIN_LIMIT as i64..=MAX_LIMIT as i64).contains(&limit) => {
            Ok(Some(limit as usize))
        }
        _ => Err(format!(
            "tool-results.toml: limit is a number of characters from {MIN_LIMIT} to {MAX_LIMIT}, \
             or \"off\""
        )),
    }
}

/// The limit a call is held to now. A file that cannot be read is the default's: a cut is the
/// safe side.
fn limit(paths: &Paths) -> Option<usize> {
    if std::env::var_os("FLOTTILLE_TEST_NO_CUT_RESULTS").is_some_and(|v| !v.is_empty()) {
        return None;
    }
    read(paths).unwrap_or(Some(DEFAULT_LIMIT))
}

/// Answers a `tool_results.*` request of the user: `tool_results.show`, `tool_results.set`. No
/// tool of an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    let answer = match method {
        "tool_results.show" => {
            let limit = read(paths).map_err(|e| Failure::new("invalid-config", e))?;
            json!({"limit": said(limit)})
        }
        "tool_results.set" => {
            let wanted = match params.get("limit") {
                Some(Value::String(word)) if word == "off" => None,
                Some(Value::Number(n))
                    if n.as_u64()
                        .is_some_and(|n| (MIN_LIMIT as u64..=MAX_LIMIT as u64).contains(&n)) =>
                {
                    n.as_u64().map(|n| n as usize)
                }
                _ => {
                    return Err(Failure::new(
                        "invalid-params",
                        format!(
                            "limit is a number of characters from {MIN_LIMIT} to {MAX_LIMIT}, or \
                             \"off\" for no cut"
                        ),
                    ));
                }
            };
            std::fs::create_dir_all(&paths.config).map_err(|e| {
                Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
            })?;
            let text = match wanted {
                Some(limit) => format!("limit = {limit}\n"),
                None => "limit = \"off\"\n".to_owned(),
            };
            providers::write_in_place(&config(paths), &text)
                .map_err(|e| Failure::new("unavailable", format!("tool-results.toml: {e}")))?;
            let answer = json!({"limit": said(wanted)});
            daemon
                .emit("tool_results.changed", answer.clone())
                .map_err(|error| Failure::new("unavailable", error.to_string()))?;
            answer
        }
        method => {
            return Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            ));
        }
    };
    Ok(answer)
}

/// The limit as the protocol says it: a number of characters, or `"off"`. Nothing is null: a
/// client leaves null fields out of what it sends.
fn said(limit: Option<usize>) -> Value {
    match limit {
        Some(limit) => json!(limit),
        None => json!("off"),
    }
}

/// A result that was cut: what its call is recorded with.
pub struct Cut {
    pub id: String,
    /// The characters of the whole result, and how many of them the agent was not shown.
    pub chars: usize,
    pub left_out: usize,
}

impl Cut {
    pub fn to_json(&self) -> Value {
        json!({"result": self.id, "chars": self.chars, "left_out": self.left_out})
    }
}

/// The lines of a text, a line longer than `LINE_CHARS` shown as several.
fn lines_of(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.trim_end_matches('\n').split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
        }
        for piece in chars.chunks(LINE_CHARS) {
            lines.push(piece.iter().collect());
        }
    }
    lines
}

/// The result of a tool as the agent is to be shown it: `text` itself when it is under the limit
/// or the cut is off, else its beginning and its end, with the whole text, masked, kept for the
/// agent under an id.
pub fn cut_done(daemon: &Daemon, grant: &Grant, tool: &str, text: String) -> (String, Option<Cut>) {
    let Some(limit) = limit(&daemon.paths) else {
        return (text, None);
    };
    if text.chars().count() <= limit {
        return (text, None);
    }
    // Masked whole before it is cut, so that no secret is split by the cut and shown in part.
    let whole = masked(&text, &grant.place);
    let Ok(id) = new_id().map(|id| format!("{PREFIX}{id}")) else {
        return (text, None);
    };
    let Some((shown, cut)) = shorten(&whole, limit, &id) else {
        return (text, None);
    };
    if let Err(error) = daemon.read(|store| store.keep_result(&id, &grant.agent, tool, &whole)) {
        // Nothing to read it from: the result goes whole, as far as the server's own limit.
        eprintln!(
            "flottille daemon: agent {}: keeping a cut result: {error}",
            grant.agent
        );
        return (text, None);
    }
    (shown, Some(cut))
}

/// The beginning and the end of `whole` (about half the limit and an eighth of it, on line
/// boundaries) with the line that says what was left out and how to read it.
fn shorten(whole: &str, limit: usize, id: &str) -> Option<(String, Cut)> {
    let chars = whole.chars().count();
    let lines = lines_of(whole);
    let (head_room, tail_room) = (limit / 2, (limit / 8).max(LINE_CHARS + 1));
    let (mut head, mut size) = (0, 0);
    while head < lines.len() && size + lines[head].chars().count() < head_room {
        size += lines[head].chars().count() + 1;
        head += 1;
    }
    let (mut tail, mut size) = (0, 0);
    while head + tail < lines.len()
        && size + lines[lines.len() - 1 - tail].chars().count() < tail_room
    {
        size += lines[lines.len() - 1 - tail].chars().count() + 1;
        tail += 1;
    }
    if head + tail >= lines.len() {
        return None;
    }
    let (start, end) = (&lines[..head], &lines[lines.len() - tail..]);
    let shown_chars = start
        .iter()
        .chain(end)
        .map(|l| l.chars().count() + 1)
        .sum::<usize>();
    let left_out = chars.saturating_sub(shown_chars);
    let from = head + 1;
    let to = lines.len() - tail;
    let marker = format!(
        "[{left_out} characters left out (lines {from}-{to} of {}): read them with run_status \
         {{\"run\": \"{id}\", \"start\": {from}}}]",
        lines.len()
    );
    let mut shown = start.join("\n");
    if head > 0 {
        shown.push('\n');
    }
    shown.push_str(&marker);
    if tail > 0 {
        shown.push('\n');
        shown.push_str(&end.join("\n"));
    }
    Some((
        shown,
        Cut {
            id: id.to_owned(),
            chars,
            left_out,
        },
    ))
}

/// `run_status` on the id of a cut result: lines of its whole text from `start` (the first by
/// default), as many as fit under the limit, or `tail` at most, with how to read the others.
pub fn read_kept(
    daemon: &Daemon,
    agent: &str,
    id: &str,
    action: Option<&str>,
    tail: Option<usize>,
    start: Option<usize>,
) -> Outcome {
    if action.is_some_and(|action| action != "output") {
        return Outcome::Refused(format!(
            "refused: {id} is a cut result, not a run: it is read with run_status, its \
             actions are output only"
        ));
    }
    let kept = match daemon.read(|store| store.kept_result(id, agent)) {
        Ok(Some(text)) => text,
        Ok(None) => {
            return Outcome::Refused(format!(
                "refused: {id} is no cut result of this session: only a result Flottille cut \
                 for this agent can be read, and it is kept for as long as the session"
            ));
        }
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let lines = lines_of(&kept);
    let start = start.unwrap_or(1);
    if start == 0 || start > lines.len() {
        return Outcome::Refused(format!(
            "refused: start is a line from 1 to {}, the lines of {id}",
            lines.len()
        ));
    }
    let room = limit(&daemon.paths)
        .unwrap_or(DEFAULT_LIMIT)
        .saturating_sub(ABOUT_CHARS);
    let from = start - 1;
    let most = tail.unwrap_or(usize::MAX).max(1);
    let (mut to, mut size) = (from, 0);
    while to < lines.len()
        && to - from < most
        && (to == from || size + lines[to].chars().count() < room)
    {
        size += lines[to].chars().count() + 1;
        to += 1;
    }
    let mut about = format!("(lines {}-{to} of {}", from + 1, lines.len());
    if from > 0 {
        about.push_str(&format!(
            "; earlier lines: run_status {{\"run\": \"{id}\", \"start\": 1}}"
        ));
    }
    if to < lines.len() {
        about.push_str(&format!(
            "; later lines: run_status {{\"run\": \"{id}\", \"start\": {}}}",
            to + 1
        ));
    } else {
        about.push_str(", the end of the result");
    }
    about.push(')');
    Outcome::Done(format!("{about}\n{}", lines[from..to].join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(lines: usize) -> String {
        (1..=lines)
            .map(|n| format!("line {n:04} of the log\n"))
            .collect()
    }

    #[test]
    fn a_cut_keeps_whole_lines_of_the_beginning_and_of_the_end() {
        let (shown, cut) = shorten(&log(600), 8_000, "res_1").unwrap();

        assert!(shown.starts_with("line 0001 of the log\n"), "{shown}");
        assert!(shown.ends_with("line 0600 of the log"), "{shown}");
        assert!(shown.chars().count() < 8_000, "{}", shown.chars().count());
        assert!(shown.contains("characters left out (lines "), "{shown}");
        assert!(cut.left_out > 7_000, "{}", cut.left_out);
        assert_eq!(cut.chars, log(600).chars().count());
    }

    #[test]
    fn a_text_whose_lines_all_fit_is_not_cut() {
        assert!(shorten("one\ntwo\n", 8_000, "res_1").is_none());
    }

    #[test]
    fn one_very_long_line_shows_as_several_and_the_cut_still_keeps_its_end() {
        let text = format!("{}END", "x".repeat(30_000));

        let (shown, _) = shorten(&text, 8_000, "res_1").unwrap();

        assert!(shown.ends_with("END"), "{}", &shown[shown.len() - 20..]);
        assert!(shown.chars().count() < 8_000);
    }

    #[test]
    fn the_smallest_limit_still_leaves_room_for_both_ends() {
        let (shown, _) = shorten(&log(600), MIN_LIMIT, "res_1").unwrap();

        assert!(
            shown.chars().count() < MIN_LIMIT,
            "{}",
            shown.chars().count()
        );
        assert!(shown.starts_with("line 0001"));
        assert!(shown.ends_with("line 0600 of the log"));
    }
}
