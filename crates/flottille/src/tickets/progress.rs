//! A project's progress (#801, `docs/tickets.md`, "Progress"): the milestones of its tracker (a
//! GitHub repository's milestones, a Jira project's versions), each with its tickets grouped and
//! what Flottille knows of them (the workspaces linked to a ticket, its pull requests and their
//! checks), for the window's Progress page (`project.progress`) and the agents' `tickets
//! progress`.
//!
//! The tickets, their states and the pull requests are what the items' rounds keep
//! (`docs/items.md`): nothing of them is asked here. The milestones themselves are not in what the
//! rounds read, so they are read here, once per round of the tracker at most (`Kept`): GitHub's in
//! one `gh api` call, Jira's in one `project view` per project and, since the rounds keep no fix
//! version, one search of the keys of each version not released yet, whose answer is kept for
//! `SEARCHED_MS` (five minutes) across the rounds. One read of a project is in flight at a time:
//! a second caller waits for it and finds what it read.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use flottille_core::declared::ProjectTracker;
use flottille_core::item::{Check, Filter, Item};
use flottille_core::project::Project;
use flottille_core::store::now_ms;
use flottille_core::workspace::Workspace;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    NO_TRACKER, answered_by, github_of, jira_search, jira_site, jql_quoted, milestones, site_of,
    versions,
};
use crate::daemon::Daemon;
use crate::forge::masked;
use crate::mcp::Outcome;
use crate::projects::{self, Failure};

/// How long the milestones read before the tracker's first round are kept: until a round says
/// when the tracker was read, a page that asks again often does not read them each time.
const UNROUNDED_MS: u64 = 60_000;

/// How long the keys of a Jira version's work items are kept, whatever the rounds say: a search
/// of each version every round is more than the tracker's quota should bear.
const SEARCHED_MS: u64 = 300_000;

/// A milestone of the tracker as the page shows it, its tickets not yet placed.
#[derive(Clone, Debug)]
pub(super) struct Milestone {
    /// Unique in the project: `github-<number>`, `jira-<project>-<id>`.
    pub id: String,
    pub name: String,
    /// The Jira project of a version.
    pub project: Option<String>,
    pub open: bool,
    /// The day it is due (GitHub) or is or was released (Jira).
    pub due: Option<String>,
    pub url: Option<String>,
    /// The day it was closed, when the tracker says it (GitHub).
    pub closed: Option<String>,
    /// The keys of a Jira version's work items, when they were read; a GitHub milestone's tickets
    /// are those whose item names it.
    pub keys: Option<Vec<String>>,
    /// Why the keys of a version not released could not be read.
    pub problem: Option<String>,
}

/// The keys of one Jira version, as its last search found them.
struct Searched {
    name: String,
    at: u64,
    keys: Vec<String>,
}

/// What was read of a project's milestones, and the round of the tracker it was read after.
#[derive(Clone)]
struct Read {
    tracker: String,
    round_at: Option<u64>,
    at: u64,
    milestones: Result<Vec<Milestone>, String>,
}

static KEPT: LazyLock<Mutex<HashMap<String, Read>>> = LazyLock::new(Default::default);

/// The searches of the versions, by project of Flottille and version id.
static SEARCHED: LazyLock<Mutex<HashMap<(String, String), Searched>>> =
    LazyLock::new(Default::default);

/// A project's read in flight: whoever holds it reads, the others wait and find the result.
static READING: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    project: String,
}

/// `project.progress {project}`.
pub async fn call(daemon: std::sync::Arc<Daemon>, params: Value) -> Result<Value, Failure> {
    let asked: Asked = serde_json::from_value(params)?;
    let project = projects::load_project(&daemon, &asked.project)?;
    progress_of(&daemon, &project).await
}

