//! The milestones of a GitHub tracker (#758, `docs/tickets.md`, "Milestones"), for `tickets
//! milestones`, `milestone_create` and `milestone_edit`, and for the milestone a ticket is put in
//! (`create`, `edit`) or listed by (`list`, `search`).
//!
//! `gh issue` names a milestone by its title and has no command to make one, so the milestones
//! themselves go through `gh api` of the tracker's repository (`repos/<owner>/<name>/milestones`,
//! the repository the project's tracker declares, never one an agent names). A milestone is named
//! by an agent by its title or its number and found in the repository's list first: the title is
//! what `gh` is then given, one that does not exist is refused listing those that do, and nothing
//! is created implicitly. On Jira a milestone is a version of a project (`versions.rs`).

use flottille_core::declared::{self, ProjectTracker};
use flottille_core::project::Project;
use serde_json::Value;

use super::versions::{self, IN_JIRA};
use super::{NO_TRACKER, TicketsCall, github_of};
use crate::forge::{self, Forge, masked};
use crate::mcp::Outcome;

/// The actions that are about milestones, and the arguments of the tool that only they take, each
/// with the actions that take it.
const ACTIONS: [&str; 3] = ["milestones", "milestone_create", "milestone_edit"];
const ARGUMENTS: [(&str, &[&str]); 4] = [
    (
        "milestone",
        &["list", "search", "create", "edit", "milestone_edit"],
    ),
    ("remove_milestone", &["edit"]),
    ("due", &["milestone_create", "milestone_edit"]),
    ("description", &["milestone_create", "milestone_edit"]),
];

/// A milestone of the tracker's repository, as GitHub's API says it.
struct Milestone {
    number: u64,
    title: String,
    state: String,
    /// The day it is due, when it has one.
    due: Option<String>,
    open: u64,
    closed: u64,
    url: Option<String>,
}

impl Milestone {
    fn of(value: &Value) -> Option<Milestone> {
        Some(Milestone {
            number: value["number"].as_u64()?,
            title: value["title"].as_str()?.to_owned(),
            state: value["state"].as_str().unwrap_or("open").to_owned(),
            due: value["due_on"]
                .as_str()
                .and_then(|at| at.get(..10))
                .map(str::to_owned),
            open: value["open_issues"].as_u64().unwrap_or(0),
            closed: value["closed_issues"].as_u64().unwrap_or(0),
            url: value["html_url"].as_str().map(masked),
        })
    }

    /// What `milestones` lists.
    fn line(&self) -> String {
        let mut line = format!("- {} {}", masked(&self.title), self.state);
        if let Some(due) = &self.due {
            line.push_str(&format!(", due {due}"));
        }
        line.push_str(&format!(
            ", {} open / {} closed issues",
            self.open, self.closed
        ));
        if let Some(url) = &self.url {
            line.push_str(&format!("  {url}"));
        }
        line
    }

    /// How a refusal names it.
    fn named(&self) -> String {
        format!("{} (#{}, {})", masked(&self.title), self.number, self.state)
    }
}

