//! `tickets fields` (`docs/tickets.md`, "Fields of a type"): the fields of a Jira work item type,
//! with their ids, read from the site's create metadata, and the custom fields a work item holds.
//!
//! `acli` lists no field of a type (`jira field` creates and deletes them, and `workitem view`
//! prints the fields of one work item), so the metadata is read with Jira's REST API and the vault
//! entry the project names for `log_time` and `worklogs`, by the client those two use: the daemon
//! holds the secret, it goes to the tracker's site alone, and it is in no answer. A read, never
//! held.

use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use serde_json::Value;

use super::worklog::no_entry_to;
use super::worklogs::{Site, signed_in};
use super::{TicketsCall, WorkItemType, created_in, sprints, types_said};
use crate::daemon::Daemon;
use crate::forge::masked;
use crate::mcp::Outcome;

/// Fields every creation says by itself (`create` takes the project, the type and the title): left
/// out of what a type requires, so that an agent never puts them in `fields`.
const SAID_BY_CREATE: [&str; 3] = ["summary", "project", "issuetype"];
/// How many allowed values of one field are said, and how many types a list without a kind reads.
const ALLOWED: usize = 25;
const TYPES: usize = 30;
/// What the site gives at most in one page of its create metadata, and how many pages are read.
const PAGE: usize = 200;
const PAGES: usize = 5;
/// How much of a custom field's value `show` quotes.
const VALUE_CHARS: usize = 500;

/// A work item type of the project, as the create metadata lists it: its id is what the fields'
/// address takes.
struct Kind {
    id: String,
    kind: WorkItemType,
}

/// One field of a type, as the create metadata describes it.
struct Field {
    id: String,
    name: String,
    required: bool,
    /// `option`, `array of option`, `string`…
    schema: String,
    allowed: Vec<String>,
}

impl Field {
    fn of(field: &Value) -> Option<Field> {
        let schema = match (
            field["schema"]["type"].as_str(),
            field["schema"]["items"].as_str(),
        ) {
            (Some("array"), Some(items)) => format!("array of {items}"),
            (Some(kind), _) => kind.to_owned(),
            (None, _) => "unknown".to_owned(),
        };
        Some(Field {
            id: masked(field["fieldId"].as_str()?),
            name: masked(field["name"].as_str().unwrap_or_default()),
            required: field["required"] == true,
            schema,
            allowed: field["allowedValues"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| {
                    // An option has its `value`, a priority or a component its `name`.
                    value["value"].as_str().or_else(|| value["name"].as_str())
                })
                .map(masked)
                .collect(),
        })
    }

    /// `customfield_10100: Severity (option), allowed values: Minor, Major`.
    fn said(&self) -> String {
        let mut said = format!("{}: {} ({})", self.id, self.name, self.schema);
        if !self.allowed.is_empty() {
            let shown: Vec<&str> = self
                .allowed
                .iter()
                .take(ALLOWED)
                .map(String::as_str)
                .collect();
            said.push_str(&format!(", allowed values: {}", shown.join(", ")));
            if let Some(more) = self.allowed.len().checked_sub(ALLOWED).filter(|n| *n > 0) {
                said.push_str(&format!(" and {more} more"));
            }
        }
        said
    }

    /// `Severity (customfield_10100: Minor, Major)`: what a refusal says of a field it lacks.
    fn lacked(&self) -> String {
        match self.allowed.is_empty() {
            true => format!("{} ({})", self.name, self.id),
            false => {
                let shown: Vec<&str> = self
                    .allowed
                    .iter()
                    .take(ALLOWED)
                    .map(String::as_str)
                    .collect();
                format!("{} ({}: {})", self.name, self.id, shown.join(", "))
            }
        }
    }
}

/// The pages of a list the site gives under `list`, from `startAt`, as one.
async fn paged(jira: &Site<'_>, path: &str, list: &str) -> Result<Vec<Value>, String> {
    let mut all = Vec::new();
    for _ in 0..PAGES {
        let start = all.len().to_string();
        let page = jira
            .get(
                path,
                &[("startAt", &start), ("maxResults", &PAGE.to_string())],
            )
            .await?;
        let items = page[list].as_array().cloned().unwrap_or_default();
        let done = items.is_empty()
            || page["total"]
                .as_u64()
                .is_some_and(|total| all.len() + items.len() >= total as usize);
        all.extend(items);
        if done {
            break;
        }
    }
    Ok(all)
}

