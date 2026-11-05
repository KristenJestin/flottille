//! The scenarios of the services of the machine (`docs/projects.md`, "Services"): one instance on
//! the machine, started on demand by the user, an agent or a task that needs it, waited for until
//! it is ready, refused naming the service that failed, stopped by the user or with the daemon,
//! and an agent's stop held for the user every time. The services are the fake development
//! server, run in temporary folders of a temporary home; the agents are the fake agent's sessions.

mod common;

use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A folder of the home a service runs in.
fn folder(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("home/tools/{name}"));
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

/// The fake server's line: waits `wait` ms, then listens on a free port and says so.
fn server(wait: u64) -> String {
    format!(
        "{} --wait {wait} --listen 0",
        common::fake_server().display()
    )
}

/// `services.toml`, as the user writes it.
fn services_file(user: &User, text: &str) {
    std::fs::write(user.path("config/flottille/services.toml"), text).unwrap();
}

/// A service that listens once started and is ready when it says so.
fn listening(user: &User, name: &str, wait: u64, needs: &[&str]) -> String {
    format!(
        "[services.{name}]\nfolder = \"{}\"\nline = \"{}\"\nneeds = {}\nready = {{ line = \"listening on\" }}\n",
        folder(user, name).display(),
        server(wait),
        json!(needs)
    )
}

fn show(user: &User, name: &str) -> Value {
    user.call("service.show", json!({"name": name})).unwrap()
}

/// The service runs the daemon runs now, by service.
fn service_runs(user: &User, name: &str) -> usize {
    Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .running_runs()
        .unwrap()
        .into_iter()
        .filter(|run| run.project.is_empty() && run.name.as_deref() == Some(name))
        .count()
}

/// The address a listening service printed.
fn address(user: &User, name: &str) -> String {
    let output = user.call("service.output", json!({"name": name})).unwrap();
    let text = output["output"].as_str().unwrap();
    text.lines()
        .find_map(|line| line.strip_prefix("listening on "))
        .unwrap_or_else(|| panic!("no address in {text:?}"))
        .to_owned()
}

fn answers(address: &str) -> bool {
    let host = address.trim_start_matches("http://");
    TcpStream::connect(host).is_ok()
}

/// A project `app` over a throwaway repository, its task `api` needing `needs`.
fn project(user: &User, needs: &[&str]) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "task.add",
        json!({"project": "app", "name": "api", "line": "git status --short", "needs": needs}),
    )
    .unwrap();
    folder
}

/// A root agent the user started, in the project folder or a workspace: its id and MCP session.
fn root(user: &User, workspace: Option<&str>) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": workspace, "message": "hello"}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "an error: {}", answer.text);
    let rest = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    rest.split('.').next().unwrap().parse().unwrap()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

fn request(user: &User, number: u64) -> Value {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap_or_else(|| panic!("no pending request #{number}"))
}

