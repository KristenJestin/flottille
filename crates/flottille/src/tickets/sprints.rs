//! The sprints of a Jira project's boards and the site's Sprint field (#362, `docs/tickets.md`,
//! "Parents and sprints"), for `tickets sprints` and a `create` in a sprint.
//!
//! `acli` 1.3.39 lists a project's boards (`jira board search`) and a board's sprints (`jira
//! board list-sprints`), and creates a work item with a custom field set only from a JSON
//! definition (`workitem create --from-json`, its `additionalAttributes`), which names the field
//! by its id. It lists no field (`acli jira field` creates, updates, deletes and restores), so the
//! Sprint field is found where it shows: on a work item that has been in a sprint, whose value
//! there is Jira's list of sprints (`[{"id", "name", "state", "boardId"}]`).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use serde_json::Value;

use super::{NO_TRACKER, TicketsCall, acli_run, and_listed, created_in, jira_site, viewed};
use crate::forge::masked;
use crate::mcp::Outcome;

/// An open sprint of a board.
pub(super) struct Sprint {
    pub id: u64,
    pub name: String,
    /// `active` or `future`.
    pub state: String,
    /// The day it ends, when it has one.
    pub ends: Option<String>,
}

impl Sprint {
    fn said(&self) -> String {
        let mut said = format!("{} [id {}], {}", self.name, self.id, self.state);
        if let Some(ends) = &self.ends {
            said.push_str(&format!(", ends {ends}"));
        }
        said
    }
}

/// A board of a project.
struct Board {
    id: String,
    name: String,
}

/// The list of a page of Jira's agile API (`{"values": [...]}`, as `board search` printed it on a
/// real site), or a bare list.
fn values(said: &str, what: &str) -> Result<Vec<Value>, String> {
    let parsed: Value = serde_json::from_str(said)
        .map_err(|error| format!("acli jira board {what} answered what is not JSON: {error}"))?;
    parsed
        .as_array()
        .or(parsed["values"].as_array())
        .cloned()
        .ok_or_else(|| format!("acli jira board {what} answered no list"))
}

/// The id of a board or a sprint, a number in Jira's answers.
fn id_of(value: &Value) -> Option<String> {
    value
        .as_u64()
        .map(|id| id.to_string())
        .or_else(|| value.as_str().map(str::to_owned))
}

async fn boards_of(project: &str) -> Result<Vec<Board>, String> {
    let said = acli_run(&["jira", "board", "search", "--project", project, "--json"]).await?;
    Ok(values(&said, "search")?
        .iter()
        .filter_map(|board| {
            Some(Board {
                id: id_of(&board["id"])?,
                name: masked(board["name"].as_str().unwrap_or_default()),
            })
        })
        .collect())
}

/// The active and future sprints of a board (**assumed** to print Jira's agile page, as `board
/// search` does: no real board with sprints could be read).
async fn open_sprints(board: &Board) -> Result<Vec<Sprint>, String> {
    let said = acli_run(&[
        "jira",
        "board",
        "list-sprints",
        "--id",
        &board.id,
        "--state",
        "active,future",
        "--json",
    ])
    .await?;
    Ok(values(&said, "list-sprints")?
        .iter()
        .filter_map(|sprint| {
            Some(Sprint {
                id: sprint["id"].as_u64()?,
                name: masked(sprint["name"].as_str()?),
                state: masked(sprint["state"].as_str().unwrap_or_default()),
                ends: sprint["endDate"]
                    .as_str()
                    .map(|at| masked(&at.chars().take(10).collect::<String>())),
            })
        })
        .collect())
}

/// How many boards of a project are read at most: one per team, rarely more.
const BOARDS: usize = 10;

