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

/// A ready command that answers once `file` exists.
fn ready_once(file: &std::path::Path) -> String {
    format!("ready = {{ command = \"test -e {}\" }}\n", file.display())
}

/// A service that listens once started and is ready once `ok` exists.
fn ready_on(user: &User, name: &str, ok: &std::path::Path) -> String {
    format!(
        "[services.{name}]\nfolder = \"{}\"\nline = \"{}\"\n{}",
        folder(user, name).display(),
        server(0),
        ready_once(ok)
    )
}

/// The runs of the project's task, going on now.
fn task_runs(user: &User, name: &str) -> usize {
    Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .running_runs()
        .unwrap()
        .into_iter()
        .filter(|run| run.project == "app" && run.name.as_deref() == Some(name))
        .count()
}

/// A workspace `name` of `app`, ready.
fn workspace(user: &User, name: &str) -> PathBuf {
    user.call("workspace.create", json!({"project": "app", "name": name}))
        .unwrap();
    let show = || {
        user.call("workspace.show", json!({"project": "app", "name": name}))
            .unwrap()
    };
    eventually("the workspace", || show()["state"] != "preparing");
    let shown = show();
    assert_eq!(shown["state"], "ready", "{shown}");
    PathBuf::from(shown["folder"].as_str().unwrap())
}

#[test]
fn an_agent_s_start_of_a_service_outside_its_place_is_held_in_ask() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &[]);
    let (agent, mcp) = root(&user, None);
    set_mode(&user, &agent, "ask");

    let number = held(&mcp.call("services", json!({"action": "start", "name": "db"})));

    let asked = request(&user, number).to_string();
    assert!(asked.contains("outside this agent's place"), "{asked}");
    assert_eq!(show(&user, "db")["state"], "stopped");
}

#[test]
fn an_agent_s_start_of_a_service_in_a_login_s_place_is_refused() {
    let user = User::with_fake_agent();
    let ssh = user.path("home/.ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    services_file(
        &user,
        &format!(
            "[services.db]\nfolder = \"{}\"\nline = \"{}\"\n",
            ssh.display(),
            server(0)
        ),
    );
    project(&user, &[]);
    let (agent, mcp) = root(&user, None);
    set_mode(&user, &agent, "full");

    let answer = mcp.call("services", json!({"action": "start", "name": "db"}));

    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.contains(".ssh"), "{}", answer.text);
    assert_eq!(service_runs(&user, "db"), 0);
}

#[test]
fn an_agent_s_start_of_a_service_marked_to_ask_is_held_even_when_its_line_alone_would_pass() {
    let user = User::with_fake_agent();
    let app = project(&user, &[]);
    let service = |name: &str, ask: bool| {
        format!(
            "[services.{name}]\nfolder = \"{}\"\nline = \"git status --short\"\nask_before_running = {ask}\n",
            app.display()
        )
    };
    services_file(
        &user,
        &format!("{}{}", service("plain", false), service("asked", true)),
    );
    let (agent, mcp) = root(&user, None);
    set_mode(&user, &agent, "ask");

    done(mcp.call("services", json!({"action": "start", "name": "plain"})));
    let number = held(&mcp.call("services", json!({"action": "start", "name": "asked"})));

    let asked = request(&user, number).to_string();
    assert!(
        asked.contains("the service asked is marked to ask before it runs"),
        "{asked}"
    );
    assert!(!asked.contains("project folder (main)"), "{asked}");
}

#[test]
fn a_need_marked_to_ask_holds_an_agent_s_run_of_the_task() {
    let user = User::with_fake_agent();
    services_file(
        &user,
        &format!(
            "{}{}ask_before_running = true\n",
            listening(&user, "cache", 0, &[]),
            listening(&user, "mongo", 0, &[])
        ),
    );
    project(&user, &["mongo"]);
    user.call(
        "task.add",
        json!({"project": "app", "name": "web", "line": "git status --short", "needs": ["cache"]}),
    )
    .unwrap();
    workspace(&user, "one");
    let (agent, mcp) = root(&user, Some("one"));
    set_mode(&user, &agent, "ask");

    let ran = done(mcp.call("run", json!({"task": "web"})));
    assert!(
        ran.starts_with("started the services it needs: cache\n"),
        "{ran}"
    );
    let number = held(&mcp.call("run", json!({"task": "api"})));

    let asked = request(&user, number).to_string();
    assert!(
        asked.contains("the service mongo is marked to ask before it runs"),
        "{asked}"
    );
    assert_eq!(show(&user, "mongo")["state"], "stopped");
}