/// `tickets progress`: what the Progress page shows, as lines.
pub(super) async fn said(daemon: &Daemon, project: &Project) -> Outcome {
    let progress = match progress_of(daemon, project).await {
        Ok(progress) => progress,
        Err(failure) => return Outcome::Refused(format!("refused: {}", failure.message)),
    };
    if let Some(problem) = progress["problem"].as_str() {
        return Outcome::Failed(format!("the milestones could not be read: {problem}"));
    }
    let milestones = progress["milestones"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let mut lines = vec![format!(
        "The milestones of {} and their tickets, as the last round of the tracker read them:",
        project.name
    )];
    let (open, closed): (Vec<&Value>, Vec<&Value>) =
        milestones.iter().partition(|m| m["state"] == "open");
    if open.is_empty() {
        lines.push("- no open milestone".into());
    }
    for milestone in open {
        let c = &milestone["counts"];
        let due = milestone["due"]
            .as_str()
            .map_or_else(|| "no due date".to_owned(), |day| format!("due {day}"));
        lines.push(format!(
            "- {} ({due}): {} done, {} in review, {} in progress, {} to do{}",
            text(&milestone["name"]),
            c["done"],
            c["review"],
            c["progress"],
            c["todo"],
            match c["other"].as_u64() {
                Some(0) | None => String::new(),
                Some(n) => format!(", {n} other"),
            }
        ));
        if let Some(problem) = milestone["problem"].as_str() {
            lines.push(format!("  its tickets could not be read: {problem}"));
        }
        for group in milestone["groups"].as_array().into_iter().flatten() {
            for ticket in group["tickets"].as_array().into_iter().flatten() {
                let mut on: Vec<String> = ticket["workspaces"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|w| format!("workspace {}", text(w)))
                    .collect();
                on.extend(ticket["pulls"].as_array().into_iter().flatten().map(|p| {
                    let ci = p["ci"]
                        .as_str()
                        .map(|ci| format!(", checks {ci}"))
                        .unwrap_or_default();
                    format!(
                        "pull request #{} {}{ci}",
                        text(&p["key"]),
                        text(&p["state"])
                    )
                }));
                let key = text(&ticket["key"]);
                let key = match key.parse::<u64>() {
                    Ok(_) => format!("#{key}"),
                    Err(_) => key,
                };
                let on = match on.is_empty() {
                    true => String::new(),
                    false => format!(" ({})", on.join("; ")),
                };
                lines.push(format!(
                    "  {}: {key} {}{on}",
                    text(&group["name"]),
                    text(&ticket["title"])
                ));
            }
        }
    }
    if !closed.is_empty() {
        let names: Vec<String> = closed.iter().map(|m| text(&m["name"])).collect();
        lines.push(format!("Closed: {}", names.join(", ")));
    }
    Outcome::Done(lines.join("\n"))
}

/// The milestones of `project`'s tracker with their tickets placed.
async fn progress_of(daemon: &Daemon, project: &Project) -> Result<Value, Failure> {
    let Some(tracker) = project.tracker.clone() else {
        return Err(Failure::refused(NO_TRACKER.to_owned()));
    };
    let round_at = daemon
        .read(|store| store.cursors(&project.name))?
        .into_iter()
        .find(|cursor| cursor.kind == "ticket")
        .and_then(|cursor| cursor.round_at);
    let milestones = kept_or_read(project, &tracker, round_at).await;
    let tickets = daemon.read(|store| {
        store.items(
            &project.name,
            &Filter {
                kind: Some("ticket".into()),
                ..Filter::default()
            },
        )
    })?;
    let pulls = daemon.read(|store| {
        store.items(
            &project.name,
            &Filter {
                kind: Some("pull".into()),
                ..Filter::default()
            },
        )
    })?;
    let workspaces = daemon.read(|store| store.workspaces(Some(&project.name)))?;
    let links = Links {
        tracker: &tracker,
        pulls: &pulls,
        workspaces: &workspaces,
    };
    let (milestones, problem) = match milestones {
        Ok(milestones) => (milestones, None),
        Err(why) => (Vec::new(), Some(why)),
    };
    let shown: Vec<Value> = milestones
        .iter()
        .map(|milestone| placed(milestone, &tickets, &links))
        .collect();
    Ok(json!({
        "project": project.name,
        "tracker": match tracker {
            ProjectTracker::Github { .. } => "github",
            ProjectTracker::Jira { .. } => "jira",
        },
        "milestones": shown,
        "problem": problem,
    }))
}

/// The milestones read after the tracker's last round, else read now.
async fn kept_or_read(
    project: &Project,
    tracker: &ProjectTracker,
    round_at: Option<u64>,
) -> Result<Vec<Milestone>, String> {
    let reading = READING
        .lock()
        .unwrap()
        .entry(project.name.clone())
        .or_default()
        .clone();
    let _reading = reading.lock().await;
    let said = serde_json::to_string(tracker).unwrap_or_default();
    let now = now_ms();
    if let Some(kept) = KEPT.lock().unwrap().get(&project.name)
        && kept.tracker == said
        && kept.round_at == round_at
        && (round_at.is_some() || now.saturating_sub(kept.at) < UNROUNDED_MS)
    {
        return kept.milestones.clone();
    }
    let milestones = read(project, tracker).await;
    KEPT.lock().unwrap().insert(
        project.name.clone(),
        Read {
            tracker: said,
            round_at,
            at: now,
            milestones: milestones.clone(),
        },
    );
    milestones
}

/// Every milestone of the tracker: GitHub's of every state; Jira's versions, the keys of the work
/// items of those not released yet.
async fn read(project: &Project, tracker: &ProjectTracker) -> Result<Vec<Milestone>, String> {
    match tracker {
        ProjectTracker::Github { repository } => {
            let forge = github_of(project, repository).await?;
            Ok(milestones::read(&forge, "all")
                .await?
                .into_iter()
                .map(|m| Milestone {
                    id: format!("github-{}", m.number),
                    name: masked(&m.title),
                    project: None,
                    open: m.state != "closed",
                    due: m.due,
                    url: m.url,
                    closed: m.closed_on,
                    keys: None,
                    problem: None,
                })
                .collect())
        }
        ProjectTracker::Jira { site, projects, .. } => {
            jira_site(site, "read").await?;
            let mut all = Vec::new();
            for key in projects {
                for version in versions::read(key).await? {
                    let open = !version.released && !version.archived;
                    let id = format!("jira-{key}-{}", version.id);
                    // A search that fails leaves this version's row saying so, the others show.
                    let (keys, problem) = match open {
                        true => match keys_of(project, &id, site, key, &version.name).await {
                            Ok(keys) => (Some(keys), None),
                            Err(why) => (None, Some(masked(&why))),
                        },
                        false => (None, None),
                    };
                    all.push(Milestone {
                        id,
                        name: version.shown(),
                        project: Some(key.clone()),
                        open,
                        due: version.date.clone(),
                        url: None,
                        closed: None,
                        keys,
                        problem,
                    });
                }
            }
            Ok(all)
        }
    }
}

/// The keys of the version `id`: those a search found less than `SEARCHED_MS` ago, else a search.
async fn keys_of(
    project: &Project,
    id: &str,
    site: &str,
    jira_project: &str,
    version: &str,
) -> Result<Vec<String>, String> {
    let now = now_ms();
    let at = (project.name.clone(), id.to_owned());
    if let Some(kept) = SEARCHED.lock().unwrap().get(&at)
        && kept.name == version
        && now.saturating_sub(kept.at) < SEARCHED_MS
    {
        return Ok(kept.keys.clone());
    }
    let keys = keys_in(site, jira_project, version).await?;
    SEARCHED.lock().unwrap().insert(
        at,
        Searched {
            name: version.to_owned(),
            at: now,
            keys: keys.clone(),
        },
    );
    Ok(keys)
}

/// The keys of the work items of the Jira project `project` in its version `version`.
async fn keys_in(site: &str, project: &str, version: &str) -> Result<Vec<String>, String> {
    let query = format!(
        "project = {} AND fixVersion = {} ORDER BY updated DESC",
        jql_quoted(project),
        jql_quoted(version)
    );
    let (found, _nulls) = jira_search(&query, "key,summary").await?;
    Ok(found
        .iter()
        .filter(|item| {
            item.get("self").is_none_or(Value::is_null)
                || site_of(item).is_some_and(|from| answered_by(item, &from, site))
        })
        .filter_map(|item| item["key"].as_str().map(masked))
        .collect())
}

/// What ties a ticket to the work on it.
struct Links<'a> {
    tracker: &'a ProjectTracker,
    pulls: &'a [Item],
    workspaces: &'a [Workspace],
}

