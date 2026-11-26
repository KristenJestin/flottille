//! Long tool results, cut at the source (`docs/tools.md`, "Long results"): a result over the
//! user's limit goes to the agent as its beginning and its end with one line between them, and
//! its whole text, masked, is kept for the agent's session and read by ranges with `run_status`
//! and the id of the cut. No model is involved. The limit is `tool-results.toml` of Flottille's
//! configuration folder, read at every call so that a change applies at once
//! (`docs/protocol.md`, "The tool results methods"). The same file says whether `fs_read` answers
//! "unchanged" for a page the session already read (`unchanged`, off unless it says `true`).

use std::path::PathBuf;

use flottille_core::agent::new_id;
use flottille_core::paths::Paths;
use flottille_core::store::{KEPT_RESULTS_PER_AGENT, Kept};
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
pub(crate) const LINE_CHARS: usize = 1_000;
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

/// The names of the two settings as values of all machines (`crate::all_settings`).
pub const SHARED_LIMIT: &str = "tool_results.limit";
pub const SHARED_UNCHANGED: &str = "tool_results.unchanged";

/// The first line of a `tool-results.toml` this version writes: what it holds is what the user
/// chose here. A file of before held the limit whatever the user chose, so its default there is
/// no choice, and all machines' value applies over it.
const OWN_MARK: &str = "# This machine's own choices: a key left out follows all machines.\n";

/// What results are held to.
struct Settings {
    /// `Some(limit)`, `None` when results are returned whole.
    limit: Option<usize>,
    /// Whether `fs_read` says a page is unchanged instead of sending it again.
    unchanged: bool,
}

/// What `tool-results.toml` says this machine chose: a key it leaves out is none.
#[derive(Default)]
struct Own {
    limit: Option<Option<usize>>,
    unchanged: Option<bool>,
}

/// All machines' limit as its value says it: a number of characters, or `off`.
pub fn shared_limit(value: &str) -> Result<Option<usize>, String> {
    match value {
        "off" => Ok(None),
        number => number
            .parse::<usize>()
            .ok()
            .filter(|limit| (MIN_LIMIT..=MAX_LIMIT).contains(limit))
            .map(Some)
            .ok_or_else(|| {
                format!("a number of characters from {MIN_LIMIT} to {MAX_LIMIT}, or off")
            }),
    }
}

/// All machines' choice of an unchanged page: `true` or `false`.
pub fn shared_unchanged(value: &str) -> Result<bool, String> {
    value.parse().map_err(|_| "true or false".to_owned())
}

/// What results are held to: this machine's choice, else all machines', else the default.
fn read(paths: &Paths) -> Result<Settings, String> {
    let own = own(paths)?;
    let all = |name| flottille_core::all_machines::value(paths, name);
    Ok(Settings {
        limit: own
            .limit
            .or_else(|| all(SHARED_LIMIT).and_then(|value| shared_limit(&value).ok()))
            .unwrap_or(Some(DEFAULT_LIMIT)),
        unchanged: own
            .unchanged
            .or_else(|| all(SHARED_UNCHANGED).and_then(|value| shared_unchanged(&value).ok()))
            .unwrap_or(false),
    })
}

/// Which of the two this machine chose itself: the limit, whether a page is said unchanged.
pub fn own_keys(paths: &Paths) -> Result<(bool, bool), String> {
    let own = own(paths)?;
    Ok((own.limit.is_some(), own.unchanged.is_some()))
}

/// Drops this machine's choice of the value `name`: all machines' applies again.
pub fn forget(paths: &Paths, name: &str) -> Result<(), String> {
    let mut own = own(paths)?;
    match name {
        SHARED_LIMIT => own.limit = None,
        _ => own.unchanged = None,
    }
    write(paths, &own)
}

fn write(paths: &Paths, own: &Own) -> Result<(), String> {
    std::fs::create_dir_all(&paths.config)
        .map_err(|e| format!("{}: {e}", paths.config.display()))?;
    let mut text = OWN_MARK.to_owned();
    match own.limit {
        Some(Some(limit)) => text.push_str(&format!("limit = {limit}\n")),
        Some(None) => text.push_str("limit = \"off\"\n"),
        None => {}
    }
    if let Some(unchanged) = own.unchanged {
        text.push_str(&format!("unchanged = {unchanged}\n"));
    }
    providers::write_in_place(&config(paths), &text).map_err(|e| format!("tool-results.toml: {e}"))
}

/// What `tool-results.toml` says this machine chose; nothing when there is no file.
fn own(paths: &Paths) -> Result<Own, String> {
    let mut settings = Own::default();
    let text = match std::fs::read_to_string(config(paths)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(settings),
        Err(error) => return Err(format!("tool-results.toml: {error}")),
    };
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| format!("tool-results.toml: {}", e.message()))?;
    for (key, _) in document.iter() {
        if key != "limit" && key != "unchanged" {
            return Err(format!("tool-results.toml: unknown key `{key}`"));
        }
    }
    if let Some(item) = document.get("limit") {
        settings.limit = Some(match item.as_integer() {
            _ if item.as_str() == Some("off") => None,
            Some(limit) if (MIN_LIMIT as i64..=MAX_LIMIT as i64).contains(&limit) => {
                Some(limit as usize)
            }
            _ => {
                return Err(format!(
                    "tool-results.toml: limit is a number of characters from {MIN_LIMIT} to \
                     {MAX_LIMIT}, or \"off\""
                ));
            }
        });
    }
    if let Some(item) = document.get("unchanged") {
        settings.unchanged = Some(
            item.as_bool()
                .ok_or("tool-results.toml: unchanged is true or false")?,
        );
    }
    if !text.starts_with(OWN_MARK) {
        if settings.limit == Some(Some(DEFAULT_LIMIT)) {
            settings.limit = None;
        }
        if settings.unchanged == Some(false) {
            settings.unchanged = None;
        }
    }
    Ok(settings)
}