#[test]
fn a_need_marked_to_ask_holds_an_agent_s_start_of_the_service_that_needs_it() {
    let user = User::with_fake_agent();
    project(&user, &[]);
    let one = workspace(&user, "one");
    services_file(
        &user,
        &format!(
            "{}ask_before_running = true\n[services.auth]\nfolder = \"{}\"\nline = \"git status --short\"\nneeds = [\"mongo\"]\n",
            listening(&user, "mongo", 0, &[]),
            one.display()
        ),
    );
    let (agent, mcp) = root(&user, Some("one"));
    set_mode(&user, &agent, "ask");

    let number = held(&mcp.call("services", json!({"action": "start", "name": "auth"})));

    let asked = request(&user, number).to_string();
    assert!(
        asked.contains("the service mongo is marked to ask before it runs"),
        "{asked}"
    );
    assert_eq!(show(&user, "mongo")["state"], "stopped");
}

#[test]
fn a_service_an_agent_wrote_started_as_a_need_by_an_ask_agent_is_held() {
    let user = User::with_fake_agent();
    project(&user, &[]);
    workspace(&user, "one");
    let (agent, mcp) = root(&user, Some("one"));
    set_mode(&user, &agent, "full");
    let mongo = folder(&user, "mongo");
    done(mcp.call(
        "services",
        json!({"action": "add", "name": "mongo", "line": server(0), "folder": mongo}),
    ));
    user.call(
        "task.edit",
        json!({"project": "app", "name": "api", "needs": ["mongo"]}),
    )
    .unwrap();
    set_mode(&user, &agent, "ask");

    let number = held(&mcp.call("run", json!({"task": "api"})));

    let asked = request(&user, number).to_string();
    assert!(asked.contains("the service mongo"), "{asked}");
    assert!(asked.contains("outside this agent's place"), "{asked}");
    assert_eq!(show(&user, "mongo")["state"], "stopped");
}

#[test]
fn a_service_the_user_wrote_started_as_a_need_is_not_judged_again() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "mongo", 0, &[]));
    project(&user, &["mongo"]);
    workspace(&user, "one");
    let (agent, mcp) = root(&user, Some("one"));
    set_mode(&user, &agent, "ask");

    let ran = done(mcp.call("run", json!({"task": "api"})));

    assert!(
        ran.starts_with("started the services it needs: mongo\n"),
        "{ran}"
    );
}

#[test]
fn a_run_stopped_while_its_services_start_starts_nothing_after_them() {
    let user = User::with_fake_agent();
    let ok = user.path("db-ok");
    services_file(&user, &ready_on(&user, "db", &ok));
    project(&user, &[]);
    user.call(
        "task.edit",
        json!({"project": "app", "name": "api", "line": server(0), "needs": ["db"]}),
    )
    .unwrap();
    let (agent, mcp) = root(&user, None);
    set_mode(&user, &agent, "full");
    let watch = user.watch(&[]);

    mcp.call_given_up("run", json!({"task": "api"}), || {
        show(&user, "db")["state"] == "starting"
    });
    std::fs::write(&ok, "").unwrap();
    eventually("db ready", || show(&user, "db")["state"] == "ready");

    let events = watch.within(Duration::from_millis(800));
    let api = events
        .iter()
        .find(|e| e["name"] == "run.started" && e["data"]["name"] == "api");
    assert!(
        api.is_none(),
        "the task ran after its call was stopped: {api:?}"
    );
}