impl Links<'_> {
    /// The workspaces linked to the ticket `key`.
    fn workspaces_of(&self, key: &str) -> Vec<&Workspace> {
        let linked = match self.tracker {
            ProjectTracker::Github { .. } => format!("#{key}"),
            ProjectTracker::Jira { .. } => key.to_owned(),
        };
        self.workspaces
            .iter()
            .filter(|w| w.ticket.as_deref() == Some(linked.as_str()))
            .collect()
    }

    /// The pull requests on the ticket `key`: one on the branch of a workspace linked to it, one
    /// whose description closes it (GitHub: `Closes #12`, in the tracker's repository), or one
    /// whose title or branch names it (Jira: `ABC-12`, as Jira's own links find them).
    fn pulls_of(&self, key: &str, workspaces: &[&Workspace]) -> Vec<&Item> {
        self.pulls
            .iter()
            .filter(|pull| {
                let head = pull.details["head"].as_str().unwrap_or_default();
                let on_branch = workspaces.iter().any(|w| {
                    w.branch == head && w.repositories.iter().any(|r| r.path == pull.repository)
                });
                on_branch
                    || match self.tracker {
                        ProjectTracker::Github { repository } => {
                            pull.repository == *repository
                                && closes(
                                    pull.details["description"].as_str().unwrap_or_default(),
                                    key,
                                )
                        }
                        ProjectTracker::Jira { .. } => names(&pull.title, key) || names(head, key),
                    }
            })
            .collect()
    }
}

