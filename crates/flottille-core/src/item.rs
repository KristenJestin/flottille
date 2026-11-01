//! The items of a project (`docs/items.md`): its pull requests and its tickets, as the daemon last
//! read them on their forge or tracker, and where each source's reads stand.

use serde::Serialize;
use serde_json::Value;

/// A pull request or a ticket, its texts masked.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    pub project: String,
    /// `pull` or `ticket`.
    pub kind: String,
    /// `github`, `bitbucket` or `jira`.
    pub source: String,
    /// The path of the project's repository it is of; empty for a Jira work item.
    pub repository: String,
    /// Its number, or its Jira key.
    pub key: String,
    pub title: String,
    /// `open`, `draft`, `merged` or `closed`; a Jira work item is closed once its status is in
    /// the done category, its status itself in its details.
    pub state: String,
    pub author: Option<String>,
    pub assignees: Vec<String>,
    pub labels: Vec<String>,
    pub url: Option<String>,
    /// When the forge or the tracker says it last changed, as it says it.
    pub updated_at: Option<String>,
    /// When Flottille last read it.
    pub seen_at: u64,
    /// Opened by, or assigned to, the user the source is signed in as, when it says who.
    pub mine: bool,
    /// What is proper to its kind or its source: a pull's branches, review and counts of comments
    /// and reviews, a work item's type and priority.
    pub details: Value,
    /// A pull request's checks; none for a ticket.
    pub checks: Vec<Check>,
}

/// A check or a pipeline of a pull request: the CI moves without the pull request's
/// `updated_at`, so it is kept beside it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Check {
    pub name: String,
    /// `passing`, `failing`, `pending` or `skipped`.
    pub state: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub link: Option<String>,
}

/// Where the reads of one source of a project stand: a forge's repository for its pull requests,
/// the tracker for its tickets.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Cursor {
    pub project: String,
    /// `pull` or `ticket`.
    pub kind: String,
    /// The repository's path; `tracker` for the tickets.
    pub target: String,
    pub source: String,
    /// When the last complete round started: the next asks only what changed since.
    pub cursor: Option<u64>,
    /// When the last round started, complete or not.
    pub round_at: Option<u64>,
    /// Rounds that failed in a row: each doubles the wait before the next.
    pub failures: u32,
    /// Why the last round failed.
    pub problem: Option<String>,
}

/// What `items.list` keeps.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub kind: Option<String>,
    pub state: Option<String>,
    pub repository: Option<String>,
    pub mine: bool,
}
