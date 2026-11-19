//! Logging time on a Jira work item (#383, `docs/tickets.md`, "Logging time"): `tickets log_time`
//! posts a worklog on the tracker's site with the vault entry the project names, under the gate,
//! and `tickets time_spent` sums the turns of a session. Jira is a fake HTTP server on this
//! machine, the vault's keyring the daemon's memory, and the `acli` the fake: no test reaches a
//! site, the user's keyring or anyone's sign-in.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::agent::{TurnUsage, Used};
use flottille_core::store::{Store, now_ms};
use serde_json::{Value, json};

const SITE: &str = "example.atlassian.net";
const SECRET: &str = "w0rklog-t0ken-never-shown";
const ACCOUNT: &str = "dev@example.com";
/// What the daemon reads to send its Jira calls to the fake server (a debug build only).
const BASE: &str = "FLOTTILLE_TEST_JIRA_BASE";

/// What the fake Jira received of one request.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    authorization: String,
    body: Value,
}

/// A Jira that answers every request with `status` and `body`, and remembers what it was asked.
struct Jira {
    base: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Jira {
    fn start(status: &'static str, body: impl Fn(&Received) -> String + Send + 'static) -> Jira {
        Jira::reply(move |request| (status, body(request)))
    }

    /// A Jira that answers each request with a status and a body of its own choosing.
    fn reply(answer: impl Fn(&Received) -> (&'static str, String) + Send + 'static) -> Jira {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let kept = received.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, mut rest) = loop {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    raw.extend_from_slice(&chunk[..read]);
                    if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (
                            String::from_utf8_lossy(&raw[..at]).into_owned(),
                            raw[at + 4..].to_vec(),
                        );
                    }
                    if read == 0 {
                        return;
                    }
                };
                let header = |name: &str| {
                    head.lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.trim().to_owned())
                        .unwrap_or_default()
                };
                let length: usize = header("content-length").parse().unwrap_or(0);
                while rest.len() < length {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    rest.extend_from_slice(&chunk[..read]);
                }
                let mut line = head.lines().next().unwrap_or_default().split(' ');
                let request = Received {
                    method: line.next().unwrap_or_default().to_owned(),
                    path: line.next().unwrap_or_default().to_owned(),
                    authorization: header("authorization"),
                    body: serde_json::from_slice(&rest).unwrap_or(Value::Null),
                };
                let (status, said) = answer(&request);
                kept.lock().unwrap().push(request);
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: \
                         {}\r\nConnection: close\r\n\r\n{said}",
                        said.len()
                    )
                    .as_bytes(),
                );
            }
        });
        Jira { base, received }
    }

    /// A site that accepts every worklog.
    fn accepting() -> Jira {
        Jira::start("201 Created", |_| json!({"id": "10042"}).to_string())
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

fn user(jira: &Jira) -> User {
    User::with_fake_agent()
        .with_fake_acli()
        .with_env(BASE, &jira.base)
}

/// A project `app` over a throwaway repository, its tracker `tracker`, and a ready workspace.
fn fixture(user: &User, tracker: Value) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": tracker}),
    )
    .unwrap();
}

fn jira_with(user: &User, entry: Option<&str>) {
    let mut tracker = json!({"kind": "jira", "site": SITE, "projects": ["ABC"]});
    if let Some(entry) = entry {
        tracker["worklog_token"] = entry.into();
    }
    fixture(user, tracker);
}

/// The entry `jira-time` of the project, with its secret and its account.
fn entry(user: &User) {
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT, "project": "app"}),
    )
    .unwrap();
}

/// The root agent of `app` and the client of its tools.
fn root_in(user: &User) -> (Mcp, String) {
    let opened = user.received_requests("session/new").len();
    let run = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        Mcp::initialize(&url, &authorization),
        run["agent"]["id"].as_str().unwrap().to_owned(),
    )
}

fn store(user: &User) -> Store {
    Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

/// A call held for the user: its request's number.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The user allows request `number`: the text the agent is handed for it.
fn allow(user: &User, number: u64) -> String {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

/// A call refused before anything is asked of the user: what the agent is told.
fn refused_at_once(user: &User, mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("tickets", args.clone());
    assert!(answer.error, "{args}: {}", answer.text);
    assert!(answer.text.starts_with("refused:"), "{}", answer.text);
    assert!(requests(user).is_empty(), "{args} was held");
    answer.text
}

/// The names of the files under `dir` whose bytes hold `needle`.
fn files_holding(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for item in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = item.path();
            if path.is_dir() {
                folders.push(path);
            } else if std::fs::read(&path)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
            {
                found.push(path);
            }
        }
    }
    found
}

fn basic() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{ACCOUNT}:{SECRET}"))
    )
}

/// The text of a comment in Jira's document format.
fn text_of(document: &Value) -> String {
    document["content"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|block| block["content"].as_array().into_iter().flatten())
        .filter_map(|node| node["text"].as_str())
        .collect()
}

#[test]
fn log_time_posts_a_worklog_with_the_vault_token() {
    let jira = Jira::accepting();
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "log_time", "ticket": "abc-1", "duration": "2h 30m",
            "started": "2026-11-06T09:00:00Z", "body": "Fixed the login redirect"}),
    ));
    // The card says the ticket, the duration and the start; nothing was sent yet.
    let card = requests(&user)
        .iter()
        .find(|r| r["number"] == number)
        .unwrap()
        .to_string();
    for said in ["ABC-1", "2h 30m", "2026-11-06T09:00:00Z"] {
        assert!(card.contains(said), "{said} is not on the card: {card}");
    }
    assert!(
        jira.received().is_empty(),
        "sent before the user allowed it"
    );

    let done = allow(&user, number);
    assert!(done.contains("10042"), "{done}");
    let received = jira.received();
    assert_eq!(received.len(), 1, "{received:?}");
    let post = &received[0];
    assert_eq!(post.method, "POST");
    assert_eq!(post.path, "/rest/api/3/issue/ABC-1/worklog");
    assert_eq!(post.authorization, basic());
    assert_eq!(post.body["timeSpent"], "2h 30m");
    assert_eq!(post.body["started"], "2026-11-06T09:00:00.000+0000");
    assert_eq!(post.body["comment"]["type"], "doc");
    assert_eq!(text_of(&post.body["comment"]), "Fixed the login redirect");
    assert!(!done.contains(SECRET), "{done}");
}