#[test]
fn two_runs_of_a_serving_task_while_its_services_start_start_it_once() {
    let user = User::with_fake_agent();
    let ok = user.path("db-ok");
    services_file(&user, &ready_on(&user, "db", &ok));
    project(&user, &[]);
    user.call(
        "task.edit",
        json!({"project": "app", "name": "api", "line": server(0), "serve": true, "needs": ["db"]}),
    )
    .unwrap();

    let ran: Vec<Value> = std::thread::scope(|scope| {
        let runs: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    user.call("task.run", json!({"project": "app", "name": "api"}))
                        .unwrap()
                })
            })
            .collect();
        eventually("db starting", || show(&user, "db")["state"] == "starting");
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&ok, "").unwrap();
        runs.into_iter().map(|run| run.join().unwrap()).collect()
    });

    assert_eq!(ran[0], ran[1], "one run, joined by the other");
    assert_eq!(task_runs(&user, "api"), 1);
}

/// The fake server, started by hand outside Flottille: its process and its address.
fn by_hand() -> (std::process::Child, String) {
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
    (by_hand, address)
}

#[test]
fn a_service_that_ran_outside_and_stopped_is_shown_stopped() {
    let user = User::new();
    let (mut by_hand, address) = by_hand();
    services_file(
        &user,
        &format!(
            "[services.db]\nfolder = \"{}\"\nline = \"{}\"\nready = {{ url = \"{address}\" }}\n",
            folder(&user, "db").display(),
            server(0)
        ),
    );
    let started = user.call("service.start", json!({"name": "db"})).unwrap();
    assert_eq!(started["service"]["state"], "outside", "{started}");

    let _ = by_hand.kill();
    let _ = by_hand.wait();

    assert_eq!(show(&user, "db")["state"], "stopped");
    let listed = user.call("service.list", json!({})).unwrap();
    assert_eq!(listed["services"][0]["state"], "stopped", "{listed}");
}

#[test]
fn two_starts_at_once_start_one_instance_since_the_second_waits_for_the_first() {
    let user = User::new();
    let marks = user.path("marks");
    std::fs::create_dir_all(&marks).unwrap();
    // Its first probe holds until the test releases it; it never answers.
    let probe = marks.join("probe.sh");
    std::fs::write(
        &probe,
        format!(
            "if [ ! -e {m}/probing ]; then\n  touch {m}/probing\n  while [ ! -e {m}/release ]; do sleep 0.05; done\nfi\nexit 1\n",
            m = marks.display()
        ),
    )
    .unwrap();
    let serve = marks.join("serve.sh");
    std::fs::write(
        &serve,
        format!(
            "echo started >> {}/starts.txt\nexec {} --listen 0\n",
            marks.display(),
            common::fake_server().display()
        ),
    )
    .unwrap();
    services_file(
        &user,
        &format!(
            "[services.db]\nfolder = \"{}\"\nline = \"sh {}\"\nready = {{ command = \"sh {}\" }}\n",
            folder(&user, "db").display(),
            serve.display(),
            probe.display()
        ),
    );

    std::thread::scope(|scope| {
        let first = scope.spawn(|| user.call("service.start", json!({"name": "db"})).unwrap());
        eventually("the first probe", || marks.join("probing").exists());
        let second = scope.spawn(|| user.call("service.start", json!({"name": "db"})).unwrap());
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(marks.join("release"), "").unwrap();
        first.join().unwrap();
        second.join().unwrap();
    });

    eventually("the start", || marks.join("starts.txt").exists());
    std::thread::sleep(Duration::from_millis(300));
    let starts = std::fs::read_to_string(marks.join("starts.txt")).unwrap();
    assert_eq!(starts.lines().count(), 1, "{starts}");
}

#[test]
fn a_setup_step_running_a_task_starts_the_services_it_needs() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &["db"]);
    user.call(
        "setup.add",
        json!({"project": "app", "step": {"task": "api"}}),
    )
    .unwrap();

    workspace(&user, "one");

    assert_eq!(show(&user, "db")["state"], "ready");
}

