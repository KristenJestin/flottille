//! `tickets log_time` and `tickets time_spent` (`docs/tickets.md`, "Logging time"): a worklog
//! posted on a Jira work item with Jira's REST API, `acli` having no command for it, and the time
//! the agents of a session actually worked.
//!
//! The token is a vault entry the project names (`worklog_token` in its `[tracker]`): the daemon
//! reads it for this one request, and it is in no result, no card, no log and no error.

use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use flottille_core::declared::ProjectTracker;
use flottille_core::project::Project;
use flottille_core::ticket::Key;
use http::{Request, header};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use url::{Host, Url};

use super::TicketsCall;
use crate::daemon::Daemon;
use crate::forge::masked;
use crate::mcp::Outcome;
use crate::place::Place;
use crate::session_read::{form, shown_date};
use crate::vault;

/// What a call to the site may take, from the connection to the last byte.
const WORKLOG_TIME: Duration = Duration::from_secs(30);
/// How much of an answer is read: a worklog's answer is a few hundred bytes.
const ANSWER_BYTES: usize = 64 * 1024;
/// Only a test sends its calls elsewhere than to the tracker's site, to a server of this machine.
const TEST_BASE: &str = "FLOTTILLE_TEST_JIRA_BASE";

/// Jira's default working time, for a start left out: a day is 8 hours and a week 5 days.
const DAY_SECONDS: u64 = 8 * 3600;

/// A duration as Jira writes it (`1w 2d 3h 4m`, each unit once, the biggest first), as it is sent
/// and how many seconds it is.
fn duration_of(text: &str) -> Result<(String, u64), String> {
    let wrong = || {
        format!(
            "refused: duration is Jira's form, whole numbers of weeks, days, hours and minutes \
             (2h 30m, 1d, 45m), not \"{}\"",
            masked(text.trim())
        )
    };
    let mut parts = Vec::new();
    let (mut seconds, mut last) = (0u64, 4u8);
    let mut digits = String::new();
    for c in text.trim().chars() {
        match c {
            '0'..='9' => digits.push(c),
            ' ' if digits.is_empty() => {}
            'w' | 'd' | 'h' | 'm' => {
                let (rank, unit) = match c {
                    'w' => (3, 5 * DAY_SECONDS),
                    'd' => (2, DAY_SECONDS),
                    'h' => (1, 3600),
                    _ => (0, 60),
                };
                let number: u64 = digits.parse().map_err(|_| wrong())?;
                if rank >= last {
                    return Err(wrong());
                }
                last = rank;
                seconds = number
                    .checked_mul(unit)
                    .and_then(|s| seconds.checked_add(s))
                    .ok_or_else(wrong)?;
                parts.push(format!("{number}{c}"));
                digits.clear();
            }
            _ => return Err(wrong()),
        }
    }
    if !digits.is_empty() || seconds == 0 {
        return Err(wrong());
    }
    Ok((parts.join(" "), seconds))
}

/// A start, in RFC 3339 with its offset, as milliseconds since 1970.
fn started_of(text: &str) -> Result<u64, String> {
    let at = form(text.trim()).filter(|_| text.contains(['T', 't', ' ']));
    at.ok_or_else(|| {
        format!(
            "refused: started is a time with its offset, such as 2026-11-06T09:00:00Z or \
             2026-11-06T11:00:00+02:00, not \"{}\"",
            masked(text.trim())
        )
    })
}

/// A time as Jira's worklog takes it: `2026-11-06T09:00:00.000+0000`.
fn jira_time(ms: u64) -> String {
    let utc = shown_date(ms);
    format!("{}.000+0000", utc.trim_end_matches('Z'))
}

fn given<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key].as_str().filter(|text| !text.trim().is_empty())
}