#[test]
fn log_time_without_a_start_begins_one_duration_before_now() {
    let jira = Jira::accepting();
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let before = now_ms();
    let number = held(&mcp.call(
        "tickets",
        json!({"action": "log_time", "ticket": "ABC-2", "duration": "1h"}),
    ));
    let card = requests(&user)
        .iter()
        .find(|r| r["number"] == number)
        .unwrap()
        .to_string();
    assert!(card.contains("1h before"), "the card says no start: {card}");
    allow(&user, number);
    let after = now_ms();

    let started = jira.received()[0].body["started"]
        .as_str()
        .unwrap()
        .to_owned();
    // `2026-11-06T09:00:00.000+0000`, in UTC.
    let seconds = started.strip_suffix(".000+0000").unwrap();
    let at = flottille_start(seconds);
    assert!(
        (before - 3_600_000 - 1_000..=after - 3_600_000 + 1_000).contains(&at),
        "{started}"
    );
    assert!(jira.received()[0].body.get("comment").is_none());
}

/// Milliseconds since 1970 of `YYYY-MM-DDTHH:MM:SS`, in UTC.
fn flottille_start(text: &str) -> u64 {
    let (day, clock) = text.split_once('T').unwrap();
    let part = |s: &str, at: usize| s.split(['-', ':']).nth(at).unwrap().parse::<i64>().unwrap();
    let (year, month, mday) = (part(day, 0), part(day, 1), part(day, 2));
    let year_shifted = if month <= 2 { year - 1 } else { year };
    let era = year_shifted.div_euclid(400);
    let yoe = year_shifted - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + mday - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let seconds = days * 86_400 + part(clock, 0) * 3600 + part(clock, 1) * 60 + part(clock, 2);
    u64::try_from(seconds).unwrap() * 1000
}

#[test]
fn log_time_is_refused_on_github() {
    let jira = Jira::accepting();
    let user = user(&jira);
    fixture(&user, json!({"kind": "github", "repository": "."}));
    let (mcp, _) = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "log_time", "ticket": "#1", "duration": "1h"}),
    );
    assert!(
        said.contains("GitHub issues have no time tracking"),
        "{said}"
    );
    assert!(jira.received().is_empty());
}

#[test]
fn log_time_without_a_configured_entry_is_refused() {
    let jira = Jira::accepting();
    let user = user(&jira);
    jira_with(&user, None);
    entry(&user);
    let (mcp, _) = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h"}),
    );
    // It says how to set one: the key of the tracker, and the entry that holds the token.
    for how in ["worklog_token", "[tracker]", "vault"] {
        assert!(said.contains(how), "{how} is not said: {said}");
    }
    // The window first, the file second: the user's way is the project's Settings.
    let (window, file) = (said.find("Settings"), said.find("worklog_token"));
    assert!(
        window.is_some() && window < file,
        "the window is not said before the file: {said}"
    );

    // An entry named, but not in the vault (or not seen by the project), is refused too.
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "jira", "site": SITE, "projects": ["ABC"],
                "worklog_token": "nowhere"}}),
    )
    .unwrap();
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h"}),
    );
    assert!(said.contains("nowhere"), "{said}");
    assert!(jira.received().is_empty());
}

#[test]
fn log_time_without_a_ticket_is_refused() {
    let jira = Jira::accepting();
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    // The workspace's own ticket is not guessed at execution: the card names the ticket.
    let said = refused_at_once(&user, &mcp, json!({"action": "log_time", "duration": "1h"}));
    assert!(said.contains("needs ticket"), "{said}");
    assert!(jira.received().is_empty());
}

#[test]
fn a_duration_that_is_not_jira_s_form_is_refused() {
    let jira = Jira::accepting();
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    for duration in ["", "two hours", "2.5h", "0m", "30m 2h", "2h2h"] {
        refused_at_once(
            &user,
            &mcp,
            json!({"action": "log_time", "ticket": "ABC-1", "duration": duration}),
        );
    }
    let missing = refused_at_once(
        &user,
        &mcp,
        json!({"action": "log_time", "ticket": "ABC-1"}),
    );
    assert!(missing.contains("duration"), "{missing}");
    let start = refused_at_once(
        &user,
        &mcp,
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h", "started": "noon"}),
    );
    assert!(start.contains("started"), "{start}");
    assert!(jira.received().is_empty());
}

#[test]
fn the_token_never_appears_in_what_is_stored() {
    // A site that fails, and echoes what it was sent back in its error: the token and the
    // header that carries it.
    let jira = Jira::start("401 Unauthorized", |request| {
        json!({"errorMessages": [format!(
            "bad credentials {SECRET} ({})", request.authorization
        )]})
        .to_string()
    });
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "45m"}),
    ));
    let failed = allow(&user, number);
    assert!(failed.contains("401"), "{failed}");
    assert_eq!(jira.received().len(), 1);

    let mut seen = vec![
        failed,
        user.call("approval.list", json!({})).unwrap().to_string(),
    ];
    seen.push(user.call("vault.list", json!({})).unwrap().to_string());
    let encoded = basic().strip_prefix("Basic ").unwrap().to_owned();
    for text in &seen {
        for forbidden in [SECRET, encoded.as_str()] {
            assert!(!text.contains(forbidden), "{forbidden} in {text}");
        }
    }
    for forbidden in [SECRET, encoded.as_str()] {
        assert_eq!(
            files_holding(&user.path("state"), forbidden),
            Vec::<PathBuf>::new(),
            "{forbidden} is stored"
        );
    }
}

/// A turn of `agent` in `session`, one row per model, from `started` for `lasted` milliseconds.
fn turn(session: &str, agent: &str, started: u64, lasted: u64, models: &[&str]) -> Vec<TurnUsage> {
    models
        .iter()
        .map(|model| TurnUsage {
            agent: agent.into(),
            title: None,
            session: session.into(),
            project: "app".into(),
            workspace: None,
            provider: "fake".into(),
            role: "root".into(),
            started_at: started,
            ended_at: started + lasted,
            stop_reason: "end_turn".into(),
            used: Used {
                model: Some((*model).into()),
                ..Used::default()
            },
        })
        .collect()
}

