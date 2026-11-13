//! The scenarios of the daemon's skeleton, against the real binary, with temporary XDG folders.

mod common;

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::time::Duration;

use common::{User, mode};
use serde_json::json;

#[test]
fn the_first_command_starts_a_daemon() {
    let user = User::new();
    assert!(!user.path("runtime/flottille/daemon.sock").exists());

    user.status();

    assert!(user.path("runtime/flottille/daemon.sock").exists());
    assert!(user.path("state/flottille/flottille.db").exists());
}

#[test]
fn the_folders_are_created_for_the_user_only() {
    let user = User::new();
    user.status();

    assert_eq!(mode(user.path("runtime/flottille")), 0o700);
    assert_eq!(mode(user.path("state/flottille")), 0o700);
}

#[test]
fn a_folder_open_to_others_is_refused() {
    let user = User::new();
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(user.path("runtime/flottille"))
        .unwrap();
    std::fs::set_permissions(
        user.path("runtime/flottille"),
        PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let output = user.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("other users"),
        "unexpected refusal: {stderr}"
    );
}

#[test]
fn a_second_daemon_refuses_to_start() {
    let user = User::new();
    let pid = user.status()["pid"].as_u64().unwrap();

    let output = user.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already running"),
        "unexpected refusal: {stderr}"
    );
    assert!(
        stderr.contains(&pid.to_string()),
        "the refusal names the running daemon: {stderr}"
    );
    assert_eq!(
        user.status()["pid"].as_u64().unwrap(),
        pid,
        "the first daemon still answers"
    );
}

#[test]
fn status_json_answers() {
    let user = User::new();

    let status = user.status();

    assert_eq!(
        status["version"].as_str().unwrap(),
        version_of_the_binary(&user)
    );
    assert!(status["uptime_s"].is_u64());
    assert!(status["pid"].is_u64());
    assert_eq!(status["agents"]["running"], 0);
    assert_eq!(status["agents"]["waiting_for_user"], 0);
    assert_eq!(status["agents"]["failed"], 0);
}

#[test]
fn status_without_json_is_read_by_a_person() {
    let user = User::new();

    let output = user.run(&["status"]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 running, 0 waiting for you, 0 failed"),
        "unexpected status: {stdout}"
    );
}

#[test]
fn version_prints_the_full_version() {
    let user = User::new();

    let version = version_of_the_binary(&user);

    assert!(
        version.starts_with(env!("CARGO_PKG_VERSION")),
        "unexpected version: {version}"
    );
}

