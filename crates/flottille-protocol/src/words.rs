//! The sentences of a request's card, said the same in the window and on the phone: the daemon's
//! reasons as sentences, who asked and who forwarded, what the judge said, and why a base request
//! always comes back; and the line that says what woke an agent. One text for every client of a machine, so that two never say different
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

/// The line that says what woke the agent, from the text of a `subscription` delivery
/// (`subscriptions.rs` of the daemon), the same in the window and on the phone: `checks of #12 ended, 1 failed`, `the timer: <why>`.
pub fn woken_by(body: &str) -> String {
    if let Some(rest) = body.strip_prefix("Woken by the timer: ") {
        return format!("the timer: {}", rest.lines().next().unwrap_or_default());
    }
    let number = |text: &str| {
        let rest = &text[text.find("pull request #")? + "pull request #".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        (!digits.is_empty()).then(|| format!("#{digits}"))
    };
    let pull = number(body).unwrap_or_else(|| "the pull request".into());
    // A deadline the agent gave: `Nothing came in 5 min: your subscription to checks of …`.
    let deadline = |rest: &str| {
        let minutes = rest.split(" min").next().unwrap_or_default().to_owned();
        let events = rest
            .split_once("your subscription to ")
            .and_then(|(_, rest)| rest.split(" of ").next())
            .unwrap_or_default()
            .to_owned();
        (minutes, events)
    };
    if let Some(rest) = body.strip_prefix("Nothing came in ") {
        let (minutes, events) = deadline(rest);
        return format!("nothing came for the {events} of {pull} after {minutes} min");
    }
    if let Some(rest) = body.strip_prefix("Its deadline, ") {
        let (minutes, events) = deadline(rest);
        return format!("the deadline for the {events} of {pull} passed after {minutes} min");
    }
    if let Some(rest) = body.strip_prefix("Your subscription to ") {
        let events = rest.split(" of pull request").next().unwrap_or_default();
        let minutes = rest
            .split_once("ended after ")
            .and_then(|(_, rest)| rest.split_once(" min"))
            .map_or_else(String::new, |(minutes, _)| format!(" after {minutes} min"));
        return format!("nothing came for the {events} of {pull}{minutes}");
    }
    let mut said: Vec<String> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        let one = if let Some(rest) = line.strip_prefix("The checks ended: ") {
            let failed = rest
                .trim_end_matches('.')
                .split(", ")
                .find_map(|part| part.strip_suffix(" failed"))
                .filter(|count| *count != "0");
            match failed {
                Some(count) => format!("checks of {pull} ended, {count} failed"),
                None => format!("checks of {pull} ended, all passed"),
            }
        } else if let Some(rest) = line.strip_prefix("A review by ") {
            let who = rest.split([',', ':']).next().unwrap_or_default();
            format!("review of {pull} by {who}")
        } else if let Some(rest) = line.strip_prefix("A comment by ") {
            let who = rest.split(':').next().unwrap_or_default();
            format!("comment on {pull} by {who}")
        } else if line.starts_with("The pull request #") && line.contains(" was ") {
            let state = line.rsplit(" was ").next().unwrap_or_default();
            format!("{pull} {}", state.split(':').next().unwrap_or_default())
        } else {
            continue;
        };
        said.push(one);
    }
    match said.is_empty() {
        true => format!("news of {pull}"),
        false => said.join("; "),
    }
}
