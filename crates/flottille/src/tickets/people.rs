//! The people an agent can name on a tracker (#795): `tickets people` lists them, and an assignee
//! an agent gives as a display name is found among them before anything is sent (`docs/tools.md`,
//! "People"). GitHub's are the repository's assignable users and collaborators, as the forge's
//! (`forge::people`); Jira's are the people `acli` shows: it has no command that looks a user up,
//! so they are the assignees of the project's newest work items, and a person nobody has been
//! assigned a work item of is not found, which is said.

use serde_json::Value;

use super::{TicketsCall, acli, issues_of, jira_site, said, searched};
use crate::daemon::Daemon;
use crate::forge;
use crate::forge::Gh;
use crate::forge::masked;
use crate::mcp::Outcome;
use crate::people::{People, Person};
use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;

/// How many work items one read of Jira's people goes through.
const SEEN: usize = 100;

/// How many people one search of the site is asked for (Jira's own default, sent so that a full
/// page is known for what it is).
const PAGE: usize = 50;

/// Who a tracker lets an agent name, where they were looked for, and why they could not be read.
struct Listing {
    people: People,
    looked: String,
    /// Whether the tracker's own list was read: a GitHub repository's users. Jira has none.
    known: bool,
    /// Whether the people are all the tracker lists: only then is a part of a name trusted.
    complete: bool,
    why: Option<String>,
}

async fn read(project: &Project) -> Result<Listing, String> {
    match &project.tracker {
        Some(ProjectTracker::Github { .. }) => {
            let forge = issues_of(project).await.map_err(refusal)?;
            let listing = forge::people::read_for(&forge, "issue").await;
            let complete = listing.complete();
            Ok(Listing {
                people: listing.people,
                looked: listing.looked,
                known: listing.known,
                complete,
                why: Some(listing.why.join("; ")).filter(|why| !why.is_empty()),
            })
        }
        Some(ProjectTracker::Jira { site, projects, .. }) => {
            let mut people = People::default();
            let why = match jira_people(site, projects).await {
                Ok(found) => {
                    for person in found {
                        people.add(person);
                    }
                    None
                }
                Err(why) => Some(why),
            };
            Ok(Listing {
                people,
                looked: format!(
                    "{}: the assignees of its newest work items, acli having no command that \
                     looks a user up",
                    projects.join(", ")
                ),
                // What acli shows is the people it shows: a person never assigned a work item is
                // not one of them, and not being found is not being absent.
                known: false,
                complete: false,
                why,
            })
        }
        None => Err(format!("refused: {}", super::NO_TRACKER)),
    }
}

/// The sentence of an outcome that did not read.
fn refusal(outcome: Outcome) -> String {
    match outcome {
        Outcome::Refused(why)
        | Outcome::Failed(why)
        | Outcome::Invalid(why)
        | Outcome::Done(why) => why,
        Outcome::Held(_) => String::new(),
    }
}

/// The assignees of the project's newest work items that are assigned: who they are, as the site's
/// JSON gives them (`accountId`, `emailAddress`, `displayName`).
async fn jira_people(site: &str, projects: &[String]) -> Result<Vec<Person>, String> {
    jira_site(site, "read").await?;
    let quoted: Vec<String> = projects
        .iter()
        .map(|p| format!("\"{}\"", p.replace('"', "")))
        .collect();
    let query = format!(
        "project in ({}) AND assignee is not EMPTY ORDER BY updated DESC",
        quoted.join(", ")
    );
    let limit = SEEN.to_string();
    let (ok, stdout, stderr) = match acli(&[
        "jira",
        "workitem",
        "search",
        "--jql",
        &query,
        "--fields",
        "key,assignee",
        "--json",
        "--limit",
        &limit,
    ])
    .await
    {
        Ok(output) => output,
        Err(Gh::Missing) => return Err("Flottille cannot find acli".into()),
        Err(Gh::Failed(why)) => return Err(format!("acli jira workitem search: {why}")),
    };
    if !ok {
        let said = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(format!(
            "acli jira workitem search failed: {}",
            masked(&said.chars().take(300).collect::<String>())
        ));
    }
    let found: Value = serde_json::from_str(&stdout)
        .map_err(|error| format!("acli jira workitem search answered what is not JSON: {error}"))?;
    let (items, _) = searched(&found)?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let user = &item["fields"]["assignee"];
            let (account, mail) = (said(&user["accountId"]), said(&user["emailAddress"]));
            // What `acli` takes for an assignee is an email or an account id: a person with
            // neither cannot be named.
            let send = [&account, &mail].into_iter().find(|id| !id.is_empty())?;
            let handle = [&mail, &account].into_iter().find(|id| !id.is_empty())?;
            Some(Person::new(
                &said(&user["displayName"]),
                handle,
                send,
                &[&account, &mail],
            ))
        })
        .collect())
}

