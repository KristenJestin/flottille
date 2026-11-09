//! A spent quota (`docs/design.md` §5.9): when a turn of a session ends on its provider reporting
//! the quota is spent (`agent.provider_limit`, whose message reads "You've hit your weekly limit ·
//! resets 1pm (Europe/Paris)"; the agent's own words are never read), the session
//! waits for the reset, stored with the instant read from those words (none when they cannot be
//! read). When the user's setting allows it (`resume_at_reset` of `sessions.toml`, or their
//! choice for this session alone), the sweep delivers the session one message at the reset, plus
//! a margin: never twice, never for an instant it could not read. A message of the user's, a
//! turn that ran without the limit, or the end of the session ends the wait.
//!
//! Every time is read from the store's clock, which a test moves ahead (`test.advance`).

use std::sync::Arc;

use flottille_core::agent::{Agent, AgentState};
use flottille_core::approval;
use flottille_core::store::{QuotaWait, Store, now_ms};
use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::deliveries;
use crate::projects::Failure;

/// How long after the reset the session is resumed: the provider's own clock may be late.
pub const MARGIN_MS: u64 = 60_000;
/// The kind of the delivery that resumes a session.
pub const KIND: &str = "quota-reset";
/// What the session is told at the reset.
pub const RESUME: &str = "Your quota has reset: continue where you stopped.";

/// What one provider's message of a reported limit says: `Some(reset)`, `reset` the instant when
/// it can be read from it, `said` being when it was said.
type Recogniser = fn(text: &str, said: u64) -> Option<Option<u64>>;

/// How far in the past a reset time is still that reset and not the next occurrence.
const JUST_PASSED_MS: i64 = 10 * 60_000;

/// The recognisers, by provider: another provider's wording is one more entry.
const RECOGNISERS: &[(&str, Recogniser)] = &[("claude", claude)];

/// Claude's bridge reports a limit whatever Claude Code's sentence is (its own for a spent usage,
/// or none known when a rate limit was rejected): the reset is after `resets`, when it is there:
/// `You've hit your weekly limit · resets 1pm (Europe/Paris)`.
fn claude(text: &str, said: u64) -> Option<Option<u64>> {
    let reset = text
        .split_once("resets ")
        .and_then(|(_, when)| reset_at(when, said));
    Some(reset)
}

/// The messages `texts` of the limits `provider` reported (the last first) say its quota is
/// spent: the reset, when it can be read. None when it reported none.
pub fn recognise(provider: &str, texts: &[String], said: u64) -> Option<Option<u64>> {
    let (_, recogniser) = RECOGNISERS.iter().find(|(name, _)| *name == provider)?;
    texts.iter().rev().find_map(|text| recogniser(text, said))
}

/// The next instant after `now` that `when` names, in the zone it names in brackets (the
/// machine's when it names none): `1pm (Europe/Paris)`, `1:30pm`, `13:00 (UTC)`, `Nov 10, 1pm
/// (Europe/Paris)`, `Nov 10 at 1pm`, `10 Nov, 1pm`. An instant passed up to `JUST_PASSED_MS` ago
/// is that one, not the next: a provider whose clock is late says a reset that came already, and
/// the next occurrence would be a day (a year) away; it is due at once, and its resume key being
/// the same, it resumes once. `None` for anything else, a zone it does not know included.
pub fn reset_at(when: &str, now: u64) -> Option<u64> {
    let when = when.trim().trim_end_matches(['.', '!']);
    let (when, zone) = match when.split_once('(') {
        Some((when, rest)) => {
            let (name, after) = rest.split_once(')')?;
            if !after.trim().trim_end_matches(['.', '!']).is_empty() {
                return None;
            }
            (when, TimeZone::get(name.trim()).ok()?)
        }
        None => (when, TimeZone::system()),
    };
    let words: Vec<String> = when
        .split([' ', ','])
        .filter(|word| !word.is_empty() && *word != "at")
        .map(str::to_lowercase)
        .collect();
    let (day, time) = match words.as_slice() {
        [time] => (None, clock(time)?),
        [time, half] if half == "am" || half == "pm" => (None, clock(&format!("{time}{half}"))?),
        [a, b, rest @ ..] => {
            let (month, day) = match (month(a), month(b)) {
                (Some(month), None) => (month, b.parse::<i8>().ok()?),
                (None, Some(month)) => (month, a.parse::<i8>().ok()?),
                _ => return None,
            };
            let time = match rest {
                [time] => clock(time)?,
                [time, half] if half == "am" || half == "pm" => clock(&format!("{time}{half}"))?,
                _ => return None,
            };
            (Some((month, day)), time)
        }
        _ => return None,
    };
    let now = Timestamp::from_millisecond(i64::try_from(now).ok()?)
        .ok()?
        .to_zoned(zone.clone());
    let at = |date: Date| -> Option<Zoned> { date.to_datetime(time).to_zoned(zone.clone()).ok() };
    let still = |at: &Zoned| {
        at.timestamp().as_millisecond() + JUST_PASSED_MS > now.timestamp().as_millisecond()
    };
    let next = match day {
        None => {
            let today = at(now.date())?;
            if still(&today) {
                today
            } else {
                at(now.date().tomorrow().ok()?)?
            }
        }
        Some((month, day)) => {
            let this_year = at(Date::new(now.year(), month, day).ok()?)?;
            if still(&this_year) {
                this_year
            } else {
                at(Date::new(now.year() + 1, month, day).ok()?)?
            }
        }
    };
    u64::try_from(next.timestamp().as_millisecond()).ok()
}

