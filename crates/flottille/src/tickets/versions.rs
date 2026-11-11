//! The versions of a Jira project as the tickets tool's milestones (#764, `docs/tickets.md`,
//! "Versions"), for `tickets milestones` and for the version a work item is created in or listed
//! by (`milestone`).
//!
//! `acli` has no command that lists, makes, releases or renames a project's versions, and no flag
//! for a work item's fix version. What it has is `jira project view --json`, which is taken to
//! print Jira's REST project with its `versions` (assumed: its page gives no output), `workitem
//! create --from-json`, whose `additionalAttributes` set a field outside its flags, and JQL's
//! `fixVersion`. So the versions are read, a work item is created in one, a search filters by one,
//! and every other write of a version is refused saying so, never done through Jira's REST API
//! (the sprint's rule, `sprints.rs`: a work item's sprint is refused the same way).

use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use serde_json::Value;

use super::{NO_TRACKER, TicketsCall, acli_run, and_listed, created_in, jira_site};
use crate::forge::masked;
use crate::mcp::Outcome;

/// What the user does where Flottille cannot: Jira's own page of a project's releases.
pub(super) const IN_JIRA: &str = "the user does it in Jira, on the project's Releases page";

/// A version of a Jira project, as its REST project holds it.
#[derive(Clone)]
pub(super) struct Version {
    pub project: String,
    id: String,
    pub name: String,
    released: bool,
    /// The day it was released or is due, when it has one.
    date: Option<String>,
    archived: bool,
}

impl Version {
    fn of(project: &str, value: &Value) -> Option<Version> {
        Some(Version {
            project: project.to_owned(),
            id: value["id"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| value["id"].as_u64().map(|id| id.to_string()))?,
            name: masked(value["name"].as_str()?),
            released: value["released"] == true,
            date: value["releaseDate"]
                .as_str()
                .map(|at| masked(&at.chars().take(10).collect::<String>())),
            archived: value["archived"] == true,
        })
    }

    /// `open` is a version not released yet, `closed` a released one: the words of a GitHub
    /// milestone's states, which `state` takes.
    fn state(&self) -> &'static str {
        match self.released {
            true => "closed",
            false => "open",
        }
    }

    /// What `milestones` lists.
    fn line(&self) -> String {
        let mut line = format!("- {} {}", self.name, self.state());
        match (&self.date, self.released) {
            (Some(date), true) => line.push_str(&format!(", released {date}")),
            (Some(date), false) => line.push_str(&format!(", due {date}")),
            (None, _) => {}
        }
        if self.archived {
            line.push_str(", archived");
        }
        line
    }

    /// How a refusal names it.
    fn named(&self) -> String {
        format!("{} (id {}, {})", self.name, self.id, self.state())
    }
}

/// The versions of `project`, from the view `acli` prints of it.
async fn read(project: &str) -> Result<Vec<Version>, String> {
    let said = acli_run(&["jira", "project", "view", "--key", project, "--json"]).await?;
    let view: Value = serde_json::from_str(&said)
        .map_err(|error| format!("acli jira project view answered what is not JSON: {error}"))?;
    let listed = view["versions"].as_array().ok_or_else(|| {
        format!(
            "acli printed no versions for {project} (its project view has no versions field): \
             the versions are read there and nowhere else, since acli has no command for them; \
             {IN_JIRA}"
        )
    })?;
    Ok(listed
        .iter()
        .filter_map(|version| Version::of(project, version))
        .collect())
}

/// The versions of each of `projects`, once `acli` is checked to be signed in to the tracker's
/// site (`nothing` says what was not done otherwise: `read` or `written`).
async fn of(site: &str, projects: &[&str], nothing: &str) -> Result<Vec<Version>, String> {
    jira_site(site, nothing).await?;
    let mut all = Vec::new();
    for project in projects {
        all.extend(read(project).await?);
    }
    Ok(all)
}

/// The projects a call is about: the one named, else every project of the tracker.
fn scope<'a>(
    tracker: &ProjectTracker,
    projects: &'a [String],
    named: Option<&str>,
) -> Result<Vec<&'a str>, String> {
    match named.map(str::trim).filter(|name| !name.is_empty()) {
        Some(_) => created_in(tracker, projects, named).map(|project| vec![project]),
        None => Ok(projects.iter().map(String::as_str).collect()),
    }
}

