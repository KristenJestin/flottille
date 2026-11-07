//! The links between tickets, and the comments an agent posted, edited or deleted by the agent
//! itself (#365, `docs/tickets.md`, "Links and comments").
//!
//! A link is said as the ticket named reads it: `ticket` `kind` `other`, so `ABC-2 blocks ABC-1`
//! and `ABC-1 is blocked by ABC-2` are the same link, and the card the user is shown says it in
//! those words. On Jira the kinds are the site's own link types (`acli jira workitem link type`),
//! the outward wording putting `ticket` on the outward end, the inward one on the other.

use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use flottille_core::ticket::Key;
use serde_json::Value;

use super::{TicketsCall, acli_run, and_listed, jira_site};
use crate::daemon::Daemon;
use crate::forge::{self, masked};
use crate::mcp::Outcome;

/// What a link may name on GitHub: sub-issues and dependencies, the two relations its API has.
const GITHUB_KINDS: [&str; 5] = [
    "parent of",
    "sub-task of",
    "sub-issue of",
    "blocks",
    "blocked by",
];

/// What a Jira link kind says of a parent: acli puts a parent at creation (`create --parent`)
/// and its `edit` has no flag for it.
const PARENT_WORDS: [&str; 6] = [
    "parent of",
    "parent",
    "sub-task of",
    "subtask of",
    "sub-issue of",
    "child of",
];

