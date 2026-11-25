//! What a pilot says of the sessions it leads (`docs/design.md` §10, "A pilot sums up"): one
//! summary, the same words in every client, `leads 4 · 1 needs you · 1 red check · 1 blocked by
//! Maya`, computed from the lines of its tree.

/// A session the pilot leads, as the summary counts it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Led {
    /// It asks something of the user: a question, a vault entry, a call held, a connection.
    pub needs_you: bool,
    /// Its pull request's checks or a run it follows ended red.
    pub red: bool,
    /// Who it waits on, by name, while it is blocked by someone else; empty otherwise. Nothing
    /// says it yet (#1451): the part is there for when the line carries it.
    pub blocked_by: Vec<String>,
}

/// What a part of the summary says, which its ink follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// How many it leads.
    Leads,
    NeedsYou,
    Red,
    Blocked,
}

/// One part of the summary, in its words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub kind: Kind,
    pub words: String,
}

/// The summary of a pilot that leads `led` (its live led sessions) and has `held` launches of
/// its own held for the user's approval, which need the user too. `None` while it leads nothing:
/// a part with a zero is never said, and neither is a summary of nothing.
pub fn summary(led: &[Led], held: usize) -> Option<Vec<Part>> {
    if led.is_empty() {
        return None;
    }
    let part = |kind, words: String| Part { kind, words };
    let mut parts = vec![part(Kind::Leads, format!("leads {}", led.len()))];
    let needs = led.iter().filter(|l| l.needs_you).count() + held;
    if needs > 0 {
        let verb = if needs == 1 { "needs" } else { "need" };
        parts.push(part(Kind::NeedsYou, format!("{needs} {verb} you")));
    }
    let red = led.iter().filter(|l| l.red).count();
    if red > 0 {
        let checks = if red == 1 { "check" } else { "checks" };
        parts.push(part(Kind::Red, format!("{red} red {checks}")));
    }
    let blocked: Vec<&Led> = led.iter().filter(|l| !l.blocked_by.is_empty()).collect();
    if !blocked.is_empty() {
        let mut by: Vec<&str> = Vec::new();
        for name in blocked.iter().flat_map(|l| &l.blocked_by) {
            if !by.contains(&name.as_str()) {
                by.push(name);
            }
        }
        parts.push(part(
            Kind::Blocked,
            format!("{} blocked by {}", blocked.len(), by.join(", ")),
        ));
    }
    Some(parts)
}

/// The summary in words, its parts joined: `leads 4 · 1 needs you`.
pub fn words(parts: &[Part]) -> String {
    parts
        .iter()
        .map(|part| part.words.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(led: &[Led], held: usize) -> Option<String> {
        summary(led, held).map(|parts| words(&parts))
    }

    #[test]
    fn a_session_that_leads_nothing_has_no_summary() {
        assert_eq!(said(&[], 0), None);
        // Its own held launch is no session it leads yet.
        assert_eq!(said(&[], 1), None);
    }

    #[test]
    fn the_summary_counts_what_needs_the_user_red_checks_and_who_blocks() {
        let quiet = Led::default();
        let needs = Led {
            needs_you: true,
            ..Led::default()
        };
        let red = Led {
            red: true,
            ..Led::default()
        };
        let maya = Led {
            blocked_by: vec!["Maya".into()],
            ..Led::default()
        };
        assert_eq!(
            said(std::slice::from_ref(&quiet), 0).as_deref(),
            Some("leads 1")
        );
        assert_eq!(
            said(
                &[quiet.clone(), needs.clone(), red.clone(), maya.clone()],
                0
            )
            .as_deref(),
            Some("leads 4 · 1 needs you · 1 red check · 1 blocked by Maya")
        );
        let noor = Led {
            blocked_by: vec!["Noor".into(), "Maya".into()],
            ..Led::default()
        };
        assert_eq!(
            said(&[needs.clone(), needs, red.clone(), red, maya, noor], 0).as_deref(),
            Some("leads 6 · 2 need you · 2 red checks · 2 blocked by Maya, Noor")
        );
        let parts = summary(&[quiet, Led::default()], 0).unwrap();
        assert_eq!(parts.len(), 1, "a part with a zero is not said: {parts:?}");
        assert_eq!(parts[0].kind, Kind::Leads);
    }

    #[test]
    fn a_held_launch_of_the_pilot_counts_as_needing_the_user() {
        let led = [Led::default()];
        assert_eq!(said(&led, 1).as_deref(), Some("leads 1 · 1 needs you"));
        let parts = summary(&led, 1).unwrap();
        assert_eq!(parts[1].kind, Kind::NeedsYou);
        let asking = [Led {
            needs_you: true,
            ..Led::default()
        }];
        assert_eq!(said(&asking, 2).as_deref(), Some("leads 1 · 3 need you"));
    }
}
