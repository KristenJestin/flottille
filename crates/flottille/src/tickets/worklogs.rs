//! `tickets worklogs` (`docs/tickets.md`, "Reading worklogs"): the worklogs of a day or a range, with
//! their durations, read with Jira's REST API and the vault entry the project names for `log_time`.
//!
//! A read, never held. The daemon uses the secret itself, toward the site the tracker names and
//! no other; it is in no result, no log and no error. The answer says whose time it is and in
//! which time zone the days are, and says when a bound cut it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex, PoisonError};

use base64::Engine as _;
use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use serde_json::Value;

use super::worklog::{entry_of, no_entry, request, rest_url};
use super::{TicketsCall, jql_quoted, people};
use crate::daemon::Daemon;
use crate::forge::masked;
use crate::mcp::Outcome;
use crate::vault;

/// What a call to the site may take, from the connection to the last byte.
const READ_TIME: std::time::Duration = std::time::Duration::from_secs(30);
/// How much of an answer is read: a page of worklogs with their comments.
const ANSWER_BYTES: usize = 2 * 1024 * 1024;
/// How many work items one search asks for: the bound of the read, said when reached.
const SEARCHED: usize = 100;
/// How many pages of worklogs one work item is read through, of what the site gives at a time.
const PAGES: usize = 50;
/// How much of a worklog's comment the answer quotes.
const COMMENT_CHARS: usize = 300;

/// The account a token belongs to, kept for the daemon's life: per project, entry and site, and
/// the moment the entry was last set, so that a changed entry never has another's account.
type AccountKey = (String, String, String, u64);
static ACCOUNTS: LazyLock<Mutex<HashMap<AccountKey, (String, String)>>> =
    LazyLock::new(Mutex::default);

/// Whether `text` is a Jira account id: 24 hexadecimal digits, or `<number>:<uuid>`.
pub(super) fn is_account(text: &str) -> bool {
    let hex = |text: &str| text.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    (text.len() == 24 && text.chars().all(|c| c.is_ascii_hexdigit()))
        || text.split_once(':').is_some_and(|(number, id)| {
            !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit())
                && !id.is_empty()
                && hex(id)
        })
}

/// One of the author's worklogs in the range.
struct Entry {
    started: Zoned,
    key: String,
    title: String,
    seconds: u64,
    comment: String,
}

/// The calls to the tracker's site, with the entry's credentials.
struct Site<'a> {
    site: &'a str,
    authorization: String,
    secrets: [String; 3],
}

impl Site<'_> {
    /// A text of the site's, with the token out of it as far as anything of ours goes, masked.
    fn clean(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in &self.secrets {
            text = text.replace(secret, "***");
        }
        masked(&text)
    }

    /// `GET /rest/api/3/<path>?<query>`: what Jira answered, as JSON.
    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let mut url = rest_url(self.site, path)?;
        url.query_pairs_mut().extend_pairs(query);
        let answer = tokio::time::timeout(
            READ_TIME,
            request(
                http::Method::GET,
                &url,
                &self.authorization,
                None,
                ANSWER_BYTES,
            ),
        )
        .await;
        match answer {
            Err(_) => Err(format!(
                "{} did not answer in {} s",
                self.site,
                READ_TIME.as_secs()
            )),
            Ok(Err(why)) => Err(self.clean(&why)),
            Ok(Ok((status, text))) if status.is_success() => serde_json::from_str(&text)
                .map_err(|_| format!("Jira on {} answered what is not JSON", self.site)),
            Ok(Ok((status, text))) => Err(format!(
                "Jira on {} answered {status}: {}",
                self.site,
                self.clean(&super::worklog::first_words(&text))
            )),
        }
    }
}

/// A day as `YYYY-MM-DD`.
fn day_of(key: &str, text: Option<&str>) -> Result<Date, String> {
    let wrong = |what: &str| {
        format!(
            "refused: tickets worklogs needs {key}{what}, a day as YYYY-MM-DD (2026-11-06), in the \
             user's time zone",
        )
    };
    let text = text.map(str::trim).ok_or_else(|| wrong(""))?;
    match text.parse::<Date>() {
        Ok(day) if text.len() == 10 => Ok(day),
        _ => Err(wrong(&format!(", not {:?}", masked(text)))),
    }
}