/// The version of `all` that `named` says: by its name, else by its id. `scope` says where it was
/// looked for, for a refusal.
fn find<'a>(all: &'a [Version], scope: &[&str], named: &str) -> Result<&'a Version, String> {
    let named = named.trim();
    all.iter()
        .find(|v| v.name == named)
        .or_else(|| all.iter().find(|v| v.name.eq_ignore_ascii_case(named)))
        .or_else(|| all.iter().find(|v| v.id == named))
        .ok_or_else(|| {
            let each: Vec<String> = all.iter().map(Version::named).collect();
            let existing = match all.is_empty() {
                true => "it has none".to_owned(),
                false => format!(
                    "its versions are {}",
                    and_listed(&each.iter().map(String::as_str).collect::<Vec<_>>())
                ),
            };
            format!(
                "there is no milestone {:?} among the versions of {} ({existing}); nothing is \
                 created implicitly, and acli makes no version: {IN_JIRA}",
                masked(named),
                scope.join(", ")
            )
        })
}

/// `tickets milestones` on Jira: the versions of the tracker's projects (the one `project` names
/// alone) of a state, a line each.
pub(super) async fn listed(project: &Project, state: &str, asked: &TicketsCall) -> Outcome {
    let Some(tracker @ ProjectTracker::Jira { site, projects, .. }) = &project.tracker else {
        return Outcome::Refused(format!("refused: {NO_TRACKER}"));
    };
    let scope = match scope(tracker, projects, asked.project.as_deref()) {
        Ok(scope) => scope,
        Err(why) => return Outcome::Refused(format!("refused: {why}")),
    };
    let mut text = String::new();
    let mut count = 0;
    for project in &scope {
        let all = match of(site, &[project], "read").await {
            Ok(all) => all,
            Err(why) => {
                return Outcome::Failed(format!("the milestones could not be listed: {why}"));
            }
        };
        text.push_str(&format!("{project}:\n"));
        for version in all.iter().filter(|v| state == "all" || v.state() == state) {
            count += 1;
            text.push_str(&version.line());
            text.push('\n');
        }
    }
    let head = match state {
        "open" => "The open milestones (the unreleased versions)",
        "closed" => "The closed milestones (the released versions)",
        _ => "All milestones (the versions)",
    };
    Outcome::Done(format!(
        "{head} of {} on {site}: {count}\n{text}tickets {{\"action\": \"list\", \"milestone\": …}} \
         lists one's tickets; create takes milestone.",
        scope.join(", ")
    ))
}

/// The version a `create` puts a work item in, found in the project it is made in.
pub(super) async fn chosen(site: &str, project: &str, named: &str) -> Result<Version, String> {
    let all = of(site, &[project], "written").await?;
    find(&all, &[project], named).cloned()
}

/// The name of the version `named` says, for a list or a search to filter by: looked for in the
/// project the call names, else in every project of the tracker.
pub(super) async fn resolved(
    project: &Project,
    named: &str,
    in_project: Option<&str>,
) -> Result<String, String> {
    let Some(tracker @ ProjectTracker::Jira { site, projects, .. }) = &project.tracker else {
        return Err(NO_TRACKER.to_owned());
    };
    let scope = scope(tracker, projects, in_project)?;
    let all = of(site, &scope, "read").await?;
    find(&all, &scope, named).map(|version| version.name.clone())
}

/// What the card of a `create` says of the version it puts the work item in, found before the user
/// is asked; refused when there is none such. Nothing for another call.
pub async fn says(project: &Project, args: &Value) -> Result<Vec<String>, String> {
    let (Some("create"), Some(named), Some(tracker @ ProjectTracker::Jira { site, projects, .. })) = (
        args["action"].as_str(),
        args["milestone"]
            .as_str()
            .map(str::trim)
            .filter(|m| !m.is_empty()),
        &project.tracker,
    ) else {
        return Ok(Vec::new());
    };
    let in_project = created_in(tracker, projects, args["project"].as_str())
        .map_err(|why| format!("refused: {why}"))?;
    let version = chosen(site, in_project, named)
        .await
        .map_err(|why| format!("refused: {why}; nothing was written"))?;
    let mut says = format!(
        "It sets its fix version to {:?} ({}, {}",
        version.name,
        version.project,
        match version.released {
            true => "released",
            false => "unreleased",
        }
    );
    if let Some(date) = &version.date {
        says.push_str(&format!(", {date}"));
    }
    says.push(')');
    Ok(vec![says])
}

/// A work item's fix versions as `show` says them, from its `fields`: none when it has none.
pub(super) fn said_in(fields: &Value) -> Option<String> {
    let names: Vec<String> = fields["fixVersions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|version| version["name"].as_str().map(masked))
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}