fn version_of_the_binary(user: &User) -> String {
    let output = user.run(&["--version"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout.trim().strip_prefix("flottille ").unwrap().to_owned()
}

#[test]
fn watch_prints_an_event_emitted_by_a_request_and_resumes_after_it() {
    let user = User::new();
    let watch = user.watch(&["--after", "0"]);
    let started = watch.next_event();
    assert_eq!(started["name"], "daemon.started");
    let started_seq = started["seq"].as_u64().unwrap();

    // `daemon stop` is a request; the daemon tells it before it goes.
    assert!(user.run(&["daemon", "stop"]).status.success());
    let stopping = watch.next_event();
    assert_eq!(stopping["name"], "daemon.stopping");
    assert_eq!(stopping["data"]["reason"], "request");
    assert!(stopping["seq"].as_u64().unwrap() > started_seq);
    drop(watch);

    // Resuming after the first event gives what came after it, from the store, then what is new.
    let resumed = user.watch(&["--after", &started_seq.to_string()]);
    assert_eq!(resumed.next_event(), stopping);
    let restarted = resumed.next_event();
    assert_eq!(restarted["name"], "daemon.started");
    assert!(restarted["seq"].as_u64().unwrap() > stopping["seq"].as_u64().unwrap());
}

#[test]
fn watch_without_after_prints_only_what_is_new() {
    let user = User::new();
    user.status();
    let watch = user.watch(&[]);
    // The subscription is answered before anything is printed; give it a moment to be in place.
    std::thread::sleep(Duration::from_secs(1));

    assert!(user.run(&["daemon", "stop"]).status.success());

    assert_eq!(watch.next_event()["name"], "daemon.stopping");
}

#[test]
fn a_stopped_daemon_is_restarted_by_the_next_command() {
    let user = User::new();
    let first = user.status()["pid"].as_u64().unwrap();

    let stop = user.run(&["daemon", "stop"]);
    assert!(stop.status.success());
    assert!(!user.path("runtime/flottille/daemon.sock").exists());

    let second = user.status()["pid"].as_u64().unwrap();
    assert_ne!(first, second);
}

#[test]
fn a_daemon_started_while_the_stop_waits_does_not_make_the_stop_fail() {
    let user = User::new();
    let first = user.status()["pid"].as_u64().unwrap();
    let asked = std::time::Instant::now();
    let stop = user
        .command(&["daemon", "stop"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // Another client (the window reconnecting) starts a daemon as soon as the stopped one lets go
    // of the lock: this test stands for it, taking the lock and writing a pid of its own.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(user.path("runtime/flottille/daemon.lock"))
        .unwrap();
    while lock.try_lock().is_err() {
        assert!(
            asked.elapsed() < Duration::from_secs(10),
            "the daemon never stopped"
        );
        std::thread::yield_now();
    }
    lock.set_len(0).unwrap();
    std::io::Write::write_all(&mut &lock, format!("{}\n", std::process::id()).as_bytes()).unwrap();

    let stopped = stop.wait_with_output().unwrap();
    assert!(
        stopped.status.success(),
        "daemon stop failed after {:?}: {}",
        asked.elapsed(),
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(!alive(first));
}

#[test]
fn stopping_when_no_daemon_runs_starts_none() {
    let user = User::new();

    let output = user.run(&["daemon", "stop"]);

    assert!(output.status.success());
    assert!(!user.path("runtime/flottille/daemon.sock").exists());
}

fn alive(pid: u64) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Waits up to `seconds` for `done`, polling.
fn within(seconds: u64, what: &str, done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "not within {seconds} s: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_dropped_user_leaves_no_daemon_and_no_folder_whatever_client_it_handed_out() {
    let user = User::new();
    let folder = user.path("");
    let pid = user.status()["pid"].as_u64().unwrap();
    // A client the test still holds after the user, as the window's link is.
    let connect = user.connector();
    connect().unwrap();

    drop(user);

    assert!(
        connect().is_err(),
        "a client of a dropped user started a daemon again"
    );
    // A daemon started again would have made its folders again.
    assert!(!folder.exists(), "the folder came back");
    within(10, "the daemon exits", || !alive(pid));
}

#[test]
fn a_daemon_whose_runtime_folder_was_removed_exits() {
    let user = User::new();
    let pid = user.status()["pid"].as_u64().unwrap();

    std::fs::remove_dir_all(user.path("runtime")).unwrap();

    within(10, "the daemon exits", || !alive(pid));
}

#[test]
fn a_daemon_whose_socket_leads_to_another_daemon_exits_and_leaves_that_one_alone() {
    let user = User::new();
    let first = user.status()["pid"].as_u64().unwrap();

    // The runtime folder is replaced under it, and a second daemon takes its place.
    std::fs::remove_dir_all(user.path("runtime")).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(user.path("runtime"))
        .unwrap();
    let second = user.status()["pid"].as_u64().unwrap();
    assert_ne!(first, second);

    within(10, "the first daemon exits", || !alive(first));
    assert!(alive(second));
    assert_eq!(user.status()["pid"].as_u64().unwrap(), second);
}

/// The permission bits of `path`, the sticky bit included.
fn bits(path: PathBuf) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn the_socket_and_the_lock_are_sticky_so_a_cleanup_of_the_runtime_folder_keeps_them() {
    let user = User::new();
    user.status();

    assert_eq!(bits(user.path("runtime/flottille/daemon.sock")), 0o1600);
    assert_eq!(bits(user.path("runtime/flottille/daemon.lock")), 0o1600);
}

#[test]
fn a_daemon_whose_socket_is_removed_records_why_and_stops_its_agents() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "hello", "do": [{"say": "Hello"}]}]}));
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let started = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    // From the run's own event, so that a turn ended already is not missed. Idle after it, the
    // agent's process stays, waiting for the next message.
    let watch = user.watch(&["--after", &started["after"].to_string()]);
    watch.until(|e| e["name"] == "agent.turn_ended" && e["data"]["agent"] == id.as_str());
    let daemon = user.status()["pid"].as_u64().unwrap();
    let agent = user
        .received()
        .iter()
        .find_map(|e| e["pid"].as_u64())
        .expect("the fake agent recorded its pid");
    assert!(alive(agent));

    std::fs::remove_file(user.path("runtime/flottille/daemon.sock")).unwrap();

    let stopping = watch.until(|e| e["name"] == "daemon.stopping");
    assert_eq!(stopping.last().unwrap()["data"]["reason"], "socket");
    within(20, "the daemon and its agent are gone", || {
        !alive(daemon) && !alive(agent)
    });
}