/// Why a time cannot be logged on the project's tracker, before anyone is asked: GitHub has no
/// time tracking, a Jira tracker names no entry for it, and a duration or a start is wrong.
pub fn unwritable(tracker: &ProjectTracker, args: &Value) -> Option<String> {
    let worklog_token = match tracker {
        ProjectTracker::Github { .. } => {
            return Some(
                "refused: tickets log_time is for Jira: GitHub issues have no time tracking".into(),
            );
        }
        ProjectTracker::Jira { worklog_token, .. } => worklog_token,
    };
    if worklog_token.is_none() {
        return Some(
            "refused: this project names no vault entry to log time with: the user creates one \
             (flottille vault set <name> --username <their Jira account e-mail>, the secret \
             being a Jira API token) and writes its name in the project's file, [tracker] \
             worklog_token = \"<name>\", or says so in the project's Settings; Flottille never \
             logs time with a sign-in it was not given"
                .into(),
        );
    }
    let Some(duration) = given(args, "duration") else {
        return Some(
            "refused: tickets log_time needs duration, in Jira's form (2h 30m); \
             time_spent says how long the session worked"
                .into(),
        );
    };
    if let Err(why) = duration_of(duration) {
        return Some(why);
    }
    match args["started"].as_str() {
        Some(started) => started_of(started).err(),
        None => None,
    }
}

/// What the card says of a worklog, and the refusal of a vault that does not hold the entry: found
/// before the user is asked, and again when the call is done.
pub fn says(
    daemon: &Daemon,
    place: &Place,
    project: &Project,
    args: &Value,
) -> Result<Vec<String>, String> {
    if args["action"] != "log_time" {
        return Ok(Vec::new());
    }
    let asked: TicketsCall = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
    entry_of(daemon, project)?;
    let key = super::key_of(daemon, place, project, &asked).map_err(|outcome| match outcome {
        Outcome::Refused(why) | Outcome::Failed(why) | Outcome::Held(why) | Outcome::Done(why) => {
            why
        }
    })?;
    let duration = given(args, "duration")
        .and_then(|d| duration_of(d).ok())
        .map_or_else(String::new, |(text, _)| text);
    let start = match given(args, "started").and_then(|s| started_of(s).ok()) {
        Some(at) => shown_date(at),
        // Fixed when it is logged, not when it is asked: the user may take a while.
        None => format!("{duration} before it is logged"),
    };
    Ok(vec![format!(
        "It logs {duration} on {}, started {start}",
        key.text()
    )])
}

/// The vault entry the project's tracker names: its name and its account. Seen by the project, or
/// the refusal that says it is not.
fn entry_of(daemon: &Daemon, project: &Project) -> Result<(String, String), String> {
    let Some(ProjectTracker::Jira {
        worklog_token: Some(name),
        ..
    }) = &project.tracker
    else {
        return Err("this project names no vault entry to log time with".into());
    };
    let entries = daemon
        .read(|store| store.vault_entries())
        .map_err(|error| error.to_string())?;
    let entry = entries
        .iter()
        .find(|entry| &entry.name == name && entry.seen_by(&project.name));
    let Some(entry) = entry else {
        return Err(format!(
            "refused: the project's worklog_token names the vault entry {name}, and the vault \
             has none of that name for this project: the user creates it (flottille vault set \
             {name} --username <their Jira account e-mail>) or corrects the project's tracker"
        ));
    };
    match entry
        .username
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        Some(account) => Ok((name.clone(), account.to_owned())),
        None => Err(format!(
            "refused: the vault entry {name} has no username, and Jira's API takes the account's \
             e-mail with the token: the user sets it again with --username"
        )),
    }
}

