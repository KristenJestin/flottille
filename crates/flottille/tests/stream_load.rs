//! A session that streams without pause and the daemon's other clients (#903): while it streams,
//! the daemon still answers them, and each piece reaches the store as it comes, not after a
//! backlog. The numbers are printed, to be read under a known load (`docs/reference/gpui.md`,
//! "Testing without a screen": note the load), and the bounds leave room for a loaded machine.
//!
//! Both tests are ignored: unloaded they pass with or without the batching, so they catch a
//! regression only with a busy disk. Run them by hand with one (`docs/reference/daemon.md`, "A
//! stream and a busy disk"), the way the measures of #903 were:
//!
//! ```text
//! ( while :; do dd if=/dev/zero of=/tmp/busy bs=1M count=1024 conv=fsync 2>/dev/null; done ) &
//! cargo test -p flottille --test stream_load -- --ignored --nocapture
//! kill %1; rm /tmp/busy
//! ```

mod common;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// The stream: a piece every 10 ms for 15 s at the least, as the window's test of it (#881).
const PIECES: usize = 1500;

/// How long 95 requests in 100 may wait while the session streams: answered in 12 to 19 ms
/// unloaded (#903), and a window's snapshot asks a dozen of them. Not every request: on a busy
/// disk one commit's `fsync` may wait seconds behind the disk's other writer, and the daemon's
/// thread with it, which writing the pieces together does not change; a backlog of commits does
/// fail it (measured: 2.4 s at the 95th before they were written together, 0.18 s after).
const ANSWERED_WITHIN: Duration = Duration::from_secs(1);

/// How long 95 pieces in 100 may take from the agent to the store: past it, the daemon is behind.
/// Not every piece, for the reason of `ANSWERED_WITHIN`.
const STORED_WITHIN_MS: u64 = 1000;

/// A user with the project `app` and a session in it that streams the wall clock, piece by piece:
/// the session's id.
fn streaming(user: &User) -> String {
    let pieces: Vec<Value> = (0..PIECES)
        .flat_map(|_| [json!({"say_clock": true}), json!({"wait": 10})])
        .collect();
    user.fake_script(json!({"turns": [{"when": "STREAM", "do": pieces}]}));
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let id = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "STREAM for a while"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let began = Instant::now();
    while messages(user, &id).len() < 20 {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "the session never streamed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    id
}

/// The `agent.message` events of the agent, every page of them.
fn messages(user: &User, agent: &str) -> Vec<Value> {
    let mut all = Vec::new();
    let mut after = 0;
    loop {
        let page = user
            .call("agent.show", json!({"agent": agent, "after": after}))
            .unwrap();
        let events = page["events"].as_array().unwrap();
        after = events.last().map_or(after, |e| e["seq"].as_u64().unwrap());
        all.extend(
            events
                .iter()
                .filter(|e| e["name"] == "agent.message")
                .cloned(),
        );
        if page["more"] != true {
            return all;
        }
    }
}

fn working(user: &User, agent: &str) -> bool {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"] == "working"
}

/// How long one request takes to be answered, over a connection of its own as a client's: the
/// socket alone, nothing else the test's `call` does first.
fn answered_in(user: &User, method: &str) -> Duration {
    let began = Instant::now();
    let mut stream = UnixStream::connect(user.path("runtime/flottille/daemon.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        json!({"id": "1", "method": method, "params": {}})
    )
    .unwrap();
    let line = common::read_line_patiently(stream);
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert!(answer.get("error").is_none(), "{method}: {answer}");
    began.elapsed()
}

/// The value found at `at` (from 0 to 1) of `values`, sorted.
fn part<T: Copy + Ord>(values: &mut [T], at: f64) -> T {
    values.sort();
    values[((values.len() - 1) as f64 * at).round() as usize]
}

/// The machine's load, said with each measure.
fn load() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[test]
#[ignore = "needs a busy disk: run with the dd loop, docs/reference/daemon.md"]
fn agent_list_is_answered_while_another_session_streams() {
    let user = User::with_fake_agent();
    let streamer = streaming(&user);
    let mut lists = Vec::new();
    let mut statuses = Vec::new();
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(8) {
        lists.push(answered_in(&user, "agent.list"));
        statuses.push(answered_in(&user, "daemon.status"));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        working(&user, &streamer),
        "the session still streamed meanwhile"
    );
    let most_lists = part(&mut lists, 0.95);
    let most_statuses = part(&mut statuses, 0.95);
    eprintln!(
        "stream-load: agent.list {} requests, median {:?}, p95 {:?}, slowest {:?}; \
         daemon.status median {:?}, p95 {:?}, slowest {:?}; load {}",
        lists.len(),
        part(&mut lists, 0.5),
        most_lists,
        part(&mut lists, 1.0),
        part(&mut statuses, 0.5),
        most_statuses,
        part(&mut statuses, 1.0),
        load(),
    );
    assert!(
        most_lists < ANSWERED_WITHIN,
        "agent.list waited {most_lists:?} at the 95th percentile"
    );
    assert!(
        most_statuses < ANSWERED_WITHIN,
        "daemon.status waited {most_statuses:?} at the 95th percentile"
    );
}

#[test]
#[ignore = "needs a busy disk: run with the dd loop, docs/reference/daemon.md"]
fn a_streamed_piece_reaches_the_store_while_the_session_streams() {
    let user = User::with_fake_agent();
    let streamer = streaming(&user);
    std::thread::sleep(Duration::from_secs(8));
    assert!(
        working(&user, &streamer),
        "the session still streamed meanwhile"
    );
    // Each piece says when the agent sent it; its event, when the daemon wrote it. A piece the
    // daemon merged with the next one counts once for each clock it holds.
    let mut taken: Vec<u64> = messages(&user, &streamer)
        .iter()
        .flat_map(|event| {
            let at = event["at"].as_u64().unwrap();
            event["data"]["text"]
                .as_str()
                .unwrap()
                .split_whitespace()
                .filter_map(|sent| sent.parse::<u64>().ok())
                .map(move |sent| at.saturating_sub(sent))
                .collect::<Vec<_>>()
        })
        .collect();
    let pieces = taken.len();
    assert!(pieces > 100, "only {pieces} pieces were stored");
    let last = *taken.last().unwrap();
    let slowest = part(&mut taken, 1.0);
    let most = part(&mut taken, 0.95);
    eprintln!(
        "stream-load: {pieces} pieces stored, from the agent to the store median {} ms, p95 {} ms, \
         slowest {slowest} ms, the last {last} ms; load {}",
        part(&mut taken, 0.5),
        most,
        load(),
    );
    assert!(
        most < STORED_WITHIN_MS,
        "a piece took {most} ms to the store, at the 95th percentile"
    );
}