/// A project `app`, one git repository with one commit.
fn app(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// The pids of the fake agent's processes, in the order they started.
fn fake_starts(user: &User) -> Vec<u64> {
    user.received()
        .iter()
        .filter(|e| e["started"] == true)
        .filter_map(|e| e["pid"].as_u64())
        .collect()
}

/// An agent that never answers its session's opening: its daemon's task never sees the order to
/// stop, so the daemon stopping waits the whole grace for it, then kills what is left.
fn held_agent(user: &User) -> (String, u64) {
    user.fake_script(json!({"stall": true}));
    let held = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    within(10, "the held agent's process starts", || {
        !fake_starts(user).is_empty()
    });
    // The agents started from now on answer.
    user.fake_script(json!({"turns": [{"when": "hello", "do": [{"say": "Hello"}]}]}));
    (held, fake_starts(user)[0])
}

fn agent_events(user: &User, agent: &str) -> Vec<serde_json::Value> {
    user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_daemon_stopping_on_socket_while_a_second_starts_kills_none_of_its_agents_and_keeps_its_states()
{
    let user = User::with_fake_agent();
    app(&user);
    let first = user.status()["pid"].as_u64().unwrap();
    let (held, held_pid) = held_agent(&user);

    // The runtime folder is replaced (a logout), and a client starts a second daemon at once, on
    // the same store, while the first stops on its lost socket.
    std::fs::remove_dir_all(user.path("runtime")).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(user.path("runtime"))
        .unwrap();
    let second = user.status()["pid"].as_u64().unwrap();
    assert_ne!(first, second);
    let started = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let fresh = started["agent"]["id"].as_str().unwrap().to_owned();
    let watch = user.watch(&["--after", &started["after"].to_string()]);
    watch.until(|e| e["name"] == "agent.turn_ended" && e["data"]["agent"] == fresh.as_str());

    within(20, "the first daemon exits", || !alive(first));
    let seconds: Vec<u64> = fake_starts(&user)
        .into_iter()
        .filter(|pid| *pid != held_pid)
        .collect();
    assert!(!seconds.is_empty());
    for pid in seconds {
        assert!(
            alive(pid),
            "the second daemon's agent process {pid} was killed"
        );
    }
    assert_eq!(user.status()["pid"].as_u64().unwrap(), second);
    // The held agent was settled by the second daemon alone: the first one, stopping, wrote
    // nothing over it.
    let events = agent_events(&user, &held);
    let names: Vec<&str> = events.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "agent.released" && e["data"]["reason"] == "restart"),
        "{names:?}"
    );
    assert!(
        !names.contains(&"agent.died") && !names.contains(&"agent.failed"),
        "{names:?}"
    );
    for agent in [&held, &fresh] {
        let state =
            user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"].clone();
        assert_ne!(state, "failed", "agent {agent}");
    }
}