/// A time as Jira writes it (`2026-11-06T09:12:00.000+0100`), as an instant.
fn started_of(text: &str) -> Option<Timestamp> {
    let (head, offset) = text.split_at_checked(text.len().checked_sub(5)?)?;
    let digits = offset.get(1..)?;
    let rfc = match offset.chars().next()? {
        '+' | '-' if digits.chars().all(|c| c.is_ascii_digit()) => {
            format!("{head}{}:{}", &offset[..3], &offset[3..])
        }
        _ => text.to_owned(),
    };
    rfc.parse().ok()
}

/// A span as the user reads it: `1h30`, `2h`, `45m`.
fn span(seconds: u64) -> String {
    let minutes = seconds / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) if seconds > 0 => "<1m".into(),
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m:02}"),
    }
}

/// The zone as the answer names it: its name, else its offset.
fn zone_said(zone: &TimeZone, at: Timestamp) -> String {
    zone.iana_name()
        .map_or_else(|| format!("UTC{}", zone.to_offset(at)), str::to_owned)
}

/// `tickets worklogs {from, to?, ticket?, author?}`.
pub async fn worklogs(daemon: &Daemon, project: &Project, asked: &TicketsCall) -> Outcome {
    let (site, worklog_token) = match &project.tracker {
        Some(ProjectTracker::Jira {
            site,
            worklog_token,
            ..
        }) => (site, worklog_token),
        Some(ProjectTracker::Github { .. }) => {
            return Outcome::Refused(
                "refused: tickets worklogs is for Jira: GitHub has no worklogs".into(),
            );
        }
        None => return Outcome::Refused(format!("refused: {}", super::NO_TRACKER)),
    };
    let from = match day_of("from", asked.from.as_deref()) {
        Ok(day) => day,
        Err(why) => return Outcome::Refused(why),
    };
    let to = match asked.to.as_deref() {
        Some(_) => match day_of("to", asked.to.as_deref()) {
            Ok(day) => day,
            Err(why) => return Outcome::Refused(why),
        },
        None => from,
    };
    if to < from {
        return Outcome::Refused(format!(
            "refused: tickets worklogs takes a to ({to}) that is not before its from ({from})"
        ));
    }
    let ticket = match asked
        .ticket
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(text) => match super::accepted(project, text) {
            Ok(key) => Some(key),
            Err(why) => return Outcome::Refused(format!("refused: {why}")),
        },
        None => None,
    };
    if worklog_token.is_none() {
        return Outcome::Refused(no_entry());
    }
    let (name, email) = match entry_of(daemon, project) {
        Ok(entry) => entry,
        Err(why) => return Outcome::Refused(why),
    };
    // The secret is read as `log_time` reads it, for the worklog's use alone.
    let secret = match vault::secret(
        daemon,
        &project.name,
        &flottille_core::vault::worklog(&project.name),
        &name,
    )
    .await
    {
        Ok(secret) => secret,
        Err(why) => return Outcome::Refused(format!("refused: {why}")),
    };
    let credentials = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{secret}"));
    let authorization = format!("Basic {credentials}");
    let jira = Site {
        site,
        secrets: [secret, credentials, authorization.clone()],
        authorization,
    };
    match read(daemon, project, &jira, &name, (from, to), ticket, asked).await {
        Ok(text) => Outcome::Done(text),
        Err(Outcome::Failed(why)) => {
            Outcome::Failed(format!("the worklogs could not be read: {why}"))
        }
        Err(other) => other,
    }
}