/// The boards of `project` (the one named alone, by its name or its id), each with its open
/// sprints or why it has none (`acli`'s words: a board without sprints is refused by it).
async fn planned(
    project: &str,
    named: Option<&str>,
) -> Result<Vec<(Board, Result<Vec<Sprint>, String>)>, String> {
    let boards = boards_of(project).await?;
    let names: Vec<&str> = boards.iter().map(|b| b.name.as_str()).collect();
    let kept: Vec<Board> = match named.map(str::trim) {
        Some(named) => {
            let names = and_listed(&names);
            let found: Vec<Board> = boards
                .into_iter()
                .filter(|b| b.id == named || b.name.eq_ignore_ascii_case(named))
                .collect();
            if found.is_empty() {
                return Err(format!(
                    "no board of {project} is \"{}\": its boards are {names}",
                    masked(named)
                ));
            }
            found
        }
        None if boards.is_empty() => return Err(format!("{project} has no board")),
        None => boards,
    };
    let mut read = Vec::new();
    for board in kept.into_iter().take(BOARDS) {
        let sprints = open_sprints(&board).await;
        read.push((board, sprints));
    }
    Ok(read)
}

/// `tickets sprints {project?, board?}`: the open sprints of the project's boards, by board.
pub(super) async fn listed(project: &Project, asked: &TicketsCall) -> Outcome {
    let (tracker, site, projects) = match &project.tracker {
        None => return Outcome::Refused(format!("refused: {NO_TRACKER}")),
        Some(ProjectTracker::Github { .. }) => {
            return Outcome::Done(
                "GitHub issues have no sprint: a GitHub Projects board's fields are set with \
                 status"
                    .into(),
            );
        }
        Some(tracker @ ProjectTracker::Jira { site, projects }) => (tracker, site, projects),
    };
    let in_project = match created_in(tracker, projects, asked.project.as_deref()) {
        Ok(project) => project,
        Err(why) => return Outcome::Refused(format!("refused: {why}")),
    };
    if let Err(problem) = jira_site(site, "read").await {
        return Outcome::Failed(problem);
    }
    let boards = match planned(in_project, asked.board.as_deref()).await {
        Ok(boards) => boards,
        Err(why) => return Outcome::Failed(why),
    };
    let mut text = format!("The open sprints of {in_project} on {site}, by board:\n");
    for (board, sprints) in &boards {
        text.push_str(&format!("Board \"{}\" ({}):", board.name, board.id));
        match sprints {
            Ok(sprints) if sprints.is_empty() => text.push_str(" no open sprint\n"),
            Ok(sprints) => {
                text.push('\n');
                for sprint in sprints {
                    text.push_str(&format!("- {}\n", sprint.said()));
                }
            }
            Err(why) => text.push_str(&format!(" no sprint: {why}\n")),
        }
    }
    text.push_str("Give one by its name or its id as sprint to create a work item in it.");
    Outcome::Done(text)
}