/// The work item types of `project` with their ids, custom ones included.
async fn kinds(jira: &Site<'_>, project: &str) -> Result<Vec<Kind>, String> {
    let listed = paged(
        jira,
        &format!("issue/createmeta/{project}/issuetypes"),
        "issueTypes",
    )
    .await?;
    Ok(listed
        .iter()
        .filter_map(|kind| {
            Some(Kind {
                id: kind["id"].as_str()?.to_owned(),
                kind: WorkItemType {
                    name: masked(kind["name"].as_str()?),
                    subtask: kind["subtask"] == true,
                },
            })
        })
        .collect())
}

/// The fields of one type, the required ones the creation says by itself left out.
async fn fields_of(jira: &Site<'_>, project: &str, kind: &Kind) -> Result<Vec<Field>, String> {
    let path = format!("issue/createmeta/{project}/issuetypes/{}", kind.id);
    let listed = paged(jira, &path, "fields").await?;
    Ok(listed
        .iter()
        .filter_map(Field::of)
        .filter(|field| !SAID_BY_CREATE.contains(&field.id.as_str()))
        .collect())
}

/// How to give a field.
const HOW: &str = "Give a field in fields of create or edit, keyed by its id and valued as Jira \
    takes it (a select: {\"value\": \"Major\"}, a user: {\"accountId\": \"...\"}, a list of them \
    for a multi-select); create takes the title, the description, the labels, the assignee and the \
    parent by their own arguments";

/// `tickets fields`: the fields of a work item type of a Jira project, or of each of its types;
/// on GitHub, what an issue has in place of fields.
pub(super) async fn fields(daemon: &Daemon, project: &Project, asked: &TicketsCall) -> Outcome {
    match &project.tracker {
        None => Outcome::Refused(format!("refused: {}", super::NO_TRACKER)),
        Some(ProjectTracker::Github { .. }) => Outcome::Done(
            "A GitHub issue has no custom fields. What it has: labels, assignees, a milestone and, \
             when the repository has issue types, a type. create and edit take the labels, the \
             assignees and the milestone; Flottille neither lists nor sets an issue's type, so \
             labels are what classifies one."
                .into(),
        ),
        Some(tracker @ ProjectTracker::Jira { site, projects, .. }) => {
            let in_project = match created_in(tracker, projects, asked.project.as_deref()) {
                Ok(project) => project,
                Err(why) => return Outcome::Refused(format!("refused: {why}")),
            };
            let none = no_entry_to("read a type's fields with", "calls Jira's API");
            let (jira, _, _) = match signed_in(daemon, project, site, none).await {
                Ok(signed) => signed,
                Err(why) => return Outcome::Refused(why),
            };
            // `kind`, as the tool names it, or `type`, as `create` does: the same thing.
            let asked_kind = asked
                .link_kind
                .as_deref()
                .or(asked.kind.as_deref())
                .map(str::trim)
                .filter(|k| !k.is_empty());
            match read(&jira, site, in_project, asked_kind).await {
                Ok(said) => Outcome::Done(said),
                Err(Outcome::Failed(why)) => {
                    Outcome::Failed(format!("the fields could not be read: {why}"))
                }
                Err(other) => other,
            }
        }
    }
}

async fn read(
    jira: &Site<'_>,
    site: &str,
    project: &str,
    asked: Option<&str>,
) -> Result<String, Outcome> {
    let failed = Outcome::Failed;
    let kinds = kinds(jira, project).await.map_err(failed)?;
    let Some(asked) = asked else {
        return types_with_required(jira, site, project, &kinds).await;
    };
    let Some(kind) = kinds
        .iter()
        .find(|kind| kind.kind.name.eq_ignore_ascii_case(asked))
    else {
        let listed: Vec<WorkItemType> = kinds.iter().map(|kind| kind.kind.clone()).collect();
        return Err(Outcome::Refused(format!(
            "refused: \"{}\" is not a work item type of {project}: its types are {}",
            masked(asked),
            types_said(&listed)
        )));
    };
    let fields = fields_of(jira, project, kind).await.map_err(failed)?;
    let mut said = format!(
        "Fields of the work item type {} of {project} on {site} (Jira's create metadata):\n",
        kind.kind.name
    );
    for (title, required) in [("Required", true), ("Optional", false)] {
        let these: Vec<&Field> = fields.iter().filter(|f| f.required == required).collect();
        match (these.is_empty(), required) {
            (true, true) => said.push_str("Required: none beyond what every creation says.\n"),
            (true, false) => said.push_str("Optional: none.\n"),
            (false, _) => {
                said.push_str(&format!("{title}:\n"));
                for field in these {
                    said.push_str(&format!("- {}\n", field.said()));
                }
            }
        }
    }
    said.push_str(HOW);
    said.push('.');
    Ok(said)
}