/// When the process started, in clock ticks after boot (`/proc/<pid>/stat`, field 22).
fn start_time(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let rest = stat.rsplit_once(')').unwrap().1;
    rest.split_whitespace().nth(19).unwrap().parse().unwrap()
}

#[test]
fn a_daemon_stopping_kills_only_the_processes_it_started_never_another_s_of_the_table() {
    let user = User::with_fake_agent();
    app(&user);
    let daemon = user.status()["pid"].as_u64().unwrap();
    let (held, held_pid) = held_agent(&user);
    // A process the table says is an agent's, which this daemon did not start.
    let mut other = std::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    flottille_core::store::Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .add_process(other.id(), start_time(other.id()), &held)
        .unwrap();

    user.run(&["daemon", "stop"]);

    within(20, "the daemon and its held agent are gone", || {
        !alive(daemon) && !alive(held_pid)
    });
    let survived = alive(other.id().into()) && other.try_wait().unwrap().is_none();
    let _ = other.kill();
    let _ = other.wait();
    assert!(survived, "the daemon killed a process it did not start");
}

/// Asks the daemon to hold its store's lock for `ms` on its runtime's thread, as a deadlock
/// would (`test.hold`), without waiting for the answer: the connection is kept open.
fn hold(user: &User, ms: u64) -> std::os::unix::net::UnixStream {
    ask(user, "test.hold", ms)
}

/// Sends `method` with `{"ms": ms}` without waiting for its answer.
fn ask(user: &User, method: &str, ms: u64) -> std::os::unix::net::UnixStream {
    let mut stream =
        std::os::unix::net::UnixStream::connect(user.path("runtime/flottille/daemon.sock"))
            .unwrap();
    let request = json!({"id": "hold", "method": method, "params": {"ms": ms}});
    std::io::Write::write_all(&mut stream, format!("{request}\n").as_bytes()).unwrap();
    stream
}

/// Whether the process ended: no longer there, or a zombie its new parent has not waited for yet.
fn gone(pid: u64) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z'))
    })
}

fn log_of(user: &User) -> String {
    std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap_or_default()
}

#[test]
fn a_stuck_daemon_is_reported_by_its_watchdog() {
    let user = User::new()
        .with_env("FLOTTILLE_TEST_HOLD", "1")
        .with_env("FLOTTILLE_WATCHDOG_MS", "500");
    let pid = user.status()["pid"].as_u64().unwrap();

    let _held = hold(&user, 3000);

    within(10, "the watchdog says the daemon answers again", || {
        log_of(&user).contains("watchdog: the daemon answers again")
    });
    let log = log_of(&user);
    let said: Vec<&str> = log.lines().filter(|l| l.contains("watchdog:")).collect();
    let line = |start: &str| {
        said.iter()
            .find(|l| l.contains(start))
            .unwrap_or_else(|| panic!("no line {start:?} in {said:#?}"))
            .to_string()
    };
    line("watchdog: the daemon is stuck: the runtime's thread has not run for");
    // The lock, the line that took it, the thread that holds it, and since when.
    let held = line("watchdog: held: store at ");
    assert!(held.contains("src/daemon.rs:"), "{held}");
    assert!(held.contains("by the thread main"), "{held}");
    assert!(held.contains(", for "), "{held}");
    line(&format!("`sudo eu-stack -p {pid}`"));
    // Once for the whole time it was stuck.
    assert_eq!(
        said.iter()
            .filter(|l| l.contains("the daemon is stuck"))
            .count(),
        1,
        "{said:#?}"
    );
    assert!(user.status()["pid"].as_u64() == Some(pid));
}