#[test]
fn time_spent_sums_the_turns_of_a_session() {
    let jira = Jira::accepting();
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    let (mcp, root) = root_in(&user);
    // The first turn of the fake agent is on record already: what is counted is what follows,
    // with a start far from it. Rows are the turns', one per model.
    eventually("the first turn's usage", || {
        !store(&user).turn_usage().unwrap().is_empty()
    });
    let minute = 60_000;
    let mut rows = Vec::new();
    // 20 minutes, spent on two models: counted once.
    rows.extend(turn(&root, &root, 1_000, 20 * minute, &["a", "b"]));
    // A sub-agent's 7 minutes, in parallel with the root's: its own turn, added.
    rows.extend(turn(&root, "child", 5_000, 7 * minute, &["a"]));
    // Another session's hour is not this one's.
    rows.extend(turn("elsewhere", "other", 1_000, 60 * minute, &["a"]));
    // A long silence between the turns is not work: the sum is of the turns.
    rows.extend(turn(&root, &root, 10_000_000_000, 3 * minute, &["a"]));
    // Written with the daemon running: the fake agent cannot be made to take its turns at these
    // times, and the rows are only what the tool sums.
    Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .record_turn_usage(&rows)
        .unwrap();

    let answer = mcp.call("tickets", json!({"action": "time_spent"}));
    assert!(!answer.error, "{}", answer.text);
    assert!(requests(&user).is_empty(), "a read was held");
    // 20 + 7 + 3 minutes, to the minute; neither the other session's hour nor the second model's
    // 20 minutes, nor the silence between the turns.
    assert!(answer.text.contains("30m"), "{}", answer.text);
    for wrong in ["1h", "50m", "37m", "57m"] {
        assert!(!answer.text.contains(wrong), "{wrong} in {}", answer.text);
    }
}

#[test]
fn an_entry_bound_elsewhere_is_refused_to_the_worklog_and_its_own_logs_time() {
    let jira = Jira::accepting();
    let user = user(&jira);
    jira_with(&user, Some("jira-time"));
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT,
            "for": "connection:almanac"}),
    )
    .unwrap();
    let (mcp, _) = root_in(&user);

    // Named by the worklog, but another feature's: refused, naming its use, before any card.
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h"}),
    );
    assert!(said.contains("the connection almanac"), "{said}");
    assert!(!said.contains(SECRET), "{said}");

    // Bound to this project's worklog, it logs time as an unbound entry does.
    user.call(
        "vault.set",
        json!({"name": "app-time", "secret": SECRET, "username": ACCOUNT,
            "for": "worklog:app"}),
    )
    .unwrap();
    jira_with_entry(&user, "app-time");
    let number = held(&mcp.call(
        "tickets",
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h"}),
    ));
    allow(&user, number);
    assert_eq!(jira.received()[0].authorization, basic());
}

/// The project's tracker naming the entry `entry` to log time with.
fn jira_with_entry(user: &User, entry: &str) {
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "jira", "site": SITE, "projects": ["ABC"],
                "worklog_token": entry}}),
    )
    .unwrap();
}

// Reading the worklogs of a day (#1010, `docs/tickets.md`, "Reading worklogs"): `tickets
// worklogs` asks Jira's REST API with the same vault entry as `log_time`, never held.

/// The account the token belongs to, as `GET /rest/api/3/myself` says it.
const MINE: &str = "5b10a2844c20165700ede21a";
/// Somebody else's account on the same site.
const OTHER: &str = "712020:0c1d9a6e-7a4e-4f3a-9b1e-2d6f5c8a1b34";
/// The time zone of the daemon in these tests: UTC+1 in November.
const ZONE: &str = "Europe/Paris";

/// A worklog as Jira's issue worklog endpoint gives it.
fn logged(account: &str, name: &str, started: &str, seconds: u64, comment: Option<&str>) -> Value {
    let mut worklog = json!({
        "id": format!("{}", 10_000 + seconds),
        "author": {"accountId": account, "displayName": name},
        "started": started,
        "timeSpentSeconds": seconds,
        "timeSpent": match (seconds / 3600, seconds % 3600 / 60) {
            (0, m) => format!("{m}m"),
            (h, 0) => format!("{h}h"),
            (h, m) => format!("{h}h {m}m"),
        },
    });
    if let Some(text) = comment {
        worklog["comment"] = json!({"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": text}]}]});
    }
    worklog
}

fn mine(started: &str, seconds: u64, comment: Option<&str>) -> Value {
    logged(MINE, "Jane Doe", started, seconds, comment)
}

/// A worklog's `started` as an instant, in milliseconds.
fn instant_of(started: &str) -> i64 {
    let (head, offset) = started.split_at(started.len() - 5);
    format!("{head}{}:{}", &offset[..3], &offset[3..])
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond()
}

/// The day of an instant in UTC, which is the zone this fake Jira keeps its worklog dates in.
fn utc_day(started: &str) -> String {
    jiff::Timestamp::from_millisecond(instant_of(started))
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
        .to_string()
}

/// A Jira that knows who the token belongs to and which work items carry which worklogs: the
/// search finds the items, as Jira does, by the days of the JQL in UTC (an item with no worklog
/// is always found; `cut` says it stopped at its bound), the worklog endpoint pages by `startAt`
/// and `maxResults` with at most `page` worklogs at a time, and keeps to `startedAfter` and
/// `startedBefore` when it is given them.
fn ledger(items: Vec<(&'static str, &'static str, Vec<Value>)>, cut: bool, page: usize) -> Jira {
    ledger_over(Arc::new(Mutex::new(items)), cut, page)
}

/// The work items of a `ledger` a test may change while the site runs.
type Items = Arc<Mutex<Vec<(&'static str, &'static str, Vec<Value>)>>>;

