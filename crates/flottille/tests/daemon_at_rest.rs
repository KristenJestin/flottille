//! The daemon at rest (#444): with its agents idle, no run and a client following its events (as
//! the window does), it wakes no more than its own timers need.

mod common;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use common::User;
use serde_json::json;

/// The daemon's own timers at rest: the socket's check every two seconds and the sweep every
/// five, about one wake-up a second (measured: 10 in 10 s), with room for a loaded machine. A
/// wake-up is a context switch of one of its threads.
const WAKE_UPS_PER_SECOND: f64 = 3.0;

/// How many times each thread of `pid` gave the processor back or had it taken: each is a
/// wake-up that followed.
fn switches(pid: u64) -> HashMap<String, u64> {
    let mut threads = HashMap::new();
    for task in std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .flatten()
    {
        let Ok(status) = std::fs::read_to_string(task.path().join("status")) else {
            continue;
        };
        let count = status
            .lines()
            .filter(|line| line.contains("ctxt_switches:"))
            .filter_map(|line| line.split_whitespace().last()?.parse::<u64>().ok())
            .sum::<u64>();
        threads.insert(task.file_name().to_string_lossy().into_owned(), count);
    }
    threads
}

/// The wake-ups between two counts: a thread that ended in between is not counted.
fn woken(before: &HashMap<String, u64>, after: &HashMap<String, u64>) -> u64 {
    after
        .iter()
        .map(|(thread, count)| count - before.get(thread).copied().unwrap_or(0))
        .sum()
}

#[test]
fn the_daemon_does_not_wake_at_rest() {
    let user = User::with_fake_agent().with_fake_escalation();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    for message in ["hello", "hello again"] {
        let answer = user
            .call("agent.start", json!({"project": "app", "message": message}))
            .unwrap();
        let id = answer["agent"]["id"].as_str().unwrap().to_owned();
        let deadline = Instant::now() + Duration::from_secs(20);
        while user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] != "idle" {
            assert!(Instant::now() < deadline, "the first turn never ended");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let pid = user.status()["pid"].as_u64().unwrap();
    // A client following the events, as the window does.
    let watch = user.watch(&[]);
    // What the starts left to finish (a title, a sweep) ends first.
    watch.within(Duration::from_secs(6));

    let span = Duration::from_secs(10);
    let before = switches(pid);
    std::thread::sleep(span);
    let woken = woken(&before, &switches(pid));

    let rate = woken as f64 / span.as_secs_f64();
    assert!(
        rate <= WAKE_UPS_PER_SECOND,
        "the daemon woke {woken} times in {span:?} ({rate:.1} per second, at most \
         {WAKE_UPS_PER_SECOND})"
    );
    let events: Vec<_> = watch
        .within(Duration::from_millis(100))
        .into_iter()
        .map(|event| event["name"].clone())
        .collect();
    assert!(events.is_empty(), "events at rest: {events:?}");
}