#[test]
fn a_request_that_waits_on_outside_work_is_not_said_stuck() {
    let user = User::new()
        .with_env("FLOTTILLE_TEST_HOLD", "1")
        .with_env("FLOTTILLE_WATCHDOG_MS", "500");
    user.status();

    let mut waiting = ask(&user, "test.wait", 2500);
    let mut answer = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(&mut waiting), &mut answer).unwrap();

    assert!(answer.contains(r#""id":"hold""#), "{answer}");
    let log = log_of(&user);
    assert!(!log.contains("the daemon is stuck"), "{log}");
}

#[test]
fn daemon_stop_ends_a_daemon_that_does_not_answer() {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_HOLD", "1")
        .with_env("FLOTTILLE_STOP_WAIT_MS", "1000");
    app(&user);
    let pid = user.status()["pid"].as_u64().unwrap();
    let (_, agent) = held_agent(&user);
    let _held = hold(&user, 600_000);

    let asked = std::time::Instant::now();
    let stop = user.run(&["daemon", "stop"]);

    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    assert!(
        asked.elapsed() < Duration::from_secs(15),
        "{:?}",
        asked.elapsed()
    );
    let said = String::from_utf8_lossy(&stop.stdout);
    assert!(
        said.contains("The daemon did not stop (it did not answer daemon.stop within 1 s): it was ended, with the"),
        "{said}"
    );
    // The daemon and what it ran: its agent's process with it.
    within(5, "the daemon is gone", || gone(pid));
    within(5, "its agent's process is gone", || gone(agent));
    assert!(
        !user.path("runtime/flottille/daemon.lock").exists() || {
            let lock = std::fs::File::open(user.path("runtime/flottille/daemon.lock")).unwrap();
            lock.try_lock().is_ok()
        }
    );
}

/// The first pid the fake agent recorded under `key`.
fn recorded(user: &User, key: &str) -> u64 {
    let pid = || user.received().iter().find_map(|e| e[key].as_u64());
    within(10, "the fake agent records its process", || pid().is_some());
    pid().unwrap()
}

#[test]
fn daemon_stop_spares_a_detached_helper() {
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_HOLD", "1")
        .with_env("FLOTTILLE_STOP_WAIT_MS", "1000");
    app(&user);
    let pid = user.status()["pid"].as_u64().unwrap();
    // An agent that starts a process in its group, and a helper that detaches in a session of
    // its own, as an ssh control master does.
    user.fake_script(json!({"stall": true, "child": true, "detached": true}));
    user.call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let child = recorded(&user, "child");
    let helper = recorded(&user, "detached");
    let _held = hold(&user, 600_000);

    let stop = user.run(&["daemon", "stop"]);

    let said = String::from_utf8_lossy(&stop.stdout);
    assert!(said.contains("The daemon did not stop"), "{said}");
    within(5, "the daemon is gone", || gone(pid));
    within(5, "the agent's child is gone", || gone(child));
    let spared = !gone(helper);
    // SAFETY: kill has no memory precondition; the helper is the test's own sleep.
    unsafe { libc::kill(helper as libc::pid_t, libc::SIGKILL) };
    assert!(spared, "daemon stop killed a helper that had detached");
}

#[test]
fn daemon_stop_waits_for_a_daemon_that_is_stopping() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_STOP_WAIT_MS", "1000");
    app(&user);
    let pid = user.status()["pid"].as_u64().unwrap();
    // Stuck while it opens its session: the daemon gives it four seconds to stop.
    let (_, agent) = held_agent(&user);

    let asked = std::time::Instant::now();
    let stop = user.run(&["daemon", "stop"]);

    let said = String::from_utf8_lossy(&stop.stdout);
    assert!(said.contains("The daemon stopped."), "{said}");
    assert!(
        asked.elapsed() > Duration::from_secs(2),
        "{:?}",
        asked.elapsed()
    );
    assert!(gone(pid));
    within(5, "its agent's process is gone", || gone(agent));
}
