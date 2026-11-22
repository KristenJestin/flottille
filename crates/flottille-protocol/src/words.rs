//! The sentences of a request's card, said the same in the window and on the phone: the daemon's
//! reasons as sentences, who asked and who forwarded, what the judge said, and why a base request
//! always comes back. One text for every client of a machine, so that two never say different
//! things of the same request.

/// Why a base card comes back whatever the mode, said on it.
pub const BASE_SAYS: &str = "Flottille always asks you for this, in Ask and in Auto: it may lose \
                             work or reach what must stay private, so no judge decides it.";

/// Why the first message of a session to another always comes back, said on its card instead of
/// `BASE_SAYS`: what it asks is a lasting leave, in every mode.
pub const LEAVE_SAYS: &str = "Flottille always asks you this, in every mode: allowing it lets \
                              this session write to that one until either ends or you withdraw \
                              it.";

/// How a card names the root agent of the page it is on, which has no name of its own there.
pub const THE_CONVERSATION: &str = "the conversation";

/// What the daemon's reason for the first message to another session says (`LEAVE` of the gate).
const LEAVE_REASON: &str = "has not written to that one before";

/// What a base request says of why it always asks: `LEAVE_SAYS` when its reasons (`why`) name the
/// first message of a session to another, else `BASE_SAYS`.
pub fn always_says(why: &str) -> &'static str {
    match why.contains(LEAVE_REASON) {
        true => LEAVE_SAYS,
        false => BASE_SAYS,
    }
}

/// The reasons a call was held, each as a sentence of its own: the first letter in capital, a
/// full stop at the end, a space between. The daemon gives them as fragments, which read as one
/// run of words (a word doubled where one ends and the next begins) when only spaced.
pub fn sentences<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
            let mut sentence = first.unwrap_or_default() + chars.as_str();
            if !sentence.ends_with(['.', '!', '?']) {
                sentence.push('.');
            }
            sentence
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What an agent that passed a request up said, `by` being its name.
pub fn forwarded(by: &str, said: &str) -> String {
    format!("{by} forwards it: {said}")
}

/// Who asked and who passed it up, with the request's number: `asked by A · forwarded by B · #4`.
pub fn source<'a>(
    number: u64,
    asker: &str,
    forwarders: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut line = format!("asked by {asker}");
    for by in forwarders {
        line.push_str(&format!(" · forwarded by {by}"));
    }
    format!("{line} · #{number}")
}

/// What the Auto judge said of a request, `verdict` being `failed` when it could not decide.
pub fn judged(by: &str, verdict: &str, reason: &str) -> String {
    match verdict {
        "failed" => format!("The judge ({by}) could not decide: {reason}"),
        _ => format!("The judge ({by}) asks you: {reason}"),
    }
}