/// `tickets log_time`: the worklog posted, once the gate let it through.
pub async fn log_time(
    daemon: &Daemon,
    project: &Project,
    asked: &TicketsCall,
    key: &Key,
) -> Outcome {
    let Some(ProjectTracker::Jira { site, .. }) = &project.tracker else {
        return Outcome::Refused(
            "refused: tickets log_time is for Jira: GitHub issues have no time tracking".into(),
        );
    };
    let nothing = "; nothing was written";
    let Some((duration, _)) = asked.duration.as_deref().and_then(|d| duration_of(d).ok()) else {
        return Outcome::Refused(format!(
            "refused: tickets log_time needs a duration{nothing}"
        ));
    };
    let started = match asked.started.as_deref() {
        Some(text) => match started_of(text) {
            Ok(at) => at,
            Err(why) => return Outcome::Refused(why + nothing),
        },
        None => {
            let seconds = duration_of(&duration).map_or(0, |(_, seconds)| seconds);
            flottille_core::store::now_ms().saturating_sub(seconds * 1000)
        }
    };
    let (name, account) = match entry_of(daemon, project) {
        Ok(entry) => entry,
        Err(why) => return Outcome::Refused(why + nothing),
    };
    let secret = match vault::secret(daemon, &project.name, &name).await {
        Ok(secret) => secret,
        Err(why) => return Outcome::Refused(format!("refused: {why}{nothing}")),
    };
    let mut body = json!({"timeSpent": duration, "started": jira_time(started)});
    if let Some(text) = asked.body.as_deref().filter(|b| !b.trim().is_empty()) {
        body["comment"] = crate::adf::from_markdown(text);
    }
    let credentials =
        base64::engine::general_purpose::STANDARD.encode(format!("{account}:{secret}"));
    let authorization = format!("Basic {credentials}");
    let url = match worklog_url(site, key) {
        Ok(url) => url,
        Err(why) => return Outcome::Failed(why),
    };
    let answer = tokio::time::timeout(WORKLOG_TIME, post(&url, &authorization, &body)).await;
    // Whatever the site said, the token is not in it as far as anything of ours goes.
    let clean = |text: &str| {
        masked(
            &text
                .replace(&secret, "***")
                .replace(&credentials, "***")
                .replace(&authorization, "***"),
        )
    };
    match answer {
        Err(_) => Outcome::Failed(format!(
            "the worklog was not confirmed: {site} did not answer in {} s; check the work item \
             before logging it again",
            WORKLOG_TIME.as_secs()
        )),
        Ok(Err(why)) => Outcome::Failed(clean(&why)),
        Ok(Ok((status, text))) if status.is_success() => {
            let id = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|answered| answered["id"].as_str().map(str::to_owned));
            Outcome::Done(format!(
                "logged {duration} on {}, started {}{}",
                key.text(),
                shown_date(started),
                id.map_or_else(String::new, |id| format!(" (worklog {})", clean(&id)))
            ))
        }
        Ok(Ok((status, text))) => Outcome::Failed(format!(
            "Jira on {site} answered {status} and logged nothing: {}",
            clean(&first_words(&text))
        )),
    }
}

/// What Jira said of a failure, as one line: its error messages, or the start of its answer.
fn first_words(text: &str) -> String {
    let messages = serde_json::from_str::<Value>(text).ok().map(|answered| {
        let mut said: Vec<String> = answered["errorMessages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m.as_str().map(str::to_owned))
            .collect();
        said.extend(
            answered["errors"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(_, m)| m.as_str().map(str::to_owned)),
        );
        said.join("; ")
    });
    let said = messages
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| text.to_owned());
    said.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}

/// `https://<site>/rest/api/3/issue/<KEY>/worklog`, or a test's own server in its place.
fn worklog_url(site: &str, key: &Key) -> Result<Url, String> {
    let base = std::env::var(TEST_BASE)
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .unwrap_or_else(|| format!("https://{site}"));
    Url::parse(&format!(
        "{}/rest/api/3/issue/{}/worklog",
        base.trim_end_matches('/'),
        key.text()
    ))
    .map_err(|error| format!("no address for {site}: {error}"))
}