/// A time of day: `1pm`, `1:30pm`, `12am`, `13:00`.
fn clock(word: &str) -> Option<Time> {
    let (digits, half) = match word {
        word if word.ends_with("am") => (&word[..word.len() - 2], Some(false)),
        word if word.ends_with("pm") => (&word[..word.len() - 2], Some(true)),
        word => (word, None),
    };
    let (hour, minute) = match digits.split_once(':') {
        Some((hour, minute)) if minute.len() == 2 => {
            (hour.parse::<i8>().ok()?, minute.parse::<i8>().ok()?)
        }
        Some(_) => return None,
        None => (digits.parse::<i8>().ok()?, 0),
    };
    let hour = match half {
        Some(pm) if (1..=12).contains(&hour) => hour % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
        // A bare number is no time: `resets 5` says nothing sure.
        None if !digits.contains(':') => return None,
        None => hour,
    };
    Time::new(hour, minute, 0, 0).ok()
}

/// A month by its name or its first three letters.
fn month(word: &str) -> Option<i8> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let word = word.trim_end_matches('.');
    if word.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|name| name.starts_with(word))
        .map(|at| at as i8 + 1)
}

/// Whether the session is resumed at its reset: the user's choice for it, else the app's setting.
fn resumes(daemon: &Daemon, wait: &QuotaWait) -> bool {
    wait.resume.unwrap_or_else(|| {
        crate::inactive::read(&daemon.paths)
            .map(|settings| settings.resume)
            .unwrap_or(false)
    })
}

/// What `agent.list` and the tree say of a session's wait: `null` when it does not wait.
pub fn shown(daemon: &Daemon, agent: &Agent) -> Value {
    if agent.parent.is_some() {
        return Value::Null;
    }
    match daemon.read(|store| store.quota_wait(&agent.id)) {
        Ok(Some(wait)) => {
            // Nothing is resumed at an instant not read.
            let resume = wait.reset_at.is_some() && resumes(daemon, &wait);
            json!({"reset_at": wait.reset_at, "resume": resume, "limited_at": wait.limited_at})
        }
        _ => Value::Null,
    }
}