#[test]
fn a_setup_step_whose_service_fails_fails_naming_it() {
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
    user.call(
        "setup.add",
        json!({"project": "app", "step": {"task": "api"}}),
    )
    .unwrap();

    user.call("workspace.create", json!({"project": "app", "name": "one"}))
        .unwrap();
    let show_one = || {
        user.call("workspace.show", json!({"project": "app", "name": "one"}))
            .unwrap()
    };
    eventually("the setup's end", || show_one()["state"] != "preparing");

    let shown = show_one();
    assert_eq!(shown["state"], "failed", "{shown}");
    let said = shown.to_string();
    assert!(said.contains("the service db failed"), "{said}");
    assert!(said.contains("port 27017 is already in use"), "{said}");
}

#[test]
fn a_restart_of_a_task_starts_the_services_it_needs() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &[]);
    user.call(
        "task.edit",
        json!({"project": "app", "name": "api", "line": server(0), "serve": true, "needs": ["db"]}),
    )
    .unwrap();
    let run = user
        .call("task.run", json!({"project": "app", "name": "api"}))
        .unwrap();
    user.call("service.stop", json!({"name": "db"})).unwrap();
    eventually("db stopped", || show(&user, "db")["state"] == "stopped");

    user.call("run.restart", json!({"run": run["run"]}))
        .unwrap();

    assert_eq!(show(&user, "db")["state"], "ready");
}

#[test]
fn a_held_run_says_the_services_it_starts() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &["db"]);
    let (agent, mcp) = root(&user, None);
    set_mode(&user, &agent, "ask");

    let number = held(&mcp.call("run", json!({"task": "api"})));

    let says = request(&user, number)["says"].to_string();
    assert!(says.contains("the service db"), "{says}");
    assert!(says.contains(&server(0)), "{says}");
    assert!(
        says.contains(&folder(&user, "db").display().to_string()),
        "{says}"
    );
}

#[test]
fn an_agent_s_stop_of_a_service_that_does_not_run_is_refused_at_once() {
    let user = User::with_fake_agent();
    services_file(&user, &listening(&user, "db", 0, &[]));
    project(&user, &[]);
    let (_, mcp) = root(&user, None);

    let answer = mcp.call("services", json!({"action": "stop", "name": "db"}));

    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.contains("not running"), "{}", answer.text);
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_service_an_agent_wrote_seen_outside_then_stopped_is_judged_when_an_ask_agent_s_run_needs_it() {
    let user = User::with_fake_agent();
    project(&user, &[]);
    workspace(&user, "one");
    let (agent, mcp) = root(&user, Some("one"));
    set_mode(&user, &agent, "full");
    let (mut by_hand, address) = by_hand();
    let mongo = folder(&user, "mongo");
    done(mcp.call(
        "services",
        json!({"action": "add", "name": "mongo", "line": server(0), "folder": mongo,
            "ready": {"url": address}}),
    ));
    done(mcp.call("services", json!({"action": "start", "name": "mongo"})));
    assert_eq!(show(&user, "mongo")["state"], "outside");
    user.call(
        "task.edit",
        json!({"project": "app", "name": "api", "needs": ["mongo"]}),
    )
    .unwrap();
    set_mode(&user, &agent, "ask");
    let _ = by_hand.kill();
    let _ = by_hand.wait();

    let number = held(&mcp.call("run", json!({"task": "api"})));

    let asked = request(&user, number).to_string();
    assert!(asked.contains("the service mongo"), "{asked}");
    assert!(asked.contains("outside this agent's place"), "{asked}");
    assert_eq!(service_runs(&user, "mongo"), 0);
}