fn blank(text: &Option<String>) -> Option<&str> {
    text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Why a write of this file cannot be done, before anyone is asked: the words it lacks, a link
/// kind the tracker has none of.
pub(super) fn unwritable(project: &Project, action: &str, args: &Value) -> Option<String> {
    let text = |key: &str| {
        args[key]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    let needs = |what: &str| format!("refused: tickets {action} needs {what}");
    match action {
        "link" | "unlink" => {
            let (Some(ticket), Some(kind), Some(other)) =
                (text("ticket"), text("kind"), text("other"))
            else {
                return Some(needs(
                    "ticket, kind and other: the link reads `ticket kind other`, as in \
                     ticket ABC-2, kind blocks, other ABC-1 (ABC-2 blocks ABC-1); link_types \
                     lists the kinds",
                ));
            };
            if let Err(why) = super::accepted(project, other) {
                return Some(format!("refused: {why}"));
            }
            if super::accepted(project, ticket) == super::accepted(project, other) {
                return Some(format!(
                    "refused: tickets {action} would link {ticket} to itself"
                ));
            }
            match project.tracker.as_ref()? {
                ProjectTracker::Github { .. } => github_kind(kind).err(),
                ProjectTracker::Jira { .. } if PARENT_WORDS.iter().any(|word| same(word, kind)) => {
                    Some(format!(
                        "refused: \"{}\" is a parent, which acli cannot set on a work item that \
                         exists: `workitem edit` has no flag for it (only `workitem create \
                         --parent` does); the links of the site are {}: link_types lists them; \
                         nothing was written",
                        masked(kind),
                        "blocks, relates to and its other link types"
                    ))
                }
                ProjectTracker::Jira { .. } => None,
            }
        }
        "comment_edit" | "comment_delete" => {
            if text("ticket").is_none() {
                return Some(needs("ticket, the ticket the comment is on"));
            }
            if text("comment").is_none() {
                return Some(needs(
                    "comment, the id `show` with detail full gives a comment an agent posted",
                ));
            }
            (action == "comment_edit" && text("body").is_none())
                .then(|| needs("body, the new text of the comment"))
        }
        _ => None,
    }
}

/// What a GitHub kind means, as the endpoint and the end it puts `ticket` on.
#[derive(Clone, Copy, PartialEq)]
enum Relation {
    /// `ticket` is the parent of `other`.
    Parent,
    /// `ticket` is a sub-issue of `other`.
    Child,
    /// `ticket` blocks `other`.
    Blocks,
    /// `ticket` is blocked by `other`.
    BlockedBy,
}

fn github_kind(kind: &str) -> Result<Relation, String> {
    let wanted = kind
        .trim()
        .to_ascii_lowercase()
        .replace("subtask", "sub-task");
    Ok(match wanted.as_str() {
        "parent of" => Relation::Parent,
        "sub-task of" | "sub-issue of" => Relation::Child,
        "blocks" => Relation::Blocks,
        "blocked by" => Relation::BlockedBy,
        _ => {
            return Err(format!(
                "refused: \"{}\" is not a link of GitHub issues: its API has sub-issues and \
                 dependencies only, {}; a pull request closes an issue with `Closes #n` in its \
                 description (forge edit); nothing was written",
                masked(kind),
                and_listed(&GITHUB_KINDS)
            ));
        }
    })
}

impl Relation {
    /// `#2 is a sub-task of #1`, `#1 is the parent of #2`, `#1 blocks #2`, `#1 is blocked by #2`.
    fn said(self, ticket: &str, other: &str) -> String {
        match self {
            Relation::Parent => format!("{ticket} is the parent of {other}"),
            Relation::Child => format!("{ticket} is a sub-task of {other}"),
            Relation::Blocks => format!("{ticket} blocks {other}"),
            Relation::BlockedBy => format!("{ticket} is blocked by {other}"),
        }
    }
}

/// Jira's own wordings of its standard link types: `link type --json` names a site's types only
/// (seen on a real site), and a type the site renamed or added is known by its name alone.
const JIRA_WORDINGS: [(&str, &str, &str); 4] = [
    ("Blocks", "is blocked by", "blocks"),
    ("Cloners", "is cloned by", "clones"),
    ("Duplicate", "is duplicated by", "duplicates"),
    ("Relates", "relates to", "relates to"),
];

/// A link type of a Jira site: its name and its two wordings (its name for both when unknown).
#[derive(Clone)]
struct LinkType {
    name: String,
    inward: String,
    outward: String,
}

/// Which end of the link `ticket` is on.
#[derive(Clone, Copy, PartialEq)]
enum End {
    Outward,
    Inward,
}

impl LinkType {
    /// The wording as `ticket` reads it, at its end.
    fn wording(&self, end: End) -> &str {
        match end {
            End::Outward => &self.outward,
            End::Inward => &self.inward,
        }
    }
}

/// `acli jira workitem link type --json`: `{"issueLinkTypes": [{"name"}]}`, as a real site answers
/// (an array is taken too). The wordings, which it does not give, are Jira's own for its standard
/// types ([`JIRA_WORDINGS`]), or those it gives where it does.
async fn link_types_of(site: &str) -> Result<Vec<LinkType>, String> {
    jira_site(site, "read").await?;
    let said = acli_run(&["jira", "workitem", "link", "type", "--json"]).await?;
    let parsed: Value = serde_json::from_str(&said).map_err(|error| {
        format!("acli jira workitem link type answered what is not JSON: {error}")
    })?;
    let list = parsed
        .as_array()
        .or(parsed["issueLinkTypes"].as_array())
        .or(parsed["linkTypes"].as_array())
        .ok_or("acli jira workitem link type answered no list of link types")?;
    Ok(list
        .iter()
        .filter_map(|kind| {
            let word = |key: &str| kind[key].as_str().map(masked);
            let name = word("name")?;
            let standard = JIRA_WORDINGS.iter().find(|(known, ..)| same(known, &name));
            let (inward, outward) = match (word("inward"), word("outward"), standard) {
                (Some(inward), Some(outward), _) => (inward, outward),
                (_, _, Some((_, inward, outward))) => ((*inward).into(), (*outward).into()),
                _ => (name.clone(), name.clone()),
            };
            Some(LinkType {
                name,
                inward,
                outward,
            })
        })
        .collect())
}

fn known_kinds(types: &[LinkType]) -> String {
    let mut words: Vec<&str> = Vec::new();
    for kind in types {
        for word in [kind.outward.as_str(), kind.inward.as_str()] {
            if !words.contains(&word) {
                words.push(word);
            }
        }
    }
    and_listed(&words)
}

/// The type a kind names, and the end `ticket` is on: the outward wording (or the type's name)
/// puts it on the outward end, the inward one on the inward end.
fn resolved<'a>(types: &'a [LinkType], kind: &str) -> Option<(&'a LinkType, End)> {
    types
        .iter()
        .find(|t| same(&t.outward, kind))
        .map(|t| (t, End::Outward))
        .or_else(|| {
            types
                .iter()
                .find(|t| same(&t.inward, kind))
                .map(|t| (t, End::Inward))
        })
        .or_else(|| {
            types
                .iter()
                .find(|t| same(&t.name, kind))
                .map(|t| (t, End::Outward))
        })
}

fn unknown_kind(site: &str, kind: &str, types: &[LinkType]) -> String {
    format!(
        "refused: \"{}\" is not a link type of {site}: its kinds are {}; nothing was written; \
         link_types lists them with the wording of each end",
        masked(kind),
        known_kinds(types)
    )
}

/// Before the user is asked: a Jira link kind the site has none of is refused, naming the ones
/// it has; a comment an agent did not post, or that is not there, is refused too.
pub async fn checked(daemon: &Daemon, project: &Project, args: &Value) -> Option<String> {
    if let Some("comment_edit" | "comment_delete") = args["action"].as_str() {
        let key = super::accepted(project, args["ticket"].as_str()?).ok()?;
        return match owned(daemon, project, &key, args["comment"].as_str()?).await {
            Err(Outcome::Refused(why)) => Some(why),
            // What cannot be read now is said when the comment is written.
            _ => None,
        };
    }
    if !matches!(args["action"].as_str(), Some("link" | "unlink")) {
        return None;
    }
    let Some(ProjectTracker::Jira { site, .. }) = &project.tracker else {
        return None;
    };
    let kind = args["kind"].as_str()?;
    // What cannot be read now is said when the link is made.
    let types = link_types_of(site).await.ok()?;
    resolved(&types, kind)
        .is_none()
        .then(|| unknown_kind(site, kind, &types))
}

/// `tickets link_types`: the kinds a link may have on the tracker.
pub(super) async fn types(project: &Project) -> Outcome {
    match &project.tracker {
        None => Outcome::Refused(format!("refused: {}", super::NO_TRACKER)),
        Some(ProjectTracker::Github { .. }) => Outcome::Done(
            "GitHub issues have two kinds of link, both through its API: sub-issues (parent of, \
             sub-task of) and dependencies (blocks, blocked by). Read them as `ticket kind \
             other`: ticket #2, kind sub-task of, other #1 means #2 is a sub-task of #1. Nothing \
             else links two issues (a pull request closes one with `Closes #n` in its \
             description)."
                .into(),
        ),
        Some(ProjectTracker::Jira { site, projects, .. }) => match link_types_of(site).await {
            Err(why) => Outcome::Failed(why),
            Ok(types) => {
                let first = projects.first().map_or("ABC", String::as_str);
                let lines: Vec<String> = types
                    .iter()
                    .map(|kind| {
                        let wordings = match same(&kind.outward, &kind.inward) {
                            true => kind.outward.clone(),
                            false => format!("{} ({})", kind.outward, kind.inward),
                        };
                        match wordings == kind.name {
                            true => format!("- {wordings}"),
                            false => format!("- {wordings} [{}]", kind.name),
                        }
                    })
                    .collect();
                Outcome::Done(format!(
                    "The link types of {site}, each as its outward wording (the inward one, \
                     which the other end reads):\n{}\nRead a link as `ticket kind other`: ticket \
                     {first}-2, kind blocks, other {first}-1 means {first}-2 blocks {first}-1; \
                     with kind is blocked by, {first}-2 is blocked by {first}-1.",
                    lines.join("\n")
                ))
            }
        },
    }
}

/// One link of a Jira work item `K` as `view`'s `issuelinks` says it.
struct Seen {
    id: String,
    /// Its type's name, and the end `K` is on.
    kind: String,
    end: End,
    /// What `K` is to the other: `blocks`, `is blocked by`.
    wording: String,
    /// The other work item's key, its title and its status.
    other: String,
    title: String,
    status: String,
}

impl Seen {
    fn said(&self) -> String {
        format!(
            "{} {} ({}): {}",
            self.wording, self.other, self.status, self.title
        )
    }
}

/// The links of `key`, from `acli jira workitem view K --json --fields issuelinks`: `link list`
/// names no other item when `K` is the outward one (seen on a real site). Each is Jira's REST
/// `{id, type: {name, inward, outward}, inwardIssue, outwardIssue}`, one of the two null, read
/// with the wording of the field that holds the other item, as Jira's page and its JQL
/// `linkedIssues` say it on a real site (#520): with an `outwardIssue`, `K blocks it`; with an
/// `inwardIssue`, `K is blocked by it`.
async fn seen_on(site: &str, key: &str) -> Result<Vec<Seen>, String> {
    let item = super::viewed(key, site, "issuelinks").await?;
    Ok(seen_in(&item))
}

/// The links of a work item `view` answered with `issuelinks` among its fields.
fn seen_in(item: &Value) -> Vec<Seen> {
    let text = |value: &Value| value.as_str().map(masked).unwrap_or_default();
    item["fields"]["issuelinks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|link| {
            let id = link["id"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| link["id"].as_u64().map(|n| n.to_string()))?;
            let kind = &link["type"];
            let (issue, end) = match link["outwardIssue"].is_object() {
                true => (&link["outwardIssue"], End::Outward),
                false if link["inwardIssue"].is_object() => (&link["inwardIssue"], End::Inward),
                false => return None,
            };
            let name = text(&kind["name"]);
            let wording = match end {
                End::Outward => text(&kind["outward"]),
                End::Inward => text(&kind["inward"]),
            };
            Some(Seen {
                id,
                wording: if wording.is_empty() {
                    name.clone()
                } else {
                    wording
                },
                kind: name,
                end,
                other: text(&issue["key"]),
                title: text(&issue["fields"]["summary"]),
                status: text(&issue["fields"]["status"]["name"]),
            })
        })
        .collect()
}

