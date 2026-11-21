//! The daemon at rest (#444): with its agents idle, no run and a client following its events (as
//! the window does), it wakes no more than its own timers need.

use crate::common;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use common::User;
use serde_json::json;

/// The daemon's own timers at rest: the socket's check every two seconds and the sweep every
/// five, about two wake-ups a second (measured: 15 to 18 in 10 s), with room. A wake-up is a
/// thread of the daemon going to sleep and being woken: its voluntary context switches. The
/// switches the scheduler forces on a running thread are not counted: a busy machine multiplies
/// them (a loaded runner counted 34 where the same commit counted 17) when the daemon does
/// nothing more.
const WAKE_UPS_PER_SECOND: f64 = 3.0;

/// How many times each thread of `pid`, by its id, went to sleep, and its name.
fn sleeps(pid: u64) -> HashMap<String, (String, u64)> {
    let mut threads = HashMap::new();
    for task in std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .flatten()
    {
        let Ok(status) = std::fs::read_to_string(task.path().join("status")) else {
            continue;
        };
        let name = status
            .lines()
            .find_map(|line| line.strip_prefix("Name:"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        let count = status
            .lines()
            .find_map(|line| line.strip_prefix("voluntary_ctxt_switches:"))
            .and_then(|count| count.trim().parse::<u64>().ok())
            .unwrap_or(0);
        threads.insert(
            task.file_name().to_string_lossy().into_owned(),
            (name, count),
        );
    }
    threads
}

/// The wake-ups between two counts, by thread name: a thread that ended in between is not
/// counted.
fn woken(
    before: &HashMap<String, (String, u64)>,
    after: &HashMap<String, (String, u64)>,
) -> Vec<(String, u64)> {
    let mut woken: Vec<(String, u64)> = Vec::new();
    for (thread, (name, count)) in after {
        let count = count - before.get(thread).map_or(0, |(_, count)| *count);
        match woken.iter_mut().find(|(seen, _)| seen == name) {
            Some((_, total)) => *total += count,
            None => woken.push((name.clone(), count)),
        }
    }
    woken.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    woken
}

/// Asserts the daemon `pid` wakes no more than its own timers need over ten seconds, and says no
/// event to the client `watch` follows with.
fn at_rest(pid: u64, watch: &common::Watch) {
    let span = Duration::from_secs(10);
    let before = sleeps(pid);
    std::thread::sleep(span);
    let threads = woken(&before, &sleeps(pid));

    let woken: u64 = threads.iter().map(|(_, count)| count).sum();
    let rate = woken as f64 / span.as_secs_f64();
    assert!(
        rate <= WAKE_UPS_PER_SECOND,
        "the daemon woke {woken} times in {span:?} ({rate:.1} per second, at most \
         {WAKE_UPS_PER_SECOND}), by thread: {threads:?}"
    );
    let events: Vec<_> = watch
        .within(Duration::from_millis(100))
        .into_iter()
        .map(|event| event["name"].clone())
        .collect();
    assert!(events.is_empty(), "events at rest: {events:?}");
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

    at_rest(pid, &watch);
}

/// Routines between their runs (#1122): the daemon keeps one deadline, the next run due, which
/// the sweep already running reads; nothing of theirs wakes it.
#[test]
fn the_daemon_stays_at_rest_between_runs() {
    let user = User::with_fake_agent().with_fake_escalation();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    // None is due within the measure: their hour is three hours ahead.
    let hour = (jiff::Timestamp::now() + jiff::SignedDuration::from_hours(3))
        .to_zoned(jiff::tz::TimeZone::UTC)
        .strftime("%H:%M")
        .to_string();
    for (name, days) in [
        ("Tickets to review", "weekdays"),
        ("Nightly", "every day"),
        ("Weekly", "mondays"),
    ] {
        user.call(
            "routine.add",
            json!({"name": name, "project": "app", "prompt": "List what waits",
                "schedule": format!("{days} {hour}"), "zone": "UTC", "mode": "read_only"}),
        )
        .unwrap();
    }
    let pid = user.status()["pid"].as_u64().unwrap();
    let watch = user.watch(&[]);
    watch.within(Duration::from_secs(6));

    at_rest(pid, &watch);
}