#[test]
fn a_service_that_stops_between_the_judgement_of_an_agent_s_run_and_its_start_is_refused() {
    let user = User::with_fake_agent();
    let ok = user.path("slow-ok");
    services_file(
        &user,
        &format!(
            "{}{}",
            listening(&user, "db", 0, &[]),
            ready_on(&user, "slow", &ok)
        ),
    );
    project(&user, &[]);
    user.call(
        "task.edit",
        json!({"project": "app", "name": "api", "line": server(0), "serve": true,
            "needs": ["db", "slow"]}),
    )
    .unwrap();
    user.call("service.start", json!({"name": "db"})).unwrap();
    let (agent, mcp) = root(&user, None);
    set_mode(&user, &agent, "full");

    let (first, second) = std::thread::scope(|scope| {
        // The first holds the task's lock while slow starts; the second is judged with db and
        // slow running, then waits for it.
        let first = scope.spawn(|| mcp.call("run", json!({"task": "api"})));
        eventually("slow starting", || {
            show(&user, "slow")["state"] == "starting"
        });
        let second = scope.spawn(|| mcp.call("run", json!({"task": "api"})));
        std::thread::sleep(Duration::from_millis(500));
        user.call("service.stop", json!({"name": "db"})).unwrap();
        eventually("db stopped", || show(&user, "db")["state"] == "stopped");
        user.call("service.stop", json!({"name": "slow"})).unwrap();
        std::fs::write(&ok, "").unwrap();
        (first.join().unwrap(), second.join().unwrap())
    });

    assert!(first.error, "the first ran: {}", first.text);
    assert!(second.error, "the second ran: {}", second.text);
    assert!(
        second
            .text
            .contains("the service db stopped since the call was judged"),
        "{}",
        second.text
    );
    assert_eq!(service_runs(&user, "db"), 0);
}

#[test]
fn a_list_of_the_services_runs_none_of_their_commands() {
    let user = User::new();
    let marks = user.path("marks");
    std::fs::create_dir_all(&marks).unwrap();
    let marker = marks.join("probed");
    let probe = marks.join("probe.sh");
    std::fs::write(&probe, format!("touch {}\nexit 0\n", marker.display())).unwrap();
    services_file(
        &user,
        &format!(
            "[services.db]\nfolder = \"{}\"\nline = \"{}\"\nready = {{ command = \"sh {}\" }}\n",
            folder(&user, "db").display(),
            server(0),
            probe.display()
        ),
    );
    let started = user.call("service.start", json!({"name": "db"})).unwrap();
    assert_eq!(started["service"]["state"], "outside", "{started}");
    std::fs::remove_file(&marker).unwrap();

    user.call("service.list", json!({})).unwrap();
    show(&user, "db");

    assert!(!marker.exists(), "a list ran the service's ready command");
}

/// A root agent in `one` that wrote the service mongo in Full, which the task api needs and a
/// setup step runs.
fn agent_s_mongo_for_a_setup(user: &User) -> (String, Mcp) {
    project(user, &[]);
    workspace(user, "one");
    let (agent, mcp) = root(user, Some("one"));
    set_mode(user, &agent, "full");
    let mongo = folder(user, "mongo");
    done(mcp.call(
        "services",
        json!({"action": "add", "name": "mongo", "line": server(0), "folder": mongo}),
    ));
    user.call(
        "task.edit",
        json!({"project": "app", "name": "api", "needs": ["mongo"]}),
    )
    .unwrap();
    user.call(
        "setup.add",
        json!({"project": "app", "step": {"task": "api"}}),
    )
    .unwrap();
    (agent, mcp)
}

#[test]
fn a_setup_step_whose_task_needs_an_agent_s_service_with_a_refused_line_is_refused() {
    let user = User::with_fake_agent();
    let (_, mcp) = agent_s_mongo_for_a_setup(&user);
    let ssh = user.path("home/.ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    let file = user.path("config/flottille/services.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    let mongo = folder(&user, "mongo").display().to_string();
    std::fs::write(&file, text.replace(&mongo, &ssh.display().to_string())).unwrap();

    let answer = mcp.call("workspace", json!({"action": "create", "name": "two"}));

    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.contains(".ssh"), "{}", answer.text);
    assert!(answer.text.contains("the service mongo"), "{}", answer.text);
}

#[test]
fn a_setup_step_whose_task_needs_an_agent_s_service_is_held_naming_it_in_ask() {
    let user = User::with_fake_agent();
    let (agent, mcp) = agent_s_mongo_for_a_setup(&user);
    set_mode(&user, &agent, "ask");

    let number = held(&mcp.call("workspace", json!({"action": "create", "name": "two"})));

    let asked = request(&user, number);
    let reasons = asked["reasons"].to_string();
    assert!(reasons.contains("the service mongo"), "{asked}");
    assert!(reasons.contains("outside this agent's place"), "{asked}");
    let says = asked["says"].to_string();
    assert!(says.contains("the service mongo"), "{says}");
}