fn approve(user: &User, number: u64) {
    let output = user.run(&["approve", &number.to_string()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_service_with_a_bad_key_is_refused_naming_its_file_and_its_key() {
    let user = User::new();
    services_file(&user, "[services.db]\nlines = \"redis-server\"\n");

    let (code, message) = user.call("service.list", json!({})).unwrap_err();

    assert_eq!(code, "invalid-config");
    assert!(
        message.starts_with("services.toml: services.db"),
        "{message}"
    );
    assert!(message.contains("lines"), "{message}");
}

#[test]
fn starting_a_running_service_starts_nothing() {
    let user = User::new();
    services_file(&user, &listening(&user, "db", 0, &[]));

    let first = user.call("service.start", json!({"name": "db"})).unwrap();
    assert_eq!(first["started"], json!(["db"]), "{first}");
    let second = user.call("service.start", json!({"name": "db"})).unwrap();

    assert_eq!(second["started"], json!([]), "{second}");
    assert!(
        ["starting", "ready"].contains(&second["service"]["state"].as_str().unwrap()),
        "{second}"
    );
    assert_eq!(service_runs(&user, "db"), 1);
}

#[test]
fn a_service_already_answering_is_running_outside_flottille_and_is_not_started() {
    let user = User::new();
    // Started by hand, before Flottille.
    let mut by_hand = Command::new(common::fake_server())
        .args(["--listen", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    BufReader::new(by_hand.stdout.take().unwrap())
        .read_line(&mut said)
        .unwrap();
    let address = said
        .trim()
        .strip_prefix("listening on ")
        .unwrap()
        .to_owned();
    services_file(
        &user,
        &format!(
            "[services.db]\nfolder = \"{}\"\nline = \"{}\"\nready = {{ url = \"{address}\" }}\n",
            folder(&user, "db").display(),
            server(0)
        ),
    );

    let started = user.call("service.start", json!({"name": "db"})).unwrap();
    let _ = by_hand.kill();
    let _ = by_hand.wait();

    assert_eq!(started["started"], json!([]), "{started}");
    assert_eq!(started["service"]["state"], "outside", "{started}");
    assert_eq!(service_runs(&user, "db"), 0);
}

#[test]
fn a_task_that_needs_a_service_starts_it_and_waits_until_it_is_ready() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 1500, &[]));
    project(&user, &["db"]);
    let (agent, mcp) = root(&user, None);
    set_mode(&user, &agent, "full");

    let ran = done(mcp.call("run", json!({"task": "api"})));

    assert!(
        ran.starts_with("started the services it needs: db\n"),
        "{ran}"
    );
    assert_eq!(show(&user, "db")["state"], "ready");
    // The user's Run of it finds it ready, and starts nothing.
    user.call("task.run", json!({"project": "app", "name": "api"}))
        .unwrap();
    assert_eq!(service_runs(&user, "db"), 1);
}

#[test]
fn a_task_whose_service_fails_is_refused_naming_it() {
    let user = User::with_fake_agent();
    services_file(
        &user,
        &format!(
            "[services.db]\nfolder = \"{}\"\nline = \"{} --print 'port 27017 is already in use' --exit 1\"\nready = {{ line = \"listening on\" }}\n",
            folder(&user, "db").display(),
            common::fake_server().display()
        ),
    );
    project(&user, &["db"]);

    let (code, message) = user
        .call("task.run", json!({"project": "app", "name": "api"}))
        .unwrap_err();

    assert_eq!(code, "refused");
    assert!(message.contains("the service db failed"), "{message}");
    assert!(
        message.contains("port 27017 is already in use"),
        "{message}"
    );
    assert_eq!(show(&user, "db")["state"], "failed");
}

#[test]
fn two_workspaces_running_a_task_that_needs_a_service_share_one_instance() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 1500, &[]));
    project(&user, &["db"]);
    let mut sessions = Vec::new();
    for name in ["one", "two"] {
        user.call("workspace.create", json!({"project": "app", "name": name}))
            .unwrap();
        eventually("the workspace", || {
            user.call("workspace.show", json!({"project": "app", "name": name}))
                .unwrap()["state"]
                == "ready"
        });
        sessions.push(root(&user, Some(name)).1);
    }

    // At the same moment, from each workspace.
    let ran: Vec<Answer> = std::thread::scope(|scope| {
        let calls: Vec<_> = sessions
            .iter()
            .map(|mcp| scope.spawn(move || mcp.call("run", json!({"task": "api"}))))
            .collect();
        calls.into_iter().map(|call| call.join().unwrap()).collect()
    });

    let said: Vec<bool> = ran
        .into_iter()
        .map(|answer| done(answer).starts_with("started the services it needs: db\n"))
        .collect();
    assert_eq!(
        said.iter().filter(|started| **started).count(),
        1,
        "one start, told once: {said:?}"
    );
    assert_eq!(service_runs(&user, "db"), 1);
    assert_eq!(show(&user, "db")["state"], "ready");
}

#[test]
fn starting_a_service_starts_what_it_needs_first() {
    let user = User::new();
    services_file(
        &user,
        &format!(
            "{}{}",
            listening(&user, "mongo", 1000, &[]),
            listening(&user, "auth", 0, &["mongo"])
        ),
    );
    let watch = user.watch(&[]);

    let started = user.call("service.start", json!({"name": "auth"})).unwrap();

    assert_eq!(started["started"], json!(["mongo", "auth"]), "{started}");
    let events = watch.until(|e| e["name"] == "service.started" && e["data"]["service"] == "auth");
    let ready = events
        .iter()
        .position(|e| e["name"] == "service.ready" && e["data"]["service"] == "mongo");
    let auth = events.len() - 1;
    assert!(ready.is_some_and(|ready| ready < auth), "{events:?}");
}

#[test]
fn a_task_s_needs_are_followed_through_its_services_needs() {
    let user = User::with_fake_agent();
    services_file(
        &user,
        &format!(
            "{}{}",
            listening(&user, "mongo", 0, &[]),
            listening(&user, "auth", 0, &["mongo"])
        ),
    );
    project(&user, &["auth"]);

    user.call("task.run", json!({"project": "app", "name": "api"}))
        .unwrap();

    assert_eq!(show(&user, "mongo")["state"], "ready");
    assert_eq!(show(&user, "auth")["state"], "ready");
}