/// Whether `text` is a day, `YYYY-MM-DD`, that the calendar has.
fn is_day(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    let [year, month, day] = parts[..] else {
        return false;
    };
    let number = |text: &str, size: usize| {
        (text.len() == size && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
    };
    let (Some(year), Some(month), Some(day)) = (number(year, 4), number(month, 2), number(day, 2))
    else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

/// What the refusal of a version's write says: `acli` has no command for it, and the daemon
/// calls no Jira REST API for it, so it is the user's, on Jira's own page.
fn cannot_make(what: &str) -> String {
    format!(
        "refused: tickets {what} on Jira: none of acli's jira commands makes, changes or releases \
         a project's version, and Flottille calls no Jira REST API for it: {IN_JIRA}; nothing was \
         written"
    )
}

/// The refusal of a fix version changed on a work item that exists.
fn cannot_move() -> String {
    format!(
        "refused: acli's workitem edit takes no fix version (it has no flag for it, and its JSON \
         takes no additionalAttributes): changing the fix version of an existing work item needs \
         Jira's REST API, which Flottille does not call; the user changes it in Jira (the work \
         item's Fix versions field, or {IN_JIRA}); nothing was written"
    )
}

/// A text the agent gave, trimmed, none when it is blank.
fn given<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// Why a milestone's action or argument cannot be done, before anyone is asked: an argument of an
/// action that takes none, Jira's, a call that is missing what it needs. None for a call that has
/// nothing to do with milestones.
pub(super) fn unusable(project: &Project, action: &str, args: &Value) -> Option<String> {
    for (key, takers) in ARGUMENTS {
        let set = !matches!(args[key], Value::Null | Value::Bool(false));
        if set && !takers.contains(&action) {
            let (last, others) = takers.split_last()?;
            let takers = match others.is_empty() {
                true => (*last).to_owned(),
                false => format!("{} and {last}", others.join(", ")),
            };
            return Some(format!(
                "refused: tickets {action} takes no {key}: {key} is for {takers}"
            ));
        }
    }
    let asked = ACTIONS.contains(&action)
        || given(args, "milestone").is_some()
        || args["remove_milestone"] == true;
    if !asked {
        return None;
    }
    if matches!(project.tracker, Some(ProjectTracker::Jira { .. })) {
        match action {
            "milestone_create" | "milestone_edit" => return Some(cannot_make(action)),
            "edit" => return Some(cannot_move()),
            _ => {}
        }
    }
    if given(args, "milestone").is_some() && args["remove_milestone"] == true {
        return Some(
            "refused: tickets edit takes milestone or remove_milestone, not both: a ticket is in \
             a milestone or in none"
                .into(),
        );
    }
    if let Some(due) = given(args, "due")
        && !is_day(due)
    {
        return Some(format!(
            "refused: due is a day, YYYY-MM-DD ({:?} is not one)",
            masked(due)
        ));
    }
    match action {
        "milestone_create" if !args["state"].is_null() => Some(
            "refused: tickets milestone_create takes no state: a new milestone is open, \
             milestone_edit closes it"
                .into(),
        ),
        "milestone_create" if given(args, "title").is_none() => {
            Some("refused: tickets milestone_create needs title, the milestone's name".into())
        }
        "milestone_edit" if given(args, "milestone").is_none() => Some(
            "refused: tickets milestone_edit needs milestone, the title or the number of the \
             one to change"
                .into(),
        ),
        "milestone_edit"
            if ["title", "description", "due", "state"]
                .iter()
                .all(|key| given(args, key).is_none()) =>
        {
            Some(
                "refused: tickets milestone_edit needs title, description, due or state: what \
                 changes"
                    .into(),
            )
        }
        "milestone_edit" => given(args, "state")
            .filter(|state| !["open", "closed"].contains(state))
            .map(|state| {
                format!(
                    "refused: tickets milestone_edit takes a state open or closed, not {:?}",
                    masked(state)
                )
            }),
        _ => None,
    }
}

/// The GitHub repository of the project's tracker.
async fn forge_of(project: &Project) -> Result<Forge, String> {
    match &project.tracker {
        Some(ProjectTracker::Github { repository }) => github_of(project, repository).await,
        Some(ProjectTracker::Jira { .. }) => Err(cannot_make("milestones")),
        None => Err(NO_TRACKER.to_owned()),
    }
}

/// Every milestone of the repository of `state` (`open`, `closed` or `all`), page after page: `gh
/// api` prints each page's list one after another.
async fn read(forge: &Forge, state: &str) -> Result<Vec<Milestone>, String> {
    forge::signed_in().await?;
    let path = format!(
        "repos/{}/milestones?state={state}&per_page=100",
        forge.slug()
    );
    let said =
        forge::gh_text(&["api", &path, "--hostname", declared::GITHUB, "--paginate"]).await?;
    let mut found = Vec::new();
    for page in serde_json::Deserializer::from_str(&said).into_iter::<Value>() {
        let page =
            page.map_err(|error| format!("gh api {path} answered what is not JSON: {error}"))?;
        found.extend(
            page.as_array()
                .into_iter()
                .flatten()
                .filter_map(Milestone::of),
        );
    }
    Ok(found)
}

/// The milestone of `all` that `named` says: by its title, else by its number.
fn find<'a>(all: &'a [Milestone], slug: &str, named: &str) -> Result<&'a Milestone, String> {
    let named = named.trim();
    all.iter()
        .find(|m| m.title == named)
        .or_else(|| all.iter().find(|m| m.title.eq_ignore_ascii_case(named)))
        .or_else(|| {
            let number = named.parse::<u64>().ok()?;
            all.iter().find(|m| m.number == number)
        })
        .ok_or_else(|| {
            let existing = match all.is_empty() {
                true => "it has none".to_owned(),
                false => format!(
                    "its milestones are {}",
                    all.iter()
                        .map(Milestone::named)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            format!(
                "there is no milestone {:?} in {slug} ({existing}); nothing is created \
                 implicitly: milestone_create makes one",
                masked(named)
            )
        })
}

/// That no milestone of `all` has the title `title` (another than `except`): GitHub keeps one
/// milestone to a title, closed or open.
fn free(all: &[Milestone], slug: &str, title: &str, except: Option<u64>) -> Result<(), String> {
    match all
        .iter()
        .find(|m| m.title.eq_ignore_ascii_case(title) && Some(m.number) != except)
    {
        Some(taken) => Err(format!(
            "the milestone {} already exists in {slug}: a title is used once, closed or open; \
             milestone_edit changes it",
            taken.named()
        )),
        None => Ok(()),
    }
}

/// The title of the milestone `named`, for `gh issue` to be given: refused, listing the existing
/// ones, when there is none.
pub(super) async fn resolved(forge: &Forge, named: &str) -> Result<String, String> {
    let all = read(forge, "all").await?;
    find(&all, &forge.slug(), named).map(|milestone| milestone.title.clone())
}

/// `tickets milestones`: the repository's milestones of a state, a line each.
pub(super) async fn listed(project: &Project, asked: &TicketsCall) -> Outcome {
    let state = asked.state.as_deref().unwrap_or("open");
    if !["open", "closed", "all"].contains(&state) {
        return Outcome::Refused(format!(
            "refused: tickets has no state {state:?}: its states are open (the default), closed \
             and all"
        ));
    }
    if matches!(project.tracker, Some(ProjectTracker::Jira { .. })) {
        return versions::listed(project, state, asked).await;
    }
    let forge = match forge_of(project).await {
        Ok(forge) => forge,
        Err(why) => return Outcome::Refused(format!("refused: {why}")),
    };
    let all = match read(&forge, state).await {
        Ok(all) => all,
        Err(why) => return Outcome::Failed(format!("the milestones could not be listed: {why}")),
    };
    let head = match state {
        "open" => "The open milestones",
        "closed" => "The closed milestones",
        _ => "All milestones",
    };
    let mut text = format!("{head} of {}: {}\n", forge.slug(), all.len());
    for milestone in &all {
        if let Some(url) = &milestone.url {
            crate::web::shown(url, crate::web::Source::Link);
        }
        text.push_str(&milestone.line());
        text.push('\n');
    }
    text.push_str("tickets {\"action\": \"list\", \"milestone\": …} lists one's tickets.");
    Outcome::Done(text)
}

/// `tickets milestone_create`: a new milestone, its title not used yet.
pub(super) async fn create(project: &Project, asked: &TicketsCall) -> Outcome {
    let Some(title) = asked
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return Outcome::Refused("refused: tickets milestone_create needs title".into());
    };
    let forge = match forge_of(project).await {
        Ok(forge) => forge,
        Err(why) => return Outcome::Refused(format!("refused: {why}; nothing was written")),
    };
    let slug = forge.slug();
    let all = match read(&forge, "all").await {
        Ok(all) => all,
        Err(why) => return Outcome::Failed(why),
    };
    if let Err(why) = free(&all, &slug, title, None) {
        return Outcome::Refused(format!("refused: {why}; nothing was written"));
    }
    let path = format!("repos/{slug}/milestones");
    let mut args = vec![
        "api",
        &path,
        "--hostname",
        declared::GITHUB,
        "--method",
        "POST",
    ];
    let fields = [
        ("title", Some(title.to_owned())),
        (
            "description",
            asked.description.as_deref().map(str::to_owned),
        ),
        ("due_on", asked.due.as_deref().map(due_on)),
    ];
    let fields: Vec<String> = fields
        .into_iter()
        .filter_map(|(key, value)| Some(format!("{key}={}", value.filter(|v| !v.is_empty())?)))
        .collect();
    for field in &fields {
        args.extend(["-f", field]);
    }
    match forge::gh_json(&args).await {
        Ok(made) => {
            let url = made["html_url"].as_str().map(masked);
            if let Some(url) = &url {
                crate::web::shown(url, crate::web::Source::Link);
            }
            Outcome::Done(format!(
                "The milestone {title:?} was created in {slug}{}{}",
                made["number"]
                    .as_u64()
                    .map_or(String::new(), |n| format!(" (#{n})")),
                url.map_or(String::new(), |url| format!(": {url}"))
            ))
        }
        Err(why) => Outcome::Failed(why),
    }
}

/// A day as GitHub's `due_on` takes it.
fn due_on(day: &str) -> String {
    format!("{}T00:00:00Z", day.trim())
}

/// `tickets milestone_edit`: a milestone renamed, described, dated, closed or reopened.
pub(super) async fn edit(project: &Project, asked: &TicketsCall) -> Outcome {
    let Some(named) = asked.milestone.as_deref() else {
        return Outcome::Refused("refused: tickets milestone_edit needs milestone".into());
    };
    let forge = match forge_of(project).await {
        Ok(forge) => forge,
        Err(why) => return Outcome::Refused(format!("refused: {why}; nothing was written")),
    };
    let slug = forge.slug();
    let all = match read(&forge, "all").await {
        Ok(all) => all,
        Err(why) => return Outcome::Failed(why),
    };
    let milestone = match find(&all, &slug, named) {
        Ok(milestone) => milestone,
        Err(why) => return Outcome::Refused(format!("refused: {why}; nothing was written")),
    };
    let title = asked
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    if let Some(title) = title
        && let Err(why) = free(&all, &slug, title, Some(milestone.number))
    {
        return Outcome::Refused(format!("refused: {why}; nothing was written"));
    }
    let state = asked
        .state
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let fields: Vec<String> = [
        ("title", title.map(str::to_owned)),
        (
            "description",
            asked.description.as_deref().map(str::to_owned),
        ),
        ("due_on", asked.due.as_deref().map(due_on)),
        ("state", state.map(str::to_owned)),
    ]
    .into_iter()
    .filter_map(|(key, value)| Some(format!("{key}={}", value?)))
    .collect();
    let path = format!("repos/{slug}/milestones/{}", milestone.number);
    let mut args = vec![
        "api",
        &path,
        "--hostname",
        declared::GITHUB,
        "--method",
        "PATCH",
    ];
    for field in &fields {
        args.extend(["-f", field]);
    }
    match forge::gh_json(&args).await {
        Ok(_) => Outcome::Done(format!(
            "The milestone {} of {slug} was {}.",
            milestone.named(),
            match state {
                Some("closed") => "closed",
                Some("open") => "reopened",
                _ => "changed",
            }
        )),
        Err(why) => Outcome::Failed(why),
    }
}

/// The milestone a `create` or an `edit` puts a ticket in, as the tracker names it: its title, for
/// `gh issue`. None when the call names none; refused when there is none such.
pub(super) async fn chosen(forge: &Forge, named: Option<&str>) -> Result<Option<String>, Outcome> {
    match named {
        None => Ok(None),
        Some(named) => resolved(forge, named)
            .await
            .map(Some)
            .map_err(|why| Outcome::Refused(format!("refused: {why}; nothing was written"))),
    }
}

/// What the card of a write says of the milestone it names, found before the user is asked: the
/// one a ticket goes in, the one that is changed, the title a new one takes. Refused when the
/// milestone is none or the title is used. Nothing for another call.
pub async fn says(project: &Project, args: &Value) -> Result<Vec<String>, String> {
    if matches!(project.tracker, Some(ProjectTracker::Jira { .. })) {
        return versions::says(project, args).await;
    }
    let action = args["action"].as_str().unwrap_or_default();
    let milestone = given(args, "milestone");
    let mut says = Vec::new();
    if let Some(due) = given(args, "due") {
        says.push(format!("Its due date: {due}"));
    }
    match (action, milestone) {
        ("edit", None) if args["remove_milestone"] == true => {
            says.push("It takes it out of its milestone".into());
            return Ok(says);
        }
        ("create" | "edit", Some(_)) | ("milestone_create" | "milestone_edit", _) => {}
        _ => return Ok(Vec::new()),
    }
    let forge = forge_of(project)
        .await
        .map_err(|why| format!("refused: {why}"))?;
    let slug = forge.slug();
    let all = read(&forge, "all")
        .await
        .map_err(|why| format!("refused: {why}"))?;
    let refused = |why: String| format!("refused: {why}; nothing was written");
    match (action, milestone) {
        ("milestone_create", _) => {
            let title = given(args, "title").unwrap_or_default();
            free(&all, &slug, title, None).map_err(refused)?;
        }
        ("milestone_edit", Some(named)) => {
            let target = find(&all, &slug, named).map_err(refused)?;
            if let Some(title) = given(args, "title") {
                free(&all, &slug, title, Some(target.number)).map_err(refused)?;
            }
            says.push(format!("It changes the milestone {}", target.named()));
            match given(args, "state") {
                Some("closed") => says.push("It closes it".into()),
                Some("open") => says.push("It reopens it".into()),
                _ => {}
            }
        }
        (_, Some(named)) => {
            let target = find(&all, &slug, named).map_err(refused)?;
            says.push(format!(
                "It puts it in the milestone {:?}{}",
                masked(&target.title),
                target
                    .due
                    .as_ref()
                    .map_or(String::new(), |due| format!(", due {due}"))
            ));
        }
        _ => {}
    }
    Ok(says)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_is_one_the_calendar_has() {
        for day in ["2026-12-31", "2028-02-29", "2000-02-29"] {
            assert!(is_day(day), "{day}");
        }
        for day in [
            "2026-02-29",
            "1900-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-04-31",
            "26-04-01",
            "2026-4-1",
            "2026-12-31T00:00:00Z",
            "next week",
            "",
        ] {
            assert!(!is_day(day), "{day}");
        }
    }
}