/// A Jira person as `people` says them: the e-mail or account id `assignee` takes, and always
/// their account id, which a mention is written with.
fn with_account(person: &Person) -> String {
    let said = person.said();
    // An e-mail holds an `@`, an account id never does.
    let account = std::iter::once(&person.send)
        .chain(&person.keys)
        .find(|id| !id.is_empty() && !id.contains('@'));
    match account {
        Some(account) if !said.contains(account.as_str()) => {
            format!("{said}, account id {account}")
        }
        _ => said,
    }
}

/// The people of the site matching `query`, as Jira's own picker offers them (`GET
/// /rest/api/3/user/search`): with a `ticket`, only those who can see it (`GET
/// /rest/api/3/user/viewissue/search`), so a mention reaches someone who can read it. Through the
/// project's tracker entry, which is the one credential here and goes to the site alone. A person
/// the site gives no account id is not one to mention, and a deactivated one is not notified. Whether
/// the page came back full is said too: there may be more people than it holds.
async fn site_people(
    daemon: &Daemon,
    project: &Project,
    site: &str,
    query: &str,
    ticket: Option<&str>,
) -> Result<(Vec<Person>, bool), String> {
    let key = match ticket {
        Some(ticket) => Some(
            super::accepted(project, ticket).map_err(|why| format!("refused: ticket: {why}"))?,
        ),
        None => None,
    };
    let none = super::worklog::no_entry_to("search the site's people with", "searches the site");
    let (jira, _, _) = super::worklogs::signed_in(daemon, project, site, none).await?;
    let page = PAGE.to_string();
    let found = match &key {
        Some(key) => {
            let key = key.text();
            jira.get(
                "user/viewissue/search",
                &[("issueKey", &key), ("query", query), ("maxResults", &page)],
            )
            .await?
        }
        None => {
            jira.get("user/search", &[("query", query), ("maxResults", &page)])
                .await?
        }
    };
    let found = found.as_array().map(Vec::as_slice).unwrap_or_default();
    let people = found
        .iter()
        .filter(|user| user["active"] != false)
        .filter_map(|user| {
            let account = said(&user["accountId"]);
            let mail = jira.clean(&said(&user["emailAddress"]));
            let name = jira.clean(&said(&user["displayName"]));
            if account.is_empty() {
                return None;
            }
            let handle = if mail.is_empty() { &account } else { &mail };
            Some(Person::new(&name, handle, &account, &[&account, &mail]))
        })
        .collect();
    Ok((people, found.len() >= PAGE))
}

/// `tickets people {query?, ticket?}`: who can be named on the project's tracker: as an assignee,
/// and on Jira in a mention. A Jira query is searched on the site too, with the tracker's entry.
pub(super) async fn people(daemon: &Daemon, project: &Project, asked: &TicketsCall) -> Outcome {
    let listing = match read(project).await {
        Ok(listing) => listing,
        Err(why) => return Outcome::Refused(why),
    };
    let query = asked
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let jira = matches!(project.tracker, Some(ProjectTracker::Jira { .. }));
    // On Jira a person found on the site is named by account id: `assignee` finds a display name
    // among the assignees `acli` shows, which the site's people may not be.
    let mut text = String::from(match jira {
        true => "The people who can be named, as an assignee or in a mention",
        false => "The people who can be named as an assignee",
    });
    if let Some(query) = query {
        text.push_str(&format!(", matching {query:?}"));
    }
    text.push_str(":\n");
    let local: Vec<&Person> = match query {
        Some(query) => listing.people.matching(query),
        None => listing.people.all().iter().collect(),
    };
    let mut people = People::default();
    for person in local {
        people.add(person.clone());
    }
    let mut looked = listing.looked.clone();
    let mut note = String::new();
    // Why the assignees could not be read matters only while they are part of the answer.
    let mut read_why = listing.why.clone();
    if let (Some(ProjectTracker::Jira { site, .. }), Some(query)) = (&project.tracker, query) {
        let ticket = asked
            .ticket
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        match site_people(daemon, project, site, query, ticket).await {
            Ok((found, full)) => {
                if let Some(ticket) = ticket {
                    // Who can see the ticket is the site's answer alone: an assignee matching the
                    // query who is not in it is not listed under that header.
                    people = People::default();
                    read_why = None;
                    looked = format!("the site's people who can see {ticket}");
                } else {
                    looked.push_str(" and the site's people");
                }
                for person in found {
                    people.add(person);
                }
                if full {
                    note = format!(
                        "\nThe site's answer is full ({PAGE} people): there may be more on the \
                         site, narrow with query."
                    );
                }
            }
            Err(why) => {
                let why = why.strip_prefix("refused: ").unwrap_or(&why);
                note = format!("\nOnly the assignees were searched, not the site: {why}.");
            }
        }
    }
    if people.is_empty() {
        return Outcome::Done(match read_why {
            Some(why) => format!("{text}\nnone: they cannot be listed: {why}.{note}"),
            None => format!("{text}\nnone in {looked}.{note}"),
        });
    }
    let found: Vec<&Person> = people.all().iter().collect();
    text.push_str(&format!("\n{} in {looked}:\n", found.len()));
    text.push_str(&match jira {
        true => crate::people::lines_saying(&found, with_account),
        false => crate::people::lines(&found),
    });
    if let Some(why) = read_why {
        text.push_str(&format!("Some of them could not be read: {why}.\n"));
    }
    if jira {
        text.push_str(
            "\nOn Jira an assignee is named by e-mail or account id: tickets {\"action\": \
             \"edit\", \"ticket\": …, \"assignee\": \"<account id>\"}; a display name does for \
             the assignees of the newest work items, not for people found only on the site. A \
             mention in a text is [@Display Name](mention:<accountId>), with the account id.",
        );
    } else {
        text.push_str(
            "\nAn assignee is named by display name, e-mail or account id: tickets {\"action\": \
             \"edit\", \"ticket\": …, \"assignee\": \"Jane Doe\"}.",
        );
    }
    text.push_str(&note);
    Outcome::Done(text)
}