/// A `ledger` over items that are read at each request: a test changes them between two.
fn ledger_over(state: Items, cut: bool, page: usize) -> Jira {
    Jira::reply(move |request| {
        let items = state.lock().unwrap().clone();
        let path = request.path.split('?').next().unwrap_or_default();
        // One worklog of an item (`/issue/<KEY>/worklog/<id>`): read, changed or deleted. A
        // worklog that is not on that item is a 404, as it is on the site.
        for (key, _, worklogs) in &items {
            let Some(id) = path.strip_prefix(&format!("/rest/api/3/issue/{key}/worklog/")) else {
                continue;
            };
            let Some(worklog) = worklogs.iter().find(|worklog| worklog["id"] == id) else {
                return (
                    "404 Not Found",
                    json!({"errorMessages": ["Worklog does not exist"]}).to_string(),
                );
            };
            return match request.method.as_str() {
                "DELETE" => ("204 No Content", String::new()),
                _ => ("200 OK", worklog.to_string()),
            };
        }
        let said = |body: String| ("200 OK", body);
        let query = request.path.split_once('?').map_or("", |(_, query)| query);
        let number = |name: &str| -> usize {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
                .and_then(|value| value.parse().ok())
                .unwrap_or(0)
        };
        if path == "/rest/api/3/myself" {
            return said(
                json!({"accountId": MINE, "displayName": "Jane Doe", "emailAddress": ACCOUNT})
                    .to_string(),
            );
        }
        if path == "/rest/api/3/search/jql" {
            let jql = unquote(query);
            let day = |after: &str| {
                jql.split_once(after)
                    .map(|(_, rest)| rest.chars().take(10).collect::<String>())
            };
            let (first, last) = (day("worklogDate >= \""), day("worklogDate <= \""));
            let found: Vec<Value> = items
                .iter()
                .filter(|(_, _, worklogs)| {
                    worklogs.is_empty()
                        || worklogs.iter().any(|worklog| {
                            let day = utc_day(worklog["started"].as_str().unwrap());
                            first.as_ref().is_none_or(|first| day >= *first)
                                && last.as_ref().is_none_or(|last| day <= *last)
                        })
                })
                .map(|(key, title, _)| json!({"key": key, "fields": {"summary": title}}))
                .collect();
            let mut answer = json!({"issues": found, "isLast": !cut});
            if cut {
                answer["nextPageToken"] = "more".into();
            }
            return said(answer.to_string());
        }
        for (key, title, worklogs) in &items {
            if path == format!("/rest/api/3/issue/{key}") {
                return said(json!({"key": key, "fields": {"summary": title}}).to_string());
            }
            if path == format!("/rest/api/3/issue/{key}/worklog") {
                let start = number("startAt");
                let size = number("maxResults").clamp(1, page);
                let bound = |name: &str| {
                    query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
                        .and_then(|value| value.parse::<i64>().ok())
                };
                let (after, before) = (bound("startedAfter"), bound("startedBefore"));
                let kept: Vec<&Value> = worklogs
                    .iter()
                    .filter(|worklog| {
                        let at = instant_of(worklog["started"].as_str().unwrap());
                        after.is_none_or(|after| at > after)
                            && before.is_none_or(|before| at < before)
                    })
                    .collect();
                let shown: Vec<&&Value> = kept.iter().skip(start).take(size).collect();
                return said(
                    json!({"startAt": start, "maxResults": size,
                        "total": kept.len(), "worklogs": shown})
                    .to_string(),
                );
            }
        }
        said(json!({"errorMessages": ["Issue does not exist"]}).to_string())
    })
}

/// A query string as the daemon sent it, decoded: `+` and `%XX` as the text they stand for.
fn unquote(query: &str) -> String {
    let bytes = query.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[at], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A user whose Jira is `jira`, in the time zone of the tests.
fn zoned(jira: &Jira) -> User {
    user(jira).with_env("TZ", ZONE)
}

fn worklogs(mcp: &Mcp, args: Value) -> String {
    let mut call = json!({"action": "worklogs"});
    call.as_object_mut()
        .unwrap()
        .extend(args.as_object().unwrap().clone());
    let answer = mcp.call("tickets", call);
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

/// The requests the fake Jira got whose path starts with `prefix`.
fn asked(jira: &Jira, prefix: &str) -> Vec<Received> {
    jira.received()
        .into_iter()
        .filter(|request| request.path.starts_with(prefix))
        .collect()
}

#[test]
fn worklogs_of_a_day_are_the_token_owner_s_with_their_durations_and_total() {
    let jira = ledger(
        vec![
            (
                "ABC-1",
                "Fix the login redirect",
                vec![mine(
                    "2026-11-06T09:12:00.000+0100",
                    5_400,
                    Some("Pairing on the redirect"),
                )],
            ),
            (
                "ABC-2",
                "Add the natures",
                vec![mine("2026-11-06T14:00:00.000+0100", 2_700, None)],
            ),
        ],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06"}));

    // Never held: nobody was asked.
    assert!(requests(&user).is_empty(), "held for the user");
    // Whose time it is, and in which zone the days are.
    assert!(text.contains("your account (Jane Doe)"), "{text}");
    assert!(text.contains(ZONE), "{text}");
    for line in [
        "ABC-1 Fix the login redirect — 1h30 (started 09:12, worklog 15400) \"Pairing on the redirect\"",
        "ABC-2 Add the natures — 45m (started 14:00, worklog 12700)",
        "Day total: 2h15",
        "Range total: 2h15",
    ] {
        assert!(text.contains(line), "{line:?} is not in:\n{text}");
    }
    // The search is the token's own, over the day, in the tracker's projects; the account was
    // asked of Jira with the entry's credentials, and nothing carries the secret out.
    let search = asked(&jira, "/rest/api/3/search/jql");
    assert_eq!(search.len(), 1, "{:?}", jira.received());
    let decoded = unquote(&search[0].path);
    for part in [
        // The day asked, a day wider on each side.
        "worklogDate >= \"2026-11-05\"",
        "worklogDate <= \"2026-11-07\"",
        &format!("worklogAuthor = \"{MINE}\""),
        "project in (\"ABC\")",
    ] {
        assert!(decoded.contains(part), "{part} is not in {decoded}");
    }
    assert_eq!(asked(&jira, "/rest/api/3/myself").len(), 1);
    for request in jira.received() {
        assert_eq!(request.method, "GET");
        assert_eq!(request.authorization, basic(), "{}", request.path);
    }
    assert!(!text.contains(SECRET), "{text}");

    // The account is kept: the second call does not ask again.
    worklogs(&mcp, json!({"from": "2026-11-06"}));
    assert_eq!(asked(&jira, "/rest/api/3/myself").len(), 1);
    assert!(files_holding(&user.path(""), SECRET).is_empty());
}

#[test]
fn worklogs_of_others_on_the_same_items_are_left_out() {
    let jira = ledger(
        vec![(
            "ABC-1",
            "Fix the login redirect",
            vec![
                mine("2026-11-06T09:00:00.000+0100", 3_600, Some("My part")),
                logged(
                    OTHER,
                    "Sam Roe",
                    "2026-11-06T10:00:00.000+0100",
                    7_200,
                    Some("Their part"),
                ),
            ],
        )],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06"}));

    assert!(text.contains("My part"), "{text}");
    assert!(!text.contains("Their part"), "{text}");
    assert!(!text.contains("Sam Roe"), "{text}");
    assert!(text.contains("Day total: 1h"), "{text}");
    assert!(!text.contains("3h"), "{text}");
}

#[test]
fn a_worklog_started_the_day_before_in_the_user_s_zone_is_left_out() {
    let jira = ledger(
        vec![(
            "ABC-1",
            "Fix the login redirect",
            vec![
                // 22:30 UTC on the 5th: the 5th in Paris.
                mine(
                    "2026-11-05T23:30:00.000+0100",
                    3_600,
                    Some("Late the day before"),
                ),
                // 23:30 UTC on the 5th, which is the 6th in Paris: kept, though not in UTC.
                mine(
                    "2026-11-06T00:30:00.000+0100",
                    1_800,
                    Some("Just after midnight"),
                ),
                // 04:30 UTC on the 7th, 05:30 on the 7th in Paris.
                mine(
                    "2026-11-06T23:30:00.000-0500",
                    900,
                    Some("Late in New York"),
                ),
            ],
        )],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06"}));

    assert!(text.contains("Just after midnight"), "{text}");
    assert!(text.contains("(started 00:30, worklog 11800)"), "{text}");
    assert!(!text.contains("Late the day before"), "{text}");
    assert!(!text.contains("Late in New York"), "{text}");
    assert!(text.contains("Range total: 30m"), "{text}");
}

#[test]
fn worklogs_over_a_range_are_totalled_per_day() {
    let jira = ledger(
        vec![
            (
                "ABC-1",
                "Fix the login redirect",
                vec![
                    mine("2026-11-05T10:00:00.000+0100", 7_200, None),
                    mine("2026-11-06T09:00:00.000+0100", 3_600, None),
                ],
            ),
            (
                "ABC-2",
                "Add the natures",
                vec![mine("2026-11-06T13:00:00.000+0100", 1_800, None)],
            ),
        ],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-05", "to": "2026-11-06"}));

    let (first, second) = (text.find("2026-11-05\n"), text.find("2026-11-06\n"));
    assert!(first.is_some() && first < second, "{text}");
    let totals: Vec<&str> = text.lines().filter(|l| l.contains("total")).collect();
    assert_eq!(
        totals,
        ["  Day total: 2h", "  Day total: 1h30", "Range total: 3h30"],
        "{text}"
    );
    let decoded = unquote(&asked(&jira, "/rest/api/3/search/jql")[0].path);
    assert!(
        decoded.contains("worklogDate >= \"2026-11-04\""),
        "{decoded}"
    );
    assert!(
        decoded.contains("worklogDate <= \"2026-11-07\""),
        "{decoded}"
    );
}

#[test]
fn worklogs_without_the_vault_entry_say_how_to_add_it() {
    let jira = ledger(Vec::new(), false, 100);
    let user = zoned(&jira);
    jira_with(&user, None);
    entry(&user);
    let (mcp, _) = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "worklogs", "from": "2026-11-06"}),
    );
    for how in ["worklog_token", "[tracker]", "vault", "Settings"] {
        assert!(said.contains(how), "{how} is not said: {said}");
    }

    // An entry named but not in the vault is refused too, saying which.
    jira_with_entry(&user, "nowhere");
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "worklogs", "from": "2026-11-06"}),
    );
    assert!(said.contains("nowhere"), "{said}");
    assert!(jira.received().is_empty(), "{:?}", jira.received());
}

