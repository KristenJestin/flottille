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
                let said = body(&request);
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
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
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
    store(&user).record_turn_usage(&rows).unwrap();

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