/// The links of a work item read together with the ticket (`ACLI_FIELDS` asks for them), one line
/// each, kept with the ticket in the cache; none for an item with none.
pub(super) fn said_in(item: &Value) -> Vec<String> {
    seen_in(item).iter().map(Seen::said).collect()
}

/// `tickets link` and `unlink`.
pub(super) async fn link(project: &Project, asked: &TicketsCall, key: &Key) -> Outcome {
    let (Some(kind), Some(other)) = (blank(&asked.link_kind), blank(&asked.other)) else {
        return Outcome::Refused("refused: tickets link needs kind and other".into());
    };
    let other = match super::accepted(project, other) {
        Ok(other) => other,
        Err(why) => return Outcome::Refused(format!("refused: {why}")),
    };
    let unlink = asked.action == "unlink";
    match (&project.tracker, key, &other) {
        (Some(ProjectTracker::Jira { site, .. }), Key::Jira(ticket), Key::Jira(other)) => {
            jira_link(site, (ticket, kind, other), unlink).await
        }
        (Some(ProjectTracker::Github { .. }), Key::Issue(ticket), Key::Issue(other)) => {
            github_link(project, (*ticket, kind, *other), unlink).await
        }
        _ => Outcome::Refused("refused: the two tickets are not of the project's tracker".into()),
    }
}