/// The open sprint of `project`'s boards that `named` names, by its id or its name (any case).
pub(super) async fn chosen(project: &str, named: &str) -> Result<Sprint, String> {
    let named = named.trim();
    let mut open = Vec::new();
    let mut refused = Vec::new();
    for (board, sprints) in planned(project, None).await? {
        match sprints {
            Ok(sprints) => open.extend(sprints),
            Err(why) => refused.push(format!("\"{}\": {why}", board.name)),
        }
    }
    let (mut found, others): (Vec<Sprint>, Vec<Sprint>) = open
        .into_iter()
        .partition(|s| s.id.to_string() == named || s.name.eq_ignore_ascii_case(named));
    match found.len() {
        1 => Ok(found.remove(0)),
        0 if others.is_empty() => Err(format!(
            "\"{}\" is no open sprint of {project}: its boards have none ({}); nothing was written",
            masked(named),
            match refused.is_empty() {
                true => "no active or future sprint".to_owned(),
                false => refused.join("; "),
            }
        )),
        0 => Err(format!(
            "\"{}\" is no open sprint of {project}: its open sprints are {}; nothing was written; \
             give one of them as sprint",
            masked(named),
            and_listed(&others.iter().map(|s| s.name.as_str()).collect::<Vec<_>>())
        )),
        _ => Err(format!(
            "\"{}\" names several open sprints of {project} ({}): give its id as sprint; nothing \
             was written",
            masked(named),
            found
                .iter()
                .map(Sprint::said)
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

/// The Sprint field's id, by site: a site's own, and never changed once it has one.
static FIELDS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Mutex::default);

/// Whether a field's value is Jira's list of sprints.
fn holds_sprints(value: &Value) -> bool {
    value[0]["boardId"].is_number() && value[0]["name"].is_string()
}

/// The id of `site`'s Sprint field (`customfield_…`), found once on a work item of the tracker's
/// `projects` that has been in a sprint, then kept.
pub(super) async fn field_of(site: &str, projects: &[String]) -> Result<String, String> {
    if let Some(field) = FIELDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(site)
    {
        return Ok(field.clone());
    }
    let quoted: Vec<String> = projects.iter().map(|p| format!("\"{p}\"")).collect();
    let jql = format!(
        "project in ({}) AND sprint is not EMPTY ORDER BY updated DESC",
        quoted.join(", ")
    );
    let unknown = |why: &str| {
        format!(
            "the Sprint field of {site} is not known ({why}; acli lists no fields, so it is found \
             on a work item that has been in a sprint): create it without sprint, and the user \
             puts it in its sprint on the board; nothing was written"
        )
    };
    let said = acli_run(&[
        "jira",
        "workitem",
        "search",
        "--jql",
        &jql,
        "--fields",
        "key,summary",
        "--json",
        "--limit",
        "1",
    ])
    .await
    .map_err(|why| unknown(&why))?;
    let found = serde_json::from_str::<Value>(&said).unwrap_or_default();
    let list = found.as_array().or(found["issues"].as_array());
    let Some(key) = list
        .and_then(|items| items.first())
        .and_then(|item| item["key"].as_str())
    else {
        return Err(unknown(&format!(
            "no work item of {} has been in a sprint yet",
            and_listed(&projects.iter().map(String::as_str).collect::<Vec<_>>())
        )));
    };
    let item = viewed(key, site, "*navigable")
        .await
        .map_err(|why| unknown(&why))?;
    let field = item["fields"]
        .as_object()
        .into_iter()
        .flatten()
        .find(|(name, value)| name.starts_with("customfield_") && holds_sprints(value))
        .map(|(name, _)| name.clone())
        .ok_or_else(|| unknown(&format!("no field of {key} holds its sprints")))?;
    FIELDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(site.to_owned(), field.clone());
    Ok(field)
}

/// The sprints a work item's `fields` say it is in, for `show`: the active and future ones, or
/// the last closed one when it is in none of those; none when it never was in a sprint.
pub(super) fn said_in(fields: &Value) -> Option<String> {
    let sprints: Vec<&Value> = fields
        .as_object()?
        .iter()
        .filter(|(name, value)| name.starts_with("customfield_") && holds_sprints(value))
        .flat_map(|(_, value)| value.as_array().into_iter().flatten())
        .collect();
    let said = |sprint: &&Value| {
        masked(&format!(
            "{} ({})",
            sprint["name"].as_str().unwrap_or_default(),
            sprint["state"].as_str().unwrap_or_default()
        ))
    };
    let open: Vec<String> = sprints
        .iter()
        .filter(|sprint| sprint["state"] != "closed")
        .map(said)
        .collect();
    match open.is_empty() {
        true => sprints.last().map(said),
        false => Some(open.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_work_item_s_sprint_is_its_open_one_or_else_its_last_closed_one() {
        let open = json!({"customfield_10020": [
            {"id": 41, "name": "Sprint 41", "state": "closed", "boardId": 7},
            {"id": 42, "name": "Sprint 42", "state": "active", "boardId": 7}]});
        let closed = json!({"customfield_10020": [
            {"id": 40, "name": "Sprint 40", "state": "closed", "boardId": 7},
            {"id": 41, "name": "Sprint 41", "state": "closed", "boardId": 7}]});

        assert_eq!(said_in(&open).as_deref(), Some("Sprint 42 (active)"));
        assert_eq!(said_in(&closed).as_deref(), Some("Sprint 41 (closed)"));
        assert_eq!(said_in(&json!({"customfield_10020": null})), None);
        assert_eq!(said_in(&json!({"customfield_10001": ["x"]})), None);
    }
}