#[test]
fn a_search_on_worklog_date_without_an_author_says_it_matched_anyone() {
    let jira = ledger(Vec::new(), false, 100);
    let user = zoned(&jira);
    user.acli_script(json!({"items": {}}));
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let anyone = mcp.call(
        "tickets",
        json!({"action": "search", "jql": "worklogDate = \"2026-11-06\""}),
    );
    assert!(!anyone.error, "{}", anyone.text);
    assert!(anyone.text.contains("anyone's worklogs"), "{}", anyone.text);
    assert!(anyone.text.contains("worklogs"), "{}", anyone.text);

    // An author that is excluded, not asked for, restricts nothing to the user's.
    for jql in [
        "worklogDate = \"2026-11-06\" AND worklogAuthor != currentUser()",
        "worklogDate = \"2026-11-06\" AND worklogAuthor not in (currentUser())",
    ] {
        let found = mcp.call("tickets", json!({"action": "search", "jql": jql}));
        assert!(!found.error, "{}", found.text);
        assert!(found.text.contains("anyone's"), "{jql}: {}", found.text);
    }

    // With an author, or with no worklog date, it says nothing of the kind.
    for jql in [
        "worklogDate = \"2026-11-06\" AND worklogAuthor = currentUser()",
        "worklogDate = \"2026-11-06\" AND worklogAuthor in (currentUser())",
        "worklogDate = \"2026-11-06\" AND worklogAuthor IN (currentUser())",
        "priority = High",
    ] {
        let found = mcp.call("tickets", json!({"action": "search", "jql": jql}));
        assert!(!found.error, "{}", found.text);
        assert!(!found.text.contains("anyone's"), "{jql}: {}", found.text);
    }
}

#[test]
fn worklogs_are_refused_on_github() {
    let jira = ledger(Vec::new(), false, 100);
    let user = zoned(&jira);
    fixture(&user, json!({"kind": "github", "repository": "."}));
    let (mcp, _) = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "worklogs", "from": "2026-11-06"}),
    );
    assert!(said.contains("GitHub has no worklogs"), "{said}");
    assert!(jira.received().is_empty());
}

#[test]
fn a_changed_entry_is_asked_for_its_account_again() {
    let jira = ledger(
        vec![(
            "ABC-1",
            "Fix the login redirect",
            vec![mine("2026-11-06T09:00:00.000+0100", 3_600, None)],
        )],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);
    worklogs(&mcp, json!({"from": "2026-11-06"}));
    assert_eq!(asked(&jira, "/rest/api/3/myself").len(), 1);

    // The user sets the entry again, with another token: another account may be behind it.
    std::thread::sleep(Duration::from_millis(5));
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": "an0ther-t0ken", "username": ACCOUNT,
            "project": "app"}),
    )
    .unwrap();
    worklogs(&mcp, json!({"from": "2026-11-06"}));

    assert_eq!(asked(&jira, "/rest/api/3/myself").len(), 2);
}

#[test]
fn a_work_item_with_more_worklogs_than_a_page_is_read_in_full() {
    let mut all: Vec<Value> = (0..120)
        .map(|n| {
            logged(
                OTHER,
                "Sam Roe",
                "2026-11-06T08:00:00.000+0100",
                60 + n,
                None,
            )
        })
        .collect();
    all.push(mine(
        "2026-11-06T09:00:00.000+0100",
        3_600,
        Some("On the last page"),
    ));
    // The site hands out 50 at a time whatever is asked.
    let jira = ledger(vec![("ABC-1", "Fix the login redirect", all)], false, 50);
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06"}));

    assert!(text.contains("On the last page"), "{text}");
    assert!(text.contains("Range total: 1h"), "{text}");
}