async fn jira_link(site: &str, (ticket, kind, other): (&str, &str, &str), unlink: bool) -> Outcome {
    if let Err(problem) = jira_site(site, "written").await {
        return Outcome::Failed(problem);
    }
    let types = match link_types_of(site).await {
        Ok(types) => types,
        Err(why) => return Outcome::Failed(why),
    };
    let Some((found, end)) = resolved(&types, kind) else {
        return Outcome::Refused(unknown_kind(site, kind, &types));
    };
    let wording = found.wording(end);
    let sentence = format!("{ticket} {wording} {other}");
    // Said from the other end too: the link is one, and the user reads it from either.
    let back = format!(
        "{other} {} {ticket}",
        found.wording(match end {
            End::Outward => End::Inward,
            End::Inward => End::Outward,
        })
    );
    if unlink {
        let links = match seen_on(site, ticket).await {
            Ok(links) => links,
            Err(why) => return Outcome::Failed(why),
        };
        let Some(there) = links.iter().find(|link| {
            same(&link.other, other) && same(&link.kind, &found.name) && link.end == end
        }) else {
            let held: Vec<String> = links.iter().map(Seen::said).collect();
            return Outcome::Refused(format!(
                "refused: {ticket} has no link \"{wording} {other}\"; its links are {}; nothing \
                 was written",
                match held.is_empty() {
                    true => "none".to_owned(),
                    false => held.join("; "),
                }
            ));
        };
        return match acli_run(&[
            "jira", "workitem", "link", "delete", "--id", &there.id, "--yes",
        ])
        .await
        {
            Ok(_) => Outcome::Done(format!("The link was removed: {sentence} ({back}).")),
            Err(why) => Outcome::Failed(why),
        };
    }
    // acli's `--out` is the item the outward wording points at: on a real site `--out A --in B
    // --type Blocks` made `B blocks A` (#520), though acli printed `A Blocks B`.
    let (out, inward) = match end {
        End::Outward => (other, ticket),
        End::Inward => (ticket, other),
    };
    // `--type` takes the type's name, as a real site was seen to.
    match acli_run(&[
        "jira",
        "workitem",
        "link",
        "create",
        "--out",
        out,
        "--in",
        inward,
        "--type",
        &found.name,
        "--yes",
    ])
    .await
    {
        Ok(_) => Outcome::Done(format!("The link was made: {sentence} ({back}).")),
        Err(why) => Outcome::Failed(why),
    }
}

