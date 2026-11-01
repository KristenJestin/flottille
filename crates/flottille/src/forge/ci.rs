//! How long the CI of a repository usually takes, for `forge show` and `forge subscribe`
//! (`docs/forges.md`, "How long CI takes"): an agent that subscribes to a pull request picks a
//! duration blind, one that is told the last runs' length can wait for as long as they need. One
//! read of the forge, made when one of those two runs and never by the watch; a repository whose
//! runs cannot be read is simply not told.

use serde_json::Value;

use super::{Forge, Pull, bitbucket, gh_json};
use crate::session_read::form;
use crate::subscriptions::{DEFAULT_MINUTES, LONGEST_MINUTES};

/// How many finished runs are measured: the newest.
const MEASURED: usize = 5;
/// How many runs `gh` is asked for, so that this many finished ones are found among the runs it
/// lists as completed (a cancelled run is completed too).
const ASKED: &str = "20";
/// The shortest a subscription sized by the runs waits.
const SHORTEST_MINUTES: u64 = 15;

/// The time the newest finished runs of a repository took.
#[derive(Clone, Debug, PartialEq)]
pub struct Runs {
    /// The repository as a forge names it.
    slug: String,
    /// Each run's seconds, newest first; never empty.
    seconds: Vec<u64>,
}

impl Runs {
    fn minutes(seconds: f64) -> u64 {
        (seconds / 60.0).round().max(1.0) as u64
    }

    /// The line `forge show` and `forge subscribe` say, for example `Recent CI runs of acme/app
    /// took 4 min on average (3 to 6 min, last 5).`
    pub fn line(&self) -> String {
        let total: u64 = self.seconds.iter().sum();
        let count = self.seconds.len();
        let shortest = self.seconds.iter().min().copied().unwrap_or_default();
        let longest = self.seconds.iter().max().copied().unwrap_or_default();
        format!(
            "Recent CI runs of {} took {} min on average ({} to {} min, last {count}).",
            self.slug,
            Self::minutes(total as f64 / count as f64),
            Self::minutes(shortest as f64),
            Self::minutes(longest as f64)
        )
    }
}

/// How long a subscription waits when the agent gives no `minutes`, and why, as a sentence: twice
/// the average of the runs, rounded up to the minute, from `SHORTEST_MINUTES` to
/// `LONGEST_MINUTES`; `DEFAULT_MINUTES` without runs.
pub fn wait_for(runs: Option<&Runs>) -> (u64, String) {
    let Some(runs) = runs else {
        return (
            DEFAULT_MINUTES,
            format!(
                "Duration: {DEFAULT_MINUTES} min, the default: no recent CI run to measure it by."
            ),
        );
    };
    // Twice the average, in minutes, rounded up: 2 * total / count / 60.
    let total: u64 = runs.seconds.iter().sum();
    let twice = (2 * total).div_ceil(60 * runs.seconds.len() as u64);
    let (minutes, why) = if twice < SHORTEST_MINUTES {
        (
            SHORTEST_MINUTES,
            "the shortest: twice the average of the recent CI runs is",
        )
    } else if twice > LONGEST_MINUTES {
        (
            LONGEST_MINUTES,
            "the longest: twice the average of the recent CI runs is",
        )
    } else {
        return (
            twice,
            format!("Duration: {twice} min, twice the average of the recent CI runs."),
        );
    };
    (
        minutes,
        format!("Duration: {minutes} min, {why} {twice} min."),
    )
}

/// Tells `pull` how long its repository's CI usually takes, when the forge says it.
pub(super) async fn attach(forge: &Forge, pull: &mut Pull) {
    pull.ci = recent(forge).await;
}

/// The newest finished runs of the repository's CI, when there are some and the forge gave them.
pub(super) async fn recent(forge: &Forge) -> Option<Runs> {
    let seconds = match forge {
        Forge::GitHub { .. } => github(forge).await,
        // Bitbucket Data Center has no CI API in common.
        Forge::Bitbucket { .. } if !bitbucket::cloud(forge) => return None,
        Forge::Bitbucket { .. } => {
            let context = bitbucket::Contexts::default().of(forge).await;
            match context {
                Ok(context) => bitbucket::pipelines(forge, &context).await,
                Err(why) => Err(why),
            }
        }
    }
    .ok()?;
    let seconds: Vec<u64> = seconds
        .into_iter()
        .filter(|s| *s > 0)
        .take(MEASURED)
        .collect();
    (!seconds.is_empty()).then(|| Runs {
        slug: forge.slug(),
        seconds,
    })
}

/// The seconds the finished runs of a GitHub repository took, newest first. A cancelled run, a
/// skipped one and the like measure nothing.
async fn github(forge: &Forge) -> Result<Vec<u64>, String> {
    let runs = gh_json(&[
        "run",
        "list",
        "-R",
        &forge.gh_repository(),
        "--status",
        "completed",
        "--limit",
        ASKED,
        "--json",
        "conclusion,startedAt,updatedAt",
    ])
    .await?;
    Ok(runs
        .as_array()
        .into_iter()
        .flatten()
        .filter(|run| matches!(run["conclusion"].as_str(), Some("success" | "failure")))
        .filter_map(|run| between(&run["startedAt"], &run["updatedAt"]))
        .collect())
}

/// The seconds from one RFC 3339 time to another, when both are one and the second is later.
pub(super) fn between(from: &Value, to: &Value) -> Option<u64> {
    let (from, to) = (form(from.as_str()?)?, form(to.as_str()?)?);
    Some(to.checked_sub(from)? / 1000)
}
