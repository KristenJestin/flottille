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
fn read_time() -> std::time::Duration {
    crate::test_time::tuned(
        "FLOTTILLE_TEST_SITE_WAIT_MS",
        std::time::Duration::from_secs(30),
    )
}
/// How much of an answer is read: a page of worklogs with their comments.
const ANSWER_BYTES: usize = 2 * 1024 * 1024;
/// How much of an answer to a change is read: Jira's refusal is a few lines.
const WRITE_ANSWER_BYTES: usize = 64 * 1024;
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
pub(crate) fn is_account(text: &str) -> bool {
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
    id: String,
    started: Zoned,
    key: String,
    title: String,
    seconds: u64,
    comment: String,
}

/// The calls to the tracker's site, with the entry's credentials.
pub(super) struct Site<'a> {
    site: &'a str,
    authorization: String,
    secrets: [String; 3],
}

impl Site<'_> {
    /// A text of the site's, with the token out of it as far as anything of ours goes, masked.
    pub(super) fn clean(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in &self.secrets {
            text = text.replace(secret, "***");
        }
        masked(&text)
    }

    /// `GET /rest/api/3/<path>?<query>`: the status and the text Jira answered.
    async fn read(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<(http::StatusCode, String), String> {
        let mut url = rest_url(self.site, path)?;
        url.query_pairs_mut().extend_pairs(query);
        let answer = tokio::time::timeout(
            read_time(),
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
                read_time().as_secs()
            )),
            Ok(Err(why)) => Err(self.clean(&why)),
            Ok(Ok(answered)) => Ok(answered),
        }
    }

    /// `GET /rest/api/3/<path>?<query>`: what Jira answered, as JSON.
    pub(super) async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        match self.read(path, query).await? {
            (status, text) if status.is_success() => serde_json::from_str(&text)
                .map_err(|_| format!("Jira on {} answered what is not JSON", self.site)),
            (status, text) => Err(format!(
                "Jira on {} answered {status}: {}",
                self.site,
                self.clean(&super::worklog::first_words(&text))
            )),
        }
    }

    /// `GET /rest/api/3/issue/<key>/worklog/<id>`: the worklog, or none when Jira has no such
    /// worklog. One that is on another work item is answered too (its `issueId` says which).
    async fn worklog(&self, key: &str, id: &str) -> Result<Option<Value>, String> {
        let (status, text) = self.read(&format!("issue/{key}/worklog/{id}"), &[]).await?;
        if status == http::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(format!(
                "Jira on {} answered {status}: {}",
                self.site,
                self.clean(&super::worklog::first_words(&text))
            ));
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| format!("Jira on {} answered what is not JSON", self.site))
    }

    /// The display name the site gives the account `id` (`GET /rest/api/3/user?accountId=`); none
    /// for an id the site does not know.
    pub(super) async fn display_name(&self, id: &str) -> Result<Option<String>, String> {
        let (status, text) = self.read("user", &[("accountId", id)]).await?;
        if status == http::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(format!(
                "Jira on {} answered {status} to the look-up of the account {}: {}",
                self.site,
                self.clean(id),
                self.clean(&super::worklog::first_words(&text))
            ));
        }
        let name = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|user| user["displayName"].as_str().map(str::to_owned))
            .filter(|name| !name.trim().is_empty());
        name.map(|name| Some(self.clean(name.trim())))
            .ok_or_else(|| {
                format!(
                    "Jira on {} answered no display name for the account {}",
                    self.site,
                    self.clean(id)
                )
            })
    }
}