async fn github_link(
    project: &Project,
    (ticket, kind, other): (u64, &str, u64),
    unlink: bool,
) -> Outcome {
    let relation = match github_kind(kind) {
        Ok(relation) => relation,
        Err(refusal) => return Outcome::Refused(refusal),
    };
    let forge = match super::issues_of(project).await {
        Ok(forge) => forge,
        Err(refusal) => return refusal,
    };
    if let Err(problem) = forge::signed_in().await {
        return Outcome::Failed(problem);
    }
    let slug = forge.slug();
    // The one whose id the endpoint takes, and the one it is called on.
    let (on, whose) = match relation {
        Relation::Parent | Relation::BlockedBy => (ticket, other),
        Relation::Child | Relation::Blocks => (other, ticket),
    };
    let id = match issue_id(&slug, whose).await {
        Ok(id) => id.to_string(),
        Err(why) => return Outcome::Failed(why),
    };
    let sentence = relation.said(&format!("#{ticket}"), &format!("#{other}"));
    let base = format!("repos/{slug}/issues/{on}");
    let result = match (relation, unlink) {
        (Relation::Parent | Relation::Child, false) => {
            gh_api(
                &format!("{base}/sub_issues"),
                "POST",
                &format!("sub_issue_id={id}"),
            )
            .await
        }
        (Relation::Parent | Relation::Child, true) => {
            gh_api(
                &format!("{base}/sub_issue"),
                "DELETE",
                &format!("sub_issue_id={id}"),
            )
            .await
        }
        (_, false) => {
            gh_api(
                &format!("{base}/dependencies/blocked_by"),
                "POST",
                &format!("issue_id={id}"),
            )
            .await
        }
        (_, true) => gh_api_bare(&format!("{base}/dependencies/blocked_by/{id}"), "DELETE").await,
    };
    match result {
        Ok(_) if unlink => Outcome::Done(format!("The link was removed in {slug}: {sentence}.")),
        Ok(_) => Outcome::Done(format!("The link was made in {slug}: {sentence}.")),
        Err(why) => Outcome::Failed(why),
    }
}

/// The id GitHub's REST gives an issue (not its number), which the link endpoints take.
async fn issue_id(slug: &str, number: u64) -> Result<u64, String> {
    let path = format!("repos/{slug}/issues/{number}");
    let issue = forge::gh_json(&["api", &path]).await?;
    issue["id"]
        .as_u64()
        .ok_or_else(|| format!("gh api {path} answered no id"))
}

async fn gh_api(path: &str, method: &str, field: &str) -> Result<String, String> {
    forge::gh_text(&["api", path, "--method", method, "-F", field]).await
}

async fn gh_api_bare(path: &str, method: &str) -> Result<String, String> {
    forge::gh_text(&["api", path, "--method", method]).await
}

/// The id of a comment as the agent names it: GitHub's `issuecomment-501` (or its number), or
/// Jira's own.
fn same_comment(have: Option<&str>, named: &str) -> bool {
    let Some(have) = have else {
        return false;
    };
    let named = named.trim();
    have == named
        || have
            .strip_prefix("issuecomment-")
            .is_some_and(|digits| digits == named)
}

/// The ticket, and the id of its comment `named`, when an agent of Flottille posted that comment;
/// else why not.
async fn owned(
    daemon: &Daemon,
    project: &Project,
    key: &Key,
    named: &str,
) -> Result<(super::Ticket, String), Outcome> {
    let ticket = super::read_known(daemon, project, key)
        .await
        .map_err(|problem| {
            Outcome::Failed(format!(
                "the ticket {} could not be read: {problem}",
                key.text()
            ))
        })?;
    let reference = &ticket.reference;
    let named = named.trim();
    let Some(found) = ticket
        .comments
        .iter()
        .find(|c| same_comment(c.id.as_deref(), named))
    else {
        return Err(Outcome::Refused(format!(
            "refused: {reference} has no comment {named} among the {} newest it can read; \
             nothing was written; show with detail full gives the id of a comment an agent \
             posted",
            ticket.comments.len()
        )));
    };
    if !found.own {
        return Err(Outcome::Refused(format!(
            "refused: the comment {named} of {reference} was not posted by an agent of \
             Flottille (no record of its id says it was): an agent changes only its own \
             comments; nothing was written"
        )));
    }
    let id = found.id.clone().unwrap_or_default();
    Ok((ticket, id))
}