/// A turn of the root `id` ended: the limit its provider reported during it decides whether the
/// session waits for its quota's reset; a turn that ran without the limit ends a wait. Called
/// before the agent goes idle, so that the sweep never sees an old wait of a turn that went well.
pub fn turn_ended(daemon: &Arc<Daemon>, id: &str) {
    let Ok(Some(agent)) = daemon.read(|store| store.agent(id)) else {
        return;
    };
    if agent.parent.is_some() || agent.state.is_final() {
        return;
    }
    let prompted = daemon
        .read(|store| store.last_agent_events(id, "agent.prompted", 1))
        .ok()
        .and_then(|events| events.first().map(|event| event.seq))
        .unwrap_or(0);
    let texts: Vec<String> = daemon
        .read(|store| store.last_agent_events(id, "agent.provider_limit", 5))
        .unwrap_or_default()
        .into_iter()
        .rev()
        .filter(|event| event.seq > prompted)
        .map(|event| {
            event.data["message"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    match recognise(&agent.provider, &texts, now_ms()) {
        Some(reset) => {
            let _ = daemon.write(|store| store.limit_quota(id, reset));
        }
        None => end(daemon, id, "a turn ran"),
    }
}

/// The session no longer waits for its quota.
pub fn end(daemon: &Daemon, id: &str, why: &str) {
    let _ = daemon.write(|store| store.end_quota_wait(id, None, why));
}

/// The user wrote to the session: it no longer waits for its reset.
pub fn user_wrote(daemon: &Daemon, id: &str) {
    end(daemon, id, "the user wrote");
}

/// `agent.quota_resume`: the user's choice to resume this session at its reset or not.
pub fn choose(daemon: &Daemon, agent: &Agent, resume: bool) -> Result<Value, Failure> {
    let wait = daemon.write(|store| store.choose_quota_resume(&agent.id, resume))?;
    match wait {
        Some(_) => Ok(json!({"quota": shown(daemon, agent)})),
        None => Err(Failure::refused(format!(
            "agent {} does not wait for its quota's reset",
            agent.id
        ))),
    }
}

/// From the daemon's start and the sweep: each wait whose reset (plus the margin) has come ends,
/// and its session is resumed when that is allowed and it is between turns. A session in a turn
/// keeps its wait: the end of that turn says whether the quota is back.
pub fn check(daemon: &Arc<Daemon>) {
    let Ok(waits) = daemon.read(Store::quota_waits) else {
        return;
    };
    let now = now_ms();
    for wait in waits {
        let Some(at) = wait.reset_at else {
            continue;
        };
        if now < at.saturating_add(MARGIN_MS) {
            continue;
        }
        let Ok(Some(agent)) = daemon.read(|store| store.agent(&wait.agent)) else {
            continue;
        };
        if agent.state != AgentState::Idle {
            continue;
        }
        let resume = resumes(daemon, &wait);
        let why = if resume { "resumed" } else { "the quota reset" };
        // Taken in one write: the wait for this instant goes once, and only its taker resumes.
        let taken = daemon
            .write(|store| store.end_quota_wait(&agent.id, Some(at), why))
            .unwrap_or(false);
        if taken && resume {
            deliveries::deliver(
                daemon,
                &agent.id,
                KIND,
                Some(format!("quota-{}-{at}", agent.id)),
                RESUME,
                approval::BETWEEN_TURNS,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8 November 2026, 10:00 in Paris (09:00 UTC).
    const NOW: u64 = 1_794_128_400_000;

    fn paris(ms: u64) -> String {
        Timestamp::from_millisecond(ms as i64)
            .unwrap()
            .to_zoned(TimeZone::get("Europe/Paris").unwrap())
            .strftime("%Y-%m-%d %H:%M")
            .to_string()
    }

    #[test]
    fn the_reset_is_the_next_occurrence_of_its_time_in_its_zone() {
        assert_eq!(paris(NOW), "2026-11-08 10:00");
        let later = reset_at("1pm (Europe/Paris)", NOW).unwrap();
        assert_eq!(paris(later), "2026-11-08 13:00");
        let tomorrow = reset_at("9am (Europe/Paris)", NOW).unwrap();
        assert_eq!(paris(tomorrow), "2026-11-09 09:00");
        let new_york = reset_at("1:30pm (America/New_York)", NOW).unwrap();
        assert_eq!(paris(new_york), "2026-11-08 19:30");
        assert_eq!(
            paris(reset_at("13:00 (UTC)", NOW).unwrap()),
            "2026-11-08 14:00"
        );
    }

    #[test]
    fn a_reset_time_a_few_minutes_past_is_not_read_as_tomorrow() {
        // 9:55, the provider's clock being late: the reset just passed, due at once.
        assert_eq!(
            paris(reset_at("9:55am (Europe/Paris)", NOW).unwrap()),
            "2026-11-08 09:55"
        );
        assert_eq!(
            paris(reset_at("Nov 8, 9:55am (Europe/Paris)", NOW).unwrap()),
            "2026-11-08 09:55"
        );
        // Longer past, it is the next one.
        assert_eq!(
            paris(reset_at("9:49am (Europe/Paris)", NOW).unwrap()),
            "2026-11-09 09:49"
        );
        assert_eq!(
            paris(reset_at("Nov 8, 9:49am (Europe/Paris)", NOW).unwrap()),
            "2027-11-08 09:49"
        );
    }

    #[test]
    fn a_reset_with_a_date_is_that_day() {
        assert_eq!(
            paris(reset_at("Nov 10, 1pm (Europe/Paris)", NOW).unwrap()),
            "2026-11-10 13:00"
        );
        assert_eq!(
            paris(reset_at("Nov 10 at 1pm (Europe/Paris)", NOW).unwrap()),
            "2026-11-10 13:00"
        );
        assert_eq!(
            paris(reset_at("10 November, 12am (Europe/Paris)", NOW).unwrap()),
            "2026-11-10 00:00"
        );
        // Past this year: next year's.
        assert_eq!(
            paris(reset_at("Jan 2, 8am (Europe/Paris)", NOW).unwrap()),
            "2027-01-02 08:00"
        );
    }

    #[test]
    fn a_reset_that_cannot_be_read_is_none() {
        for when in [
            "soon",
            "1pm (Mars/Olympus)",
            "5 (Europe/Paris)",
            "13pm (Europe/Paris)",
            "Feb 30, 1pm (Europe/Paris)",
            "1pm (Europe/Paris) or later",
            "",
        ] {
            assert_eq!(reset_at(when, NOW), None, "{when}");
        }
    }

    #[test]
    fn claude_s_words_for_a_spent_quota_are_recognised() {
        let said = |text: &str| recognise("claude", &[text.to_owned()], NOW);
        let weekly = said("You've hit your weekly limit · resets 1pm (Europe/Paris)").unwrap();
        assert_eq!(paris(weekly.unwrap()), "2026-11-08 13:00");
        assert_eq!(said("You've hit your limit · resets soon"), Some(None));
        assert_eq!(said("You're out of extra usage"), Some(None));
        // A reported limit, whatever its sentence: it waits, its reset unknown.
        assert_eq!(said("Request rejected"), Some(None));
        assert_eq!(recognise("claude", &[], NOW), None);
        // Another provider's words are its recogniser's: none yet.
        assert_eq!(
            recognise(
                "codex",
                &["You've hit your limit · resets 1pm (Europe/Paris)".into()],
                NOW
            ),
            None
        );
    }
}