/// Whether `text` closes the issue `#number` with one of GitHub's keywords.
fn closes(text: &str, number: &str) -> bool {
    const KEYWORDS: [&str; 9] = [
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
    ];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    words.windows(2).any(|pair| {
        let keyword = pair[0].trim_end_matches(':');
        let target = pair[1].trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        KEYWORDS.contains(&keyword) && target.strip_prefix('#') == Some(number)
    })
}

/// Whether `text` names the Jira key `key` as a word of its own.
fn names(text: &str, key: &str) -> bool {
    let upper = text.to_uppercase();
    upper.match_indices(key).any(|(at, _)| {
        let before = upper[..at].chars().next_back();
        let after = upper[at + key.len()..].chars().next();
        before.is_none_or(|c| !c.is_ascii_alphanumeric())
            && after.is_none_or(|c| !c.is_ascii_digit())
    })
}

/// A pull request's checks as one mark: failing before pending before passing.
fn ci(checks: &[Check]) -> Option<&'static str> {
    let has = |state: &str| checks.iter().any(|c| c.state == state);
    match () {
        _ if has("failing") => Some("failing"),
        _ if has("pending") => Some("pending"),
        _ if has("passing") => Some("passing"),
        _ => None,
    }
}

/// The groups of a GitHub milestone, in the order shown, with their status category.
const GITHUB_GROUPS: [(&str, &str); 4] = [
    ("To do", "new"),
    ("In progress", "indeterminate"),
    ("In review", "review"),
    ("Done", "done"),
];

/// Where Jira's status categories are shown, in Jira's order; one Flottille cannot read is
/// `other`, last.
fn category_rank(category: &str) -> usize {
    match category {
        "new" => 0,
        "indeterminate" => 1,
        "done" => 2,
        _ => 3,
    }
}