/// Whose worklogs are read: the account, and how the answer names it.
async fn account(
    daemon: &Daemon,
    project: &Project,
    jira: &Site<'_>,
    entry: &str,
    asked: &TicketsCall,
) -> Result<(String, String), Outcome> {
    let failed = |why: String| Outcome::Failed(why);
    match asked
        .author
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        Some(author) if is_account(author) => Ok((author.to_owned(), String::new())),
        Some(author) => people::account_of(project, author)
            .await
            .map_err(Outcome::Refused),
        None => {
            let stamp = daemon
                .read(|store| store.vault_entries())
                .map_err(|error| failed(error.to_string()))?
                .iter()
                .find(|e| e.name == entry)
                .map_or(0, |e| e.updated_at);
            let key = (
                project.name.clone(),
                entry.to_owned(),
                jira.site.to_owned(),
                stamp,
            );
            let kept = ACCOUNTS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&key)
                .cloned();
            if let Some((account, name)) = kept {
                return Ok((account, format!("your account ({name})")));
            }
            let me = jira.get("myself", &[]).await.map_err(failed)?;
            let account = me["accountId"].as_str().unwrap_or_default().to_owned();
            if account.is_empty() {
                return Err(failed(format!(
                    "Jira on {} did not say whose account the token is",
                    jira.site
                )));
            }
            let name = masked(me["displayName"].as_str().unwrap_or(&account));
            ACCOUNTS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key, (account.clone(), name.clone()));
            Ok((account, format!("your account ({name})")))
        }
    }
}

