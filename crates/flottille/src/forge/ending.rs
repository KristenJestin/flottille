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

    /// What a job's line says in place of its error line, with how long it took and how to run it
    /// again when a call is known; none for the code's failure, which is said by its error line.
    /// Nothing is said of running it again when no call is known to be accepted.
    pub(super) fn said(&self, took: Option<u64>, rerun: Option<&str>) -> Option<String> {
        match self {
            Ending::Code => None,
            Ending::Ci(what) => Some(match rerun {
                Some(again) => format!("the CI failed, not the code: {what}; {again}"),
                None => format!("the CI failed, not the code: {what}"),
            }),
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

/// The call that runs a failed job again, once the forge is known to accept it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Rerun {
    /// Bitbucket Cloud, a pull request's pipeline: `forge` `checks` runs it again.
    Checks,
    /// Bitbucket Cloud, a branch's or a custom pipeline: `forge` `run` of the pipeline `pipeline`
    /// (as `forge pipelines` names it) on `branch`.
    Run { pipeline: String, branch: String },
    /// GitHub: `forge` `step` runs a job of a run again.
    Step { run: u64, job: u64 },
}

impl Rerun {
    /// How the line says to run it again: the call, as the agent makes it. Bitbucket cannot run
    /// one step again, so it says that what runs is the whole pipeline.
    fn said(&self, repository: &str) -> String {
        let repository = serde_json::Value::from(repository);
        const WHOLE: &str = "the whole pipeline: Bitbucket cannot run one step again";
        match self {
            Rerun::Checks => format!(
                "run it again (forge {{\"action\": \"checks\", \"repository\": {repository}}}), {WHOLE}"
            ),
            Rerun::Run { pipeline, branch } => {
                let (pipeline, branch) = (
                    serde_json::Value::from(pipeline.as_str()),
                    serde_json::Value::from(branch.as_str()),
                );
                format!(
                    "run it again (forge {{\"action\": \"run\", \"repository\": {repository}, \"pipeline\": {pipeline}, \"branch\": {branch}}}), {WHOLE}"
                )
            }
            Rerun::Step { run, job } => format!(
                "run it again (forge {{\"action\": \"step\", \"repository\": {repository}, \"run\": {run}, \"step\": \"{job}\", \"do\": \"rerun\"}})"
            ),
        }
    }
}

impl Job {
    /// What its line says of how it ended, in place of an error line: none for the code's failure.
    pub(super) fn said(&self, repository: &str) -> Option<String> {
        let again = self.rerun.as_ref().map(|rerun| rerun.said(repository));
        self.ending.said(self.took, again.as_deref())
    }

    /// The job a failing check is, its log not read yet.
    pub(super) fn of(check: &Check) -> Job {
        Job {
            name: check.name.clone(),
            details: check.link.clone(),
            log: None,
            ending: Ending::of(check.outcome.as_deref()),
            took: took(check.started_at.as_deref(), check.ended_at.as_deref()),
            rerun: None,
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
                Some("the CI failed, not the code: startup failure"),
            ),
            (Some("error"), Some("the CI failed, not the code: error")),
            (Some("cancelled"), Some("stopped, not failed")),
            (Some("stopped"), Some("stopped, not failed")),
            (Some("timed_out"), Some("timed out")),
        ] {
            assert_eq!(
                Ending::of(word).said(None, None).as_deref(),
                said,
                "{word:?}"
            );
        }
    }

    #[test]
    fn a_rerun_is_named_only_when_there_is_a_call_for_it() {
        let ci = Ending::Ci("error".into());
        let job = |rerun| Job {
            name: "deploy".into(),
            details: None,
            log: None,
            ending: ci.clone(),
            took: None,
            rerun,
        };
        assert_eq!(
            job(None).said("api").as_deref(),
            Some("the CI failed, not the code: error")
        );
        assert_eq!(
            job(Some(Rerun::Checks)).said("api").as_deref(),
            Some(
                "the CI failed, not the code: error; run it again (forge {\"action\": \"checks\", \"repository\": \"api\"}), the whole pipeline: Bitbucket cannot run one step again"
            )
        );
        assert_eq!(
            job(Some(Rerun::Run {
                pipeline: "custom: deploy".into(),
                branch: "app/login".into()
            }))
            .said("api")
            .as_deref(),
            Some(
                "the CI failed, not the code: error; run it again (forge {\"action\": \"run\", \"repository\": \"api\", \"pipeline\": \"custom: deploy\", \"branch\": \"app/login\"}), the whole pipeline: Bitbucket cannot run one step again"
            )
        );
        assert_eq!(
            job(Some(Rerun::Step { run: 900, job: 2 }))
                .said("api")
                .as_deref(),
            Some(
                "the CI failed, not the code: error; run it again (forge {\"action\": \"step\", \"repository\": \"api\", \"run\": 900, \"step\": \"2\", \"do\": \"rerun\"})"
            )
        );
    }

    #[test]
    fn a_timeout_says_after_how_long() {
        assert_eq!(
            Ending::TimedOut.said(Some(21_600), None).as_deref(),
            Some("timed out after 6 h")
        );
    }

    #[test]
    fn a_stop_says_who_when_it_is_known() {
        assert_eq!(
            Ending::Stopped(Some("Alice".into()))
                .said(None, None)
                .as_deref(),
            Some("stopped, not failed, by Alice")
        );
    }
}