/// Each type of the project with the fields it requires.
async fn types_with_required(
    jira: &Site<'_>,
    site: &str,
    project: &str,
    kinds: &[Kind],
) -> Result<String, Outcome> {
    let mut said =
        format!("The work item types of {project} on {site} and their required fields:\n");
    for kind in kinds.iter().take(TYPES) {
        let fields = fields_of(jira, project, kind)
            .await
            .map_err(Outcome::Failed)?;
        let required: Vec<String> = fields
            .iter()
            .filter(|field| field.required)
            .map(Field::said)
            .collect();
        let name = match kind.kind.subtask {
            true => format!("{} (a sub-task)", kind.kind.name),
            false => kind.kind.name.clone(),
        };
        match required.is_empty() {
            true => said.push_str(&format!(
                "- {name}: no required field beyond what every creation says\n"
            )),
            false => said.push_str(&format!("- {name}: {}\n", required.join("; "))),
        }
    }
    if let Some(more) = kinds.len().checked_sub(TYPES).filter(|n| *n > 0) {
        said.push_str(&format!("… and {more} more types: ask for one with kind\n"));
    }
    said.push_str("fields with a kind lists all the fields of a type, the optional ones too. ");
    said.push_str(HOW);
    said.push('.');
    Ok(said)
}

/// What a creation `acli` refused for a required field adds to Jira's words: the fields `kind`
/// requires that `given` (the creation's `fields`) does not set, by name and id, read with the
/// project's entry. `None` when the project has no entry, the site does not answer or the type
/// requires nothing more: the refusal then says what it said before.
pub(super) async fn lacked(
    daemon: &Daemon,
    project: &Project,
    (site, in_project): (&str, &str),
    kind: &str,
    given: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    let (jira, _, _) = signed_in(daemon, project, site, String::new()).await.ok()?;
    let kinds = kinds(&jira, in_project).await.ok()?;
    let kind = kinds
        .iter()
        .find(|known| known.kind.name.eq_ignore_ascii_case(kind))?;
    let fields = fields_of(&jira, in_project, kind).await.ok()?;
    let lacking: Vec<String> = fields
        .iter()
        .filter(|field| field.required && !given.is_some_and(|g| g.contains_key(&field.id)))
        .map(Field::lacked)
        .collect();
    (!lacking.is_empty()).then(|| {
        format!(
            "{} requires, and fields does not give: {}; tickets fields {{\"kind\": \"{}\"}} \
             lists every field of the type",
            kind.kind.name,
            lacking.join(", "),
            kind.kind.name
        )
    })
}

/// The custom fields of a work item that hold a value, one line each, for `show` with `detail
/// full`: the id, the name when the site said it (`names`, when `acli` prints it), and the value
/// (a select's chosen option by its text). The sprint field is the sprint's, said as such.
pub(super) fn held_in(item: &Value) -> Vec<String> {
    let Some(fields) = item["fields"].as_object() else {
        return Vec::new();
    };
    let mut held: Vec<(&String, String)> = fields
        .iter()
        .filter(|(id, value)| id.starts_with("customfield_") && !sprints::holds_sprints(value))
        .map(|(id, value)| (id, value_text(value)))
        .filter(|(_, text)| !text.is_empty())
        .collect();
    held.sort_by(|a, b| a.0.cmp(b.0));
    held.into_iter()
        .map(|(id, text)| {
            let text = super::cut(&masked(&text), VALUE_CHARS).replace('\n', "\n    ");
            match item["names"][id.as_str()].as_str() {
                Some(name) => format!("{} ({id}): {text}", masked(name)),
                None => format!("{id}: {text}"),
            }
        })
        .collect()
}

/// A field's value as words: a text, a number, an option by its text, a document as Markdown, a
/// list of them joined; nothing for an empty one.
fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.trim().to_owned(),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
        Value::Array(items) => items
            .iter()
            .map(value_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) if value["type"] == "doc" => super::document_text(value),
        Value::Object(map) => ["value", "name", "displayName", "key"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .map_or_else(|| value.to_string(), str::to_owned),
    }
}