async fn read(
    daemon: &Daemon,
    project: &Project,
    jira: &Site<'_>,
    entry: &str,
    (from, to): (Date, Date),
    ticket: Option<flottille_core::ticket::Key>,
    asked: &TicketsCall,
) -> Result<String, Outcome> {
    let failed = Outcome::Failed;
    let (account, mut whose) = account(daemon, project, jira, entry, asked).await?;
    let zone = TimeZone::system();
    let mut notes = Vec::new();

    // Which work items: the one named, or those the site says have a worklog of the account in
    // the range (bounded, as a search is).
    let items: Vec<(String, String)> = match &ticket {
        Some(key) => {
            let item = jira
                .get(&format!("issue/{}", key.text()), &[("fields", "summary")])
                .await
                .map_err(failed)?;
            vec![(
                key.text(),
                masked(item["fields"]["summary"].as_str().unwrap_or("")),
            )]
        }
        None => {
            let projects = match &project.tracker {
                Some(ProjectTracker::Jira { projects, .. }) => projects
                    .iter()
                    .map(|p| jql_quoted(p))
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => String::new(),
            };
            let jql = format!(
                "project in ({projects}) AND worklogDate >= {} AND worklogDate <= {} AND \
                 worklogAuthor = {} ORDER BY key ASC",
                jql_quoted(&from.to_string()),
                jql_quoted(&to.to_string()),
                jql_quoted(&account)
            );
            let max = SEARCHED.to_string();
            let found = jira
                .get(
                    "search/jql",
                    &[("jql", &jql), ("fields", "summary"), ("maxResults", &max)],
                )
                .await
                .map_err(failed)?;
            if found["isLast"] == false || found["nextPageToken"].is_string() {
                notes.push(format!(
                    "the search was cut at {SEARCHED} work items: narrow the range, or name a \
                     ticket"
                ));
            }
            found["issues"]
                .as_array()
                .into_iter()
                .flatten()
                .take(SEARCHED)
                .filter_map(|item| {
                    Some((
                        item["key"].as_str()?.to_owned(),
                        masked(item["fields"]["summary"].as_str().unwrap_or("")),
                    ))
                })
                .collect()
        }
    };

    // Each item's worklogs, paged as the site pages them, kept those of the account and started
    // inside the range in the user's time zone.
    let mut days: BTreeMap<Date, Vec<Entry>> = BTreeMap::new();
    let mut name_seen = None;
    for (key, title) in &items {
        let mut start = 0usize;
        for page in 0.. {
            if page == PAGES {
                notes.push(format!(
                    "{key} has more worklogs than the {PAGES} pages read: the rest is not counted"
                ));
                break;
            }
            let at = start.to_string();
            let answer = jira
                .get(
                    &format!("issue/{key}/worklog"),
                    &[("startAt", &at), ("maxResults", "100")],
                )
                .await
                .map_err(failed)?;
            let worklogs = answer["worklogs"].as_array().cloned().unwrap_or_default();
            for worklog in &worklogs {
                if worklog["author"]["accountId"].as_str() != Some(account.as_str()) {
                    continue;
                }
                let Some(at) = worklog["started"].as_str().and_then(started_of) else {
                    continue;
                };
                let started = at.to_zoned(zone.clone());
                if !(from..=to).contains(&started.date()) {
                    continue;
                }
                name_seen = name_seen
                    .or_else(|| worklog["author"]["displayName"].as_str().map(str::to_owned));
                let comment = crate::adf::to_markdown(&worklog["comment"]);
                let comment: String = masked(
                    comment
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim(),
                )
                .chars()
                .take(COMMENT_CHARS)
                .collect();
                days.entry(started.date()).or_default().push(Entry {
                    started,
                    key: key.clone(),
                    title: title.clone(),
                    seconds: worklog["timeSpentSeconds"].as_u64().unwrap_or(0),
                    comment,
                });
            }
            start += worklogs.len();
            let total = answer["total"].as_u64().unwrap_or(0) as usize;
            if worklogs.is_empty() || start >= total {
                break;
            }
        }
    }

    if whose.is_empty() {
        whose = match name_seen {
            Some(name) => format!("{} (account {account})", masked(&name)),
            None => format!("account {account}"),
        };
    }
    let range = match from == to {
        true => format!("on {from}"),
        false => format!("from {from} to {to}"),
    };
    let at = from
        .to_zoned(zone.clone())
        .map_or_else(|_| Timestamp::UNIX_EPOCH, |z| z.timestamp());
    let mut text = format!(
        "Worklogs of {whose} {range}, the days in the time zone {}:\n",
        zone_said(&zone, at)
    );
    let mut all = 0;
    for (day, entries) in &mut days {
        entries.sort_by(|a, b| a.started.cmp(&b.started));
        text.push_str(&format!("{day}\n"));
        for entry in entries.iter() {
            text.push_str(&format!(
                "- {} {} — {} (started {:02}:{:02})",
                entry.key,
                entry.title,
                span(entry.seconds),
                entry.started.hour(),
                entry.started.minute()
            ));
            if !entry.comment.is_empty() {
                text.push_str(&format!(" \"{}\"", entry.comment));
            }
            text.push('\n');
        }
        let day_total: u64 = entries.iter().map(|e| e.seconds).sum();
        all += day_total;
        text.push_str(&format!("  Day total: {}\n", span(day_total)));
    }
    if days.is_empty() {
        text.push_str("none: no worklog of that account started in that range\n");
    }
    text.push_str(&format!("Range total: {}\n", span(all)));
    for note in notes {
        text.push_str(&format!("({note})\n"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_of_jira_is_read_with_its_offset_and_a_span_is_short() {
        let at = started_of("2026-11-06T09:12:00.000+0100").unwrap();
        assert_eq!(at.to_string(), "2026-11-06T08:12:00Z");
        assert_eq!(
            started_of("2026-11-06T09:12:00.000-0500")
                .unwrap()
                .to_string(),
            "2026-11-06T14:12:00Z"
        );
        assert!(started_of("yesterday").is_none());
        assert_eq!(span(5_400), "1h30");
        assert_eq!(span(2_700), "45m");
        assert_eq!(span(7_200), "2h");
        assert_eq!(span(3_900), "1h05");
        assert_eq!(span(20), "<1m");
        assert_eq!(span(0), "0m");
    }

    #[test]
    fn an_account_id_is_told_from_a_name() {
        assert!(is_account("5b10a2844c20165700ede21a"));
        assert!(is_account("712020:0c1d9a6e-7a4e-4f3a-9b1e-2d6f5c8a1b34"));
        assert!(!is_account("Jane Doe"));
        assert!(!is_account("jane@example.com"));
    }
}