impl Site<'_> {
    /// `PUT /rest/api/3/<path>` with `body` as JSON: Jira answers a change with `204` and nothing
    /// to read, or says why it refused. A write: "nothing was written" is said of a status that is no
    /// success alone; a time-out, or a failure once the request is sent, may follow a change Jira
    /// applied, so it is said as not confirmed.
    pub(super) async fn put(&self, path: &str, body: &Value) -> Result<(), String> {
        self.write(http::Method::PUT, path, Some(body))
            .await
            .map(drop)
    }

    /// `POST /rest/api/3/<path>` with `body` as JSON: what Jira created, as JSON, said as `put`
    /// says a refusal and a doubt.
    pub(super) async fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let text = self.write(http::Method::POST, path, Some(body)).await?;
        serde_json::from_str(&text).map_err(|_| {
            format!(
                "the change was not confirmed: Jira on {} answered what is not JSON; check the \
                 work item before changing it again",
                self.site
            )
        })
    }

    /// `DELETE /rest/api/3/<path>`: Jira answers `204`, or says why it refused; said as `put` says
    /// a refusal and a doubt.
    pub(super) async fn delete(&self, path: &str) -> Result<(), String> {
        self.write(http::Method::DELETE, path, None).await.map(drop)
    }

    async fn write(
        &self,
        method: http::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<String, String> {
        let url = rest_url(self.site, path)?;
        let answer = tokio::time::timeout(
            read_time(),
            request(method, &url, &self.authorization, body, WRITE_ANSWER_BYTES),
        )
        .await;
        match answer {
            Err(_) => Err(format!(
                "the change was not confirmed: {} did not answer in {} s; check the work item \
                 before changing it again",
                self.site,
                read_time().as_secs()
            )),
            Ok(Err(why)) => Err(format!(
                "{}; the change was not confirmed: check the work item before changing it again",
                self.clean(&why)
            )),
            Ok(Ok((status, text))) if status.is_success() => Ok(text),
            Ok(Ok((status, text))) => Err(format!(
                "Jira on {} answered {status}: {}; nothing was written",
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
    let site = match &project.tracker {
        Some(ProjectTracker::Jira { site, .. }) => site,
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
    let (jira, name, stamp) = match signed_in(daemon, project, site, no_entry()).await {
        Ok(signed) => signed,
        Err(why) => return Outcome::Refused(why),
    };
    match read(project, &jira, (&name, stamp), (from, to), ticket, asked).await {
        Ok(text) => Outcome::Done(text),
        Err(Outcome::Failed(why)) => {
            Outcome::Failed(format!("the worklogs could not be read: {why}"))
        }
        Err(other) => other,
    }
}

/// The calls to `site` as the project's vault entry signs them: the client, the entry's name and
/// the moment it was last set, or the refusal (`none` when the project names no entry) that says
/// how to set one. The secret is read for the worklog's use alone, as `log_time` reads it, and
/// goes to the tracker's site and nowhere else.
pub(super) async fn signed_in<'a>(
    daemon: &Daemon,
    project: &Project,
    site: &'a str,
    none: String,
) -> Result<(Site<'a>, String, u64), String> {
    if !matches!(
        &project.tracker,
        Some(ProjectTracker::Jira {
            worklog_token: Some(_),
            ..
        })
    ) {
        return Err(none);
    }
    let (name, email, stamp) = entry_of(daemon, project)?;
    let secret = vault::secret(
        daemon,
        &project.name,
        &flottille_core::vault::worklog(&project.name),
        &name,
    )
    .await
    .map_err(|why| format!("refused: {why}"))?;
    let credentials = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{secret}"));
    let authorization = format!("Basic {credentials}");
    let jira = Site {
        site,
        secrets: [secret, credentials, authorization.clone()],
        authorization,
    };
    Ok((jira, name, stamp))
}

/// A worklog's comment as one line, masked and cut.
fn comment_line(comment: &Value) -> String {
    let text = crate::adf::to_markdown(comment);
    masked(text.split_whitespace().collect::<Vec<_>>().join(" ").trim())
        .chars()
        .take(COMMENT_CHARS)
        .collect()
}

/// One worklog of the token's own account, as Jira has it now.
pub(super) struct Current {
    /// Jira's form of the time spent (`1h 30m`).
    pub time: String,
    /// When it started, in milliseconds since 1970.
    pub started: u64,
    pub comment: String,
}

/// A worklog the work item does not have: it does not exist, or it is on another work item.
fn not_on_the_item(key: &str, id: &str) -> Outcome {
    Outcome::Refused(format!(
        "refused: {key} has no worklog {}: tickets worklogs gives the id of each worklog, and the \
         work item it is on; nothing was written",
        masked(id)
    ))
}

/// The worklog `id` of the work item `key`, once it is known to be the token's own account's:
/// refused, before anything is written, when it is not on that work item (or does not exist) or
/// is another author's.
pub(super) async fn own_worklog(
    project: &Project,
    jira: &Site<'_>,
    entry: (&str, u64),
    key: &str,
    id: &str,
) -> Result<Current, Outcome> {
    let Some(worklog) = jira
        .worklog(key, id)
        .await
        .map_err(|why| Outcome::Failed(format!("the worklog could not be read: {why}")))?
    else {
        return Err(not_on_the_item(key, id));
    };
    // Jira answers a worklog of another work item too, naming the work item it is on: that one is
    // compared with this one's id, not taken for a 404.
    let item = jira
        .get(&format!("issue/{key}"), &[("fields", "id")])
        .await
        .map_err(|why| Outcome::Failed(format!("the work item could not be read: {why}")))?;
    if item["id"].as_str().is_none() || worklog["issueId"].as_str() != item["id"].as_str() {
        return Err(not_on_the_item(key, id));
    }
    let (account, _) = own_account(project, jira, entry).await?;
    if worklog["author"]["accountId"].as_str() != Some(account.as_str()) {
        let author = worklog["author"]["displayName"]
            .as_str()
            .or_else(|| worklog["author"]["accountId"].as_str())
            .unwrap_or("another account");
        return Err(Outcome::Refused(format!(
            "refused: worklog {} is {}'s, not your account's; nothing was written",
            masked(id),
            masked(author)
        )));
    }
    let seconds = worklog["timeSpentSeconds"].as_u64().unwrap_or(0);
    Ok(Current {
        time: worklog["timeSpent"]
            .as_str()
            .map_or_else(|| span(seconds), masked),
        started: worklog["started"]
            .as_str()
            .and_then(started_of)
            .map_or(0, |at| u64::try_from(at.as_millisecond()).unwrap_or(0)),
        comment: comment_line(&worklog["comment"]),
    })
}

/// Whose worklogs are read: the account, and how the answer names it.
async fn account(
    project: &Project,
    jira: &Site<'_>,
    entry: (&str, u64),
    asked: &TicketsCall,
) -> Result<(String, String), Outcome> {
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
        None => own_account(project, jira, entry).await,
    }
}

/// The account the token belongs to, asked of the site once and kept (`GET /rest/api/3/myself`),
/// and how an answer names it.
pub(super) async fn own_account(
    project: &Project,
    jira: &Site<'_>,
    entry: (&str, u64),
) -> Result<(String, String), Outcome> {
    let (account, name) = own_account_named(project, jira, entry).await?;
    Ok((account, format!("your account ({name})")))
}

/// The account the token belongs to and its display name, asked of the site once and kept.
pub(super) async fn own_account_named(
    project: &Project,
    jira: &Site<'_>,
    (entry, stamp): (&str, u64),
) -> Result<(String, String), Outcome> {
    let failed = |why: String| Outcome::Failed(why);
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
    if let Some(kept) = kept {
        return Ok(kept);
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
    Ok((account, name))
}

async fn read(
    project: &Project,
    jira: &Site<'_>,
    entry: (&str, u64),
    (from, to): (Date, Date),
    ticket: Option<flottille_core::ticket::Key>,
    asked: &TicketsCall,
) -> Result<String, Outcome> {
    let failed = Outcome::Failed;
    let (account, mut whose) = account(project, jira, entry, asked).await?;
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
            // Jira reads the days of a JQL in the zone of the account's profile, which may not be
            // the machine's: a day of margin on each side, and the exact cut is the local one
            // below. Zones more than 24 hours apart are not covered.
            let (before, after) = (
                from.yesterday().unwrap_or(from),
                to.tomorrow().unwrap_or(to),
            );
            let jql = format!(
                "project in ({projects}) AND worklogDate >= {} AND worklogDate <= {} AND \
                 worklogAuthor = {} ORDER BY key ASC",
                jql_quoted(&before.to_string()),
                jql_quoted(&after.to_string()),
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
    // The site is asked for the range only, a day of margin each side (as the search is), so that
    // the worklogs of every other day and person are not all read; the cut below stays exact.
    let day_ms = 24 * 3600 * 1000;
    let edge = |day: Date| {
        day.to_zoned(zone.clone())
            .map_or(0, |zoned| zoned.timestamp().as_millisecond())
    };
    let asked_after = (edge(from) - day_ms).to_string();
    let asked_before = (edge(to.tomorrow().unwrap_or(to)) + day_ms).to_string();
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
                    &[
                        ("startAt", &at),
                        ("maxResults", "100"),
                        ("startedAfter", &asked_after),
                        ("startedBefore", &asked_before),
                    ],
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
                let comment = comment_line(&worklog["comment"]);
                days.entry(started.date()).or_default().push(Entry {
                    id: worklog["id"].as_str().map(masked).unwrap_or_default(),
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
                "- {} {} — {} (started {:02}:{:02}, worklog {})",
                entry.key,
                entry.title,
                span(entry.seconds),
                entry.started.hour(),
                entry.started.minute(),
                entry.id
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