/// `tickets comment_edit` and `comment_delete`: only a comment Flottille recorded as posted by an
/// agent (`posts`), else refused, whatever its author's name says.
pub(super) async fn comment(
    daemon: &Daemon,
    project: &Project,
    asked: &TicketsCall,
    key: &Key,
) -> Outcome {
    let Some(named) = blank(&asked.comment) else {
        return Outcome::Refused("refused: tickets comment_edit needs comment".into());
    };
    let delete = asked.action == "comment_delete";
    let body = blank(&asked.body);
    // Checked again right before the write: the comments may have changed since the card.
    let (ticket, id) = match owned(daemon, project, key, named).await {
        Ok(owned) => owned,
        Err(outcome) => return outcome,
    };
    let reference = &ticket.reference;
    let (repository, item) = super::events::item_of(project, key).unwrap_or_default();
    let on = (
        project.name.as_str(),
        ticket.tracker.as_str(),
        repository.as_str(),
        item.as_str(),
    );
    let done = |what: &str| format!("The comment {id} of {reference} was {what}.");
    match (&project.tracker, key) {
        (Some(ProjectTracker::Jira { site, .. }), Key::Jira(key)) => {
            if let Err(problem) = jira_site(site, "written").await {
                return Outcome::Failed(problem);
            }
            if delete {
                return match acli_run(&[
                    "jira", "workitem", "comment", "delete", "--key", key, "--id", &id,
                ])
                .await
                {
                    Ok(_) => {
                        crate::posts::drop_comment(daemon, on, &id);
                        Outcome::Done(done("deleted"))
                    }
                    Err(why) => Outcome::Failed(why),
                };
            }
            let body = body.expect("a body: the call was checked");
            let file = match super::AdfFile::of(daemon, body) {
                Ok(file) => file,
                Err(why) => return Outcome::Failed(why),
            };
            match acli_run(&[
                "jira",
                "workitem",
                "comment",
                "update",
                "--key",
                key,
                "--id",
                &id,
                // `update` takes a document with `--body-adf` only: its `--body-file` is plain text.
                "--body-adf",
                file.path(),
            ])
            .await
            {
                Ok(_) => {
                    crate::posts::replace_comment(daemon, on, &id, &crate::adf::round_trip(body));
                    Outcome::Done(done("changed"))
                }
                Err(why) => Outcome::Failed(why),
            }
        }
        (Some(ProjectTracker::Github { .. }), Key::Issue(_)) => {
            let forge = match super::issues_of(project).await {
                Ok(forge) => forge,
                Err(refusal) => return refusal,
            };
            if let Err(problem) = forge::signed_in().await {
                return Outcome::Failed(problem);
            }
            let number = id.trim_start_matches("issuecomment-");
            let path = format!("repos/{}/issues/comments/{number}", forge.slug());
            if delete {
                return match gh_api_bare(&path, "DELETE").await {
                    Ok(_) => {
                        crate::posts::drop_comment(daemon, on, &id);
                        Outcome::Done(done("deleted"))
                    }
                    Err(why) => Outcome::Failed(why),
                };
            }
            let body = body.expect("a body: the call was checked");
            match forge::gh_text(&[
                "api",
                &path,
                "--method",
                "PATCH",
                "-f",
                &format!("body={body}"),
            ])
            .await
            {
                Ok(_) => {
                    crate::posts::replace_comment(daemon, on, &id, body);
                    Outcome::Done(done("changed"))
                }
                Err(why) => Outcome::Failed(why),
            }
        }
        _ => Outcome::Refused("refused: the ticket is not of the project's tracker".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `view K --json --fields issuelinks` as a real site answered for K, holding one link of the
    /// type Blocks to `other` in the field named.
    fn viewed_with(field: &str, other: &str) -> Value {
        let mut link = json!({"id": "10001",
            "type": {"name": "Blocks", "inward": "is blocked by", "outward": "blocks"},
            "inwardIssue": null, "outwardIssue": null});
        link[field] = json!({"key": other,
            "fields": {"summary": "Add search", "status": {"name": "To Do"}}});
        json!({"key": "ABC-8", "fields": {"issuelinks": [link]}})
    }

    #[test]
    fn a_link_holding_an_outward_issue_reads_with_the_outward_wording() {
        assert_eq!(
            said_in(&viewed_with("outwardIssue", "ABC-9")),
            ["blocks ABC-9 (To Do): Add search"]
        );
    }

    #[test]
    fn a_link_holding_an_inward_issue_reads_with_the_inward_wording() {
        assert_eq!(
            said_in(&viewed_with("inwardIssue", "ABC-7")),
            ["is blocked by ABC-7 (To Do): Add search"]
        );
    }
}
