//! How a failing check ended (`docs/forges.md`, "What kind of failure it was"): the code failed,
//! the CI itself failed, someone stopped it, or it ran out of time. An agent told only "failed"
//! goes to read the run to find out; a line that says which spares it that, the same words for
//! every forge.

use flottille_core::item::Check;

use super::Job;
use crate::session_read::form;

/// What a failing check says of itself.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ending {
    /// A step or a job failed: the code is what to look at.
    Code,
    /// The CI failed, not the code, and what the forge says of it.
    Ci(String),
    /// Cancelled or stopped, and by whom when the forge says it.
    Stopped(Option<String>),
    TimedOut,
}

impl Ending {
    /// What the forge's own word (`Check::outcome`) says: a failure of the code when it says
    /// nothing more than that it failed.
    pub(super) fn of(outcome: Option<&str>) -> Ending {
        match outcome {
            Some(word @ ("startup_failure" | "error" | "action_required")) => {
                Ending::Ci(word.replace('_', " "))
            }
            Some("cancelled" | "stopped") => Ending::Stopped(None),
            Some("timed_out") => Ending::TimedOut,
            _ => Ending::Code,
        }
    }

    /// What a job's line says in place of its error line, with how long it took; none for the
    /// code's failure, which is said by its error line.
    pub(super) fn said(&self, took: Option<u64>) -> Option<String> {
        match self {
            Ending::Code => None,
            Ending::Ci(what) => Some(format!(
                "the CI failed, not the code: {what}; run it again (forge step … do: rerun)"
            )),
            Ending::Stopped(None) => Some("stopped, not failed".into()),
            Ending::Stopped(Some(who)) => Some(format!("stopped, not failed, by {who}")),
            Ending::TimedOut => Some(match took {
                Some(seconds) => format!("timed out after {}", super::events::lasted(seconds)),
                None => "timed out".into(),
            }),
        }
    }
}

/// The seconds from `started` to `ended`, when both are times.
pub(super) fn took(started: Option<&str>, ended: Option<&str>) -> Option<u64> {
    let (start, end) = (form(started?)?, form(ended?)?);
    end.checked_sub(start).map(|ms| ms / 1000)
}

impl Job {
    /// The job a failing check is, its log not read yet.
    pub(super) fn of(check: &Check) -> Job {
        Job {
            name: check.name.clone(),
            details: check.link.clone(),
            log: None,
            ending: Ending::of(check.outcome.as_deref()),
            took: took(check.started_at.as_deref(), check.ended_at.as_deref()),
        }
    }
}

/// How many of `endings` are of each kind, in the order a wake says them: the code's failures
/// and the timeouts, the CI's, the stops.
pub(super) fn counted(endings: &[Ending]) -> (usize, usize, usize) {
    let count = |kind: fn(&Ending) -> bool| endings.iter().filter(|e| kind(e)).count();
    (
        count(|e| matches!(e, Ending::Code | Ending::TimedOut)),
        count(|e| matches!(e, Ending::Ci(_))),
        count(|e| matches!(e, Ending::Stopped(_))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forge_s_word_tells_the_code_from_the_ci() {
        for (word, said) in [
            (Some("failure"), None),
            (Some("failed"), None),
            (None, None),
            (Some("something new"), None),
            (
                Some("startup_failure"),
                Some(
                    "the CI failed, not the code: startup failure; run it again (forge step … do: rerun)",
                ),
            ),
            (
                Some("error"),
                Some("the CI failed, not the code: error; run it again (forge step … do: rerun)"),
            ),
            (Some("cancelled"), Some("stopped, not failed")),
            (Some("stopped"), Some("stopped, not failed")),
            (Some("timed_out"), Some("timed out")),
        ] {
            assert_eq!(Ending::of(word).said(None).as_deref(), said, "{word:?}");
        }
    }

    #[test]
    fn a_timeout_says_after_how_long() {
        assert_eq!(
            Ending::TimedOut.said(Some(21_600)).as_deref(),
            Some("timed out after 6 h")
        );
    }

    #[test]
    fn a_stop_says_who_when_it_is_known() {
        assert_eq!(
            Ending::Stopped(Some("Alice".into())).said(None).as_deref(),
            Some("stopped, not failed, by Alice")
        );
    }
}