/// The milestone with its tickets in their groups, and its counts.
fn placed(milestone: &Milestone, tickets: &[Item], links: &Links<'_>) -> Value {
    let jira = matches!(links.tracker, ProjectTracker::Jira { .. });
    let held: Vec<&Item> = match (&milestone.keys, jira) {
        (Some(keys), _) => tickets.iter().filter(|t| keys.contains(&t.key)).collect(),
        (None, false) => tickets
            .iter()
            .filter(|t| t.details["milestone"].as_str() == Some(milestone.name.as_str()))
            .collect(),
        (None, true) => Vec::new(),
    };
    // (group, category, ticket)
    let mut placed: Vec<(String, String, Value)> = Vec::new();
    for ticket in held {
        let workspaces = links.workspaces_of(&ticket.key);
        let pulls = links.pulls_of(&ticket.key, &workspaces);
        let open_pull = pulls
            .iter()
            .any(|p| matches!(p.state.as_str(), "open" | "draft"));
        let (group, category) = match jira {
            true => {
                let category = ticket.details["category"]
                    .as_str()
                    .filter(|c| ["new", "indeterminate", "done"].contains(c))
                    .unwrap_or("other");
                let group = match category {
                    "other" => "Other".to_owned(),
                    _ => ticket.details["status"]
                        .as_str()
                        .unwrap_or("Other")
                        .to_owned(),
                };
                (group, category.to_owned())
            }
            false => {
                // A pull request closed without being merged is work that was dropped.
                let worked_on = pulls.iter().any(|p| p.state != "closed");
                let at = match () {
                    _ if ticket.state == "closed" => 3,
                    _ if open_pull => 2,
                    _ if !workspaces.is_empty() || worked_on => 1,
                    _ => 0,
                };
                let (group, category) = GITHUB_GROUPS[at];
                (group.to_owned(), category.to_owned())
            }
        };
        let shown = json!({
            "key": ticket.key,
            "title": ticket.title,
            "state": ticket.state,
            "status": ticket.details["status"],
            "url": ticket.url,
            "workspaces": workspaces.iter().map(|w| &w.name).collect::<Vec<_>>(),
            "pulls": pulls.iter().map(|p| json!({
                "repository": p.repository,
                "key": p.key,
                "title": p.title,
                "state": p.state,
                "ci": ci(&p.checks),
                "url": p.url,
            })).collect::<Vec<_>>(),
        });
        placed.push((group, category, shown));
    }
    let mut groups: Vec<(String, String)> = match jira {
        true => {
            let mut seen: Vec<(String, String)> = Vec::new();
            for (group, category, _) in &placed {
                if !seen.iter().any(|(g, _)| g == group) {
                    seen.push((group.clone(), category.clone()));
                }
            }
            seen.sort_by(|a, b| {
                category_rank(&a.1)
                    .cmp(&category_rank(&b.1))
                    .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
            });
            seen
        }
        false => GITHUB_GROUPS
            .iter()
            .map(|(g, c)| ((*g).to_owned(), (*c).to_owned()))
            .collect(),
    };
    groups.retain(|(group, _)| placed.iter().any(|(g, _, _)| g == group) || !jira);
    let count = |category: &str| placed.iter().filter(|(_, c, _)| c == category).count();
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|(group, category)| {
            let mut tickets: Vec<&Value> = placed
                .iter()
                .filter(|(g, _, _)| *g == group)
                .map(|(_, _, t)| t)
                .collect();
            tickets.sort_by(|a, b| key_order(a["key"].as_str(), b["key"].as_str()));
            json!({"name": group, "category": category, "tickets": tickets})
        })
        .collect();
    json!({
        "id": milestone.id,
        "name": milestone.name,
        "source": if jira { "jira" } else { "github" },
        "jira_project": milestone.project,
        "state": if milestone.open { "open" } else { "closed" },
        "due": milestone.due,
        "url": milestone.url,
        "read": !jira || milestone.keys.is_some(),
        "problem": milestone.problem,
        "closed_at": milestone.closed,
        "counts": {
            "done": count("done"),
            "review": count("review"),
            "progress": count("indeterminate"),
            "todo": count("new"),
            "other": count("other"),
            "total": placed.len(),
        },
        "groups": groups,
    })
}

/// Tickets by their number (GitHub) or the number of their key (Jira), the newest last.
fn key_order(a: Option<&str>, b: Option<&str>) -> std::cmp::Ordering {
    let number = |key: Option<&str>| -> (String, u64) {
        let key = key.unwrap_or_default();
        let (prefix, digits) = key.rsplit_once('-').unwrap_or(("", key));
        (prefix.to_owned(), digits.parse().unwrap_or(u64::MAX))
    };
    number(a).cmp(&number(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_request_closes_an_issue_with_one_of_github_s_keywords() {
        assert!(closes("This works.\n\nCloses #12", "12"));
        assert!(closes("fixes: #12.", "12"));
        assert!(!closes("Closes #123", "12"));
        assert!(!closes("See #12", "12"));
    }

    #[test]
    fn a_jira_key_is_named_as_a_word_of_its_own() {
        assert!(names("ABC-12 Export the report", "ABC-12"));
        assert!(names("feat/abc-12-export", "ABC-12"));
        assert!(!names("ABC-123", "ABC-12"));
        assert!(!names("XABC-12", "ABC-12"));
    }
}