#[test]
fn a_search_cut_at_its_bound_says_so() {
    let jira = ledger(
        vec![(
            "ABC-1",
            "Fix the login redirect",
            vec![mine("2026-11-06T09:00:00.000+0100", 3_600, None)],
        )],
        true,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06"}));

    assert!(text.contains("cut"), "{text}");
    assert!(text.contains("narrow"), "{text}");
}

#[test]
fn worklogs_of_one_ticket_are_read_without_a_search() {
    let jira = ledger(
        vec![
            (
                "ABC-1",
                "Fix the login redirect",
                vec![mine("2026-11-06T09:00:00.000+0100", 3_600, None)],
            ),
            (
                "ABC-2",
                "Add the natures",
                vec![mine("2026-11-06T13:00:00.000+0100", 1_800, None)],
            ),
        ],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06", "ticket": "abc-2"}));

    assert!(text.contains("ABC-2 Add the natures — 30m"), "{text}");
    assert!(!text.contains("ABC-1"), "{text}");
    assert!(asked(&jira, "/rest/api/3/search/jql").is_empty());
}

#[test]
fn the_worklogs_of_another_account_are_asked_by_its_id() {
    let jira = ledger(
        vec![(
            "ABC-1",
            "Fix the login redirect",
            vec![
                mine("2026-11-06T09:00:00.000+0100", 3_600, Some("Jane's")),
                logged(
                    OTHER,
                    "Sam Roe",
                    "2026-11-06T10:00:00.000+0100",
                    7_200,
                    Some("Sam's"),
                ),
            ],
        )],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06", "author": OTHER}));

    assert!(text.contains("Sam's"), "{text}");
    assert!(!text.contains("Jane's"), "{text}");
    assert!(text.contains("Sam Roe"), "{text}");
    assert!(!text.contains("your account"), "{text}");
    assert!(asked(&jira, "/rest/api/3/myself").is_empty());
}

#[test]
fn a_date_that_is_not_a_day_is_refused() {
    let jira = ledger(Vec::new(), false, 100);
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    for args in [
        json!({"action": "worklogs"}),
        json!({"action": "worklogs", "from": "last friday"}),
        json!({"action": "worklogs", "from": "2026-11-06", "to": "2026-11-05"}),
    ] {
        let said = refused_at_once(&user, &mcp, args);
        assert!(said.contains("from"), "{said}");
    }
    assert!(jira.received().is_empty());
}

#[test]
fn a_worklog_jira_files_under_the_day_before_is_still_found() {
    // 00:30 on the 6th in Paris is 23:30 on the 5th in UTC, where this Jira files it: a search
    // for the 6th alone would not name its ticket.
    let jira = ledger(
        vec![
            (
                "ABC-1",
                "Fix the login redirect",
                vec![mine(
                    "2026-11-06T00:30:00.000+0100",
                    1_800,
                    Some("Just after midnight"),
                )],
            ),
            (
                "ABC-2",
                "Tidy the footer",
                vec![mine(
                    "2026-11-06T10:00:00.000+0100",
                    3_600,
                    Some("Mid-morning"),
                )],
            ),
        ],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06"}));

    assert!(text.contains("Just after midnight"), "{text}");
    assert!(text.contains("Mid-morning"), "{text}");
    assert!(text.contains("Range total: 1h30"), "{text}");
    // The search is wider than the days asked by one day each side; the exact cut is local.
    let search = unquote(&asked(&jira, "/rest/api/3/search/jql")[0].path);
    assert!(search.contains("worklogDate >= \"2026-11-05\""), "{search}");
    assert!(search.contains("worklogDate <= \"2026-11-07\""), "{search}");
}

#[test]
fn the_worklogs_of_an_item_are_asked_for_the_range_with_a_day_of_margin() {
    let jira = ledger(
        vec![(
            "ABC-1",
            "Fix the login redirect",
            vec![
                mine("2026-11-06T09:00:00.000+0100", 3_600, Some("In the range")),
                // Months before: the site is asked not to send it.
                mine("2026-08-01T09:00:00.000+0200", 7_200, Some("Long before")),
            ],
        )],
        false,
        100,
    );
    let user = zoned(&jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06", "to": "2026-11-07"}));

    assert!(text.contains("In the range"), "{text}");
    assert!(text.contains("Range total: 1h"), "{text}");
    // In Paris the 6th begins at 23:00 UTC on the 5th and the 8th at 23:00 UTC on the 7th: a day
    // earlier and a day later.
    let ms = |at: &str| at.parse::<jiff::Timestamp>().unwrap().as_millisecond();
    let query = asked(&jira, "/rest/api/3/issue/ABC-1/worklog")[0]
        .path
        .clone();
    for (name, at) in [
        ("startedAfter", "2026-11-04T23:00:00Z"),
        ("startedBefore", "2026-11-08T23:00:00Z"),
    ] {
        let wanted = format!("{name}={}", ms(at));
        assert!(
            query.split('&').any(|pair| pair.ends_with(&wanted)),
            "{wanted} is not in {query}"
        );
    }
}

#[test]
fn an_author_given_as_a_name_is_found_among_the_people() {
    let jira = ledger(
        vec![(
            "ABC-1",
            "Fix the login redirect",
            vec![
                mine("2026-11-06T09:00:00.000+0100", 3_600, Some("Jane's")),
                logged(
                    OTHER,
                    "Sam Roe",
                    "2026-11-06T10:00:00.000+0100",
                    7_200,
                    Some("Sam's"),
                ),
            ],
        )],
        false,
        100,
    );
    let user = zoned(&jira);
    user.acli_script(json!({"search": [{"key": "ABC-1", "fields": {
        "assignee": {"accountId": OTHER, "emailAddress": "sam@example.com",
            "displayName": "Sam Roe"}}}]}));
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);

    let text = worklogs(&mcp, json!({"from": "2026-11-06", "author": "Sam Roe"}));

    assert!(text.contains("Sam's"), "{text}");
    assert!(!text.contains("Jane's"), "{text}");
    assert!(text.contains("Sam Roe"), "{text}");
    assert!(!text.contains("your account"), "{text}");
    assert!(asked(&jira, "/rest/api/3/myself").is_empty());
    let search = unquote(&asked(&jira, "/rest/api/3/search/jql")[0].path);
    assert!(
        search.contains(&format!("worklogAuthor = \"{OTHER}\"")),
        "{search}"
    );
}

#[test]
fn a_refused_worklogs_offers_to_set_up_time_logging() {
    let jira = ledger(Vec::new(), false, 100);
    let user = zoned(&jira);
    jira_with(&user, None);
    entry(&user);
    let (mcp, agent) = root_in(&user);

    refused_at_once(
        &user,
        &mcp,
        json!({"action": "worklogs", "from": "2026-11-06"}),
    );

    eventually("the call's event", || {
        store(&user)
            .agent_events(&agent, 0, 200)
            .unwrap()
            .iter()
            .any(|e| e.name == "agent.tool_served" && e.data["setup"] == "time_logging")
    });
}

// Correcting a worklog already logged (#1092, `docs/tickets.md`, "Logging time"): `worklogs` gives
// each worklog's id, `worklog_edit` and `worklog_delete` change or delete one of the token's own
// account, held for the user like `log_time`.

/// A site with the account's hour on ABC-1 (worklog 13600), somebody else's half hour on it
/// (11800), and the account's 45 minutes on ABC-2 (12700).
fn ledger_to_correct() -> Jira {
    ledger_to_change().0
}

/// `ledger_to_correct`, with its items for a test to change while the site runs.
fn ledger_to_change() -> (Jira, Items) {
    let items: Items = Arc::new(Mutex::new(vec![
        (
            "ABC-1",
            "Fix the login redirect",
            vec![
                mine(
                    "2026-11-06T09:00:00.000+0100",
                    3_600,
                    Some("Pairing on the redirect"),
                ),
                logged(
                    OTHER,
                    "John Roe",
                    "2026-11-06T10:00:00.000+0100",
                    1_800,
                    None,
                ),
            ],
        ),
        (
            "ABC-2",
            "Add the natures",
            vec![mine("2026-11-06T14:00:00.000+0100", 2_700, None)],
        ),
    ]));
    (ledger_over(items.clone(), false, 100), items)
}

/// A user over `jira` with the entry set, and the client of the root agent's tools.
fn ready(jira: &Jira) -> (User, Mcp) {
    let user = zoned(jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    let (mcp, _) = root_in(&user);
    (user, mcp)
}

/// A change of a worklog: a new duration for an edit, nothing more for a delete.
fn changing(action: &str, ticket: &str, worklog: &str) -> Value {
    let mut args = json!({"action": action, "ticket": ticket, "worklog": worklog});
    if action == "worklog_edit" {
        args["duration"] = "1h".into();
    }
    args
}

/// The writes the site received.
fn written(jira: &Jira) -> Vec<Received> {
    jira.received()
        .into_iter()
        .filter(|request| request.method != "GET")
        .collect()
}

/// The card of request `number`, as the user reads it.
fn card_of(user: &User, number: u64) -> String {
    requests(user)
        .iter()
        .find(|r| r["number"] == number)
        .unwrap()
        .to_string()
}

#[test]
fn worklogs_gives_each_worklog_s_id() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    let text = worklogs(&mcp, json!({"from": "2026-11-06"}));

    assert!(
        text.contains(
            "ABC-1 Fix the login redirect — 1h (started 09:00, worklog 13600) \
             \"Pairing on the redirect\""
        ),
        "{text}"
    );
    assert!(
        text.contains("ABC-2 Add the natures — 45m (started 14:00, worklog 12700)"),
        "{text}"
    );
    assert!(requests(&user).is_empty());
}

#[test]
fn worklog_edit_changes_the_duration_of_the_account_s_worklog() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "worklog_edit", "ticket": "abc-1", "worklog": "13600",
            "duration": "2h"}),
    ));
    assert!(
        written(&jira).is_empty(),
        "written before the user allowed it"
    );

    let done = allow(&user, number);
    assert!(done.contains("13600"), "{done}");
    assert!(!done.contains(SECRET), "{done}");
    let writes = written(&jira);
    assert_eq!(writes.len(), 1, "{:?}", jira.received());
    assert_eq!(writes[0].method, "PUT");
    assert_eq!(writes[0].path, "/rest/api/3/issue/ABC-1/worklog/13600");
    assert_eq!(writes[0].authorization, basic());
    // What changes only: the start and the comment are left as they are.
    assert_eq!(writes[0].body, json!({"timeSpent": "2h"}));
}