/// The limit a call is held to now. A file that cannot be read is the default's: a cut is the
/// safe side.
fn limit(paths: &Paths) -> Option<usize> {
    read(paths).map_or(Some(DEFAULT_LIMIT), |settings| settings.limit)
}

/// Whether `fs_read` says a page is unchanged now. Off by default until what it saves is measured
/// (#547 records the cost of a turn): a file that cannot be read is the default's.
pub fn says_unchanged(paths: &Paths) -> bool {
    read(paths).is_ok_and(|settings| settings.unchanged)
}

/// How many bytes of text a result may hold now and still go to the agent whole: a tool that
/// pages its own output sizes its page to it. The limit less what a page says of itself.
pub fn page_room(paths: &Paths) -> usize {
    limit(paths).map_or(usize::MAX, |limit| limit.saturating_sub(ABOUT_CHARS))
}

/// Answers a `tool_results.*` request of the user: `tool_results.show`, `tool_results.set`. No
/// tool of an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    let answer = match method {
        "tool_results.show" => said(&read(paths).map_err(|e| Failure::new("invalid-config", e))?),
        "tool_results.set" => {
            // Only what is given is this machine's choice: the rest stays as it was.
            let mut wanted = own(paths).unwrap_or_default();
            let (limit, unchanged) = (params.get("limit"), params.get("unchanged"));
            if limit.is_none() && unchanged.is_none() {
                return Err(Failure::new(
                    "invalid-params",
                    "give a limit, unchanged, or both",
                ));
            }
            match limit {
                None => {}
                Some(Value::String(word)) if word == "off" => wanted.limit = Some(None),
                Some(Value::Number(n))
                    if n.as_u64()
                        .is_some_and(|n| (MIN_LIMIT as u64..=MAX_LIMIT as u64).contains(&n)) =>
                {
                    wanted.limit = Some(n.as_u64().map(|n| n as usize))
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
            }
            match unchanged {
                None => {}
                Some(Value::Bool(on)) => wanted.unchanged = Some(*on),
                _ => {
                    return Err(Failure::new("invalid-params", "unchanged is true or false"));
                }
            }
            write(paths, &wanted).map_err(|e| Failure::new("unavailable", e))?;
            let answer = said(&read(paths).map_err(|e| Failure::new("invalid-config", e))?);
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

/// Says the settings as they are now, `tool_results.changed`: after a change of all machines.
pub fn announce(daemon: &Daemon) -> Result<(), String> {
    let answer = said(&read(&daemon.paths)?);
    daemon
        .emit("tool_results.changed", answer)
        .map(drop)
        .map_err(|error| error.to_string())
}

/// The settings as the protocol says them: the limit is a number of characters, or `"off"`.
/// Nothing is null: a client leaves null fields out of what it sends.
fn said(settings: &Settings) -> Value {
    let limit = match settings.limit {
        Some(limit) => json!(limit),
        None => json!("off"),
    };
    json!({"limit": limit, "unchanged": settings.unchanged})
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
pub(crate) fn shorten(whole: &str, limit: usize, id: &str) -> Option<(String, Cut)> {
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
/// default), as many as fit under the limit, `tail` at most, or the last `tail` without a start,
/// with how to read the others.
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
        Ok(Some(Kept::Text(text))) => text,
        Ok(Some(Kept::Dropped)) => {
            return Outcome::Refused(format!(
                "refused: {id} was dropped: only the last {KEPT_RESULTS_PER_AGENT} results cut \
                 for an agent are kept, and this one is older: run the call again"
            ));
        }
        Ok(None) => {
            return Outcome::Refused(format!(
                "refused: {id} is no cut result of this session: only a result Flottille cut \
                 for this agent can be read, and it is kept for as long as the session"
            ));
        }
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let lines = lines_of(&kept);
    if start.is_some_and(|start| start == 0 || start > lines.len()) {
        return Outcome::Refused(format!(
            "refused: start is a line from 1 to {}, the lines of {id}",
            lines.len()
        ));
    }
    let room = limit(&daemon.paths)
        .unwrap_or(DEFAULT_LIMIT)
        .saturating_sub(ABOUT_CHARS);
    // As on a run: `tail` alone is the last lines, with `start` it is how many follow it.
    let (from, to) = match (tail, start) {
        (Some(tail), None) => {
            let (mut from, mut size) = (lines.len(), 0);
            while from > 0
                && lines.len() - from < tail
                && (from == lines.len() || size + lines[from - 1].chars().count() < room)
            {
                size += lines[from - 1].chars().count() + 1;
                from -= 1;
            }
            (from, lines.len())
        }
        _ => {
            let from = start.unwrap_or(1) - 1;
            let most = tail.unwrap_or(usize::MAX).max(1);
            let (mut to, mut size) = (from, 0);
            while to < lines.len()
                && to - from < most
                && (to == from || size + lines[to].chars().count() < room)
            {
                size += lines[to].chars().count() + 1;
                to += 1;
            }
            (from, to)
        }
    };
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