/// One POST of `body` as JSON: the status and the start of the answer. A redirect is not followed,
/// since the credentials would go with it.
async fn post(
    url: &Url,
    authorization: &str,
    body: &Value,
) -> Result<(http::StatusCode, String), String> {
    let host = url.host_str().unwrap_or_default().to_owned();
    let failed = |what: &str, error: &dyn std::fmt::Display| format!("{what} {host}: {error}");
    let port = url.port_or_known_default().unwrap_or(443);
    let stream = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| failed("could not connect to", &e))?;
    let io: Box<dyn crate::web::Io> = if url.scheme() == "https" {
        let name = match url.host() {
            Some(Host::Domain(name)) => rustls_name(name)?,
            _ => return Err(format!("no name for TLS for {host}")),
        };
        let config = crate::web::tls().map_err(|e| failed("no TLS for", &e))?;
        Box::new(
            tokio_rustls::TlsConnector::from(config)
                .connect(name, stream)
                .await
                .map_err(|e| failed("TLS failed with", &e))?,
        )
    } else {
        Box::new(stream)
    };
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .map_err(|e| failed("HTTP failed with", &e))?;
    tokio::spawn(async move {
        let _ = tokio::time::timeout(WORKLOG_TIME, connection).await;
    });
    let host_header = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.clone(),
    };
    let request = Request::post(&url[url::Position::BeforePath..url::Position::AfterQuery])
        .header(header::HOST, host_header)
        .header(header::USER_AGENT, "flottille")
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, authorization)
        .header(header::CONNECTION, "close")
        .body(Full::new(Bytes::from(body.to_string())))
        .map_err(|e| failed("no request for", &e))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| failed("no answer from", &e))?;
    let status = response.status();
    let mut incoming = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = incoming.frame().await {
        let frame = frame.map_err(|e| failed("the answer broke off from", &e))?;
        if let Ok(data) = frame.into_data() {
            let room = ANSWER_BYTES - bytes.len();
            bytes.extend_from_slice(&data[..data.len().min(room)]);
            if data.len() >= room {
                break;
            }
        }
    }
    Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
}

fn rustls_name(name: &str) -> Result<tokio_rustls::rustls::pki_types::ServerName<'static>, String> {
    tokio_rustls::rustls::pki_types::ServerName::try_from(name.to_owned())
        .map_err(|e| format!("no name for TLS for {name}: {e}"))
}

/// `tickets time_spent`: how long the agents of the caller's session actually worked, the sum of
/// their turns, so that a duration can be proposed to the user.
pub fn time_spent(daemon: &Daemon, agent: &str) -> Outcome {
    let session = crate::tree::root_of(daemon, agent);
    let turns = match daemon.read(|store| store.turns_of_session(&session)) {
        Ok(turns) => turns,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let total: u64 = turns
        .iter()
        .map(|(_, started, ended)| ended.saturating_sub(*started))
        .sum();
    let mut agents: Vec<&str> = turns.iter().map(|(agent, _, _)| agent.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();
    if turns.is_empty() {
        return Outcome::Done(
            "no turn of this session is on record yet: there is no time to propose".into(),
        );
    }
    Outcome::Done(format!(
        "The agents of this session worked {} in {} turn{} of {} agent{}: the sum of the turns, \
         the agents that worked at the same time each counted, not the time between the first and \
         the last message. Propose it to the user as the duration of log_time; they confirm or \
         change it.",
        spent(total),
        turns.len(),
        if turns.len() == 1 { "" } else { "s" },
        agents.len(),
        if agents.len() == 1 { "" } else { "s" },
    ))
}

/// A span as Jira's form, to the minute and at least one: `1h 5m`.
fn spent(ms: u64) -> String {
    let minutes = ((ms + 30_000) / 60_000).max(1);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_jira_s_form_and_counted_in_its_working_time() {
        assert_eq!(duration_of("2h 30m"), Ok(("2h 30m".into(), 9_000)));
        assert_eq!(duration_of("1w 2d"), Ok(("1w 2d".into(), 7 * DAY_SECONDS)));
        assert_eq!(duration_of(" 90m "), Ok(("90m".into(), 5_400)));
        assert_eq!(duration_of("2h30m"), Ok(("2h 30m".into(), 9_000)));
        for wrong in [
            "",
            "0m",
            "2.5h",
            "two hours",
            "30m 2h",
            "2h 2h",
            "2",
            "h",
            "-1h",
        ] {
            assert!(duration_of(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_start_has_its_offset_and_goes_to_jira_in_utc() {
        let at = started_of("2026-11-06T11:00:00+02:00").unwrap();
        assert_eq!(jira_time(at), "2026-11-06T09:00:00.000+0000");
        assert!(started_of("2026-11-06").is_err());
        assert!(started_of("noon").is_err());
    }
}