#[test]
fn worklog_edit_changes_the_start_and_the_comment() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "worklog_edit", "ticket": "ABC-1", "worklog": "13600",
            "started": "2026-11-06T11:00:00+02:00", "comment": "Reviewed the fix"}),
    ));
    allow(&user, number);

    let writes = written(&jira);
    assert_eq!(writes.len(), 1, "{:?}", jira.received());
    assert_eq!(writes[0].method, "PUT");
    assert_eq!(writes[0].body["started"], "2026-11-06T09:00:00.000+0000");
    assert_eq!(writes[0].body["comment"]["type"], "doc");
    assert_eq!(text_of(&writes[0].body["comment"]), "Reviewed the fix");
    assert!(
        writes[0].body.get("timeSpent").is_none(),
        "{}",
        writes[0].body
    );
}

#[test]
fn worklog_edit_without_a_change_is_refused() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "worklog_edit", "ticket": "ABC-1", "worklog": "13600"}),
    );
    assert!(said.contains("duration, started or comment"), "{said}");
    // A duration that is not Jira's form is refused the same way.
    refused_at_once(
        &user,
        &mcp,
        json!({"action": "worklog_edit", "ticket": "ABC-1", "worklog": "13600",
            "duration": "soon"}),
    );
    assert!(written(&jira).is_empty());
}

#[test]
fn worklog_delete_removes_the_account_s_worklog() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "worklog_delete", "ticket": "abc-1", "worklog": "13600"}),
    ));
    assert!(
        written(&jira).is_empty(),
        "deleted before the user allowed it"
    );

    let done = allow(&user, number);
    assert!(done.contains("13600"), "{done}");
    let writes = written(&jira);
    assert_eq!(writes.len(), 1, "{:?}", jira.received());
    assert_eq!(writes[0].method, "DELETE");
    // Jira's default estimate adjustment: nothing is asked of it.
    assert_eq!(writes[0].path, "/rest/api/3/issue/ABC-1/worklog/13600");
    assert_eq!(writes[0].authorization, basic());
}

#[test]
fn a_worklog_of_another_author_is_refused_before_anything_is_asked() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    for action in ["worklog_edit", "worklog_delete"] {
        let said = refused_at_once(&user, &mcp, changing(action, "ABC-1", "11800"));
        assert!(
            said.contains("worklog 11800 is John Roe's, not your account's"),
            "{said}"
        );
    }
    assert!(written(&jira).is_empty());
}

#[test]
fn a_worklog_not_on_that_work_item_is_refused() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    // 12700 is the account's, on ABC-2; 99999 is nobody's.
    for worklog in ["12700", "99999"] {
        for action in ["worklog_edit", "worklog_delete"] {
            let said = refused_at_once(&user, &mcp, changing(action, "ABC-1", worklog));
            assert!(
                said.contains(&format!("ABC-1 has no worklog {worklog}")),
                "{said}"
            );
        }
    }
    assert!(written(&jira).is_empty());
}

