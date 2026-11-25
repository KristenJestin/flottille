//! A prompt as the daemon writes it and its event keeps it: one section per delivery, each
//! started by a line `[flottille:<kind>]`. Read the same by the daemon, the window and the phone.

/// What an id of a delivery that replays the conversation to a new session carries: its brief,
/// the summary and the latest messages (`brief-<agent>-<epoch>-replay`).
pub const REPLAY: &str = "-replay";

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