#[test]
fn an_agent_lists_the_services_with_their_state() {
    let user = User::with_fake_agent();
    services_file(
        &user,
        &format!(
            "{}{}",
            listening(&user, "mongo", 0, &[]),
            listening(&user, "auth", 0, &["mongo"])
        ),
    );
    project(&user, &[]);
    user.call("service.start", json!({"name": "mongo"}))
        .unwrap();
    eventually("mongo ready", || show(&user, "mongo")["state"] == "ready");
    let (_, mcp) = root(&user, None);

    let listed = done(mcp.call("services", json!({"action": "list"})));

    let mongo = format!("- mongo: ready in {}", folder(&user, "mongo").display());
    assert!(listed.contains(&mongo), "{listed}");
    let auth = format!("- auth: stopped in {}", folder(&user, "auth").display());
    assert!(listed.contains(&auth), "{listed}");
    let shown = done(mcp.call("services", json!({"action": "show", "name": "mongo"})));
    assert!(shown.contains("listening on http://127.0.0.1:"), "{shown}");
}

#[test]
fn an_agent_s_stop_of_a_service_is_held_for_the_user_every_time_even_in_full() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &["db"]);
    let (agent, mcp) = root(&user, None);
    set_mode(&user, &agent, "full");
    user.call("service.start", json!({"name": "db"})).unwrap();

    let first = mcp.call("services", json!({"action": "stop", "name": "db"}));
    let number = held(&first);
    let asked = request(&user, number);
    assert_eq!(asked["class"], "base", "{asked}");
    let says = asked["says"].to_string();
    assert!(says.contains("Stop the service db"), "{says}");
    assert!(says.contains("app (api)"), "{says}");
    assert_ne!(show(&user, "db")["state"], "stopped");
    approve(&user, number);
    eventually("the stop", || show(&user, "db")["state"] == "stopped");

    user.call("service.start", json!({"name": "db"})).unwrap();
    let again = mcp.call("services", json!({"action": "stop", "name": "db"}));
    let number = held(&again);
    assert_ne!(show(&user, "db")["state"], "stopped");
    assert_eq!(request(&user, number)["class"], "base");
}

#[test]
fn an_agent_s_service_write_is_judged_as_a_task_write() {
    let user = User::with_fake_agent();
    project(&user, &[]);
    let file =
        || std::fs::read_to_string(user.path("config/flottille/services.toml")).unwrap_or_default();
    let (agent, mcp) = root(&user, None);
    let line = server(0);
    let db = folder(&user, "db");

    let number = held(&mcp.call(
        "services",
        json!({"action": "add", "name": "db", "line": line, "folder": db}),
    ));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "action", "{asked}");
    assert!(
        asked["says"].to_string().contains("Add the service db"),
        "{asked}"
    );
    assert!(!file().contains("[services.db]"));
    approve(&user, number);
    eventually("the write", || file().contains("[services.db]"));

    // In Full, a write passes as a task's does.
    set_mode(&user, &agent, "full");
    done(mcp.call(
        "services",
        json!({"action": "edit", "name": "db", "ready": {"line": "listening on"}}),
    ));
    assert!(
        file().contains("ready = { line = \"listening on\" }"),
        "{}",
        file()
    );
}

#[test]
fn a_child_s_service_write_is_refused() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &[]);
    let (_, lead) = root(&user, None);
    let opened = user.received_requests("session/new").len();
    let launched = lead.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);

    let listed = done(child.call("services", json!({"action": "list"})));
    assert!(listed.contains("- db: stopped"), "{listed}");
    let write = child.call("services", json!({"action": "remove", "name": "db"}));

    assert!(write.error, "not refused: {}", write.text);
    assert!(write.text.contains("ask_parent"), "{}", write.text);
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn services_stop_with_the_daemon() {
    let user = User::new();
    services_file(&user, &listening(&user, "db", 0, &[]));
    user.call("service.start", json!({"name": "db"})).unwrap();
    eventually("db ready", || show(&user, "db")["state"] == "ready");
    let address = address(&user, "db");
    assert!(answers(&address));

    assert!(user.run(&["daemon", "stop"]).status.success());

    eventually("db stopped", || !answers(&address));
}

#[test]
fn the_cli_lists_starts_and_stops_a_service() {
    let user = User::new();
    let db = folder(&user, "db");
    let line = server(0);
    let ran = |args: &[&str]| {
        let output = user.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    ran(&[
        "service",
        "add",
        "db",
        "--line",
        &line,
        "--folder",
        db.to_str().unwrap(),
        "--ready-line",
        "listening on",
    ]);
    let listed = ran(&["service", "list"]);
    assert_eq!(listed, format!("db stopped ({})\n", db.display()));
    let started = ran(&["service", "start", "db"]);
    assert!(started.starts_with("Started db.\n"), "{started}");
    eventually("db ready", || show(&user, "db")["state"] == "ready");
    let listed: Value = serde_json::from_str(&ran(&["service", "list", "--json"])).unwrap();
    assert_eq!(listed["services"][0]["state"], "ready", "{listed}");
    let output = ran(&["service", "output", "db"]);
    assert!(
        output.contains("listening on http://127.0.0.1:"),
        "{output}"
    );

    let stopped = ran(&["service", "stop", "db"]);

    assert_eq!(stopped, format!("db stopped ({})\n", db.display()));
}