/// The Jira account `who` names for `tickets worklogs`: found among the tracker's people, and
/// how an answer says them. Jira's REST API takes the account id, which `acli` shows for a person.
pub(super) async fn account_of(project: &Project, who: &str) -> Result<(String, String), String> {
    let listing = read(project).await?;
    if listing.people.is_empty() && listing.why.is_some() {
        return Err(format!(
            "refused: author: {who:?} is a name, and the tracker's people cannot be looked up \
             ({}); give an account id (tickets people shows them)",
            listing.why.as_deref().unwrap_or_default()
        ));
    }
    let person = listing
        .people
        .find(who, listing.complete)
        .map_err(|miss| miss.refusal("author", who, &listing.looked))?;
    let account = std::iter::once(&person.send)
        .chain(&person.keys)
        .find(|id| super::worklogs::is_account(id))
        .ok_or_else(|| {
            format!(
                "refused: author: {} has no account id among what acli shows of them: give it",
                person.said()
            )
        })?;
    Ok((account.clone(), person.said()))
}

/// The person `who` names for `key`, as what the tracker's command takes and how an answer says
/// them: found among its people, or refused saying why. A GitHub repository whose users cannot
/// be read takes a login as it was given, as before.
fn pick(listing: &Listing, who: &str, key: &str, jira: bool) -> Result<(String, String), String> {
    let cannot = || {
        format!(
            "refused: {key}: {who:?} is a name, which {} takes for nobody, and the tracker's \
             people cannot be looked up ({}); give an {}, or me; nothing was written",
            if jira { "acli" } else { "gh" },
            listing.why.as_deref().unwrap_or("none were listed"),
            if jira { "email or account id" } else { "login" },
        )
    };
    if !jira && !listing.known {
        return match who.contains(char::is_whitespace) {
            true => Err(cannot()),
            false => Ok((who.to_owned(), String::new())),
        };
    }
    if jira && listing.people.is_empty() && listing.why.is_some() {
        return Err(cannot());
    }
    match listing.people.find(who, listing.complete) {
        Ok(person) => Ok((person.send.clone(), person.said())),
        Err(miss) => Err(format!(
            "{}; nothing was written",
            miss.refusal(key, who, &listing.looked)
        )),
    }
}

/// The assignees a write names, found among the tracker's people before anything is sent: a Jira
/// `assignee` that is no `me`, e-mail or account id, and the GitHub `assignees` and
/// `remove_assignees`. What is said of them, for the answer.
pub(super) async fn resolve(project: &Project, asked: &mut TicketsCall) -> Result<String, String> {
    let me = |who: &str| ["me", "@me"].contains(&who);
    let jira = matches!(project.tracker, Some(ProjectTracker::Jira { .. }));
    let wanted = match (jira, asked.assigned_to()) {
        (true, Some(who)) => !super::assignable(who),
        (true, None) => false,
        (false, _) => [&asked.assignees, &asked.remove_assignees]
            .into_iter()
            .flat_map(super::named)
            .any(|who| !me(&who)),
    };
    if !wanted || project.tracker.is_none() {
        return Ok(String::new());
    }
    let listing = read(project).await?;
    if jira {
        let who = asked.assigned_to().unwrap_or_default().to_owned();
        let (sent, said) = pick(&listing, &who, "assignee", true)?;
        asked.assignee = Some(Some(sent));
        return Ok(format!(" Assigned to {said}."));
    }
    let mut told = Vec::new();
    for (key, verb, list) in [
        ("assignees", "added", &mut asked.assignees),
        ("remove_assignees", "removed", &mut asked.remove_assignees),
    ] {
        let mut said = Vec::new();
        for name in list.iter_mut().flatten() {
            let who = name.trim().to_owned();
            if who.is_empty() || me(&who) {
                continue;
            }
            let (sent, shown) = pick(&listing, &who, key, false)?;
            *name = sent;
            if !shown.is_empty() {
                said.push(shown);
            }
        }
        if !said.is_empty() {
            told.push(format!("{verb} {}", said.join(", ")));
        }
    }
    Ok(match told.is_empty() {
        true => String::new(),
        false => format!(" Assignees: {}.", told.join("; ")),
    })
}