#[test]
fn the_card_shows_the_worklog_as_it_is_and_what_it_becomes() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    let edit = held(&mcp.call(
        "tickets",
        json!({"action": "worklog_edit", "ticket": "ABC-1", "worklog": "13600",
            "duration": "2h", "comment": "Reviewed the fix"}),
    ));
    let card = card_of(&user, edit);
    for said in [
        "ABC-1",
        "13600",
        // As it is.
        "It is now: 1h, started 2026-11-06T08:00:00Z, \\\"Pairing on the redirect\\\"",
        // What it becomes: what is not changed stays as it is.
        "It becomes: 2h, started 2026-11-06T08:00:00Z, \\\"Reviewed the fix\\\"",
    ] {
        assert!(card.contains(said), "{said} is not on the card: {card}");
    }

    let delete = held(&mcp.call(
        "tickets",
        json!({"action": "worklog_delete", "ticket": "ABC-2", "worklog": "12700"}),
    ));
    let card = card_of(&user, delete);
    for said in [
        "ABC-2",
        "12700",
        "It deletes the worklog",
        "45m, started 2026-11-06T13:00:00Z",
    ] {
        assert!(card.contains(said), "{said} is not on the card: {card}");
    }
    assert!(
        written(&jira).is_empty(),
        "written before the user allowed it"
    );
}

/// What the user is told when what they allowed is not done: the answer the agent is handed.
fn not_done(jira: &Jira, user: &User, number: u64) -> String {
    let told = allow(user, number);
    assert!(
        written(jira).is_empty(),
        "written though the worklog changed: {:?}",
        written(jira)
    );
    told
}

#[test]
fn a_worklog_changed_between_the_card_and_the_allow_is_not_written() {
    for action in ["worklog_edit", "worklog_delete"] {
        let (jira, items) = ledger_to_change();
        let (user, mcp) = ready(&jira);
        let number = held(&mcp.call("tickets", changing(action, "ABC-1", "13600")));

        // Somebody changes its time in Jira after the card was shown.
        items.lock().unwrap()[0].2[0]["timeSpentSeconds"] = 1_800.into();
        items.lock().unwrap()[0].2[0]["timeSpent"] = "30m".into();

        let told = not_done(&jira, &user, number);
        assert!(told.contains("changed since"), "{action}: {told}");
        assert!(told.contains("30m"), "{action}: {told}");
    }
}

#[test]
fn a_worklog_deleted_between_the_card_and_the_allow_is_not_written() {
    for action in ["worklog_edit", "worklog_delete"] {
        let (jira, items) = ledger_to_change();
        let (user, mcp) = ready(&jira);
        let number = held(&mcp.call("tickets", changing(action, "ABC-1", "13600")));

        items.lock().unwrap()[0].2.remove(0);

        let told = not_done(&jira, &user, number);
        assert!(
            told.contains("ABC-1 has no worklog 13600"),
            "{action}: {told}"
        );
    }
}

#[test]
fn a_worklog_whose_author_changed_between_the_card_and_the_allow_is_not_written() {
    for action in ["worklog_edit", "worklog_delete"] {
        let (jira, items) = ledger_to_change();
        let (user, mcp) = ready(&jira);
        let number = held(&mcp.call("tickets", changing(action, "ABC-1", "13600")));

        items.lock().unwrap()[0].2[0]["author"] =
            json!({"accountId": OTHER, "displayName": "John Roe"});

        let told = not_done(&jira, &user, number);
        assert!(
            told.contains("John Roe's, not your account's"),
            "{action}: {told}"
        );
    }
}

#[test]
fn worklog_edit_and_delete_are_refused_on_github() {
    let jira = Jira::accepting();
    let user = user(&jira);
    fixture(&user, json!({"kind": "github", "repository": "."}));
    let (mcp, _) = root_in(&user);

    for action in ["worklog_edit", "worklog_delete"] {
        let said = refused_at_once(&user, &mcp, changing(action, "#1", "13600"));
        assert!(said.contains("GitHub has no worklogs"), "{said}");
    }
    assert!(jira.received().is_empty());
}

#[test]
fn worklog_edit_and_delete_without_an_entry_say_how_to_add_one() {
    let jira = ledger_to_correct();
    let user = zoned(&jira);
    jira_with(&user, None);
    let (mcp, _) = root_in(&user);

    for action in ["worklog_edit", "worklog_delete"] {
        let said = refused_at_once(&user, &mcp, changing(action, "ABC-1", "13600"));
        assert!(said.contains("worklog_token"), "{said}");
    }
    assert!(jira.received().is_empty());
}

#[test]
fn a_worklog_edit_and_a_delete_need_their_worklog() {
    let jira = ledger_to_correct();
    let (user, mcp) = ready(&jira);

    for args in [
        json!({"action": "worklog_edit", "ticket": "ABC-1", "duration": "1h"}),
        json!({"action": "worklog_delete", "ticket": "ABC-1"}),
        json!({"action": "worklog_delete", "worklog": "13600"}),
    ] {
        let said = refused_at_once(&user, &mcp, args);
        assert!(said.contains("needs"), "{said}");
    }
}

#[test]
fn the_token_never_appears_in_what_is_stored_of_a_worklog_edit() {
    // A site that reads fine and fails every write, echoing the token and the header back.
    let jira = Jira::reply(|request| {
        let path = request.path.split('?').next().unwrap_or_default();
        match request.method.as_str() {
            "GET" if path == "/rest/api/3/myself" => (
                "200 OK",
                json!({"accountId": MINE, "displayName": "Jane Doe"}).to_string(),
            ),
            "GET" => (
                "200 OK",
                mine("2026-11-06T09:00:00.000+0100", 3_600, None).to_string(),
            ),
            _ => (
                "401 Unauthorized",
                json!({"errorMessages": [format!(
                    "bad credentials {SECRET} ({})", request.authorization
                )]})
                .to_string(),
            ),
        }
    });
    let (user, mcp) = ready(&jira);

    let mut seen = Vec::new();
    for args in [
        json!({"action": "worklog_edit", "ticket": "ABC-1", "worklog": "13600",
            "duration": "2h"}),
        json!({"action": "worklog_delete", "ticket": "ABC-1", "worklog": "13600"}),
    ] {
        let number = held(&mcp.call("tickets", args));
        seen.push(card_of(&user, number));
        let failed = allow(&user, number);
        assert!(failed.contains("401"), "{failed}");
        seen.push(failed);
    }
    seen.push(user.call("vault.list", json!({})).unwrap().to_string());
    let encoded = basic().strip_prefix("Basic ").unwrap().to_owned();
    for text in &seen {
        for forbidden in [SECRET, encoded.as_str()] {
            assert!(!text.contains(forbidden), "{forbidden} in {text}");
        }
    }
    for forbidden in [SECRET, encoded.as_str()] {
        assert_eq!(
            files_holding(&user.path("state"), forbidden),
            Vec::<PathBuf>::new(),
            "{forbidden} is stored"
        );
    }
}
